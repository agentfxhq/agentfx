// Template mode: gallery -> form -> live preview -> render. Videos get a format switch, stills a
// multi-size picker with a thumbnail per size. Embeddable (`?embed=1`, `/embed/<id>`), en + de.
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { absUrl, api, ApiError, type Output, type TemplateManifest } from '../api';
import { localize, optionLabel, prettySize, t } from '../i18n';
import { csvToRows } from '../model/csv';
import { formatSize, nativeOnlyFeatures, sizeLayout, videoInfo, type ReelDoc } from '../model/reel';
import { parseDur } from '../model/time';
import { Preview } from '../preview/Preview';
import { Transport } from '../preview/Transport';
import { useDebounced } from '../ui/hooks';
import { Check, Chevron, Code, Download, Search } from '../ui/icons';
import { navigate } from '../ui/router';
import { defaults, ParamForm, type AssetLabel, type ParamValues } from './ParamForm';
import { doneState, pollJob, RenderPanel } from './RenderPanel';

export const HANDOFF_KEY = 'afx:handoff';

const FORMAT_LABEL: Record<string, () => string> = { '16:9': () => t('landscape'), '9:16': () => t('vertical'), '1:1': () => t('square'), '4:5': () => t('portrait') };

export function TemplateMode({ initialId, embed }: { initialId: string | null; embed: boolean }) {
  const [list, setList] = useState<TemplateManifest[] | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [id, setId] = useState<string | null>(initialId);
  const [values, setValues] = useState<ParamValues>({});
  const [assets, setAssets] = useState<AssetLabel>({});
  const [format, setFormat] = useState('16:9');
  const [picked, setPicked] = useState<string[]>([]); // still sizes (names)
  const [active, setActive] = useState<string | null>(null);
  const [source, setSource] = useState<string | null>(null);
  const [doc, setDoc] = useState<ReelDoc | null>(null);
  const [paramErr, setParamErr] = useState<string | null>(null);
  const [time, setTime] = useState(0);
  const [playing, setPlaying] = useState(true);
  const [duration, setDuration] = useState(0);
  const [query, setQuery] = useState('');

  const load = () => {
    setLoadErr(null);
    api.templates().then((l) => setList(l.map(localize)), (e) => setLoadErr(e instanceof ApiError ? e.message : String(e)));
  };
  useEffect(load, []);

  const tpl = useMemo(() => list?.find((x) => x.id === id) ?? list?.[0] ?? null, [list, id]);
  const still = tpl?.kind === 'still';
  const sizes = tpl?.sizes ?? {};
  const stillAt = parseDur(tpl?.still_at ?? '0s')?.seconds ?? 0;

  // a new template: reset the form, format, sizes and playhead
  useEffect(() => {
    if (!tpl) return;
    setValues(defaults(tpl.params));
    setAssets({});
    setFormat(tpl.formats.includes(format) ? format : tpl.formats[0] ?? '16:9');
    const names = Object.keys(tpl.sizes ?? {});
    setPicked(names.slice(0, 1));
    setActive(names[0] ?? null);
    setTime(tpl.kind === 'still' ? stillAt : 0);
    setPlaying(tpl.kind !== 'still');
    setDuration(tpl.duration_s);
    if (!embed || initialId !== tpl.id) {
      const q = new URLSearchParams(location.search);
      q.set('t', tpl.id);
      navigate(`${location.pathname.startsWith('/embed') ? location.pathname : '/play'}?${q}`, true);
    }
  }, [tpl?.id]);

  // params -> source (POST /instantiate) -> JSON (native-only effects for the exact-frame badge)
  const dv = useDebounced(values, 220);
  useEffect(() => {
    if (!tpl) return;
    const ac = new AbortController();
    api.instantiate(tpl.id, dv, ac.signal).then(
      (src) => {
        setSource(src);
        setParamErr(null);
        api.toJson(src, ac.signal).then(setDoc, () => {});
      },
      (e) => {
        if (e.name !== 'AbortError') setParamErr(e instanceof ApiError ? e.message : String(e));
      },
    );
    return () => ac.abort();
  }, [tpl?.id, dv]);

  // embed: tell the host page our height
  useEffect(() => {
    if (!embed || parent === window) return;
    const ro = new ResizeObserver(() => parent.postMessage({ type: 'agentfx:resize', height: document.documentElement.scrollHeight }, '*'));
    ro.observe(document.body);
    return () => ro.disconnect();
  }, [embed]);

  const info = videoInfo(doc);
  const native = useMemo(() => nativeOnlyFeatures(doc), [doc]);
  const filtered = (list ?? []).filter((x) => !query || (x.title + ' ' + x.description + ' ' + x.tags.join(' ')).toLowerCase().includes(query.toLowerCase()));
  const activeSize = still && active ? sizes[active] : undefined;
  const pickedSizes = still ? Object.fromEntries(picked.filter((n) => sizes[n]).map((n) => [n, sizes[n]])) : undefined;

  const openStudio = () => {
    if (!source) return;
    try {
      sessionStorage.setItem(HANDOFF_KEY, JSON.stringify({ source, format, template: tpl?.id }));
    } catch {
      /* ignore */
    }
    if (embed) window.open(`/studio?t=${tpl?.id ?? ''}&from=embed`, '_blank');
    else navigate(`/studio?t=${tpl?.id ?? ''}`);
  };

  if (loadErr)
    return (
      <div class="insp-empty" style={{ margin: 'auto' }}>
        <p>{t('loadFailed', { e: loadErr })}</p>
        <button class="btn" onClick={load}>
          {t('retry')}
        </button>
      </div>
    );

  return (
    <div class={'tm' + (still ? ' still' : '')} data-testid="template-mode">
      {!embed && (
        <aside class="gallery">
          <div class="gallery-head">
            <div class="panel-title">{t('templates')}</div>
            <div class="gallery-search">
              <Search />
              <input class="input" placeholder={t('search')} value={query} onInput={(e) => setQuery((e.target as HTMLInputElement).value)} aria-label={t('templates')} />
            </div>
          </div>
          <div class="gallery-list scroll" data-testid="gallery">
            {!list && [0, 1, 2].map((i) => <div key={i} class="tcard" style={{ opacity: 0.4 }}><div class="thumb" /><div /></div>)}
            {filtered.map((x) => (
              <button key={x.id} class="tcard" aria-current={x.id === tpl?.id} onClick={() => setId(x.id)} data-template={x.id}>
                <div class="thumb" style={{ backgroundImage: x.preview_url ? `url(${absUrl(x.preview_url)})` : undefined }}>
                  {x.kind === 'still' && <span class="thumb-tag">IMG</span>}
                </div>
                <div>
                  <div class="t">{x.title}</div>
                  <div class="d">{x.description}</div>
                </div>
              </button>
            ))}
            {list && !filtered.length && <div class="faint" style={{ padding: 12 }}>{t('noMatch', { q: query })}</div>}
          </div>
        </aside>
      )}

      <section class="stage">
        <div class="stage-canvas">
          <Preview
            source={source}
            format={format}
            size={activeSize}
            baseSize={activeSize ? sizeLayout(info, activeSize) : formatSize(info, format)}
            time={time}
            playing={playing && !still}
            onTime={setTime}
            onDuration={setDuration}
            exact={still ? 'always' : 'auto'}
            nativeFeatures={native}
          />
        </div>
        {still ? (
          <SizePicker sizes={sizes} picked={picked} active={active} source={source} at={stillAt} onPick={setPicked} onActive={setActive} />
        ) : (
          <Transport
            time={time}
            duration={duration || tpl?.duration_s || 0}
            playing={playing}
            onPlay={setPlaying}
            onSeek={(x) => {
              setPlaying(false);
              setTime(x);
            }}
            right={
              <div class="seg" role="group" aria-label={t('format')} data-testid="formats">
                {(tpl?.formats?.length ? tpl.formats : ['16:9']).map((f) => (
                  <button key={f} type="button" aria-pressed={format === f} onClick={() => setFormat(f)} title={FORMAT_LABEL[f]?.() ?? f} data-format={f}>
                    <FormatGlyph f={f} /> {f}
                  </button>
                ))}
              </div>
            }
          />
        )}
      </section>

      <aside class="side">
        <div class="side-body scroll">
          <div class="side-head">
            <h1>{tpl?.title ?? ' '}</h1>
            <p>{tpl?.description}</p>
          </div>
          <div class="divider" />
          {tpl && (
            <ParamForm
              params={tpl.params}
              values={values}
              assets={assets}
              optionLabel={(pid, o) => optionLabel(tpl, pid, o)}
              onChange={(pid, v, name) => {
                setValues((s) => {
                  const n = { ...s };
                  if (v === undefined) delete n[pid];
                  else n[pid] = v;
                  return n;
                });
                if (name !== undefined || v === undefined) setAssets((a) => ({ ...a, [pid]: name ?? '' }));
              }}
            />
          )}
          {paramErr && <div class="err-text" style={{ marginTop: 12 }}>{paramErr}</div>}
          {tpl && !embed && <BatchPanel tpl={tpl} still={still} sizes={pickedSizes} at={tpl.still_at} />}
        </div>
        <div class="side-foot">
          <RenderPanel source={source} format={format} fileStem={tpl?.id ?? 'agentfx'} disabled={!!paramErr} compact={embed && !still} still={still} sizes={pickedSizes} at={still ? tpl?.still_at ?? '0s' : undefined} />
          <button class="btn ghost block" onClick={openStudio} disabled={!source} data-testid="open-studio">
            <Code /> {t('openStudio')}
          </button>
          {embed && (
            <div class="made-with">
              {t('madeWith')}{' '}
              <a href={`/play?t=${tpl?.id ?? ''}`} target="_blank" rel="noopener">
                AgentFX
              </a>
            </div>
          )}
        </div>
      </aside>
    </div>
  );
}

