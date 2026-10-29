# reel HTML-mode input (v1)

> **Changes (M3, 2026-10-26)** — **breaking for producers: three inputs that used to render
> wrong silently now fail loudly** (found in the M3 review):
> - an animated element (layer) that itself clips (`overflow`, `clip-path`), has a
>   non-animated `opacity` < 1, a `filter` or a `mix-blend-mode`, and contains another
>   animated element. Exempt: clips that cover the whole 1920x1080 viewport (full-screen
>   scene containers, as in a typical screencast recording);
> - a held (discrete) animation of `transform`/`translate`/`rotate`/`scale`/`opacity` that
>   outranks an interpolated animation of the same property on the same element while both
>   are active (e.g. `animation: a .., b ..` with `b` a `%`-translate);
> - the transform origin of a rotated/scaled element moving by layout, or the static
>   ancestor transform of any animated element changing between change frames (was a
>   warning, "kept at the first").
>
> Rendering: lines inside a paragraph now stack at fractional positions (Chrome's 1/64 px
> line boxes; Parley rounded every line top to whole px, which drifted text by ~0.5 px per
> line, magnified by camera zoom). Font metrics stay quantized as in Chrome.
> `AGENTFX_PARLEY_ROUND_LINES=1` restores the old behaviour. Gradients are dithered and text
> gets Skia's gamma (see `IR.md`).

> **Changes (M2, 2026-10-26)** — the format is a superset of v0; every v0 scene still
> compiles (and renders as before: SSIM vs Chrome unchanged on the M1 scenes).
> - **Accepted now** (were rejected in v0): inline `animation` declarations (style
>   attribute); individual `translate` / `rotate` / `scale` properties as keyframes, each
>   with its own easing; `display` keyframes (mount/unmount); keyframes for *any* other
>   property, including custom properties (`--x`), shorthands (`background`, `border`, …)
>   and values with `var()` / `color-mix()` (held discretely: use `step-end`/`steps()`;
>   interpolated only for px/number/%/colour values); `transform` keyframes with `%`
>   translates or `matrix()` (held discretely); z-index and stacking contexts; static
>   transforms, opacity and clips on ancestors of animated elements; `::before`/`::after`
>   content; non-zero `transition` is still rejected; `mix-blend-mode` and filter chains
>   paint as Blitz paints them (filter chains of one primitive per element).
> - **Planner:** the default is the M2 *probe planner* (`compiler/src/plan2.rs`); the M1
>   layer planner remains as `--planner m1`.
> - **Fonts:** a fixed system set is registered in addition to the scene's own fonts (see
>   *Fonts*); generic families follow Chrome on Linux.
> - **Recorded scenes** (`<meta name="reel-frames">`, written by the recorder) get LCD
>   text antialiasing (IR `text_aa: lcd`), matching Remotion/HyperFrames renders.

This is the contract for producers of HTML-mode scenes (hand-written scenes, the
recorder, a separate tool not included here). `agentfx run <scene_dir> --out x.mp4` renders a
scene directory; `agentfx html <scene_dir> --out-dir d` only compiles it to IR
(`d/scene.ir.json`, see `IR.md`).

## Scene directory

```
<scene>/
  index.html        required
  *.css             linked from index.html (relative paths)
  tracks.json       optional, named by <meta name="reel-tracks">
  images, fonts     local files only (no network)
```

`index.html`:

```html
<!doctype html>
<html><head>
<meta charset="utf-8">
<meta name="reel-duration" content="8">        <!-- seconds, required -->
<meta name="reel-fps" content="30">            <!-- integer, default 30 -->
<meta name="reel-frames" content="240">        <!-- optional; marks a recorded scene -->
<meta name="reel-tracks" content="tracks.json"><!-- optional -->
<link rel="stylesheet" href="scene.css">
<script src="../_shared/reel.js"></script>     <!-- only for the Chrome reference -->
</head><body> ... </body></html>
```

