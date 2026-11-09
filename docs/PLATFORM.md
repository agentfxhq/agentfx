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
| `{{ramp a b n [mirror]}}` | `n` colours from colour `a` to `b` (linear-light mix), comma-separated, for `colors=[...]`; `mirror` runs a -> b -> a |
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
