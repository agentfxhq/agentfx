#!/usr/bin/env python3
"""AgentFX MCP server (stdio, JSON-RPC 2.0, MCP 2024-11-05 subset; Python stdlib only).

Exposes the agent loop of the `agentfx` CLI as tools. The CLI's JSON diagnostics are passed
through unchanged.

  agentfx_spec          the Reel Script spec (compact overview by default; section= for detail)
  agentfx_catalog       capability catalog: every element kind, verb, effect, status (JSON)
  agentfx_lint          lint a .reel source (static, or measured in every format)
  agentfx_timing        resolved timeline (scenes, sentences, word anchors)
  agentfx_frame         one frame PNG at an anchor or time, returned as an image
  agentfx_sheet         contact sheet PNG, returned as an image
  agentfx_storyboard    hand-drawn storyboard PNG (one sketch panel per shot, arrows, notes) + shot list
  agentfx_render        render the video (MP4, or GIF/WebP/alpha by extension) to a file
  agentfx_design_guide  the design layer guide and the list of lenses
  agentfx_lens          one lens: essay (LENS.md) and tokens (design.reel)
  agentfx_critique      optional vision critique of a sheet or video (needs ANTHROPIC_API_KEY)
  agentfx_list_templates  the templates (templates/<id>/manifest.json): params, kind, sizes
  agentfx_render_template render a template with params, one file per size (still: png/jpg/pdf,
                        video: mp4/gif/webp) into `out_dir`

Sources are passed inline (`source`) or as a path (`file`). Inline sources are written to a
temporary file in `assets_dir` (so relative asset paths resolve there), else in the system
temp directory.

Register with Claude Code:
  claude mcp add agentfx -- python3 /path/to/agentfx/mcp/server.py

Environment:
  AGENTFX_BIN   the agentfx binary (default: `agentfx` on PATH, else <repo>/target/release/agentfx)
  AGENTFX_HOME  data directory (default: the repository this file lives in, or an install's
                share/agentfx when this file was installed there)
"""
import base64
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
HOME = os.environ.get("AGENTFX_HOME") or os.environ.get("REEL_HOME") or os.path.dirname(HERE)


def find_bin():
    for c in (os.environ.get("AGENTFX_BIN"), os.environ.get("REEL_BIN"),
              os.path.join(os.path.dirname(os.path.dirname(HOME)), "bin", "agentfx"),
              shutil.which("agentfx"), os.path.join(HOME, "target", "release", "agentfx")):
        if c and os.path.isfile(c) and os.access(c, os.X_OK):
            return c
    raise RuntimeError("agentfx binary not found: build it (cargo build --release), run tools/install.sh, or set AGENTFX_BIN")


SRC = {"source": {"type": "string", "description": "Reel Script text (alternative to file)"},
       "file": {"type": "string", "description": "path to a .reel file"},
       "assets_dir": {"type": "string", "description": "directory relative asset paths resolve against (inline source)"}}
