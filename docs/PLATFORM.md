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

Still graphics (added 2026-11-09): `"kind": "video"` (default) or `"still"`. A still renders one
frame with `agentfx frame --at <"still_at", default "0s">`. Any manifest may declare named size
presets, `"sizes": {"instagram-post": "1080x1080", "story": "1080x1920", "a4-flyer": "2480x3508"}`.
The service (and `agentfx_render_template` in the stdio server) applies a preset by setting
`size=WxH` on the instantiated source's `video` header line (`set_video_size` in
`mcp/server.py`), so templates are written responsive (`when portrait`/`landscape`/`square`
blocks, %-based layout) and must keep their `video` header on the first non-comment line.

Param types `image` (like `audio`: an uploaded asset id or name, or a file of the template) and
`data`: a table, a JSON array of objects or CSV text with a header row, at most 500 rows / 64 KB
(the service checks the size and passes the value through; `templates/render_template.py`
expands it, e.g. with a `{{#rows}}...{{/rows}}` block whose body sees the row's columns).
Files in `templates/<id>/` with a media extension (mp3/wav/ogg/flac/m4a/png/jpg/svg/ttf/otf/
woff/woff2, not `preview.*`) are the template's default assets; sources may reference them by
file name like uploaded assets.
Substitution is plain text: `{{id}}` is replaced by the value. The substituter escapes `"` and `\`
inside text values and rejects newlines in `text` params; `longtext` is only used where the template
expands it (P3 documents how, e.g. a `{{#lines}}...{{/lines}}` block, implemented in
`templates/render_template.py`, which P1 imports). Colours must match `^#[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$`,
numbers are clamped to min/max, enums must be one of options. `audio` is an uploaded asset id
that the service maps to a file name in the job dir.

`python3 templates/render_template.py <id> [--set k=v ...] [-o out.reel]` instantiates a template
locally (stdlib only) and is the single implementation of substitution.

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
