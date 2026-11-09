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

## HTTP API (P1 serves, P2 and the remote MCP use)

Base `/api/v1`. JSON in and out. Auth: `Authorization: Bearer <key>`; anonymous requests get the
free tier, keyed by IP. Errors: `{"error": {"code": "...", "message": "..."}}` with 4xx/5xx.

| Method | Path | Body / query | Response |
|---|---|---|---|
| GET | `/templates` | | `[manifest + {"preview_url"}]` |
| GET | `/templates/{id}` | | manifest + `{"source": "<template.reel>"}` |
| POST | `/instantiate` | `{"template": id, "params": {...}}` | `{"source": "..."}` |
| POST | `/parse` | `{"source"}` or `{"json"}`, `to: "json"|"text"` | `{"json": {...}}` or `{"source": "..."}` |
| POST | `/lint` | `{"source", "format"?}` | `{"diagnostics": [...]}` (CLI JSON unchanged) |
| POST | `/timing` | `{"source", "format"?}` | CLI timing JSON |
| POST | `/frame` | `{"source", "at": "2.5s", "format"?, "width"?}` | `image/png` (preview width default 640) |
| POST | `/sheet` | `{"source", "n"?, "format"?}` | `image/png` |
| POST | `/assets` | multipart `file` (audio/image/svg/font, size caps) | `{"asset_id", "name"}` |
| POST | `/renders` | `{"source", "format"?, "output": "mp4|gif|webp|png|jpg|pdf", "quality": "preview|hd", "sizes"?: {"name": "WxH"}, "at"?: "0.5s"}` | `{"job_id"}` 202 |
| GET | `/renders/{job_id}` | | `{"status": "queued|running|done|error", "progress": 0..1, "url"?, "files"?: [{"name", "size", "url"}], "zip_url"?, "error"?}` |
| POST | `/batch` | `{"template", "rows": [{params}...] or CSV text, "sizes"?, "output", "at"?}` | `{"job_id"}` 202; status as `/renders`, `url` = the zip |
| GET | `/files/{job_id}/{name}` | | the rendered file (signed, expires) |
| POST | `/preview` | `{"source", "format"?, "size"?}` | `text/html`: the HTML backend (`agentfx html`) for the live preview |
| GET | `/fonts/{name}` | | a font file the preview HTML references (CORS `*`, cacheable) |
| GET | `/catalog` | | `agentfx catalog` JSON unchanged (easings, channels, elements for the editor) |

Additions from P2 (2026-11-09): `/preview`, `/fonts`, `/catalog`, `/batch`; `size: "WxH"` on
`/frame`, `/sheet`, `/lint`, `/timing` and `/preview` (same rule as manifest `sizes`); `sizes`, `at`
and the still outputs `png|jpg|pdf` on `/renders` (stills render the frame at `at`, PDF = one page
with the image at 300 dpi). A job with more than one file sets `files` and `zip_url` (`url` = the
zip); a single file sets `url` (and `files` with one entry).

Live preview (`/preview`): the page is `agentfx html --no-embed` with `file://` references rewritten to
`/api/v1/fonts/<name>` (fonts) and `/api/v1/files/<job>/<name>` (assets). The playground loads it into a
sandboxed iframe (`allow-scripts`, opaque origin, so font responses need `Access-Control-Allow-Origin: *`)
and drives `window.__reel` (`ready`, `seek(t)`, `anims`) by postMessage. Compile errors still return
the page (200), so the preview keeps working while lint shows the errors.

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
