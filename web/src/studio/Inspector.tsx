// Property inspector: the selected keyframe (value, easing with a curve preview), event, scene or
// element (its props, editable, and a new keyframe track at the playhead).
import { useEffect, useState } from 'preact/hooks';
import { easeGroups, easePath, normEase } from '../model/easings';
import { getNode, type PropValue, type ReelDoc } from '../model/reel';
import type { Timeline, TLEvent, TLKey, TLRow, TLScene } from '../model/timeline';
import { num } from '../model/time';
import type { Catalog } from '../api';
import { Diamond, Plus, Trash } from '../ui/icons';
import type { Sel } from './Timeline';

export interface InspectorActions {
  setKey: (k: TLKey, patch: { value?: string; ease?: string | null }) => void;
  moveKey: (k: TLKey, t: number) => void;
  deleteKey: (k: TLKey) => void;
  setEventExpr: (e: TLEvent, expr: string) => void;
  setProp: (path: number[], key: string, value: PropValue | null) => void;
  setSceneDur: (s: TLScene, dur: number) => void;
  addTrack: (row: TLRow, channel: string, value: string) => void;
  seek: (t: number) => void;
}

interface Props {
  doc: ReelDoc | null;
  tl: Timeline | null;
  sel: Sel;
  catalog: Catalog | null;
  time: number;
  act: InspectorActions;
}

export function Inspector({ doc, tl, sel, catalog, time, act }: Props) {
  if (!tl || !doc || !sel) {
    return (
      <div class="insp-empty">
        <Diamond size={22} style={{ color: 'var(--key)', marginBottom: 8 }} />
        <div style={{ color: 'var(--text-2)', fontWeight: 600, marginBottom: 6 }}>Nothing selected</div>
        Click a keyframe, an <span style={{ color: 'var(--event)' }}>@event</span> or a layer in the timeline.
        <br />
        Drag to retime · double-click a channel lane to add a key.
        <div style={{ marginTop: 14, display: 'grid', gap: 6, justifyContent: 'center', textAlign: 'left' }}>
          <span><span class="kbd">Space</span> play / pause</span>
          <span><span class="kbd">⌘/Ctrl Z</span> undo · <span class="kbd">⇧⌘Z</span> redo</span>
          <span><span class="kbd">Shift</span> drag snaps to 0.1 s</span>
          <span><span class="kbd">Del</span> delete keyframe</span>
        </div>
      </div>
    );
  }
  if (sel.type === 'key') {
    const key = tl.rows.flatMap((r) => r.tracks.flatMap((t) => t.keys.map((k) => ({ k, r })))).find((x) => x.k.uid === sel.uid);
    if (!key) return null;
    return <KeyInspector k={key.k} row={key.r} catalog={catalog} act={act} />;
  }
  if (sel.type === 'event') {
    const ev = tl.events.find((e) => e.uid === sel.uid);
    if (!ev) return null;
    return (
      <div class="inspector" data-testid="inspector">
        <div class="insp-title">
          <span style={{ color: 'var(--event)' }}>@</span> Event <span class="kind">at</span>
        </div>
        <div class="field">
          <label class="field-label">Anchor <span class="count">{num(ev.t, 3)}s</span></label>
          <CommitInput value={ev.expr} mono onCommit={(v) => act.setEventExpr(ev, v)} />
          <div class="faint" style={{ fontSize: 12 }}>
            {ev.anchor?.base ? <>Anchored to <code>{ev.anchor.base}</code>; dragging keeps the anchor and changes the offset.</> : 'Scene time. Write a word anchor (from [[...]] in a say) to follow the narration.'}
          </div>
        </div>
        <div class="field">
          <label class="field-label">Does</label>
          <div class="mono" style={{ font: '12px var(--mono)', color: 'var(--text-2)', lineHeight: 1.6 }}>{ev.label}</div>
        </div>
        <button class="btn sm" onClick={() => act.seek(ev.t)}>Move playhead here</button>
      </div>
    );
  }
  if (sel.type === 'scene') {
    const s = tl.scenes[sel.idx];
    const node = s && getNode(doc, s.path);
    if (!s || !node) return null;
    return (
      <div class="inspector" data-testid="inspector">
        <div class="insp-title">
          {s.id} <span class="kind">scene</span>
        </div>
        <div class="field">
          <label class="field-label">Length <span class="count">starts {num(s.start, 2)}s</span></label>
          <CommitInput value={s.durProp ?? num(s.dur, 3) + 's'} mono onCommit={(v) => {
            const n = parseFloat(v);
            if (!Number.isNaN(n)) act.setSceneDur(s, /ms$/.test(v) ? n / 1000 : n);
          }} />
        </div>
        <PropList props={node.props ?? {}} skip={['dur']} onSet={(k, v) => act.setProp(s.path, k, v)} />
      </div>
    );
  }
  const row = tl.rows.find((r) => r.uid === sel.uid);
  if (!row) return null;
  const node = row.path ? getNode(doc, row.path) : null;
  return (
    <div class="inspector" data-testid="inspector">
      <div class="insp-title">
        #{row.id} <span class="kind">{row.path ? row.kind : 'target'}</span>
      </div>
      {node && node.args && node.args.length > 1 && (
        <div class="field">
          <label class="field-label">Content</label>
          {node.args.slice(1).map((a, i) => (
            <CommitInput key={i} value={a} onCommit={(v) => act.setProp(row.path!, '\u0000arg' + (i + 1), v)} />
          ))}
        </div>
      )}
      {node && <PropList props={node.props ?? {}} onSet={(k, v) => act.setProp(row.path!, k, v)} />}
      <AddTrack row={row} catalog={catalog} time={time} act={act} />
    </div>
  );
}

