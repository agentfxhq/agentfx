# Imports: brand kits, SVG, Lottie, Figma (P5)

What a company brings (a brand guide, a logo file, After Effects animations, a Figma file) and how
it gets into a Reel Script video without hand-writing tokens. Status as of P5; syntax in
[SPEC.md](SPEC.md) ("Brand kits and imports"), agent usage in `skill/agentfx/SKILL.md`.

| Source | Path into a video | Status |
|---|---|---|
| Brand guide (colours, fonts, logo, motion, safe areas) | `brand "brand.json"` | implemented |
| Logo / icon / illustration SVG | `svg #logo src=logo.svg parts` (native vector parts) or `logo #l` | implemented |
| Lottie (Bodymovin JSON) | `lottie #l src=anim.json` (rendered by velato) | implemented (subset, see below) |
| Figma variables | `agentfx brand from-figma vars.json` -> brand kit | implemented |
| Figma frames | Figma SVG export (with ids) -> `svg ... parts` | works today (documented path) |
| Website CSS | `agentfx brand from-url` | not built (see below) |

## Brand kit

One JSON file next to the video (template: `agentfx brand init`, which also writes a sample
`logo.svg`). It expands at load time into the plain theme lines, `font` declarations and asset
tokens it stands for, so every backend, lint and the determinism rules see ordinary tokens; lines
in a `theme` block after the `brand` line override it. Paths in the kit are relative to the kit.

Why JSON and not a `.reel` block: the theme block already is the `.reel` form; a brand kit is the
artefact a design team or a tool (Figma, a token pipeline, an agent reading a brand guide) hands
over, and JSON is what those produce and validate (`$schema` is accepted and ignored).

Role mapping (what the defaults of every component use):

| kit | theme tokens |
|---|---|
| `colors.primary` | `$accent` (gradients, numbers, cloud and fluid defaults) and `$primary` |
| `colors.secondary` (default: primary lightened/darkened) | `$accent2` (default shape fill, glows, bars) and `$secondary` |
| `colors.accent` (default: secondary) | `$mint` (text accents `{...}`, kicker labels) and `$highlight` |
| `colors.fg` | `$text`, `$fg`; `$muted` (fg mixed 28 % toward bg), `$faint` (fg at 55 %) |
| `colors.bg` | `$bg`, `$bg2`, `$surface`, `$surface2`, `$border`, `$code-bg` (derived), the background paint |
| `colors.named.*` / other keys | `$<name>` (lower-case, `-` separated) |
| `background` | the scene background paint (any reel paint) |
| `fonts.display/body/mono` `{family, src}` | `font "family" src=...` + `type display= body= mono=` |
| `logo.src / on-dark / on-light / mark` | `$logo` (follows the bg: the on-dark variant on a dark bg), `$logo-on-dark`, `$logo-on-light`, `$mark` |
| `motion.ease` | eases `enter out std move`; `ease-exit`, `ease-enter`, `ease-move` per role |
| `motion.enter/exit/move/fast/base/slow/hold/stagger/beat/distance` | the motion durations (bare numbers are seconds) |
| `layout.safe/safe-x/safe-y/margin/radius/radius-sm/grid/gutter` | layout tokens |
| `type.base/ratio/...` | the design layer's type scale (`type base= ratio=`) |
| `base: dark|light` | built-in base theme for the tokens a kit does not set (default: from the bg luminance) |

Diagnostics: `E_BRAND` (file missing or not JSON), `E_BRAND_KEY` (unknown key, did-you-mean),
`E_BRAND_COLOR`, `E_BRAND_FONT`, `E_BRAND_FILE` (font/logo/asset file missing or of the wrong
type), `E_BRAND_MOTION`, `W_BRAND_LOGO`; unknown `$asset` tokens are `E_TOKEN` with the list of
defined ones.

### `agentfx brand from-url`: not built

Extracting a kit from a live site is not cheap and not robust enough to ship:

- **Network at compile time** breaks the determinism rule (SPEC 12: no network at render time);
  it would have to be a separate fetch step anyway.
- **CSS does not say which colour is the brand colour.** Real sites ship minified bundles, CSS
  custom properties chained through frameworks (Tailwind layers, design-system tokens), dark-mode
  overrides and dozens of greys; a frequency count picks link-blue and border-grey. Ranking needs
  rendering (computed styles of the visible hero/buttons), i.e. a browser.
- **Fonts are usually not embeddable**: webfont licences commonly cover browser use, not
  rendering into a video; downloading them silently would be the wrong default.
- The useful part already exists: `agentfx design harvest <frame.html>` extracts computed styles
  of saved pages into a design spec, and an agent with a browser can read a brand page and write
  `brand.json` directly (the format is small and validated with fix hints).

## SVG import (`svg ... parts`)

The file goes through usvg (already in the dependency tree via Blitz; same version), which
normalises CSS, `<use>`, units, transforms and basic shapes into paths. Every path becomes a reel
`path` layer (absolute geometry, transforms baked, mapped into the element's box with the SVG's
aspect, centered when `w` and `h` disagree), every `<g id>` (and every group with opacity) a group
layer at its bounding box. So the parts are real vector layers: crisp at any scale and zoom,
trim/draw-on, fill and stroke colour tracks, morph, all verbs and channels.

- Addressing: `#logo.<svg id>` (ids sanitised to `[A-Za-z0-9_-]`, so `word.mark` becomes
  `word-mark`); unnamed paths are `#logo.p1`, `#logo.p2`, ... in document order. Unknown part ids
  are `E_SVG_ID` with the part list and a did-you-mean.
