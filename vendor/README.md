# Vendored crates

These crates are copies of the crates.io releases listed below, patched for the renderer
and wired in through `[patch.crates-io]` in the workspace `Cargo.toml`. Each keeps its
upstream licence; the licence files sit in the crate directory. They are not covered by
the project's own licence.

| Directory | Crate | Upstream version | Upstream | Licence | Licence files |
|---|---|---|---|---|---|
| `blitz-dom/` | blitz-dom | 0.3.0-beta.4 | https://github.com/dioxuslabs/blitz | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` |
| `blitz-paint/` | blitz-paint | 0.3.0-beta.4 | https://github.com/dioxuslabs/blitz | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` |
| `parley/` | parley | 0.12.0 | https://github.com/linebender/parley | Apache-2.0 OR MIT | `LICENSE-APACHE`, `LICENSE-MIT` (upstream) |
| `stylo/` | stylo | 0.22.0 | https://github.com/servo/stylo | MPL-2.0 | `LICENSE` (full MPL-2.0 text); per-file MPL headers kept |

The crates.io packages of blitz-dom, blitz-paint and stylo ship without licence files;
the texts added here are the standard ones named by each crate's `license` field
(the MIT text has no copyright line, like upstream blitz's `LICENSE-MIT`).

## Modifications

Every change is marked with a `reel patch` comment (`grep -rn -F "reel patch" vendor`).
Modified files:

- `blitz-dom/assets/default.css`
- `blitz-dom/src/layout/construct.rs`
- `blitz-dom/src/layout/inline.rs`
- `blitz-dom/src/lib.rs`
- `blitz-dom/src/resolve.rs`
- `blitz-dom/src/stylo_to_parley.rs`
- `blitz-paint/src/gradient.rs`
- `blitz-paint/src/kurbo_css/css_box.rs`
- `blitz-paint/src/layers.rs`
- `blitz-paint/src/lib.rs`
- `blitz-paint/src/render.rs`
- `blitz-paint/src/render/background.rs`
- `blitz-paint/src/render/border.rs`
- `blitz-paint/src/render/box_shadow.rs`
- `blitz-paint/src/render/form_controls.rs`
- `blitz-paint/src/text.rs`
- `parley/src/layout/line_break.rs`
- `stylo/properties/longhands.toml`

The stylo files above remain under MPL-2.0 (file-level copyleft): their source,
including the modifications, must stay available under MPL-2.0.

## Third-party files inside the vendored crates

- `blitz-dom/assets/moz-bullet-font.otf`: shipped unchanged by upstream blitz-dom (used
  for `-moz-bullet-font` list markers). Its name table reads "Mozilla Bullet", Version
  1.0, vendor "Mozilla", designer "Mats Palmgren"; it carries no copyright or licence
  string. It comes from Firefox, whose sources are MPL-2.0, so treat it as MPL-2.0
  (presumed, not stated in the file).
- `stylo/properties/vendored_python/`: Mako, toml and MarkupSafe (build-time code
  generation), under their own licences (MIT, MIT, BSD-3-Clause), carried in the
  wheels / package directories.
