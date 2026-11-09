// Live preview: the HTML backend in a sandboxed iframe (instant scrub and playback), double
// buffered so edits never flash, plus the exact native frame (POST /frame) when paused, which
// shows the effects the HTML backend does not draw.
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { api, ApiError } from '../api';
import { useLatest, useSize } from '../ui/hooks';
import { withBridge, type FromFrame } from './bridge';
import { t } from '../i18n';

export type ExactMode = 'auto' | 'always' | 'off';

interface Props {
  source: string | null;
  format: string;
  size?: string; // WxH override (stills, named sizes)
  baseSize: [number, number]; // expected pixel size before the frame reports it
  time: number;
  playing: boolean;
  onTime: (t: number) => void;
  onDuration?: (d: number) => void;
  exact: ExactMode;
  nativeFeatures: string[];
  onState?: (s: { loading: boolean; error: string | null }) => void;
}

interface Slot {
  id: number;
  html: string;
}

let nextId = 1;

export function Preview(p: Props) {
  const [hostRef, host] = useSize<HTMLDivElement>();
  const [slots, setSlots] = useState<Slot[]>([]);
  const [visible, setVisible] = useState<number | null>(null);
  const [frameSize, setFrameSize] = useState<[number, number] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [exactImg, setExactImg] = useState<{ key: string; url: string } | null>(null);
  const iframes = useRef(new Map<number, HTMLIFrameElement>());
  const latest = useLatest(p);
  const visibleRef = useLatest(visible);

  // fetch the HTML backend for the current source/format
  useEffect(() => {
    if (!p.source) return;
    const ac = new AbortController();
    const t = setTimeout(async () => {
      setLoading(true);
      try {
        const html = await api.preview(p.source!, p.size ? undefined : p.format || undefined, ac.signal, p.size);
        const id = nextId++;
        setSlots((s) => [...s.filter((x) => x.id === visibleRef.current), { id, html: withBridge(html, id) }]);
        setError(null);
      } catch (e) {
        if ((e as Error).name === 'AbortError') return;
        setError(e instanceof ApiError ? e.message : String(e));
        setLoading(false);
      }
    }, 120);
    return () => {
      clearTimeout(t);
      ac.abort();
    };
  }, [p.source, p.format, p.size]);

  // messages from the frames
  useEffect(() => {
    const onMsg = (e: MessageEvent) => {
      const m = e.data as FromFrame;
      if (!m || m.__afx !== 1) return;
      const fr = iframes.current.get(m.id);
      if (!fr || e.source !== fr.contentWindow) return;
      if (m.type === 'ready') {
        const cur = latest.current;
        if (m.w && m.h) setFrameSize([m.w, m.h]);
        if (m.duration) cur.onDuration?.(m.duration);
        fr.contentWindow?.postMessage(cur.playing ? { __afx: 1, type: 'play', t: cur.time } : { __afx: 1, type: 'seek', t: cur.time }, '*');
        setVisible(m.id);
        setSlots((s) => s.filter((x) => x.id >= m.id));
        setLoading(false);
      } else if (m.type === 'time' && m.id === visibleRef.current && latest.current.playing) {
        latest.current.onTime(m.t);
      }
    };
    addEventListener('message', onMsg);
    return () => removeEventListener('message', onMsg);
  }, []);

  const post = (msg: object) => {
    const fr = visible !== null ? iframes.current.get(visible) : null;
    fr?.contentWindow?.postMessage({ __afx: 1, ...msg }, '*');
  };

  // transport -> frame
  useEffect(() => {
    if (p.playing) post({ type: 'play', t: p.time });
    else post({ type: 'pause' });
  }, [p.playing, visible]);
  useEffect(() => {
    if (!p.playing) post({ type: 'seek', t: p.time });
  }, [p.time, p.playing]);

  useEffect(() => p.onState?.({ loading, error }), [loading, error]);

  // exact native frame when paused
  const wantsExact = p.exact === 'always' || (p.exact === 'auto' && p.nativeFeatures.length > 0);
  const exactKey = `${p.format}|${p.size}|${p.time.toFixed(3)}|${p.source?.length}|${hash(p.source ?? '')}`;
  useEffect(() => {
    if (!wantsExact || p.playing || !p.source) return;
    const ac = new AbortController();
    const t = setTimeout(async () => {
      try {
        const width = Math.min(1920, Math.round((host.w || 640) * (window.devicePixelRatio || 1)));
        const url = await api.frame(p.source!, p.time, p.size ? undefined : p.format || undefined, width, ac.signal, p.size);
        setExactImg((old) => {
          if (old) URL.revokeObjectURL(old.url);
          return { key: exactKey, url };
        });
      } catch {
        /* compile errors are shown by the HTML preview / lint */
      }
    }, 280);
    return () => {
      clearTimeout(t);
      ac.abort();
    };
  }, [exactKey, wantsExact, p.playing]);

  const [W, H] = frameSize ?? p.baseSize;
  const expected = p.baseSize;
  // while a format switch is loading, size the box to the target format already
  const [bw, bh] = expected[0] / expected[1] !== W / H ? expected : [W, H];
  const k = useMemo(() => (host.w && host.h ? Math.min(host.w / bw, host.h / bh) : 0), [host.w, host.h, bw, bh]);
  const showExact = wantsExact && !p.playing && exactImg?.key === exactKey;

  return (
    <div ref={hostRef} class="preview-host">
      <div class="preview-box" data-testid="preview" style={{ width: bw * k, height: bh * k, position: 'absolute', left: (host.w - bw * k) / 2, top: (host.h - bh * k) / 2 }}>
        {slots.map((s) => (
          <iframe
            key={s.id}
            ref={(el) => {
              if (el) iframes.current.set(s.id, el);
              else iframes.current.delete(s.id);
            }}
            class={s.id === visible ? '' : 'hidden'}
            sandbox="allow-scripts"
            srcdoc={s.html}
            title="Live preview"
            data-frame-id={s.id}
            style={{ width: W, height: H, transform: `scale(${(bw * k) / W})` }}
          />
        ))}
        {exactImg && <img class={'exact' + (showExact ? ' on' : '')} src={exactImg.url} alt="" data-testid="exact-frame" />}
        {!slots.length && !error && <div class="preview-empty">Loading preview…</div>}
        <div class="preview-badges">
          {wantsExact && (
            <span class={'chip' + (showExact ? ' accent' : '')} title={p.nativeFeatures.length ? `The live preview approximates: ${p.nativeFeatures.join(', ')}. Pause to see the exact frame.` : ''}>
              <span class="dot" />
              {showExact ? t('exact') : p.playing ? t('liveApprox') : t('renderingFrame')}
            </span>
          )}
        </div>
        <div class={'preview-loading' + (loading ? ' on' : '')} />
        {error && <div class="preview-error">{error}</div>}
      </div>
    </div>
  );
}

function hash(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return h >>> 0;
}
