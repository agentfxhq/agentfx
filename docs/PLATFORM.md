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
  "kind": "video",
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

Stills, sizes, data and translations (added 2026-11-09):
```json
{
  "kind": "still",                       // "video" (default) | "still"
  "still_at": "0.5s",                    // stills: the time of the frame (default 0s)
  "sizes": {"instagram-post": "1080x1080", "story": "1080x1920", "a4-flyer": "2480x3508"},
  "params": [
    {"id": "logo", "type": "image", "label": "Logo (png)", "required": false},
    {"id": "rows", "type": "data", "label": "Data", "max_rows": 24,
     "columns": [{"id": "label", "label": "Label", "type": "text"}, {"id": "value", "label": "Value", "type": "number"}],
     "default": [{"label": "Q1", "value": 12}, {"label": "Q2", "value": 18}]}
  ],
  "i18n": {"de": {"title": "...", "description": "...", "params": {"rows": "Daten"}, "options": {"style": {"rgb": "RGB"}}}}
}
```
- `image`: an uploaded asset id like `audio` (PNG for the native renderer).
- `data`: a JSON array of row objects, or CSV text (header row optional; without one, columns are
  positional). Number columns are validated, text cells escaped like `text` params. In the template
  `{{rows.value}}` expands to the column as a list (`12, 18`; text columns quoted: `"Q1", "Q2"`) and
  `{{#rows}}...{{label}}...{{i}}...{{/rows}}` repeats a block per row.
- `sizes`: named pixel sizes (stills, also allowed for videos). A size keeps the script's short side
  for layout, like formats (SPEC §11): the header gets the size's aspect at the base short side (so
  `when portrait|landscape|square` apply and px values keep their meaning), and stills are scaled to
  the exact pixels afterwards (videos are delivered at the layout size).
- `i18n`: optional per-language overrides of title, description, param labels and option labels.

`python3 templates/render_template.py <id> [--set k=v ...] [-o out.reel]` instantiates a template
locally (stdlib only) and is the single implementation of substitution.

### Template language (P3, implemented in `templates/render_template.py`)

Python API for P1: `instantiate(template_id, params, templates_dir) -> str` (raises `ValueError` with
a message fit for the user), `resolve_params(manifest, params) -> dict`, `load_manifest(id, dir)`,
`list_templates(dir)` and `default_assets(id, params, dir) -> {name: path}` (files from the
template's own directory that the source references because the user did not upload a
replacement, e.g. the default track; copy them into the job dir next to the source).