FMT = {"type": "string", "description": "16:9, 9:16, 1:1 or 4:5"}
TOOLS = [
    {"name": "agentfx_spec", "description": "Reel Script spec. Returns a compact overview by default (section ids + essentials). Pass section= for one section in full, or section=\"all\" for the entire spec.",
     "inputSchema": {"type": "object", "properties": {"section": {"type": "string", "description": "section number/id (e.g. '3', '7', 'values', 'story'), or 'all' for the full spec"}}}},
    {"name": "agentfx_catalog", "description": "Capability catalog (every element kind, verb, effect, channel, status). The source of truth for what AgentFX can do.",
     "inputSchema": {"type": "object", "properties": {"md": {"type": "boolean", "description": "Markdown instead of JSON"}}}},
    {"name": "agentfx_lint", "description": "Lint a Reel Script video. JSON diagnostics with fix hints. measure=true checks real layout in every format (16:9 and 9:16, or the formats= list of the video). Fix every error; warnings about size (W_SCALE_FILL, W_SMALL_MOTION), length (W_SCENE_SHORT), number format (W_NUMBER_FORMAT) and the end (W_STATIC_END) are what users complain about first.",
     "inputSchema": {"type": "object", "properties": {**SRC, "measure": {"type": "boolean", "default": True}, "no_tts": {"type": "boolean"}}}},
    {"name": "agentfx_timing", "description": "Resolved timeline JSON (scenes, sentences, anchors).",
     "inputSchema": {"type": "object", "properties": {**SRC, "no_tts": {"type": "boolean"}}}},
    {"name": "agentfx_frame", "description": "One frame PNG at an anchor name, scene:anchor or seconds. Look at it to check layout.",
     "inputSchema": {"type": "object", "properties": {**SRC, "at": {"type": "string"}, "format": FMT}, "required": ["at"]}},
    {"name": "agentfx_sheet", "description": "Contact sheet PNG of the video (n evenly spaced frames, or one per event).",
     "inputSchema": {"type": "object", "properties": {**SRC, "n": {"type": "integer", "default": 12}, "events": {"type": "boolean"}, "format": FMT, "cols": {"type": "integer"}}}},
    {"name": "agentfx_storyboard", "description": "Director's storyboard PNG: one hand-drawn sketch panel per shot (layers as outlines, motion arrows from the tracks, camera moves), time range, transition and a note per shot (`note=` / `@t note \"...\"`, else automatic). Also returns the shot list as JSON.",
     "inputSchema": {"type": "object", "properties": {**SRC, "format": FMT, "cols": {"type": "integer"}, "seed": {"type": "integer"}}}},
    {"name": "agentfx_render", "description": "Render the video to `out` (.mp4; .gif, .webp, .mov or .webm by extension). Blocks until the file is written (seconds to several minutes; wait for it, do not start a second render meanwhile) and returns the output path and the renderer's summary. Render once at the end, one call per requested format (format=\"9:16\" for vertical); check frames and sheets before.",
     "inputSchema": {"type": "object", "properties": {**SRC, "out": {"type": "string", "description": "output file path"}, "format": FMT,
                                                      "no_tts": {"type": "boolean"}, "alpha": {"type": "boolean"}}, "required": ["out"]}},
    {"name": "agentfx_design_guide", "description": "Design layer guide: lenses, design spec format, per-step detail components, lint rules.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "agentfx_lens", "description": "A named look (lens): essay with numbers, negatives, motion tendencies, and its design.reel tokens.",
     "inputSchema": {"type": "object", "properties": {"name": {"type": "string", "description": "a lens name from agentfx_design_guide"}}, "required": ["name"]}},
    {"name": "agentfx_critique", "description": "Optional vision critique of a sheet/video against the design rubric (needs ANTHROPIC_API_KEY in the server's environment).",
     "inputSchema": {"type": "object", "properties": {"file": {"type": "string", "description": "sheet .png or .mp4"}, "brief": {"type": "string"},
                                                      "model": {"type": "string", "description": "haiku (default) or sonnet"}}, "required": ["file"]}},
    {"name": "agentfx_list_templates", "description": "List the templates: id, title, description, kind (video|still), params (types, defaults, limits), sizes (named presets such as instagram-post=1080x1080) and formats.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "agentfx_render_template", "description": "Render a template with params (see agentfx_list_templates): one file per size. Stills: png (default), jpg or pdf; videos: mp4 (default), gif or webp. `data` params take a JSON array of objects or CSV text; image/audio params an asset name.",
     "inputSchema": {"type": "object", "properties": {
         "template": {"type": "string", "description": "template id"},
         "params": {"type": "object", "description": "param id -> value (omitted params use their defaults)"},
         "sizes": {"type": "array", "items": {"type": "string"}, "description": "preset names from the manifest's sizes (default: the template's own size)"},
         "output": {"type": "string", "enum": ["png", "jpg", "pdf", "mp4", "gif", "webp"]},
         "out_dir": {"type": "string", "description": "directory for the files (default: a new temporary directory)"},
         "assets_dir": {"type": "string", "description": "directory image/audio param files are taken from"}},
         "required": ["template"]}},
]

