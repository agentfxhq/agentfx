# Animation kinds menu

What to offer per beat in stage 4. Checked against `agentfx catalog --md` and `docs/SPEC.md`
(AgentFX 0.1.0). Before offering anything not listed here, run `agentfx catalog --md` and check its
`Status` column. Offer 2 to 4 per beat that fit its intent and energy; name them in plain words,
keep the syntax for yourself (it goes into `kinds[].syntax`).

Status: **implemented** (offer freely), **partial** (offer with the limit stated),
**planned** (may be shown greyed with "planned", never chosen).
"Direct only" means the native renderer path (the default); the HTML backend ignores it.

## Output runtime

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| macOS hardware encode | `agentfx render film.reel --encoder videotoolbox\|videotoolbox-hevc` | implemented | MP4 export on Apple Silicon and Intel Macs; `auto` probes it first, then falls back to x264 |
| GPU backend | `agentfx-render ... --gpu=auto` | implemented | Vulkan on Linux, Metal on macOS, DX12 or Vulkan on Windows; CPU fallback stays automatic |

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
| Tunnel / fold into a tube | `tunnel=0 tunnel-speed=1` + `anim #grid tunnel 0s:0 1.5s:1`; `tunnel-map=perspective` for a grid already drawn in perspective | implemented (direct only) | vaporwave flythroughs, a cut that is a fold |
| Kaleidoscope | `kaleido=8` + `anim #g kaleido-amount 0s:0 2s:1` / `kaleido-angle` | implemented (direct only) | psychedelic blooms of the real layer, music videos |
| Directional / radial blur | `dblur=24 dblur-angle=30deg`; `rblur=0.3 rblur-mode=zoom\|spin rblur-center=50%,50%` + `anim #t dblur 0s:200 0.5s:0` | implemented (direct only) | whip-ins, speed lines, zoom bursts, spinning wheels |
| Chromatic aberration | `chromatic=6 chromatic-center=50%,50%` + `anim #x chromatic 0s:2 0.1s:16 0.9s:2` | implemented (direct only) | lens fringe, beat hits, glitchy titles with a sharp center |
| Displacement | `displace=noise displace-amount=12 displace-scale=80 displace-speed=0.5`; `displace=#map` (child layer: red moves x, green y) | implemented (direct only) | heat haze, wobble, liquid ripples, refraction by a moving map |
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

