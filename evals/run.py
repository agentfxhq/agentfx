#!/usr/bin/env python3
"""AgentFX agent eval harness: how good is an agent's FIRST video with AgentFX?

For each case (evals/cases/*.yaml) a headless Claude Code session (`claude -p`) gets the request as
a user would type it, with the AgentFX skill installed in the project and the AgentFX MCP server and
CLI available. It works in a throwaway directory and must leave `film.reel` there. Then the harness
scores the result without the agent's help:

  hard checks   lint clean in every requested format (measured), renders, duration within +-15 %,
                the must-have copy is present in the source
  judge         a vision model looks at a contact sheet of the render (and the source, plus the
                diff for edit cases) and checks the case's must-have list, gives a quality score
                1-10 and says whether a non-expert would accept it as the first draft

usage:
  python3 evals/run.py run     [--cases a,b|glob] [--tags t1,t2] [--run-id ID] [--model sonnet] [--judge-model sonnet]
                               [--parallel 1] [--max-turns 40] [--timeout 900] [--max-load 10] [--skip-done] [--mode both|cli|mcp] [--work DIR]
  python3 evals/run.py score   <run-id> [--cases ...] [--judge-model sonnet] [--no-judge]   (re-score kept workdirs)
  python3 evals/run.py summary <run-id>
  python3 evals/run.py compare <run-a> <run-b>
  python3 evals/run.py list

Results: evals/results/<run-id>/cases/<case>.json, summary.md, summary.json. Workdirs (scripts,
renders, sheets, transcripts) live in --work (default $AGENTFX_EVAL_WORK or <tmp>/agentfx-evals) and are
not committed. Agent and judge run through the local `claude` CLI and its login (no API key needed).
"""
import argparse
import concurrent.futures as cf
import fnmatch
import glob
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
EVALS = ROOT / "evals"
CASES = EVALS / "cases"
RESULTS = EVALS / "results"
LOCK = threading.Lock()  # one scorer render at a time across workers

ENV_NOTE = """Environment for this session: AgentFX is installed. The `agentfx` CLI is on PATH; its data
directory (docs/SPEC.md, examples/) is $AGENTFX_HOME = {home}. The AgentFX MCP server is connected
(tools mcp__agentfx__*). The AgentFX skill is installed. This is a non-interactive session: nobody will
answer questions, so make sensible choices yourself and finish the job. Work in the current directory.
Leave the final script at ./film.reel (edit it in place if it already exists) and render the final
video to ./out/ (one .mp4 per requested format)."""

JUDGE_SCHEMA = {
    "type": "object",
    "properties": {
        "checklist": {"type": "array", "items": {"type": "object", "properties": {
            "item": {"type": "string"}, "pass": {"type": "boolean"}, "evidence": {"type": "string"}},
            "required": ["item", "pass", "evidence"]}},
        "quality": {"type": "integer", "description": "1-10 visual and motion quality"},
        "accept_first_try": {"type": "boolean"},
        "problems": {"type": "array", "items": {"type": "string"}},
        "verdict": {"type": "string"},
    },
    "required": ["checklist", "quality", "accept_first_try", "problems", "verdict"],
}

JUDGE_PROMPT = """You judge a short video an AI agent made with AgentFX for a non-expert user. Be strict but fair.

The user's request:
<<<
{prompt}
>>>
Requested format(s): {formats}. Requested length: {duration}. Rendered: {rendered}.

Read these images with the Read tool (contact sheets: frames in time order, labelled with the time):
{sheets}
{extra}
The final Reel Script source:
<<<
{source}
>>>

Tasks:
1. Checklist: for each item below decide pass/fail from what you SEE in the frames (use the source only
   to understand motion between frames, never as a substitute for seeing it). Evidence: one short line.
{items}
2. quality 1-10: a professional motion designer's view of the frames as a short social/marketing video
   (composition, hierarchy, type size, contrast, colour, use of the frame, motion inferred from frame to
   frame, polish). 5 = acceptable but generic, 8 = would post it, 3 = visibly broken (clipped or
   overlapping text, empty frames, illegible).
3. accept_first_try: would the user accept this as the first draft without asking for changes?
4. problems: the concrete problems a user would complain about, most important first (max 5).
Return only the JSON."""