STILL_OUTPUTS = ("png", "jpg", "pdf")
VIDEO_OUTPUTS = ("mp4", "gif", "webp")
SIZE_RE = re.compile(r"^\s*(\d{2,5})\s*[xX]\s*(\d{2,5})\s*$")


def set_video_size(source, size):
    """Set `size=WxH` on the `video` header line. Templates are written responsive (`when`
    blocks, format rules); a manifest's size presets are applied this way."""
    m = SIZE_RE.match(str(size))
    if not m:
        raise ValueError(f"size must look like 1080x1920, got {size!r}")
    w, h = int(m.group(1)), int(m.group(2))
    lines = source.split("\n")
    for i, line in enumerate(lines):
        st = line.lstrip()
        if not st or st.startswith("//"):
            continue
        if not (st.startswith("video ") or st == "video"):
            break
        if re.search(r"(^|\s)size=\S+", line):
            lines[i] = re.sub(r"(^|\s)size=\S+", lambda mm: f"{mm.group(1)}size={w}x{h}", line, count=1)
        else:
            lines[i] = line.rstrip() + f" size={w}x{h}"
        return "\n".join(lines)
    raise ValueError("the source has no `video` header line")


def jpeg_to_pdf(jpeg_bytes, width, height, dpi=300):
    """One-page PDF holding a JPEG of width x height px (page size at `dpi`). Stdlib only."""
    pw, ph = width * 72.0 / dpi, height * 72.0 / dpi
    content = f"q {pw:.2f} 0 0 {ph:.2f} 0 0 cm /Im0 Do Q".encode()
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            (f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2f} {ph:.2f}] "
             f"/Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>").encode(),
            (f"<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB "
             f"/BitsPerComponent 8 /Filter /DCTDecode /Length {len(jpeg_bytes)} >>\nstream\n").encode()
            + jpeg_bytes + b"\nendstream",
            f"<< /Length {len(content)} >>\nstream\n".encode() + content + b"\nendstream"]
    out, offsets = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n"), []
    for i, o in enumerate(objs, 1):
        offsets.append(len(out))
        out += f"{i} 0 obj\n".encode() + o + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
    out += b"".join(f"{off:010d} 00000 n \n".encode() for off in offsets)
    out += f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return bytes(out)