## Fluid, particles, point clouds, generators (direct only)

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Type dissolves into ink | `fluid #ink` + `ink #title into #ink` + `stir` | implemented | soft transitions, endings |
| Ink reveal | `reveal #shot with #ink` | implemented | revealing images |
| Pour | `pour #ink at= dir=` | implemented | openings |
| Particle burst | `particles #p ... ; burst #p` | implemented | celebrations, hits |
| Point cloud formations | `cloud n=16000` + `form text\|galaxy\|sphere\|terrain\|spiral` | implemented | numbers, data, logos made of dots |
| Breathing fractal | `gen #j kind=julia palette=vapor` + `anim #j cx 0s:-0.8 6s:-0.745` + `anim #j color-offset 0s:0 6s:1` | implemented | hypnotic backgrounds, intros, music loops |
| Infinite zoom | `gen #m kind=mandelbrot re= im=` + `anim #m depth 0s:0 8s:13 ease=linear` | implemented (GPU f32 to about 2e4x, CPU f64 deeper) | dives, "going deeper" beats |
| Liquid neon, marble, oil | `gen #n kind=noise palette=neon` + `anim #n warp 0s:2 6s:3.5` | implemented | music-video backgrounds, transitions |
| Flow-field streaks | `gen #f kind=flow particles=1500 trail=0.6` (over `bg` or a dim noise) | implemented (CPU) | energy, wind, data streams |
| Truchet weave | `gen #t kind=truchet mode=1` + `anim #t phase 0s:0 4s:4 ease=linear` | implemented | beat-synced twists, retro grids |
| Voronoi cells | `gen #v kind=voronoi mode=0\|1\|2` | implemented | organic tech, crystals, cell biology |
| Growing L-systems | `gen #l kind=lsystem preset=plant\|dragon\|koch\|hilbert grow=0` + `anim #l grow 0s:0 3s:1` | implemented (CPU) | growth, "building up", reveals |
| Penrose tiling | `gen #p kind=penrose grow=0` + `anim #p grow 0s:0 3s:1` | implemented (CPU) | sacred-geometry intros, order out of chaos |
| Game of Life, rule 30 | `gen #c kind=life rule="B3/S23"` / `kind=life mode=1 rule=30` | implemented (CPU) | emergence, computation, pixel aesthetics |
| Raymarched Mandelbulb | `gen #m kind=mandelbulb palette=vapor` + `anim #m yaw 0s:20 8s:85 ease=linear` + `anim #m power 0s:6 8s:9` | implemented (GPU kernel, use `--gpu`; CPU about 0.7 s per 360p frame) | psychedelic music videos, alien worlds, "inside the machine" |
| Writhing 3D sponge (KIFS) | `gen #k kind=kifs mode=menger\|sierpinski\|octahedron palette=vapor` + `anim #k twist 0s:0 8s:9` + `anim #k yaw 0s:25 8s:85 ease=linear` | implemented (GPU kernel, use `--gpu`) | sacred geometry, "infinite complexity", techno visuals |
| Fractal flame silk | `gen #f kind=flame symmetry=1 palette=vapor seed=12` + `anim #f morph 0s:0 8s:1 ease=linear` (seamless loop) | implemented (CPU) | glowing psychedelic backgrounds, dream sequences, music videos |
| Strange attractor dust | `gen #a kind=attractor system=clifford\|dejong\|lorenz` + `anim #a a 0s:-1.4 6s:-1.7` (morph) / `anim #a turn 0s:0 8s:360` (Lorenz spin) | implemented (CPU) | abstract science, chaos, hypnotic morphs |
| Reaction-diffusion | `gen #r kind=reaction preset=coral\|zebra\|spots\|maze speed=400 palette=vapor` (grows over time, scrubbable) | implemented (CPU) | organic growth, living textures, cellular transitions |

## Time

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Rewind / time remap | `rewind #g speed=4 then=hold` / `timemap` | implemented | undo, "what if", tape stops |
| Freeze | `freeze #x for=1s` | implemented | a held moment |
| Echo trails | `echo #x n=6 dt=60ms` | implemented | speed, rhythm |
| Slit-scan, time displacement | `slitscan #g span=` / `timedisplace #g map=` | implemented | dreamy, psychedelic |
| Motion blur | `mblur=12 shutter=180deg` | implemented | any fast move |
| Motion paths with auto-orient | `move #x along="M0 0 C200 -150 400 150 600 0" orient dur=3s` / `along=#road` | implemented | vehicles, paper planes, arrows along a route, cursors on a curve |

## Shapes and drawing

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Draw-on strokes | `trim=0` + `anim trim` / `draw #p` | implemented | diagrams, signatures, lines |
| Charts building | `chart ... ; draw #c` | implemented | data |
| Connectors | `link #a -> #b` | implemented | diagrams |
| Path morph | `morph #p to=#star dur=0.8s` / `to="M..."`, chain by repeating | implemented (direct only) | logo reveals, icon to icon, shape transitions |
| Boolean ops, repeaters | | planned | |

## Looks (direct only)

| Kind | Syntax | Status | Good for |
|---|---|---|---|
| Risograph, halftone, paper, grain | `riso=[..]` / `halftone=10` / `paper=0.6` / `grain=0.06` | implemented | editorial, print, warmth |
| Glitch, RGB split, VHS | `glitch=0.5` / `rgbsplit=1` / `vhs=0.4` | implemented | hits, tech, retro |
| Dither, ASCII | `dither=3` / `ascii=8` | implemented | retro, data |
| Liquid glass | `glass=12 glass-dispersion=2 glass-rim=0.2` | implemented | translucent cards, lens moments |
| Cinematic optics | `optics streak=0.5 halation=0.3 vignette=0.4` | implemented | premium product and title finishing |
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
| Native video clips | `video src=clip.mp4 fit=cover in=1s rate=1 loop hold` | implemented | screen recordings |
| Lottie | `lottie src=` | planned | |
