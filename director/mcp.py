#!/usr/bin/env python3
"""Director MCP server: the agent's side of the desk, over stdio (stdlib only).

    python3 director/mcp.py [--root ./agentfx-projects]

Speaks the small part of the Model Context Protocol an agent needs: initialize, ping, tools/list and
tools/call, as newline-delimited JSON-RPC 2.0 on stdin/stdout (logs go to stderr). The protocol is
implemented by hand so no package is needed; the state logic is shared
with director/desk.py (imported), so the desk and this server write the same files the same way.

Register it for Claude Code (see skill/agentfx/desk.md):
    claude mcp add agentfx-director -- python3 /path/to/agentfx/director/mcp.py --root /path/to/agentfx-projects
"""
import json
import os
import shlex
import subprocess
import sys
import traceback
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import desk  # noqa: E402

PROTOCOL = "2025-06-18"
RENDER_TIMEOUT = 1800
AGENTFX = os.environ.get("AGENTFX_BIN") or os.environ.get("REEL_BIN") or "agentfx"


def log(*a):
    print("director-mcp:", *a, file=sys.stderr, flush=True)


ROOT = desk.Root(desk.DEFAULT_ROOT.expanduser().absolute())


def project(slug: str) -> Path:
    d = ROOT.get(slug or "")
    if d is None:
        raise ValueError(f"no project {slug!r} under {ROOT.path} (list_projects shows the slugs)")
    return d


# ---------- tools ----------
def t_list_projects():
    out = []
    for d in ROOT.projects():
        c = desk.card(d)
        out.append({k: c[k] for k in ("slug", "name", "client", "status", "stage", "badge", "pending", "open_cards", "agent", "updated")})
    return {"root": str(ROOT.path), "projects": out}


def t_get_state(slug):
    d = project(slug)
    state = desk.read_json(d / "direction.json", {}) or {}
    meta = desk.read_json(d / "project.json", {}) or {}
    fb = desk.read_feedback(d)
    cursor = int(state.get("feedback_cursor", 0) or 0)
    cards = desk.read_cards(d)
    return {
        "slug": d.name, "dir": str(d),
        "direction": state, "project": meta,
        "stale_approvals": desk.stale_approvals(state),
        "open_cards": [c for c in cards if c.get("status") == "open"],
        "cards_total": len(cards),
        "unread_feedback": fb[cursor:],
        "feedback_cursor": cursor,
    }


def t_post_card(slug, card):
    if isinstance(card, str):
        card = json.loads(card)
    return {"ok": True, "card": desk.post_card(project(slug), card)}


def t_withdraw_card(slug, id, note=""):
    return {"ok": True, "card": desk.withdraw_card(project(slug), id, note)}


def t_read_feedback(slug, all=False):
    d = project(slug)
    state = desk.read_json(d / "direction.json", {}) or {}
    fb = desk.read_feedback(d)
    cursor = int(state.get("feedback_cursor", 0) or 0)
    return {"cursor": cursor, "total": len(fb), "lines": fb if all else fb[cursor:]}


def t_ack(slug, upto=None):
    d = project(slug)
    with desk.LOCK, desk.file_lock(d / "feedback.jsonl"):
        state = desk.load_state(d)
        n = len(desk.read_feedback(d))
        state["feedback_cursor"] = min(n, int(upto)) if upto is not None else n
        desk.write_json(d / "direction.json", state)
    return {"ok": True, "feedback_cursor": state["feedback_cursor"]}


MEDIA_SUFFIXES = {".mp3", ".wav", ".ogg", ".m4a", ".mp4", ".webm", ".jpg", ".jpeg", ".png", ".gif", ".webp"}
# media may be imported from the user's home directory and the temp directory only
MEDIA_ROOTS = [Path.home(), Path(os.environ.get("TMPDIR") or "/tmp")]