| Tag | Meaning |
|---|---|
| `{{id}}` | the value. `text`: markup-safe (`**`, `{`, `}`, `` ` `` are neutralised, whitespace collapsed) and `"`/`\` escaped, so use it inside quotes |
| `{{id.rgb}}` | a colour without its alpha (`#RRGGBB`), to append an alpha: `{{color1.rgb}}55` |
| `{{= expr}}` | arithmetic over number params and literals: `+ - * / // % ( )`, `min max round abs floor ceil clamp`; also `<text>_len`, `<longtext>_count`, `<longtext>_end` |
| `{{ramp a b n [mirror]}}` | `n` colours from colour `a` to `b` (linear-light mix), comma-separated, for `colors=[...]`; `mirror` is cyclic: a at entry 0, b at entry n/2, back toward a (entry n would be a again), for radial visualizers |
| `{{#id}}...{{/id}}` / `{{^id}}...{{/id}}` | block shown when the param is non-empty / non-zero (`^`: when empty) |
| `{{#id=v}}...{{/id=v}}` / `{{^id=v}}...{{/id=v}}` | block shown when the param equals / does not equal `v` (enums) |
| `{{#lyrics}}...{{/lyrics}}` | for a `longtext` param (here `lyrics`): the block repeats once per non-empty line, with `{{text}}` (escaped line), `{{n}}` (1-based), `{{i}}`, `{{start}}`, `{{end}}`, `{{dur}}` (seconds), `{{#first}}`, `{{#last}}`; in `{{= }}` also `len` (characters of the line) |
| `{{id.count}}` / `{{id.end}}` | a `longtext`: number of lines / end of the last line in seconds |

A section tag alone on its line removes the whole line. Unknown names, unclosed blocks and any
`{{`/`}}` left in the output are errors.

Param types beyond the list above: `image` (uploaded asset id like `audio`; `.png .jpg .jpeg .svg`;
empty when optional and not given, so templates can fall back with `{{^logo}}`; an optional
`"accept": [".png", ".jpg"]` narrows the extensions, e.g. where the renderer reads raster only) and `times`
(per-line times, below). Asset params (`audio`, `image`) take plain file names only
(`[A-Za-z0-9][A-Za-z0-9._-]*`, no paths); a `default` names a file shipped in the template dir.
`longtext` takes `max_lines` (default and cap 64) and `max_line_len` (default 120). Text params
with `required: true` reject an empty value. Numbers are clamped; `step` is a UI hint only.

**Line timing (lyric videos).** A `longtext` param may carry
`"timing": {"bpm": "<number param>", "offset": "<number param>", "beats": "<number param>", "times": "<times param>", "gap": <s>}`
(each value a param id or a literal). Line k starts at `offset + k * beats * 60 / bpm` (the beat
grid) and ends where the next starts (minus `gap`). A `times` param overrides the grid line by
line: a JSON list or a string of starts (`"0, 2.4, 5.1"`) or start-end pairs (`[[0, 2.2], [2.4, 4.9]]`,
`"0-2.2, 2.4-4.9"`), non-decreasing seconds. Lines past the given times continue on the beat grid
from the next beat after the last given line. This is the hook for forced alignment: a later
service step can run `tools/transcribe.py` on the uploaded song, match the words to the lyric
lines and fill `times` with `[start, end]` pairs; the template does not change.

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
  `facebook-post` 1200x630, `facebook-cover` 1640x624, `linkedin-post` 1200x628,
  `linkedin-cover` 1584x396, `x-post` 1600x900, `x-header` 1500x500, `youtube-thumbnail` 1280x720,
  `youtube-banner` 2560x1440, `og-image` 1200x630, `pinterest-pin` 1000x1500, `full-hd` 1920x1080,
  `leaderboard` 728x90, `medium-rectangle` 300x250, `skyscraper` 160x600, `wide-skyscraper`
  300x600, `billboard` 970x250, `mobile-banner` 320x100, `a4-flyer` 2480x3508, `a5-flyer`
  1748x2480, `a3-poster` 3508x4960, `a4-landscape` 3508x2480, `letter-flyer` 2550x3300,
  `letter-landscape` 3300x2550 (print sizes at 300 dpi). The table lives in
  `templates/render_template.py` (`SIZE_PRESETS`). A template must look right at every size it
  declares; any other `WxH` (16..8192 px a side, at most 36 MP) is accepted best effort. The renderer rounds odd dimensions up to even (`lower::format_size`), so presets are even and an odd custom WxH comes out 1 px larger.
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
  them, including image cells of data params; params the caller supplied are left out, their
  names refer to uploads). An empty optional image hides its block
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
  `rows_groups_<col>` (runs), `rows_maxrun_<col>` (longest run), `rows_maxlen_<col>`,
  `rows_filled_<col>`, `rows_max_<col>`, `rows_min_<col>`, and inside the loop `i`, `n`,
  `row_<col>` (number cells), `row_<col>_len` and `row_<col>_num` (first number in a text cell,
  e.g. the sign of a delta). `{{.value|en}}` / `{{value|de}}` / `{{value|<enum param>}}` prints
  a number with thousands separators.
- Expressions and layout logic (stdlib, no `eval`): `{{= expr}}` takes numbers, number params,
  `+ - * / // % ( )`, `< <= > >= == != and or not`, `a if c else b`, `min max round abs floor ceil
  clamp sqrt`; `{{: name = expr}}` binds a number for the rest of its block; `{{#= expr}}...{{/= expr}}`
  shows a block when the expression is non-zero. Also available: `<text|image>_len`,
  `<colour>_luma` (relative luminance 0..1, e.g. light or dark theme), `<enum>_<option>` (1/0).
- `batch` (optional): `{"param": <data param>, "index": <number param>}`. One still per row: the
  caller renders with `index` = 1..rows (`batch_params(manifest, params)` yields the param sets).
