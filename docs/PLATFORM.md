# AgentFX platform (hosted render, remote MCP, playground, templates)

Status: in development (started 2026-11-09). This file is the contract between the work packages;
change it only together with the code on both sides.

## Packages

| # | Package | Dir | Depends on |
|---|---------|-----|-----------|
| P1 | Render service + remote MCP (HTTP) | `service/` | CLI, `mcp/server.py` |
| P2 | Web playground + light timeline editor | `web/` | P1 API (this file), P3 manifest format |
| P3 | Templates (music visualizer, lyric video, effect templates) | `templates/` | CLI |
| P4 | Agent eval harness + quality work on spec/lint/skill | `evals/`, crates, `skill/` | CLI, MCP |
| P5 | Brand kits and imports (SVG, Lottie) | crates, `docs/SPEC.md` | CLI |
| P6 | Effect landing pages with embedded playground | agentfx.dev site (Lore) | P2, P3 |

## Templates (P3 writes, P1 and P2 read)

```
templates/<id>/
  template.reel     Reel Script with {{param}} placeholders
  manifest.json     see below
  preview.webp      small looping preview (<= 1.5 MB), rendered from the defaults
  README.md         one paragraph: what it is, which search intent it serves
```

manifest.json:
```json
{
  "id": "glitch-text",
  "title": "Glitch text",
  "description": "One line for the gallery card.",
  "tags": ["text", "glitch"],
  "formats": ["16:9", "9:16", "1:1"],
  "duration_s": 6,
  "params": [
    {"id": "text",   "type": "text",   "label": "Text", "default": "AGENTFX", "max_len": 40},
    {"id": "color",  "type": "color",  "label": "Colour", "default": "#3CBF87"},
    {"id": "speed",  "type": "number", "label": "Speed", "default": 1, "min": 0.5, "max": 2, "step": 0.1},
    {"id": "style",  "type": "enum",   "label": "Style", "default": "rgb", "options": ["rgb", "mono"]},
    {"id": "audio",  "type": "audio",  "label": "Music (mp3/wav)", "required": false},
    {"id": "lyrics", "type": "longtext", "label": "Lyrics, one line per row", "max_len": 4000}
  ]
}
```
Substitution is plain text: `{{id}}` is replaced by the value. The substituter escapes `"` and `\`
inside text values and rejects newlines in `text` params; `longtext` is only used where the template
expands it (P3 documents how, e.g. a `{{#lines}}...{{/lines}}` block, implemented in
`templates/render_template.py`, which P1 imports). Colours must match `^#[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$`,
numbers are clamped to min/max, enums must be one of options. `audio` is an uploaded asset id
that the service maps to a file name in the job dir.

`python3 templates/render_template.py <id> [--set k=v ...] [-o out.reel]` instantiates a template
locally (stdlib only) and is the single implementation of substitution.

### Still templates (P3b)

A template renders a single still with `agentfx frame` (same engine as video: exact text, brand
colours) when its manifest says so:

```json
{
  "id": "flyer",
  "kind": "still",
  "still_at": "0s",
  "sizes": ["a4-flyer", "a5-flyer", "instagram-post", "story"],
  "batch": {"param": "recipients", "index": "index"},
  "params": [
    {"id": "image", "type": "image", "label": "Photo", "default": "photo.png", "accept": [".png"]},
    {"id": "rows", "type": "data", "label": "Menu", "min_rows": 1, "max_rows": 48,
     "columns": [{"id": "item", "type": "text", "max_len": 40, "required": true},
                 {"id": "price", "type": "number"}, {"id": "photo", "type": "image"}],
     "default": [{"item": "Soup", "price": 6}]}
  ]
}
```

- `kind`: `"video"` (default) or `"still"`. A still has no `duration_s`/`formats`; it renders one
  frame at `still_at` (default `"0s"`) per size: `agentfx frame <src> --at <still_at> -o x.png`.
- `sizes`: named presets the template is designed for, the first one is its main size (preview).
  Presets (px): `instagram-post` 1080x1080, `instagram-portrait` 1080x1350, `story` 1080x1920,
  `facebook-post` 1200x630, `facebook-cover` 1640x624, `linkedin-post` 1200x627,
  `linkedin-cover` 1584x396, `x-post` 1600x900, `x-header` 1500x500, `youtube-thumbnail` 1280x720,
  `youtube-banner` 2560x1440, `og-image` 1200x630, `pinterest-pin` 1000x1500, `full-hd` 1920x1080,
  `leaderboard` 728x90, `medium-rectangle` 300x250, `skyscraper` 160x600, `wide-skyscraper`
  300x600, `billboard` 970x250, `mobile-banner` 320x100, `a4-flyer` 2480x3508, `a5-flyer`
  1748x2480, `a3-poster` 3508x4961, `a4-landscape` 3508x2480, `letter-flyer` 2550x3300,
  `letter-landscape` 3300x2550 (print sizes at 300 dpi). The table lives in
  `templates/render_template.py` (`SIZE_PRESETS`). A template must look right at every size it
  declares; any other `WxH` (16..8192 px a side, at most 36 MP) is accepted best effort.
- The size is chosen at instantiation: `instantiate(id, params, templates_dir, size=None)` (default:
  the first size), `POST /instantiate {"template", "params", "size"?}`, CLI `--size`. The
  instantiated source has `video size=WxH` set to the exact output pixels, so `/frame` and
  `/lint` need no `format` for stills. Sizes are not CLI `--format` aspects (those keep the short
  side at the base size).
- Size names inside the template (uppercase, so no param can shadow them): `{{W}}` `{{H}}` output
  px, `{{S}}` scale, `{{DW}}` `{{DH}}` the frame in design units (the short side is 1080, or the
  manifest's `design_short`), `{{A}}` aspect W/H, `{{SIZE}}` the preset name, and `SIZE_<name>`
  (1 for the current size, `-` written `_`) in expressions. Still templates draw into one
  `layer #page w={{DW}} h={{DH}} x=0 y=0 scale={{S}} origin=top-left`: one design, crisp at
  every output size (vectors are drawn after the scale), with % positions inside the page.
- `image` param: an uploaded asset name (plain file name, no paths or URLs), PNG for direct mode
  (`accept`, the service converts JPEG/WebP uploads to PNG). Default images ship in the template
  dir and are copied into the job dir with the source (`default_assets(id, params, dir)` lists
  them, including image cells of data params). An empty optional image hides its block
  (`{{#image}}...{{/image}}`, `{{^image}}` for the fallback).
- `data` param: a table, given as a JSON array of objects (or of arrays in column order), JSON
  text, or CSV text with a header row (`,` `;` or tab). At most 500 rows (`max_rows` may lower
  it, `min_rows` sets a floor) and 64 KB. `columns` declare `id`, `type` (`text` (default),
  `number`, `image`, `color`), `max_len`, `required`, `default`; unknown columns are dropped.
  Text cells are single-line, markup-safe and escaped like text params.
- Table blocks: `{{#rows}}...{{/rows}}` repeats once per row with `{{.col}}` (the cell),
  `{{@index}}` (0-based), `{{@n}}` (1-based), `{{#@first}}`, `{{#@last}}`, `{{#.col}}` /
  `{{^.col}}` / `{{#.col=v}}` blocks, and `{{#@changed.col}}` (the first row of a run of equal
  cells: section headers). `{{rows.count}}` is the row count. In expressions: `rows_count`,
  `rows_groups_<col>` (runs), `rows_filled_<col>`, `rows_max_<col>`, `rows_min_<col>`, and inside
  the loop `i`, `n` and `row_<col>` for number cells.
- Expressions and layout logic (stdlib, no `eval`): `{{= expr}}` takes numbers, number params,
  `+ - * / // % ( )`, `< <= > >= == != and or not`, `a if c else b`, `min max round abs floor ceil
  clamp sqrt`; `{{: name = expr}}` binds a number for the rest of its block; `{{#= expr}}...{{/= expr}}`
  shows a block when the expression is non-zero. Also available: `<text|image>_len`,
  `<colour>_luma` (relative luminance 0..1, e.g. light or dark theme), `<enum>_<option>` (1/0).
- `batch` (optional): `{"param": <data param>, "index": <number param>}`. One still per row: the
  caller renders with `index` = 1..rows (`batch_params(manifest, params)` yields the param sets).
- Lint for stills: `agentfx lint --measure` per size must have no errors and no warnings except
  the video reading-time rules that do not apply to a still (`W_TEXT_DENSITY`, `W_WORDY`,
  `W_READ_SIZE`, `I_LINE_LENGTH`). Templates set `lint motion=false` (no motion to judge).
- `preview.webp` or `preview.png` (<= 600 KB) shows the defaults at the main size.

## HTTP API (P1 serves, P2 and the remote MCP use)

Base `/api/v1`. JSON in and out. Auth: `Authorization: Bearer <key>`; anonymous requests get the
free tier, keyed by IP. Errors: `{"error": {"code": "...", "message": "..."}}` with 4xx/5xx.

| Method | Path | Body / query | Response |
|---|---|---|---|
| GET | `/templates` | | `[manifest + {"preview_url"}]` |
| GET | `/templates/{id}` | | manifest + `{"source": "<template.reel>"}` |
| POST | `/instantiate` | `{"template": id, "params": {...}, "size"?: "story"}` (size: stills) | `{"source": "..."}` |
| POST | `/parse` | `{"source"}` or `{"json"}`, `to: "json"|"text"` | `{"json": {...}}` or `{"source": "..."}` |
| POST | `/lint` | `{"source", "format"?}` | `{"diagnostics": [...]}` (CLI JSON unchanged) |
| POST | `/timing` | `{"source", "format"?}` | CLI timing JSON |
| POST | `/frame` | `{"source", "at": "2.5s", "format"?, "width"?}` | `image/png` (preview width default 640) |
| POST | `/sheet` | `{"source", "n"?, "format"?}` | `image/png` |
| POST | `/assets` | multipart `file` (audio/image/svg/font, size caps) | `{"asset_id", "name"}` |
| POST | `/renders` | `{"source", "format"?, "output": "mp4|gif|webp", "quality": "preview|hd"}` | `{"job_id"}` 202 |
| GET | `/renders/{job_id}` | | `{"status": "queued|running|done|error", "progress": 0..1, "url"?, "error"?}` |
| GET | `/files/{job_id}/{name}` | | the rendered file (signed, expires) |

Sources reference uploaded assets by the name returned from `/assets`. The service rejects
absolute paths, `..`, URLs and any file reference not in the job's asset set.

Free tier limits (initial, configurable): source <= 64 KB, duration <= 20 s, 720p, preview
quality, 10 renders/day/IP, frame/sheet 60/min/IP, one running render per client. API keys raise
the limits. Watermark: small "agentfx.dev" corner mark on free renders (renderer-side overlay
appended by the service, not by the user's script).

## Remote MCP (P1)

Streamable HTTP at `/mcp`, same tools and schemas as `mcp/server.py` (reuse its tool table; do not
fork it). `agentfx_render` returns a job URL (and waits up to ~50 s for short renders). Frames and
sheets come back as image content, like the stdio server. Auth as the HTTP API.

## Runtime and security (P1)

- Each request runs the CLI in a throwaway job dir, as an unprivileged user, with CPU time,
  wall clock, memory and output size limits; no network for the renderer process.
- One job queue, N workers (N = cores / threads per render); priority: frame/sheet > render.
- Storage: job dirs on local disk, outputs kept 24 h (free) / 7 d (key), then deleted.
- Deploy target: Docker compose (service + Caddy in front), same image as ghcr.io/agentfxhq/agentfx.