def template_module(root=None):
    """templates/render_template.py, the single implementation of substitution."""
    import importlib.util
    root = root or os.path.join(HOME, "templates")
    spec = importlib.util.spec_from_file_location("render_template", os.path.join(root, "render_template.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


TEMPLATE_KEYS = ("id", "title", "description", "kind", "tags", "formats", "duration_s", "sizes", "still_at", "params")


def template_list(root=None):
    root = root or os.path.join(HOME, "templates")
    try:
        rt = template_module(root)
    except OSError:
        return []
    out = []
    for tid in rt.list_templates(root):
        try:
            m = rt.load_manifest(tid, root)
        except (ValueError, OSError):
            continue
        d = {k: m[k] for k in TEMPLATE_KEYS if k in m}
        d["kind"] = m.get("kind", "video")
        out.append(d)
    return out


def template_plan(manifest, sizes, output, rt=None):
    """Validated (kind, output, [(label, "WxH" or None)]) for a template render. The manifest's
    `sizes` is a list of preset names (templates/render_template.py SIZE_PRESETS) or a legacy
    {name: "WxH"} map. Instantiate each entry with `size=label` (None: the template's own size)."""
    kind = manifest.get("kind", "video")
    if kind not in ("video", "still"):
        raise ValueError(f"unknown template kind {kind!r}")
    allowed = STILL_OUTPUTS if kind == "still" else VIDEO_OUTPUTS
    output = output or allowed[0]
    if output not in allowed:
        raise ValueError(f"a {kind} template renders to {', '.join(allowed)}, not {output}")
    declared = manifest.get("sizes") or []
    if not sizes:
        return kind, output, [("default", None)]
    if not isinstance(sizes, list) or len(set(map(str, sizes))) != len(sizes):
        raise ValueError("sizes must be a list of distinct preset names")
    plan = []
    for s in sizes:
        if s not in declared:
            raise ValueError(f"unknown size {s!r} (sizes: {', '.join(declared) or 'none'})")
        if isinstance(declared, dict):
            px = str(declared[s])
        else:
            if rt is None:
                raise ValueError("size presets need templates/render_template.py")
            w, h = rt.size_px(s)
            px = f"{w}x{h}"
        if not SIZE_RE.match(px):
            raise ValueError(f"manifest size {s!r} is not WxH")
        plan.append((s, px))
    return kind, output, plan


# -- spec section support --

def _parse_spec_sections(text):
    """Split SPEC.md into sections by ## headings. Returns [(id, title, body), ...]."""
    sections = []
    current_id = None
    current_title = None
    current_lines = []
    for line in text.splitlines():
        m = re.match(r'^## +(\d+)\.\s+(.*)', line)
        if not m:
            m = re.match(r'^## +(.*)', line)
        if m:
            if current_id is not None:
                sections.append((current_id, current_title, "\n".join(current_lines)))
            groups = m.groups()
            if len(groups) == 2:
                current_id = groups[0]
                current_title = groups[1]
            else:
                current_id = groups[0].split()[0].rstrip(".")
                current_title = groups[0]
            current_lines = [line]
        else:
            current_lines.append(line)
    if current_id is not None:
        sections.append((current_id, current_title, "\n".join(current_lines)))
    return sections


def _spec_overview(text):
    """Compact overview: section ids, the first 5 sections in full (enough to write a script),
    and one-line summaries of the rest."""
    sections = _parse_spec_sections(text)
    # sections 1-7 are the essentials to write a first script
    essential_ids = {"1", "2", "3", "4", "5", "6", "7"}
    parts = ["# Reel Script spec -- compact overview\n"]
    parts.append("Sections: " + ", ".join(f"{s[0]}. {s[1]}" for s in sections))
    parts.append("\nUse agentfx_spec(section=\"<id>\") for any section in full, or section=\"all\" for the entire spec.\n")
    parts.append("--- Essential sections (enough to write a first script) ---\n")
    for sid, title, body in sections:
        if sid in essential_ids:
            parts.append(body.strip())
            parts.append("")
    parts.append("--- Remaining sections (request by id for full text) ---\n")
    for sid, title, body in sections:
        if sid not in essential_ids:
            # first non-empty non-heading line as summary
            summary_lines = [l.strip() for l in body.splitlines() if l.strip() and not l.startswith("#")]
            summary = summary_lines[0][:200] if summary_lines else ""
            parts.append(f"  {sid}. {title}: {summary}")
    return "\n".join(parts)


def _spec_section(text, query):
    """Return one section by number or keyword match."""
    sections = _parse_spec_sections(text)
    q = query.strip().lower().rstrip(".")
    # exact id match
    for sid, title, body in sections:
        if sid.lower() == q:
            return body
    # keyword match in title
    for sid, title, body in sections:
        if q in title.lower():
            return body
    available = ", ".join(f"{s[0]}. {s[1]}" for s in sections)
    return f"Section '{query}' not found. Available: {available}"


def src_path(args, workdir=None):
    if args.get("file"):
        return os.path.abspath(args["file"]), None
    base = workdir or args.get("assets_dir") or tempfile.gettempdir()
    fd, p = tempfile.mkstemp(suffix=".reel", dir=base)
    with os.fdopen(fd, "w") as f:
        f.write(args.get("source", ""))
    return p, p


def run(cmd, cwd=None):
    r = subprocess.run([find_bin()] + cmd, capture_output=True, text=True, cwd=cwd or HOME)
    return r.returncode, r.stdout, r.stderr


def read(*parts):
    with open(os.path.join(HOME, *parts), encoding="utf-8") as f:
        return f.read()


def _tmpdir():
    """Return the temp directory, included in failure messages."""
    return tempfile.gettempdir()


def _rm(path):
    try:
        os.remove(path)
    except OSError:
        pass


def call(name, args, run_cli=None, workdir=None):
    """Run one tool. `run_cli(cmd, cwd) -> (code, stdout, stderr)` replaces the local CLI runner
    and `workdir` holds the inline source and temporary PNGs (the hosted service passes a
    sandboxed runner and a throwaway job dir; see service/agentfx_service/mcp_http.py)."""
    run_cli = run_cli or run
    if name == "agentfx_spec":
        spec_text = read("docs", "SPEC.md")
        section = args.get("section")
        if section and section.lower() == "all":
            return [{"type": "text", "text": spec_text}]
        if section:
            return [{"type": "text", "text": _spec_section(spec_text, section)}]
        return [{"type": "text", "text": _spec_overview(spec_text)}]

    if name == "agentfx_catalog":
        return [{"type": "text", "text": run_cli(["catalog"] + (["--md"] if args.get("md") else []))[1]}]

    if name == "agentfx_design_guide":
        root = os.path.join(HOME, "design", "lenses")
        lenses = sorted(d for d in os.listdir(root) if os.path.isdir(os.path.join(root, d)))
        return [{"type": "text", "text": read("design", "AUTHORING.md") + "\n\nLenses: " + ", ".join(lenses)}]

    if name == "agentfx_lens":
        d = os.path.basename(args["name"])
        return [{"type": "text", "text": read("design", "lenses", d, "LENS.md") + "\n\n" + read("design", "lenses", d, "design.reel")}]

    if name == "agentfx_list_templates":
        return [{"type": "text", "text": json.dumps(template_list(), indent=1)}]

    if name == "agentfx_render_template":
        return _render_template_local(args, run_cli)

    if name == "agentfx_critique":
        file_path = args.get("file", "")
        if not file_path:
            return _tool_error("file is required")
        abs_path = os.path.abspath(file_path)
        if not os.path.isfile(abs_path):
            return _tool_error(f"file not found: {abs_path}")
        py = os.path.join(HOME, ".venv", "bin", "python")
        py = py if os.path.exists(py) else sys.executable
        argv = [abs_path, "--tag", "mcp"] + (["--brief", args["brief"]] if args.get("brief") else []) + (["--model", args["model"]] if args.get("model") else [])
        r = subprocess.run([py, os.path.join(HOME, "tools", "critique.py")] + argv, capture_output=True, text=True, cwd=HOME)
        if r.returncode != 0 and not r.stdout.strip():
            return _tool_error(f"critique failed (exit {r.returncode}): {r.stderr[-3000:]}")
        return [{"type": "text", "text": r.stdout or r.stderr[-3000:]}]

    path, tmp = src_path(args, workdir)
    cwd = os.path.dirname(path)
    try:
        if name == "agentfx_lint":
            cmd = ["lint", path] + (["--measure", "--all-formats"] if args.get("measure", True) else []) + (["--no-tts"] if args.get("no_tts") else [])
            code, out, err = run_cli(cmd, cwd)
            # lint diagnostics are data, not failures -- return them even on non-zero exit
            # (exit 1 = has errors, which is normal lint output)
            return [{"type": "text", "text": out + "\n" + (err.strip().splitlines() or [""])[-1]}]

        if name == "agentfx_timing":
            code, out, err = run_cli(["timing", path] + (["--no-tts"] if args.get("no_tts") else []), cwd)
            if code != 0 and not out.strip():
                return _tool_error(f"timing failed (exit {code}, tmp: {_tmpdir()}): {err[-3000:]}")
            return [{"type": "text", "text": out or err}]

        if name == "agentfx_render":
            out_path = os.path.abspath(args["out"])
            cmd = ["render", path, "-o", out_path] + (["--format", args["format"]] if args.get("format") else []) \
                + (["--no-tts"] if args.get("no_tts") else []) + (["--alpha"] if args.get("alpha") else [])
            code, out, err = run_cli(cmd, cwd)
            if code != 0 or not os.path.exists(out_path):
                return _tool_error(f"render failed (exit {code}, work: {cwd}, tmp: {_tmpdir()}):\n{err[-4000:]}")
            return [{"type": "text", "text": f"rendered {out_path} ({os.path.getsize(out_path)} bytes)\n" + (out or err)[-2000:]}]

        if name == "agentfx_storyboard":
            fd, png = tempfile.mkstemp(suffix=".png", dir=workdir)
            os.close(fd)
            cmd = ["storyboard", path, "-o", png] + (["--format", args["format"]] if args.get("format") else []) \
                + (["--cols", str(args["cols"])] if args.get("cols") else []) + (["--seed", str(args["seed"])] if args.get("seed") is not None else [])
            code, out, err = run_cli(cmd, cwd)
            try:
                if code != 0 or os.path.getsize(png) == 0:
                    return _tool_error(f"storyboard failed (exit {code}, work: {cwd}, tmp: {_tmpdir()}):\n{err[-4000:]}")
                with open(png, "rb") as f:
                    data = base64.b64encode(f.read()).decode()
            finally:
                _rm(png)
            return [{"type": "image", "data": data, "mimeType": "image/png"}, {"type": "text", "text": out[-4000:] + err[-1000:]}]

        if name in ("agentfx_sheet", "agentfx_frame"):
            fd, png = tempfile.mkstemp(suffix=".png", dir=workdir)
            os.close(fd)
            cmd = ["sheet" if name == "agentfx_sheet" else "frame", path, "-o", png]
            if name == "agentfx_sheet":
                cmd += ["--events"] if args.get("events") else ["--n", str(args.get("n", 12))]
                if args.get("cols"):
                    cmd += ["--cols", str(args["cols"])]
            else:
                cmd += ["--at", str(args["at"])]
            if args.get("format"):
                cmd += ["--format", args["format"]]
            code, out, err = run_cli(cmd, cwd)
            try:
                if code != 0 or os.path.getsize(png) == 0:
                    return _tool_error(f"{'sheet' if name == 'agentfx_sheet' else 'frame'} failed (exit {code}, work: {cwd}, tmp: {_tmpdir()}):\n{err[-4000:]}")
                with open(png, "rb") as f:
                    data = base64.b64encode(f.read()).decode()
            finally:
                _rm(png)
            return [{"type": "image", "data": data, "mimeType": "image/png"}, {"type": "text", "text": err[-2000:]}]
    finally:
        if tmp:
            _rm(tmp)
    raise ValueError(f"unknown tool {name}")


def _render_template_local(args, run_cli):
    """stdio: instantiate, copy assets, render each size into out_dir."""
    root = os.path.join(HOME, "templates")
    rt = template_module(root)
    tid = args.get("template", "")
    params = args.get("params") or {}
    try:
        manifest = rt.load_manifest(tid, root)
        kind, output, plan = template_plan(manifest, args.get("sizes"), args.get("output"), rt)
        srcs = {label: rt.instantiate(tid, params, root, size=label if size else None) for label, size in plan}
    except ValueError as e:
        return _tool_error(str(e))
    out_dir = os.path.abspath(args.get("out_dir") or tempfile.mkdtemp(prefix=f"agentfx-{tid}-"))
    os.makedirs(out_dir, exist_ok=True)
    work = tempfile.mkdtemp(prefix="agentfx-tpl-")
    tdir = os.path.join(root, tid)
    for n in os.listdir(tdir):  # template default assets (demo song, images)
        if os.path.isfile(os.path.join(tdir, n)) and not n.startswith(("template.", "manifest.", "README", "preview.")):
            shutil.copyfile(os.path.join(tdir, n), os.path.join(work, n))
    if args.get("assets_dir"):
        for v in params.values():
            if isinstance(v, str) and v and os.path.basename(v) == v and os.path.isfile(os.path.join(args["assets_dir"], v)):
                shutil.copyfile(os.path.join(args["assets_dir"], v), os.path.join(work, v))
    made, content = [], []
    try:
        for label, size in plan:
            p = os.path.join(work, f"{label}.reel")
            with open(p, "w", encoding="utf-8") as f:
                f.write(set_video_size(srcs[label], size) if size else srcs[label])
            base = os.path.join(out_dir, f"{tid}-{label}")
            if kind == "still":
                png = base + ".png"
                code, out, err = run_cli(["frame", p, "--at", str(manifest.get("still_at", "0s")), "-o", png], work)
                if code != 0 or not os.path.isfile(png):
                    return _tool_error(f"{label}: frame failed (exit {code}):\n{err[-3000:]}")
                final = still_convert(png, output)
            else:
                final = f"{base}.{output}"
                code, out, err = run_cli(["render", p, "-o", final, "--no-tts"], work)
                if code != 0 or not os.path.isfile(final):
                    return _tool_error(f"{label}: render failed (exit {code}):\n{err[-3000:]}")
            made.append(final)
        if kind == "still" and output == "png" and made:
            with open(made[0], "rb") as f:
                content.append({"type": "image", "data": base64.b64encode(f.read()).decode(), "mimeType": "image/png"})
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return [{"type": "text", "text": "rendered:\n" + "\n".join(made)}] + content


def still_convert(png, output, run=None):
    """png -> png|jpg|pdf next to it; returns the final path. `run(argv) -> exit code` runs
    ffmpeg (default: subprocess; the hosted service passes its sandbox)."""
    if output == "png":
        return png
    run = run or (lambda argv: subprocess.run(argv, capture_output=True).returncode)
    jpg = os.path.splitext(png)[0] + ".jpg"
    if run(["ffmpeg", "-v", "error", "-y", "-i", png, "-q:v", "2", jpg]) != 0 or not os.path.isfile(jpg):
        raise RuntimeError("jpeg conversion failed")
    if output == "jpg":
        _rm(png)
        return jpg
    with open(png, "rb") as f:
        head = f.read(24)
    w, h = int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big")
    pdf = os.path.splitext(png)[0] + ".pdf"
    with open(jpg, "rb") as f:
        data = jpeg_to_pdf(f.read(), w, h)
    with open(pdf, "wb") as f:
        f.write(data)
    _rm(png)
    _rm(jpg)
    return pdf


def _tool_error(msg):
    """Return an isError result with a clean message."""
    return [{"type": "text", "text": msg, "_isError": True}]


def main():
    for line in sys.stdin:
        if not line.strip():
            continue
        msg = json.loads(line)
        mid, method, params = msg.get("id"), msg.get("method"), msg.get("params") or {}
        if mid is None:
            continue  # notification
        try:
            if method == "initialize":
                res = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "agentfx", "version": "0.1.1"}}
            elif method == "tools/list":
                res = {"tools": TOOLS}
            elif method == "tools/call":
                content = call(params["name"], params.get("arguments") or {})
                is_error = any(c.pop("_isError", False) for c in content if isinstance(c, dict))
                res = {"content": content}
                if is_error:
                    res["isError"] = True
            elif method == "ping":
                res = {}
            else:
                raise ValueError(f"method {method} not supported")
            out = {"jsonrpc": "2.0", "id": mid, "result": res}
        except Exception as e:  # tool errors are results, protocol errors are errors
            out = {"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": str(e)}], "isError": True}}
        sys.stdout.write(json.dumps(out) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
