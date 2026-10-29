#!/usr/bin/env python3
"""Director's desk: a steering page for AgentFX film projects (stdlib only).

    desk.py serve [ROOT_OR_PROJECT] [--port 8771] [--host 127.0.0.1]
    desk.py list                       projects under the root with stage, status, pending actions
    desk.py inbox [--json]             pending desk actions across all projects (one JSON line each)
    desk.py new NAME [--client C] [--goal G] [--aspect 16:9 ...] [--length 30]
    desk.py status   <slug|project-dir>   stage, stale approvals, unread feedback
    desk.py feedback <slug|project-dir> [--all]  print unread feedback (the agent's inbox)
    desk.py ack      <slug|project-dir>   mark all feedback as read (moves feedback_cursor)
    desk.py cards    <slug|project-dir> [--all]   open cards (all: every card, latest version)
    desk.py post     <slug|project-dir> CARD.json   post a card (or "-" for stdin); prints it

ROOT defaults to ./agentfx-projects (or --root / $AGENTFX_PROJECTS). A root holds one folder per
project (slug name) with project.json, direction.json and feedback.jsonl. Passing a single project
dir (one that holds direction.json) still works: the desk then shows only that project.

Every button on the page POSTs one action that is appended to <project>/feedback.jsonl (one JSON
object per line). The agent reads the lines past direction.json's "feedback_cursor", acts, updates
direction.json and moves the cursor. The desk writes direction.json only for brief edits; it writes
project.json only to archive/unarchive, and creates the files of a new project.

v3: the agent posts decision cards into <project>/cards.jsonl (append-only, latest version of an id
wins) through POST /api/p/<slug>/cards (localhost only) or director/mcp.py. The desk renders them as
a stream; an answer is one feedback line (action "answer") plus an answered version of the card.
"""
import argparse
import contextlib
import fcntl
import hashlib
import json
import mimetypes
import os
import re
import sys
import threading
import time
import unicodedata
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlparse

HERE = Path(__file__).resolve().parent
DEFAULT_ROOT = Path(os.environ.get("AGENTFX_PROJECTS") or os.environ.get("REEL_PROJECTS") or "agentfx-projects").expanduser()
# loopback only by default; pass --host explicitly to listen on another (private) interface
DEFAULT_HOSTS = ["127.0.0.1"]
STAGES = ["mode", "brief", "directions", "dramaturgy", "storyboard", "variants", "render"]
STAGE_OF_TARGET = {"brief": "brief", "direction": "directions", "dramaturgy": "dramaturgy", "storyboard": "storyboard"}
ACTIONS = {"pick", "mix", "note", "new-direction", "approve", "reorder", "branch", "reject"}
STATUSES = {"active", "paused", "done", "archived"}
ASPECTS = ["16:9", "9:16", "1:1", "4:5"]
SLUG_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,47}$")
WATCH_SUFFIXES = {".json", ".jpg", ".jpeg", ".png", ".mp4", ".webm", ".jsonl", ".mp3", ".wav"}
STATIC = {"desk.css": "text/css; charset=utf-8", "desk.js": "text/javascript; charset=utf-8"}

mimetypes.add_type("video/mp4", ".mp4")
mimetypes.add_type("audio/mpeg", ".mp3")
mimetypes.add_type("application/json", ".json")
LOCK = threading.Lock()


def now_iso() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds")


def read_json(p: Path, default=None):
    try:
        return json.loads(p.read_text())
    except (OSError, json.JSONDecodeError):
        return default


def write_json(p: Path, obj):
    tmp = p.with_name(p.name + ".tmp")
    tmp.write_text(json.dumps(obj, ensure_ascii=False, indent=2) + "\n")
    tmp.replace(p)


def load_state(project: Path) -> dict:
    return json.loads((project / "direction.json").read_text())


def read_feedback(project: Path) -> list:
    f = project / "feedback.jsonl"
    if not f.exists():
        return []
    out = []
    for line in f.read_text().splitlines():
        line = line.strip()
        if line:
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                out.append({"action": "invalid", "raw": line})
    return out


@contextlib.contextmanager
def file_lock(path: Path):
    """Exclusive flock on a state file, so the desk and the MCP server (two processes) never
    interleave a read-modify-append. Opening in append mode creates the file if missing."""
    with open(path, "a") as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(f, fcntl.LOCK_UN)


def append_feedback(project: Path, rec: dict) -> int:
    """Append one action; the caller holds LOCK. Returns its seq."""
    with file_lock(project / "feedback.jsonl"):
        seq = len(read_feedback(project)) + 1
        with open(project / "feedback.jsonl", "a") as f:
            f.write(json.dumps({"seq": seq, **rec}, ensure_ascii=False) + "\n")
    return seq


# ---------- cards (v3) ----------
CARD_TYPES = {"choice", "compare", "note", "approve", "question"}
CARD_STATUSES = {"open", "answered", "withdrawn"}
CARD_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$")
APPROVE_TARGETS = {"brief", "direction", "dramaturgy", "storyboard", "music", "variant", "render", "lock", "item"}
CARD_FIELDS = {"id", "v", "type", "title", "body", "stage", "beat", "options", "media", "default", "status",
               "answer", "created", "updated", "by", "attributes", "target", "rev", "item", "placeholder"}


def read_cards(project: Path, history=False):
    """Latest version of every card, in order of first appearance (history=True: every line)."""
    f = project / "cards.jsonl"
    if not f.exists():
        return []
    lines = []
    for line in f.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(obj, dict) and obj.get("id"):
            lines.append(obj)
    if history:
        return lines
    latest = {}
    for c in lines:
        latest[c["id"]] = c          # dict keeps first-insertion order, value is the newest line
    return list(latest.values())


def get_card(project: Path, cid: str):
    return next((c for c in read_cards(project) if c["id"] == cid), None)


def _str(v, field, maxlen, required=False):
    if v is None or v == "":
        if required:
            raise ValueError(f"{field} is required")
        return None
    if not isinstance(v, str) or len(v) > maxlen:
        raise ValueError(f"{field} must be text up to {maxlen} characters")
    return v


