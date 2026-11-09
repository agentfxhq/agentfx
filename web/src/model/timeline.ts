// Timeline model: JSON form + resolved timing -> scenes, element rows, keyframes, events; and the
// edits the timeline makes, as pure functions on the JSON form (JSON -> edit -> JSON).

import { getNode, idOf, updateNode, videoInfo, type NodePath, type PropValue, type ReelDoc, type ReelNode } from './reel';
import { joinKey, parseAnim, splitKey } from './anim';
import { formatAnchor, formatDur, num, parseAnchor, parseDur, snap, type Anchor, type Unit } from './time';

// ---------------------------------------------------------------- CLI timing JSON

export interface TimingAnchor {
  name: string;
  start: number;
  end: number;
  say: number;
}
export interface TimingScene {
  id: string;
  start: number;
  dur: number;
  says: { start: number; end: number; written?: string }[];
  anchors: TimingAnchor[];
}
export interface Timing {
  timeline: { fps: number; total: number; scenes: TimingScene[] };
}

// ---------------------------------------------------------------- model

export interface TLKey {
  uid: string;
  animPath: NodePath;
  argIndex: number;
  channel: string;
  t: number; // absolute seconds
  base: number; // absolute time of the anim's 0s
  time: string; // as written
  unit: Unit | 'anchor';
  pctOf?: number;
  value: string;
  ease: string | null;
  locked: boolean;
}

export interface TLTrack {
  channel: string;
  keys: TLKey[];
}

export interface TLChip {
  eventUid: string;
  verb: string;
  t: number;
  dur: number;
}

export interface TLRow {
  uid: string;
  sceneIdx: number;
  id: string;
  kind: string;
  path: NodePath | null; // null: anim target that is not declared in the scene (#camera, #scene)
  depth: number;
  tracks: TLTrack[];
  chips: TLChip[];
}

export interface TLEvent {
  uid: string;
  path: NodePath;
  sceneIdx: number;
  t: number;
  expr: string;
  anchor: Anchor | null;
  baseAbs: number; // absolute time of the anchor base (scene start for plain times)
  locked: boolean;
  label: string;
}

export interface TLScene {
  idx: number;
  id: string;
  path: NodePath;
  start: number;
  dur: number;
  durProp: string | null;
  title: string;
}

export interface Timeline {
  fps: number;
  total: number;
  scenes: TLScene[];
  rows: TLRow[];
  events: TLEvent[];
}

const NOT_ELEMENTS = new Set(['at', 'when', 'anim', 'set', 'say', 'pause', 'note', 'camera', 'music', 'captions', 'pronounce']);
export const ELEMENT_KINDS = new Set(['text', 'title', 'display', 'h1', 'h2', 'h3', 'body', 'small', 'label', 'stack', 'row', 'col', 'grid',
  'box', 'card', 'spacer', 'image', 'video', 'svg', 'html', 'icon', 'browser', 'phone', 'terminal', 'code', 'chart', 'counter', 'particles',
  'layer', 'group', 'rect', 'ellipse', 'circle', 'path', 'line', 'poly', 'star', 'lower3', 'toast', 'chapter', 'link', 'diagram', 'node',
  'lottie', 'shader', 'textpath', 'stat', 'note', 'steps', 'pill', 'quote', 'list', 'window', 'device', 'progress', 'panel', 'cloud',
  'presenter', 'light', 'world', 'place', 'fluid', 'nest', 'scene3d', 'gen', 'visualizer']);

function eventLabel(n: ReelNode): string {
  const parts = (n.children ?? []).map((c) => [c.kind, ...(c.args ?? []).slice(0, c.kind === 'anim' ? 2 : 1)].join(' '));
  return parts.join(' ; ') || '@' + (n.args?.[0] ?? '');
}