def load_cases(sel=None, tags=None):
    cases = []
    for p in sorted(CASES.glob("*.yaml")):
        c = yaml.safe_load(p.read_text())
        c.setdefault("files", {})
        c.setdefault("must_text", [])
        c.setdefault("must_haves", [])
        cases.append(c)
    if sel:
        pats = [s.strip() for s in sel.split(",") if s.strip()]
        cases = [c for c in cases if any(fnmatch.fnmatch(c["id"], p) for p in pats)]
    if tags:
        ts = set(t.strip() for t in tags.split(","))
        cases = [c for c in cases if ts & set(c.get("tags", []))]
    return cases


def agentfx_bin():
    for c in (os.environ.get("AGENTFX_BIN"), ROOT / "target" / "release" / "agentfx"):
        if c and Path(c).is_file():
            return str(c)
    w = shutil.which("agentfx")
    if w:
        return w
    sys.exit("run.py: no agentfx binary (cargo build --release, or set AGENTFX_BIN)")


def tool_env(work_root):
    """Environment for the agent and the scorer: our CLI first on PATH, our data dir, no TTS."""
    bindir = Path(work_root) / "_bin"
    bindir.mkdir(parents=True, exist_ok=True)
    real = Path(agentfx_bin()).resolve()
    for name in ("agentfx", "agentfx-render"):
        link = bindir / name
        if link.is_symlink() or link.exists():
            link.unlink()
    # the agent's and the scorer's `agentfx`: heavy subcommands (render, sheet, storyboard; single frames are light)
    # wait for the machine-wide heavy-job lock, so a shared box runs one heavy job at a time
    lock = os.environ.get("AGENTFX_HEAVY_LOCK", "/tmp/claude-1000/agentfx-heavy.lock")
    (bindir / "agentfx").write_text(
        "#!/bin/sh\n"
        f"export AGENTFX_RENDERER={real.parent / 'agentfx-render'}\n"
        "case \"$1\" in\n"
        f"  render|sheet|storyboard) exec flock {lock} {real} \"$@\" ;;\n"
        f"  *) exec {real} \"$@\" ;;\n"
        "esac\n")
    (bindir / "agentfx").chmod(0o755)
    env = dict(os.environ)
    for k in list(env):
        if k == "CLAUDECODE" or k.startswith("CLAUDE_CODE_") or k.startswith("CLAUDE_AGENT"):
            env.pop(k)
    env["PATH"] = f"{bindir}:{env.get('PATH', '')}"
    env["AGENTFX_HOME"] = str(ROOT)
    env["AGENTFX_BIN"] = str(bindir / "agentfx")
    env["AGENTFX_TTS"] = "none"
    env["AGENTFX_CACHE"] = str(Path(work_root) / "_cache")
    return env


def prepare_workdir(case, wd, mode):
    if wd.exists():
        shutil.rmtree(wd)
    (wd / ".claude" / "skills").mkdir(parents=True)
    if mode in ("both", "cli", "mcp"):
        shutil.copytree(ROOT / "skill" / "agentfx", wd / ".claude" / "skills" / "agentfx")
    for dest, src in case["files"].items():
        d = wd / dest
        d.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(ROOT / src, d)
    (wd / "out").mkdir()


def mcp_config(wd, env):
    cfg = {"mcpServers": {"agentfx": {"command": sys.executable, "args": [str(ROOT / "mcp" / "server.py")],
                                       "env": {k: env[k] for k in ("AGENTFX_BIN", "AGENTFX_HOME", "AGENTFX_TTS", "AGENTFX_CACHE", "PATH")}}}}
    p = wd / ".mcp-eval.json"
    p.write_text(json.dumps(cfg))
    return p


def run_agent(case, wd, args, env):
    tools = ["Bash", "Read", "Write", "Edit", "Glob", "Grep", "Skill"]
    cmd = ["claude", "-p", case["prompt"], "--output-format", "stream-json", "--verbose",
           "--model", args.model, "--max-turns", str(args.max_turns),
           "--setting-sources", "project", "--strict-mcp-config", "--no-session-persistence",
           "--permission-mode", "dontAsk",
           "--append-system-prompt", ENV_NOTE.format(home=ROOT)]
    if args.mode in ("both", "mcp"):
        cmd += ["--mcp-config", str(mcp_config(wd, env))]
        tools.append("mcp__agentfx")
    if args.mode == "mcp":
        tools.remove("Bash")
        tools.append("Bash(ffmpeg *)")
    cmd += ["--allowedTools", " ".join(tools)]
    if args.effort:
        cmd += ["--effort", args.effort]
    t0 = time.time()
    log = wd / "_transcript.jsonl"
    with open(log, "w") as f:
        try:
            p = subprocess.run(cmd, cwd=wd, env=env, stdin=subprocess.DEVNULL, stdout=f, stderr=subprocess.PIPE,
                               text=True, timeout=args.timeout)
            rc, err = p.returncode, p.stderr[-2000:]
        except subprocess.TimeoutExpired:
            rc, err = "timeout", ""
    orphans = reap(wd)
    return {"rc": rc, "stderr": err, "wall_s": round(time.time() - t0, 1), "orphans_killed": orphans}