function SizePicker({ sizes, picked, active, source, at, onPick, onActive }: { sizes: Record<string, string>; picked: string[]; active: string | null; source: string | null; at: number; onPick: (p: string[]) => void; onActive: (n: string) => void }) {
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const ds = useDebounced(source, 500);
  // small exact thumbnails, one request at a time (the box is shared)
  useEffect(() => {
    if (!ds) return;
    const ac = new AbortController();
    (async () => {
      for (const [n, wh] of Object.entries(sizes)) {
        try {
          const url = await api.frame(ds, at, undefined, 220, ac.signal, wh);
          setThumbs((th) => {
            if (th[n]) URL.revokeObjectURL(th[n]);
            return { ...th, [n]: url };
          });
        } catch {
          if (ac.signal.aborted) return;
        }
      }
    })();
    return () => ac.abort();
  }, [ds, JSON.stringify(sizes)]);

  return (
    <div class="sizes" data-testid="sizes">
      <div class="sizes-head">
        <span class="panel-title">{t('sizes')}</span>
        <span class="faint" style={{ fontSize: 12 }}>{t('sizesHint')}</span>
      </div>
      <div class="sizes-row">
        {Object.entries(sizes).map(([n, wh]) => {
          const [w, h] = wh.split('x').map(Number);
          const on = picked.includes(n);
          const box = 64;
          const tw = w >= h ? box : (box * w) / h, th = w >= h ? (box * h) / w : box;
          return (
            <div key={n} class={'size-card' + (on ? ' on' : '') + (active === n ? ' active' : '')} data-size={n}>
              <button type="button" class="size-thumb" onClick={() => {
                onActive(n);
                if (!on) onPick([...picked, n]);
              }} title={`${prettySize(n)} · ${wh}`}>
                <span style={{ width: tw, height: th, backgroundImage: thumbs[n] ? `url(${thumbs[n]})` : undefined }} />
              </button>
              <label class="size-meta">
                <input type="checkbox" checked={on} onChange={() => onPick(on ? picked.filter((x) => x !== n) : [...picked, n])} aria-label={prettySize(n)} />
                <span class="box">{on && <Check size={11} />}</span>
                <span>
                  <span class="nm">{prettySize(n)}</span>
                  <span class="wh">{wh.replace('x', '×')}</span>
                </span>
              </label>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function BatchPanel({ tpl, still, sizes, at }: { tpl: TemplateManifest; still: boolean; sizes?: Record<string, string>; at?: string }) {
  const [open, setOpen] = useState(false);
  const [csv, setCsv] = useState('');
  const [state, setState] = useState<{ s: 'idle' } | { s: 'running'; p: number } | { s: 'done'; url: string } | { s: 'error'; m: string }>({ s: 'idle' });
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => () => clearTimeout(timer.current), []);
  const cols = tpl.params.filter((p) => ['text', 'longtext', 'color', 'number', 'enum'].includes(p.type)).map((p) => ({ id: p.id, label: p.label, type: p.type === 'number' ? ('number' as const) : ('text' as const) }));
  const rows = csv.trim() ? csvToRows(csv, cols) : [];
  const n = rows.length * Math.max(1, sizes ? Object.keys(sizes).length : 1);
  const output: Output = still ? 'png' : 'mp4';
  const go = async () => {
    setState({ s: 'running', p: 0 });
    try {
      const { job_id } = await api.batch(tpl.id, rows, output, sizes, at);
      pollJob(job_id, (r) => {
        if (r.status === 'done') setState({ s: 'done', url: doneState(r, output).zip ?? absUrl(r.url ?? '') });
        else if (r.status === 'error') setState({ s: 'error', m: r.error ?? t('renderFailed') });
        else setState({ s: 'running', p: r.progress });
      }, timer);
    } catch (e) {
      setState({ s: 'error', m: e instanceof ApiError ? e.message : t('startFailed') });
    }
  };
  return (
    <div class="batch">
      <button type="button" class="batch-head" onClick={() => setOpen(!open)} aria-expanded={open} data-testid="batch-toggle">
        <Chevron size={12} style={{ transform: open ? 'rotate(90deg)' : '', transition: 'transform .15s' }} />
        {t('batch')}
      </button>
      {open && (
        <div style={{ display: 'grid', gap: 8 }}>
          <div class="faint" style={{ fontSize: 12 }}>{t('batchHint', { cols: cols.map((c) => c.id).join(', ') })}</div>
          <textarea class="textarea mono" rows={5} value={csv} placeholder={cols.map((c) => c.id).join(',')} onInput={(e) => {
              setCsv((e.target as HTMLTextAreaElement).value);
              if (state.s !== 'running') setState({ s: 'idle' });
            }} spellcheck={false} data-testid="batch-csv" />
          {state.s === 'done' ? (
            <a class="btn primary" href={state.url} download={`${tpl.id}-batch.zip`} data-testid="batch-download">
              <Download /> {t('downloadAll')}
            </a>
          ) : (
            <button class="btn" disabled={!rows.length || state.s === 'running'} onClick={go} data-testid="batch-go">
              {state.s === 'running' ? t('rendering', { p: Math.round(state.p * 100) }) : t('batchGo', { n })}
            </button>
          )}
          {state.s === 'error' && <div class="err-text">{state.m}</div>}
        </div>
      )}
    </div>
  );
}

function FormatGlyph({ f }: { f: string }) {
  const m = /^(\d+):(\d+)$/.exec(f);
  const a = m ? +m[1] / +m[2] : 1;
  const w = a >= 1 ? 12 : 12 * a, h = a >= 1 ? 12 / a : 12;
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
      <rect x={(14 - w) / 2} y={(14 - h) / 2} width={w} height={h} rx="1.5" fill="none" stroke="currentColor" stroke-width="1.4" />
    </svg>
  );
}
