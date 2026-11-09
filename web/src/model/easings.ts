// Ease names of Reel Script (docs/SPEC.md §4 "ease"), grouped for the picker, merged with the names
// found in the catalog's syntax strings, plus an approximate evaluator for the curve preview.
import type { Catalog } from '../api';

export interface EaseGroup {
  label: string;
  names: string[];
}

const FAM = ['sine', 'quad', 'cubic', 'quart', 'expo', 'back'];

export const BASE_GROUPS: EaseGroup[] = [
  { label: 'Basic', names: ['linear', 'hold', 'steps(4)'] },
  { label: 'Out', names: FAM.map((f) => `out-${f}`) },
  { label: 'In', names: FAM.map((f) => `in-${f}`) },
  { label: 'In-out', names: FAM.map((f) => `inout-${f}`) },
  {
    label: 'GSAP',
    names: ['power1.out', 'power2.out', 'power3.out', 'power4.out', 'power2.in', 'power2.inOut', 'power3.inOut', 'sine.inOut',
      'expo.out', 'expo.inOut', 'circ.out', 'circ.inOut', 'back.out(1.7)', 'elastic.out(1,0.3)', 'bounce.out', 'none'],
  },
  { label: 'Physics', names: ['spring', 'spring(170,26)', 'spring(260,20)', 'snappy', 'soft', 'bouncy', 'bounce(3)'] },
];

export function easeGroups(cat: Catalog | null): EaseGroup[] {
  const known = new Set(BASE_GROUPS.flatMap((g) => g.names.map(normEase)));
  const extra: string[] = [];
  for (const f of cat?.features ?? []) {
    if (!/eas|spring/i.test(f.name)) continue;
    for (const m of f.syntax.matchAll(/ease=([^\s|;]+(?:\([^)]*\))?)/g)) add(m[1]);
    for (const part of f.syntax.split('|')) add(part.trim().replace(/^ease=/, ''));
  }
  function add(n: string) {
    if (!/^[a-z][\w.-]*(\([\d., ]*\))?$/i.test(n)) return;
    const k = normEase(n);
    if (!known.has(k)) {
      known.add(k);
      extra.push(n.replace(/\s+/g, ''));
    }
  }
  return extra.length ? [...BASE_GROUPS, { label: 'More', names: extra }] : BASE_GROUPS;
}

export function normEase(n: string): string {
  return n.toLowerCase().replace(/[-_\s]/g, '');
}

// ---------------------------------------------------------------- evaluator (preview only)

const bez = (x1: number, y1: number, x2: number, y2: number) => (x: number) => {
  let t = x;
  for (let i = 0; i < 8; i++) {
    const cx = 3 * x1 * t * (1 - t) ** 2 + 3 * x2 * t * t * (1 - t) + t ** 3 - x;
    const dx = 3 * x1 * (1 - t) ** 2 + 6 * (x2 - x1) * t * (1 - t) + 3 * (1 - x2) * t * t;
    if (Math.abs(dx) < 1e-6) break;
    t = Math.min(1, Math.max(0, t - cx / dx));
  }
  return 3 * y1 * t * (1 - t) ** 2 + 3 * y2 * t * t * (1 - t) + t ** 3;
};
const outOf = (f: (t: number) => number) => (t: number) => 1 - f(1 - t);
const inoutOf = (f: (t: number) => number) => (t: number) => (t < 0.5 ? f(2 * t) / 2 : 1 - f(2 - 2 * t) / 2);
const pow = (p: number) => (t: number) => t ** p;
const backIn = (s: number) => (t: number) => t * t * ((s + 1) * t - s);
const bounceOut = (t: number) => {
  const n = 7.5625, d = 2.75;
  if (t < 1 / d) return n * t * t;
  if (t < 2 / d) return n * (t -= 1.5 / d) * t + 0.75;
  if (t < 2.5 / d) return n * (t -= 2.25 / d) * t + 0.9375;
  return n * (t -= 2.625 / d) * t + 0.984375;
};
const elasticOut = (a: number, p: number) => (t: number) => {
  if (t === 0 || t === 1) return t;
  const amp = Math.max(1, a);
  const s = (p / (2 * Math.PI)) * Math.asin(1 / amp);
  return amp * 2 ** (-10 * t) * Math.sin(((t - s) * 2 * Math.PI) / p) + 1;
};
const spring = (k: number, c: number, m = 1) => (t: number) => {
  // damped harmonic oscillator from 0 to 1 over a settle time normalised to the key span
  const w0 = Math.sqrt(k / m), z = c / (2 * Math.sqrt(k * m));
  const T = t * (z < 1 ? 6 / (z * w0) : 8 / w0);
  if (z < 1) {
    const wd = w0 * Math.sqrt(1 - z * z);
    return 1 - Math.exp(-z * w0 * T) * (Math.cos(wd * T) + ((z * w0) / wd) * Math.sin(wd * T));
  }
  return 1 - Math.exp(-w0 * T) * (1 + w0 * T);
};

