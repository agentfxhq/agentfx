// The JSON form of Reel Script (docs/SPEC.md §3 "JSON form"): {reel, nodes:[{kind, args, props, body, children}]}.

export type PropValue = string | number | boolean | PropValue[];

export interface ReelNode {
  kind: string;
  args?: string[];
  props?: Record<string, PropValue>;
  body?: string;
  children?: ReelNode[];
}

export interface ReelDoc {
  reel: string;
  nodes: ReelNode[];
}

/** Index path from doc.nodes down through children. */
export type NodePath = number[];

export function getNode(doc: ReelDoc, path: NodePath): ReelNode | undefined {
  let list: ReelNode[] | undefined = doc.nodes;
  let node: ReelNode | undefined;
  for (const i of path) {
    node = list?.[i];
    if (!node) return undefined;
    list = node.children;
  }
  return node;
}

/** Immutable update of the node at `path` (structural sharing for untouched branches). */
export function updateNode(doc: ReelDoc, path: NodePath, fn: (n: ReelNode) => ReelNode | null): ReelDoc {
  const rec = (list: ReelNode[], depth: number): ReelNode[] => {
    const i = path[depth];
    const out = list.slice();
    if (depth === path.length - 1) {
      const r = fn(list[i]);
      if (r === null) out.splice(i, 1);
      else out[i] = r;
      return out;
    }
    const n = list[i];
    out[i] = { ...n, children: rec(n.children ?? [], depth + 1) };
    return out;
  };
  return { ...doc, nodes: rec(doc.nodes, 0) };
}

export function idOf(n: ReelNode): string | null {
  const a = n.args?.[0];
  return typeof a === 'string' && a.startsWith('#') ? a.slice(1) : null;
}

export function pathKey(p: NodePath): string {
  return p.join('.');
}

export interface VideoInfo {
  title: string;
  width: number;
  height: number;
  fps: number;
  formats: string[];
}

export function videoInfo(doc: ReelDoc | null): VideoInfo {
  const v = doc?.nodes.find((n) => n.kind === 'video');
  const size = String(v?.props?.size ?? '1920x1080');
  const m = /^(\d+)x(\d+)$/.exec(size);
  const fps = Number(v?.props?.fps ?? 30) || 30;
  const raw = v?.props?.formats;
  const formats = Array.isArray(raw) ? raw.map(String) : typeof raw === 'string' ? raw.replace(/[[\]]/g, '').split(',').map((s) => s.trim()).filter(Boolean) : [];
  return {
    title: String(v?.args?.[0] ?? 'Untitled'),
    width: m ? +m[1] : 1920,
    height: m ? +m[2] : 1080,
    fps,
    formats,
  };
}

/** Pixel size of a format: the short side stays at the base size (SPEC §11). */
export function formatSize(info: VideoInfo, format: string | null): [number, number] {
  const short = Math.min(info.width, info.height);
  const m = format ? /^(\d+):(\d+)$/.exec(format) : null;
  if (!m) return [info.width, info.height];
  const a = +m[1], b = +m[2];
  if (a >= b) return [Math.round((short * a) / b / 2) * 2, short];
  return [short, Math.round((short * b) / a / 2) * 2];
}

// Kinds and props the HTML backend does not draw (catalog: "Direct mode only").
const NATIVE_KINDS = new Set(['fluid', 'gen', 'cloud', 'scene3d', 'nest', 'world', 'shader', 'visualizer', 'presenter',
  'temporalglitch', 'ink', 'pour', 'stir', 'slitscan', 'echo', 'timedisplace', 'freeze', 'morph', 'sweep', 'shatter', 'physics']);
const NATIVE_PROPS = ['glitch', 'rgbsplit', 'vhs', 'dither', 'ascii', 'halftone', 'paper', 'kaleido', 'tunnel', 'glass', 'chromatic',
  'displace', 'dblur', 'rblur', 'mblur', 'riso', 'tint', 'levels', 'lut', 'rays', 'optics', 'streak', 'bokeh', 'halation'];

/** Effects in the script that only the native renderer draws (the live HTML preview skips them). */
export function nativeOnlyFeatures(doc: ReelDoc | null): string[] {
  const found = new Set<string>();
  const walk = (n: ReelNode) => {
    if (NATIVE_KINDS.has(n.kind)) found.add(n.kind);
    if (n.kind === 'anim') for (const a of n.args ?? []) if (NATIVE_PROPS.some((p) => a === p || a.startsWith(p + '-'))) found.add(a.split('-')[0]);
    for (const k of Object.keys(n.props ?? {})) {
      const p = NATIVE_PROPS.find((x) => k === x || k.startsWith(x + '-'));
      if (p) found.add(p);
    }
    n.children?.forEach(walk);
  };
  doc?.nodes.forEach(walk);
  return [...found];
}

/** Layout size for a `WxH` size override: its aspect at the script's short side (PLATFORM.md). */
export function sizeLayout(info: VideoInfo, size: string): [number, number] {
  const m = /^(\d+)x(\d+)$/.exec(size);
  if (!m) return [info.width, info.height];
  const w = +m[1], h = +m[2];
  const k = Math.min(info.width, info.height) / Math.min(w, h);
  return [Math.round((w * k) / 2) * 2, Math.round((h * k) / 2) * 2];
}