def reap(wd):
    """Kill what the agent left running in its workdir (backgrounded renders outlive `claude -p`)."""
    killed = []
    wd = str(Path(wd).resolve())
    for pid in os.listdir("/proc"):
        if not pid.isdigit() or int(pid) == os.getpid():
            continue
        try:
            cwd = os.readlink(f"/proc/{pid}/cwd")
        except OSError:
            continue
        if cwd == wd or cwd.startswith(wd + "/"):
            try:
                cmd = open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")[:120]
                os.kill(int(pid), 15)
                killed.append(cmd)
            except OSError:
                pass
    return killed


LINT_RE = re.compile(r'"code":\s*"([EW]_[A-Z0-9_]+)"')


def parse_transcript(path):
    """Turns, tool calls, lint rounds and the errors seen, cost and tokens from a stream-json log."""
    m = {"num_turns": None, "cost_usd": None, "tokens": {}, "tool_calls": {}, "lint_calls": 0,
         "lint_errors_seen": 0, "first_lint_errors": None, "error_codes": {}, "warning_codes": {},
         "renders": 0, "frames": 0, "sheets": 0, "skill_loaded": False, "result": None, "spec_reads": 0,
         "tool_errors": 0, "is_error": None}
    pending = {}
    if not Path(path).exists():
        return m
    for line in open(path, errors="replace"):
        try:
            ev = json.loads(line)
        except Exception:
            continue
        t = ev.get("type")
        if t == "assistant":
            for b in ev.get("message", {}).get("content", []) or []:
                if b.get("type") != "tool_use":
                    continue
                name, inp = b.get("name", ""), b.get("input") or {}
                m["tool_calls"][name] = m["tool_calls"].get(name, 0) + 1
                cmdline = inp.get("command", "") if name == "Bash" else ""
                kind = None
                if name.endswith("agentfx_lint") or re.search(r"\bagentfx\s+lint\b", cmdline):
                    kind = "lint"
                elif name.endswith("agentfx_render") or re.search(r"\bagentfx\s+render\b", cmdline):
                    kind = "render"
                elif name.endswith("agentfx_frame") or re.search(r"\bagentfx\s+frame\b", cmdline):
                    kind = "frame"
                elif name.endswith("agentfx_sheet") or re.search(r"\bagentfx\s+sheet\b", cmdline):
                    kind = "sheet"
                if name == "Skill":
                    m["skill_loaded"] = True
                if name.endswith("agentfx_spec") or "SPEC.md" in json.dumps(inp):
                    m["spec_reads"] += 1
                if kind:
                    m[{"lint": "lint_calls", "render": "renders", "frame": "frames", "sheet": "sheets"}[kind]] += 1
                pending[b.get("id")] = kind
        elif t == "user":
            content = ev.get("message", {}).get("content", [])
            if not isinstance(content, list):
                continue
            for b in content:
                if b.get("type") != "tool_result":
                    continue
                if b.get("is_error"):
                    m["tool_errors"] += 1
                kind = pending.get(b.get("tool_use_id"))
                if kind != "lint":
                    continue
                c = b.get("content")
                text = c if isinstance(c, str) else " ".join(x.get("text", "") for x in (c or []) if isinstance(x, dict))
                codes = LINT_RE.findall(text)
                errs = [x for x in codes if x.startswith("E_")]
                if m["first_lint_errors"] is None:
                    m["first_lint_errors"] = len(errs)
                m["lint_errors_seen"] += len(errs)
                for x in codes:
                    bucket = m["error_codes"] if x.startswith("E_") else m["warning_codes"]
                    bucket[x] = bucket.get(x, 0) + 1
        elif t == "result":
            m["num_turns"] = ev.get("num_turns")
            m["cost_usd"] = ev.get("total_cost_usd")
            u = ev.get("usage") or {}
            m["tokens"] = {k: u.get(k) for k in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")}
            m["result"] = (ev.get("result") or "")[-1500:]
            m["is_error"] = ev.get("is_error")
            m["subtype"] = ev.get("subtype")
    return m


# ---------------------------------------------------------------- scoring

def afx(args, cwd, env, timeout=600):
    r = subprocess.run(["nice", "-n", "19", env["AGENTFX_BIN"]] + args, cwd=cwd, env=env, capture_output=True,
                       text=True, timeout=timeout)
    return r.returncode, r.stdout, r.stderr


def find_reel(wd):
    p = wd / "film.reel"
    if p.exists():
        return p
    cands = [x for x in wd.rglob("*.reel") if "_score" not in x.parts and ".claude" not in x.parts]
    return max(cands, key=lambda x: x.stat().st_mtime) if cands else None


def probe(mp4):
    r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
                        "stream=width,height:format=duration", "-of", "json", str(mp4)], capture_output=True, text=True)
    try:
        d = json.loads(r.stdout)
        s = d["streams"][0]
        return {"w": s["width"], "h": s["height"], "dur": float(d["format"]["duration"])}
    except Exception:
        return None