def _media(m, field="media"):
    """{still?, clip?, cue?, dur?, t0?, t1?, clip_t0?, clock?}: paths are project-relative."""
    if m is None:
        return None
    if not isinstance(m, dict):
        raise ValueError(f"{field} must be an object")
    out = {}
    for k in ("still", "clip", "audio", "src"):
        if m.get(k):
            p = _str(m[k], f"{field}.{k}", 400)
            if p.startswith("/") or ".." in Path(p).parts:
                raise ValueError(f"{field}.{k} must be a path inside the project")
            out[k] = p
    for k in ("cue", "dur", "t0", "t1", "clip_t0"):
        if m.get(k) is not None:
            if not isinstance(m[k], (int, float)):
                raise ValueError(f"{field}.{k} must be a number of seconds")
            out[k] = float(m[k])
    if m.get("clock") in ("clip", "film"):
        out["clock"] = m["clock"]
    if m.get("label"):
        out["label"] = _str(m["label"], f"{field}.label", 120)
    return out


def validate_card(c: dict, prev: dict = None) -> dict:
    """Check a card the agent posts. With prev (an update), missing fields come from prev."""
    if not isinstance(c, dict):
        raise ValueError("card must be an object")
    unknown = set(c) - CARD_FIELDS
    if unknown:
        raise ValueError(f"unknown card fields: {sorted(unknown)}")
    merged = {**(prev or {}), **{k: v for k, v in c.items() if v is not None}}
    t = merged.get("type")
    if t not in CARD_TYPES:
        raise ValueError(f"type must be one of {sorted(CARD_TYPES)}")
    if prev and prev.get("type") != t:
        raise ValueError("a card keeps its type; post a new card instead")
    out = {"id": merged["id"], "type": t,
           "title": _str(merged.get("title"), "title", 200, required=True),
           "body": _str(merged.get("body"), "body", 4000) or ""}
    for k in ("stage", "beat", "item"):
        if merged.get(k):
            out[k] = _str(merged[k], k, 80)
    if merged.get("media") is not None:
        out["media"] = _media(merged["media"])
    opts = merged.get("options")
    if t in ("choice", "compare"):
        if not isinstance(opts, list) or not 2 <= len(opts) <= 4:
            raise ValueError(f"a {t} card needs 2 to 4 options")
        clean, ids = [], set()
        for i, o in enumerate(opts):
            if not isinstance(o, dict):
                raise ValueError(f"options[{i}] must be an object")
            oid = _str(o.get("id"), f"options[{i}].id", 40, required=True)
            if oid in ids:
                raise ValueError(f"duplicate option id {oid!r}")
            ids.add(oid)
            co = {"id": oid, "label": _str(o.get("label"), f"options[{i}].label", 120, required=True),
                  "detail": _str(o.get("detail"), f"options[{i}].detail", 600) or ""}
            m = _media({k: o[k] for k in ("still", "clip", "audio", "cue", "dur", "clip_t0") if k in o}, f"options[{i}]")
            co.update(m or {})
            if t == "compare" and not co.get("clip"):
                raise ValueError(f"options[{i}].clip is required on a compare card")
            if o.get("status") in ("implemented", "partial", "planned"):
                co["status"] = o["status"]
            if o.get("attrs") is not None:
                if not isinstance(o["attrs"], dict):
                    raise ValueError(f"options[{i}].attrs must be an object")
                co["attrs"] = {str(k)[:40]: str(v)[:200] for k, v in o["attrs"].items()}
            clean.append(co)
        out["options"] = clean
        d = merged.get("default")
        if d is not None and d not in ids:
            raise ValueError("default must be one of the option ids")
        if d is not None and any(o.get("status") == "planned" and o["id"] == d for o in clean):
            raise ValueError("a planned option cannot be the default")
        if d is not None:
            out["default"] = d
        attrs = merged.get("attributes")
        if attrs is not None:
            if t != "choice" or not isinstance(attrs, list) or not all(isinstance(a, str) and a for a in attrs) or len(attrs) > 8:
                raise ValueError("attributes: a list of up to 8 names, choice cards only")
            out["attributes"] = attrs
    elif opts:
        raise ValueError(f"a {t} card has no options")
    if t == "note" and not (out.get("media") or {}).get("clip") and not (out.get("media") or {}).get("still"):
        raise ValueError("a note card needs media.clip (or media.still)")
    if t == "approve":
        tgt = merged.get("target")
        if tgt not in APPROVE_TARGETS:
            raise ValueError(f"target must be one of {sorted(APPROVE_TARGETS)}")
        out["target"] = tgt
        if merged.get("rev") is not None:
            if not isinstance(merged["rev"], int):
                raise ValueError("rev must be an integer")
            out["rev"] = merged["rev"]
    if t == "question":
        out["default"] = _str(merged.get("default"), "default", 2000, required=True)
        if merged.get("placeholder"):
            out["placeholder"] = _str(merged["placeholder"], "placeholder", 200)
    st = merged.get("status", "open")
    if st not in CARD_STATUSES:
        raise ValueError(f"status must be one of {sorted(CARD_STATUSES)}")
    out["status"] = st
    if st == "answered" and merged.get("answer") is not None:
        out["answer"] = merged["answer"]
    return out


