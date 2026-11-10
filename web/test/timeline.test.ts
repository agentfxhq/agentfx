import { describe, expect, it } from 'vitest';
import { formatAnchor, formatDur, parseAnchor, parseDur } from '../src/model/time';
import { parseAnim, splitKey } from '../src/model/anim';
import { addKey, buildTimeline, deleteKey, moveEvent, moveKey, setKey, setProp, setSceneDur } from '../src/model/timeline';
import { lineTokens, patchSource } from '../src/model/patch';
import { formatSize, nativeOnlyFeatures, videoInfo, type ReelDoc } from '../src/model/reel';
import { CLI, timing, toJson, toText } from './cli';

const SRC = `video "Demo" size=1280x720 fps=30 captions=off formats=[16:9, 9:16]
// a comment that must survive timeline edits
scene intro dur=3s bg=#10121D
  text #word "Hello world" x=50% y=50% nowrap anchor=center size=120   // the hero
    anim opacity 0s:0 0.5s:1~out-cubic
  rect #bar x=50% y=70% w=200 h=8 fill=#3CBF87
  anim #bar sx 0.5s:0 1.2s:1~out-back
  @1.5s anim #word scale 0s:1 12f:1.2~spring(170,20) 0.8s:1 ; pulse #bar
scene outro
  say "Thanks for [[watching]] this."
    @watching+0.2s anim #bye y 0s:60% 0.4s:50%~power3.out
  text #bye "Bye" x=50% y=50%
`;

describe('time', () => {
  it('parses and formats durations keeping the unit', () => {
    expect(parseDur('1.5s')).toEqual({ seconds: 1.5, unit: 's' });
    expect(parseDur('300ms')!.seconds).toBeCloseTo(0.3);
    expect(parseDur('12f', 30)!.seconds).toBeCloseTo(0.4);
    expect(parseDur('50%', 30, 2)!.seconds).toBe(1);
    expect(parseDur('50%')).toBeNull();
    expect(formatDur(0.4, 'f', 30)).toBe('12f');
    expect(formatDur(0.25, 'ms')).toBe('250ms');
    expect(formatDur(1.0000001, 's')).toBe('1s');
    expect(formatDur(0.5, '%', 30, 2)).toBe('25%');
  });
  it('parses anchors', () => {
    expect(parseAnchor('@Klick+0.2s')).toEqual({ base: 'Klick', offset: { seconds: 0.2, unit: 's' } });
    expect(parseAnchor('offen.end-6f', 30)!.offset!.seconds).toBeCloseTo(-0.2);
    expect(parseAnchor('2.5s')).toEqual({ base: null, offset: { seconds: 2.5, unit: 's' } });
    expect(parseAnchor('bar:5+2f', 30)).toEqual({ base: 'bar:5', offset: { seconds: 2 / 30, unit: 'f' } });
    expect(formatAnchor({ base: 'x', offset: { seconds: -0.3, unit: 's' } })).toBe('x-0.3s');
    expect(formatAnchor({ base: 'x', offset: { seconds: 0.001, unit: 's' } })).toBe('x');
  });
});

describe('anim args', () => {
  it('splits keys with values that contain colons, hashes and eases', () => {
    expect(splitKey('0s:#fff~in-cubic')).toEqual({ time: '0s', value: '#fff', ease: 'in-cubic' });
    expect(splitKey('1.6s:560~elastic.out(1,0.3)')).toEqual({ time: '1.6s', value: '560', ease: 'elastic.out(1,0.3)' });
    expect(splitKey('#id')).toBeNull();
  });
  it('parses multi-channel anims', () => {
    const p = parseAnim(['#a', 'x', '0s:0', '1s:5', 'y', '0s:0', '1s:3~out-back', 'rot', 'wiggle(2,5deg,1)']);
    expect(p.target).toBe('a');
    expect(p.tracks.map((t) => [t.channel, t.keys.length])).toEqual([['x', 2], ['y', 2], ['rot', 0]]);
    expect(p.tracks[1].keys[1]).toMatchObject({ argIndex: 6, ease: 'out-back' });
    expect(p.tracks[2].expr).toBe('wiggle(2,5deg,1)');
  });
});

describe('patchSource', () => {
  it('token keys ignore quoting, number spelling and comments', () => {
    expect(lineTokens('  text #a "A" x=50% size=120.0   // c')).toBe(lineTokens('text #a A x=50% size=120'));
  });
  it('keeps comments and formatting of untouched lines', () => {
    const orig = 'video "T" fps=30\n// keep me\nscene a dur=2s   // and me\n  text #t "Hi"\n';
    const c0 = 'video T fps=30\nscene a dur=2s\n  text #t Hi\n';
    const c1 = 'video T fps=30\nscene a dur=2.5s\n  text #t Hi\n';
    expect(patchSource(orig, c0, c1)).toEqual({ text: 'video "T" fps=30\n// keep me\nscene a dur=2.5s   // and me\n  text #t "Hi"\n', normalized: false });
  });
  it('falls back to canonical text when lines do not match', () => {
    const r = patchSource('a\n  b ; c\n', 'a\n  b\n  c\n', 'a\n  b\n  d\n');
    expect(r.normalized).toBe(true);
    expect(r.text).toBe('a\n  b\n  d\n');
  });
});

describe('reel helpers', () => {
  it('derives format sizes from the short side', () => {
    const info = { title: '', width: 1280, height: 720, fps: 30, formats: [] };
    expect(formatSize(info, '9:16')).toEqual([720, 1280]);
    expect(formatSize(info, '1:1')).toEqual([720, 720]);
    expect(formatSize(info, '16:9')).toEqual([1280, 720]);
  });
});