def aspect_of(fmt):
    a, b = fmt.split(":")
    return float(a) / float(b)


def norm(s):
    s = re.sub(r"\*\*|[{}`]|\[\[|\]\]", "", s.lower())
    s = s.replace("’", "'").replace("\\\"", "\"")
    return re.sub(r"\s+", " ", s)


def text_check(source, must):
    """Each must-have string: every word of it appears in the source (kinetic type splits lines)."""
    src = norm(source)
    words_src = set(re.findall(r"[\w%.,'&+-]+", src))
    res = []
    for m in must:
        n = norm(m)
        ok = n in src or all((w in words_src or w in src) for w in re.findall(r"[\w%.,'&+-]+", n))
        res.append({"text": m, "pass": bool(ok)})
    return res


def load_critique():
    spec = importlib.util.spec_from_file_location("critique", ROOT / "tools" / "critique.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def make_sheet(mp4, out, n=12):
    cr = load_critique()
    info = probe(mp4)
    dur = info["dur"]
    portrait = info["h"] > info["w"]
    w = 300 if portrait else 480
    ts = [(k + 0.5) / n * dur for k in range(n)]
    frames = [cr.grab(str(mp4), t, w) for t in ts]
    sheet = cr.grid(frames, [f"{t:.2f}s" for t in ts], 6 if portrait else 4, w)
    sheet.save(out)
    return out


def pick_agent_mp4(wd, reel, fmt):
    want = aspect_of(fmt)
    best = None
    for p in list((wd / "out").rglob("*.mp4")) + list(wd.glob("*.mp4")):
        if "_score" in p.parts or p.stat().st_mtime + 1 < reel.stat().st_mtime:
            continue
        info = probe(p)
        if not info or abs(info["w"] / info["h"] - want) > 0.03:
            continue
        if best is None or p.stat().st_mtime > best[0].stat().st_mtime:
            best = (p, info)
    return best


def judge(case, wd, sheets, source, rendered, args, env):
    extra = ""
    orig = case["files"].get("film.reel")
    if orig:
        before = (ROOT / orig).read_text()
        extra = f"\nThis was an EDIT request. The script before the edit:\n<<<\n{before}\n>>>\n"
    items = "\n".join(f"   - {x}" for x in case["must_haves"])
    prompt = JUDGE_PROMPT.format(prompt=case["prompt"], formats=", ".join(case["formats"]), duration=f'about {case["duration_s"]} s' if case.get("duration_s") else "not stated",
                                 rendered=rendered, sheets="\n".join(f"- {s}" for s in sheets), extra=extra,
                                 source=source[:12000], items=items)
    cmd = ["claude", "-p", prompt, "--model", args.judge_model, "--output-format", "json", "--setting-sources", "project",
           "--strict-mcp-config", "--tools", "Read", "--allowedTools", "Read", "--permission-mode", "dontAsk",
           "--no-session-persistence", "--json-schema", json.dumps(JUDGE_SCHEMA)]
    jenv = {k: v for k, v in env.items()}
    for attempt in range(2):
        try:
            r = subprocess.run(cmd, cwd=wd / "_score", env=jenv, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=600)
            d = json.loads(r.stdout)
            out = d.get("structured_output")
            if out:
                out["cost_usd"] = d.get("total_cost_usd")
                out["model"] = args.judge_model
                return out
        except Exception as e:  # noqa: BLE001
            err = str(e)
    return {"error": "judge failed", "quality": None, "checklist": [], "accept_first_try": False, "problems": [], "verdict": ""}


def score_case(case, wd, args, env):
    s = {"reel": False, "lint": {}, "lint_clean": False, "renders": False, "agent_rendered": False,
         "duration_ok": False, "durations": {}, "text": [], "judge": None}
    sd = wd / "_score"
    sd.mkdir(exist_ok=True)
    reel = find_reel(wd)
    if not reel:
        s["score"] = 0.0
        return s
    s["reel"] = str(reel.relative_to(wd))
    source = reel.read_text(errors="replace")
    clean = True
    for fmt in case["formats"]:
        rc, out, err = afx(["lint", reel.name, "--measure", "--format", fmt], reel.parent, env)
        try:
            diags = json.loads(out)
        except Exception:
            diags = [{"code": "E_LINT_CRASH", "severity": "error", "message": (err or out)[-400:]}]
        errs = [d for d in diags if d.get("severity") == "error"]
        warns = [d for d in diags if d.get("severity") == "warning"]
        s["lint"][fmt] = {"errors": len(errs), "warnings": len(warns), "codes": sorted({d["code"] for d in errs + warns}),
                          "error_msgs": [f'{d["code"]}: {d.get("message", "")}'[:200] for d in errs[:6]]}
        clean = clean and not errs
    s["lint_clean"] = clean
    sheets = []
    rendered = []
    delivered = True
    total = None
    rc, out, err = afx(["timing", reel.name], reel.parent, env)
    try:
        total = float(json.loads(out)["timeline"]["total"])
    except Exception:  # noqa: BLE001
        pass
    for fmt in case["formats"]:
        tag = fmt.replace(":", "x")
        sh = sd / f"sheet-{tag}.png"
        got = pick_agent_mp4(wd, reel, fmt)
        if got and total and got[1]["dur"] < 0.9 * total - 0.2:  # killed or crashed mid-render
            s.setdefault("render_errors", []).append(f"{fmt}: incomplete agent render {got[0].name} ({got[1]['dur']:.1f} of {total:.1f} s)")
            got = None
        if got:  # the agent's own render of the final script: sheet from the video itself
            mp4, info = got
            s["durations"][fmt] = round(info["dur"], 2)
            rendered.append(f'{fmt}: agent render {info["dur"]:.1f} s ({info["w"]}x{info["h"]})')
            try:
                make_sheet(mp4, sh)
                sheets.append(str(sh))
            except Exception as e:  # noqa: BLE001
                s.setdefault("render_errors", []).append(f"sheet: {e}")
            continue
        # no video delivered for this format: score the script anyway from a native contact sheet
        # (same renderer, 12 frames; far cheaper than rendering every frame on a shared box)
        delivered = False
        with LOCK:
            rc, out, err = afx(["sheet", reel.name, "--n", "12", "--cols", "4", "--format", fmt, "-o", str(sh)], reel.parent, env, timeout=3600)  # may wait for the heavy-job lock
        if rc == 0 and sh.exists():
            sheets.append(str(sh))
            rendered.append(f"{fmt}: NOT rendered by the agent; frames from `agentfx sheet` ({total or 0:.1f} s script)")
        else:
            s.setdefault("render_errors", []).append(f"{fmt}: {err[-600:]}")
        if total:
            s["durations"][fmt] = round(total, 2)
    s["agent_rendered"] = delivered
    s["renders"] = delivered and bool(s["durations"])
    s["compiles"] = bool(sheets)
    want = case.get("duration_s")
    if want:  # the user stated a length: +-15 %
        def ok(d):
            return abs(d - want) <= 0.15 * want + 0.05
    else:  # no length stated: a sane social/marketing length
        def ok(d):
            return 3.0 <= d <= 25.0
    s["duration_ok"] = bool(s["durations"]) and all(ok(d) for d in s["durations"].values())
    s["text"] = text_check(source, case["must_text"])
    if sheets and not args.no_judge:
        s["judge"] = judge(case, wd, sheets, source, "; ".join(rendered) or "no render", args, env)
    j = s.get("judge") or {}
    # first-try acceptance: the judge would accept it AND the user actually got a clean video
    s["accepted"] = bool(j.get("accept_first_try")) and s["renders"] and s["lint_clean"]
    s["score"] = composite(s)
    return s


def composite(s):
    """0-100. Hard checks 40, judge checklist 30, judge quality 30."""
    if not s["reel"]:
        return 0.0
    txt = s["text"]
    hard = 12 * s["lint_clean"] + (10 if s["renders"] else 4 if s.get("compiles") else 0) + 8 * s["duration_ok"] + 10 * (sum(t["pass"] for t in txt) / len(txt) if txt else 1)
    j = s.get("judge") or {}
    cl = j.get("checklist") or []
    q = j.get("quality")
    soft = (30 * sum(bool(x.get("pass")) for x in cl) / len(cl) if cl else 0) + (30 * (q - 1) / 9 if q else 0)
    return round(hard + soft, 1)


# ---------------------------------------------------------------- orchestration

def run_case(case, args, run_dir, work_root, env, do_agent=True):
    wd = Path(work_root) / case["id"]
    rec = {"id": case["id"], "tags": case.get("tags", []), "formats": case["formats"], "duration_s": case["duration_s"],
           "model": args.model, "mode": args.mode}
    if do_agent:
        wait_for_load(args.max_load)
        prepare_workdir(case, wd, args.mode)
        print(f"[{time.strftime('%H:%M:%S')}] start {case['id']}", flush=True)
        rec["agent"] = run_agent(case, wd, args, env)
    else:
        old = run_dir / "cases" / f"{case['id']}.json"
        if old.exists():
            prev = json.loads(old.read_text())
            rec["agent"] = prev.get("agent")
            rec["model"], rec["mode"] = prev.get("model"), prev.get("mode")
    rec["metrics"] = parse_transcript(wd / "_transcript.jsonl")
    t0 = time.time()
    try:
        rec["score"] = score_case(case, wd, args, env)
    except Exception as e:  # noqa: BLE001
        rec["score"] = {"error": repr(e), "score": 0.0, "reel": False}
    rec["score_wall_s"] = round(time.time() - t0, 1)
    rec["workdir"] = str(wd)
    (run_dir / "cases").mkdir(parents=True, exist_ok=True)
    (run_dir / "cases" / f"{case['id']}.json").write_text(json.dumps(rec, indent=1, ensure_ascii=False))
    j = rec["score"].get("judge") or {}
    print(f"[{time.strftime('%H:%M:%S')}] done  {case['id']}: score {rec['score'].get('score')} q={j.get('quality')} "
          f"accept={j.get('accept_first_try')} turns={rec['metrics'].get('num_turns')} ${rec['metrics'].get('cost_usd') or 0:.2f}", flush=True)
    return rec


def wait_for_load(max_load, max_wait=1800):
    """Shared box: do not start a case while the 1-min load is above max_load (wait up to 30 min)."""
    t0 = time.time()
    while os.getloadavg()[0] > max_load and time.time() - t0 < max_wait:
        print(f"[{time.strftime('%H:%M:%S')}] load {os.getloadavg()[0]:.0f} > {max_load}, waiting", flush=True)
        time.sleep(60)


def load_run(run_id):
    d = RESULTS / run_id / "cases"
    return {json.loads(p.read_text())["id"]: json.loads(p.read_text()) for p in sorted(d.glob("*.json"))}


def agg(recs):
    rs = list(recs.values())
    n = len(rs) or 1

    def mean(xs):
        xs = [x for x in xs if x is not None]
        return round(sum(xs) / len(xs), 2) if xs else None
    sc = [r["score"] for r in rs]
    js = [s.get("judge") or {} for s in sc]
    cl = [x for j in js for x in (j.get("checklist") or [])]
    return {
        "cases": len(rs),
        "score": mean([s.get("score") for s in sc]),
        "accept_rate": round(sum(bool(s.get("accepted")) for s in sc) / n, 3),
        "judge_accept": round(sum(bool(j.get("accept_first_try")) for j in js) / n, 3),
        "quality": mean([j.get("quality") for j in js]),
        "checklist_pass": round(sum(bool(x.get("pass")) for x in cl) / len(cl), 3) if cl else None,
        "lint_clean": round(sum(bool(s.get("lint_clean")) for s in sc) / n, 3),
        "renders": round(sum(bool(s.get("renders")) for s in sc) / n, 3),
        "duration_ok": round(sum(bool(s.get("duration_ok")) for s in sc) / n, 3),
        "text_ok": round(sum(all(t["pass"] for t in s.get("text", [])) for s in sc) / n, 3),
        "turns": mean([r["metrics"].get("num_turns") for r in rs]),
        "lint_calls": mean([r["metrics"].get("lint_calls") for r in rs]),
        "first_lint_errors": mean([r["metrics"].get("first_lint_errors") for r in rs]),
        "lint_errors_seen": mean([r["metrics"].get("lint_errors_seen") for r in rs]),
        "agent_cost_usd": round(sum((r["metrics"].get("cost_usd") or 0) for r in rs), 2),
        "judge_cost_usd": round(sum((j.get("cost_usd") or 0) for j in js), 2),
        "agent_wall_s": mean([(r.get("agent") or {}).get("wall_s") for r in rs]),
    }


def summary(run_id):
    recs = load_run(run_id)
    a = agg(recs)
    meta_p = RESULTS / run_id / "meta.json"
    meta = json.loads(meta_p.read_text()) if meta_p.exists() else {}
    codes = {}
    for r in recs.values():
        for k, v in r["metrics"].get("error_codes", {}).items():
            codes[k] = codes.get(k, 0) + v
    L = [f"# Eval run `{run_id}`", ""]
    if meta:
        L += [f"Model {meta.get('model')}, judge {meta.get('judge_model')}, mode {meta.get('mode')}, max turns {meta.get('max_turns')}, "
              f"commit {meta.get('commit')}, wall {meta.get('wall_min')} min.", ""]
    L += ["| Metric | Value |", "|---|---|"]
    for k, v in a.items():
        L.append(f"| {k} | {v} |")
    L += ["", "Lint errors the agents ran into (code: count over all lint calls): " +
          (", ".join(f"`{k}` {v}" for k, v in sorted(codes.items(), key=lambda x: -x[1])) or "none"), ""]
    L += ["| Case | Score | Q | Accept | Checklist | Lint | Render | Dur | Text | Turns | Lint calls | 1st lint E | $ | Problems |",
          "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for cid, r in sorted(recs.items()):
        s = r["score"]
        j = s.get("judge") or {}
        cl = j.get("checklist") or []
        m = r["metrics"]
        probs = "; ".join(j.get("problems") or [])[:220].replace("|", "/").replace("\n", " ")
        L.append(f"| {cid} | {s.get('score')} | {j.get('quality')} | {'yes' if s.get('accepted') else ('judge' if j.get('accept_first_try') else 'no')} | "
                 f"{sum(bool(x.get('pass')) for x in cl)}/{len(cl)} | {'ok' if s.get('lint_clean') else 'ERR'} | "
                 f"{'ok' if s.get('renders') else 'FAIL'} | {'ok' if s.get('duration_ok') else s.get('durations')} | "
                 f"{sum(t['pass'] for t in s.get('text', []))}/{len(s.get('text', []))} | {m.get('num_turns')} | {m.get('lint_calls')} | "
                 f"{m.get('first_lint_errors')} | {(m.get('cost_usd') or 0):.2f} | {probs} |")
    text = "\n".join(L) + "\n"
    (RESULTS / run_id / "summary.md").write_text(text)
    (RESULTS / run_id / "summary.json").write_text(json.dumps({"run_id": run_id, "meta": meta, "aggregate": a, "error_codes": codes}, indent=1))
    return text


def compare(a, b):
    ra, rb = load_run(a), load_run(b)
    common = sorted(set(ra) & set(rb))
    A, B = agg({k: ra[k] for k in common}), agg({k: rb[k] for k in common})
    L = [f"# Compare `{a}` -> `{b}` ({len(common)} common cases)", "", "| Metric | " + a + " | " + b + " | delta |", "|---|---|---|---|"]
    for k in A:
        va, vb = A[k], B[k]
        d = round(vb - va, 3) if isinstance(va, (int, float)) and isinstance(vb, (int, float)) else ""
        L.append(f"| {k} | {va} | {vb} | {d} |")
    L += ["", "| Case | score A | score B | delta | Q A | Q B | accept A | accept B |", "|---|---|---|---|---|---|---|---|"]
    for c in common:
        sa, sb = ra[c]["score"], rb[c]["score"]
        ja, jb = sa.get("judge") or {}, sb.get("judge") or {}
        L.append(f"| {c} | {sa.get('score')} | {sb.get('score')} | {round((sb.get('score') or 0) - (sa.get('score') or 0), 1)} | "
                 f"{ja.get('quality')} | {jb.get('quality')} | {sa.get('accepted')} | {sb.get('accepted')} |")
    return "\n".join(L) + "\n"


def git_commit():
    r = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"], capture_output=True, text=True)
    dirty = subprocess.run(["git", "-C", str(ROOT), "status", "--porcelain", "--", "crates", "skill", "mcp", "docs"], capture_output=True, text=True).stdout.strip()
    return r.stdout.strip() + ("+dirty" if dirty else "")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("run", "score"):
        p = sub.add_parser(name)
        if name == "score":
            p.add_argument("run_id")
        else:
            p.add_argument("--run-id")
        p.add_argument("--cases")
        p.add_argument("--tags")
        p.add_argument("--model", default="sonnet")
        p.add_argument("--judge-model", default="sonnet")
        p.add_argument("--parallel", type=int, default=1)
        p.add_argument("--max-turns", type=int, default=40)
        p.add_argument("--timeout", type=int, default=900)
        p.add_argument("--mode", default="both", choices=["both", "cli", "mcp"])
        p.add_argument("--effort")
        p.add_argument("--work", default=os.environ.get("AGENTFX_EVAL_WORK") or os.path.join(tempfile.gettempdir(), "agentfx-evals"))
        p.add_argument("--no-judge", action="store_true")
        p.add_argument("--max-load", type=float, default=float(os.environ.get("AGENTFX_EVAL_MAX_LOAD", 10)))
        p.add_argument("--skip-done", action="store_true", help="run: skip cases that already have a result in this run")
    sub.add_parser("summary").add_argument("run_id")
    pc = sub.add_parser("compare")
    pc.add_argument("a")
    pc.add_argument("b")
    sub.add_parser("list")
    args = ap.parse_args()

    if args.cmd == "list":
        for c in load_cases():
            print(f"{c['id']:34} {','.join(c['formats']):10} {str(c.get('duration_s') or '-'):>3}s  {','.join(c.get('tags', []))}")
        return
    if args.cmd == "summary":
        print(summary(args.run_id))
        return
    if args.cmd == "compare":
        print(compare(args.a, args.b))
        return

    run_id = args.run_id or time.strftime("%Y%m%d-%H%M")
    run_dir = RESULTS / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    work_root = Path(args.work) / run_id
    work_root.mkdir(parents=True, exist_ok=True)
    env = tool_env(work_root)
    cases = load_cases(args.cases, args.tags)
    if args.cmd == "score":  # only cases this run actually has
        cases = [c for c in cases if (work_root / c["id"] / "_transcript.jsonl").exists()]
    if args.cmd == "run" and args.skip_done:
        cases = [c for c in cases if not (run_dir / "cases" / f"{c['id']}.json").exists()]
    if args.parallel > 2:
        print("run.py: parallel capped at 2 (shared machine)")
        args.parallel = 2
    meta_p = run_dir / "meta.json"
    meta = json.loads(meta_p.read_text()) if meta_p.exists() else {}
    if args.cmd == "run":
        meta.setdefault("commit_first", git_commit())
        meta.update({"run_id": run_id, "model": args.model, "judge_model": args.judge_model, "mode": args.mode,
                     "max_turns": args.max_turns, "commit": git_commit(), "started": time.strftime("%Y-%m-%dT%H:%M:%S"),
                     "work": str(work_root)})
        meta_p.write_text(json.dumps(meta, indent=1))
    t0 = time.time()
    with cf.ThreadPoolExecutor(max_workers=max(1, args.parallel)) as ex:
        futs = [ex.submit(run_case, c, args, run_dir, work_root, env, args.cmd == "run") for c in cases]
        for f in cf.as_completed(futs):
            try:
                f.result()
            except Exception as e:  # noqa: BLE001
                print("case failed:", repr(e), flush=True)
    if args.cmd == "run":
        meta["wall_min"] = round((meta.get("wall_min") or 0) + (time.time() - t0) / 60, 1)
        meta_p.write_text(json.dumps(meta, indent=1))
    print(summary(run_id))


if __name__ == "__main__":
    main()
