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
  agentfx_render        render the video (MP4, or GIF/WebP/alpha by extension) to a file
  agentfx_design_guide  the design layer guide and the list of lenses
  agentfx_lens          one lens: essay (LENS.md) and tokens (design.reel)
  agentfx_critique      optional vision critique of a sheet or video (needs ANTHROPIC_API_KEY)

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
    {"name": "agentfx_lint", "description": "Lint a Reel Script video. JSON diagnostics with fix hints. measure=true checks real layout in every format.",
     "inputSchema": {"type": "object", "properties": {**SRC, "measure": {"type": "boolean", "default": True}, "no_tts": {"type": "boolean"}}}},
    {"name": "agentfx_timing", "description": "Resolved timeline JSON (scenes, sentences, anchors).",
     "inputSchema": {"type": "object", "properties": {**SRC, "no_tts": {"type": "boolean"}}}},
    {"name": "agentfx_frame", "description": "One frame PNG at an anchor name, scene:anchor or seconds. Look at it to check layout.",
     "inputSchema": {"type": "object", "properties": {**SRC, "at": {"type": "string"}, "format": FMT}, "required": ["at"]}},
    {"name": "agentfx_sheet", "description": "Contact sheet PNG of the video (n evenly spaced frames, or one per event).",
     "inputSchema": {"type": "object", "properties": {**SRC, "n": {"type": "integer", "default": 12}, "events": {"type": "boolean"}, "format": FMT, "cols": {"type": "integer"}}}},
    {"name": "agentfx_render", "description": "Render the video to `out` (.mp4; .gif, .webp, .mov or .webm by extension). Returns the output path and the renderer's summary.",
     "inputSchema": {"type": "object", "properties": {**SRC, "out": {"type": "string", "description": "output file path"}, "format": FMT,
                                                      "no_tts": {"type": "boolean"}, "alpha": {"type": "boolean"}}, "required": ["out"]}},
    {"name": "agentfx_design_guide", "description": "Design layer guide: lenses, design spec format, per-step detail components, lint rules.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "agentfx_lens", "description": "A named look (lens): essay with numbers, negatives, motion tendencies, and its design.reel tokens.",
     "inputSchema": {"type": "object", "properties": {"name": {"type": "string", "description": "a lens name from agentfx_design_guide"}}, "required": ["name"]}},
    {"name": "agentfx_critique", "description": "Optional vision critique of a sheet/video against the design rubric (needs ANTHROPIC_API_KEY in the server's environment).",
     "inputSchema": {"type": "object", "properties": {"file": {"type": "string", "description": "sheet .png or .mp4"}, "brief": {"type": "string"},
                                                      "model": {"type": "string", "description": "haiku (default) or sonnet"}}, "required": ["file"]}},
]


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


def src_path(args):
    if args.get("file"):
        return os.path.abspath(args["file"]), None
    base = args.get("assets_dir") or tempfile.gettempdir()
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


def call(name, args):
    if name == "agentfx_spec":
        spec_text = read("docs", "SPEC.md")
        section = args.get("section")
        if section and section.lower() == "all":
            return [{"type": "text", "text": spec_text}]
        if section:
            return [{"type": "text", "text": _spec_section(spec_text, section)}]
        return [{"type": "text", "text": _spec_overview(spec_text)}]

    if name == "agentfx_catalog":
        return [{"type": "text", "text": run(["catalog"] + (["--md"] if args.get("md") else []))[1]}]

    if name == "agentfx_design_guide":
        root = os.path.join(HOME, "design", "lenses")
        lenses = sorted(d for d in os.listdir(root) if os.path.isdir(os.path.join(root, d)))
        return [{"type": "text", "text": read("design", "AUTHORING.md") + "\n\nLenses: " + ", ".join(lenses)}]

    if name == "agentfx_lens":
        d = os.path.basename(args["name"])
        return [{"type": "text", "text": read("design", "lenses", d, "LENS.md") + "\n\n" + read("design", "lenses", d, "design.reel")}]

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

    path, tmp = src_path(args)
    cwd = os.path.dirname(path)
    try:
        if name == "agentfx_lint":
            cmd = ["lint", path] + (["--measure", "--all-formats"] if args.get("measure", True) else []) + (["--no-tts"] if args.get("no_tts") else [])
            code, out, err = run(cmd, cwd)
            # lint diagnostics are data, not failures -- return them even on non-zero exit
            # (exit 1 = has errors, which is normal lint output)
            return [{"type": "text", "text": out + "\n" + (err.strip().splitlines() or [""])[-1]}]

        if name == "agentfx_timing":
            code, out, err = run(["timing", path] + (["--no-tts"] if args.get("no_tts") else []), cwd)
            if code != 0 and not out.strip():
                return _tool_error(f"timing failed (exit {code}, tmp: {_tmpdir()}): {err[-3000:]}")
            return [{"type": "text", "text": out or err}]

        if name == "agentfx_render":
            out_path = os.path.abspath(args["out"])
            cmd = ["render", path, "-o", out_path] + (["--format", args["format"]] if args.get("format") else []) \
                + (["--no-tts"] if args.get("no_tts") else []) + (["--alpha"] if args.get("alpha") else [])
            code, out, err = run(cmd, cwd)
            if code != 0 or not os.path.exists(out_path):
                return _tool_error(f"render failed (exit {code}, work: {cwd}, tmp: {_tmpdir()}):\n{err[-4000:]}")
            return [{"type": "text", "text": f"rendered {out_path} ({os.path.getsize(out_path)} bytes)\n" + (out or err)[-2000:]}]

        if name in ("agentfx_sheet", "agentfx_frame"):
            fd, png = tempfile.mkstemp(suffix=".png")
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
            code, out, err = run(cmd, cwd)
            try:
                if code != 0 or os.path.getsize(png) == 0:
                    return _tool_error(f"{'sheet' if name == 'agentfx_sheet' else 'frame'} failed (exit {code}, work: {cwd}, tmp: {_tmpdir()}):\n{err[-4000:]}")
                with open(png, "rb") as f:
                    data = base64.b64encode(f.read()).decode()
            finally:
                os.remove(png)
            return [{"type": "image", "data": data, "mimeType": "image/png"}, {"type": "text", "text": err[-2000:]}]
    finally:
        if tmp:
            os.remove(tmp)
    raise ValueError(f"unknown tool {name}")


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
                res = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "agentfx", "version": "0.1.0"}}
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
