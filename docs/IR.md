# reel IR (v1)

> **Changes (text v2, 2026-10-29)**: additive: optional `stroke` on `glyphs` draw items (text
> stroke from the outlines), optional `text` on `paint` content (text animators with range /
> key selectors per char, word or line; text on a path) and the track properties `text.fill`,
> `text.stroke`, `text.stroke-width`, `text.path-offset`, `animators.<i>.start|end|offset|amount`;
> see [Text v2](#text-v2). Files without them load and render unchanged.

> **Changes (world canvas X9, 2026-10-27)**: additive: optional node field `world` (static
> subtree drawn from a mip tile cache while the node's transform moves); see
> [World canvas](#world-canvas). Files without it load and render unchanged. `AGENTFX_WORLD=0`
> draws worlds as plain vectors, `AGENTFX_WORLD=tiles` composites every frame from tiles.

> **Changes (fluid X5, 2026-10-27)**: additive: new content type `fluid` (2D stable-fluids dye
> layer, simulated once and cached per frame) and the track property `fluid.time`; see
> [Fluid](#fluid). Files without it load and render unchanged.

> **Changes (time effects X3, 2026-10-27)**: additive: optional node field `time` (slit-scan,
> echo, time displacement) and the track property `time.span`; see [Time effects](#time-effects).
> Files without it load and render unchanged. `AGENTFX_TIME_FX=0` draws such nodes plainly.

> **Changes (point clouds, 2026-10-27)** — additive: new content type `cloud` (instanced point
> cloud, one paint unit for any number of points; see [Point cloud](#point-cloud)). v1 files
> without it load and render unchanged.

> **Changes (M4b, 2026-10-26)** — no format change; pixels differ in one case: a node whose
> opacity track is below 1 at the frame *and* whose own transform changed since the previous
> frame is drawn from a raster of its inner content taken at its rest transform (the
> transform at the time the opacity track is at its maximum, nearest to the frame), with
> grayscale text, resampled bilinearly at the frame's transform, as Chrome composites a
> fading layer from its cached raster. Nodes at rest, nodes that only
> move or only fade, and nodes with effects, motion blur or mattes are unchanged.
> `AGENTFX_FADE_RASTER=0` disables it.

> **Changes (M4, 2026-10-26)** — no format change; pixels differ in `text_aa: lcd` scenes
> only: a grayscale glyph run (inside an isolation layer) whose device matrix scales by
> anything other than 1 and whose device text size is 16.5–162 px is drawn as Skia
> distance-field text (bucket raster 32/72/162 px, signed distance field, bilinear
> sampling, ±0.65 px coverage ramp, fractional glyph positions), as Chrome does
> (`compiler/src/sdft.rs`). `AGENTFX_SDFT=0` restores M3's coverage masks.

> **Changes (M3, 2026-10-26)** — no format change; v1 files load unchanged. Pixels and
> output differ:
> - **Output colour: BT.709 limited range** (was BT.601), tagged `bt709`/`tv` in the MP4,
>   for x264, VAAPI and the `--rgba-pipe` path alike.
> - **Encoders:** `agentfx render|run --encoder auto|x264|vaapi|qsv` (default `auto`: Intel
>   Quick Sync via VAAPI when `/dev/dri/renderD128` works, directly or through
>   `sg render`; else x264 with a one-line reason). `--qp N` (default 21, CQP),
>   `--rc cqp|icq|qvbr`, `--low-power 0|1`, `--compression-level N`. QSV is implemented but
>   unavailable without a oneVPL GPU runtime. Render stats report `encoder` and `hw`.
> - **Gradients are dithered** (Skia's 8x8 ordered dither, +-0.49 LSB, anchored to screen
>   pixels; linear, concentric radial and sweep brushes in sRGB; others are drawn undithered).
>   `AGENTFX_DITHER=0` turns it off (also off with `AGENTFX_CACHE`).
> - **Text:** Skia's mask gamma/contrast (`SK_GAMMA_CONTRAST` 0.2, exponent 1.2) on glyph
>   coverage, for LCD and grayscale runs (`AGENTFX_TEXT_CONTRAST`, `AGENTFX_TEXT_GAMMA`,
>   `AGENTFX_A8_GAMMA=0`). Axis-aligned solid runs go through a cached coverage mask.
> - An effective opacity >= 1 - 0.5/255 is drawn as opaque (no isolation layer, so text in
>   it keeps LCD antialiasing).

> **Changes (M2, 2026-10-26)** — non-breaking: v1 files load unchanged.
> - New optional scene field `text_aa`: `"gray"` (default, omitted) or `"lcd"`: glyph runs
>   with a solid brush drawn directly onto opaque content get LCD subpixel antialiasing
>   (FreeType default filter, like Chrome on Linux); inside isolated groups (opacity,
>   blend, filters, offscreen effects) text stays grayscale, as in Chrome.
> - Renderer only (no format change): push groups without alpha/blend/filter are drawn
>   as clip paths (no isolation); large box shadows (sigma > 2 px) are rasterized once
>   per geometry and blitted; display-list items and groups are culled against the damaged
>   rect; the static-content raster cache is opt-in (`AGENTFX_CACHE=1`).
> - Producers: the HTML frontend now emits `switch` contents for all repaint states of
>   recorded scenes and node chains translate → rotate → scale → transform per element.

The renderer's only input. Crate: `crates/ir/` (`agentfx-ir`): types in `crates/ir/src/lib.rs`,
evaluation in `ir/src/eval.rs` + `ir/src/timing.rs`, checks in `ir/src/validate.rs`.
v0 files (M0) are a subset and still load (`version: 0` or `1`).

Frontends compile to it:

- **HTML mode**: `agentfx html <scene_dir> --out-dir out/x` writes `out/x/scene.ir.json`
  (display lists per paint unit, `fonts/`, `images/`). `agentfx run <scene_dir> --out x.mp4`
  does the same in-process and renders. Input contract: `INPUT.md`.
- **Direct mode**: hand-written or generated JSON (Reel Script, a Rust API, …).
  Examples: `examples/direct/lower-third.ir.json` (v0 subset),
  `examples/direct/showcase-16x9.ir.json` / `-9x16` (all M1 features, generated by
  `examples/direct/showcase.py`).

`agentfx render scene.ir.json --out x.mp4 [--mode vfr|cfr] [--segments N] [--threads N]
[--preset veryfast] [--lossless x.mkv] [--backend vello|tiny-skia]` renders either;
`agentfx-render stats scene.ir.json` prints the damage schedule.

## Model

A **scene** is a tree of **nodes** in paint order, **tracks** that animate node
properties over time, **assets** (PNG rasters) and **fonts**. `f(frame)` is a pure
function, so the core plans every frame (states, damage, repeats, cut points) before
drawing.

Coordinates: one scene space (px, y down, origin top-left). A node's transform maps its
subtree into its parent's space, applied about `origin` (scene coordinates):
`M = T(origin) · transform · T(-origin)`. Content geometry and clip rects are scene
coordinates.

```jsonc
{
  "version": 1, "width": 1920, "height": 1080, "fps": 30, "frames": 240,
  "fonts": [ { "path": "../../scenes/_shared/Inter.ttf" } ],
  "assets": [ { "path": "img/photo.png", "scale": 1 } ],
  "text_aa": "lcd",                            // optional: gray (default) | lcd
  "nodes": [                                   // nodes[0] = root
    { "name": "root", "children": [1, 2] },
    { "name": "title", "origin": [960, 300],
      "content": { "type": "text", "text": "reel", "font": 0, "size": 230, "weight": 820,
                   "pos": [0, 200], "max_width": 1920, "align": "center",
                   "brush": { "solid": [1, 1, 1, 1] } },
      "effects": [ { "type": "glow", "radius": 12, "color": [0.3, 1, 0.7, 1], "strength": 0.8 } ] },
    { "name": "line", "content": { "type": "path", "d": "M100 500 L1800 500", "trim": [0, 0],
      "stroke": { "width": 12, "cap": "round", "brush": { "solid": [0.2, 0.9, 0.6, 1] } } } }
  ],
  "tracks": [
    { "node": 1, "property": "glyph.transform", "delay": 0.5, "duration": 1, "fill": "both",
      "easing": { "spring": { "mass": 1, "stiffness": 140, "damping": 13, "velocity": 0 } },
      "stagger": { "each": 0.09 },
      "keyframes": [ { "offset": 0, "value": { "transform": [ { "translate": [0, 120] }, { "scale": [0.4, 0.4] } ] } },
                     { "offset": 1, "value": { "transform": [ { "translate": [0, 0] }, { "scale": [1, 1] } ] } } ] },
    { "node": 2, "property": "trim-end", "delay": 1, "duration": 1.4, "fill": "both", "easing": "linear",
      "keyframes": [ { "offset": 0, "value": { "number": 0 } }, { "offset": 1, "value": { "number": 1 } } ] }
  ]
}
```

### Node

| field | meaning |
|---|---|
| `children` | paint order, back to front |
| `content` | drawn below the children (see Content) |
| `origin` | transform origin (scene coords) |
| `transform` | base transform (function list) when no track has an effect |
| `opacity` | base opacity |
| `clips` | rounded rects in the *parent's* space; clip the node and its subtree |
| `blend` | `normal`, `multiply`, `screen`, `overlay`, `darken`, `lighten`, `color-dodge`, `color-burn`, `hard-light`, `soft-light`, `difference`, `exclusion`, `hue`, `saturation`, `color`, `luminosity` (W3C compositing, source-over), `plus` (additive, unchanged), `src-atop` (Porter-Duff source-atop: shows only where the backdrop inside the enclosing group has alpha; meant for display-list groups, e.g. an image fill clipped to glyphs drawn before it) |
| `isolate` | group compositing (CSS group opacity), default true; `false` distributes opacity over children |
| `effects` | list, applied in order to the node's composited subtree (see Effects) |
| `motion_blur` | `{ "samples": 2..64, "shutter": 0..4 frames }`: subtree averaged over sub-frame samples centered on the frame time |
| `hidden` | evaluated but not drawn in the normal pass (matte sources) |

### Content

| type | fields | notes |
|---|---|---|
| `rect` | `rect: {rect:[x,y,w,h], radius:[tl,tr,br,bl]}`, `color` (straight RGBA 0..1) | |
| `path` | `d` (SVG path data), `fill?: Brush`, `fill_transform?: [a,b,c,d,e,f]` (brush space -> scene, places an `image` brush), `stroke?: {width, cap, join, miter_limit, dash, dash_offset, brush}`, `trim: [start, end]` | draw-on: the stroke shows only `[start, end]` of the total arc length; tracks `trim-start` / `trim-end`. The fill is drawn only when untrimmed. |
| `text` | `text`, `font` (index into `fonts`), `size`, `weight` (wght), `letter_spacing` px, `line_height` (× size), `pos` (top-left of the layout box), `max_width?`, `align` left/center/right (center/right need `max_width`), `brush`, `unit` glyph/word/line | shaped by reel at load (Parley/HarfRust); `opsz` follows `size`; one font per node (no fallback). Glyphs are drawn at their exact fractional position. `\n` breaks lines. |
| `paint` | `rect` (ink bounds), `items: [Draw]`, `elements?`, `text?` | resolved display list (what the HTML frontend emits per paint unit); `elements` = identity of the painted DOM elements (document-order indices); `text` = text animators / text on a path over its glyph runs ([Text v2](#text-v2)) |
| `switch` | `rect`, `frames: [[first_frame, state]]`, `states: [paint]` | content that changes (repaint states, typed text in HTML mode); a switch is damage |
| `image` | `asset`, `rect`, `variants?` | M0 raster content (`asset.scale` = device px per scene px) |
| `sequence` | `rect`, `frames: [[first_frame, asset]]` | M0 raster sequence |

| `cloud` | `count`, `seed`, `radius`, `radius_jitter`, `palette`, `weights`, `forms`, `projection` | instanced point cloud, see [Point cloud](#point-cloud) |

**Draw** (display-list items, the anyrender command set; all geometry scene space,
optional `transform` `[a,b,c,d,e,f]`):

| op | fields |
|---|---|
| `fill` | `shape`, `brush`, `transform?`, `brush_transform?`, `even_odd?` |
| `stroke` | `shape`, `brush`, `style {width, join, miter_limit, cap, dash, dash_offset}`, `transform?`, `brush_transform?` |
| `glyphs` | `font`, `size`, `coords?` (normalized variation coords, i16), `brush`, `transform?`, `glyph_transform?`, `glyphs: [[id, x, y]]`, `hint?` (recorded, reel renders unhinted like Chrome), `stroke?: {width, brush, join (round), over}` (text stroke: the outlines stroked, below the fill unless `over`) |
| `box-shadow` | `rect [x0,y0,x1,y1]`, `radius`, `std_dev`, `color`, `transform?` |
| `push` | `clip?: shape`, `transform?`, `alpha`, `blend`, `filter?` (`{type: blur, std_dev}` or `{type: drop-shadow, dx, dy, std_dev, color}`), `even_odd?` |
| `pop` | closes the last `push` |

`shape`: `{"rect": [x0,y0,x1,y1]}`, `{"rounded-rect": {"rect": [...], "radii": [tl,tr,br,bl]}}`,
`{"path": "M…"}`.
`brush`: `{"solid": [r,g,b,a]}`, `{"linear": {start, end, stops: [{offset, color}], extend}}`,
`{"radial": {start_center, start_radius, end_center, end_radius, stops, extend}}`,
`{"sweep": {center, start_angle, end_angle, stops, extend}}`,
`{"image": {asset, quality 0|1|2, extend: [x, y], alpha}}` (asset at natural size,
placed by `brush_transform`). Colors are straight-alpha sRGB 0..1, gradients interpolate
in sRGB.

### Text v2

**Text stroke.** `glyphs.stroke = {width, brush, join, over}`: the glyph outlines are stroked
with `width` (run units, centered on the outline; vello_cpu `stroke_glyphs`, unhinted, round
caps). `over: false` (default) draws the stroke before the fill, so only the outer half shows
(outlined captions); `over: true` draws it after (CSS `-webkit-text-stroke`). Ink grows by
`width / 2` (x4 for miter joins). Runs with a stroke still take the LCD / mask fast path for the
fill.

**Text animation** (`paint.text`):

```jsonc
"text": {
  "animators": [
    { "by": "char", "selector": { "type": "range", "start": 0, "end": 0.25, "offset": -0.25,
        "shape": "ramp-up", "amount": 1, "order": "forward", "seed": 0 },
      "opacity": 0, "translate": [0, 60], "blur": 10 },
    { "by": "word", "selector": { "type": "keys", "units": [[[1.0, 1], [1.3, 0]], [[1.4, 1], [1.7, 0]]],
        "easing": { "cubic-bezier": [0.33, 1, 0.68, 1] } },
      "scale": [0.5, 0.5], "rotate": -12, "color": [1, 0.3, 0.3, 1] }
  ],
  "path": { "d": "M240 540 C480 380 720 380 960 540", "offset": 0 }
}
```

- **Units** come from the list's glyph runs in paint order, in scene space: `char` = every
  glyph that is not a space (font cmap of U+0020, U+00A0, U+2009, U+3000, tab; a space joins the
  unit before it), `word` = starts at the first non-space after a space or a line break, `line`
  = a new baseline (|dy| > 0.3 em). Unit boxes = union of their glyph ink centers; scale and
  rotate act about the unit's box center (on a path: the mean of its placed glyph centers).
- **Range selector** (After Effects): unit i of n covers `[i/n, (i+1)/n]` after `order`
  (`reverse`, or a SplitMix64-seeded permutation for `random`); the range is
  `[min(start, end) + offset, max(start, end) + offset]`. `square` = covered fraction of the
  unit; the others at the unit center c with `u = (c - s) / (e - s)`: `ramp-up` = clamp(u, 0, 1)
  (0 before, 1 after the range), `ramp-down` = 1 - that, `triangle` = 1 - |2u - 1|, `round` =
  sqrt(1 - (2u - 1)^2), `smooth` = (1 - cos 2 pi u) / 2, these three 0 outside. Times `amount`.
- **Key selector**: unit i's amount follows `units[min(i, len - 1)]` (`[t, amount]`, absolute
  seconds, held outside, `easing` per segment), times `amount`.
- **Properties** at amount a, composed over the animators in order: opacity `x lerp(1, o, a)`,
  translate `a * [dx, dy]`, scale `lerp(1, s, a)` and rotate `a * deg` about the unit center,
  blur `+ a * sigma` (scene px, a filtered group per run of equal state), fill color
  `lerp(fill, color, a)` (solid fills; a gradient fill gets the color drawn over it at alpha
  a). Amounts are quantized to 1/4096, opacity to 1/255.
- **Path**: glyph origin (gx, gy) goes to arc length `s = gx - x0 + offset + h` (x0 = leftmost
  origin, h = half the advance to the next glyph on its line), minus `h` along the tangent
  there, plus `gy - baseline0` along the normal (y down); rotated to the tangent angle (averaged
  over +-1.5 px of a 0.05 px flattening). Glyphs whose center falls outside `[0, length]` are
  not drawn.
- **Evaluation**: per frame the renderer computes the state (per-unit amounts, `text.*` colors
  and width, path offset); at rest (all amounts 0, no path, no overrides) it draws the list as
  it is, otherwise it rewrites the list (runs split where the per-glyph state changes, one item
  per glyph on a path, groups for opacity with non-solid brushes or strokes and for blur) and
  draws that like any `paint`. **Planner**: signature = hash of the state; ink = the rewritten
  list's bounds; a key selector makes the content time-dependent (evaluated every frame).
  Key times are absolute: producers cutting a frame range shift them like track delays.

### Point cloud

`{"type": "cloud", ...}`: `count` points (up to 4M) whose positions, radii, colors and
opacities are evaluated from a compact description at every frame and motion-blur sample,
in Rust. No per-point tracks: the IR stays a few KB for any count, and the planner sees one
content node.

```jsonc
{ "type": "cloud", "count": 20000, "seed": 7, "radius": 2.4, "radius_jitter": 0.25,
  "palette": [[0.95, 0.93, 0.9, 1], [0.91, 0.27, 0.18, 1]], "weights": [0.64, 0.36],
  "forms": [
    { "shape": { "type": "field", "rect": [-80, -60, 2080, 1200], "flow": [60, -10] },
      "drift": { "amp": 40, "scale": 300, "speed": 0.2 } },
    { "at": 1.0, "dur": 1.0, "stagger": 0.6, "stagger_by": "x",
      "easing": { "spring": { "mass": 1, "stiffness": 150, "damping": 15, "velocity": 0 } },
      "shape": { "type": "text", "text": "3000", "font": 0, "size": 600, "weight": 800,
                 "pos": [0, 160], "max_width": 1920, "align": "center" },
      "color": { "by": "dir", "angle": 30, "split": [0.64, 0.36] } },
    { "at": 3.4, "dur": 1.1, "stagger": 0.4, "stagger_by": "distance",
      "shape": { "type": "spiral", "center": [960, 540], "radius": 700, "squash": 0.62, "spin": 25 } }
  ] }
```

| field | meaning |
|---|---|
| `count`, `seed` | number of points; every random choice derives from `seed` (SplitMix64) |
| `radius`, `radius_jitter` | point radius (scene px); per-point factor uniform in `[1-j, 1+j]` |
| `palette`, `weights` | colors (straight sRGB RGBA); initial random split when `forms[0]` has no `color` |
| `forms` | formations in time order; `forms[0]` is the start (its timing is ignored) |
| `time_offset` | seconds added to the scene time before evaluating (a frame range cut out of a longer scene keeps the cloud's motion) |
| `projection` | optional `[{t, m: [12]}]`: row-major 3x4 matrices (rows x, y, w; screen = x/w, y/w) mapping `(x, y, z, 1)` into the node's space, piecewise linear in t. Empty: 2D, z ignored. With a projection points are drawn far to near and scaled by the projection (sphere size). Reel Script bakes the scene's 2.5D camera into it. |

**Form**: `at` (s, morph start), `dur` (s per point), `stagger` (s from the first to the last
point; negative = reversed) ordered by `stagger_by` (`index` slot order, `random`, `distance`
from the form's center, `x`, `y`, `angle`), `easing` (any track easing; springs overshoot),
`assign` (`curve` default, `angle`, `index`, `random`), `shape`, `drift`
(`{amp, scale, speed}`: seeded curl-noise displacement while in this form), `size` (radius
factor), `opacity`, `color` (`{by, split, angle, center, wave}`: slots sorted by
`random|index|x|y|z|distance|angle|dir` and split into palette groups by the `split`
fractions; `dir` sorts along `angle` (deg), `wave: [amp, period]` bends that boundary).

**Shapes** (`shape.type`): `field {rect, depth, flow: [vx, vy]}` (uniform random, streaming
with wrap-around), `grid {rect, cols, jitter}`, `spiral {center, radius, arms, twist, spread,
squash, spin}` (log-spiral galaxy, differential rotation in deg/s), `gaussian {center: [x, y, z],
sigma: [sx, sy, sz]}`, `sphere {center: [x, y, z], radius, spin}` (Fibonacci lattice),
`terrain {rect, deep, rows, amp, scale, speed}` (height field of travelling waves; `deep`: rows
run into depth, z from 0 to -h, for a projection), `text {text, font, size, weight,
letter_spacing, line_height, pos, max_width, align, outline}` (glyphs shaped like `text`
content, rasterized to a coverage mask and sampled on a hex grid, or along the outline),
`points {points: [[x, y(, z)], ...]}` (cycled up to `count`).

**Evaluation at t.** Point i starts at its last form it has fully reached and blends towards
every later form it has started: `p = lerp(p, slot_k(t), ease_k((t - start_k,i) / dur_k))`,
likewise size, opacity and color; slots are evaluated at t (spin, flow, waves, drift keep
moving during a morph), so overlapping morphs never jump. A point's start of form k is
`at + stagger * key` and never earlier than its start of form k-1.

**Matching** (load time): points go to the slots of the next form in Hilbert-curve order of
both sets (each normalized to its own box: a moved or rescaled copy maps onto itself, left goes
to left), then nearby pairs are swapped where that lowers the summed squared travel (removes
most crossings). With a projection, positions are compared on screen. When the form has a
`color` rule whose group sizes equal the current ones, matching runs per color (red points
stay red and go to the red region); otherwise points blend to their slot's color.

**Planner.** Ink = a conservative box from per-form boxes (spring overshoot, drift and radius
included, projected corners with a projection); signature = constant while every point rests
in a form without time dependence (no drift/flow/spin/waves) and the projection is still, else
the time. So resting clouds cost nothing; moving ones damage their box. Motion blur on the
cloud node (or any ancestor) evaluates the cloud at each sample time.

**Raster.** Points are evaluated in parallel (order kept) and splatted into one
premultiplied buffer that is composited 1:1: discs with an analytic coverage ramp (no square
root in the interior), sub-pixel points as area-preserving bilinear dots, integer "over" in
point order. Points are binned into 32-row bands in order and the bands drawn in parallel (each
pixel sees the same sequence as a plain loop: identical result, cache-local). A motion-blurred
cloud node splats its samples in parallel into pooled buffers and averages them in one pass
(no vector pass). i5-9500T (6 cores), 1080p30, 15 s, `motion_blur` 8 samples, one segment: 3k points 12 s,
20k 21 s, 100k 64 s (`agentfx render` total incl. encode, machine shared).

### Fluid

`{"type": "fluid", "rect": [x, y, w, h], ...}`: a dye density field simulated with stable fluids
(Jos Stam) on a grid of `round(w*res) x round(h*res)` cells, drawn as `color` with coverage
`1 - exp(-density * dye)`, upsampled with a Mitchell-Netravali bicubic (B = C = 1/3: slight blur).
Per step (one per frame, step k at simulation time k/fps): forces (emitters active at that time,
`turbulence`, `buoyancy`), vorticity confinement, implicit viscosity (Jacobi), pressure projection
(`iterations` multigrid V-cycles: weighted-Jacobi smoothing, warm-started), semi-Lagrangian velocity advection, MacCormack dye
advection clamped to the source neighbourhood, dissipation. Closed walls.

| field | default | meaning |
|---|---|---|
| `rect` | | scene coordinates of the box (content coordinates of the node) |
| `res` | 0.25 | grid cells per px, 0.05..1 |
| `viscosity` | 0 | kinematic viscosity, rect-widths^2 / s |
| `dissipation` / `drag` | 0.99 / 0.995 | dye / velocity kept per 1/30 s |
| `vorticity` | 0 | confinement strength (grid units; 10..30 curls) |
| `buoyancy` / `turbulence` | 0 | px/s^2 up per unit dye; seeded curl-noise px/s^2 (feature 220 px) |
| `seed`, `color`, `density` (2.5), `shade` (0) | | noise seed; straight RGBA; display gain; dense dye darkened by up to `shade` |
| `edge` | 0 | (0..1) drawn only as a rim: opaque for coverage 0.5·edge..edge, fading out by edge+0.35; a matte effect sourcing the fluid sees `smoothstep(0.5·edge, edge+0.15, coverage)` (ink reveal: dark rim, layer opaque inside) |
| `iterations` | 2 | pressure multigrid V-cycles, 1..20 |
| `emitters` | [] | below; `start <= t < end`, seconds of simulation time |
| `time_offset` | 0 | simulation time = scene time + offset (cut-out frame ranges) |

Emitters (`type`): `pour` `{at, dir (deg, 0 = +x, 90 = down), speed (px/s, 400), radius (px), rate
(dye/s at the center, 1), wobble (deg, seeded swing)}`: Gaussian splat of dye, velocity pulled to
the jet. `stir` `{at, radius, strength (rad/s)}`: velocity pulled towards a rigid rotation. `source`
`{node, amount (1), speed, dir, noise (px/s)}`: node's subtree coverage (evaluated from the tracks at
that time, own and ancestors' opacity ignored, hidden descendants skipped) rasterized at grid
resolution and added as dye, `amount` spread over the window's steps (one step when shorter), with
velocity set to `speed` along `dir` plus seeded curl noise (feature 70 px) inside the coverage.
The source must not contain a fluid.

Track `fluid.time` (number, seconds): the simulation time shown (rewinds, time maps); without it
`t + time_offset`. Fractional frame positions lerp the two cached frames.

**Determinism and cache.** The simulation runs once per process, sequentially, under a lock shared
by all painters (parallel segments); each frame's coverage is stored as u8 per cell
(`w*res * h*res * frames` bytes: 480x270 x 300 frames = 39 MB). Validation warns above 512 MB.
Smoother/advection rows run in parallel (no reductions) and an AVX2 copy of the smoother kernel
is chosen at run time (no FMA contraction), so results are identical on every path; a cut-out
range (`time_offset`) re-simulates from 0. **Planner.** Dye only enters through `pour`/`source`,
advection never raises the maximum and dissipation shrinks it by `q = dissipation^(30/fps)` per
frame, so the last visible frame is `last injection + ceil(ln(eps/dmax)/ln q)` (eps = half a u8
step of coverage). Bounds = `rect` from the first dye frame to that frame (`dissipation = 1`: to
the end), else none; signature = shown frame position. Not with the tiny-skia backend.
`fluid` as a matte source (`reveal`) needs no `hidden` (no warning).

### Time effects

`"time": {...}` on a node: the node's subtree (content + children, like `motion_blur`) is
evaluated at other times than the frame's and the renders are combined; then the node's
effects, clips, opacity and blend apply as usual. Frame n is a pure function of n, so this is
exact for everything in the subtree (springs, glyph tracks, trims, clouds, switches at the
sample's frame). `span` (seconds, may be negative = the future) is animatable with the number
track `time.span`; `|span| < 1e-6` draws the plain subtree. Not on the same node as `motion_blur`.

| type | fields | pixel p shows the subtree at |
|---|---|---|
| `slitscan` | `axis` x/y, `span`, `range?: [a, b]` (screen px along the axis, default the frame; b < a reverses), `band` px (1..64, default 4) | `t - span*u`, u = (p - a)/(b - a) clamped; time sampled every `band` px, linear cross-fade between the two nearest samples |
| `echo` | `count` 1..32, `span` (dt), `decay` (0..1], `blend` normal/plus/multiply/screen, `tint?` 0..2 colours | the subtree at t over copies k = count..1 at `t - k*span`, copy k scaled by `decay^k`, recoloured (rgb = tint*alpha, tint lerped newest→oldest), composited oldest first with `blend` |
| `displace` | `map`, `span`, `levels` 2..64 (default 24) | `t - span*map(p)`, map quantized to `levels` samples and cross-faded |

`map`: `{"type":"radial","center":[x,y],"radius":r}` (distance/radius), `{"type":"linear","from":[x,y],"to":[x,y]}`
(projection), `{"type":"noise","scale":px,"seed":n,"octaves":1..8}` (seeded value-noise fbm),
`{"type":"layer","node":i}` (luma of node i's subtree, usually `hidden`, at the frame, composited over black).
All map coordinates are screen pixels (centres).

**Raster.** Levels whose subtree state (absolute chains, opacities, content states, signatures)
is identical form one run and share one render, so rests cost one render. Each render covers only
the pixels that need it: slit-scan strips (about 2x the band area in total), displace per 256 px
tile, echo copies their own ink. Sample rasters are cached by (state, rect) per painter (192 MB),
so an echo copy is drawn once and reused while it trails. **Planner.** The subtree's signature
and ink include the states at the sample times: every echo copy and displace level; slit-scans
at most two samples per frame interval of the window (motions shorter than half a frame inside
the window can be missed by damage). Ink = union over those samples. Render stats count
`time_renders` / `time_px` / `time_hits` (`AGENTFX_DEBUG=1`).

### World canvas

`"world": {"tile": 256, "cache_mb": 512}` on a node (usually the child of a camera group whose
transform track zooms by 1e4 or more): while the node's chain moves (frame f differs from f+1),
its **static** descendants are composited from a tiled mip pyramid; frames where it rests draw
the subtree as exact vectors (so the settle frame is crisp, and rest frames are repeats anyway).

| field | meaning |
|---|---|
| `tile` | tile edge in device px, power of two 64..1024 |
| `cache_mb` | tile cache budget per renderer segment (LRU; the current frame's tiles are never evicted) |

- **World space** = the node's content space. A descendant is static when its local transform,
  opacity, content state and signature are the same in every frame and it has no effects, motion
  blur, time effect, cloud, blend mode or hidden flag (and its parent is static). Animated
  subtrees are drawn live, above the static world (their static ancestors' clips/opacity are not
  applied to them).
- **Levels**: level L holds the static world at 2^L device px per world px. A frame with camera
  scale s (sqrt |det| of the chain) uses L = ceil(log2 s): 1..2 texels per screen px.
- **Tiles** are rastered with the normal vector painter (culled by subtree bounds; only where
  static content is; empty tiles are remembered), on demand, and **prefetched**: at every tile
  frame the missing tiles of the next 12 frames (from the schedule's chains, i.e. the planned
  camera path) are rastered in parallel, one painter per worker. Pixels never depend on order,
  cache size or prefetch (tile = pure function of level, index and content).
- **Resampling**: per screen pixel a 2x2 box of bilinear taps at +-1/4 px (a supersampled
  downscale of the finer level; no blur from a coarser level, no aliasing at level switches).
  Axis-aligned cameras use a separable two-pass version, rotated ones the general per-pixel path.
- Planner: unchanged (camera moves damage the whole frame through the subtree's ink).
- Render stats: `world: {tile_frames, tiles_rastered, tiles_prefetched, tiles_empty, tile_hits,
  evicted, peak_mb_per_segment, tile_raster_s_sum, sample_s_sum, levels, scale_range}`.
- Validation: tile/cache ranges; no nested worlds; not on a node with `time`/`motion_blur`.

### Effects ("raw Rust": not bound to CSS)

| type | fields | animatable (`effects.<i>.<field>`) |
|---|---|---|
| `blur` | `radius` (gaussian σ, scene px) | radius |
| `drop-shadow` | `dx`, `dy`, `radius`, `color` | radius, dx, dy |
| `glow` | `radius`, `color`, `strength` (tinted blurred alpha behind the subtree) | radius, strength |
| `matte` | `source` (node, usually `hidden`), `mode` alpha / alpha-inverted / luma / luma-inverted | – |

| `riso` | `inks` (1..4 RGBA), `seed`, `misreg` px, `drift` px, `drift_speed` Hz, `grain` 0..1, `density` 0..1, `paper?` | misreg |
| `halftone` | `cell` px, `angle` deg, `mode` mono / cmyk, `ink`, `paper?` | cell, angle |
| `grain` | `amount`, `seed`, `live`, `size` px | amount |
| `paper` | `texture`, `bleed` px, `rough`, `seed`, `color` | texture, bleed, rough |
| `shadow` | `light` [x, y, z], `light_radius`, `height`, `color`, `strength` | lx, ly, lz, radius, height, strength |
| `sweep` | `pos` 0..1, `angle` deg, `width` px, `color`, `strength` | pos, angle, width, strength |
| `rays` | `light` [x, y], `length` 0..0.95, `color`, `strength` | lx, ly, length, strength |
| `color` | `brightness` (x, default 1), `saturate` (default 1), `contrast` (default 1), `hue` (deg), `tint?` RGBA + `tint_amount` (default 1), `levels?` [in_black, in_white, gamma, out_black, out_white] 0..1, `lut?` (`.cube` 3D LUT path relative to the IR file) | brightness, saturate, contrast, hue, tint (= tint_amount) |

`color` works in straight sRGB on the unpremultiplied pixel, in this order, each step
clamped to 0..1 like a CSS filter chain: brightness, saturate, contrast, hue-rotate (the
Filter Effects matrices, so `brightness=1.2 saturate=0.5` matches CSS `filter: brightness(1.2)
saturate(0.5)`), then tint (`mix(c, luma(c) * tint, tint_amount * tint.a)`, luma Rec. 709),
levels (per channel `out_black + ((c - in_black) / (in_white - in_black))^(1/gamma) *
(out_white - out_black)`), LUT (trilinear, input mapped from DOMAIN_MIN..MAX). Alpha is kept;
an all-identity effect leaves the pixels bit-exact.

Effects render the node's subtree offscreen, apply the list in order and composite
the result with the node's clips/opacity/blend. A subtree that is static inside and
only moves rigidly reuses its filtered raster (placed bilinearly at fractional offsets).

### Looks and light (X8, X6)

Print looks (`compiler/src/look.rs`) and light on flat design (`compiler/src/light.rs`) are
node effects like blur: they run on the node's composited subtree (premultiplied RGBA8) in
list order. Every random choice is an integer hash of (layer-local pixel, `seed`[, frame]):
no RNG state, identical in any frame order, segment or thread. Noise is anchored to the
node's rounded translation, so it moves with the layer and the translation-invariant effect
cache stays exact.

- **riso**: per pixel the unpremultiplied color (relative to `paper` if given) is separated
  into ink densities by non-negative least squares on optical density (Beer-Lambert,
  projected Gauss-Seidel), coverage folded in. Each plate is sampled at its own offset:
  a seeded vector of length 0.4..1 x `misreg` plus, with `drift`, a seeded two-sine wander
  of amplitude `drift` at `drift_speed` Hz. Plates get fine ink speckle (`grain`) and
  low-frequency density variation (`density`), then multiply: `c = exp(-sum d_k D_k)`.
  Output: with `paper` the covered area is `paper x c` at the subtree's alpha; without,
  transparent ink (alpha = 1 - min(c), color = c - min(c)), exact over white.
- **halftone**: the subtree blurred over about a third of a cell gives the tone at each dot
  center; dots of area `cell^2 x darkness` (mono: 1 - luma; cmyk: full-GCR C, M, Y, K at
  15/75/0/45 deg + `angle` - 45), antialiased, 3x3 neighbouring cells (dark tones merge).
  Same output rule as riso (`paper` = opaque paper inside the subtree).
- **grain**: triangular noise +-`amount` added to the color inside the alpha, `size` px
  grains (value noise above 1 px; per-pixel hashes with an AVX2 build of the row below).
  `live`: the frame number joins the seed.
- **paper**: `bleed` blurs the subtree, the alpha is re-thresholded at 0.5 + `rough` x
  fractal noise (rough, swollen ink edges); `texture` darkens along paper fibers and lets
  `color` through in the paper tooth.
- **shadow**: the subtree is a sheet at `height` above the page; light sphere at `light`
  (scene px, z above the page) of radius `light_radius`. A page point p sees the light
  through the sheet plane at `q = L + (p - L)(z - h)/z`, where the light's disc covers a disc
  of radius `r = R h / z`; the occluded fraction is a smoothstep of the sheet's signed
  distance at q over [-r, r] (exact Euclidean distance transform, Felzenszwalb-Huttenlocher,
  coarser for wide penumbras). The subtree is drawn over its shadow.
- **sweep**: a gaussian band (`width`) along `angle`, crossing the subtree's alpha box as
  `pos` goes 0 -> 1, screen-blended in `color` x `strength` inside the alpha.
- **rays**: holes = transparent pixels inside the subtree's opaque box; at half resolution
  every pixel integrates 64 samples of that mask toward the light over `length` x the
  distance (decaying), and the result is added as light (`color` x `strength`).

**Planner.** Params are evaluated per frame (`effects.<i>.<field>` tracks); `riso` with
`drift` and `live` grain put the time into their parameters, so the node changes on every
frame (that defeats frame skipping for it). Ink margins: riso `misreg + drift`, halftone a
cell, paper `3 x bleed`, shadow `h/(z-h)` x the largest distance from the light to a frame
corner + penumbra, rays `length/(1-length)` x that distance. A moving light changes the
parameters of every effect that uses it, so the damage is the union of those receivers'
inks. `shadow` and `rays` depend on the node's absolute position: their effect-cache key
includes it. `sweep` and `rays` always filter the whole subtree (they measure its box).

### Track

Same timing model as a CSS animation / Web Animation with iteration count 1: active in
`[delay, delay + duration)`, `fill` (`none|forwards|backwards|both`) decides the value
outside it, and when several tracks of a node set the same property the highest
`order` with an effect wins (else the base value). Keyframe offsets 0..1; a keyframe's
`easing` applies to the segment starting there (CSS semantics); the track `easing` is
the default per segment.

| property | value | applies to | stage (what a change costs / where damage comes from) |
|---|---|---|---|
| `transform` | `{"transform": [{"translate":[x,y]}, {"scale":[sx,sy]}, {"rotate":deg}]}` | any node | composite: subtree moves; damage = ink before ∪ after, quantized to 1/16 px |
| `opacity` | `{"number": v}` | any node | composite; quantized to 1/255 |
| `glyph.transform` / `glyph.opacity` | transform / number, per unit (glyph/word/line) about the unit's center; with `stagger: {each, from: start|end|center}` unit i runs `i·each` later | text | per-frame vector redraw of that text node; damage = union of the evaluated glyph boxes |
| `trim-start` / `trim-end` | number 0..1 | path | per-frame redraw of the path; damage = path ink |
| `fill.color` / `stroke.color` | `{"color": [r,g,b,a]}` straight sRGB 0..1, interpolated per channel | path with a fill / stroke | the brush is replaced by that solid color; per-frame redraw of the path; damage = path ink |
| `fluid.time` | number (s) | fluid node | simulation time shown; damage = rect while the shown frame changes |
| `text.fill` / `text.stroke` | color | `paint` content | every glyph run's fill / stroke brush replaced by that solid color; per-frame redraw of the list; damage = list ink |
| `text.stroke-width` | number (run units) | `paint` content | stroke width of every glyph run (adds a black stroke below the fill where a run has none) |
| `text.path-offset` | number (px) | `paint` with `text.path` | arc length where the text starts; damage = evaluated glyph ink |
| `animators.<i>.start` / `.end` / `.offset` / `.amount` | number (fractions) | `paint` with `text.animators[i]` (`amount` also for key selectors) | selector parameters; damage = union of the evaluated glyph boxes before and after |
| `time.span` | number (s) | node with `time` | per-frame re-sample of the subtree while anything in the window moves; damage = ink over the samples |
| `effects.<i>.radius` / `.dx` / `.dy` / `.strength` | number | node with that effect | per-frame re-filter of the subtree; damage = evaluated ink (content grown by 3σ and offsets) before ∪ after |

`motion_blur` is per frame while the subtree moves within the shutter (damage = union of
the swept ink); a matte target is damaged when its source changes.

Easing (externally tagged JSON): `"linear"`, `{"cubic-bezier": [x1,y1,x2,y2]}` (solved
like Chrome's `gfx::CubicBezier`), `{"steps": [n, "jump-end"]}` (CSS step positions incl.
the before-flag rule), `{"spring": {"mass", "stiffness", "damping", "velocity"}}`
(closed-form damped oscillator stretched over the segment, settles exactly at 1).

### Asset / font

`assets: [{ "path": "...png", "scale": s }]`: PNG (straight alpha) relative to the IR
file (absolute paths allowed). For `image`/`sequence` content the PNG must be
`round(rect.w*s) × round(rect.h*s)`. `fonts: [{ "path": "...ttf", "index": 0 }]`.

### Camera

`camera: { perspective }` (2.5D) is **planned**, rejected.

## What the core derives from the IR (no pixels involved)

- per frame and node: committed local transform/opacity, chain to the frame, effective
  opacity; **quantized commits** (a node changes only when its subtree moves ≥ 1/16 px on
  screen or its opacity changes ≥ 1/255; `compiler/src/schedule.rs`), frames draw
  committed values, so damage and pixels agree;
- per frame and node: a signature of everything else that changes its pixels (content
  switch, trim, glyph tracks, effect parameters of the node and its ancestors, matte
  source state, motion-blur samples);
- ink per content node: content bounds through the chain, grown by ancestor effect
  margins, swept over motion-blur samples, clipped by ancestor clips;
- damage per frame = ink before ∪ after of every changed content node (merged rects);
  empty = repeated frame (VFR: not sent);
- full-damage frames = cut points for parallel segments.

## Validation

`agentfx render` validates first and fails loudly (all errors listed): tree shape,
track/keyframe consistency, property/value/node compatibility, interpolable transform
lists, asset/font existence, PNG sizes, push/pop balance, effect parameters, matte
sources, and planned-but-unimplemented features (`camera`).

### Digital and tape looks

These CPU node effects run in list order on the composited subtree. Colors are straight
sRGB RGBA arrays in 0..1. All five retain source coverage, except ASCII background alpha
which multiplies it. Samples clamp at the subtree boundary. New effects do not change
existing scenes. See `examples/looks/`.

| type | parameters | animatable fields |
|---|---|---|
| `rgbsplit` | `dx`, `dy`: -200..200 scene px; `amount`: 0..1; `mode`: `linear` (default) or `radial` | dx, dy, amount |
| `glitch` | `amount`: 0..1; `seed`: u64 (default 0); `rate`: 0..120 events/s; `block`: 1..1024 scene px | amount, rate, block |
| `vhs` | `amount`: 0..1; `seed`: u64 (default 0) | amount |
| `dither` | `levels`: 2..256 (rounded); `mode`: `bayer`, `bluenoise`, `floyd`; `size`: Bayer 2/4/8 (default 4); optional `palette`: 2..16 opaque RGBA colors | levels |
| `ascii` | `cell`: 6..24 scene px; `charset`: nonempty selection of ` .:-=+*#%@`, ordered dark to light; `color`: `mono` (white) or `source`; `bg`: RGBA | cell |

Tracks use `effects.<index>.<field>`. Seeds, modes, palettes, charset, and background are
static configuration. Animated numeric values are clamped to their supported range at
render time. Glitch and VHS include scene time in their signatures every frame, including
when their base amount is zero, so an amount track can enable them correctly. RGB split,
dither, and ASCII have no clock dependence and reuse the existing static effect cache.

RGB split offsets red and blue oppositely around green. Radial offsets grow from zero at
the subtree center toward the specified displacement at its edges. Glitch hashes the seed,
local block/slice, and `floor(time * rate)`; rate zero freezes one event. It combines block
shifts, thin horizontal tears, red channel separation, and occasional four-level blocks.
VHS combines drifting line wobble, nine horizontal chroma samples with unblurred luminance,
alternating scanlines, desaturation, and a vertically rolling noisy tracking band.

Bayer uses a recursive ordered matrix. Blue noise uses a fixed 32x32 toroidal progressive
farthest-point rank tile, generated once. Floyd-Steinberg diffuses error left to right with
two row buffers and attenuates error by coverage. No dither mode changes its noise with
time. Palettes replace channel quantization with nearest RGB palette matching. ASCII uses
an embedded 5x7 bitmap atlas, alpha-weighted cell color, and luminance to select a glyph;
no font assets are required. It is a deliberately small punctuation atlas, not arbitrary
Unicode text rendering. Cell size scales with the node and rounds to output pixels.
