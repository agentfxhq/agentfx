// The light timeline: scenes as blocks, `@` events as flags, element rows with verb chips, and one
// sub-row per animated channel with keyframe diamonds joined by their ease curves. Drag keys,
// events, chips and scene edges to retime; click to select; double-click a channel lane to add a key.
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { easePath } from '../model/easings';
import type { Timeline as TL, TLEvent, TLKey, TLRow, TLScene, TLTrack } from '../model/timeline';
import { fmtClock, num, snap } from '../model/time';
import { useLatest, useSize } from '../ui/hooks';
import { Diamond, Layers, Shape, Text as TextIcon } from '../ui/icons';

export type Sel = { type: 'key'; uid: string } | { type: 'event'; uid: string } | { type: 'row'; uid: string } | { type: 'scene'; idx: number } | null;

interface Props {
  tl: TL;
  time: number;
  onSeek: (t: number) => void;
  sel: Sel;
  onSelect: (s: Sel) => void;
  onMoveKey: (k: TLKey, t: number) => void;
  onMoveEvent: (e: TLEvent, t: number) => void;
  onSceneDur: (s: TLScene, dur: number) => void;
  onAddKey: (track: TLTrack, row: TLRow, t: number) => void;
}

type Line =
  | { type: 'ruler' }
  | { type: 'scenes' }
  | { type: 'events' }
  | { type: 'group'; scene: TLScene }
  | { type: 'row'; row: TLRow }
  | { type: 'track'; row: TLRow; track: TLTrack };

const LABEL_W = 200;
const PAD = 16;
const H: Record<Line['type'], number> = { ruler: 28, scenes: 36, events: 28, group: 24, row: 28, track: 24 };

type Drag = { uid: string; t: number } | null;

const TEXT_KINDS = new Set(['text', 'title', 'display', 'h1', 'h2', 'h3', 'body', 'small', 'label', 'quote', 'textpath', 'code', 'terminal', 'counter']);