export function buildTimeline(doc: ReelDoc, timing: Timing | null): Timeline {
  const info = videoInfo(doc);
  const fps = timing?.timeline.fps || info.fps;
  const scenes: TLScene[] = [];
  const rows: TLRow[] = [];
  const events: TLEvent[] = [];
  let cursor = 0;
  let sceneIdx = 0;

  doc.nodes.forEach((sn, si) => {
    if (sn.kind !== 'scene') return;
    const ts = timing?.timeline.scenes[sceneIdx];
    const durProp = sn.props?.dur !== undefined ? String(sn.props.dur) : null;
    const start = ts ? ts.start : cursor;
    const dur = ts ? ts.dur : parseDur(durProp ?? '3s', fps)?.seconds ?? 3;
    cursor = start + dur;
    const scene: TLScene = {
      idx: sceneIdx, id: String(sn.args?.[0] ?? `scene${sceneIdx + 1}`), path: [si], start, dur, durProp,
      title: String(sn.props?.title ?? sn.args?.[0] ?? ''),
    };
    scenes.push(scene);
    const anchors = ts?.anchors ?? [];
    const rowById = new Map<string, TLRow>();
    const declared = new Set<string>();
    let prevEvent = start;

    const rowFor = (id: string, kind: string, path: NodePath | null, depth: number): TLRow => {
      let r = rowById.get(id);
      if (!r) {
        r = { uid: `r${scene.idx}:${id}`, sceneIdx: scene.idx, id, kind, path, depth, tracks: [], chips: [] };
        rowById.set(id, r);
        rows.push(r);
      }
      return r;
    };

    const resolveBase = (a: Anchor, sayIdx: number | null): number | null => {
      if (a.base === null) return start;
      const b = a.base;
      if (b === 'start') return start;
      if (b === 'end') return start + dur;
      if (b === 'prev') return prevEvent;
      const isEnd = b.endsWith('.end');
      const name = isEnd ? b.slice(0, -4) : b;
      const cands = anchors.filter((x) => x.name === name);
      const hit = cands.find((x) => sayIdx !== null && x.say === sayIdx) ?? cands[0];
      return hit ? (isEnd ? hit.end : hit.start) : null;
    };

    const addAnim = (an: ReelNode, path: NodePath, base: number, ownerId: string | null, depth: number) => {
      const pa = parseAnim(an.args ?? []);
      const target = pa.target ?? ownerId;
      if (!target) return;
      const row = rowFor(target, 'layer', null, depth);
      const pctOf = parseDur(an.props?.dur, fps)?.seconds;
      for (const tr of pa.tracks) {
        let track = row.tracks.find((t) => t.channel === tr.channel);
        if (!track) {
          track = { channel: tr.channel, keys: [] };
          row.tracks.push(track);
        }
        for (const k of tr.keys) {
          let t: number;
          let unit: TLKey['unit'];
          let locked = false;
          if (k.time.startsWith('@')) {
            const a = parseAnchor(k.time, fps);
            const b = a ? resolveBase(a, null) : null;
            t = (b ?? base) + (a?.offset?.seconds ?? 0);
            unit = 'anchor';
            locked = true;
          } else {
            const d = parseDur(k.time, fps, pctOf);
            t = base + (d?.seconds ?? 0);
            unit = d?.unit ?? 's';
            locked = !d;
          }
          track.keys.push({
            uid: `${path.join('.')}:${k.argIndex}`, animPath: path, argIndex: k.argIndex, channel: tr.channel, t, base,
            time: k.time, unit, pctOf, value: k.value, ease: k.ease, locked,
          });
        }
        track.keys.sort((a, b) => a.t - b.t);
      }
    };

    const addEvent = (en: ReelNode, path: NodePath, sayIdx: number | null) => {
      const expr = String(en.args?.[0] ?? '0s');
      const a = parseAnchor(expr, fps);
      const baseAbs = a ? resolveBase(a, sayIdx) : null;
      const t = (baseAbs ?? start) + (a?.offset?.seconds ?? 0);
      const ev: TLEvent = {
        uid: `e${path.join('.')}`, path, sceneIdx: scene.idx, t, expr, anchor: a, baseAbs: baseAbs ?? start,
        locked: !a || baseAbs === null || (a.base !== null && a.base.startsWith('beat')), label: eventLabel(en),
      };
      events.push(ev);
      prevEvent = t;
      (en.children ?? []).forEach((c, ci) => {
        const cp = [...path, ci];
        if (c.kind === 'anim') addAnim(c, cp, t, null, 0);
        else {
          const tgt = c.args?.find((x) => x.startsWith('#'));
          if (tgt) {
            const row = rowFor(tgt.slice(1), 'layer', null, 0);
            row.chips.push({ eventUid: ev.uid, verb: c.kind, t, dur: parseDur(c.props?.dur ?? c.props?.for, fps)?.seconds ?? 0 });
          }
        }
      });
    };

    let sayCount = 0;
    const walk = (list: ReelNode[], parentPath: NodePath, depth: number, ownerId: string | null, ownerStart: number) => {
      list.forEach((n, i) => {
        const p = [...parentPath, i];
        if (n.kind === 'when') return; // per-format overrides: not on the shared timeline
        if (n.kind === 'at') return addEvent(n, p, null);
        if (n.kind === 'say') {
          const si2 = sayCount++;
          (n.children ?? []).forEach((c, ci) => {
            if (c.kind === 'at') addEvent(c, [...p, ci], si2);
          });
          return;
        }
        if (n.kind === 'anim') return addAnim(n, p, ownerStart, ownerId, depth);
        const id = idOf(n);
        const isElement = !NOT_ELEMENTS.has(n.kind) && (ELEMENT_KINDS.has(n.kind) || (id !== null && !declared.has(id)));
        if (isElement && id) {
          declared.add(id);
          const row = rowFor(id, n.kind, p, depth);
          row.kind = n.kind;
          row.path = p;
          row.depth = depth;
          walk(n.children ?? [], p, depth + 1, id, ownerStart);
        } else if (isElement) {
          walk(n.children ?? [], p, depth, ownerId, ownerStart);
        }
      });
    };
    walk(sn.children ?? [], [si], 0, null, start);
    // rows of this scene in declaration order first, synthetic targets last
    sceneIdx++;
  });

  const total = timing?.timeline.total ?? cursor;
  return { fps, total, scenes, rows, events };
}