def post_card(project: Path, card: dict, by="agent") -> dict:
    """Add a card, or a new version of one (same id; fields not given are kept). Returns it."""
    if not isinstance(card, dict):
        raise ValueError("card must be an object")
    cid = card.get("id")
    if cid is not None and (not isinstance(cid, str) or not CARD_ID_RE.match(cid)):
        raise ValueError("id: letters, digits, '.', '_' or '-', up to 64 characters")
    with LOCK, file_lock(project / "cards.jsonl"):
        cards = read_cards(project)
        prev = next((c for c in cards if c["id"] == cid), None) if cid else None
        if cid is None:
            n = len(cards) + 1
            taken = {c["id"] for c in cards}
            while f"c{n}" in taken:
                n += 1
            cid = f"c{n}"
        out = validate_card({**card, "id": cid}, prev)
        if prev and prev.get("status") == "answered" and "status" not in card:
            out["status"] = "answered"
            out["answer"] = prev.get("answer")
        if out["status"] != "answered":
            out.pop("answer", None)
        ts = now_iso()
        out = {**out, "v": int(prev.get("v", 0)) + 1 if prev else 1,
               "created": prev.get("created", ts) if prev else ts, "updated": ts, "by": by}
        with open(project / "cards.jsonl", "a") as f:
            f.write(json.dumps(out, ensure_ascii=False) + "\n")
    return out


def withdraw_card(project: Path, cid: str, note: str = "") -> dict:
    prev = get_card(project, cid)
    if not prev:
        raise ValueError(f"no card {cid!r}")
    upd = {"id": cid, "status": "withdrawn"}
    if note:
        upd["body"] = (prev.get("body", "") + f"\n\nWithdrawn: {note}").strip()
    return post_card(project, upd)


def _num(v, field):
    if not isinstance(v, (int, float)) or v != v or v < 0 or v > 36000:
        raise ValueError(f"{field} must be seconds")
    return round(float(v), 3)


def _notes(notes, card):
    if notes is None:
        return []
    if not isinstance(notes, list) or len(notes) > 50:
        raise ValueError("notes: a list of up to 50")
    clip_ids = {o["id"] for o in card.get("options") or []}
    out = []
    for i, n in enumerate(notes):
        if not isinstance(n, dict):
            raise ValueError(f"notes[{i}] must be an object")
        rec = {"text": _str(n.get("text"), f"notes[{i}].text", 2000, required=True)}
        if n.get("t") is not None:
            rec["t"] = _num(n["t"], f"notes[{i}].t")
        if n.get("t0") is not None or n.get("t1") is not None:
            t0, t1 = _num(n.get("t0"), f"notes[{i}].t0"), _num(n.get("t1"), f"notes[{i}].t1")
            if t1 <= t0:
                raise ValueError(f"notes[{i}]: t1 must be after t0")
            rec["t0"], rec["t1"] = t0, t1
        for k in ("t_film", "t0_film", "t1_film"):
            if n.get(k) is not None:
                rec[k] = _num(n[k], f"notes[{i}].{k}")
        if n.get("clip"):
            if clip_ids and n["clip"] not in clip_ids:
                raise ValueError(f"notes[{i}].clip must be one of the card's clips")
            rec["clip"] = _str(n["clip"], f"notes[{i}].clip", 40)
        if n.get("beat"):
            rec["beat"] = _str(n["beat"], f"notes[{i}].beat", 40)
        if n.get("clock") in ("clip", "film"):
            rec["clock"] = n["clock"]
        out.append(rec)
    return out


def _context(ctx):
    """What the human had selected: {beat?, point?: {src, x, y}, range?: {src, clip?, t0, t1}}."""
    if not ctx:
        return None
    if not isinstance(ctx, dict) or len(json.dumps(ctx)) > 2000:
        raise ValueError("context must be a small object")
    out = {}
    if ctx.get("beat"):
        out["beat"] = _str(ctx["beat"], "context.beat", 40)
    p = ctx.get("point")
    if isinstance(p, dict) and isinstance(p.get("x"), (int, float)) and isinstance(p.get("y"), (int, float)):
        out["point"] = {"src": _str(p.get("src"), "context.point.src", 400) or "",
                        "x": round(min(1, max(0, p["x"])), 4), "y": round(min(1, max(0, p["y"])), 4)}
        if p.get("label"):
            out["point"]["label"] = _str(p["label"], "context.point.label", 120)
    r = ctx.get("range")
    if isinstance(r, dict):
        t0, t1 = _num(r.get("t0"), "context.range.t0"), _num(r.get("t1"), "context.range.t1")
        if t1 > t0:
            out["range"] = {"t0": t0, "t1": t1}
            for k in ("src", "clip", "label"):
                if r.get(k):
                    out["range"][k] = _str(r[k], f"context.range.{k}", 400)
            for k in ("t0_film", "t1_film"):
                if r.get(k) is not None:
                    out["range"][k] = _num(r[k], f"context.range.{k}")
    return out or None