export function Timeline(p: Props) {
  const { tl } = p;
  const [scrollRef, size] = useSize<HTMLDivElement>();
  const [pps, setPps] = useState(120);
  const [fitted, setFitted] = useState(false);
  const [drag, setDrag] = useState<Drag>(null);
  const latest = useLatest(p);
  const ppsRef = useLatest(pps);

  // fit the whole video into view once the size is known (and when the length changes a lot)
  const fitFor = useRef(0);
  useEffect(() => {
    if (!size.w || !tl.total) return;
    if (!fitted || Math.abs(fitFor.current - tl.total) / tl.total > 0.5) {
      setPps(Math.max(20, Math.min(600, (size.w - LABEL_W - PAD * 3) / tl.total)));
      fitFor.current = tl.total;
      setFitted(true);
    }
  }, [size.w, tl.total]);

  const lines = useMemo(() => {
    const out: Line[] = [{ type: 'ruler' }, { type: 'scenes' }, { type: 'events' }];
    tl.scenes.forEach((s) => {
      out.push({ type: 'group', scene: s });
      for (const r of tl.rows.filter((r) => r.sceneIdx === s.idx)) {
        out.push({ type: 'row', row: r });
        for (const t of r.tracks) out.push({ type: 'track', row: r, track: t });
      }
    });
    return out;
  }, [tl]);

  const X = (t: number) => PAD + t * pps;
  const width = PAD * 2 + Math.max(tl.total, 1) * pps + 120;

  const snapT = (t: number, ev: PointerEvent, self?: number) => {
    let v = Math.max(0, t);
    if (ev.shiftKey) v = Math.round(v * 10) / 10;
    else v = snap(v, tl.fps);
    // magnet: the playhead
    if (!ev.altKey && Math.abs((v - latest.current.time) * ppsRef.current) < 6 && self !== latest.current.time) v = latest.current.time;
    return v;
  };

  const beginDrag = (e: PointerEvent, uid: string, t0: number, locked: boolean, commit: (t: number) => void) => {
    e.stopPropagation();
    e.preventDefault();
    if (locked) return;
    const x0 = e.clientX;
    let moved = false;
    let cur = t0;
    const move = (ev: PointerEvent) => {
      const dx = ev.clientX - x0;
      if (!moved && Math.abs(dx) < 3) return;
      moved = true;
      cur = snapT(t0 + dx / ppsRef.current, ev, t0);
      setDrag({ uid, t: cur });
    };
    const up = () => {
      removeEventListener('pointermove', move);
      removeEventListener('pointerup', up);
      setDrag(null);
      if (moved && Math.abs(cur - t0) > 1e-6) commit(cur);
    };
    addEventListener('pointermove', move);
    addEventListener('pointerup', up);
  };

  const scrub = (e: PointerEvent) => {
    const el = scrollRef.current!;
    const at = (ev: PointerEvent) => {
      const r = el.getBoundingClientRect();
      const x = ev.clientX - r.left + el.scrollLeft - LABEL_W - PAD;
      latest.current.onSeek(Math.min(tl.total, snap(Math.max(0, x / ppsRef.current), tl.fps)));
    };
    at(e);
    const up = () => {
      removeEventListener('pointermove', at);
      removeEventListener('pointerup', up);
    };
    addEventListener('pointermove', at);
    addEventListener('pointerup', up);
  };

  const onWheel = (e: WheelEvent) => {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    const el = scrollRef.current!;
    const r = el.getBoundingClientRect();
    const mx = e.clientX - r.left + el.scrollLeft - LABEL_W - PAD;
    const tAt = mx / pps;
    const next = Math.max(10, Math.min(1200, pps * Math.exp(-e.deltaY * 0.0025)));
    setPps(next);
    requestAnimationFrame(() => (el.scrollLeft += tAt * (next - pps)));
  };

  const sel = p.sel;
  const isSel = (type: string, uid: string) => sel && sel.type === type && 'uid' in sel && sel.uid === uid;
  const ticks = useMemo(() => rulerTicks(pps, tl.total, tl.fps), [pps, tl.total, tl.fps]);
  const totalH = lines.reduce((a, l) => a + H[l.type], 0);

  const keyT = (k: TLKey) => (drag?.uid === k.uid ? drag.t : k.t);
  const evT = (e: TLEvent) => (drag?.uid === e.uid ? drag.t : e.t);

  const lane = (l: Line) => {
    switch (l.type) {
      case 'ruler':
        return (
          <div class="tl-lane tl-ruler" onPointerDown={scrub} data-testid="ruler">
            {ticks.map((t) => (
              <div key={t.t} class={'tk' + (t.major ? ' major' : '')} style={{ left: X(t.t) }} />
            ))}
            {ticks.filter((t) => t.major).map((t) => (
              <div key={'n' + t.t} class="tl-num" style={{ left: X(t.t) }}>
                {t.label}
              </div>
            ))}
          </div>
        );
      case 'scenes':
        return (
          <div class="tl-lane" onPointerDown={scrub}>
            {tl.scenes.map((s) => {
              const end = drag?.uid === 'edge' + s.idx ? drag.t : s.start + s.dur;
              return (
                <div key={s.idx}>
                  <div
                    class={'tl-sceneblock' + (sel?.type === 'scene' && sel.idx === s.idx ? ' sel' : '')}
                    style={{ left: X(s.start) + 1, width: Math.max(4, (end - s.start) * pps - 2) }}
                    onPointerDown={(e) => {
                      e.stopPropagation();
                      p.onSelect({ type: 'scene', idx: s.idx });
                    }}
                    title={s.durProp ? `dur=${s.durProp}` : 'Length from narration; drag the edge to set dur='}
                  >
                    {s.title || s.id}
                    <span class="dur">{num(end - s.start, 2)}s</span>
                  </div>
                  <div
                    class={'tl-edge' + (drag?.uid === 'edge' + s.idx ? ' drag' : '')}
                    style={{ left: X(end) }}
                    data-testid={`scene-edge-${s.id}`}
                    title="Drag to change the scene length"
                    onPointerDown={(e) => beginDrag(e, 'edge' + s.idx, s.start + s.dur, false, (t) => p.onSceneDur(s, t - s.start))}
                  >
                    {drag?.uid === 'edge' + s.idx && <div class="tl-tip">{num(drag.t - s.start, 3)}s</div>}
                  </div>
                </div>
              );
            })}
          </div>
        );
      case 'events':
        return (
          <div class="tl-lane" onPointerDown={() => p.onSelect(null)}>
            {tl.events.map((ev) => (
              <div
                key={ev.uid}
                class={'tl-event' + (isSel('event', ev.uid) ? ' sel' : '') + (ev.locked ? ' locked' : '')}
                style={{ left: X(evT(ev)) }}
                data-testid="event"
                title={`@${ev.expr}  ${ev.label}${ev.locked ? '\n(anchored to the music grid or unresolved: edit in code)' : ''}`}
                onPointerDown={(e) => {
                  p.onSelect({ type: 'event', uid: ev.uid });
                  beginDrag(e, ev.uid, ev.t, ev.locked, (t) => p.onMoveEvent(ev, t));
                }}
              >
                <div class="flag">
                  @{drag?.uid === ev.uid ? num(drag.t - tl.scenes[ev.sceneIdx].start, 3) + 's' : ev.expr} <span class="faint">{ev.label.split(' ')[0]}</span>
                </div>
              </div>
            ))}
          </div>
        );
      case 'group':
        return <div class="tl-lane tl-group-lane" />;
      case 'row': {
        const r = l.row;
        const s = tl.scenes[r.sceneIdx];
        const all = r.tracks.flatMap((t) => t.keys);
        return (
          <div class={'tl-lane' + (isSel('row', r.uid) ? ' sel' : '')} onPointerDown={() => p.onSelect({ type: 'row', uid: r.uid })}>
            <div class="tl-life" style={{ left: X(s.start), width: s.dur * pps }} />
            {r.chips.map((c, i) => {
              const ev = tl.events.find((e) => e.uid === c.eventUid);
              const t = ev ? evT(ev) : c.t;
              return (
                <div
                  key={i}
                  class="tl-chip"
                  style={{ left: X(t), width: Math.max(6, c.dur * pps) }}
                  title={`${c.verb} @${ev?.expr ?? ''}`}
                  onPointerDown={(e) => {
                    if (!ev) return;
                    p.onSelect({ type: 'event', uid: ev.uid });
                    beginDrag(e, ev.uid, ev.t, ev.locked, (tt) => p.onMoveEvent(ev, tt));
                  }}
                >
                  {c.dur * pps > 40 ? c.verb : ''}
                </div>
              );
            })}
            {all.map((k) => (
              <div key={k.uid} class="tl-key ghost" style={{ left: X(keyT(k)) }} />
            ))}
          </div>
        );
      }
      case 'track': {
        const keys = l.track.keys.map((k) => ({ k, t: keyT(k) })).sort((a, b) => a.t - b.t);
        return (
          <div
            class="tl-lane"
            onPointerDown={() => p.onSelect({ type: 'row', uid: l.row.uid })}
            onDblClick={(e) => {
              const el = scrollRef.current!;
              const r = el.getBoundingClientRect();
              const x = (e as MouseEvent).clientX - r.left + el.scrollLeft - LABEL_W - PAD;
              p.onAddKey(l.track, l.row, snap(Math.max(0, x / pps), tl.fps));
            }}
          >
            {keys.slice(1).map(({ k, t }, i) => {
              const prev = keys[i];
              const w = (t - prev.t) * pps;
              if (w < 2) return null;
              const up = Number(k.value) >= Number(prev.k.value) || Number.isNaN(Number(k.value));
              const d = easePath(k.ease, w, 14, 0.1);
              return (
                <div key={'s' + k.uid} class="tl-span curve" style={{ left: X(prev.t), width: w }}>
                  <svg width={w} height="14" viewBox={`0 0 ${w} 14`} preserveAspectRatio="none" style={up ? undefined : { transform: 'scaleY(-1)' }}>
                    <path d={`M0,${14 - 1.4} L${w},${1.4}`} stroke="var(--border-2)" stroke-width="1" fill="none" vector-effect="non-scaling-stroke" />
                    {d && <path d={d} stroke="var(--key)" stroke-opacity="0.75" stroke-width="1.5" fill="none" vector-effect="non-scaling-stroke" />}
                  </svg>
                </div>
              );
            })}
            {keys.map(({ k, t }) => (
              <div
                key={k.uid}
                class={'tl-key' + (isSel('key', k.uid) ? ' sel' : '') + (k.locked ? ' locked' : '')}
                style={{ left: X(t) }}
                data-testid="key"
                data-key={`${l.row.id}.${k.channel}@${num(k.t, 3)}`}
                title={`${k.channel} ${k.value}${k.ease ? ' ~' + k.ease : ''} @ ${num(k.t, 3)}s${k.locked ? ' (anchored: edit in code)' : ''}`}
                onPointerDown={(e) => {
                  p.onSelect({ type: 'key', uid: k.uid });
                  beginDrag(e, k.uid, k.t, k.locked, (tt) => p.onMoveKey(k, tt));
                }}
              >
                {drag?.uid === k.uid && <div class="tl-tip" style={{ transform: 'rotate(-45deg) translate(-30%, -40%)' }}>{num(drag.t, 3)}s</div>}
              </div>
            ))}
          </div>
        );
      }
    }
  };

  const label = (l: Line) => {
    switch (l.type) {
      case 'ruler':
        return (
          <div class="tl-label tl-corner">
            <span class="kbd">{fmtClock(p.time, tl.fps, true)}</span>
            <span class="tl-zoom" title="Zoom (Ctrl + wheel)">
              <input type="range" min={10} max={600} value={pps} style={{ '--p': `${((pps - 10) / 590) * 100}%` }} onInput={(e) => setPps(Number((e.target as HTMLInputElement).value))} aria-label="Timeline zoom" />
            </span>
          </div>
        );
      case 'scenes':
        return <div class="tl-label scene">Scenes</div>;
      case 'events':
        return (
          <div class="tl-label events">
            <span class="nm">@ Events</span>
            <span class="kd">{tl.events.length}</span>
          </div>
        );
      case 'group':
        return (
          <div class="tl-label group" onClick={() => p.onSelect({ type: 'scene', idx: l.scene.idx })}>
            <span>{l.scene.id}</span>
            <span class="kd">{num(l.scene.dur, 2)}s</span>
          </div>
        );
      case 'row': {
        const r = l.row;
        const Icon = TEXT_KINDS.has(r.kind) ? TextIcon : ['layer', 'group', 'stack', 'row', 'col', 'grid'].includes(r.kind) ? Layers : Shape;
        return (
          <div class={'tl-label' + (isSel('row', r.uid) ? ' sel' : '')} style={{ paddingLeft: 12 + r.depth * 14 }} onClick={() => p.onSelect({ type: 'row', uid: r.uid })} data-testid="row-label">
            <Icon size={13} />
            <span class="nm">#{r.id}</span>
            <span class="kd">{r.path ? r.kind : ''}</span>
          </div>
        );
      }
      case 'track':
        return (
          <div class="tl-label sub" style={{ paddingLeft: 30 + l.row.depth * 14 }} onClick={() => p.onSelect({ type: 'row', uid: l.row.uid })}>
            <Diamond size={10} style={{ color: 'var(--key)' }} />
            <span class="nm">{l.track.channel}</span>
            <span class="kd">{l.track.keys.length}</span>
          </div>
        );
    }
  };

  return (
    <div class="tl scroll" ref={scrollRef} onWheel={onWheel} data-testid="timeline">
      <div class="tl-inner" style={{ width: LABEL_W + width, height: totalH }}>
        {lines.map((l, i) => (
          <div key={i} class={`tl-line ${l.type}`} style={{ height: H[l.type] }}>
            {label(l)}
            {lane(l)}
          </div>
        ))}
        <div class="tl-bands" style={{ left: LABEL_W, top: H.ruler, height: totalH - H.ruler }}>
          {tl.scenes.map((s) => (
            <div key={s.idx} class={'tl-band' + (s.idx % 2 ? ' odd' : '')} style={{ left: X(s.start), width: s.dur * pps }} />
          ))}
        </div>
        <div class="tl-playhead" style={{ left: LABEL_W + X(p.time), height: totalH }} />
        <div class="tl-playhead-cap" style={{ left: LABEL_W + X(p.time) }} />
      </div>
    </div>
  );
}

function rulerTicks(pps: number, total: number, fps: number) {
  const minor = [1 / fps, 0.1, 0.25, 0.5, 1, 2, 5, 10].find((s) => s * pps >= 9) ?? 10;
  const major = [0.5, 1, 2, 5, 10, 30].find((s) => s * pps >= 64 && s >= minor) ?? 30;
  const out: { t: number; major: boolean; label: string }[] = [];
  const end = total + 120 / pps;
  for (let i = 0; i * minor <= end; i++) {
    const t = Math.round(i * minor * 1000) / 1000;
    const isMajor = Math.abs(t / major - Math.round(t / major)) < 1e-6;
    out.push({ t, major: isMajor, label: major < 1 ? `${num(t, 1)}s` : `${num(t, 0)}s` });
  }
  return out;
}
