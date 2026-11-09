// Durations and anchor expressions of Reel Script (docs/SPEC.md §4, §6).

export type Unit = 's' | 'ms' | 'f' | 'bare' | '%';

export interface Dur {
  seconds: number;
  unit: Unit;
}

const DUR_RE = /^(-?\d+(?:\.\d+)?|-?\.\d+)(s|ms|f|%)?$/;

/** Parse `1.5s`, `300ms`, `12f`, `1.5`, `50%` (of `pctOf` seconds). Numbers pass through. */
export function parseDur(v: unknown, fps = 30, pctOf?: number): Dur | null {
  if (typeof v === 'number' && Number.isFinite(v)) return { seconds: v, unit: 'bare' };
  if (typeof v !== 'string') return null;
  const m = DUR_RE.exec(v.trim());
  if (!m) return null;
  const n = parseFloat(m[1]);
  switch (m[2]) {
    case 's':
      return { seconds: n, unit: 's' };
    case 'ms':
      return { seconds: n / 1000, unit: 'ms' };
    case 'f':
      return { seconds: n / fps, unit: 'f' };
    case '%':
      return pctOf === undefined ? null : { seconds: (n / 100) * pctOf, unit: '%' };
    default:
      return { seconds: n, unit: 'bare' };
  }
}

/** Shortest decimal for a number (no float noise, at most `digits` decimals). */
export function num(n: number, digits = 3): string {
  const r = Math.round(n * 10 ** digits) / 10 ** digits;
  const s = digits > 0 ? r.toFixed(digits).replace(/\.?0+$/, '') : r.toFixed(0);
  return s === '-0' ? '0' : s;
}

/** Format seconds in the given unit (keeps the author's spelling style). */
export function formatDur(seconds: number, unit: Unit, fps = 30, pctOf?: number): string {
  switch (unit) {
    case 'ms':
      return `${num(seconds * 1000, 0)}ms`;
    case 'f':
      return `${Math.round(seconds * fps)}f`;
    case '%':
      return pctOf ? `${num((seconds / pctOf) * 100, 2)}%` : `${num(seconds)}s`;
    case 'bare':
      return num(seconds);
    default:
      return `${num(seconds)}s`;
  }
}

export function snap(t: number, fps: number): number {
  return Math.round(t * fps) / fps;
}

/** Anchor expression: `2.5s`, `Reiter`, `Reiter.end+0.2s`, `start`, `end-1s`, `prev+0.3s`. */
export interface Anchor {
  base: string | null; // null = scene time
  offset: Dur | null; // offset (or the time itself when base is null)
}

// word anchors, `start`/`end`/`prev`, and the music grid (`beat:4`, `bar:5`)
const ANCHOR_RE = /^((?:beat|bar):\d+(?:\.\d+)?|[A-Za-z_À-￿][\wÀ-￿-]*(?:\.end)?)(?:([+-])(.+))?$/;

export function parseAnchor(expr: string, fps = 30): Anchor | null {
  const e = expr.trim().replace(/^@/, '');
  const d = parseDur(e, fps);
  if (d) return { base: null, offset: d };
  const m = ANCHOR_RE.exec(e);
  if (!m) return null;
  if (!m[2]) return { base: m[1], offset: null };
  const off = parseDur(m[3], fps);
  if (!off) return null;
  if (m[2] === '-') off.seconds = -off.seconds;
  return { base: m[1], offset: off };
}

export function formatAnchor(a: Anchor, fps = 30): string {
  if (a.base === null) return formatDur(a.offset?.seconds ?? 0, a.offset?.unit === 'bare' ? 's' : a.offset?.unit ?? 's', fps);
  const off = a.offset;
  if (!off || Math.abs(off.seconds) < 0.5 / fps) return a.base;
  const unit = off.unit === 'bare' ? 's' : off.unit;
  return `${a.base}${off.seconds < 0 ? '-' : '+'}${formatDur(Math.abs(off.seconds), unit, fps)}`;
}

export function fmtClock(t: number, fps = 30, frames = false): string {
  const s = Math.max(0, t);
  const m = Math.floor(s / 60);
  const sec = s - m * 60;
  if (frames) {
    const whole = Math.floor(sec + 1e-6);
    const f = Math.round((sec - whole) * fps);
    return `${m}:${String(whole).padStart(2, '0')}:${String(f).padStart(2, '0')}`;
  }
  return `${m}:${sec.toFixed(2).padStart(5, '0')}`;
}