describe.skipIf(!CLI)('timeline model through the CLI', () => {
  const doc: ReelDoc = toJson(SRC);
  const tm = timing(SRC);
  const tl = buildTimeline(doc, tm);

  it('reads the header and native-only features', () => {
    expect(videoInfo(doc)).toMatchObject({ width: 1280, height: 720, fps: 30, formats: ['16:9', '9:16'] });
    expect(nativeOnlyFeatures(doc)).toEqual([]);
  });

  it('builds scenes, rows, keys and events', () => {
    expect(tl.scenes.map((s) => [s.id, s.start, s.durProp])).toEqual([['intro', 0, '3s'], ['outro', 3, null]]);
    const word = tl.rows.find((r) => r.id === 'word')!;
    expect(word.kind).toBe('text');
    expect(word.tracks.map((t) => t.channel)).toEqual(['opacity', 'scale']);
    const scale = word.tracks[1].keys;
    expect(scale.map((k) => +k.t.toFixed(3))).toEqual([1.5, 1.9, 2.3]);
    expect(scale[1].unit).toBe('f');
    const bar = tl.rows.find((r) => r.id === 'bar')!;
    expect(bar.tracks[0].keys.map((k) => k.t)).toEqual([0.5, 1.2]);
    expect(bar.chips).toHaveLength(1);
    const evs = tl.events;
    expect(evs).toHaveLength(2);
    expect(evs[0]).toMatchObject({ t: 1.5, locked: false });
    // the word anchor resolves through the timing JSON (absolute), plus its offset
    const anchor = tm.timeline.scenes[1].anchors[0];
    expect(evs[1].t).toBeCloseTo(anchor.start + 0.2);
    const bye = tl.rows.find((r) => r.id === 'bye')!;
    expect(bye.tracks[0].keys[1].t).toBeCloseTo(anchor.start + 0.6);
  });

  it('moves a keyframe: JSON -> timeline -> edit -> JSON -> text -> JSON', () => {
    const key = tl.rows.find((r) => r.id === 'bar')!.tracks[0].keys[1];
    const d2 = moveKey(doc, key, 1.53, tl.fps); // snaps to the frame grid
    const text = toText(d2);
    expect(text).toContain('anim #bar sx 0.5s:0 1.533s:1~out-back');
    const back = buildTimeline(toJson(text), timing(text));
    expect(back.rows.find((r) => r.id === 'bar')!.tracks[0].keys[1].t).toBeCloseTo(1.533, 3);
  });

  it('keeps frame units and re-sorts keys dragged past a neighbour', () => {
    const k = tl.rows.find((r) => r.id === 'word')!.tracks[1].keys[1]; // 12f
    const d2 = moveKey(doc, k, 2.5, tl.fps); // past the 0.8s key
    const text = toText(d2);
    expect(text).toContain('anim #word scale 0s:1 0.8s:1 30f:1.2~spring(170,20)');
  });

  it('edits value and ease, adds and deletes keys', () => {
    const track = tl.rows.find((r) => r.id === 'word')!.tracks[0];
    let d2 = setKey(doc, track.keys[1], { value: '0.8', ease: 'power2.out' }, tl.fps);
    expect(toText(d2)).toContain('anim opacity 0s:0 0.5s:0.8~power2.out');
    d2 = addKey(doc, track, 1, '0.5', tl.fps);
    expect(toText(d2)).toContain('anim opacity 0s:0 0.5s:1~out-cubic 1s:0.5~out-cubic');
    d2 = deleteKey(doc, track.keys[0]);
    expect(toText(d2)).toContain('anim opacity 0.5s:1~out-cubic');
  });

  it('moves events, keeping word anchors', () => {
    const [plain, anchored] = tl.events;
    let d2 = moveEvent(doc, plain, 2.0, 0, tl.fps);
    expect(toText(d2)).toContain('@2s anim #word scale');
    d2 = moveEvent(doc, anchored, anchored.baseAbs - 0.1, 3, tl.fps);
    expect(toText(d2)).toContain('@watching-0.1s anim #bye');
    const back = buildTimeline(toJson(toText(d2)), timing(toText(d2)));
    expect(back.events[1].t).toBeCloseTo(anchored.baseAbs - 0.1, 3);
  });

  it('retimes a scene edge and sets props', () => {
    let d2 = setSceneDur(doc, tl.scenes[0], 4.5, tl.fps);
    let text = toText(d2);
    expect(text).toContain('scene intro dur=4.5s');
    expect(timing(text).timeline.scenes[1].start).toBeCloseTo(4.5);
    d2 = setProp(d2, tl.rows.find((r) => r.id === 'bar')!.path!, 'fill', '#FF0000');
    text = toText(d2);
    expect(text).toContain('fill=#FF0000');
  });

  it('patches the original text: comments survive a timeline edit', () => {
    const key = tl.rows.find((r) => r.id === 'bar')!.tracks[0].keys[0];
    const d2 = moveKey(doc, key, 0.7, tl.fps);
    const r = patchSource(SRC, toText(doc), toText(d2));
    expect(r.normalized).toBe(false);
    expect(r.text).toContain('// a comment that must survive timeline edits');
    expect(r.text).toContain('size=120   // the hero');
    expect(r.text).toContain('anim #bar sx 0.7s:0 1.2s:1~out-back');
    expect(r.text.split('\n')).toHaveLength(SRC.split('\n').length);
    // and the patched text parses to the edited JSON
    expect(toJson(r.text)).toEqual(toJson(toText(d2)));
  });
});