// ---------------------------------------------------------------- edits (pure)

function setArg(doc: ReelDoc, path: NodePath, argIndex: number, value: string): ReelDoc {
  return updateNode(doc, path, (n) => {
    const args = (n.args ?? []).slice();
    args[argIndex] = value;
    return { ...n, args };
  });
}

/** Keep the keys of every channel in time order (the compiler wants ascending key times). */
function sortKeys(n: ReelNode, fps: number): ReelNode {
  const args = (n.args ?? []).slice();
  const pctOf = parseDur(n.props?.dur, fps)?.seconds;
  let i = 0;
  while (i < args.length) {
    if (!splitKey(args[i]) || args[i].startsWith('#')) {
      i++;
      continue;
    }
    let j = i;
    while (j < args.length && splitKey(args[j]) && !args[j].startsWith('#')) j++;
    const seg = args.slice(i, j);
    const times = seg.map((a) => parseDur(splitKey(a)!.time, fps, pctOf)?.seconds);
    if (times.every((t) => t !== undefined)) {
      const order = seg.map((a, k) => ({ a, t: times[k]! })).sort((x, y) => x.t - y.t);
      order.forEach((o, k) => (args[i + k] = o.a));
    }
    i = j;
  }
  return { ...n, args };
}

export function moveKey(doc: ReelDoc, key: TLKey, newAbs: number, fps: number): ReelDoc {
  if (key.locked) return doc;
  const rel = Math.max(0, snap(newAbs - key.base, fps));
  const unit = key.unit === 'anchor' ? 's' : key.unit;
  const time = formatDur(rel, unit, fps, key.pctOf);
  const d = setArg(doc, key.animPath, key.argIndex, joinKey({ time, value: key.value, ease: key.ease }));
  return updateNode(d, key.animPath, (n) => sortKeys(n, fps));
}

export function setKey(doc: ReelDoc, key: TLKey, patch: { value?: string; ease?: string | null }, fps: number): ReelDoc {
  const value = patch.value !== undefined ? patch.value.trim().replace(/\s+/g, '') || key.value : key.value;
  const ease = patch.ease !== undefined ? (patch.ease ? patch.ease.replace(/\s+/g, '') : null) : key.ease;
  const d = setArg(doc, key.animPath, key.argIndex, joinKey({ time: key.time, value, ease }));
  return updateNode(d, key.animPath, (n) => sortKeys(n, fps));
}