const FAMS: Record<string, (t: number) => number> = {
  sine: (t) => 1 - Math.cos((t * Math.PI) / 2), quad: pow(2), cubic: pow(3), quart: pow(4), quint: pow(5),
  expo: (t) => (t === 0 ? 0 : 2 ** (10 * t - 10)), circ: (t) => 1 - Math.sqrt(1 - t * t), back: backIn(1.70158),
};

export function easeFn(name: string | null): ((t: number) => number) | null {
  if (!name) return (t) => t;
  const n = name.trim().toLowerCase();
  const args = (/\(([^)]*)\)/.exec(n)?.[1] ?? '').split(',').map((x) => parseFloat(x)).filter((x) => !Number.isNaN(x));
  const head = n.replace(/\(.*$/, '');
  if (head === 'linear' || head === 'none') return (t) => t;
  if (head === 'hold') return (t) => (t < 1 ? 0 : 1);
  if (head === 'steps') return (t) => Math.floor(t * (args[0] || 4)) / (args[0] || 4);
  if (head === 'cubic' && args.length === 4) return bez(args[0], args[1], args[2], args[3]);
  if (head === 'spring') return spring(args[0] ?? 170, args[1] ?? 26, args[2] ?? 1);
  if (head === 'snappy') return spring(260, 26);
  if (head === 'soft') return spring(120, 20);
  if (head === 'bouncy') return spring(180, 12);
  if (head === 'bounce') return bounceOut;
  let m = /^(in|out|inout|in-out)-(\w+)$/.exec(head);
  if (m && FAMS[m[2]]) {
    const f = m[2] === 'elastic' ? elasticOut(1, 0.3) : m[2] === 'bounce' ? bounceOut : FAMS[m[2]];
    return m[1] === 'in' ? f : m[1] === 'out' ? outOf(f) : inoutOf(f);
  }
  m = /^(power[1-4]|sine|expo|circ|back|elastic|bounce)(?:\.(in|out|inout))?$/.exec(head);
  if (m) {
    const kind = m[2] ?? 'out';
    let f: (t: number) => number;
    if (m[1].startsWith('power')) f = pow(+m[1].slice(5) + 1);
    else if (m[1] === 'back') f = backIn(args[0] ?? 1.70158);
    else if (m[1] === 'elastic') {
      const o = elasticOut(args[0] ?? 1, args[1] ?? 0.3);
      f = outOf(o);
    } else if (m[1] === 'bounce') f = outOf(bounceOut);
    else f = FAMS[m[1]];
    return kind === 'in' ? f : kind === 'out' ? outOf(f) : inoutOf(f);
  }
  return null;
}

/** SVG path of the curve in a w x h box (y up), with overshoot room. */
export function easePath(name: string | null, w: number, h: number, pad = 0.2): string | null {
  const f = easeFn(name);
  if (!f) return null;
  const pts: string[] = [];
  for (let i = 0; i <= 48; i++) {
    const t = i / 48;
    const v = f(t);
    const y = h - (pad * h + v * h * (1 - 2 * pad));
    pts.push(`${(t * w).toFixed(1)},${y.toFixed(1)}`);
  }
  return 'M' + pts.join(' L');
}
