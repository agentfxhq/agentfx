# Animation kinds menu

What to offer per beat in stage 4. Checked against `agentfx catalog --md` and `docs/SPEC.md`
(AgentFX 0.1.0). Before offering anything not listed here, run `agentfx catalog --md` and check its
`Status` column. Offer 2 to 4 per beat that fit its intent and energy; name them in plain words,
keep the syntax for yourself (it goes into `kinds[].syntax`).

Status: **implemented** (offer freely), **partial** (offer with the limit stated),
**planned** (may be shown greyed with "planned", never chosen).
"Direct only" means the native renderer path (the default); the HTML backend ignores it.

## Type

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Kinetic type, word or letter | `text ... split=word\|char in=rise stagger=80ms` | implemented | hooks, claims, low to mid energy |
| Line by line | `split=line in=rise stagger=180ms` | implemented | longer sentences |
| Typed text | `enter #t type` / `terminal` | implemented | product, code, calm |
| Weight animation | `anim #t wght 0s:300 1s:900` | implemented (variable fonts only) | emphasis on one word |
| Social captions | `captions style=pop\|pill\|karaoke\|hormozi\|typewriter` | implemented | voice-over films, 9:16 |
| Gradient / image in type | `fill=linear(...)` / `fill=image(a.png)` | implemented | hero words |
| Text on a path | `textpath` | planned | |

## Camera and space

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| World canvas camera | `world` + `camera tour\|zoom-through\|follow\|focus` | implemented | journeys, steps, maps of ideas |
| Infinite zoom | nested `place ... scale=0.01` + `camera zoom-through` | implemented | "zoom into" reveals |
| Zoom / pan onto an element | `zoom #el` / `pan #el` | implemented | UI focus |
| 2.5D parallax | `camera persp=1200`, `layer z=-400`, `dolly`, `orbit` | partial (planes stay flat) | depth on stills |
| Stylised 3D | `scene3d #w camera=(0, 2, 8)` + `box`, `extrude "L"`, `screen src=#ui`; `orbit #w`, `dolly #w`, `lookat`, `spin`, `drop`, `assemble`, `stack` | implemented (planar shadows, no glTF) | letter blocks, product stages, UI on a screen in 3D |
| Depth of field | `camera dof=` | planned | |

## Transitions

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Cut, fade, push, slide, wipe, iris, zoom | `scene x in=...` | implemented | everything |
| Magic move | `scene b in=magic` (same ids) | implemented | continuity between beats |
| Mask transitions | `in=mask(shape.svg)` | planned | |

## Physics and fracture (direct only, baked at compile time)

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Shatter | `shatter #x pieces=18 force=1500 gravity=3600` | implemented | release, drops, "breaking" problems |
| Push, drop, pile, swing | `push #x dir= speed=` / `drop #x into #box` / `pile` / `swing` | implemented | playful energy, weight |

## Fluid, particles, point clouds (direct only)

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Type dissolves into ink | `fluid #ink` + `ink #title into #ink` + `stir` | implemented | soft transitions, endings |
| Ink reveal | `reveal #shot with #ink` | implemented | revealing images |
| Pour | `pour #ink at= dir=` | implemented | openings |
| Particle burst | `particles #p ... ; burst #p` | implemented | celebrations, hits |
| Point cloud formations | `cloud n=16000` + `form text\|galaxy\|sphere\|terrain\|spiral` | implemented | numbers, data, logos made of dots |

## Time

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Rewind / time remap | `rewind #g speed=4 then=hold` / `timemap` | implemented | undo, "what if", tape stops |
| Freeze | `freeze #x for=1s` | implemented | a held moment |
| Echo trails | `echo #x n=6 dt=60ms` | implemented | speed, rhythm |
| Slit-scan, time displacement | `slitscan #g span=` / `timedisplace #g map=` | implemented | dreamy, psychedelic |
| Motion blur | `mblur=12 shutter=180deg` | implemented | any fast move |
| Motion paths with auto-orient | `anim #x path=... orient` | planned | |

## Shapes and drawing

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Draw-on strokes | `trim=0` + `anim trim` / `draw #p` | implemented | diagrams, signatures, lines |
| Charts building | `chart ... ; draw #c` | implemented | data |
| Connectors | `link #a -> #b` | implemented | diagrams |
| Path morph | `anim #p d ...` | planned | |
| Boolean ops, repeaters | | planned | |

## Looks (direct only)

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Risograph, halftone, paper, grain | `riso=[..]` / `halftone=10` / `paper=0.6` / `grain=0.06` | implemented | editorial, print, warmth |
| Glitch, RGB split, VHS | `glitch=0.5` / `rgbsplit=1` / `vhs=0.4` | implemented | hits, tech, retro |
| Dither, ASCII | `dither=3` / `ascii=8` | implemented | retro, data |
| Color adjust, tint, levels, LUT | `saturate=0`, `tint=`, `levels=`, `lut=grade.cube` | implemented | grading, "colour comes up" |
| Mesh gradient | `fill=mesh(2x2, ...)` | implemented (static) | soft backgrounds |
| Blend modes | `blend=screen\|multiply\|...` | implemented | glows, overlays |

## Light (direct only)

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Light sweep | `sweep #x dur=1s` | implemented | logos, premium feel |
| Moving light with soft shadows | `light #sun` + `shadow=soft(#sun, height=40)` | implemented | flat design with depth |
| God rays through type | `rays=#sun` + `text ... knockout` | implemented | reveals, endings |
| Glow | `glow=24 glow-color=` | implemented | accents |

## Sound and rhythm

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Hits on the beat grid | `at beat:32` / `at bar:8` anchors | implemented | cutting to music |
| Audio-reactive channels | `scale=audio(band=1, gain=0.1)` | implemented | logos breathing with the kick |
| Pulse on beat or bar | `pulse #x on=beat amount=0.15` | implemented | drum rolls, energy |
| Music visualizer | `visualizer type=bars\|radial\|waveform` | implemented | music pieces |
| SFX on events | `sfx whoosh\|pop\|click` | implemented | UI demos |

## UI and explainer

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Screenshot with cursor, click, type | `browser\|phone shot=` + `cursor`, `click`, `type` | implemented | product demos |
| Device mock | `device type=laptop\|phone shot=` | implemented | cases |
| Cards, stats, steps, diagrams | `card`, `stat`, `steps`, `diagram` + `node`/`link` | implemented | explainers |
| Talking presenter | `presenter #p skin=flat` + `say` | implemented | explainers with a guide |
| Native video clips | `video src=clip.mp4` | planned (HTML backend only) | screen recordings |
| Lottie | `lottie src=` | planned | |