- `draw #logo` (or a part group) traces its paths one after another within `dur`; filled shapes
  (no stroke, typical for logos) trace their outline in the fill colour, then the fill fades in
  while the outline fades out.
- Colour: `fill=` / `stroke=` override every part, `recolor="#e30613:$primary, #111:$fg"` maps the
  file's colours (alpha kept) onto tokens. Gradients: linear exact (stops re-projected onto the
  part's gradient line), radial with centre and radius (focal point dropped).
- Fill rule (`evenodd`), stroke caps, joins and two-value dashes are kept (fill rule is a new,
  optional reel-ir field `fill_rule`).
- Dropped with `W_SVG_PARTS` (fix hint each): clip paths, masks, filters, pattern fills, embedded
  images, text that is not outlined (no font database: outline text in the editor). Use the raw
  `svg` (no `parts`) when those matter; it is unchanged.

## Lottie

### Options evaluated

| Option | Licence | Fit with the renderer | Verdict |
|---|---|---|---|
| **velato 0.12** (linebender) | Apache-2.0 OR MIT | Pure Rust; depends on kurbo 0.13 and peniko 0.6, the exact versions the renderer already uses; vello is an optional feature (off). Exposes a renderer-agnostic `RenderSink` trait (push layer / clip, draw shape with brush or stroke, pop) | **chosen** |
| Own importer (Lottie -> reel layers + tracks at compile time) | ours | Natural mapping for the common subset: layers -> group layers, `ks` transforms -> transform tracks with the per-key cubic eases, shapes -> paths, `tm` trim -> trim tracks, `fl`/`st` colours -> colour tracks, shape keyframes -> path morph | not now (cost below); worth it only for editing Lottie inside reel |
| rlottie bindings, dotLottie (ThorVG) | MIT-family, C/C++ libraries | Native C++ build and a second rasterizer; output is a bitmap per frame (no vectors in our pipeline) | no |
| `lottie` / `lottie-*` crates on crates.io | various | Parsers or engine-specific players (Bevy, GPUI), mostly stale or tied to a UI framework | no |

Cost of an own importer, in this architecture: parsing is cheap (serde), but the semantics are
not: shape-group transform stacks, repeaters, trim modes (simultaneous / individual), merge
paths, rounded corners, parenting, precomps with their own time, masks and track mattes,
hold/spatial bezier keyframes. A useful subset (shapes, transforms, trim, colour and path
keyframes, parenting, precomps) is roughly 1500-2500 lines plus a test corpus, and every gap is
visible as a wrong frame. velato already implements that subset and more.

### What is implemented

`lottie #l src=anim.json w=600 [loop|hold] [speed=1] [offset=0s]`: a new reel-ir content
`lottie` (path, rect, start, offset, rate, looped, hold; the same clock as `video`). The renderer
loads the file with velato at prepare time and, per frame, evaluates the composition at the exact
fractional Lottie frame (frame n is a pure function of n) and draws it through a `RenderSink` on
the vello_cpu context: contain fit, centered in the box; the element's own opacity, transforms,
effects and blend modes apply as for any layer. The box takes the file's size (or aspect when one
of `w`/`h` is given). Lint reads the header (`E_LOTTIE` for files that are not Lottie JSON) and
warns (`W_LOTTIE`) for what velato does not draw: text layers, image layers, expressions, time
remap, layer effects. Velato's own list of gaps: position keyframe tangents (`ti`/`to`) easing,
stroke dashes and zig-zag, colour stop handling details, split rotation/position.

Not done: Lottie in the Chrome reference backend (it shows the empty box; a lottie-web runtime
would be the way), recolouring Lottie fills to brand tokens (possible in the sink: map colours
like `recolor=`), `.lottie` (zip) containers.

## Figma

Paths, cheapest first:

1. **Variables -> brand kit** (implemented): `agentfx brand from-figma vars.json [-o brand.json]`.
   Input is any of: the REST response of `GET /v1/files/:key/variables/local` (default mode of
   each collection, aliases resolved), the Figma MCP `get_variable_defs` map (`{"Colors/Primary":
   "#0A5CFF", "Font/Body": "Font(family: \"Inter\", ...)"}`), or W3C / Tokens Studio design tokens
   (`$value`/`$type`). Colour roles come from the variable names (primary/brand, secondary,
   accent/highlight, background/bg, foreground/fg/text); every colour also becomes a named token;
   font family strings fill `fonts.*.family` (add a local `src`); a radius variable fills
   `layout.radius`. It prints what it mapped and what is missing.
2. **Frames -> SVG -> `svg ... parts`** (works today, no code): export the frame or the logo as SVG
   with *Include "id" attribute* on (layer names become ids, so `#logo.wordmark` matches the Figma
   layer) and *Outline text* on. Automation: REST `GET /v1/images/:key?ids=<node>&format=svg&svg_include_id=true`
   or the Figma MCP asset download. Effects (shadows, blurs) export as SVG filters and images as
   `<image>`: both are dropped with `W_SVG_PARTS`; re-create shadows with reel `shadow=`/`glow=`.
3. **Not built**: turning Figma prototype transitions or Smart Animate into reel motion, and
   mapping Figma auto layout to reel layout containers. Both need a node-tree importer (REST
   `GET /v1/files/:key/nodes`); the SVG path covers logos and illustrations, which is what brand
   videos need first.
