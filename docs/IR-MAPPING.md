# Reel Script motion IR → reel-ir (direct mode)

Status 2026-10-26, aligned with `docs/IR.md` (v1).

> **S1 update (supersedes the S0 notes below where they differ):** the geometry step is native.
> `src/native.rs` writes a static snapshot of the HTML backend's page (every channel a literal,
> no animation) and lays it out with Blitz (Stylo + Taffy + Parley; reel's patched crates, used
> unmodified; `native::layout_mode` picks whole-px box rounding with 1/64 px shrink-to-fit widths). Every layer's *own* paint (box, borders,
> shadows, rich text glyph runs, inline SVG, raw `html`) is recorded by reel's subset painter as a
> reel-ir `paint` display list (vector, no rasters, no Chrome, no `--compat m0`). Paint/layout
> channels reel-ir has no track for become `switch` repaint states plus layout-offset keys;
> wipe/iris/masks become mattes; 3D is linearized per frame. What reel-ir still lacks:
> `IR-PROPOSALS.md`. Box agreement with Chrome: 99 % of layers within 1 px (product tour 311/314,
> showcase 228/228), the rest are SVG intrinsic sizes.

```
.reel → AST → timing (TTS words) → motion IR (src/ir.rs) ─┬─ html backend (src/html.rs): preview, Chrome reference
                                                          └─ reel-ir backend (src/reelir.rs): `agentfx ir --target reel-ir`
```

The motion IR is backend-neutral on purpose: layers with layout *constraints*, typed channels with
symbolic eases (springs stay springs), paints stay paints. reel-ir is a *resolved* scene: scene-space
geometry, CSS-timed tracks. The gap between them is **layout**, so the reel-ir backend has a
geometry step:

- **S0 (now):** one Chrome pass (`capture.mjs geom`): every layer's box in scene px with all
  animation off and all transform channels at identity, plus text metrics (content box, size,
  weight, tracking, line height, color, align). Raw SVG/HTML layers and rich text are rasterized at
  2x on a transparent page (`*.ir.rasters/`). Total: ~1 s for the showcase, 1.6 s for an 80 s product tour.
- **S1+ (proposed):** native layout in Rust (taffy for flex/grid, parley for text), so direct mode
  needs no Chrome at all. The geometry step is isolated (`Value` in, `G` rects out), so it can be
  swapped without touching the mapping.

## Mapping