- Lint for stills: `agentfx lint --measure` per size must have no errors and no warnings except
  the video reading-time rules that do not apply to a still (`W_TEXT_DENSITY`, `W_WORDY`,
  `W_READ_SIZE`, `I_LINE_LENGTH`, and the video taste rule `W_SCALE_FILL`). Templates set `lint motion=false` (no motion to judge).
- `preview.webp` or `preview.png` (<= 600 KB) shows the defaults at the main size.

## HTTP API (P1 serves, P2 and the remote MCP use)

Base `/api/v1`. JSON in and out. Auth: `Authorization: Bearer <key>`; anonymous requests get the
free tier, keyed by IP. Errors: `{"error": {"code": "...", "message": "..."}}` with 4xx/5xx.

| Method | Path | Body / query | Response |
|---|---|---|---|
| GET | `/templates` | | `[manifest + {"kind", "preview_url"}]` (`preview_url` null without preview.webp) |
| GET | `/templates/{id}` | | manifest + `{"kind", "source": "<template.reel>", "preview_url"}` |
| GET | `/templates/{id}/preview.webp` | | the preview (public, cached) |
| POST | `/instantiate` | `{"template": id, "params": {...}, "size"?: preset}` | `{"source": "..."}` |
| POST | `/parse` | `{"source"}` or `{"json"}`, `to: "json"|"text"` | `{"json": {...}}` or `{"source": "..."}` (+ `"diagnostics"` when it does not parse cleanly) |
| POST | `/lint` | `{"source", "format"?, "measure"?: true}` | `{"diagnostics": [...], "summary": "lint: ..."}` (diagnostics = CLI JSON unchanged) |
| POST | `/timing` | `{"source", "format"?}` | CLI timing JSON |
| POST | `/frame` | `{"source", "at": "2.5s", "format"?, "width"?}` | `image/png` (preview width default 640) |
| POST | `/sheet` | `{"source", "n"?, "format"?}` | `image/png` |
| POST | `/assets` | multipart `file` (audio/image/svg/font, size caps) | `{"asset_id", "name"}` |
| POST | `/renders` | `{"source", "format"?, "output": "mp4|gif|webp|png|jpg|pdf", "quality": "preview|hd", "at"?}` or `{"template", "params", "sizes"?: [preset], "output"?, "quality"?}` | `{"job_id", "status_url", ...}` 202 |
| POST | `/batch` | `{"template", "rows": [{params}, ...], "params"?: {shared}, "sizes"?, "output"?}` | `{"job_id", "status_url", ...}` 202, the job yields one zip |
| GET | `/renders/{job_id}` | | `{"status": "queued|running|done|error", "progress": 0..1, "kind", "output", "url"?, "expires_at"?, "files"?: [{"name", "size"?, "row"?, "url"}], "failures"?, "error"?}` |
| GET | `/files/{job_id}/{name}` | `?exp=&sig=` | the rendered file (HMAC signed URL, expires with the job) |
| POST | `/preview` | `{"source", "format"?, "size"?}` | `text/html`: the HTML backend (`agentfx html`) for the live preview |
| GET | `/fonts/{name}` | | a font file the preview HTML references (CORS `*`, cacheable) |
| GET | `/catalog` | | `agentfx catalog` JSON unchanged (easings, channels, elements for the editor) |

Additions from P2 (2026-11-09, reconciled 2026-11-10): `/preview`, `/fonts`, `/catalog`; `size: "WxH"`
on `/frame`, `/sheet`, `/lint`, `/timing` and `/preview` (sets the `video` header size, like a
template size preset). `GET /templates` and `GET /templates/{id}` add `size_px: {name: "WxH"}` for
the manifest's `sizes` list, so clients can lay out size pickers without the preset table. Template
renders go through `{"template", "params", "sizes"}` (the service instantiates each size, so
size-aware still templates get their `{{W}}`/`{{S}}` scope); a raw `source` render has no `sizes`.

Live preview (`/preview`): the page is `agentfx html --no-embed` with `file://` references rewritten to
`/api/v1/fonts/<name>` (fonts) and `/api/v1/files/<job>/<name>` (assets). The playground loads it into a
sandboxed iframe (`allow-scripts`, opaque origin, so font responses need `Access-Control-Allow-Origin: *`)
and drives `window.__reel` (`ready`, `seek(t)`, `anims`) by postMessage. Compile errors still return
the page (200), so the preview keeps working while lint shows the errors.