export function deleteKey(doc: ReelDoc, key: TLKey): ReelDoc {
  const anim = getNode(doc, key.animPath);
  if (!anim) return doc;
  const pa = parseAnim(anim.args ?? []);
  const track = pa.tracks.find((t) => t.keys.some((k) => k.argIndex === key.argIndex));
  // last key of the only track: drop the whole anim node
  if (track && track.keys.length === 1 && pa.tracks.length === 1) return updateNode(doc, key.animPath, () => null);
  return updateNode(doc, key.animPath, (n) => {
    const args = (n.args ?? []).slice();
    if (track && track.keys.length === 1) args.splice(track.channelArgIndex, 2);
    else args.splice(key.argIndex, 1);
    return { ...n, args };
  });
}

/** Add a key at `abs` to the track's anim that starts last before it (else the first one). */
export function addKey(doc: ReelDoc, track: TLTrack, abs: number, value: string, fps: number): ReelDoc {
  const cands = track.keys.filter((k) => !k.locked);
  if (!cands.length) return doc;
  const host = cands.filter((k) => k.base <= abs + 1e-9).sort((a, b) => b.base - a.base)[0] ?? cands[0];
  const sibling = cands.filter((k) => k.animPath.join('.') === host.animPath.join('.')).sort((a, b) => b.argIndex - a.argIndex)[0];
  const rel = Math.max(0, snap(abs - host.base, fps));
  const unit = host.unit === 'anchor' ? 's' : host.unit;
  const tok = joinKey({ time: formatDur(rel, unit, fps, host.pctOf), value, ease: sibling.ease });
  const d = updateNode(doc, host.animPath, (n) => {
    const args = (n.args ?? []).slice();
    args.splice(sibling.argIndex + 1, 0, tok);
    return { ...n, args };
  });
  return updateNode(d, host.animPath, (n) => sortKeys(n, fps));
}

export function moveEvent(doc: ReelDoc, ev: TLEvent, newAbs: number, sceneStart: number, fps: number): ReelDoc {
  if (ev.locked || !ev.anchor) return doc;
  const a = ev.anchor;
  let expr: string;
  if (a.base === null) {
    const rel = Math.max(0, snap(newAbs - sceneStart, fps));
    expr = formatAnchor({ base: null, offset: { seconds: rel, unit: a.offset?.unit ?? 's' } }, fps);
  } else {
    const off = snap(newAbs - ev.baseAbs, fps);
    expr = formatAnchor({ base: a.base, offset: { seconds: off, unit: a.offset?.unit ?? 's' } }, fps);
  }
  return setArg(doc, ev.path, 0, expr);
}

export function setSceneDur(doc: ReelDoc, scene: TLScene, dur: number, fps: number): ReelDoc {
  const d = Math.max(1 / fps, snap(dur, fps));
  const unit = scene.durProp ? parseDur(scene.durProp, fps)?.unit ?? 's' : 's';
  return updateNode(doc, scene.path, (n) => ({ ...n, props: { ...(n.props ?? {}), dur: formatDur(d, unit === 'bare' ? 's' : unit, fps) } }));
}

export function setProp(doc: ReelDoc, path: NodePath, key: string, value: PropValue | null): ReelDoc {
  return updateNode(doc, path, (n) => {
    const props = { ...(n.props ?? {}) };
    if (value === null || value === '') delete props[key];
    else props[key] = value;
    return { ...n, props };
  });
}

/** A new keyframe track: `anim #id <channel> <t>:<value>` at scene level (times from scene start). */
export function addTrack(doc: ReelDoc, scene: TLScene, id: string, channel: string, abs: number, value: string, fps: number): ReelDoc {
  const rel = Math.max(0, snap(abs - scene.start, fps));
  const node: ReelNode = { kind: 'anim', args: ['#' + id, channel, joinKey({ time: formatDur(rel, 's', fps), value, ease: null })] };
  return updateNode(doc, scene.path, (n) => ({ ...n, children: [...(n.children ?? []), node] }));
}

export function setArgAt(doc: ReelDoc, path: NodePath, index: number, value: string): ReelDoc {
  return setArg(doc, path, index, value);
}

export { num };
