// `anim [#id] <channel> <t>:<v>[~ease] ... [<channel> ...]` (docs/SPEC.md §8 Keyframes).

export interface KeyTok {
  argIndex: number;
  time: string; // `0.6s`, `12f`, `50%`, `@Klick`
  value: string;
  ease: string | null;
}

export interface AnimTrack {
  channel: string;
  keys: KeyTok[];
  expr: string | null; // `wiggle(2, 5deg, 1)` style value lists
  channelArgIndex: number;
}

export interface ParsedAnim {
  target: string | null; // id without '#'
  tracks: AnimTrack[];
}

const CHANNEL_RE = /^[A-Za-z][\w-]*$/;

export function splitKey(tok: string): { time: string; value: string; ease: string | null } | null {
  if (tok.startsWith('#')) return null;
  // `@anchor:value` keys: the anchor may not contain ':' except music-grid anchors (`beat:2`).
  const i = tok.indexOf(':', tok.startsWith('@beat:') || tok.startsWith('@bar:') ? tok.indexOf(':') + 1 : 0);
  if (i <= 0) return null;
  const time = tok.slice(0, i);
  const rest = tok.slice(i + 1);
  const j = rest.indexOf('~');
  return j < 0 ? { time, value: rest, ease: null } : { time, value: rest.slice(0, j), ease: rest.slice(j + 1) };
}

export function joinKey(k: { time: string; value: string; ease: string | null }): string {
  return `${k.time}:${k.value}${k.ease ? '~' + k.ease : ''}`;
}

export function parseAnim(args: string[]): ParsedAnim {
  let target: string | null = null;
  const tracks: AnimTrack[] = [];
  let cur: AnimTrack | null = null;
  args.forEach((a, i) => {
    if (i === 0 && a.startsWith('#')) {
      target = a.slice(1);
      return;
    }
    const k = splitKey(a);
    if (k && cur) {
      cur.keys.push({ argIndex: i, ...k });
    } else if (!k && CHANNEL_RE.test(a)) {
      cur = { channel: a, keys: [], expr: null, channelArgIndex: i };
      tracks.push(cur);
    } else if (cur && !k) {
      cur.expr = a;
    }
  });
  return { target, tracks };
}