function KeyInspector({ k, row, catalog, act }: { k: TLKey; row: TLRow; catalog: Catalog | null; act: InspectorActions }) {
  const groups = easeGroups(catalog);
  const [custom, setCustom] = useState(k.ease ?? '');
  useEffect(() => setCustom(k.ease ?? ''), [k.uid, k.ease]);
  const path = easePath(k.ease, 240, 72, 0.18);
  return (
    <div class="inspector" data-testid="inspector">
      <div class="insp-title">
        <Diamond size={13} style={{ color: 'var(--key)' }} /> {k.channel} <span class="kind">#{row.id}</span>
      </div>
      <div class="props" style={{ gridTemplateColumns: '64px 1fr' }}>
        <label>time</label>
        <CommitInput value={num(k.t, 3) + 's'} mono disabled={k.locked} testid="key-time" onCommit={(v) => {
          const n = parseFloat(v);
          if (!Number.isNaN(n)) act.moveKey(k, n);
        }} />
        <label>value</label>
        <CommitInput value={k.value} mono testid="key-value" onCommit={(v) => act.setKey(k, { value: v })} />
      </div>
      {k.locked && <div class="faint" style={{ fontSize: 12 }}>This key is anchored to <code>{k.time}</code>; retime it in the code.</div>}
      <div class="field">
        <label class="field-label">
          Easing <span class="count">into this key</span>
        </label>
        <svg class="ease-preview" viewBox="0 0 240 72" preserveAspectRatio="none" aria-hidden="true">
          <line x1="0" y1={72 - 0.18 * 72} x2="240" y2={72 - 0.18 * 72} stroke="var(--border)" />
          <line x1="0" y1={0.18 * 72} x2="240" y2={0.18 * 72} stroke="var(--border)" />
          {path ? <path d={path} fill="none" stroke="var(--key)" stroke-width="2" vector-effect="non-scaling-stroke" /> : <text x="120" y="40" text-anchor="middle" fill="var(--text-3)" font-size="11">{k.ease}</text>}
        </svg>
        <div class="ease-grid" data-testid="ease-grid">
          {groups.map((g) => (
            <>
              <div class="ease-group-label">{g.label}</div>
              {g.names.map((n) => (
                <button key={g.label + n} type="button" aria-pressed={!!k.ease && normEase(k.ease) === normEase(n)} onClick={() => act.setKey(k, { ease: n })}>
                  {n}
                </button>
              ))}
            </>
          ))}
        </div>
        <div class="row">
          <input class="input sm mono" placeholder="cubic(.2,.8,.2,1)" value={custom} onInput={(e) => setCustom((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.key === 'Enter' && act.setKey(k, { ease: custom || null })} aria-label="Custom easing" />
          <button class="btn sm" onClick={() => act.setKey(k, { ease: custom || null })}>Set</button>
          {k.ease && <button class="btn sm ghost" onClick={() => act.setKey(k, { ease: null })} title="Remove easing (linear)">Clear</button>}
        </div>
      </div>
      <div class="row">
        <button class="btn sm" onClick={() => act.seek(k.t)}>Go to key</button>
        <button class="btn sm ghost" style={{ color: 'var(--danger)', marginLeft: 'auto' }} onClick={() => act.deleteKey(k)} disabled={k.locked} data-testid="delete-key">
          <Trash size={14} /> Delete
        </button>
      </div>
    </div>
  );
}

function AddTrack({ row, catalog, time, act }: { row: TLRow; catalog: Catalog | null; time: number; act: InspectorActions }) {
  const channels = catalog?.channels ?? ['x', 'y', 'scale', 'rot', 'opacity', 'blur', 'color'];
  const [ch, setCh] = useState('opacity');
  const [val, setVal] = useState('1');
  return (
    <div class="field">
      <label class="field-label">
        New keyframe <span class="count">at {num(time, 2)}s</span>
      </label>
      <div class="row">
        <select class="select sm" value={ch} onChange={(e) => setCh((e.target as HTMLSelectElement).value)} aria-label="Channel" style={{ flex: 1 }}>
          {channels.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        <input class="input sm mono" style={{ width: 72 }} value={val} onInput={(e) => setVal((e.target as HTMLInputElement).value)} aria-label="Value" />
        <button class="btn sm icon" title="Add keyframe" aria-label="Add keyframe" onClick={() => act.addTrack(row, ch, val)}>
          <Plus size={14} />
        </button>
      </div>
    </div>
  );
}

function PropList({ props, skip = [], onSet }: { props: Record<string, PropValue>; skip?: string[]; onSet: (k: string, v: PropValue | null) => void }) {
  const [newKey, setNewKey] = useState('');
  const entries = Object.entries(props).filter(([k]) => !skip.includes(k));
  return (
    <div class="field">
      <label class="field-label">Properties</label>
      <div class="props" data-testid="props">
        {entries.map(([k, v]) => (
          <>
            <label key={'l' + k} title={k}>{k}</label>
            <div key={'v' + k} style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
              {typeof v === 'string' && /^#[0-9a-f]{3,8}$/i.test(v) && <span style={{ width: 16, height: 16, borderRadius: 4, flex: 'none', background: v, boxShadow: '0 0 0 1px var(--border-2)' }} />}
              <CommitInput value={fmtProp(v)} mono onCommit={(nv) => onSet(k, parseProp(nv))} />
            </div>
          </>
        ))}
        <input class="input sm mono" placeholder="new prop" value={newKey} onInput={(e) => setNewKey((e.target as HTMLInputElement).value.replace(/[^\w-]/g, ''))} aria-label="New property name" />
        <CommitInput value="" mono placeholder="value" onCommit={(v) => {
          if (newKey && v) {
            onSet(newKey, parseProp(v));
            setNewKey('');
          }
        }} />
      </div>
    </div>
  );
}

function fmtProp(v: PropValue): string {
  if (Array.isArray(v)) return '[' + v.map(fmtProp).join(', ') + ']';
  if (typeof v === 'number') return num(v, 4);
  return String(v);
}

function parseProp(s: string): PropValue {
  const t = s.trim();
  if (/^-?\d+(\.\d+)?$/.test(t)) return Number(t);
  return t;
}

function CommitInput({ value, onCommit, mono, disabled, placeholder, testid }: { value: string; onCommit: (v: string) => void; mono?: boolean; disabled?: boolean; placeholder?: string; testid?: string }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  const commit = () => {
    if (v !== value) onCommit(v);
  };
  return (
    <input
      class={'input sm' + (mono ? ' mono' : '')}
      value={v}
      disabled={disabled}
      placeholder={placeholder}
      data-testid={testid}
      onInput={(e) => setV((e.target as HTMLInputElement).value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          commit();
          (e.target as HTMLInputElement).blur();
        }
        if (e.key === 'Escape') setV(value);
      }}
    />
  );
}