def t_import_media(slug, src, name=None):
    """Copy a media file from elsewhere (e.g. a music track you bring) into the
    project's out/cards/, so a card can show it (the desk only serves files inside the project)."""
    import shutil
    d = project(slug)
    p = Path(src).expanduser().resolve()
    if not p.is_file() or p.suffix.lower() not in MEDIA_SUFFIXES:
        raise ValueError(f"src must be an existing media file ({', '.join(sorted(MEDIA_SUFFIXES))})")
    if not any(str(p).startswith(str(r.resolve()) + os.sep) for r in MEDIA_ROOTS):
        raise ValueError("src must be under your home directory or the temp directory")
    if p.stat().st_size > 200 * 1024 * 1024:
        raise ValueError("src is larger than 200 MB")
    name = name or p.name
    if "/" in name or name.startswith(".") or Path(name).suffix.lower() != p.suffix.lower():
        raise ValueError("name: a plain file name with the same extension")
    dest = d / "out" / "cards" / name
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(p, dest)
    return {"ok": True, "path": str(dest.relative_to(d))}


def t_set_agent_state(slug, state, note=""):
    if state not in ("working", "waiting", "idle"):
        raise ValueError("state must be working, waiting or idle")
    d = project(slug)
    ts = desk.now_iso()
    with desk.LOCK:
        meta = desk.read_json(d / "project.json", None)
        if not isinstance(meta, dict):
            st = desk.read_json(d / "direction.json", {}) or {}
            meta = {"schema": "reel-project/1", "slug": d.name,
                    "name": (st.get("project") or {}).get("title") or d.name, "status": "active"}
        meta["agent"] = {"state": state, "note": (note or "")[:240], "at": ts}
        meta["updated"] = ts
        desk.write_json(d / "project.json", meta)
    return {"ok": True, "agent": meta["agent"]}


def mem_available_kb() -> int:
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemAvailable:"):
                return int(line.split()[1])
    except OSError:
        pass
    return 0


def _inside(d: Path, rel: str, what: str) -> Path:
    if not rel or rel.startswith("/") or ".." in Path(rel).parts:
        raise ValueError(f"{what} must be a path inside the project")
    return d / rel


def _run(cmd, cwd, timeout=RENDER_TIMEOUT):
    log("run:", " ".join(shlex.quote(str(c)) for c in cmd))
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError(f"{cmd[0]} failed ({r.returncode}): {(r.stderr or r.stdout)[-1500:]}")
    return r