def answer_card(project: Path, cid: str, payload: dict, by="operator") -> dict:
    """The human answers an open card: one feedback line (action "answer") and an answered version
    of the card. payload.v must be the version the human saw (409-style error if it moved)."""
    if not isinstance(payload, dict):
        raise ValueError("expected a JSON object")
    with LOCK, file_lock(project / "cards.jsonl"):
        card = get_card(project, cid)
        if not card:
            raise LookupError(f"no card {cid!r}")
        if card.get("status") != "open":
            raise ValueError(f"card {cid} is {card.get('status')}, not open")
        seen = payload.get("v")
        if seen is not None and seen != card.get("v"):
            raise ValueError(f"card {cid} changed (v{card.get('v')}, you saw v{seen}); look again")
        t, ans = card["type"], {}
        ids = [o["id"] for o in card.get("options") or []]
        text = _str(payload.get("text"), "text", 4000)
        if t == "choice":
            if payload.get("mix") is not None:
                mix = payload["mix"]
                attrs = card.get("attributes") or []
                if not attrs:
                    raise ValueError("this card does not offer a mix")
                if not isinstance(mix, list) or not mix:
                    raise ValueError("mix: a list of {attribute, from}")
                clean = []
                for m in mix:
                    if not isinstance(m, dict) or m.get("attribute") not in attrs or m.get("from") not in ids:
                        raise ValueError("mix items need an attribute of the card and an option id in from")
                    clean.append({"attribute": m["attribute"], "from": m["from"]})
                ans["mix"] = clean
            else:
                pick = payload.get("pick", card.get("default") if payload.get("accept") else None)
                if pick not in ids:
                    raise ValueError("pick must be one of the option ids")
                if any(o["id"] == pick and o.get("status") == "planned" for o in card["options"]):
                    raise ValueError("a planned option cannot be picked")
                ans["pick"] = pick
                if payload.get("accept"):
                    ans["accepted_default"] = True
        elif t == "compare":
            notes = _notes(payload.get("notes"), card)
            if payload.get("pick") is not None:
                if payload["pick"] not in ids:
                    raise ValueError("pick must be one of the clip ids")
                ans["pick"] = payload["pick"]
            elif payload.get("branch") is not None:
                if payload["branch"] not in ids:
                    raise ValueError("branch must be one of the clip ids")
                if not text:
                    raise ValueError("a branch needs a text: what should change")
                ans["branch"] = payload["branch"]
            elif payload.get("accept") and card.get("default"):
                ans["pick"], ans["accepted_default"] = card["default"], True
            elif not notes and not payload.get("none"):
                raise ValueError("pick, branch, notes or none")
            if payload.get("none"):
                ans["none"] = True
            if notes:
                ans["notes"] = notes
        elif t == "note":
            notes = _notes(payload.get("notes"), card)
            if not notes and not payload.get("none") and not text:
                raise ValueError("notes, a text, or none")
            if notes:
                ans["notes"] = notes
            if payload.get("none"):
                ans["none"] = True
        elif t == "approve":
            ok = payload.get("approve", True if payload.get("accept") else None)
            if not isinstance(ok, bool):
                raise ValueError("approve must be true or false")
            if not ok and not text:
                raise ValueError("say what should change")
            ans["approve"] = ok
            if card.get("rev") is not None:
                ans["rev"] = card["rev"]
        elif t == "question":
            if payload.get("accept"):
                ans["text"], ans["accepted_default"] = card["default"], True
            elif not text:
                raise ValueError("a text, or accept the default")
        if text is not None and "text" not in ans:
            ans["text"] = text
        ctx = _context(payload.get("context"))
        if ctx:
            ans["context"] = ctx
        state = read_json(project / "direction.json", {}) or {}
        ts = now_iso()
        rec = {"ts": ts, "seen_revision": state.get("revision"), "stage": card.get("stage") or state.get("stage"),
               "by": by, "action": "answer", "card": cid, "v": card["v"], "card_type": t,
               "title": card["title"], **({"beat": card["beat"]} if card.get("beat") else {}), **ans}
        if t == "approve":
            rec["target"] = card["target"]
        seq = append_feedback(project, rec)
        done = {**card, "v": card["v"] + 1, "status": "answered", "updated": ts,
                "answer": {**ans, "seq": seq, "at": ts, "by": by, "answered_v": card["v"]}}
        with open(project / "cards.jsonl", "a") as f:
            f.write(json.dumps(done, ensure_ascii=False) + "\n")
    return {"ok": True, "seq": seq, "card": done}


def slugify(name: str) -> str:
    s = unicodedata.normalize("NFKD", name.replace("ß", "ss")).encode("ascii", "ignore").decode()
    s = re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")
    return (s[:48].rstrip("-") or "project")


def version(project: Path, media_only=False) -> str:
    """Cheap change signature: state files and every render (media_only: renders only)."""
    h = hashlib.sha1()
    paths = [] if media_only else [project / "direction.json", project / "feedback.jsonl", project / "project.json",
                                   project / "cards.jsonl"]
    for sub in ("stills", "out/variants", "out/renders", "out/previews", "out/cards"):
        d = project / sub
        if d.is_dir():
            paths += sorted(p for p in d.iterdir() if p.suffix.lower() in WATCH_SUFFIXES)
    for p in paths:
        try:
            st = p.stat()
            h.update(f"{p.name}:{st.st_mtime_ns}:{st.st_size};".encode())
        except FileNotFoundError:
            h.update(f"{p.name}:-;".encode())
    return h.hexdigest()[:16]


def stale_approvals(state: dict) -> list:
    """An approval is stale when its target has moved past the approved revision."""
    revs = {
        "brief": (state.get("brief") or {}).get("rev"),
        "direction": (state.get("chosen_direction") or {}).get("rev"),
        "dramaturgy": (state.get("dramaturgy") or {}).get("rev"),
        "storyboard": (state.get("storyboard") or {}).get("rev"),
    }
    latest = {}
    for a in state.get("approvals", []):
        t = a.get("target")
        if t not in latest or a.get("rev", 0) >= latest[t].get("rev", 0):
            latest[t] = a
    out = []
    for t, a in latest.items():
        cur = revs.get(t)
        if cur is not None and a.get("rev") != cur:
            out.append({"target": t, "approved_rev": a.get("rev"), "current_rev": cur})
    return out


