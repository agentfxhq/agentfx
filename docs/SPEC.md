# Reel Script: spec v1.1 (S1 + design layer D1)

Reel Script is a text format for information videos (explainers, product and UI walkthroughs,
data stories) that LLMs and people write, and that compiles to video. One line per idea, time
anchored to narration, layout by constraints, every error explainable.

- Text form: `*.reel` (this document). JSON form: 1:1, same AST (`schema/reel-script.schema.json`).
- Tooling: `agentfx parse | lint | timing | ir [--target reel-ir] | html | frame | sheet [--events] | render | catalog | schema`.
- Status marks: **implemented**, **partial**, **planned** (specified, rejected or ignored with a
  warning, never faked). Generated table: [Capability catalog](#15-capability-catalog).
- S1: the compiler owns layout (native, no browser): `lint --measure`, `ir`, `frame`, `sheet` and
  `render` run without Chrome; `render` goes Reel Script -> reel-ir -> the reel renderer. The
  HTML/Chrome backend stays as preview and reference (`--backend chrome`).

## 1. Design rules for LLM writability

1. **Few tokens.** Defaults come from the theme; one line carries an element or an event.
2. **Forgiving where safe:** tabs count as two spaces; bare words need no quotes; `0.4`, `0.4s`
   and `12f` are all durations; ease names ignore case and `-`/`_`; `#id` and hotspot ids share one
   namespace per scene; unknown props on known kinds are ignored by the HTML backend but flagged by
   lint.
3. **Strict where it matters:** ids, anchors, presets, channels, colors and tokens are checked.
   Timing overruns, events outside their scene, overflowing text and out-of-frame boxes are errors.
4. **Every diagnostic has a fix hint**, usually a "did you mean" (Damerau-Levenshtein over the
   valid names in scope) or the exact syntax to write. Diagnostics are JSON:
   `{code, severity, message, hint, line, col, id?, at?, format?}`.
5. **Semantic before geometric.** Prefer `zoom #tabs` to coordinates, `place=bottom` to `y=930`,
   `@Reiter` to `3.47s`.

## 2. Three levels

| Level | For | Examples |
|---|---|---|
| **story** | default for agents: semantic verbs on ids, anchored to narration words | `@Reiter zoom #tabs ; click #tab-content` |
| **motion** | After Effects / Motion class: layers, channels, keyframes, easings incl. springs, effects, 2.5D camera, generators | `anim #logo scale 0s:0.8 0.6s:1~spring(170,20)` |
| **raw** | escape hatches | `svg |`, `html |`, `image`, `video` |

The story level compiles into the motion level (verbs become channel tracks); the motion level is
the compiler's IR (`agentfx ir`), which maps onto the renderer IR (see `IR-MAPPING.md`).

## 3. Surface syntax

```
video "What's new" size=1920x1080 fps=30 voice=en_US-ljspeech-medium rate=+6%

scene panels title="Panels" pre=40f
  browser #app url=studio.example.test shot=assets/ui/changes.png
  say "The side [[panel]] now has tabs. [[Counts]] show where something is open." min=4.5s
    @panel   zoom x=0.84 y=0.33 z=1.9 ; cursor -> #tab-history
    @Counts  pulse #tab-changes ; pulse #tab-content
  code #snip lang=ts |
    const a = 1
```

Grammar (informal):

```
file      := line*
line      := INDENT statement (';' statement)* [comment]       // one idea per line
statement := '@' anchor verb-statement?                        // event: verbs become children
           | kind token*  ['|']                                 // '|' starts a raw body
token     := key '=' value | value
value     := "quoted" | [list, of, values] | bare             // bare may contain (...) with spaces
comment   := '//' to end of line (at line start or after whitespace)
```

- **Nesting by indentation**: deeper lines are children. Elements nest into layout containers;
  `@anchor` lines nest under `say` (anchors of that sentence) or directly under `scene`.
- **Ids**: the first `#name` argument of an element declares its id; in a verb it is the target.
- **Raw body**: `kind ... |` takes all deeper-indented lines verbatim (dedented). A body node has no
  children (so a `when` block written under it becomes body text; write it at the parent's level).
- **Values keep their spelling** in the AST (`"0.4s"`, `"#17845a"`, `"$accent"`); typing happens
  against the catalog, which is where errors with hints come from.

### JSON form

Each node is `{kind, args?, props?, body?, children?}`; a document is `{"reel":"0","nodes":[...]}`.
Event anchors are nodes of kind `at` with `args[0]` = the anchor expression.
`agentfx parse x.reel` prints JSON; `agentfx parse x.json` prints canonical text. Both directions are
lossless except comments (round-trip tests in `tests/parse_roundtrip.rs`).

```json
{"kind":"say","args":["Ein [[Klick]] wechselt den Reiter."],"props":{"min":"3s"},
 "children":[{"kind":"at","args":["Klick"],"children":[{"kind":"click","args":["#tab-content"]}]}]}
```

## 4. Values

| Type | Forms | Notes |
|---|---|---|
| duration | `1.5s` `300ms` `12f` `1.5` | `f` = frames at the video fps |
| anchor | `Reiter` `Reiter.end` `Reiter+0.2s` `offen.end-6f` `2.5s` `start` `end-1s` `prev+0.3s` | word anchors from `[[...]]`, scene-relative times, previous event |
| color | `#rgb` `#rrggbbaa` `rgb(23 132 90 / 50%)` `hsl(...)` `white` `$accent` | `$name` = theme token |
| paint | color, `linear(155deg, #a 0%, #b 100%)`, `radial(at 30% 40%, ...)`, `conic(0deg, ...)`, `mesh(2x2, a, b, c, d)`, `image(path cover)` | stops may exceed 100% |
| ease | `linear` `hold` `steps(n)` `in/out/inout[-sine/-quad/-cubic/-quart/-expo/-back]` `cubic(x1,y1,x2,y2)` `spring(k, c[, m[, v0]])` `spring` `snappy` `soft` `bouncy` `bounce(n)` `$pop` | springs stay physical in the IR |
| length | `120` `120px` `50%` `10vw` `2em` `$h1` | `%` of the parent box |
| keyframe | `<time>:<value>[~<ease>]` | time = duration, `%` of `dur=`, or `@anchor` |
| size / aspect | `1920x1080`, `16:9` | |

## 5. Document

| Node | Meaning |
|---|---|
| `video "Title" size= fps= voice= rate= theme= captions=plain\|karaoke\|off\|<caption style> formats=[..] lead= tail=` | header (caption styles: [Caption pack](#caption-pack-word-by-word-social-captions)) |
| `theme <builtin>` + children `colors k=v`, `type k=px min=`, `motion k=dur\|ease`, `layout safe= safe-x= safe-y= radius=`, `lint min-contrast= no-emdash=`, `bg <paint>` | brand tokens |
| `pronounce "written" "spoken"` | spoken substitution; captions keep the written form |
| `music src= vol= duck= bpm=` | music bed, ducked under narration; drives [audio reactivity](#audio-reactivity) (`bpm=` overrides the detected tempo) |
| `chapterbar label= scenes=[..]` | progress overlay for titled scenes |
| `font "Brand" src="fonts/Brand.woff2"` | custom font file (TTF, OTF, TTC, WOFF, WOFF2; path relative to the .reel file) registered under the alias for `font=Brand` ([Text v2](#text-v2)) |
| `scene <id> title= in=<transition> tdur= pre= post= dur= min= bg= nocaptions` | a shot |
| `captions src= audio= style= ...` (in a scene) | caption pack: transcript beat or narration restyle (§7) |

## 6. Timing model

1. **Narration drives length.** A scene is `pre` + its beats + `post` (default 0.5 s). A beat is a
   `say` (lead + spoken audio + tail, at least `min=`) or a `pause <dur>`. Scenes run back to back.
2. **Word timings** come from the TTS engine (`--tts piper|edge|none` or `$AGENTFX_TTS`; default
   `piper`, local, voice `en_US-ljspeech-medium`, see docs/INSTALL-NOTES.md), cached per sentence
   hash (`cache/tts/<sha>.mp3|json`). Written words are aligned to spoken words by edit-distance
   DP, so `pronounce` substitutions (1 written word, 5 spoken) and engine tokenization differences
   map back correctly. With `--tts none` / `--no-tts`, or when the engine is not installed
   (`I_TTS_NONE`), the video is silent and words are **estimated** (15 chars/s scaled by `rate`);
   lint reports `I_TTS_ESTIMATED`. `edge` (Microsoft Edge read-aloud via `edge-tts`) is opt-in only.
3. **Events anchor to words.** `[[word]]` or `[[several words|name]]` in a `say` defines an anchor.
   `@name` events resolve to the word start (`.end` for its end) plus an offset. Lookups prefer the
   enclosing `say`, then the scene; unknown names fail with suggestions.
4. **Explicit times override**: `@2.5s` (scene time), `dur=` on a scene (error `E_TIMING_OVERRUN`
   when narration does not fit), `at=@x` / `until=@y` on elements.
5. **Verb timing conventions**: `click` lands *on* its anchor (the cursor travels before it);
   `cursor`, `zoom`, `enter` start at it. Later events win on a channel and start from its current
   value, so overlapping moves never jump.
6. Edit a sentence and everything anchored to it reflows; nothing else moves.

## 7. Story level

### Elements

Layout is flex/grid inside the scene's safe area; captions reserve their own band. Common props on
every element: `w h maxw x y anchor place=top|bottom|left|right|center|top-left|... pad gap align
justify grow aspect fill stroke sw radius border shadow glow glow-color blur opacity rot scale z
blend mask mask-mode origin hud mblur shutter` and timing props `in=<preset> at=@x out=<preset> until=@y dur=
delay= stagger=`. Text props: `style size color weight align upper tracking lh nowrap font wght
axes features stroke sw stroke-over on offset split fit` ([Text v2](#text-v2)). Inline text markup: `**bold**`, `{accent}`, `` `code` ``.

Kinds: `text h1 h2 h3 display body small label title stack row col grid box card node diagram spacer
image video svg html icon browser phone terminal code chart counter particles cloud layer group rect
ellipse circle path line poly star lower3 toast chapter link camera stat note steps pill quote list
window device scene3d` (details in the catalog). Inside a `scene3d`: `box cube plane floor sphere
cylinder cone torus extrude mesh screen group light` ([Stylised 3D](#stylised-3d-scene3d)).

### Component library (S1)

Designed by default, on the theme's tokens (colors, radius, type scale); props are optional.
Write the component, not the styling: no `fill=`/`shadow=`/`radius=` needed.

| Component | Syntax | Looks like |
|---|---|---|
| `card` | `card #c "Title" sub="Body" icon=bolt kicker="NEU" variant=surface\|glass\|accent\|outline` | layered surface, icon tile on the accent gradient, kicker, title, body. **Cards (and stats) in a `row` share its full width and equal height**; in portrait they stack full width. |
| `stat` | `stat #s value=12480 from=0 label="Nutzer" suffix=" %" delta="+12 %" sub="seit Juli" icon=user spark=[3, 5, 8] in=count` | KPI tile: label, big counting number, delta pill (red when it starts with `-`), sparkline. `count #s to=..` works on the tile. |
| `note` | `note #n "Text" title="Heads up" kind=info\|tip\|warn\|danger\|success` | callout box, colored accent bar, tinted icon. |
| `node` | `node #a "Label" sub="Detail" icon=db variant=default\|primary\|muted status=ok\|warn\|err` | diagram node (in a `diagram`, joined with `link #a -> #b`). |
| `chart` | `chart #c type=bar\|line\|area\|pie\|donut data=[..] labels=[..] unit=k highlight=4` | y axis with nice ticks, gridlines, value labels, x labels; highlighted bar/point in mint with glow; donut shows the largest slice in its center and a legend. `grid=false`, `axis=false` turn parts off. |
| `steps` | `steps #p "Plan" "Build" "Ship" active=2` | numbered circles with connectors (done ones filled). |
| `list` | `list #l "Eins" "Zwei" icon=check` | icon list; `in=rise stagger=150ms` staggers the items. |
| `quote` | `quote #q "Text" by="Name, Rolle"` | pull quote with a large accent quote mark. |
| `pill` | `pill "Neu" color=$accent2` | badge. |
| `lower3` | `lower3 #l "Name" sub="Rolle" icon=user in=left` | glass lower third with accent bar. |
| `window` | `window #w title="Dashboard" w=900 h=520` + child elements | app window mock; children go into its body. |
| `device` | `device #d type=laptop\|tablet\|phone shot=app.png` | device frame around a screenshot (use `browser`/`phone` when you need hotspots). |

`activate #a for=2s` / `deactivate #a` switch a `node`, `card` or `stat` into its accent state
(gradient, glow, slight pop): mark the active step of a diagram while it is spoken.
Charts format numbers like the narration: decimal comma by default (`locale=en` for points).

For anything the library cannot express, use a raw `html` block (§9).

### Caption pack (word-by-word social captions)

```
scene talk pre=0.2s
  captions src=talk.json audio=talk.mp3 style=hormozi        // transcript + prerecorded voice
scene explain
  say "Every [[format]] gets its own safe area."
  captions style=pill size=60                                // restyle this scene's narration
video "x" captions=pop                                       // header: every narrated scene, style pop
```

- **Sources.** Without `src=` the node styles the scene's narration (`say`, TTS word timings).
  With `src=` it is a timing beat like a `say` with prerecorded audio: `.json` = Whisper JSON
  (openai-whisper / faster-whisper / WhisperX `segments[].words[]`, whisper.cpp `-oj`/`-ojf`
  `transcription[].tokens[]` with sub-word pieces merged, or a bare `[{word, start, end}]` array),
  `.srt`, `.vtt` (inline `<00:00:01.200>` word times are used; YouTube-style repeated lines are
  dropped). SRT/VTT cues without word times spread their words over the cue by length (info
  `I_CAPTION_ESTIMATED`). `audio=` plays the file at the beat (as voice: music ducks under it),
  `gain=`, `pre=`/`post=` pad the beat, `offset=` shifts the words against the audio (may be
  negative), `#id` makes the whole transcript an anchor. Paths are relative to the `.reel` file.
  `tools/transcribe.sh talk.wav -o talk.json --lang en` writes Whisper JSON with word times using
  a local faster-whisper (no install; it finds an environment that has it, or `AGENTFX_WHISPER_PY`).
- **Styles** (`style=`, default `karaoke`; the header `video captions=<style>` applies one to
  every narrated scene, except `plain`/`karaoke`/`off`, which keep the inline legacy captions):

  | Style | Look |
  |---|---|
  | `karaoke` | whole phrase, each word switches to the highlight color as it is spoken and stays; thin outline |
  | `pop` | words appear as spoken with a scale bounce, the active word in the highlight color; center |
  | `pill` | whole phrase, a rounded pill (`box=`, default `$accent`) pops behind the active word |
  | `hormozi` | 2 lines, uppercase, weight 900, thick outline + shadow, phrase pops in, active word yellow and slightly larger, one keyword per sentence green (`key=`); center |
  | `typewriter` | mono font on dark line boxes, each word typed in letter by letter over its spoken time |
  | `rise` | words fade up into place as spoken |
  | `outline` | classic subtitles: phrase at once, white with black outline and shadow |
  | `safe` | plain text on translucent line boxes, no per-word motion (readable over any footage) |

- **Props**: `font=` (a registered family or `inter mono serif rounded grotesk news display`),
  `size= weight= lh= tracking= upper color= hi=` (highlight) `key=` (keyword color) `box=`
  (pill/line box) `stroke=<color>|none sw=<px> shadow=on|off`, `place=bottom|center|top` with `y=`
  (fraction of the frame where that edge of the block sits) and `w=` (line box width, fraction),
  `maxchars=` per line, `lines=` max lines, `maxwords=` per phrase, `gap=` (pause that starts a new
  phrase, 0.45s), `lead=`/`hold=` (show before the first / after the last word), `keywords=[..]`
  (also every `[[anchor]]` word of the narration), `autokey=` (hormozi: on), `cues=false` (let
  phrases cross source cues), `minword=`/`mincue=` (read-time lint), `emoji=keep`.
  Defaults depend on the format: 9:16 keeps the block above the platform UI band (bottom 20 %)
  and inside the middle 72 % of the width (clear of the right action rail), 1:1 and 16:9 sit
  lower with wider lines.
- **Phrases and lines.** Words group into phrases at pauses longer than `gap`, sentence ends,
  source cue boundaries, clause punctuation once a phrase is half full, `maxwords`, and capacity
  (on a capacity break a trailing clause moves to the next phrase). A single leftover word joins
  the phrase before it when there is no pause. Each phrase is broken into the fewest lines that
  fit `w`/`maxchars`/`lines`, then the most balanced split (smallest widest line), bottom-heavy
  on ties. Widths are measured with the same fonts direct mode registers (Parley), so the boxes
  match the render. A phrase shows from `lead` before its first word until `hold` after its last
  (never overlapping the next), stretched toward `mincue` when there is room.
- **Rendering**: composite-stage only. Every word is a positioned layer (its own group); a
  highlight is a second text layer in `hi` crossfaded over the base (no color tracks), outlines
  are four crisp drop shadows (a 3x3 dilation) plus a soft shadow, pills and line boxes are
  rounded rects, typewriter uses a stepped left wipe. Characters the font has no glyph for (all
  emoji: there is no color emoji font yet) are dropped with `W_CAPTION_GLYPH`.
- **Lint**: `E_CAPTION_SRC` (missing/unreadable/unrecognized file), `E_CAPTION_STYLE` (unknown style,
  did you mean), `W_CAPTION_PROP`, `W_CAPTION_FONT`, `E_CAPTION_OVERFLOW` (a word wider than the
  line box at this format), `E_CAPTION_OFF_FRAME`, `W_CAPTION_SAFE` (outside the format's social
  safe area), `W_CAPTION_READ` (a phrase shows less than `max(mincue, words x minword)`, defaults
  0.7 s, 0.5 s for `pop`/`hormozi` whose words are read as spoken, and 160 ms), `W_CAPTION_GLYPH`, `I_CAPTION_ESTIMATED`. Run `agentfx lint --all-formats`.
  Examples: `examples/captions/`.

### Layout-aware verbs (S1)

- `zoom #element` (any laid-out element, not only screenshot hotspots): the scene camera centers the
  element and fits it to ~60 % of the frame (`z=` overrides); `zoom out` returns.
- `move #a to=#b`: moves `#a` so its center lands on `#b`'s center (`x=`/`y=` add an offset).
- `text ... split=line`: one layer per *laid-out* line (the compiler breaks the lines itself), so
  `in=rise stagger=` reveals line by line.

**Screenshots with hotspots** (`browser`, `phone`): `shot=x.png` plus `x.json` (`{"id":[x,y,w,h]}`
in shot pixels; `hotspots=` overrides the path; `hot #id x y w h` adds one inline). Hotspot ids are
verb targets like element ids. `swap #app shot=next.png` changes the UI state and its hotspot set.
This is the natural way to show a real product: capture states once, narrate over them.

### Verbs

`enter exit show hide emph pulse shake glow bounce flash highlight spotlight callout zoom pan cursor
click dblclick drag type swap count draw diff burst move set anim rewind timemap slitscan echo timedisplace
freeze pour stir ink reveal dolly orbit sfx truck pedestal lookat camera3d spin tumble assemble explode
stack` (table in
the catalog). Enter presets: `fade rise drop left right pop zoom blur wipe iris draw type flip count
none`; emphasis: `pulse shake glow bounce wobble flash`.

## 8. Motion level

- **Layers**: every element is a layer; `layer`/`group` are explicit precomps with absolute
  placement. Transforms compose through the tree.
- **Channels** (animatable): `x y z px py scale sx sy rot rotx roty opacity blur glow brightness
  saturate contrast hue fill color stroke trim-start trim reveal mask-x grad-angle grad0..3 wght
  (`font-weight`) wdth opsz slnt <AXIS> path-offset (`offset`) tracking w h count halftone sweep rays
  misreg`, 3D: `x y z rx ry rz scale sx sy sz opacity color emissive intensity` on objects and
  `cam-x cam-y cam-z look-x look-y look-z fov roll ambient` on a `scene3d`, where `<AXIS>` is a custom uppercase variable-font axis tag (`GRAD`, `SOFT`, ...). Each has a pipeline stage (composite / paint / layout); see the catalog.
  Direct mode: `brightness saturate contrast hue` are a reel-ir `color` effect (CSS filter
  semantics; static grading props `tint= tint-amount= levels= lut=` join it), `fill`/`stroke`
  on paths are reel-ir color tracks, other color/gradient/layout channels become repaint states.
- **Keyframes**: `anim #id <channel> <t>:<v>[~ease] ... [<channel> ...] [ease=] [repeat=n] [yoyo]
  [dur=]`. Times are relative to the event (or element start); `@anchor:value` keys are absolute.
  As a child of an element, `anim` targets that element.
- **Expressions**: `wiggle(freq, amp, seed)`, `sin(freq, amp)` as a keyframe value list, sampled
  into keys (pure functions of time, seeded).
- **Springs**: `spring(stiffness, damping, mass, velocity)`, closed form in the native IR; the HTML
  backend samples it into CSS `linear()` (approximation, sub-pixel at 30 fps).
- **Motion blur**: `mblur=<samples>|on|off shutter=<angle>|<n>f` on any element (or on `anim`, which
  sets it on the target): the layer's subtree is rendered at `samples` sub-frame times spread over
  the shutter, centered on the frame time, and averaged (reel-ir `motion_blur`). Shutter as a film
  camera's angle: `180deg` = half a frame (default), `360deg` = one frame, at most `4f` (1440deg);
  `mblur=on` = 16 samples. Lint: `E_VALUE` for samples outside 2..64 or a shutter outside (0, 4f].
  Costs a per-frame re-render of the subtree while it moves within the shutter (renderer damage =
  swept ink); at rest it costs nothing. Direct mode only: the HTML/Chrome backend ignores it.
  `mblur=16 shutter=180deg` ; `anim #dot y 0s:0 0.25s:600 mblur=24 shutter=1f`.
- **Time remap**: a group's local time as a function of its parent's time. Everything inside plays
  through it: tracks with any ease (springs, steps, cubic), `split=` staggers, trim draw-on
  (un-draws when reversed), particles (seeded, re-gather exactly; a `particles` element is timed
  with the group it is declared in), repeats/yoyo, counters.
  - `rewind #g speed=4 dur=0.5s [then=play|hold] [ease=]`: local time runs backwards at `speed`x
    for `dur`, then plays on forward from there (`then=play`, default) or holds (`then=hold`).
    `speed=0` freezes for `dur`.
  - `timemap #g <dt>:<dl>[~ease] ... [then=play|hold]`: piecewise local time. `dt` is the parent
    time since the event (`0.5s`, `12f`, `50%` of `dur=`, or `@anchor`), `dl` the local time offset
    from where the subtree's clock stood at the event (may be negative). Linear between keys unless
    a key has an ease; a jump at `0s` cuts. After the last key: `then=play` (slope 1) or `hold`.
    `timemap #g 0s:0s 0.3s:0s 0.9s:-2.4s then=hold` = hold 0.3 s, scrub 2.4 s back at 4x, hold.
  - `#scene` targets the scene's world (all elements and the camera; captions and the scene
    transition are not remapped). Remaps nest: an inner remap's event times are authored times, so
    they fire when the outer local time reaches them, and an outer rewind replays inner ones
    backwards. A later remap event on the same group continues from the local time reached.
  - Compiled, not rendered: every track of the subtree is evaluated through the composed remap at
    every output frame (from the first remap event on) and baked into linear per-frame keys; keys
    before it stay symbolic. Frames are exact (a rewound frame evaluates to the same reel-ir values
    as its mirrored forward frame, `tests/remap.rs`); between frames (motion-blur sub-samples) the
    baked track is linear. Damage follows the values: remapped frames change when what they show
    changes, a frozen remap (`then=hold`, `speed=0`) produces no damage.
- **Time effects** (X3): the renderer evaluates a group's subtree at other times than the frame's
  (frame n is a pure function of n) and combines the renders (reel-ir node `time`). One effect per
  layer; `span` (echo: `dt`) is the channel `time-span`: the first verb switches the effect on at
  its event time (span 0 before, i.e. the plain group), later verbs on the same layer change the
  span, instantly or over `dur=` with `ease=`; `span=0` = sharp again. Direct mode only.
  - `slitscan #g axis=y span=1.2s [band=4] [dir=down|up]`: the row at fraction u of the group's
    box (axis y; `axis=x`: columns, `dir=right|left`) shows the group as it was `span * u` earlier
    (`dir=up`: u runs bottom to top). Time is sampled every `band` px (1..64) and cross-faded.
    `@beat slitscan #g span=0 dur=0.15s` snaps it sharp.
  - `echo #x n=6 dt=60ms decay=0.6 [blend=normal|add|multiply|screen] [colors=c1,c2]`: `n` (1..32)
    copies at `t - k*dt` under the element, copy k at opacity `decay^k`, tinted from `c1` (newest)
    to `c2` (oldest), composited oldest first with `blend`. Covers everything (springs, split text,
    clouds, remaps) because the subtree is rendered at those times.
  - `timedisplace #g map=radial|linear|noise|#layer span=0.8s [levels=24]`: pixel p shows the
    group `span * map(p)` earlier, map 0..1 quantized to `levels` (2..64) and cross-faded. `radial`
    `center=0.5,0.5` (box fractions) `radius=` (px, default the farthest corner), `linear angle=0`
    (deg, delay grows along it), `noise scale=160 seed=1`, `#layer`: that element's luma (black =
    now; the element becomes hidden, it can be animated).
  - `freeze #x [for=1s]`: the subtree's local time stops while the rest plays (a hold remap,
    baked like `timemap`); with `for=` it resumes from where it stopped.
  - Cost: per frame the subtree is rendered once per distinct sample state (rests are cached and
    shared); slit-scan ~2x the box area in strips, echo n+1 renders of each copy's ink, displace ~2x
    the box area in tiles. Lint `W_TIME_COST` above 400 subtree renders per frame, `W_TIME_PROP`
    for unknown props (did-you-mean), `E_VALUE` for bad values. Damage: the group's ink swept over
    the sampled window; frames where nothing in the window moves cost nothing.
  `slitscan #word axis=y span=1.2s` ; `echo #ball n=6 dt=50ms decay=0.7 blend=add colors=#ff3b30,#2979ff` ;
  `timedisplace #grid map=radial span=0.8s levels=24` ; `freeze #clock for=1.5s`.
  - Not remapped: audio (narration, music, sfx), `video` layers, grain. A step edge that lands exactly
    on a frame time can flip by float rounding (as in any render). `bounce(n)` eases: the renderer
    approximates them with a cubic in symbolic tracks, so a rewound bounce frame matches the
    compiler's exact curve, not the approximation.
- **Camera (2.5D)**: `camera persp=1100` in a scene; layers get `z=`; `anim #camera x/y/z/rotx/roty`,
  `dolly`, `orbit` move it, depth gives parallax. Use `hud` for elements that must not move with the camera.
- **Generators**: `chart` (bar/line/area/pie/donut from inline data), `counter`, `code` (highlight +
  `diff`), `particles` (seeded SplitMix64, exact projectile paths), `link` connectors between ids,
  `terminal` (typed commands), `chapter`, `lower3`, `toast`, captions with karaoke.
- **Fluid** (`fluid`, X5): a 2D ink/smoke layer driven by a stable-fluids simulation (Jos Stam:
  semi-Lagrangian advection, pressure projection, vorticity confinement) on a grid at `res=` of
  the box (default 1/4), upsampled with a slightly blurring bicubic. The renderer simulates once,
  sequentially, and caches the dye per frame (u8 per grid cell), so frame n is a pure function of
  n: parallel segments, `agentfx frame --at` and sheets show identical frames (a cut-out range
  simulates from 0, its ink sources stay alive). Direct mode only (reel-ir `fluid`, reel `IR.md`).

  ```
  fluid #ink res=1/4 viscosity=0.0005 dissipation=0.99 vorticity=12 seed=3 color=#E8442E shade=0.3
  @0.2s pour #ink at=30%,80% dir=-80deg speed=900 radius=3% rate=4 dur=1.5s
  @0.6s stir #ink at=50%,45% radius=20% strength=5 dur=1s
  @1s ink #title into #ink dur=0.5s noise=500
  reveal #shot with #ink
  ```

  - **Element**: `res=` grid cells per px (`1/4`, `0.25`; 1/20..1), `viscosity=` (box-widths²/s,
    default 0.0004: damps grid-scale noise into smooth ink; 0 = sharper but jagged), `dissipation=` dye kept per 1/30 s (default 0.99; 1 = never fades), `drag=`
    velocity kept per 1/30 s (0.995), `vorticity=` curl confinement (12; 0 = off), `buoyancy=`
    px/s² up per unit dye (smoke), `turbulence=` ambient seeded curl noise px/s², `seed=`,
    `color=`, `density=` display gain (coverage = 1 - exp(-density·dye), 2.5), `shade=` dense dye
    darker by up to that fraction, `iterations=` pressure multigrid V-cycles (2), `edge=` rim-only display above that coverage. Without
    `x/y/w/h/place` it fills the frame in the scene world. Walls are closed (ink pools).
  - **Verbs** (times = event time, `dur=` the window): `pour #f at=x,y dir=-90deg speed=700
    radius=2.5% rate=3 wobble=14 dur=1s` (a jet: velocity pulled to `speed` px/s along `dir`, 0 =
    right, -90 = up; dye `rate`/s at its center; `dir` swings by a seeded ±`wobble`°); `stir #f
    at= radius=18% strength=4 dur=0.8s` (a vortex, rad/s, negative = counter-clockwise); `ink #el
    into #f dur=0.5s amount=1.6 noise=420 speed=0 dir=-90deg [keep]` (the element's coverage, its
    subtree at that time without its own opacity, becomes dye spread over `dur`, with a curl-noise
    velocity kick of `noise` px/s plus `speed` along `dir`; the element fades out over `dur`
    unless `keep`: type dissolves into ink); `reveal #el with #f [mode=alpha|alpha-inverted]
    [only]` (the element shows only where the dye is, an alpha matte for its whole life; the fluid
    then draws only its rim, fading out above coverage `edge=0.45`, so declare it after the
    element: a dark-edged stain with the element inside, lint `W_FLUID_ORDER`; `only` hides
    the fluid itself). `at=` and `radius=` take px or % of the box.
  - **Time**: a `rewind`/`timemap` of an enclosing group replays the cached frames (ink flows back
    into the letters); the simulation itself always runs forward (channel `fluid-time`, baked like
    any remapped track, reel-ir `fluid.time`).
  - **Cost**: grid w·h cells; a 1080p frame at 1/4 is 480x270 (~127 KB per frame cached, 38 MB
    for 10 s). Lint `W_FLUID_MEM` above 256 MB, `W_FLUID_PROP` unknown props (did-you-mean),
    `E_FLUID` bad values / unknown fluid ids. Damage: the box from the first dye until the dye has
    provably dissipated below one u8 step (with `dissipation=1`: to the end); nothing before.
- **Point clouds** (`cloud`): thousands to millions of dots that morph between formations, as
  ONE layer. The renderer evaluates every point per frame (and per motion-blur sample) from a
  few lines of description (reel-ir `cloud`, reel `IR.md`): no per-point layers or keyframes,
  so a 20k-point scene compiles in milliseconds and its IR is a few KB. Direct mode only (the
  HTML/Chrome backend leaves the box empty). Examples: `examples/cloud/`.

  ```
  cloud #swarm n=20000 seed=7 r=2.4 jitter=0.25 palette=[paper, #E8442E] weights=[0.64, 0.36] mblur=8
    form 0s    field flow=[60, -10] noise=40 noise-scale=300
    form 1.2s  text "3000" weight=800 size=60% dur=1s stagger=0.6s by=x ease=spring(150,15) color=dir angle=30 split=[0.64, 0.36]
    form 4s    spiral r=45% squash=0.62 spin=25 dur=1.1s stagger=0.4s by=distance color=distance split=[0.64, 0.36]
    form 7s    terrain amp=80 rows=120 speed=0.3 dot=0.8 dur=1.2s stagger=0.5s by=y
    form @reel text "reel" weight=900 dur=1s stagger=0.4s by=x noise=1.5 noise-scale=60
  ```

  - **Element**: `n=` points (default 5000; lint warns above 300k), `seed=`, `r=` radius px,
    `jitter=` per-point radius variation 0..1, `palette=[..]` colors or theme token names,
    `weights=[..]` initial split, `persp=` own perspective (px, about the box center) when the
    scene has no camera. Common props apply (`opacity`, `in=fade`, `mblur=`, `x/y/w/h`, `rotx`
    ...). Without `x/y/w/h/place` the cloud fills the frame and lives in the scene world (scene
    coordinates); otherwise it is laid out like any box. Shape coordinates are relative to that
    box: `x=`/`y=` (top-left or center, see below), `w=`/`h=`/`r=`/`size=` take px or `%` of it.
  - **`form <time> <shape> ...`** (children of the cloud, in time order): the time is scene time
    or `@anchor`; the first form is where the points start. Morph: `dur=` per point (default 1s),
    `stagger=` total spread over all points (negative = reversed) ordered
    `by=index|random|distance|x|y|angle`, `ease=` (any ease; springs overshoot; default
    inout-cubic), `assign=curve|angle|index|random` (default `curve`: Hilbert order of source
    and target plus local swaps, so neighbours stay neighbours and morphs look choreographed).
    Drift while in the form: `noise=<px> noise-scale=<px> noise-speed=<1/s>` (seeded curl noise).
    `dot=` radius factor, `opacity=`. Colors: `color=random|index|x|y|z|distance|angle|dir` with
    `split=[fraction per palette color]` (`dir`: along `angle=` deg; `wave=[amp, period]` bends
    the boundary). When the split sizes equal the current ones, points keep their color and move
    into their color's region; otherwise they blend to the new color during the morph.
  - **Shapes**: `field` (rect `x y w h`; `flow=[vx, vy]` px/s streams through the box with
    wrap-around; `depth=`), `grid` (`cols= jitter=`), `spiral`/`galaxy` (center `x y`; `r= arms=
    twist= spread= squash= spin=` deg/s, faster inside), `cluster`/`gaussian` (center; `sx= sy=
    sz=`), `sphere` (center, `z=`; `r= spin=`), `terrain` (`rows= amp= scale= speed=`; `z=on`
    lays it into depth: use a camera or `persp=`), `text "..."` (`font= weight= size= tracking=
    lh= align= w=`, `outline`; filled on a hex grid or along the outline, centered on `y`),
    `points` (`points=[[x, y(, z)], ...]` px in the box, or `src=file.json`).
  - **3D**: in a scene with `camera` (or with `persp=`, or `rotx`/`roty`/`z` on the cloud) the
    compiler bakes the full projective chain per frame into the cloud (exact, not linearized):
    perspective per point, drawn far to near, dot size by distance.
  - **Lint**: `E_CLOUD` (unknown shape or order with "did you mean", bad lengths, forms out of
    order, text form without text, `form` outside a cloud, no forms, bad count), `W_CLOUD_PROP`
    (unknown or stray props, with "did you mean"), `W_CLOUD_LARGE`.
  - **Cost**: per-frame while any point moves; resting forms (no drift, flow, spin or waves) cost
    nothing. 20k points, 1080p30, 15 s with `mblur=8`: about 27 s render on one segment (6-core desktop CPU).
- **Print looks and light** (`src/look.rs`; reel-ir effects `riso`, `halftone`, `grain`, `paper`,
  `shadow`, `sweep`, `rays`, reel `IR.md`): props on any element, or on the `scene` line (the
  whole scene, background included). Seeded and deterministic (`seed=`, default 1); noise is
  anchored to the layer, so it travels with it. Direct mode only (the HTML backend ignores them,
  except `grain`, which it draws as an overlay).

  ```
  scene poster dur=6s bg=#F4EFE6 riso=[#FF48B0, #0078BF, #FFE800] misreg=3 riso-drift=1.5 riso-grain=0.35
    light #sun x=10% y=-20% z=900 radius=80
    card #c shadow=soft(#sun, height=40, opacity=0.5)
    rect #plate fill=#111 rays=#sun rays-length=0.6
    image #shot src=ui.png halftone=10 halftone-mode=cmyk
    @1s anim #sun x 0s:0 4s:1400
    @2s sweep #c dur=1.4s angle=20deg width=25%
  ```

  - **`riso=[inks]`** (1 to 4 colors or theme names): the layer is separated into spot-ink plates
    (least squares on optical density), each plate gets its own seeded misregistration
    `misreg=` px (0..200, default 2; channel `misreg`), optionally a slow seeded wander
    `riso-drift=` px at `riso-speed=` Hz (default 0.25), `riso-grain=` ink speckle 0..1 (0.3),
    `riso-density=` blotchy density variation 0..1 (0.2); plates multiply. `riso-paper=` is the
    paper color of the layer (default: its solid fill, e.g. the scene `bg`): printed areas stay
    opaque paper; without a paper the ink is transparent (exact over white).
  - **`halftone=<cell px>`**: AM round dots on a grid rotated by `halftone-angle=` (45deg), dot
    area from darkness (`halftone-mode=mono`, ink `halftone-ink=`) or four CMYK screens at 15/75/0/45
    deg (`cmyk`). `halftone-paper=` fills the layer with paper between the dots. The cell is the
    channel `halftone`: `anim #shot halftone 0s:160 4s:7~out-cubic` resolves an image out of dots.
  - **`grain=<amount>`** (0..1): monochrome grain inside the layer, `grain-size=` px (1),
    `grain-mode=static|live`. `static` (default) is one pattern: at rest the layer costs nothing.
    `live` draws a new pattern every frame, so the layer changes on every frame (no frame
    skipping, and the whole layer is re-filtered per frame).
  - **`paper=<texture>`** (0..1) `bleed=` px `rough=` 0..1 `paper-color=`: ink bleed (the layer's
    edges blurred and re-thresholded), noisy rough edges, paper fibers and tooth.
  - **`light #id x= y= z= radius=`**: an invisible light in the scene world. `x`/`y` place its
    center like any element (animate it with `anim #sun x ...`, `move`, ...), `z` is its height
    above the page in px (default 900; lower = longer shadows), `radius` its size (default 60;
    larger = softer). Its position is baked per frame into the effects that use it.
  - **`shadow=soft(#light, height=24, opacity=0.45, color=#140F1E)`**: the layer casts a soft
    shadow onto everything below it, as a flat sheet `height` px above the page lit by the light:
    the shadow falls away from the light and its penumbra (from the layer's signed distance
    field) widens with height and light radius. A moving light sweeps the shadows; a shadow
    reaches at most `height / (z - height)` x the distance to the light (that is its ink).
  - **`rays=#light`** `rays-length=` 0..0.95 (0.5) `rays-strength=` (1; channel `rays`)
    `rays-color=`: god rays. Light behind the layer comes through its holes (transparent pixels
    inside its ink box, e.g. type knocked out of a dark plate) and streams away from the light.
  - **`knockout`** (flag on a child): the child is cut out of its parent instead of drawn
    (reel-ir alpha-inverted matte), e.g. a word knocked out of a dark plate for `rays`:
    `rect #plate w=100% h=100% fill=#111 rays=#sun` with child `text "LIGHT" knockout`.
  - **`sweep #id dur= angle= width= color= strength= ease=`** (verb): a soft band of light
    crosses the layer once, restrained (screen blend, gaussian profile; `width` px or % of the
    layer, default 22%; strength default 0.35). Channel `sweep` (0 = before the ink, 1 = past it).
  - **Lint**: `E_LOOK` (unknown `riso-`/`halftone-`/`grain-`/`paper-`/`rays-` prop or `soft()`
    argument, with "did you mean"; ink count), `E_VALUE` (ranges, `halftone-mode`/`grain-mode`
    values with "did you mean"), `E_LIGHT` (light not declared in the scene, "did you mean").
  - **Cost**: looks are re-filtered when the layer changes (or every frame for `live` grain,
    `riso-drift`, animated cells/lights); a resting layer with static looks is cached like any
    effect. Examples: `examples/features/print-looks.reel`, `examples/features/light.reel`.
- **World canvas + camera director** (X9, `src/world.rs`; reel-ir node `world`, reel `IR.md`): one
  large canvas instead of cut scenes; the camera travels through it in one continuous move.

  ```
  scene tour dur=12s
    world #w 16000x9000 bg=#0B0B0E
      text #mark "zoom" size=6200 weight=900 x=600 y=-700 nowrap
      place #grid at 5405,3613 scale=0.52          // a 1920x1080 board centered at x,y
        place #app at 725,540 scale=0.23           // nested: powers of ten
          text #para "Small moves, every day." size=34 x=110 y=700
          place #seed at 253.3,727.7 scale=0.004   // inside the counter of one "o"
            text "start here" size=300 weight=900
    @0s   camera zoom-through #seed -> #para -> #app dur=4s
    @4s   camera focus #kpi overlay=#000000d8 for=1.3s
    @6.5s camera zoom-through #grid -> #mark dur=5s fit=0.95
  ```

  - **`world #id WxH`** (`max-zoom=` default 1e5, `bg=`, `tile=` px 64..1024, `cache=` MB): a canvas
    in the scene's camera space (world px = scene px before the camera; the frame shows its top-left
    1920x1080 until a camera verb moves). Children are placed absolutely (`x= y=`) or with `place`.
  - **`place #id at x,y`** `[scale=] [rot=] [w=1920 h=1080] [gap= pad= align= justify=]`: a board
    centered at (x, y) of its parent (world px, or the parent board's px), laid out like a scene
    (column flow, centered). Places nest; `scale=0.01` inside `scale=0.01` is a 10000x zoom level.
    Coordinates stay local to each board, so deep levels keep full precision.
  - **`nest #id levels=N scale=k [rot=r] [cycle=[a, b, c]] [at x,y] [w= h=]`** (`src/nest.rs`): a
    self-similar stack for infinite zooms. Its children are one level, drawn in a box of the frame
    size (or `w=`/`h=`) centered at `at x,y` or at the parent's center; the nest places N copies,
    copy i scaled by k^i and turned by r*i degrees about the box center (innermost painted first).
    Inside a copy, `$cycle` becomes `cycle[i mod len]`, `$level` becomes i, and ids get the suffix
    `-L<i>` (`#dot` -> `#dot-L0`, `#dot-L1`, ...). Levels default 12 (max 64), scale 0.8. The nest's
    `scale=`/`rot=`/`levels=` are per level, not its own transform or color grade. Works in a scene
    or a world.
  - **Camera verbs** (events like any verb: `@anchor camera <verb> ...`; at scene level = scene
    start). They move the scene camera (scale about the frame center + translation). Targets are
    any ids (`#a -> #b -> #c`); a target is framed to `fit=` of the safe area by its bounds through
    every ancestor transform (nested scales, rotations, animated moves at that time). Moves follow
    the van Wijk & Nuij smooth zoom-pan path (`rho=`, default sqrt 2: far pans zoom out on the way,
    nested targets zoom straight in), exact at any zoom ratio, baked into per-frame camera keys at
    full precision. A verb at the scene start (or `start=cut`) begins *at* its first target.
    - `camera tour #a -> #b -> #c [ease=] [move=<dur per leg>] [dwell=narration|1.5s] [fit=0.85]`:
      a leg per stop (default duration from the path length, 0.8..3.6 s) with `ease` (springs
      overshoot along the path), then a dwell. `dwell=narration` (default) spreads the rest of the
      scene evenly over the stops.
    - `camera zoom-through #a -> #b [dur=] [ease=] [fit=]`: one continuous move through all
      stops (constant perceived speed between them, one ease over the whole; default duration from
      the path length), e.g. 100x to 30000x in one shot.
    - `camera follow #x [fit=0.7] [lead=120ms] [for=|until=@x] [smooth=0.25s] [zoom=follow]`: keeps
      an animated element framed (its position `lead` ahead), exponentially smoothed; `zoom=follow`
      also tracks its size.
    - `camera focus #k [overlay=#000a] [pad=28] [for=1.6s] [dur=0.35s] [fit=0.7] [move=<dur>|off]`:
      frames the target and dims everything else with a dark overlay (a hole of `pad` screen px
      around it) for `for`, fading in/out over `dur`.
    - `camera loop #nest [levels=<cycle length>] [dur=<rest of scene>] [dir=in|out] [zoom=1]
      [wrap=on|off]`: a seamless infinite zoom. The camera scales about the nest center at constant
      log-speed (exactly `levels` nest levels per `dur`, per-frame keys, no ease steps) and the nest
      turns back by its `rot` per level, so the frame at `dur` equals the frame at the start; then it
      wraps (repeats for the rest of the scene). The target is a nest or a layer holding nests with
      equal scale/rot/cycle (e.g. a blurred halo nest under a sharp one); `scale=`/`rot=` loop other
      self-similar content (unchecked). Static `blur=`/`glow=` radii inside the loop are
      counter-scaled so they stay constant on screen (a group halo stays self-similar). `wrap=off`
      keeps zooming (seam check: frame `dur` vs frame 0). `#scene`-wide content beside the nest jumps
      at the seam: mark it `hud`. Example: `examples/features/neon-tunnel.reel`; encode loops with
      `tools/encode-loop.sh master.mkv outdir name` (exact N frames, CFR 30, no B-frames, one
      keyframe at 0).
  - **Rendering**: the world's static content goes into the renderer's mip tile cache (tiles at
    power-of-two scales, rastered on demand along the planned camera path and prefetched in
    parallel, LRU within `cache=`); moving frames resample the tiles, rest frames draw exact
    vectors, so text is crisp at every zoom. Animated elements inside the world render live (drawn
    above the static world). Damage is the full frame while the camera moves, nothing at rest.
    `world-zoom.reel`: 30000x, 369 frames, ~29 s on a loaded 4-core desktop incl. 2100 tiles.
  - **Lint**: `E_CAMERA` (unknown verb with "did you mean", no targets), `E_CAMERA_TARGET` (id not
    found, "did you mean"), `E_CAMERA_TINY` (target under 24 px at the world's `max-zoom`),
    `W_CAMERA_ZOOM_LIMIT` (framing needs more than `max-zoom`; clamped), `W_CAMERA_FAST` (zoom
    above 5 octaves/s or pan above 2.5 frame widths/s: unreadable), `W_CAMERA_PROP`, `W_WORLD_PROP`
    (unknown props, "did you mean"), `E_WORLD` (bad size, `place` without `at x,y`). Elements in a
    world are exempt from frame/safe-area/min-font checks (the camera frames them).
    Loops: `E_NEST` (levels not 1..64, scale <= 0 or 1, bad `at`), `W_NEST_CYCLE` (`cycle=` unused
    or `$cycle` without `cycle=`), `E_CAMERA` (loop target not a nest and no `scale=`),
    `W_LOOP_SELF_SIMILAR` (not a nest, nests that differ, levels not a multiple of the cycle,
    nests off the loop center), `W_LOOP_SEAM` (innermost levels above 2 px at the start, with the
    levels= needed; outer levels not past the frame after one step; nest has no more levels than
    the loop zooms), `W_LOOP_CONTENT` (other camera-space content, or an animation inside the loop
    whose value differs at start and end), `W_CAMERA_FAST` (above 5 octaves/s). Any scene:
    `W_GLOW_COST` (more than 8 glow/blur layers, each its own blur pass; e.g. `glow=` on a nest
    child is one pass per level: put it on the nest).
  - Limits (v1): no camera rotation (a loop turns the nest instead); static ancestors' clips/opacity do not apply to animated
    elements inside a world; not with a 2.5D `camera persp=` in the same scene. Scene looks under a
    loop are not self-similar when the renderer scales them with the content (`rgbsplit` grows with
    the zoom); `vhs`/`grain` noise is not periodic. Direct mode only
    (the HTML backend shows the world without the tile cache).
- **Transitions**: scene `in=cut|fade|push|push-up|push-right|slide|slide-up|wipe|wipe-right|iris|zoom|magic`.
  `magic` morphs every id that exists in both scenes (position and size, measured after layout).

## 9. Raw level

`svg |`, `html |` bodies, `image src=`, `video src=` are layers like any other: they can be animated,
masked and timed. An `svg` with `w=`/`h=` is scaled into that box (the root `<svg>` gets
width/height 100% and keeps or gains a viewBox; it is not cropped at its own width/height).
`video` draws only in the HTML backend so far (direct mode: native decode planned); direct mode
reads PNG images. `lottie` and `shader` (WGSL) are planned.

**`html` block (S1, sanctioned escape hatch):** raw HTML with its own `<style>` (flex, grid,
gradients, borders, radius, box-shadow, images, inline SVG, Inter / Noto Sans Mono fonts). In direct
mode it is laid out and painted natively by Blitz (the same engine as reel's HTML mode), as vector
display lists, no browser and no raster. Its CSS is static: animate the whole block with verbs
(`in=rise`, `move`, `zoom #block`), or mark elements inside it with `data-part="x"`: each becomes a
layer `#block.x` that every verb and `anim` can target (`pulse #block.x`, `anim #block.x scale ...`).
A part is drawn above the rest of its block (direct mode) and its margin is reset. Give it a `w=` so its layout is defined. Unsupported CSS
(transitions, `@keyframes` inside the block, filters other than one blur/drop-shadow) is reported.

## 10. Themes and brand tokens

Built-ins `studio-dark` (deep green) and `studio-light` (`dark` and `light` are short forms). Tokens:
- colors: `bg bg2 surface surface2 border text muted faint mint accent accent2 danger warn ink paper code-bg`
- type: `display h1 h2 h3 body small caption label code`, `min` (lint minimum font size);
- motion: durations `fast base slow move hold`, eases `std out in pop soft`;
- layout: `safe safe-x safe-y radius`;
- lint: `min-contrast no-emdash`, and the taste-lint thresholds `motion=false` (switch off), `min-area=8%`, `max-still=1s`, `max-still-silent=2s`, `small-span=1.5s`, `end-window=2s`, `end-still=2s` (see "Motion energy and taste lint").
`$name` works wherever a value of that type is expected; unknown tokens are errors with suggestions.

## 11. Multi-format

`video formats=[16:9, 9:16, 1:1, 4:5]`; the short side stays at the base size (1080). Compile one
with `--format 9:16`; `agentfx lint --all-formats` checks all of them. Per-format adjustments:

```
row #charts dir=auto        // row in landscape, column in portrait
when portrait               // also: landscape, square, 9:16, 16:9 (comma lists)
  set #c3 y=70%             // prop overrides by id
  @Ebenen anim #camera y 0s:-160 3s:160   // format-only events and elements
```

## 12. Determinism

1. Output frame `n` is a pure function of the source, assets and `n`. No clocks, no network at
   render time (fonts embedded, images decoded before frame 0), no unseeded randomness.
2. All time is resolved at compile time into absolute keyframes. Load-time layout work (magic-move
   deltas, connector paths, `fit`) runs once before frame 0 and never per frame.
3. TTS is the only non-deterministic input; it is cached by sentence hash and the cache is an input.
4. **Quantization (no visible change):** values are snapped before planning, and a frame only counts
   as changed if some value moves at least one step: translation 1/64 px (renderer commits at
   ≥ 1/16 px on screen), opacity 1/255, color channels 1/255, scale 1/4096, angles 0.01°. Keyframe
   times snap to 0.1 ms. Runs of identical keys are dropped. (E1: 18 % of frames in a Remotion video
   change the DOM by less than this, e.g. opacity 0.000977 or 0.28 px moves.)
5. Seeking is exact: the HTML backend pauses every animation and sets `currentTime`; the reference
   renderer never plays in real time.

## 13. Diagnostics (selection)

| Code | Severity | Meaning |
|---|---|---|
| `E_INDENT E_STRING E_KIND E_DUP_PROP E_BRACKET` | error | syntax |
| `E_ID` | error | unknown id/hotspot (lists ids in scope, "did you mean") |
| `E_ANCHOR W_ANCHOR_UNUSED W_ANCHOR_DUP` | error/warn | narration anchors |
| `E_TIMING_OVERRUN E_EVENT_OUTSIDE W_TIMING_CLICKS W_TIMING_CONFLICT W_STATIC_SCENE` | | timing |
| `E_PRESET E_VERB E_CHANNEL E_VALUE E_TOKEN E_COLOR E_PAINT E_EASE` | error | vocabulary/values |
| `E_OVERFLOW E_OUT_OF_FRAME W_SAFE_AREA W_MIN_FONT W_CONTRAST W_OVERLAP` | | layout (static, and measured with `--measure`) |
| `E_LINK_GEOMETRY W_LINK_SHORT` | error/warn | connectors (measured) |
| `W_IN_NO_ID E_CROP E_CHART E_ICON` | | element props |
| `E_CLOUD W_CLOUD_PROP W_CLOUD_LARGE` | error/warn | point clouds (`cloud`, `form`) |
| `W_PLANNED W_ZOOM_LAYOUT` | warning | planned feature or backend approximation |
| `W_IR_UNSUPPORTED W_IR_COMPAT W_IR_AUDIO` | warning | direct mode: what reel-ir cannot express yet |
| `W_EMDASH` | warning | house rule: no em-dashes in user-facing copy |
| `W_COLD_OPEN W_SMALL_MOTION W_DEAD_AIR W_STATIC_END` | warning | taste lint over the motion-energy map (`agentfx lint --motion`, also part of `--measure`) |
| `E_AUDIO E_BEAT_RANGE W_BEAT_PAST_END W_AUDIO_FLASH W_AUDIO_MUSIC` | error/warn | audio reactivity: music missing or undecodable, bad `audio(...)`, beat anchor beyond the track or the video, flash risk (§ Audio reactivity) |
| `I_TTS_ESTIMATED` | info | word timings estimated |

`agentfx lint --measure` samples each scene (after entrances, at the end of every sentence, at the scene
end) with the native layout (S1; about 0.1 s per format, `--chrome` uses the old Chrome measure) and
checks real boxes: overflow, frame and safe area, rendered font size (including zoom), contrast
against the effective background, overlaps, connector geometry. Elements under a moving scene camera
(`zoom #element`, `dolly`) are exempt from the frame checks while the camera is moved.

## 14. Pipeline and backends

```
.reel / .json -> AST -> (format specialization) -> timing (TTS words) -> motion IR -> backend
                                                                               |-> reel-ir (direct mode, default): native layout (Blitz) -> reel renderer
                                                                               '-> html (CSS @keyframes, Chrome preview/reference: --backend chrome)
```

Direct mode details: `IR-MAPPING.md`; what reel-ir still lacks: `IR-PROPOSALS.md`.

Every feature declares its stage: **composite** (transform/opacity of a cached layer),
**paint** (re-raster of one layer), **layout** (re-layout of a subtree), **per-frame** (particles,
grain, video). The IR keeps channels, paints and eases symbolic; only the HTML backend turns them
into CSS (registered custom properties per channel, one `@keyframes` per track, springs as
`linear()`, no animation library).

## 15. Capability catalog

Generated by `agentfx catalog --md` from `src/catalog.rs` (the same table drives lint and the schema).

#### document

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| video | doc | - | implemented | `video "Title" size=1920x1080 fps=30 voice=<tts voice> rate=+6% theme=studio-dark captions=plain\|karaoke\|off formats=[16:9, 9:16] lead=2f tail=6f` | Header. One per file. |
| theme | doc | - | implemented | `theme studio-dark ⏎   colors accent=#17845a ... ⏎   type h1=92 min=20 ⏎   motion base=0.5s pop=spring(260,20) ⏎   layout safe=6% radius=18 ⏎   lint min-contrast=3 no-emdash=true ⏎   bg linear(...)` | Brand tokens; referenced as $name. |
| pronounce | doc | - | implemented | `pronounce "staging.example.test" "staging dot example dot test"` | Spoken substitution; captions keep the written form. |
| design | doc | - | implemented | `design lens=swiss\|editorial\|keynote\|flat\|blueprint ; design src=design.reel ; design "Name" base=light\|dark ⏎   colors ... ⏎   type display="Family" body="Family" base=40 ratio=1.333 ⏎   surface style=elevated\|flat\|outline\|glass\|tinted ⏎   skin node=tile\|step\|tag` | Design spec (L2): extends the theme tokens; see design/AUTHORING.md. |

#### audio

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| music | doc | audio | implemented | `music src=bed.mp3 vol=0.25 duck=0.08` | Music bed, auto-ducked under every say. |
| audio reactivity | motion | composite | implemented | `music src=track.wav bpm=128 ⏎   rect #b origin=bottom scale-y=audio(band=3, gain=1.5, smooth=80ms) ; opacity=audio(level) ⏎   pulse #ring on=beat\|bar amount=0.15 ; at beat:32 / at bar:8` | Offline music analysis (level, onsets, 8 bands, beat grid) lowered to linear keyframe tracks; beat:N / bar:N anchors. |
| visualizer | story | composite | implemented | `visualizer #v type=bars\|radial\|circle\|waveform count=32 w= h= gap= radius= thick= colors=[..] mirror gain= smooth=` | Music visualizer: a layer of rects with per-frame tracks. |
| TTS word timings | story | audio | implemented | `video voice=en_US-ljspeech-medium rate=+6%` | piper (default, local; words spread over the audio and snapped to pauses), edge-tts WordBoundary (opt-in) or estimated (`--tts none`); cached per sentence hash. |
| SFX on events | story | audio | implemented | `@x click #b sfx ; sfx click\|pop\|whoosh\|file.wav` | Built-in synthesized sfx or files. |

#### component

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| chapterbar | doc | composite | implemented | `chapterbar label="..." scenes=[a, b, c]` | Progress bar for scenes with title=. |
| title | story | layout | implemented | `title #t "Main" sub="Subline" kicker="LABEL"` | Title block; svg/icon/image children go left of the title. |
| card | story | layout | implemented | `card #c "Title" sub="Body" icon=bolt kicker="NEW" variant=surface\|glass\|accent\|outline` | Designed card: icon tile, kicker, title, body. Cards in a row share its width and height. |
| stat | story | layout | implemented | `stat #s value=12480 from=0 label="Nutzer" prefix= suffix=" %" delta="+12 %" sub="seit Juli" icon=user spark=[3, 5, 8] in=count` | KPI tile: label, counting number, delta pill, sparkline. `count #s to=` works on the tile. |
| note | story | layout | implemented | `note #n "Text" title="Heads up" kind=info\|tip\|warn\|danger\|success icon=` | Callout box with accent bar and tinted icon. |
| node | story | layout | implemented | `node #a "Label" sub="Detail" icon=db variant=default\|primary\|muted status=ok\|warn\|err` | Diagram node with icon tile and status dot (use in `diagram` + `link`). |
| steps | story | layout | implemented | `steps #p "Plan" "Build" "Ship" active=2` | Numbered steps with connectors; parts #p-s0.. stagger on enter. |
| list | story | layout | implemented | `list #l "Eins" "Zwei" icon=check` | Icon list; items stagger on enter. |
| pill | story | paint | implemented | `pill "Neu" color=$accent2` | Badge. |
| quote | story | layout | implemented | `quote #q "Text" by="Name, Rolle"` | Pull quote. |
| progress | story | layout | implemented | `progress #p n=6 value=0 label="Schritt"` | Segmented progress with a value / n counter; `advance #p to=3` fills it. |
| panel | story | layout | implemented | `panel #d kicker="Details" ⏎   pane #a title="Agent" ⏎     item "lint" chip="ok" status=ok\|run\|wait\|fail icon=` | Detail panel: one pane at a time; `page #d to=a` swaps panes. |
| skins | story | layout | implemented | `node #a "Chat" sub=".." n=01 skin=tile\|step\|tag` | HTML/CSS skins for nodes (design/skins), drawn with the design tokens; parts active/done. |
| icon | story | paint | implemented | `icon name=sync size=48 color=$mint` | Built-in stroke icons. |
| lower3 | story | composite | implemented | `lower3 #l "Name" sub="Role" icon=user in=left` | Lower third (glass panel, accent bar). |
| toast | story | composite | implemented | `toast #t "Saved" in=rise at=@word` | Notification pill. |
| chapter | story | composite | implemented | `chapter "Panels" n=1 for=1.2s` | Full-frame chapter card at scene start. |

#### format

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| when | doc | - | implemented | `when 9:16 \| portrait \| landscape \| square ⏎   set #id k=v ⏎   <elements>` | Per-format overrides and format-only elements. |

#### timing

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| scene | doc | - | implemented | `scene <id> title="..." in=fade\|push\|slide\|wipe\|iris\|zoom\|magic\|cut tdur=0.6s pre=0.5s post=0.5s dur=<override> min= bg=<paint>` | A shot. Length = narration unless dur= overrides. |
| say | story | audio | implemented | `say "Text with [[anchor]] and [[two words\|name]]." min=5s pre= post= spoken="..."` | Voice-over beat; TTS word timings drive all anchors. |
| pause | story | - | implemented | `pause 0.8s` | Silent beat. |
| @anchor | story | - | implemented | `@Reiter verb #id ... ; verb ... ⏎ @Reiter.end+0.2s  @2.5s  @start  @end-1s  @prev+0.3s` | Event time: word start/end, scene time, previous event; verbs follow. |

#### text

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| text | story | layout | implemented | `text #id "**bold** {accent} `code`" style=display\|h1\|h2\|h3\|body\|small\|label size= color= weight= align= upper split=word\|char` | Rich text. h1/h2/h3/body/small/label are shorthands. |
| split (kinetic type) | motion | composite | implemented | `text #t "..." split=word\|char in=rise stagger=60ms` | Per-word/per-glyph animation. |
| split=line | motion | layout | implemented | `text #t "..." w=760 split=line in=rise stagger=180ms` | One layer per laid-out line (native line breaks, S1). |
| variable font axes | motion | layout | implemented | `text ... axes="wdth 80, GRAD 50" ; anim #t wght 0s:300 1s:900 wdth 0s:75 1s:125 GRAD 0s:0 1s:100` | Static axes on any variable font; animated wght/font-weight, wdth, opsz, slnt and up to 4 custom (uppercase) axes per video, relaid out per frame (repaint states). |
| custom font file | doc | layout | implemented | `font "Brand" src="fonts/Brand.woff2" ; text ... font=Brand` | TTF/OTF/TTC/WOFF/WOFF2 registered under the alias (direct mode decodes WOFF/WOFF2; Chrome reference gets @font-face). E_FONT, W_FONT_FAMILY. |
| opentype features | story | layout | implemented | `text ... features="ss01 off, tnum, cv05 2"` | font-feature-settings on the text (replaces the stage default 'cv11','ss01'). |
| text stroke | story | paint | implemented | `text ... stroke=#000 sw=6 [stroke-over]` | Real outline from the glyph outlines (reel-ir glyph stroke): sw = visible width below the fill, or centered over it with stroke-over. Contrast lint counts it. |
| text color track | motion | paint | implemented | `anim #t color 0s:#fff 1s:$accent` | Plain text (no accent/code runs, no matte, no child layers): one reel-ir text.fill track instead of repaint states. |
| text animators | motion | paint | implemented | `animate #t by=char\|word\|line [range=0%..100%] [start= end= offset=-100%..100% amount=] shape=square\|ramp-up\|ramp-down\|triangle\|round\|smooth order=forward\|reverse\|random seed= \| times=[0s,0.3s,..] dir=in\|out ; dur= ease= opacity= x= y= scale= sx= sy= rot= blur= color=` | After Effects range selectors on one text node (renderer evaluates per unit, per frame), or per-unit key times. Parameters given as a..b animate over dur=. |
| text on path | motion | paint | implemented | `path #p d="M..." ; text #t "..." on=#p offset=40 ; anim #t offset 0s:0 2s:600` | Glyphs ride the path (rest geometry, scene space), rotated to its tangent; offset px animates (path-offset channel); glyphs off the ends hide. |
| auto-fit | story | layout | implemented | `text ... w=800 h=200 fit` | Shrinks font to fit (load-time layout hook). |
| captions | story | paint | implemented | `video captions=plain\|karaoke` | Captions from VO words; karaoke highlights the spoken word; anchors in mint. |
| caption pack | story | composite | implemented | `captions style=karaoke\|pop\|pill\|hormozi\|typewriter\|rise\|outline\|safe [src=talk.srt\|talk.vtt\|talk.json audio=talk.wav offset=] font= size= weight= color= hi= key= box= stroke= sw= shadow= place=bottom\|center\|top y= w= maxchars= lines= maxwords= upper keywords=[..] gap= minword= mincue=` | Word-by-word social captions from narration or a transcript (SRT/VTT/Whisper JSON); phrase grouping, balanced lines with real font metrics, per-format safe areas, read-time lint. Header `video captions=<style>` applies a style to every narrated scene. |

#### layout

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| stack/row/col/grid | story | layout | implemented | `row gap=24 align= justify= wrap ; grid cols=3 ; stack dir=auto` | Flex/grid layout. dir=auto = row in landscape, col in portrait. |
| box | story | layout | implemented | `box #b pad=30 ...` | Plain surface container. |
| place / x,y | story | layout | implemented | `place=top-left\|bottom\|... x=40% y=120 anchor=center` | Absolute placement (place uses the safe area). |
| safe areas | story | layout | implemented | `theme layout safe=6%` | Content flows inside; lint flags anything outside. |
| multi-format | story | layout | implemented | `video formats=[16:9, 9:16, 1:1, 4:5] ; agentfx html --format 9:16 ; when portrait ...` | One source, several aspect ratios. |
| constraints solver | story | layout | planned | `` | Explicit constraints between ids planned; flex/grid now. |

#### mock

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| window | story | layout | implemented | `window #w title="Dashboard" w=900 h=520 ⏎   stat ... ; note ...` | App window mock whose body holds elements. |
| device | story | composite | implemented | `device #d type=laptop\|tablet\|phone shot=app.png` | Device frame around a screenshot (no hotspots; use browser/phone for those). |
| browser / phone | story | composite | implemented | `browser #app url=... shot=a.png hotspots=a.json w=1600 crop=[x, y, w, h] ⏎   hot #extra 10 20 300 40 ⏎   toast #t "Saved" in=rise at=@x` | Screenshot in a window/device; hotspot ids become verb targets. |
| terminal | story | paint | implemented | `terminal #t in=type \| ⏎   $ cargo build ⏎   Finished` | Typed commands, then output. |

#### media

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| image | raw | paint | implemented | `image src=a.png w= h= fit=cover` | Bitmap layer. Direct mode reads PNG only (other formats: W_IR_UNSUPPORTED). |
| video | raw | per-frame | partial | `video src=clip.mp4` | HTML backend only (seeked per frame); direct mode does not draw video yet (native decode planned). |
| svg | raw | paint | implemented | `svg w=120 h=120 \| ⏎   <svg ...>...</svg>` | Inline SVG escape hatch. With w=/h= the root <svg> is scaled into the box (viewBox fit, its preserveAspectRatio; a viewBox is derived from width/height when missing). |
| html | raw | layout | implemented | `html #b w=900 \| ⏎   <style>.k{display:grid}</style> ⏎   <div class="k">...</div>` | Raw HTML + CSS block; laid out and painted natively by Blitz in direct mode (static CSS). data-part="x" makes an inner element the layer #block.x for verbs. |
| lottie | raw | per-frame | planned | `lottie src=anim.json` | Planned. |
| shader | raw | per-frame | planned | `shader wgsl=fx.wgsl` | Planned (native renderer). |

#### generator

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| code | story | layout | implemented | `code #c lang=ts diff \| ⏎   const a = 1 ⏎ - const b = 2 ⏎ + const b = 3` | Highlighted code; `diff #c` animates -/+ lines. |
| chart | story | composite | implemented | `chart #c type=bar\|line\|area\|pie\|donut data=[..] labels=[..] unit=% max= highlight=4 grid=false axis=false values` | Chart with y axis, gridlines, tick and value labels, legend (donut: largest slice in the center); `draw #c` or in=draw. Parts: #c.b1 (bar), #c.d1 (point), #c.s1 (slice). |
| counter | story | layout | implemented | `counter #n from=0 to=1280 prefix= suffix=" users"` | Integer counter; count #n to=. |
| cloud (point cloud) | motion | per-frame | implemented | `cloud #swarm n=20000 seed=7 r=2.4 jitter=0.25 palette=[paper, #E8442E] weights=[0.64, 0.36] persp= mblur=8 ⏎   form 0s field flow=[60, -10] noise=40 ⏎   form 1.2s text "3000" weight=800 size=60% dur=1s stagger=0.6s by=x ease=spring(150,15) color=dir angle=30 split=[0.64, 0.36] ⏎   form 4s spiral r=40% arms=3 spin=25 ⏎   form 7s terrain z=on amp=80 rows=120 ⏎   form 9s cluster sx=20% noise=10 ; sphere ; grid ; points` | Instanced point cloud: N points (up to millions) morph between formations with per-point stagger, springs, seeded drift and color rules; one layer, evaluated per frame by the renderer (reel-ir `cloud`). 3D forms go through the scene camera. Direct mode only. |
| fluid | motion | per-frame | implemented | `fluid #ink w=100% h=100% res=1/4 viscosity=0.0005 dissipation=0.99 drag=0.995 vorticity=12 buoyancy=0 turbulence=0 seed=3 color=#E8442E density=2.5 shade=0.3 ⏎ @0.2s pour #ink at=30%,80% dir=-80deg rate=3 ⏎ stir #ink at=50%,50% strength=5 ⏎ @1s ink #title into #ink ⏎ reveal #shot with #ink` | 2D stable-fluids layer: dye advected by a simulated flow (vorticity confinement, pressure projection) on a reduced grid, upsampled. Simulated once by the renderer and cached per frame (deterministic, frame n = f(n)); rewinds and time maps replay the cache. Direct mode only. |
| particles | motion | per-frame | implemented | `particles #p count=60 seed=3 x=50% y=40% speed=900 spread=360 gravity=1400 life=1.4s shape=confetti\|dot\|square colors=[..] emit=@word` | Seeded particle burst (exact projectile paths). |
| link (connector) | story | paint | implemented | `link #l #a -> #b curve dashed` | Connector between two element ids (load-time layout), draw-on. |
| diagram | story | layout | partial | `diagram dir=row gap=90 ⏎   node #a "Input" ⏎   node #b "Model" ⏎ link #a -> #b` | Row/col auto layout of nodes; graph layout planned. |
| maps | story | paint | planned | `map ...` | Planned. |

#### shape

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| rect/ellipse/circle | motion | paint | implemented | `rect #r w=300 h=200 radius=24 fill=$accent stroke=#fff sw=4` | Primitive shapes. |
| poly/star | motion | paint | implemented | `star #s points=5 inner=0.45 size=200` | Polygon and star paths. |
| path | motion | paint | implemented | `path #p d="M0 0 c ..." vb=24x24 w=400 h=200 fill= stroke=$mint\|linear(...) sw=6 trim=0 ; anim #p stroke 0s:#fff 1s:$mint fill ...` | Bezier path (full SVG path data: absolute and relative M L H V C S Q T A Z). Gradient strokes; animated fill/stroke colors are reel-ir color tracks (no repaint). |
| trim paths | motion | paint | implemented | `draw #p ; anim #p trim 0s:0 1s:1 ; trim-start` | Draw-on strokes. |
| dashes | motion | paint | implemented | `dash="10 8"` | Dashed strokes. |
| boolean ops | motion | paint | planned | `union/subtract` | Planned (native). |
| path morphing | motion | paint | planned | `anim #p d ...` | Planned (native; HTML would need JS). |
| SVG import | motion | paint | partial | `svg src=icon.svg` | Embedded as raw layer; per-path addressing planned. |

#### paint

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| solid / linear / radial / conic | motion | paint | implemented | `fill=#hex \| $token \| linear(155deg, #a 0%, #b 100%) \| radial(at 30% 40%, ...) \| conic(0deg, ...)` | Paints. |
| mesh gradient | motion | paint | implemented | `fill=mesh(2x2, #a, #b, #c, #d)` | Grid of colors, bicubic in OKLab; rastered once into a cached image both backends draw (static). Box, path, text and mask fills. Literal colors (theme tokens resolve first). |
| image fill / pattern | motion | paint | partial | `fill=image(tex.png cover\|contain\|fill\|none)` | Image fills on boxes, paths, text and masks (direct mode: PNG only; contain/none on paths and text pad the edges). Patterns planned. |
| animated gradient | motion | paint | implemented | `anim #bg grad-angle 0s:120deg 6s:240deg grad0 0s:#a 3s:#b` | Angle + up to 4 stop colors animate. |

#### compositing

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| groups / precomps | motion | composite | implemented | `layer #g x= y= w= h= ⏎   ...` | Nested layers; transforms compose. |
| alpha / luma masks | motion | paint | implemented | `mask=linear(90deg, #000 60%, transparent) mask-mode=luma ; anim #x mask-x ...` | Paint/image masks. mask-x 1 -> 0 slides a 3x-wide mask across the layer (put the soft edge between 33% and 66%). |
| track mattes (layer as matte) | motion | paint | partial | `text #t "BIG" fill=image(a.png) \| fill=linear(...) \| fill=mesh(...)` | Text matte via text fill; arbitrary layer mattes planned. |
| blend modes | motion | composite | implemented | `blend=normal\|multiply\|screen\|overlay\|darken\|lighten\|color-dodge\|color-burn\|hard-light\|soft-light\|difference\|exclusion\|hue\|saturation\|color\|luminosity\|plus` | Layer blend mode (W3C compositing; plus = add, also `add`). |
| clip / wipe / iris reveal | motion | paint | implemented | `enter #x wipe\|iris ; clip=box` | Animated clip shapes. |

#### effects

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| gaussian blur | motion | paint | implemented | `blur=8 ; anim #x blur 0s:20 1s:0` |  |
| directional / radial blur | motion | paint | planned | `` | Planned (native). |
| motion blur | motion | per-frame | implemented | `mblur=16 shutter=180deg ; anim #x x 0s:0 0.3s:900 mblur=24 shutter=1f` | Sub-frame shutter (reel-ir motion_blur): samples 2..64, shutter angle (180deg = half a frame, default) or frames, max 4f. Direct mode only; the HTML backend ignores it. |
| drop / inner shadow | motion | paint | implemented | `shadow=sm\|md\|lg\|inner\|"0 20 60 #0008"` |  |
| glow | motion | paint | implemented | `glow=24 glow-color=$mint ; glow #x` |  |
| color adjust | motion | paint | implemented | `brightness=1.2 saturate=0 contrast=1.4 hue=90deg ; anim #x saturate 0s:0 1s:1` | CSS filter semantics in both backends (direct: reel-ir `color` effect, animatable). |
| tint / levels / LUT | motion | paint | implemented | `tint=#F2C14E tint-amount=0.8 ; levels=0.05,0.95,1.1[,0,1] ; lut=grade.cube` | Tint maps luma to a color, levels remap black/white/gamma, 3D .cube LUTs (trilinear). Direct mode only; the HTML backend ignores them. |
| grain / vignette | motion | paint | implemented | `grain=0.06 grain-mode=static\|live grain-size=1 ; vignette=0.6` | Seeded grain inside the layer (reel-ir grain). static (default) costs nothing at rest; live changes every frame and defeats frame skipping. HTML: animated overlay. |
| risograph | motion | paint | implemented | `riso=[#FF48B0, #0078BF, #FFE800] misreg=3 riso-drift=1.5 riso-grain=0.35 riso-density=0.2 riso-paper=$bg seed=4 ; anim #x misreg 0s:12 1s:2` | Layer (or scene) split into 1-4 spot-ink plates, seeded misregistration per plate (slow drift: per-frame), multiplied, grain and density variation in the ink. Direct mode only. |
| rgbsplit | motion | paint | implemented | `rgbsplit=1 rgbsplit-dx=7 rgbsplit-dy=0 rgbsplit-mode=linear\|radial` | RGB channel offsets. Amount and dx/dy are animatable. Direct mode only. |
| glitch | motion | paint | implemented | `glitch=0.5 glitch-rate=6 glitch-block=22 seed=42` | Seeded digital displacement. Amount, rate and block are animatable. Direct mode only. |
| vhs | motion | paint | implemented | `vhs=0.5 seed=42` | Animated tape noise, scanlines and chroma bleed. Animatable amount. Direct mode only. |
| dither | motion | paint | implemented | `dither=3 dither-mode=bayer\|bluenoise\|floyd dither-size=4 dither-palette=[#000, #fff]` | Quantization with optional opaque palette. Animatable levels. Direct mode only. |
| ascii | motion | paint | implemented | `ascii=8 ascii-color=source\|mono ascii-bg=#000` | Bitmap character filter. Animatable cell size, custom ascii-charset. Direct mode only. |
| halftone | motion | paint | implemented | `halftone=10 halftone-angle=45deg halftone-mode=mono\|cmyk halftone-ink=#111 halftone-paper=#fff ; anim #x halftone 0s:160 4s:7` | AM halftone dots (area from darkness) on a rotated grid; cmyk = four screens. Animatable cell. Direct mode only. |
| paper | motion | paint | implemented | `paper=0.6 bleed=1.5 rough=0.5 paper-color=#F6F1E7` | Ink bleed (blur-threshold), rough edges, paper fibers and tooth. Seeded, static. Direct mode only. |
| light source | motion | composite | implemented | `light #sun x=20% y=-10% z=900 radius=60 ; anim #sun x 0s:0 6s:1400` | Invisible point/sphere light in the scene world (z = height above the page, radius = penumbra). Animate it like any element. |
| soft shadow (SDF) | motion | paint | implemented | `shadow=soft(#sun, height=40, opacity=0.5, color=#000)` | Shadow of the layer as a sheet `height` px above the page, penumbra from its signed distance field; a moving light sweeps it. Direct mode only. |
| light sweep | motion | paint | implemented | `sweep #card dur=1.4s angle=20deg width=25% color=#FFF1D6 strength=0.35` | A soft band of light crosses the layer once (channel sweep 0..1). Direct mode only. |
| god rays | motion | paint | implemented | `rays=#sun rays-length=0.6 rays-strength=1 rays-color=#FFE9C4 ; anim #plate rays 0s:0 1s:1.4` | Light behind the layer streams through its holes (knocked-out type), radial blur away from the light. Direct mode only. |
| chromatic aberration / displacement | motion | paint | planned | `` | Planned. |
| time effects | motion | per-frame | implemented | `slitscan #g axis=y span=1.2s band=4 ; echo #x n=6 dt=60ms decay=0.6 blend=add colors=$red,$blue ; timedisplace #grid map=radial\|linear\|noise\|#layer span=0.8s levels=24 ; freeze #x for=1s` | The subtree rendered at other times (reel-ir `time`): slit-scan rows/columns from earlier times, decaying echo copies, per-pixel time displacement. span/dt animate with later verbs (`slitscan #g span=0 dur=0.2s`). Direct mode only. freeze = hold remap. |

#### space

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| depth of field | motion | paint | planned | `camera focus=0 dof=0.02` | Planned (static blur by z works via blur=). |
| 2.5D camera / parallax | motion | composite | partial | `camera persp=1200 ; layer z=-400 ; anim #camera x 0s:0 6s:300 ; dolly z=200 ; orbit roty=12` | Perspective camera; depth gives parallax. Direct mode linearizes each 3D layer per frame (rotated planes stay parallelograms, W_IR_APPROX). |
| world canvas + camera director | story | composite | implemented | `world #w 16000x9000 ⏎   place #p at 7000,4000 scale=0.01 rot=-3 ; camera tour #a -> #b ease=spring(170,24) dwell=narration ; camera zoom-through #a -> #b dur=5s ; camera follow #x fit=0.7 lead=120ms ; camera focus #k overlay=#000a` | Boards on one large canvas (nested scale = powers of ten); the camera frames ids on smooth zoom-pan paths (exact at 1e4+ zoom). The renderer draws the static world from a mip tile cache while moving, vectors at rest. Direct mode. |
| nest + camera loop (infinite zoom) | story | composite | implemented | `nest #t levels=28 scale=0.72 rot=4 cycle=[#FF2BD6, #22E4FF, #FFC23D] w=1920 h=1920 ⏎   rect x=50 y=50 w=1820 h=1820 radius=150 stroke=$cycle sw=14 ; camera loop #t levels=6 dur=6s` | Self-similar nest (copy i scaled k^i, turned r*i, hue cycle) and a seamless loop: constant log-speed zoom of exactly N levels per loop, frame N equals frame 0. Lint checks the seam. Direct mode. |

#### motion

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| keyframes | motion | - | implemented | `anim #x <channel> <t>:<v>[~ease] ... [ease=] [repeat=3] [yoyo]` | Explicit keyframes; times relative to the event, or @anchor:value. |
| bezier easing | motion | - | implemented | `ease=out-cubic \| cubic(.2,.8,.2,1) \| steps(4) \| hold` |  |
| spring / bounce | motion | - | implemented | `ease=spring(170,26) \| bouncy \| snappy \| bounce(3)` | Physical spring kept symbolic in the IR; HTML samples it into CSS linear(). |
| stagger / repeat | motion | - | implemented | `in=rise stagger=80ms ; anim ... repeat=3 yoyo` |  |
| seeded random | motion | - | implemented | `seed=7` | SplitMix64; same seed, same frames. |
| expressions | motion | - | partial | `anim #x rot wiggle(2, 5deg, 1) dur=3s ; sin(freq, amp)` | Pure functions of time, sampled to keys; general expressions planned. |
| time remap | motion | - | implemented | `rewind #g speed=4 dur=0.5s [then=play\|hold] ; timemap #g 0s:0s 0.3s:0s 0.9s:-2.4s~in-cubic [then=hold] ; #scene` | A subtree's local time as a function of its parent's: reverse, freeze, slow/fast, scrub. Baked per frame by the compiler (exact at frames for every ease, split text, trim, particles, repeats). |
| motion paths + auto-orient | motion | composite | planned | `anim #x path="M..." orient` | Planned. |

#### transitions

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| magic move | story | composite | implemented | `scene b in=magic   (same #id in both scenes)` | Shared elements morph position/size between scenes. |
| transitions | story | composite | implemented | `scene x in=cut\|fade\|push\|push-up\|slide\|wipe\|wipe-right\|iris\|zoom\|magic tdur=0.6s` | Zoom-through blurs the outgoing scene. |
| custom mask transitions | story | paint | planned | `in=mask(shape.svg)` | Planned. |

#### 3d

| Feature | Level | Stage | Status | Syntax | Notes |
|---|---|---|---|---|---|
| scene3d | motion | per-frame | implemented | `scene3d #w fov=40 camera=(0, 2, 8) target=(0, 0, 0) roll=0 near=0.1 far=200 [ortho=8 \| iso] light=(-3, 5, 4) light-color= light-intensity=0.75 ambient=0.4 ambient-color= fog=#05010D fog-near=10 fog-far=30 [fog-density=] bg= aa=2 bloom=14 bloom-strength=1 shadow=0.45\|off shadow-blur=6 shadow-color= outline=2 outline-color= material=toon color=` | A 3D scene rasterised by the CPU renderer into one layer of the 2D pipeline (looks, effects, blend, masks, motion blur apply to it). y up, world units free. A static scene is drawn once and cached; camera or object tracks re-render it. Direct mode only. |
| 3D objects | motion | per-frame | implemented | `box #b w=2 h=1.4 d=2 bevel=0.1 ; cube size=1 ; plane w= h= ; floor y=-1 [size=200] ; sphere r=1 seg=48 ; cylinder r=0.5 h=2 r2= ; cone r= h= ; torus r=1 tube=0.3 ; group #g ⏎   x= y= z= \| pos=(x, y, z) rx= ry= rz= scale= sx= sy= sz= pivot=(x, y, z) shadow=off ground` | Children of a scene3d (nest under a group or any object: transforms compose). Rotations in degrees, yaw applied last. floor = a plane facing up that receives shadows. |
| 3D extruded text / path | motion | per-frame | implemented | `extrude #L "L" size=1.6 depth=0.6 font=Inter weight=800 tracking=0 ; extrude #logo d="M0 0 L10 0 L5 8 Z" size=2 depth=0.3` | Glyph outlines from the text stack (or SVG path data, nonzero fill) triangulated and extruded, centered on the ink box; size = font size (text) or height (path) in world units. |
| 3D bevel on extrusions | motion | per-frame | planned | `extrude ... bevel=0.05` | Planned; boxes have bevel= (rounded edges). |
| 3D mesh (OBJ) | motion | per-frame | partial | `mesh #m src=model.obj size=2` | Wavefront OBJ: v, vn, vt, f (polygons fanned), normalized to size. No MTL materials, no glTF yet (planned). |
| 3D screen | motion | per-frame | implemented | `layer #ui x=0 y=0 w=1920 h=1080 ⏎   ... ; scene3d ⏎   screen #crt w=4 h=2.25 src=#ui [res=1] emissive=0.2` | A plane whose texture is a 2D layer of the scene (hidden in the 2D pass), rendered every frame it changes (mip-mapped, trilinear): UI mockups, code, charts and kinetic type on a monitor or stage screen. |
| 3D materials | motion | per-frame | implemented | `material=flat\|lambert\|toon\|phong\|wire color= opacity= emissive=0.6 bands=3 specular=0.5 shininess=32 outline=2 outline-color= crease=35 wire=1.5 wire-color= double-sided texture=wood.png` | Toon = banded light; outline = screen-space silhouette and crease lines from the depth/normal buffer; wire = feature edges as lines; emissive drives the scene's bloom. Texture: PNG on box faces / planes (no UV editing). |
| 3D lights | motion | per-frame | implemented | `scene3d light=(-3, 5, 4) ambient=0.4 ; light #lamp point=(0, 4, 3) color=#7CFF6B intensity=0.5 range=6 ; light #rim dir=(4, 2, -3)` | Directional key light + hemispheric ambient on the scene line, extra point/directional lights as children (x/y/z and intensity animatable). |
| 3D ground shadows | motion | per-frame | partial | `floor ... ; scene3d shadow=0.45 shadow-blur=6 shadow-color=#000` | Planar: casters projected onto receiving planes (floor, ground) along the first light, softened by a blur. No self-shadowing or shadows on other objects (shadow maps planned). |
| 3D transparency | motion | per-frame | partial | `box ... opacity=0.5` | Triangles sorted back to front per frame (no order-independent transparency); transparent objects get no outline. |
| 3D GPU path | motion | per-frame | planned | `` | Planned (wgpu); the CPU rasteriser is the reference. |

#### Verbs (story level; compile into motion tracks)

| Verb | Syntax | Meaning |
|---|---|---|
| enter | `enter #id fade\|rise\|drop\|left\|right\|pop\|zoom\|blur\|wipe\|iris\|draw\|type\|flip\|count dur= ease= stagger=` | Make an element appear (also as element prop in=preset at=@anchor). |
| exit | `exit #id fade\|rise\|drop\|left\|right\|pop\|zoom\|blur\|wipe dur=` | Make it disappear (prop out=preset until=@anchor). |
| show / hide | `show #id ; hide #id` | Instant. |
| activate / deactivate | `activate #node for=2s ; deactivate #node` | Accent state of a node/card/stat (gradient, glow, slight pop): mark the active step of a diagram while it is spoken. |
| done / undone | `done #node ; undone #node` | Check badge on a node (native or skin part `done`); ends its active state. |
| advance | `advance #progress to=3` | Fill a progress component up to step 3 and count. |
| page | `page #panel to=<pane id>` | Swap a detail panel to another pane (blur swap). |
| emph | `emph #id pulse\|shake\|glow\|bounce\|wobble\|flash ; pulse #id ; glow #id for=1.5s` | Emphasis. On a hotspot: expanding ring. |
| highlight | `highlight #hotspot for=1.6s color=$accent2` | Ring + tint around a screenshot hotspot. |
| spotlight | `spotlight #hotspot for=2s` | Dim everything but the hotspot. |
| callout | `callout #hotspot "Label" side=right\|left\|top\|bottom for=2s` | Label next to a hotspot. |
| zoom / pan | `zoom #hotspot z=1.9 dur=1s ; zoom #element ; zoom out ; zoom x=0.7 y=0.3 z=2 ; pan #hotspot` | Camera inside a screenshot (clamped), or the scene camera onto any laid-out element (centered, z fits it). |
| cursor | `cursor -> #hotspot dur=0.6s dx= dy=` | Move the cursor (starts at the anchor). |
| click / dblclick | `click #hotspot sfx` | Cursor arrives AT the anchor, presses, ripple. |
| drag | `drag #hotspot dx=240 dy=0 dur=1.6s` | Press, drag, release. |
| type | `type #hotspot "text" ; type #textElement` | Types text into a hotspot field or reveals a text element. |
| swap | `swap #app shot=next.png [hotspots=next.json] via=fade\|cut dur=0.2s` | Next UI state (screenshot) with its own hotspots. |
| count | `count #n to=1280 dur=1.2s` | Animate a counter. |
| draw | `draw #path\|#chart\|#link dur=1.2s` | Trim-path draw-on / chart build. |
| diff | `diff #code` | Removed lines flash red and collapse, added lines expand green. |
| burst | `burst #particles` | Emit the particle burst. |
| move | `move #id x=120 y=-40 rot=10 scale=1.2 dur= ease= ; move #a to=#b` | Relative move (px); to=#b centers it on another element's box (x/y add on top). |
| set | `set #id opacity=0.4 color=$mint` | Instant channel change. |
| anim | `anim #id scale 0s:0.8 0.6s:1~spring(170,20) opacity 0s:0 0.3s:1` | Motion-level keyframes (several channels per line). |
| rewind | `rewind #g speed=4 dur=0.5s then=play\|hold ; rewind #scene ...` | Play the subtree's local time backwards at `speed`x for `dur`, then play on (default) or hold. speed=0 freezes. |
| timemap | `timemap #g 0s:0s 0.5s:-2s~in-quad 1s:-2s then=play\|hold` | Piecewise local time: <parent offset>:<local offset> keys relative to the event, eases between keys. |
| slitscan | `slitscan #g axis=y\|x span=1.2s band=4 dir=down\|up\|right\|left [dur= ease=]` | Each row (column) of the group shows it span*(position in its box) earlier. Repeat with a new span (and dur=) to animate it; span=0 = sharp. |
| echo | `echo #x n=6 dt=60ms decay=0.6 blend=normal\|add\|multiply\|screen colors=#f00,#00f` | n trailing copies at t - k*dt, opacity decay^k, tinted newest -> oldest. dt animates like span. |
| timedisplace | `timedisplace #g map=radial\|linear\|noise\|#layer span=0.8s levels=24 center=0.5,0.5 angle=0 scale=160 seed=1` | Per-pixel delay from a map (0 = now, 1 = span ago), quantized to levels; a #layer map is hidden and read as luma. |
| freeze | `freeze #x [for=1s]` | The subtree's time stops (the rest plays on); with for= it resumes from where it stopped. |
| pour | `pour #ink at=30%,80% dir=-80deg speed=700 radius=2.5% rate=3 wobble=14 dur=1s` | A jet of dye into a fluid: velocity pulled to speed along dir (0 = right, -90 = up), rate = dye per second at its center. |
| stir | `stir #ink at=50%,50% radius=18% strength=4 dur=0.8s` | A vortex in a fluid: rigid rotation of strength rad/s inside a Gaussian of radius (negative = counter-clockwise). |
| ink | `ink #title into #ink dur=0.5s amount=1.6 noise=420 speed=0 dir=-90deg [keep]` | The layer's shape becomes dye of the fluid (it fades out while it pours in, unless keep) with a curl-noise kick: type dissolves into ink or smoke. |
| reveal | `reveal #shot with #ink [mode=alpha\|alpha-inverted] [only]` | The layer shows only where the fluid's dye is (a matte); only = the fluid itself is not drawn. |
| animate | `animate #title by=char\|word\|line shape=ramp-up range=0%..25% offset=-25%..100% dur=1.6s ease=linear opacity=0 y=60 blur=10 ; animate #cap by=word times=[0s, 0.3s, 0.8s] dur=0.3s y=30 opacity=0` | Text animator (AE range selector or per-unit key times) on one text node: the selected units get the props scaled by their amount. |
| sweep | `sweep #card dur=1.4s angle=20deg width=25% color=#FFF1D6 strength=0.35 ease=` | A soft band of light crosses the layer (look `sweep`). |
| camera tour / zoom-through / follow / focus | `camera tour #a -> #b -> #c ease=spring(170,24) dwell=narration\|1.5s move=1.2s fit=0.85 ; camera zoom-through #a -> #b dur=5s ; camera follow #x fit=0.7 lead=120ms for=3s ; camera focus #k overlay=#000a pad=28 for=1.6s` | Camera director on world canvases: frames ids (safe area, fit), van Wijk zoom-pan paths, dark-overlay focus. Lint: E_CAMERA_TARGET, E_CAMERA_TINY, W_CAMERA_FAST. |
| camera loop | `camera loop #nest levels=6 dur=6s dir=in\|out zoom=1 wrap=on\|off` | Seamless infinite zoom into a `nest`: constant log-speed, exactly `levels` levels per `dur`, the nest turns back by its rot, then wraps. Lint: W_LOOP_SEAM, W_LOOP_CONTENT, W_LOOP_SELF_SIMILAR. |
| dolly / orbit | `dolly z=300 dur=3s ; orbit roty=15 ; orbit #world by=30deg pitch=10deg dur=4s ease= ; dolly #world by=2\|to=5 dur=` | 2.5D camera moves; on a scene3d id: the 3D camera circles its target (yaw, pitch) or moves toward it (by= forward, to= distance), baked per frame. |
| truck / pedestal / lookat | `truck #world by=1.5 dur=2s ; pedestal #world by=1 ; lookat #world #crate dur=1s ; lookat #world at=(0, 1, 0)` | 3D camera: slide sideways / up with the target, or aim at an object or point. |
| camera3d | `camera3d #world orbit=30deg pitch= dolly=2 truck= pedestal= lookat=#obj fov=30 roll=5 dur=4s ease=inout-cubic` | Several 3D camera moves in one eased move. |
| spin / tumble | `spin #crate axis=y by=360deg\|turns=1 dur=1.2s ease= ; tumble #die speed=120 dur=2s` | 3D object: turn about an axis; tumble about all three. |
| drop (3D) | `drop #crate from=3 bounce=0.35 g=25 ; box ... in=drop at=@1s from=5` | On a 3D object: falls from `from` above its height and bounces (baked per frame); on 2D layers the physics drop. |
| assemble / explode | `assemble #group dist=4 spin=160 stagger=70ms dur=0.9s seed=1 ; explode #group` | Pieces (the group's children, or the object itself) fly in from seeded offsets and turns, or fly apart and fade. |
| stack | `@1s stack #ship axis=y gap=0.04 from=3 stagger=180ms dur=0.45s` | Piles a group's objects along an axis from the group's origin, each falling in from `from` above (containers on a ship). Write it as an event (`@t`). |
| 3D presets | `box ... in=fade\|pop\|rise\|drop\|spin\|flip\|assemble out=fade\|pop\|sink\|spin\|explode` | Entrances and exits of 3D objects. |
| sfx | `sfx click\|pop\|whoosh\|file.wav gain=0.6` | Sound effect at the event. |
| focus / play | `focus #id ; play #video` | Planned. |

#### Channels (motion level)

| Channel | Stage |
|---|---|
| x | Composite |
| y | Composite |
| z | Composite |
| px | Composite |
| py | Composite |
| scale | Composite |
| sx | Composite |
| sy | Composite |
| rot | Composite |
| rotx | Composite |
| roty | Composite |
| opacity | Composite |
| blur | Paint |
| glow | Paint |
| brightness | Paint |
| saturate | Paint |
| contrast | Paint |
| hue | Paint |
| fill | Paint |
| color | Paint |
| stroke | Paint |
| trim-start | Paint |
| trim | Paint |
| reveal | Paint |
| mask-x | Paint |
| grad-angle | Paint |
| grad0 | Paint |
| grad1 | Paint |
| grad2 | Paint |
| grad3 | Paint |
| wght | Layout |
| tracking | Layout |
| w | Layout |
| h | Layout |
| count | Layout |
| halftone | Paint |
| sweep | Paint |
| rays | Paint |
| misreg | Paint |
| rgbsplit | Paint |
| rgbsplit-dx | Paint |
| rgbsplit-dy | Paint |
| glitch | Paint |
| glitch-rate | Paint |
| glitch-block | Paint |
| vhs | Paint |
| dither | Paint |
| ascii | Paint |
| wdth | Layout |
| opsz | Layout |
| slnt | Layout |
| path-offset | Paint |
| sz | PerFrame |
| cam-x | PerFrame |
| cam-y | PerFrame |
| cam-z | PerFrame |
| look-x | PerFrame |
| look-y | PerFrame |
| look-z | PerFrame |
| fov | PerFrame |
| roll | PerFrame |
| ambient | PerFrame |
| emissive | PerFrame |
| intensity | PerFrame |

## 16. Clarifications (rev 1, from the authoring test)

Questions the test agents asked, answered here.

- **Icons:** `panels upload sync check x arrow play chart code cloud db user lock bolt star gear search mail file
  globe server phone sparkle chat git eye rocket clock cart trend link cursor`.
- **`node` / `card` / `box`:** a text argument is the label, `sub=` a second line; child elements (an `icon`, text)
  follow below. `node` is a compact card for diagrams.
- **Visibility:** an element whose first event is `enter` (or `in=`) is hidden until then. `hide` hides it;
  `show`, `enter` and `draw` make it visible again. `in=` needs an id (`W_IN_NO_ID`).
- **Centering a block:** `place=center` (absolute, safe area). Flow containers are centered in the scene by default;
  `w=100%`/`h=100%` size a box but do not center its content (use `justify=`/`align=`).
- **`when`:** allowed anywhere (file, scene, inside elements, inside `say`). It may contain `set #id k=v` (any prop,
  including `dir`, `w`, `h`, `gap`, `size`), elements and `@anchor` events. Per-format camera or zoom moves are
  events inside `when` (example: `examples/showcase.reel`, scene `space`).
- **Screenshots in portrait:** a 16:9 shot in a 9:16 frame is a band. Use `crop=[x, y, w, h]` (shot px; added
  after the run from this feedback): the window shows that region at zoom 1, its aspect follows the crop, and
  hotspots, zoom, cursor and clicks keep working in shot coordinates (zoom can pan outside the crop). Typical:
  `when portrait` + `set #app crop=[0, 380, 960, 1240]`. `aspect=` takes `16:9` or a number.
- **Hotspots per state:** `swap shot=b.png` loads `b.json` (or `hotspots=`); inline `hot` lines belong to the
  `browser` and stay valid across swaps. Different inline hotspots per state: one scene per state.
- **Counters:** integers only in S0; a thousands separator (`sep=.`) and decimals are planned.
- **Charts:** `unit` is appended to every value label; line/area charts label the x axis (a y axis is planned); pie/donut legends sit right of the ring (give the chart `w=` room);
  chart parts are addressable after the run (added from this feedback): `#rev.b4` = 4th bar, `#conv.d6` = 6th
  point, `#dev.s1` = 1st slice (1-based), e.g. `@Quartal pulse #rev.b4 scale=1.15 ; glow #rev.b4`; line/area
  charts take the flag `values` for value labels above the points.
- **Anchor names:** `[[text]]` uses the text as name; names are letters (incl. umlauts), digits, `_`, `-`.
  Use `[[any text|name]]` for anything else.
- **Lint output:** JSON on stdout, a summary line on stderr (`lint: 0 errors, 0 warnings, 0 info (16:9, 9:16, measured)`).
- **Connectors:** measured after layout with all animation off (entrances do not affect them). `lint --measure` checks
  connector geometry (missing endpoints, leaving the frame, too short).

## 17. Design layer (D1)

The look of a video comes from a design spec, not from per-element styling. Details and the workflow:
`design/AUTHORING.md`; rules: `design/PRINCIPLES.md`.

- `design lens=<name>` (built-in looks: `swiss editorial keynote flat blueprint`, `design/lenses/`)
  or `design src=design.reel` (a spec file) or an inline `design "Name" base=light|dark` block with the
  theme lines plus `surface style=`, `elevation shadow=`, `icons style=`, `scale components= diagram= chart=`,
  `skin node=`, `texture`, `space`, `treatment`, `do "..."`, `dont "..."`. Type roles: `type display="Family"
  body="Family" base=40 ratio=1.333 display-weight= ...` (sizes follow the modular scale).
- Per-step detail: `progress #p n=6` + `advance #p to=3`; `panel #d` with `pane #id title=` and
  `item "text" chip= status=ok|run|wait|fail` lines + `page #d to=<pane>`; `done #node` / `undone #node`
  (check badge); `chart ... title= kicker= source= note=` (chart frame and an annotation on the highlighted bar).
- Skins: `node ... skin=tile|step|tag` draws the node from an HTML/CSS skin bound to the node contract
  (slots label/sub/icon/n/kicker; parts active/done), with the spec's tokens.
- Motion defaults are measured (`design/MOTION-REPORT.md`): `enter` 0.4 s out-cubic, 28 px travel; `exit`
  0.3 s in-quad; `move` 0.63 s inout-sine; tokens `$stagger` (133 ms) and `$beat` (0.43 s).
- New lint (L0): `W_SCALE_FILL W_FOCAL W_BEAT_MOVES W_TEXT_DENSITY W_WORDY W_READ_SIZE W_EXIT_SLOW W_STAGGER
  I_ROLE_TREATMENT` and the design spec checks `E_DESIGN_TOKEN E_DESIGN_FONT W_DESIGN_CONTRAST`.
- Tools: `agentfx design harvest <frame.html>` (style frames to a spec), `agentfx design pitch <brief>`
  (divergent style frames + contact sheet), `agentfx critique <sheet|video>` (VLM critic), `agentfx motion mine`.

## Motion energy and taste lint

The compiler computes a **motion-energy map** from the motion IR, without rendering: for every frame the
share (0..1) of the frame area that changed since the previous frame. Every visible, paint-bearing layer
whose world transform, world opacity or own channel value changed contributes the union of its previous
and current screen box (rest layout from the native Blitz layout, channels from the tracks, the same
`motion::world` chain the renderer uses); overlaps count once. Layers that animate by themselves (video,
`cloud`, `fluid`) count while visible; film grain and static looks do not. Layout-stage channels (`w`,
`h`, `count`, ...) count the layer's whole rest box. It is an area measure (not a pixel diff), so a
fade of a full-frame layer is 100 %. Code: `src/energy.rs` (`energy::compute`, `EnergyMap`).

- `agentfx energy <file> [--format F] [--bins 100] [--json]`: a block-character strip of the energy over time
  plus mean/peak/still seconds, or the per-frame array as JSON.
- `agentfx sheet ... --energy`: a motion bar under every thumbnail (max energy within +-0.5 s of the frame)
  and the percentage in its label.
- `agentfx lint <file> --motion` (or `--measure`, which includes it) runs the taste rules (`src/taste.rs`):

| Code | Fires when | Fix |
|---|---|---|
| `W_COLD_OPEN` | nothing moves until after frame 2 (keyframes, `principles.rs`) | start motion on frame 0 |
| `W_SMALL_MOTION` | a span of motion (gaps under 0.5 s merged) lasting >= `small-span` (1.5 s) never moves more than `min-area` (8 %) of the frame | scale the hero to >= 40 % of the frame or add a camera move |
| `W_DEAD_AIR` | nothing moves (energy <= 0.3 %) for >= `max-still` (1 s) of narration, or >= `max-still-silent` (2 s) in a film without narration; still start and still tail are left to cold-open / static-end | anchor a beat to a word in that line, or add a slow camera drift |
| `W_STATIC_END` | the film ends on >= `end-still` (2 s) without motion, or the last `end-window` (2 s) only move a small element (peak under `min-area`) | end on a move; keep the final hold short |

Thresholds are theme `lint` tokens (`lint min-area=10% max-still=0.8s`, `lint motion=false` to switch the
rules off). Defaults were tuned on the examples: the timid single-element pieces (single-shot frames,
`rewind-src`) warn, the full-stop films and the presenter demo mostly do not.

## Presenter

**Compiler-only spike.** A `presenter` draws a vector character with a nested bone
rig and ordinary transform and opacity tracks. It requires no renderer extensions.

```reel
presenter #mira skin=flat x=72% y=55% h=70% look=#title seed=7
say "The [[panel]] is ready. [[Welcome]] back."
  @panel gesture #mira point #tabs
  @Welcome gesture #mira wave ; mood #mira happy
```

`x` and `y` locate the character's centre; `h` is its frame-relative height
(default 70%). Use `anchor=` for another alignment. `seed=` defaults to 1.
`color=` overrides the skin colour (`$mint`); `accent=` overrides the shirt
(`$accent`). Facial ink uses `$ink`; eye whites and teeth use `$paper`. Define
theme tokens or use literal colours. The character is stylised: large head, simple
shapes, big readable eyes and mouth, no gloss. Frame it large (`h=70%` to `90%`).

| Syntax | Motion |
|---|---|
| `gesture #mira point #target` | Two-bone arm IK toward the target's native layout centre |
| `gesture #mira present #target` | Reach toward the target with an open palm |
| `gesture #mira wave` | Raised arm with repeated wrist motion |
| `gesture #mira nod` | Head dip and recovery |
| `gesture #mira shrug` | Both shoulders and forearms rise |
| `gesture #mira think` | Hand moves to chin |
| `gesture #mira count 3` | Raised hand shows 1 to 5 fingers |
| `mood #mira happy` | Change brows and the resting mouth |

Moods are `neutral`, `happy`, `serious`, and `surprised`; an initial `mood=` is
also accepted. Skins share the rig and differ only in how the art is drawn:
`flat` uses solid fills from `$mint`, `$accent` and `$ink`; `line` uses one outline
colour at one stroke weight (the interior is filled with `$bg` so limbs occlude the
torso); `paper` is a cut-paper look with straight scissor facets from seeded edge
jitter, a slightly different tone per sheet, an offset shadow under every piece,
and warm defaults (cream skin, `$warn` shirt). Every skin is deterministic per `seed=`.
`look=#id` and targeted gestures turn the pupils and head toward an element.
Gestures approach in 0.32 s, hold, and return in 0.35 s. Hold times: `point` and
`present` 1.6 s, `think` 1.3 s, `wave` 1.5 s (seven wrist swings), others 1.2 s.
A pointing hand shows one finger. Forearms follow the upper arms by 60 ms and
hands by 120 ms; later events interrupt earlier motion on the same channel.
The presenter springs in from the side at scene start (motion on frame one), and
the head dips slightly on every spoken word.

Every presenter in a scene follows that scene's `say` word timings, including
estimated timings with `--no-tts`. German and English graphemes approximate eight
mouth poses: rest, A/I, E, O, U/W, M/B/P, F/V, and L/T/D/N. Poses last at least
two frames; gaps between words return to rest when the frame budget allows.
Seeded blinks last 120 ms, spaced 2.5 to 5 seconds apart. Breathing has a four
second cycle with a 1.2% scale excursion, accompanied by head and torso sway.

Limits: this is orthographic lip sync, not phonetic alignment. There is no speaker
selection when several presenters share a scene. Targets use their layout boxes
at rest, not their animated positions. Unreachable targets clamp to arm reach.
The rig is frontal 2D, with no occlusion solving or finger articulation beyond
count poses. Head gestures replace idle head sway on that channel.
Use frame-relative `h` or pixels; nested scaling and rotated target containers
are outside this spike's IK contract. Unknown skins, gestures, moods, or target
ids produce `E_PRESENTER` with a fix hint. Heights below 25% of the frame produce
`W_PRESENTER_SMALL`.

## Physics and fracture

Direct-mode physics is simulated once by the Script compiler against native layout
rectangles, at a fixed 1/240 s step (with a final partial step at output frame times).
The result is ordinary per-frame x/y/rotate tracks; rewind/timemap apply afterward.
No renderer physics state or new renderer IR types are required.

```reel
@1s drop #letters into #box gravity=2400 bounce=0.35 friction=0.6 stagger=60ms
@2s pile #cards at=#floor
@0.5s swing #sign from=#hook angle=35deg damping=0.15
@3s push #ball dir=30deg speed=900
@0s shatter #logo pieces=24 seed=7 origin=40%,60% force=1600 gravity=1800
@3s rewind #film speed=1 dur=3s
```

`drop` and `pile` release the target's direct children, or the target itself if
it is a leaf. Use `split=char` on text to make glyph children. `into #box` uses
its interior walls and floor; `at=#floor` uses the floor's top and horizontal
extent. Otherwise `walls=frame` is the default; `walls=none` allows escape.
`push` supplies initial velocity (degrees clockwise from right) with default
zero gravity. `swing` uses a damped pendulum about the hook's center, with the
angle measured from the initial tether direction. `dur` defaults to scene end.
Gravity is px/s², speed/force are px/s, bounce and friction are in 0..1. `spin=<deg/s>` and
`scatter=<px/s>` give each body a deterministic, index-seeded initial angular and sideways
velocity (tumbling piles). Text bodies collide on their cap-height band (about 20 % to 81 % of
the line box), not the full line box.

`shatter` clips a leaf's original content into 2..128 seeded Voronoi polygons.
Cells exactly partition its rectangle. The intact original is shown at the
initial instant, avoiding antialias seams at assembly; shards take over as they
move. `origin` sets the outward impulse center (percentages of the source).
The source remains a group so remapping it or its ancestors reassembles shards.
Seeds are nonnegative integers, default 7; pieces default 24.

The v1 fallback solver uses convex polygon separating axes and linear collision
impulses with damping, eight contact iterations, equal masses and no collision
torque. Ellipses use 32-sided convex collision polygons. Sleeping bodies stop adding
keys after 120 quiet substeps. Swing is an analytic tether and ignores collision
response. Each physics command starts a fresh simulation, taking existing
position and rotation at that time, but not prior velocity. Avoid transformed
ancestors and overlapping physics commands on interacting groups. Fracture
currently requires a leaf, not a group or split text; it replaces existing leaf
masks. At most 256 bodies and 120 seconds per command. Invalid parameters,
unknown targets/colliders and empty bodies report `E_PHYSICS` with a repair hint.

## Digital looks and export controls

The direct backend supports these subtree effects alongside `riso=`, `halftone=`
and `grain=`. See `examples/features/looks.reel`. Effects use a fixed order:
riso, halftone, grain, paper, sweep, shadow, rays, rgbsplit, glitch, vhs, dither, ascii.
Unknown effect-prefixed parameters are lint errors.

| Property | Parameters and ranges | Animation channels |
|---|---|---|
| `rgbsplit=1` | amount 0..1; `rgbsplit-dx=7`, `rgbsplit-dy=0` (-200..200 px); `rgbsplit-mode=linear\|radial` | `rgbsplit`, `rgbsplit-dx`, `rgbsplit-dy` |
| `glitch=0.5` | amount 0..1; `glitch-rate=6` (0..120/s), `glitch-block=22` (1..1024 px), shared `seed=1` | `glitch`, `glitch-rate`, `glitch-block` |
| `vhs=0.5` | amount 0..1; shared `seed=1` | `vhs` |
| `dither=3` | levels 2..256; `dither-mode=bayer\|bluenoise\|floyd`, `dither-size=2\|4\|8` (default 4); optional `dither-palette=[#000, #fff]` of 2..16 opaque colors | `dither` |
| `ascii=8` | cell 6..24 px; `ascii-charset=" .:-=+*#%@"` (nonempty subset in darkness order), `ascii-color=source\|mono`, `ascii-bg=#000` | `ascii` |

For example, `anim #title rgbsplit 0s:0 1s:1` emits an effect-field track.
RGB split, glitch and VHS are identity at zero amount. Glitch and VHS vary with
time; dither and ASCII are stable for static input.

`image src=` and image paints accept local PNG and JPEG files (content detection,
independent of extension). Direct compilation normalizes JPEG assets to PNG for
the renderer. Unsupported or unreadable image formats produce `E_ASSET` in lint.

Text with `split=word` or `split=char` honors `nowrap`; gradient fills paint the
glyphs across the split group's coordinate space. `origin=30%,70%` sets a pivot
relative to the element box; percentages may lie outside the box. Named presets
are `center`, `top`, `bottom`, `left`, `right` and the four hyphenated corners.

`agentfx render clip.reel --mode cfr -o master.mov --alpha` selects CFR and passes
alpha to the renderer. The default remains VFR. Output extension selects MP4,
ProRes MOV, VP9 WebM, GIF, animated WebP, or a PNG sequence. Non-MP4 exports use
CFR in the renderer. MP4 cannot carry alpha. The scene must itself contain
transparent pixels for an alpha export to be transparent.

Audio is muxed with copied video for MP4/MOV (AAC) and WebM (Opus). GIF, WebP and
PNG sequences omit audio with a message. `--cache` uses the existing segment
cache for opaque VFR MP4 only; other combinations render directly. These flags
apply to the direct backend.

## Audio reactivity

Music-synced motion without renderer support: the compiler analyses the `music` bed offline and
lowers everything to ordinary elements and linear keyframe tracks (one key per output frame,
collinear keys dropped). Example: `examples/features/audio.reel`.

**Analysis** (`src/audio.rs`, deterministic). ffmpeg (`$AGENTFX_FFMPEG`, else `ffmpeg` on PATH)
decodes the first `music src=` to 16 kHz mono. Per output frame at the video fps:

| Feature | Meaning (all 0..1, normalized to the loudest frame of the track) |
|---|---|
| `level` | RMS loudness of the frame |
| `onset` | onset strength: spectral flux of log magnitudes, slow trend removed |
| `band=0..7` | energy in 8 log-spaced bands from 40 Hz to 8 kHz (0 = bass); fractional bands such as `band=2.5` blend neighbours |

The beat grid: onset autocorrelation with a 120 bpm prior picks the tempo (50..200 bpm),
refined against a comb filter over the whole track (no drift on long tracks), then the
best-fitting phase gives beat 1. `music ... bpm=128` overrides the tempo; the phase is still
fitted. Results are cached by content hash, fps and bpm in `<doc dir>/cache/audio/<sha>.json`.
Tracks longer than 10 minutes are rejected. Documents that use none of the features below never
run the analysis, so their output is unchanged.

**Driving any numeric channel.** `<channel>=audio(<feature>, gain=1, smooth=0ms, offset=0)` as a
prop of an element with an id: the value is `offset + gain * feature`, smoothed by a one-pole
filter with time constant `smooth` that runs from the track start (scene cuts do not reset it).
Any numeric animatable channel works (`scale sy sx opacity glow blur rot x y brightness ...`);
opacity is clamped to 0..1. The track replaces the channel for the whole scene, so do not also
animate that channel with `in=` or verbs.

```text
rect #bar w=40 h=300 origin=bottom scale-y=audio(band=3, gain=1.5, smooth=80ms)
image #logo src=logo.png opacity=audio(level, offset=0.3, gain=0.7, smooth=150ms)
ellipse #core w=200 h=200 glow=audio(band=0, gain=40, smooth=120ms)
```

**Beat pulses.** `pulse #id on=beat|bar amount=0.15 decay=220ms [channel=scale]`: an instant
attack on every beat (or every bar = 4 beats), quadratic settle over `decay`. As a child of an
element it targets that element. Without `on=`, `pulse` is the ordinary emphasis verb.

**Beat anchors.** `beat:N` and `bar:N` (1-based, 4 beats per bar) are anchors in every scene,
in absolute video time, usable wherever anchors are: `at bar:8`, `at beat:32+0.1s`, `at=@beat:4`,
`until=@bar:9`, keyframes `@beat:5:1.2`, `agentfx frame --at beat:16`. A beat past the end of the
track is `E_BEAT_RANGE`; one after the end of the video is `W_BEAT_PAST_END`. Downbeats are not
detected: `bar:N` is beat `4N-3`, so it matches the music's bars when the track starts on a
downbeat (trim it there, or use `beat:N`).

**`visualizer`** lowers to a `layer` (with the given `w h x y anchor ...`) of `rect` children
with per-frame tracks:

```text
visualizer #id type=bars|radial|circle|waveform count= w= h= gap= radius= thick= colors=[..] mirror gain= smooth=
```

| Type | Look | Defaults |
|---|---|---|
| `bars` | vertical bars from low (left) to high bands, growing from the bottom; `mirror` puts the bass in the middle and grows both ways from the center line | `count=32 w=960 h=280 gap=6 smooth=80ms` |
| `radial` (`circle`) | bars around a circle of `radius`, growing outward, low to high bands clockwise from 12 o'clock; `mirror` makes it symmetric (bass at 12, highs at 6 o'clock) | `count=32 w=h=600 radius=min(w,h)/4` |
| `waveform` | the frame's waveform as a polyline of `count-1` segments of width `thick`, ends pinned | `count=64 thick=4` |

`colors` cycle over the bars or segments (default `$accent`), `gain` scales the response. Bar
children are addressable as `#id.b1..` (waveform segments `#id.s1..`).

**Lint.** `E_AUDIO`: no `music`, file unreadable or undecodable, bad `bpm`, bad `audio(...)`
parameters or channel, missing id. `W_AUDIO_FLASH`: an audio-driven `opacity` or `brightness`
track has more than three flash pairs (opposing swings of at least 0.1) within one second
(WCAG 2.3.1 general flash threshold; a signal check, not a luminance/area measurement). Raise
`smooth=` or drive `scale` instead.

Limits: one music bed drives everything (the first `music`); features follow the source file from
its start (no trim/offset yet); `visualizer` inside `when <format>` blocks is not supported
(its tracks live at scene level).

## Text v2

Custom fonts, variable axes, OpenType features, text stroke, text color tracks, text animators
and text on a path. Direct mode draws all of them natively (reel-ir `GlyphStroke`, `TextFx` on
the text's `paint`, see reel `IR.md`); the Chrome reference gets the CSS equivalents where CSS
has one (fonts, axes, features, `-webkit-text-stroke`, color) and ignores animators and paths.

```
font "Brand" src="fonts/Brand.woff2"

scene s dur=6s
  text #t "Variable type" font=Brand axes="opsz 144, SOFT 0" features="ss01, tnum" stroke=#000 sw=4
    anim wght 0s:200 1.6s:900~inout-cubic
    anim SOFT 0s:0 1.6s:100
  @0.5s animate #t by=char shape=ramp-up range=0%..25% offset=-25%..100% dur=1.6s opacity=0 y=60 blur=10
  path #wave d="M0 160 C240 0 480 0 720 160" w=720 h=320
  text #ride "on a path" on=#wave offset=0
    anim offset 0s:0 3s:400~out-cubic
```

- **`font "Name" src=`** (top level): the file is decoded (WOFF/WOFF2 with `wuff`) and checked
  at compile time and registered under `Name` in the native layout (Parley); TTF/OTF are
  referenced in place, decoded WOFF/WOFF2 are written next to the IR. `E_FONT`: no name, no
  `src`, wrong extension, missing or undecodable file, declared twice. `W_FONT_FAMILY`: a
  `font=` that names no registered or declared family (with "did you mean"). The caption pack
  still measures with the registered families only.
- **`axes="wdth 80, GRAD 50"`**: static variable axes (CSS `font-variation-settings`). Animated
  axes are channels: `wght` (alias `font-weight`), `wdth`, `opsz`, `slnt` and up to four custom
  uppercase tags per video (`anim #t GRAD 0s:0 1s:100`). Axes change advances, so they are
  layout channels: the compiler relays out per frame (repaint states), like `wght` before.
  `E_VALUE` on bad syntax.
- **`features="ss01 off, tnum, cv05 2"`**: OpenType features (`font-feature-settings`), `tag`,
  `tag on|off` or `tag <n>`. They replace the stage default `'cv11','ss01'` (Inter's look) on
  that text. `E_VALUE` on bad syntax.
- **`stroke=<color> sw=<px>`** on text: a real outline from the glyph outlines, drawn by the
  rasterizer (no drop-shadow stack). `sw` is the visible outline width: the stroke is drawn
  below the fill at `2*sw`; `stroke-over` draws it centered over the fill at `sw`. Round joins.
  Contrast lint takes the better of fill and stroke.
- **`color` on text** animates as one reel-ir `text.fill` track (no repaint states) when the
  text has plain runs (no `{accent}`, `` `code` `` or explicit run colors), no text matte and
  no child layers (`split=` text keeps repaint states). Pixels match the repaint path.
- **`animate #t by=char|word|line ...`** (verb): an After Effects text animator on one text
  node. Units are found in the shaped glyphs: `char` = every glyph that is not a space (spaces
  ride with the unit before them), `word` starts after a space or a line break, `line` = one
  baseline. Properties at full amount: `opacity` (multiplied), `x y` (px), `scale sx sy` and
  `rot` (deg) about the unit's box center, `blur` (px sigma, per unit), `color` (mix of the fill
  toward it). Several `animate` on one text compose in order.
  - Range selector (default): `range=start..end` (fractions or %, default `0%..100%`), `offset=`,
    `amount=` (default 1), `start=`/`end=`; any of them written `a..b` animates from a to b over
    `dur=` (default 1s) with `ease=` (default out-cubic) from the event time, holding before and
    after. Unit i of n covers `[i/n, (i+1)/n]` in `order=forward|reverse|random` (`seed=`).
    `shape=square` (covered fraction, partial at the edges), `ramp-up` (alias `ramp`: 0 before
    the range, 1 after it, linear inside), `ramp-down`, `triangle`, `round`, `smooth` (cosine
    bell; 0 outside). A soft wipe in: `shape=ramp-up range=0%..25% offset=-25%..100%`.
  - Per-unit key times: `times=[0s, 0.3s, 0.9s]` (from the event, one per unit; units past the
    list use the last) with `dur=` per unit and `ease=`; `dir=in` (default: from the props to
    rest) or `dir=out`. This is how spoken-word timing drives one text node.
  - Lint: `E_TARGET` (not a text element; `split=` text is several layers, animate the whole
    text instead), `E_VALUE` (by, shape with "did you mean", order, range/offset syntax, dir,
    times), `W_ANIMATE_PROP` (unknown prop, or no property set).
- **`on=#path offset=<px>`** on text: the glyphs ride the path's rest geometry (scene space):
  glyph origin at arc length `x - x0 + offset`, rotated to the tangent at the glyph's center,
  lines below the first stacked along the normal; glyphs off either end hide. `offset`
  animates with the `path-offset` channel (`anim #t offset 0s:0 2s:600`). The text is laid out
  on one line (`nowrap`). Animators apply on top of the placement.
- **Emoji**: the renderer draws outline glyphs only; there is no color emoji font on the build
  machines (`fc-list :charset=1f600` is empty), so emoji keep the `W_CAPTION_GLYPH` warning in
  captions and draw nothing elsewhere.

Limits: animators, paths and `text.fill` need a text whose paint does not change otherwise (no
animated `wght`/axes/tracking/size on the same text: those become repaint states and the
animator is reported as unsupported); time remaps (`rewind`, `timemap`) do not remap animator
selectors or per-unit key times; text on a path uses the path's rest position (a moving path
does not carry the text); the HTML/Chrome backend ignores animators and paths.

## Stylised 3D (scene3d)

Simple stylised 3D for motion graphics: extruded letter blocks, boxes, a stage with a screen, a
ship with containers, orbit and dolly camera moves; flat or toon shading with crisp outlines,
glow and fog. Not photoreal (no PBR, no global illumination). A `scene3d` is one layer of the 2D
pipeline: every look, effect, blend mode, mask and motion blur applies to it, and 2D layers can
sit above or below it, or on a `screen` inside it.

```text
scene3d #world fov=40 camera=(0, 2, 8) target=(0, 0, 0) light=(-3, 5, 4) ambient=0.35 fog=#05010D fog-near=10 fog-far=30
  floor y=-1 color=#0B1A10
  box #crate w=2 h=1.4 d=2 color=#C9A36B material=toon outline=2 outline-color=#2A1A0A
  extrude #L "L" depth=0.6 size=1.6 color=#7CFF6B emissive=0.6 x=-3
  screen #crt w=4 h=2.25 z=-2 src=#ui        // #ui is a normal 2D layer in the same scene
@0s orbit #world by=30deg dur=4s ease=inout-cubic
anim #crate ry 0s:0deg 2s:180deg~out-back
```

- **World.** y is up, z toward the viewer, units are free (think metres). Positions `x= y= z=` or
  `pos=(x, y, z)`; rotations `rx= ry= rz=` in degrees (`rot=30` turns about y), applied yaw last,
  so a spin about y stays upright on a tilted object; `scale=` (number or triple), `sx= sy= sz=`,
  `pivot=(x, y, z)`. Objects nest: children of a `group` (or of any object) move with it.
  Vectors are written `(x, y, z)`, `[x, y, z]` or `x,y,z`.
- **Scene line.** Camera: `camera=` (position), `target=`, `fov=` (vertical, degrees; default
  40), `roll=`, `near=`/`far=`, `ortho=<world units visible vertically>` or `iso` (an isometric
  orthographic camera). Light: `light=(x, y, z)` (a directional key light shining from there
  toward the origin; `light=off` for none), `light-color=`, `light-intensity=` (0.75),
  `ambient=` (0.4, a little brighter from above), `ambient-color=`. `fog=<color>` with
  `fog-near=`/`fog-far=` (linear, default from the camera distance) or `fog-density=` (exp).
  `bg=` (default transparent), `aa=1..4` (supersampling per axis, default 2), `bloom=<px>`
  `bloom-strength=` (default on with emissive objects, `bloom=off`), `shadow=<opacity>|off`
  `shadow-blur=<px>` `shadow-color=`, and defaults for all objects: `material= outline=
  outline-color= color=`. Without `w/h/x/y/place` the scene fills the frame (in the scene's
  world). Other props are the layer's own (placement, `glow=`, looks, `opacity=`, ...).
- **Objects.** `box w= h= d= bevel=` (rounded edges), `cube size=`, `plane w= h=` (faces the
  camera at rest, drawn from both sides), `floor y= size=` (a plane facing up that receives
  shadows; `ground` marks any plane as a receiver), `sphere r= seg=`, `cylinder r= h= r2=`,
  `cone r= h=`, `torus r= tube=`, `extrude "Text" size= depth= font= weight= tracking=` (the
  glyph outlines of the text stack, nonzero fill, centered on the ink box; `size` is the font
  size in world units) or `extrude d="M..." size=` (an SVG path, `size` = its height), `mesh
  src=model.obj size=` (Wavefront OBJ, normalized to `size`), `screen w= h= src=#layer res=`,
  `group`, `light point=(x, y, z) | dir=(x, y, z) color= intensity= range=`.
- **Materials.** `material=flat|lambert|toon|phong|wire` (default `toon`), `color=`,
  `opacity=`, `emissive=` (self-light, adds `emissive x color` and feeds the bloom),
  `bands=` (toon steps, 3), `specular= shininess=` (phong; toon draws a hard highlight),
  `outline=<px>` `outline-color=` (screen-space lines on silhouettes and on creases sharper than
  `crease=` degrees, from the depth/normal buffer, so they stay crisp at any size), `wire=<px>`
  `wire-color=` (the mesh's feature edges as lines; `material=wire` draws only them),
  `double-sided`, `texture=img.png` (box faces, planes), `shadow=off` (casts no shadow).
- **Screens.** `screen src=#ui` shows a 2D layer of the same scene on a plane in 3D: the layer is
  hidden in the 2D pass and rendered into a mip-mapped texture over its laid-out box whenever it
  changes (code typing, charts drawing, kinetic type). Give the source an absolute box
  (`layer #ui x=0 y=0 w=1920 h=1080`) so it takes no space in the scene's layout. `res=` sets
  texels per px (default from the screen's size on screen). The back of a screen shows its
  `color=`.
- **Camera verbs** (target: the scene3d's id; all baked into per-frame camera keys, so arcs stay
  arcs): `orbit #w by=30deg pitch=10deg` (around the target), `dolly #w by=2` (toward the
  target; negative = back) or `to=5` (distance), `truck #w by=1` (sideways with the target),
  `pedestal #w by=1` (up with the target), `lookat #w #obj` or `at=(x, y, z)`, and `camera3d #w
  orbit= pitch= dolly= truck= pedestal= lookat= fov= roll=` for several at once; all take
  `dur= ease=`. `anim #w cam-x ... look-y ... fov ... roll ...` keys them directly. `orbit` and
  `dolly` on anything that is not a scene3d keep their 2.5D meaning.
- **Object verbs.** `spin #o axis=x|y|z by=360deg|turns=1`, `tumble #o speed=120` (deg/s),
  `drop #o from=3 bounce=0.35 g=25` (a bouncing fall onto its current height), `assemble #g
  dist=4 spin=160 stagger=70ms seed=` (the group's pieces fly in from seeded offsets), `explode #g`
  (they fly apart and fade), `@t stack #g axis=y gap=0.04 from=3 stagger=180ms` (piles the
  group's objects from its origin, each falling in; write it as an event). Presets: `in=fade|pop|
  rise|drop|spin|flip|assemble`, `out=fade|pop|sink|spin|explode`; `anim`, `set` and `move` work
  on the object channels (`x y z rx ry rz scale sx sy sz opacity color emissive`, lights: `x y z
  intensity`).
- **Rendering** (`crates/render/src/s3d.rs`): a CPU triangle rasteriser with a z-buffer,
  fixed-point edges with a top-left tie rule (no cracks, no double hits), perspective-correct
  interpolation, near-plane clipping, back-face culling, `aa x aa` supersampling with shading once
  per pixel and triangle (like MSAA), per-pixel lighting, toon bands with a soft shoulder (a fully
  lit face shows its exact colour), fog, outlines (nearest seed within the width, exact distance),
  wire lines, planar ground shadows softened by a blur, transparent surfaces sorted back to front,
  bloom of emissive light at half resolution. Bands of rows run in parallel, each walking the
  triangles in one global order, so frames are bit-identical across runs, thread counts and
  segments. **Planner:** the content's signature is the evaluated 3D state (camera, lights,
  objects), so a still scene is drawn once and every later frame is a repeat; the raster is cached
  per node and reused when 2D layers around it change. Screen sources are dependencies like matte
  sources. Cost on a 6-core desktop CPU at 1920x1080, `aa=2`: about 150 to 250 ms per frame while
  the camera moves (most in shading and outlines), 0 ms at rest.
- **Lint.** `W_S3D_PROP` (unknown prop, did you mean), `E_S3D` (bad value, unknown material),
  `E_S3D_KIND` (not a 3D object inside a scene3d, or a 3D object outside one), `E_S3D_SCREEN`
  (missing or unknown `src=`, a source that is 3D or contains the scene), `W_S3D_SCREEN` (source
  in the layout flow), `E_S3D_TARGET` (3D verb on a non-3D id), `W_S3D_VIEW` (an object behind the
  camera or outside the frame for all its visible time, sampled every 0.2 s), `W_S3D_CLIP`
  (crosses the near plane), `W_S3D_SIZE` (absurd size, smaller than 2 px on screen),
  `W_S3D_ORBIT` (an orbit circles target= while the content sits elsewhere, so the content slides
  more than 15 % of the frame width: set target= to the content or aim with `lookat=#obj`).
- **Limits.** No shadow maps (shadows fall only on receiving planes), no bevel on extrusions, no
  glTF, OBJ without materials, no order-independent transparency, no GPU path yet. Direct mode
  only (the HTML/Chrome backend leaves the box empty).
  Examples: `examples/features/3d.reel`, `3d-letters.reel`, `3d-screen.reel`, `3d-shapes.reel`.