def t_render_preview(slug, source, out=None, at=None, t0=None, t1=None, format=None):
    """A still (at) or a short clip (t0..t1) of a .reel in the project.

    Still: `agentfx frame <source> --at <at>s -o <png>`; a .jpg out is converted with ffmpeg (1280 px).
    Clip: `agentfx render <source>` (the whole source; there is no range render yet), then
    `ffmpeg -ss t0 -t (t1-t0)` scaled to 1280. Keep clip sources short (a beat, a hero .reel).
    Runs locally with nice -n 19."""
    d = project(slug)
    src = _inside(d, source, "source")
    if not src.is_file():
        raise ValueError(f"no source {source} in {d.name}")
    still = at is not None
    if still == (t0 is not None or t1 is not None):
        raise ValueError("give either at (a still) or t0 and t1 (a clip)")
    if not still:
        if t0 is None or t1 is None or float(t1) <= float(t0):
            raise ValueError("a clip needs t0 < t1")
        if float(t1) - float(t0) > 20:
            raise ValueError("a preview clip is at most 20 s")
    stem = Path(source).stem
    if out is None:
        out = f"out/previews/{stem}-{float(at):g}s.jpg" if still else f"out/previews/{stem}-{float(t0):g}-{float(t1):g}s.mp4"
    dest = _inside(d, out, "out")
    dest.parent.mkdir(parents=True, exist_ok=True)
    fmt = ["--format", format] if format else []
    mem = mem_available_kb()
    tmpname = f"out/previews/.tmp-{stem}"
    raw_rel = tmpname + (".png" if still else ".mp4")
    if still:
        reel_cmd = [AGENTFX, "frame", source, "--at", f"{float(at):g}s", *fmt, "-o", raw_rel]
    else:
        reel_cmd = [AGENTFX, "render", source, *fmt, "-o", raw_rel]
    (d / "out/previews").mkdir(parents=True, exist_ok=True)
    _run(["nice", "-n", "19", *reel_cmd], cwd=d)
    raw = d / raw_rel
    try:
        if still:
            if dest.suffix.lower() in (".jpg", ".jpeg"):
                _run(["ffmpeg", "-v", "error", "-y", "-i", str(raw), "-vf", "scale='min(1280,iw)':-2", "-q:v", "3", str(dest)], cwd=d, timeout=120)
            else:
                raw.replace(dest)
        else:
            _run(["nice", "-n", "19", "ffmpeg", "-v", "error", "-y", "-ss", f"{float(t0):.3f}", "-i", str(raw), "-t", f"{float(t1) - float(t0):.3f}",
                  "-vf", "scale='min(1280,iw)':-2", "-c:v", "libx264", "-crf", "20", "-preset", "veryfast", "-pix_fmt", "yuv420p",
                  "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart", str(dest)], cwd=d, timeout=600)
    finally:
        if raw.exists():
            raw.unlink()
    return {"ok": True, "path": str(dest.relative_to(d)), "mem_available_gb": round(mem / 1048576, 2),
            "hint": "use this path as media.still / media.clip or an option's still / clip in a card"}


S_SLUG = {"type": "string", "description": "project slug (folder name under the projects root)"}
CARD_SCHEMA = {
    "type": "object",
    "description": ("A decision card. type: choice | compare | note | approve | question. Fields: id (optional; "
                    "same id = new version), title, body, stage, beat, media {still, clip, cue, dur, clip_t0, clock}, "
                    "options (choice/compare: 2-4 of {id, label, detail, still, clip, audio, cue, dur, status}), "
                    "default (option id; question: proposed answer text), attributes (choice: mixable attribute names), "
                    "target + rev (approve), status (open|withdrawn). See skill/agentfx/schema.md."),
}
TOOLS = [
    ("list_projects", "List director projects with stage, badge, unread feedback and open cards.", {}, [], t_list_projects),
    ("get_state", "Everything about one project: direction.json, project.json, stale approvals, open cards, unread feedback.",
     {"slug": S_SLUG}, ["slug"], t_get_state),
    ("post_card", "Post a decision card to the desk stream (or a new version of one: same id). Returns the stored card.",
     {"slug": S_SLUG, "card": CARD_SCHEMA}, ["slug", "card"], t_post_card),
    ("withdraw_card", "Withdraw an open card (it moves to the history as withdrawn).",
     {"slug": S_SLUG, "id": {"type": "string"}, "note": {"type": "string", "description": "why, one line"}}, ["slug", "id"], t_withdraw_card),
    ("read_feedback", "Unread feedback lines (card answers, picks, notes, brief edits) past the cursor. all=true for every line.",
     {"slug": S_SLUG, "all": {"type": "boolean"}}, ["slug"], t_read_feedback),
    ("ack", "Mark feedback as read (moves direction.json feedback_cursor to the end, or to upto).",
     {"slug": S_SLUG, "upto": {"type": "integer", "description": "last seq handled (default: all)"}}, ["slug"], t_ack),
    ("render_preview", "Render a still (at) or a short clip (t0, t1; whole source rendered, then cut) of a .reel in the project. "
     "Runs locally (nice 19). Returns the project-relative path.",
     {"slug": S_SLUG, "source": {"type": "string", "description": "a .reel path inside the project"},
      "at": {"type": "number", "description": "seconds, for a still"}, "t0": {"type": "number"}, "t1": {"type": "number"},
      "out": {"type": "string", "description": "project-relative output (.jpg/.png for stills, .mp4 for clips); default out/previews/..."},
      "format": {"type": "string", "description": "aspect, e.g. 16:9 or 9:16"}}, ["slug", "source"], t_render_preview),
    ("import_media", "Copy a media file (audio, video, image under your home or temp directory) into the project's out/cards/ so a card can use it. "
     "Returns the project-relative path.",
     {"slug": S_SLUG, "src": {"type": "string", "description": "absolute path of the file"},
      "name": {"type": "string", "description": "file name in out/cards/ (default: the source name)"}}, ["slug", "src"], t_import_media),
    ("set_agent_state", "Set the agent line on the project card: working (before a render), waiting (the human's turn), idle.",
     {"slug": S_SLUG, "state": {"type": "string", "enum": ["working", "waiting", "idle"]}, "note": {"type": "string"}},
     ["slug", "state"], t_set_agent_state),
]
BY_NAME = {t[0]: t for t in TOOLS}