| Motion IR | reel-ir | Status |
|---|---|---|
| `Layer` tree | `Node` tree (`nodes[0]` = stage), paint order = child order | done |
| `Layout` (flow/abs/fill, flex, grid, padding) | resolved into scene-space rects (geometry step) | done (via Chrome) |
| `style.origin` (fractions) | `origin` = rect.xy + origin * rect.wh (scene coords) | done |
| `Kind::Image` | `content: image {asset, rect}`, `asset.scale = png_w / rect_w` (screenshots at 2x land exactly on 2.0 at rest, as reel wants) | done |
| `Kind::Rect/Ellipse/Group` + solid fill | `content: rect {rect, radius, color}` | done |
| fill with gradient / border / box-shadow | `content: paint {items: [box-shadow, fill(linear/radial/sweep brush), stroke]}` | emitted; this reel build accepts but does not draw `paint` yet |
| `Kind::Text` (single style) | `content: text {text, font, size, weight, letter_spacing, line_height, pos, max_width, align, brush}` (a child node, so a box fill can sit below) | emitted; drawn once reel M1 text lands |
| text matte (`fill=linear(...)` on text) | `text.brush = linear` | emitted |
| rich text (`**bold**`, `{accent}`, per-run colors), `svg`/`html` raw | `image` from a 2x Chrome raster | done (fallback) |
| `Kind::Path` (+ `vb`) | `content: path {d (absolute and relative SVG data mapped to scene coords), fill (+ `fill_transform` for mesh/image), stroke{width, cap, join, dash, brush (solid or gradient)}, trim}`; animated fill/stroke colors -> `fill.color` / `stroke.color` tracks | done |
| `brightness` `saturate` `contrast` `hue` + `tint` `levels` `lut` | node effect `color` (+ `effects.<i>.<field>` tracks) | done |
| mesh / image paints | mesh: one cached OKLab raster (`src/mesh.rs`) drawn as an image (box: CSS background via Blitz; path: image brush + `fill_transform`; text: image fill composited `src-atop` over the glyphs; mask: image brush) | done |
| text v2: `font "X" src=`, `axes=`, `features=` | fonts registered in the native layout (WOFF/WOFF2 decoded, written next to the IR); axes/features as CSS on the snapshot, so the recorded glyph runs carry the coords and feature-shaped glyphs | done |
| text `stroke=` `sw=` | `glyphs.stroke {width: 2*sw (or sw with stroke-over), brush, join: round, over}` on every glyph run of the text's paint (and its repaint states) | done |
| `color` on plain text (no accent/code runs, matte or child layers) | `text.fill` color track on the paint (no repaint states; pixels equal) | done |
| `animate #t ...` (range selector / per-unit key times) | `paint.text.animators[i]` (`by`, `selector`, props) + `animators.<i>.start/end/offset/amount` tracks for `a..b` parameters | done |
| `on=#path offset=` + `path-offset` channel | `paint.text.path {d (the path layer's scene-space data), offset}` + `text.path-offset` track | done |
| `Kind::Counter` + `count` track | `content: switch {frames: [[frame, state]], states: [text, ...]}`, one state per integer change | done |
| channels `x y px py scale sx sy rot magic` | one `transform` track, list `[translate, rotate, scale]`; single animated channel → symbolic keys with their eases; several overlapping → sampled per frame (runs of equal values collapsed) | done |
| `opacity` | `opacity` track + node `opacity` base | done |
| `blur`, `glow` | `effects: [blur{radius}, glow{radius, color}]` + `effects.<i>.radius` tracks | emitted; current build rejects effect tracks (`--compat m0` drops them) |
| `trim-start`, `trim` | `trim-start` / `trim-end` tracks | emitted; current build rejects (dropped by `--compat m0`) |
| eases | `linear`, `cubic-bezier`, `steps(n, jump-end)` (hold = steps(1)), `spring{mass, stiffness, damping, velocity}` (reel's closed form, no CSS sampling), bounce → approx. cubic | done |
| `overflow_clip`, wipe/iris clip shape | `clips: [RRect]` (static) | static only |
| `blend` | `blend` (all W3C modes + plus; reel `82b0c5e`) | done |
| `style.motion_blur` (`mblur=` `shutter=`) | node `motion_blur {samples, shutter}` (shutter in frames) | done |
| `style.looks` (`riso=` `halftone=` `grain=` `paper=` `shadow=soft()` `rays=`, verb `sweep`) | node effects `riso` `halftone` `grain` `paper` `shadow` `sweep` `rays` (`src/look.rs`); channels `misreg` `halftone` `sweep` `rays` -> `effects.<i>.misreg/cell/pos/strength`; a `light` element's world center baked per frame into `effects.<i>.lx/ly` | done |
| `style.looks` (`rgbsplit`, `glitch`, `vhs`, `dither`, `ascii`) | matching subtree effects; nine numeric channels map to `effects.<i>.amount/dx/dy/rate/block/levels/cell` | done |
| `style.knockout` (child flag) | child `hidden`, parent effect `matte {source, mode: alpha-inverted}` first in its list | done |
| time remap (`rewind`, `timemap`) | nothing new: the compiler evaluates every track of the subtree through the composed remap at each output frame and emits per-frame linear keys (`src/remap.rs`); keys before the first remap event stay symbolic | done (exact at frames) |
| magic move hook | geometry deltas computed from the two measured rects, baked into the transform track | done |
| link (connector) hook | needs the connector path from layout | planned (path computed in the geometry step) |
| `fit` hook | already applied by the geometry pass (font size measured after fitting) | done |
| scenes, transitions | scene roots are nodes; visibility and fade/push/slide/zoom are opacity/transform tracks; wipe/iris need clip tracks | partly |
| quantization | same constants: 1/16 px commit, 1/255 opacity (reel `POS_EPS_PX`, `OPACITY_EPS`); Reel Script snaps keys to 1/64 px and 1/255 before emitting | aligned |
| audio reactivity (`audio(...)`, `pulse on=beat`, `visualizer`, `beat:N` anchors) | nothing new: `src/audio.rs` analyses the music at compile time and lowers to `layer`/`rect` nodes and per-frame linear keys before compile, so they map like any other transform/opacity/effect track | done (no IR change) |
| audio (voice, music, sfx) | not in reel-ir; `agentfx render` mixes and muxes | proposal below |

## What reel-ir cannot express yet (reported as `W_IR_UNSUPPORTED`, never faked)

Counts from `agentfx ir --target reel-ir examples/showcase.reel`:

| Feature | Needed in reel-ir | Proposal |
|---|---|---|
| 2.5D camera, `z`, `rotx/roty` (86 nodes) | `camera {perspective}` + per-node z | as planned in IR.md (projective warp) |
| `fill`, `stroke` tracks on boxes, `color` on rich/split text | color interpolation of a brush | plain text color is done (`text.fill`); boxes and rich text still repaint |
| `grad-angle`, `grad0..3` | animated gradient | `brush.angle`, `brush.stops.<i>.color` tracks on `paint`/`text` content |
| `reveal` (wipe/iris/typing reveal) | animated clip | `clip` track interpolating an RRect (wipe) or radius (iris); typing reveal is better as glyph tracks |
| typing, kinetic split text | per-glyph/word tracks | `animate` covers it on one node (text v2 animators); `split=` still emits one layer per unit |
| `line-h`, `w`, `h` (code diff collapse, layout channels) | relayout | keep layout out of reel-ir: Reel Script resolves layout per keyframe and emits `transform`/`clips` (diff collapse = translate of following lines + clip) |
| paint masks + `mask-x` | matte | `effects: matte{source}` with a hidden source node carrying a paint; `mask-x` = transform track on the source |
| grain | per-frame noise | `effects: grain{amount, seed}` (per-frame stage) |
| box-shadow spread / inset | `Draw::BoxShadow` has no spread/inset | add `spread`, `inset` |
| audio | voice/music/sfx clips with ducking | top-level `audio: [{src, at, gain, duck}]`, mixed by the encoder step (DESIGN M6) |

## Measured (2026-10-25)

`--compat m0` (for the current reel build, which draws `image`/`rect` only) adds a raster fallback.
Text, paths, caption groups and painted boxes are cut out of the compiled page in place, in one Chrome pass
(everything else hidden, transparent background, 2x). This is the same per-layer idea as reel's HTML mode.
Native text/paint content is emitted without `--compat` and takes over once reel M1 draws it.

| | HTML backend → Chrome (i7-2600K, 4 workers) | reel-ir direct → reel (i5-9500T, load ~10) |
|---|---|---|
| Product tour, 2394 frames | 115 s (20.8 fps, load ~3); 470 s under load 9 to 12 | **18 s emit (geometry + 98 rasters) + 52 s render**, VFR (1120 distinct frames) |
| Fidelity (direct vs Chrome, time-aligned, both x264) | | **SSIM mean 0.994, median 0.994, min 0.988** (target ≥ 0.98) |
| Showcase 16:9, 648 frames | 85 s (7.6 fps) | 5.5 s without the raster fallback (3D camera, gradient animation, masks: unsupported in reel-ir v0) |


## Presenter compiler spike

`presenter` expands in `src/presenter.rs` to body, torso, head, facial features,
and upper-arm/forearm/hand layers. Each bone has an explicit pivot. Artwork is
`Kind::Path`; breathing, gaze, blinks, gestures, and lip poses use composite
transform/opacity tracks. No new renderer content or damage mechanism is used.
Pointing resolves native layout boxes in the existing two-pass compiler, then
solves two-bone IK. Mouth poses are opacity-swapped path groups driven by the
scene's absolute word times. Paper edge jitter and idle timing are seed-stable.