- Viewport is **1920×1080** (CSS px = device px). The document must not scroll.
- Frame n shows time n / fps. Frames = round(duration × fps).
- `reel.js` (in `scenes/_shared/`, or the recorder's `runtime.js`) makes the page seekable
  in Chrome (`window.reel.seek(t)`); reel itself ignores scripts.

## Fonts

Deterministic, no fontconfig lookup. Registered, in this order:
1. `<scene>/fonts/*.ttf|otf`, `<scene>/../_shared/*.ttf|otf`, and `@font-face` with a
   relative `url()` (ttf, otf, woff, woff2);
2. the system set of the render machine: `~/.local/share/fonts`,
   `/usr/share/fonts/truetype/liberation`, `/usr/share/fonts/truetype/noto` (glyph
   fallback), Nimbus Sans (urw-base35).

Generic families resolve like Chrome on Linux (measured with CDP
`getPlatformFontsForNode`): `sans-serif` → Liberation Sans, `serif` → Liberation Serif,
`monospace` → Liberation Mono, `system-ui` → Nimbus Sans; `ui-*`, `-apple-system` and
unknown families fall through to the next family in the list.

- Web fonts (any `@font-face` in the document): automatic optical sizing (`opsz` = font
  size) and exact `font-weight` on the `wght` axis, as Chrome does for web fonts.
- Local fonts (no `@font-face`): Chrome resolves them through fontconfig, i.e. to the
  nearest *named instance* of a variable font (550 → Medium 500, 650 → SemiBold 600) at its
  default optical size; reel does the same.

## Animation

CSS `@keyframes` + `animation` (shorthand or longhands, in stylesheets or in the `style`
attribute), with:

- iteration count 1, direction `normal`, play state `running`;
- `animation-fill-mode` none / forwards / backwards / both (outside an animation's active
  interval with fill `none` the element shows its stylesheet value);
- timing functions: `linear`, `ease*`, `cubic-bezier()`, `steps()` (all positions), also
  per keyframe;
- explicit `0%` and `100%` keyframes for every animated property;
- several animations per element (later in the list wins per property).

| kind | properties | how reel renders it |
|---|---|---|
| composite (IR node tracks, no repaint) | `transform` (px translate, scale, rotate), `translate`, `rotate`, `scale`, `opacity` | one IR node per property (translate → rotate → scale → transform), each with its own keyframes/easing; values conjugated into scene space by the static ancestor transform |
| state (repaint/re-layout per distinct state) | everything else: colours, shadows, sizes, `display`, `visibility`, custom properties, shorthands, `%`/`matrix()` transforms, … | every frame where a state value changes is a *change frame*: Blitz re-styles, re-lays out and paints the document once (composites neutral); only the elements whose paint changed are re-recorded (see *Planner*) |

Interpolation of state values: px lengths, numbers, %, colours (`#hex`, `rgb[a]()`,
`transparent`, `white`, `black`) and space-separated lists of those. Anything else
(`var()`, `color-mix()`, keywords, multi-shadow lists, …) must be held (`step-end` /
`steps()` between different values); interpolating it fails loudly.

## Planner (what reel does with a scene)

`compiler/src/plan2.rs`: change frames → probe paints (whole document, culling off, each
display-list item attributed to the element that issued it) → layers (elements with
composite tracks), groups (elements whose clip/opacity/filter/blend encloses a layer;
their outset shadow/outline stays outside the group), regions (lowest common ancestor
of the elements whose paint changed at a change frame) → units (runs of elements in
the paint order Blitz produces, merged over all probes) → IR `Switch` contents (one
display list per distinct state). Paint order comes from Blitz's own stacking contexts,
so z-index works; layers keep their stacking context while neutralized (identity values
plus `isolation: isolate`).

## tracks.json (escape hatch)

```json
{ "tracks": [
  { "selector": "#typed", "property": "text",
    "text": "Footer: Öffnungszeiten …",
    "keys": [ { "t": 0.9, "v": 0 }, { "t": 3.6, "v": 54, "ease": "linear" } ] } ] }
```

`text`: the element shows the first floor(v) characters of `text` (typed text). The
selector must match exactly one element whose content is text only. `ease` describes the
segment arriving at that key (`linear`, `ease*`, `cubic-bezier()`, `steps(n)`).

## Fails loudly (the whole list is reported)

- iteration count ≠ 1, `animation-direction`, non-`running` play state, non-zero
  transitions, animations inside `@media`/`@supports`/…, dynamic pseudo-classes in
  animated selectors, `!important` in keyframes, non-integer fps;
- 3D transforms/rotations, skew in animated transforms (held discretely only via `%`/matrix);
- rotate/non-uniform scale tracks under a rotated or skewed static ancestor transform;
- a group effect (clip/opacity/filter) around a layer that changes over time, or a
  non-rectangular clip (`clip-path`) around a layer;
- a composite property with no active animation at some frame *and* a static `transform`
  (the stylesheet `translate`/`rotate`/`scale`/`opacity` fallback works);
- paint order of an IR parent that changes between states (cycle);
- `backdrop-filter`, synthetic bold, unsupported paints (reported by the recorder).

Known gaps (render differs from Chrome, not rejected): sub-pixel *box* layout (Blitz
rounds boxes to whole px; inside zoomed panels this still moves text by ~1 px, the main
remaining SSIM loss on a recorded product tour); `accent-color` (Stylo/Servo has
no such property: checkboxes use the text colour), focus rings (`:focus-visible` state
is not part of the input), box-shadow on non-atomic inline elements, layout at
sub-pixel precision (Blitz rounds boxes to whole px; shrink-to-fit widths use 1/64 px).