Renders: a raw `source` with a still output (png/jpg/pdf) renders the frame at `at` (default
`0s`). A template job renders one file per size (default: the template's own size); `url` is
the first file, `files` lists all. Outputs: stills png (default) / jpg / pdf (the raster at its
pixel size wrapped in a one-page PDF, page size at 300 dpi), videos mp4 (default) / gif / webp.
A batch renders rows x sizes and zips them (`url` = the zip; rows that fail are listed in
`failures` and in `failures.json` in the zip; the job fails only when every row fails). Each
row's params override the shared `params`. The first file is checked synchronously, so most
bad requests get a 4xx instead of a failed job. Errors carry `code`: `invalid_request`,
`invalid_source`, `forbidden_reference`, `unknown_asset`, `parse_error`, `compile_error`,
`invalid_params`, `limit_exceeded`, `rate_limited`, `too_many_renders`, `too_large`,
`unsupported_type`, `unauthorized`, `not_found`, `busy`, `render_error`, `internal`.

Sources reference uploaded assets by the name returned from `/assets` (`a<24 hex>.<ext>`; the
name is the capability, assets are not bound to a client). The service rejects absolute paths,
`..`, URLs and any file reference not in the job's asset set (uploads + template default
assets). Uploads are typed by their content (magic bytes), never by name or content type:
PNG/JPEG (<= 8000 px per side, <= 40 MP), SVG (UTF-8, markup checked like inline svg), MP3/WAV/
OGG/FLAC/M4A, TTF/OTF/WOFF/WOFF2; caps 8 MB image, 512 KB svg, 25 MB audio, 8 MB font.

Free tier limits (initial, configurable via `AFX_FREE_*`, keys via `AFX_KEY_*` and per-key
overrides): source <= 64 KB, duration <= 20 s, 720p (a video source whose header size has a
short side over 720 is rejected with `limit_exceeded`; templates meant for the free tier
default to 1280x720 or offer such a size preset), preview quality, 10 render jobs/day/IP, 200
output files/day, 100 files/job, 3 videos/job, 50 batch rows, stills <= 12 MP, frame/sheet
60/min/IP (lint/timing/parse/instantiate 240/min), one queued-or-running render job per client.
Watermark: small "agentfx.dev" corner mark on free renders and stills, burned in by an ffmpeg
pass over the finished output (the user's script cannot reach it).

## Remote MCP (P1)

Streamable HTTP at `/mcp` (stateless, JSON responses, protocol 2025-06-18; also answers
2025-11-25/2025-03-26/2024-11-05 clients), same tools and schemas as `mcp/server.py` (its tool
table and handlers are imported, not forked) with two hosted-only differences: the filesystem
arguments `file`, `assets_dir`, `out`, `out_dir` are removed from the schemas (`source` is
required instead), and `agentfx_critique` is not offered (it reads local files and spends the
operator's Anthropic key). `agentfx_render` and `agentfx_render_template` return the job status
and URLs (and wait up to ~50 s; `structuredContent` = the `/renders/{id}` JSON + `status_url`);
template stills come back as image content too. `agentfx_list_templates` lists the manifests.
Frames and sheets come back as image content, like the stdio server. Auth as the HTTP API.

## Runtime and security (P1)

- Each request runs the CLI in a throwaway job dir, as an unprivileged user, with CPU time,
  wall clock, memory and output size limits; no network for the renderer process.
  Implementation (service/README.md "Security"): uid per worker slot, prlimit (AS, CPU, FSIZE,
  NOFILE, NPROC, CORE), wall clock + job dir size watchdog killing the process group, clean
  environment (`AGENTFX_TTS=none`, own HOME/cache), iptables owner rule for the sandbox uid
  range, static source checks before compiling.
- One job queue, N workers (N = cores / threads per render); priority: frame/sheet > render.
- Storage: job dirs on local disk, outputs kept 24 h (free) / 7 d (key), then deleted.
- Deploy target: Docker compose (service + Caddy in front), same image as ghcr.io/agentfxhq/agentfx.