def tool_list():
    return [{"name": n, "description": desc, "inputSchema": {"type": "object", "properties": props, "required": req}}
            for n, desc, props, req, _ in TOOLS]


def call_tool(name, args):
    if name not in BY_NAME:
        raise KeyError(name)
    fn = BY_NAME[name][4]
    return fn(**(args or {}))


# ---------- JSON-RPC over stdio ----------
def reply(id_, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": id_}
    if error is not None:
        msg["error"] = error
    else:
        msg["result"] = result
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def handle(msg):
    method, id_, params = msg.get("method"), msg.get("id"), msg.get("params") or {}
    is_note = "id" not in msg
    if method == "initialize":
        return reply(id_, {"protocolVersion": params.get("protocolVersion") or PROTOCOL,
                           "capabilities": {"tools": {"listChanged": False}},
                           "serverInfo": {"name": "director", "version": "3.0"},
                           "instructions": "AgentFX director desk: post decision cards, read the human's answers, render previews. "
                                           "Projects root: " + str(ROOT.path)})
    if method == "ping":
        return reply(id_, {})
    if method == "tools/list":
        return reply(id_, {"tools": tool_list()})
    if method == "tools/call":
        name, args = params.get("name"), params.get("arguments") or {}
        try:
            out = call_tool(name, args)
            return reply(id_, {"content": [{"type": "text", "text": json.dumps(out, ensure_ascii=False, indent=1)}],
                               "structuredContent": out if isinstance(out, dict) else {"result": out}, "isError": False})
        except KeyError:
            return reply(id_, error={"code": -32602, "message": f"unknown tool {name!r}"})
        except TypeError as e:
            return reply(id_, {"content": [{"type": "text", "text": f"bad arguments for {name}: {e}"}], "isError": True})
        except (ValueError, RuntimeError, OSError, subprocess.TimeoutExpired, json.JSONDecodeError) as e:
            return reply(id_, {"content": [{"type": "text", "text": f"{name}: {e}"}], "isError": True})
    if is_note:
        return None                       # notifications/initialized, cancelled, ...: no answer
    return reply(id_, error={"code": -32601, "message": f"method not found: {method}"})


def main():
    global ROOT
    args = sys.argv[1:]
    if "--root" in args:
        ROOT = desk.Root(Path(args[args.index("--root") + 1]).expanduser().absolute())
    log(f"serving {ROOT.path} ({len(ROOT.projects())} projects)")
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            reply(None, error={"code": -32700, "message": "parse error"})
            continue
        batch = msg if isinstance(msg, list) else [msg]
        for m in batch:
            try:
                handle(m)
            except Exception as e:  # never die on one bad request
                log(traceback.format_exc())
                if isinstance(m, dict) and "id" in m:
                    reply(m.get("id"), error={"code": -32603, "message": str(e)})


if __name__ == "__main__":
    main()