def card(project: Path) -> dict:
    """project.json merged with what the desk computes: stage pipeline, badge, pending count."""
    meta = read_json(project / "project.json", {}) or {}
    state = read_json(project / "direction.json", {}) or {}
    fb = read_feedback(project)
    cursor = int(state.get("feedback_cursor", 0) or 0)
    pending = max(0, len(fb) - cursor)
    stage = state.get("stage") or meta.get("stage") or "mode"
    cur = STAGES.index(stage) if stage in STAGES else 0
    status = meta.get("status") if meta.get("status") in STATUSES else "active"
    agent = meta.get("agent") or {}
    if pending:
        badge = "pending"
    elif agent.get("state") == "working" or (state.get("render") or {}).get("status") == "rendering":
        badge = "working"
    elif status == "active":
        badge = "waiting"
    else:
        badge = None
    open_cards = sum(1 for c in read_cards(project) if c.get("status") == "open")
    if open_cards and status != "archived":
        badge = "cards"
    stale = stale_approvals(state)
    stale_stages = {STAGE_OF_TARGET.get(s["target"]) for s in stale}
    approved = {STAGE_OF_TARGET.get(a.get("target")) for a in state.get("approvals", [])}
    pipeline = []
    for i, s in enumerate(STAGES):
        if s in stale_stages:
            st = "stale"
        elif status == "done" or i < cur or (i > cur and s in approved):
            st = "done"
        elif i == cur:
            st = "waiting" if badge in ("waiting", "cards") else "current"
        else:
            st = "todo"
        pipeline.append(st)
    mtimes = [meta.get("updated") or ""]
    for f in ("direction.json", "feedback.jsonl", "project.json"):
        try:
            mtimes.append(datetime.fromtimestamp((project / f).stat().st_mtime, timezone.utc).astimezone().isoformat(timespec="seconds"))
        except FileNotFoundError:
            pass
    title = (state.get("project") or {}).get("title")

    def exists(rel):
        return rel if rel and (project / rel).is_file() else None

    return {
        "slug": project.name,
        "name": meta.get("name") or title or project.name,
        "client": meta.get("client") or "",
        "goal": meta.get("goal") or "",
        "status": status,
        "stage": stage,
        "stage_index": cur,
        "pipeline": pipeline,
        "tags": meta.get("tags") or [],
        "delivery": meta.get("delivery") or "",
        "created": meta.get("created") or (state.get("project") or {}).get("created"),
        "updated": max(mtimes, key=lambda s: _ts(s)),
        "poster": exists(meta.get("poster")),
        "preview": exists(meta.get("preview")),
        "agent": agent,
        "badge": badge,
        "pending": pending,
        "open_cards": open_cards,
        "feedback_total": len(fb),
        "revision": state.get("revision"),
        "stale": stale,
    }


def _ts(s: str) -> float:
    try:
        return datetime.fromisoformat(s).timestamp()
    except (TypeError, ValueError):
        return 0.0


class Root:
    """A projects root, or a single project dir shown as a root with one project."""

    def __init__(self, path: Path):
        self.path = path
        self.single = (path / "direction.json").exists()

    def projects(self) -> list:
        if self.single:
            return [self.path]
        if not self.path.is_dir():
            return []
        out = []
        for d in sorted(self.path.iterdir()):
            if d.is_dir() and not d.name.startswith((".", "_")) and SLUG_RE.match(d.name) and \
                    ((d / "project.json").exists() or (d / "direction.json").exists()):
                out.append(d)
        return out

    def get(self, slug: str):
        if self.single:
            return self.path if slug == self.path.name else None
        if not SLUG_RE.match(slug or ""):
            return None
        d = self.path / slug
        return d if d.is_dir() and ((d / "project.json").exists() or (d / "direction.json").exists()) else None

    def resolve(self, arg: str) -> Path:
        p = Path(arg).expanduser()
        if p.is_dir() and (p / "direction.json").exists():
            return p.absolute()
        d = self.get(arg)
        if d is None:
            sys.exit(f"no project {arg!r} under {self.path}")
        return d

    def version(self) -> str:
        h = hashlib.sha1()
        for d in self.projects():
            h.update(d.name.encode())
            for f in ("project.json", "direction.json", "feedback.jsonl", "cards.jsonl"):
                try:
                    st = (d / f).stat()
                    h.update(f"{f}:{st.st_mtime_ns}:{st.st_size};".encode())
                except FileNotFoundError:
                    pass
        return h.hexdigest()[:16]

    def inbox(self) -> list:
        """Every unread desk action across projects, oldest first per project."""
        out = []
        for d in self.projects():
            state = read_json(d / "direction.json", {}) or {}
            cursor = int(state.get("feedback_cursor", 0) or 0)
            for rec in read_feedback(d)[cursor:]:
                out.append({**rec, "project": d.name})
        return out

    def create(self, name: str, client: str = "", goal: str = "", aspect=None, length_s=30.0, by="operator") -> Path:
        """New project folder: project.json, a stage-0 direction.json, a new-project event."""
        if self.single:
            raise ValueError("the desk serves a single project; start it on a projects root to create projects")
        name = (name or "").strip()
        if not name or len(name) > 80:
            raise ValueError("name: 1 to 80 characters")
        client, goal = (client or "").strip()[:80], (goal or "").strip()[:240]
        aspect = [a for a in (aspect or ["16:9"]) if a in ASPECTS] or ["16:9"]
        try:
            length_s = float(length_s)
        except (TypeError, ValueError):
            raise ValueError("length must be a number of seconds")
        if not 1 <= length_s <= 600:
            raise ValueError("length: 1 to 600 s")
        self.path.mkdir(parents=True, exist_ok=True)
        with LOCK:
            base = slugify(name)
            slug, n = base, 2
            while (self.path / slug).exists():
                slug = f"{base[:44]}-{n}"
                n += 1
            d = self.path / slug
            (d / "stills").mkdir(parents=True)
            ts = now_iso()
            length_txt = f"{length_s:g} s"
            write_json(d / "project.json", {
                "schema": "reel-project/1", "slug": slug, "name": name, "client": client, "goal": goal,
                "created": ts, "updated": ts, "status": "active", "stage": "mode",
                "poster": None, "preview": None, "tags": [],
                "delivery": f"{length_txt}, {' and '.join(aspect)}",
                "agent": {"state": "idle", "note": "New project: waiting for the agent to start the brief.", "at": ts},
            })
            write_json(d / "direction.json", stage0(slug, name, goal, aspect, length_s, ts, by))
            (d / ".gitignore").write_text("out/\n")
            append_feedback(d, {
                "ts": ts, "seen_revision": 0, "stage": "mode", "by": by, "action": "new-project",
                "spec": {"name": name, "client": client, "goal": goal, "aspect": aspect, "length_s": length_s},
                "text": f"New project \"{name}\"{' for ' + client if client else ''}: {goal.rstrip('.') or 'no goal yet'}. "
                        f"{length_txt}, {', '.join(aspect)}. Start the brief dialogue.",
            })
        return d

    def set_archived(self, d: Path, archived: bool, by="operator") -> dict:
        with LOCK:
            meta = read_json(d / "project.json", None)
            if meta is None:
                state = read_json(d / "direction.json", {}) or {}
                meta = {"schema": "reel-project/1", "slug": d.name,
                        "name": (state.get("project") or {}).get("title") or d.name, "status": "active"}
            before = meta.get("status") if meta.get("status") in STATUSES else "active"
            if archived and before != "archived":
                meta["status_before_archive"] = before
                meta["status"] = "archived"
            elif not archived and before == "archived":
                meta["status"] = meta.pop("status_before_archive", "active")
            meta["updated"] = now_iso()
            write_json(d / "project.json", meta)
            state = read_json(d / "direction.json", {}) or {}
            seq = append_feedback(d, {
                "ts": meta["updated"], "seen_revision": state.get("revision"), "stage": state.get("stage"),
                "by": by, "action": "archive" if archived else "unarchive",
                "text": f"status {before} -> {meta['status']}",
            })
        return {"ok": True, "status": meta["status"], "seq": seq}


def stage0(slug, name, goal, aspect, length_s, ts, by) -> dict:
    """The direction.json a new project starts with: stage mode, everything open."""
    open_ = lambda v="": {"value": v, "source": "open"}
    return {
        "schema": "reel-director/1",
        "project": {"id": slug, "title": name, "source": None, "mode": "deliberate", "created": ts,
                    "operator": by, "notes": goal},
        "stage": "mode", "revision": 0, "feedback_cursor": 0,
        "brief": {
            "rev": 0, "audience": open_(), "outcome": open_(goal),
            "content": {"must": [], "proposed": [], "source": "open"},
            "envelope": {"length_s": length_s, "aspect": aspect, "fps": 30, "sound": "", "source": "answered"},
            "taste": {"value": "", "references": [], "source": "open"},
            "priority": {"value": "", "beat": None, "source": "open"},
            "open": [], "dialogue": [],
        },
        "locks": [], "directions": [],
        "chosen_direction": {"base": None, "mix": [], "rev": 0, "note": ""},
        "dramaturgy": {"rev": 0, "shape": "", "hero": None, "arc": [],
                       "music": {"src": None, "bpm": None, "bar_s": None, "beat_s": None, "notation": "", "hits": []}},
        "beats": [], "storyboard": {"rev": 0, "contact_sheet": None, "changes": []},
        "variants": [], "notes": [], "approvals": [], "checks": [],
        "render": {"status": "not-started", "planned": "", "outputs": []},
        "history": [{"rev": 0, "at": ts, "by": by, "action": "new-project", "detail": "created from the desk"}],
    }


class Desk(BaseHTTPRequestHandler):
    root: Root = None
    server_version = "desk/3"

    def log_message(self, fmt, *args):
        if os.environ.get("DESK_LOG"):
            super().log_message(fmt, *args)

    def _send(self, code, body: bytes, ctype="application/json", extra=None):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def _json(self, obj, code=200):
        self._send(code, json.dumps(obj, ensure_ascii=False).encode())

    def _err(self, code, msg):
        self._json({"error": msg}, code)

    def do_HEAD(self):
        self.do_GET()

    def do_GET(self):
        url = urlparse(self.path)
        path = url.path
        if path in ("/", "/index.html") or path.startswith("/p/"):
            return self._send(200, (HERE / "index.html").read_bytes(), "text/html; charset=utf-8")
        if path.startswith("/static/") and path[8:] in STATIC:
            return self._send(200, (HERE / path[8:]).read_bytes(), STATIC[path[8:]])
        if path == "/api/projects":
            return self._json({
                "projects": [card(d) for d in self.root.projects()],
                "version": self.root.version(),
                "single": self.root.single,
                "root": str(self.root.path),
                "stages": STAGES,
            })
        if path == "/api/version":
            q = parse_qs(url.query)
            out = {"home": self.root.version()}
            d = self.root.get((q.get("p") or [""])[0])
            if d:
                out["project"] = version(d)
            return self._json(out)
        if path == "/api/inbox":
            items = self.root.inbox()
            return self._json({"pending": items, "count": len(items)})
        m = re.match(r"^/api/p/([^/]+)/state$", path)
        if m:
            d = self.root.get(m.group(1))
            if not d:
                return self._err(404, "no such project")
            try:
                state = load_state(d)
            except (OSError, json.JSONDecodeError) as e:
                return self._err(500, f"direction.json: {e}")
            fb = read_feedback(d)
            cursor = int(state.get("feedback_cursor", 0) or 0)
            return self._json({
                "card": card(d),
                "state": state,
                "version": version(d),
                "media_version": version(d, media_only=True),
                "stale": stale_approvals(state),
                "feedback": fb[-40:],
                "feedback_total": len(fb),
                "feedback_unread": max(0, len(fb) - cursor),
                "feedback_cursor": cursor,
                "cards": read_cards(d)[-200:],
            })
        m = re.match(r"^/api/p/([^/]+)/cards$", path)
        if m:
            d = self.root.get(m.group(1))
            if not d:
                return self._err(404, "no such project")
            q = parse_qs(url.query)
            cards = read_cards(d, history=bool(q.get("history")))
            st = (q.get("status") or [""])[0]
            if st:
                cards = [c for c in cards if c.get("status") == st]
            return self._json({"cards": cards, "version": version(d),
                               "open": sum(1 for c in read_cards(d) if c.get("status") == "open")})
        m = re.match(r"^/files/([^/]+)/(.+)$", path)
        if m:
            d = self.root.get(m.group(1))
            if not d:
                return self._err(404, "no such project")
            return self._file(d, unquote(m.group(2)))
        self._err(404, "not found")

    def _file(self, project: Path, rel: str):
        target = (project / rel)
        try:
            real = target.resolve()
        except OSError:
            return self._err(404, "not found")
        # Only paths under the project dir (symlinked asset folders inside it are followed).
        if not str(target.absolute()).startswith(str(project.absolute()) + os.sep) or ".." in Path(rel).parts:
            return self._err(403, "outside project")
        if not real.is_file():
            return self._err(404, "not found")
        size = real.stat().st_size
        ctype = mimetypes.guess_type(real.name)[0] or "application/octet-stream"
        rng = self.headers.get("Range")
        start, end = 0, size - 1
        code = 200
        if rng and rng.startswith("bytes="):
            a, _, b = rng[6:].split(",")[0].partition("-")
            try:
                if a:
                    start = int(a)
                    end = int(b) if b else size - 1
                else:
                    start = max(0, size - int(b))
                end = min(end, size - 1)
                if start > end:
                    raise ValueError
                code = 206
            except ValueError:
                return self._send(416, b"", extra={"Content-Range": f"bytes */{size}"})
        length = end - start + 1
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(length))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Cache-Control", "no-cache")
        if code == 206:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        if self.command == "HEAD":
            return
        with open(real, "rb") as f:
            f.seek(start)
            left = length
            try:
                while left > 0:
                    chunk = f.read(min(1 << 16, left))
                    if not chunk:
                        break
                    self.wfile.write(chunk)
                    left -= len(chunk)
            except (BrokenPipeError, ConnectionResetError):
                pass

    def _body(self):
        n = int(self.headers.get("Content-Length", "0"))
        if n > 64 * 1024:
            raise ValueError("too large")
        obj = json.loads(self.rfile.read(n) or b"{}")
        if not isinstance(obj, dict):
            raise ValueError("expected a JSON object")
        return obj

    def _local(self) -> bool:
        """Loopback only. A request to another bound address (for example a VPN interface), even from
        this machine, arrives with that address as its source and is refused."""
        return self.client_address[0] in ("127.0.0.1", "::1") and \
            self.server.server_address[0] in ("127.0.0.1", "::1")

    def do_POST(self):
        path = urlparse(self.path).path
        try:
            obj = self._body()
        except (ValueError, json.JSONDecodeError) as e:
            return self._err(400, f"invalid request: {e}")
        if path == "/api/projects":
            try:
                d = self.root.create(obj.get("name"), obj.get("client"), obj.get("goal"),
                                     obj.get("aspect"), obj.get("length_s", 30))
            except ValueError as e:
                return self._err(400, str(e))
            return self._json({"ok": True, "slug": d.name, "card": card(d)})
        m = re.match(r"^/api/p/([^/]+)/cards(?:/([^/]+)/(answer|withdraw))?$", path)
        if m:
            d = self.root.get(m.group(1))
            if not d:
                return self._err(404, "no such project")
            if m.group(3) == "answer":
                try:
                    return self._json(answer_card(d, unquote(m.group(2)), obj))
                except LookupError as e:
                    return self._err(404, str(e))
                except ValueError as e:
                    return self._err(409 if "changed" in str(e) or "not open" in str(e) else 400, str(e))
            # posting and withdrawing cards is the agent's job: only from this machine's loopback
            if not self._local():
                return self._err(403, "cards are posted by the agent from localhost only")
            try:
                if m.group(3) == "withdraw":
                    return self._json({"ok": True, "card": withdraw_card(d, unquote(m.group(2)), obj.get("note", ""))})
                return self._json({"ok": True, "card": post_card(d, obj.get("card", obj))})
            except ValueError as e:
                return self._err(400, str(e))
        m = re.match(r"^/api/p/([^/]+)/(feedback|archive|brief)$", path)
        if not m:
            return self._err(404, "not found")
        d = self.root.get(m.group(1))
        if not d:
            return self._err(404, "no such project")
        if m.group(2) == "archive":
            return self._json(self.root.set_archived(d, bool(obj.get("archived", True))))
        if m.group(2) == "brief":
            try:
                return self._json(edit_brief(d, obj.get("field"), obj.get("value")))
            except ValueError as e:
                return self._err(400, str(e))
        if obj.get("action") not in ACTIONS:
            return self._err(400, f"action must be one of {sorted(ACTIONS)}")
        try:
            if "context" in obj:
                obj["context"] = _context(obj["context"])
                if not obj["context"]:
                    obj.pop("context")
        except ValueError as e:
            return self._err(400, str(e))
        state = read_json(d / "direction.json", {}) or {}
        rec = {
            "ts": now_iso(),
            "seen_revision": state.get("revision"),
            "stage": obj.pop("stage", None) or state.get("stage"),
            "by": "operator",
            **obj,
        }
        with LOCK:
            seq = append_feedback(d, rec)
        self._json({"ok": True, "seq": seq})


BRIEF_FIELDS = {"audience": "audience", "outcome": "outcome", "content": "required content", "sound": "sound",
                "taste": "taste anchor", "priority": "priority moment", "length_s": "length", "aspect": "formats"}


def edit_brief(project: Path, field, value) -> dict:
    """The operator edits one brief field in the desk. The desk writes it straight into
    direction.json (source "answered", brief rev + 1, so earlier approvals go stale) and logs
    an "edit" line to feedback.jsonl so the agent sees what changed."""
    if field not in BRIEF_FIELDS:
        raise ValueError(f"field must be one of {sorted(BRIEF_FIELDS)}")
    if not isinstance(value, str) or len(value) > 4000:
        raise ValueError("value must be text up to 4000 characters")
    value = value.strip()
    ts = now_iso()
    with LOCK:
        state = load_state(project)
        b = state.setdefault("brief", {})
        if field == "content":
            items = [x.strip(" -*\t") for x in value.splitlines() if x.strip(" -*\t")]
            b["content"] = {**b.get("content", {}), "must": items, "source": "answered"}
        elif field in ("sound", "length_s", "aspect"):
            env = b.setdefault("envelope", {})
            if field == "sound":
                env["sound"] = value
            elif field == "length_s":
                try:
                    secs = float(value.rstrip("s ").replace(",", "."))
                except ValueError:
                    raise ValueError("length must be a number of seconds")
                if not 1 <= secs <= 600:
                    raise ValueError("length must be 1 to 600 s")
                env["length_s"] = secs
            else:
                aspects = [a.strip() for a in re.split(r"[,\s]+", value) if a.strip()]
                if not aspects or not all(re.fullmatch(r"\d{1,2}:\d{1,2}", a) for a in aspects):
                    raise ValueError("aspect must look like 16:9 or 16:9, 9:16")
                env["aspect"] = aspects
            env["source"] = "answered"
        else:
            b[field] = {**b.get(field, {}), "value": value, "source": "answered"}
        b["rev"] = int(b.get("rev", 0)) + 1
        state["revision"] = int(state.get("revision", 0)) + 1
        b.setdefault("dialogue", []).append({"turn": len(b["dialogue"]) + 1, "by": "operator",
                                             "text": f"Edited {BRIEF_FIELDS[field]} in the desk: {value}"})
        state.setdefault("history", []).append({"rev": state["revision"], "at": ts, "by": "operator",
                                                "action": "edit-brief", "detail": f"{field}: {value[:200]}"})
        write_json(project / "direction.json", state)
        card_p = project / "project.json"
        pj = read_json(card_p, None)
        if isinstance(pj, dict):
            pj["updated"] = ts
            env = b.get("envelope", {})
            if field in ("length_s", "aspect") and env.get("length_s"):
                pj["delivery"] = f"{env['length_s']:g} s, {', '.join(env.get('aspect') or [])}"
            write_json(card_p, pj)
        seq = append_feedback(project, {"ts": ts, "seen_revision": state["revision"] - 1, "stage": "brief",
                                        "by": "operator", "action": "edit", "target": "brief",
                                        "field": field, "value": value, "rev": b["rev"]})
    return {"ok": True, "seq": seq, "rev": b["rev"]}


def serve(root: Root, port: int, hosts: list):
    if not root.single and not root.path.is_dir():
        sys.exit(f"no projects root at {root.path} (create it, or pass a project dir)")
    Desk.root = root
    servers = []
    for host in hosts:
        if host in ("0.0.0.0", "::", ""):
            sys.exit("refusing to bind a wildcard address; pass explicit --host values")
        try:
            srv = ThreadingHTTPServer((host, port), Desk)
        except OSError as e:
            print(f"skip {host}:{port}: {e}", file=sys.stderr)
            continue
        servers.append(srv)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        what = f"project {root.path.name}" if root.single else f"{len(root.projects())} projects in {root.path}"
        print(f"desk: http://{host}:{port}/  ({what})", flush=True)
    if not servers:
        sys.exit("could not bind any address")
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        pass
    finally:
        for s in servers:
            s.shutdown()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--root", help=f"projects root (default {DEFAULT_ROOT}, or $AGENTFX_PROJECTS)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("serve", parents=[common])
    s.add_argument("path", nargs="?", help="projects root or a single project dir")
    s.add_argument("--port", type=int, default=8771)
    s.add_argument("--host", action="append", help="bind address (repeatable); default 127.0.0.1 only. Never bind a public interface: the desk has no authentication")
    sub.add_parser("list", parents=[common])
    i = sub.add_parser("inbox", parents=[common])
    i.add_argument("--json", action="store_true", help="one JSON array instead of JSON lines")
    n = sub.add_parser("new", parents=[common])
    n.add_argument("name")
    n.add_argument("--client", default="")
    n.add_argument("--goal", default="")
    n.add_argument("--aspect", action="append", choices=ASPECTS)
    n.add_argument("--length", type=float, default=30.0)
    n.add_argument("--by", default="agent")
    for name in ("status", "feedback", "ack", "cards", "post"):
        p = sub.add_parser(name, parents=[common])
        p.add_argument("project", help="slug under the root, or a project dir")
        if name in ("feedback", "cards"):
            p.add_argument("--all", action="store_true")
        if name == "post":
            p.add_argument("card", help="card JSON file, or - for stdin")
    a = ap.parse_args()
    root = Root(Path(getattr(a, "path", None) or a.root or DEFAULT_ROOT).expanduser().absolute())

    if a.cmd == "serve":
        return serve(root, a.port, a.host or DEFAULT_HOSTS)
    if a.cmd == "list":
        for d in root.projects():
            c = card(d)
            print(f"{c['slug']:<28} {c['status']:<9} stage {c['stage_index']} {c['stage']:<11} "
                  f"{c['pending']} pending  {c['client']}: {c['name']}")
        return
    if a.cmd == "inbox":
        items = root.inbox()
        if a.json:
            print(json.dumps(items, ensure_ascii=False, indent=2))
        else:
            for rec in items:
                print(json.dumps(rec, ensure_ascii=False))
        return
    if a.cmd == "new":
        try:
            d = root.create(a.name, a.client, a.goal, a.aspect, a.length, by=a.by)
        except ValueError as e:
            sys.exit(str(e))
        print(d)
        return

    project = root.resolve(a.project)
    if a.cmd == "cards":
        for c in read_cards(project):
            if a.all or c.get("status") == "open":
                print(json.dumps(c, ensure_ascii=False))
        return
    if a.cmd == "post":
        raw = sys.stdin.read() if a.card == "-" else Path(a.card).read_text()
        try:
            print(json.dumps(post_card(project, json.loads(raw)), ensure_ascii=False))
        except (ValueError, json.JSONDecodeError) as e:
            sys.exit(f"card not posted: {e}")
        return
    state = load_state(project)
    fb = read_feedback(project)
    cursor = int(state.get("feedback_cursor", 0) or 0)
    if a.cmd == "status":
        print(f"{state['project']['title']}: stage {state.get('stage')}, revision {state.get('revision')}")
        for st in stale_approvals(state):
            print(f"  stale approval: {st['target']} approved at rev {st['approved_rev']}, now rev {st['current_rev']}")
        print(f"  feedback: {len(fb)} lines, {max(0, len(fb) - cursor)} unread (cursor {cursor})")
    elif a.cmd == "feedback":
        for rec in (fb if a.all else fb[cursor:]):
            print(json.dumps(rec, ensure_ascii=False))
    elif a.cmd == "ack":
        state["feedback_cursor"] = len(fb)
        write_json(project / "direction.json", state)
        print(f"feedback_cursor = {len(fb)}")


if __name__ == "__main__":
    main()
