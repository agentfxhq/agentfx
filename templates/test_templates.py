#!/usr/bin/env python3
"""Tests for the template library (pytest, or run directly: python3 templates/test_templates.py).

1. Unit tests of render_template.py (substitution, escaping, validation, line timing).
2. Every template: instantiate with defaults and with edge inputs (max-length text, empty optional
   params, every enum value) and lint in all three formats with `agentfx lint --measure
   --all-formats --no-tts`; no errors allowed. Needs the agentfx binary: $AGENTFX_BIN, else
   <repo>/target/release/agentfx, else `agentfx` on PATH (must include the distort pack, 27d2a14 or later); the lint part is skipped without it.
"""
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import render_template as rt  # noqa: E402

LONG = "The quick brown fox jumps over the lazy dog while the band plays on and on"


def agentfx_bin():
    for c in (os.environ.get("AGENTFX_BIN"), str(HERE.parent / "target/release/agentfx"), shutil.which("agentfx")):
        if c and Path(c).is_file():
            return c
    return None


def tiny_png(path, w=64, h=32):
    raw = b"".join(b"\x00" + bytes([230, 120, 40, 255]) * w for _ in range(h))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)) \
        + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
    Path(path).write_bytes(png)


# ------------------------------------------------------------------ unit tests

M = {"id": "t", "params": [
    {"id": "text", "type": "text", "default": "Hi", "max_len": 30},
    {"id": "opt", "type": "text", "default": ""},
    {"id": "c", "type": "color", "default": "#112233"},
    {"id": "n", "type": "number", "default": 1, "min": 0.5, "max": 2},
    {"id": "s", "type": "enum", "default": "a", "options": ["a", "b"]},
    {"id": "song", "type": "audio", "default": "x.mp3"},
    {"id": "logo", "type": "image"},
    {"id": "bpm", "type": "number", "default": 120, "min": 40, "max": 220},
    {"id": "off", "type": "number", "default": 0.5, "min": 0, "max": 60},
    {"id": "beats", "type": "number", "default": 4, "min": 1, "max": 32},
    {"id": "times", "type": "times", "default": ""},
    {"id": "lyr", "type": "longtext", "default": "one\n\ntwo\nthree",
     "timing": {"bpm": "bpm", "offset": "off", "beats": "beats", "times": "times"}},
]}


def r(src, **params):
    return rt.render_source(src, M, params)


def test_substitution_and_escape():
    assert r('t "{{text}}"', text='say "hi" \\ {x} **b** `c`') == 't "say \\"hi\\" \\\\ (x) *b* \'c\'"'
    assert r("{{c}} {{c.rgb}}55", c="#aabbccdd") == "#AABBCCDD #AABBCC55"
    assert r("{{n}} {{= n * 3}} {{= clamp(10 / n, 1, 4)}}", n=5) == "2 6 4"
    assert r("{{#s=b}}B{{/s=b}}{{^s=b}}notB{{/s=b}}") == "notB"
    assert r("{{#opt}}has{{/opt}}{{^opt}}none{{/opt}}") == "none"
    assert r("x\n{{#s=a}}\n  line\n{{/s=a}}\ny") == "x\n  line\ny"   # standalone tags take their line
    assert r("{{ramp c c 3}}") == "#112233, #112233, #112233"
    assert r("{{ramp #000000 #FFFFFF 4 mirror}}").split(", ") == ["#000000", "#BCBCBC", "#FFFFFF", "#BCBCBC"]


def test_validation_errors():
    bad = [({"text": "x" * 31}, "at most 30"), ({"text": "a\nb"}, "newlines"), ({"c": "red"}, "colour"),
           ({"s": "z"}, "not one of"), ({"song": "../etc/passwd"}, "plain file name"),
           ({"song": "a.exe"}, "not a supported"), ({"logo": "https://x.png"}, "plain file name"),
           ({"nope": 1}, "unknown param"), ({"n": "abc"}, "number"), ({"times": "3, 1"}, "decrease")]
    for params, msg in bad:
        try:
            r("{{text}}", **params)
        except ValueError as e:
            assert msg in str(e), (params, str(e))
        else:
            raise AssertionError(f"no error for {params}")
    for src, msg in [("{{#s}}x", "never closed"), ("{{zzz}}", "unknown name"), ("{{lyr}}", "only used in"),
                     ("{{= __import__('os')}}", "only numbers")]:
        try:
            r(src)
        except ValueError as e:
            assert msg in str(e), (src, str(e))
        else:
            raise AssertionError(f"no error for {src}")


def test_default_assets_never_replace_uploads():
    assert "night-drive.mp3" in rt.default_assets("music-visualizer", {}, HERE)
    assert rt.default_assets("music-visualizer", {"audio": "night-drive.mp3"}, HERE) == {}
    assert rt.default_assets("music-visualizer", {"audio": "song.mp3"}, HERE) == {}


def test_line_timing_grid_and_times():
    src = "{{#lyr}}{{n}}:{{text}}@{{start}}-{{end}};{{/lyr}}|{{lyr.count}}|{{lyr.end}}"
    # 120 bpm, 4 beats per line = 2 s per line, from 0.5 s; empty rows are dropped
    assert r(src) == "1:one@0.5-2.5;2:two@2.5-4.5;3:three@4.5-6.5;|3|6.5"
    # given times win line by line; the rest continue on the grid from the next beat
    assert r(src, times="[[0, 1.2], 1.6]") == "1:one@0-1.2;2:two@1.6-4;3:three@4-6;|3|6"
    assert r(src, times="0-1, 1.1-2.05") == "1:one@0-1;2:two@1.1-2.05;3:three@2.5-4.5;|3|4.5"


def test_every_template_instantiates():
    ids = rt.list_templates(HERE)
    assert len(ids) >= 8, ids
    for tid in ids:
        m = rt.load_manifest(tid, HERE)
        for k in ("title", "description", "tags", "formats", "duration_s", "params"):
            assert k in m, (tid, k)
        assert (HERE / tid / "README.md").is_file(), tid
        prev = HERE / tid / "preview.webp"
        assert prev.is_file() and prev.stat().st_size <= 1.5 * 1024 * 1024, (tid, "preview.webp missing or > 1.5 MB")
        for _, params in cases(m):
            src = rt.instantiate(tid, params, HERE)
            assert "{{" not in src and src.startswith("//"), tid


# ------------------------------------------------------------------ edge cases + lint

def cases(manifest):
    """Defaults, then edge inputs: every enum value, max-length text, empty optional params."""
    params = manifest["params"]
    out = [("defaults", {})]
    for p in params:
        if p["type"] == "enum":
            for o in p["options"]:
                if o != p.get("default"):
                    out.append((f"{p['id']}={o}", {p["id"]: o}))
    long = {}
    for p in params:
        if p["type"] == "text":
            n = p.get("max_len", 40)
            long[p["id"]] = (LONG * 3)[:n].strip()
        elif p["type"] == "longtext":
            lines = [(LONG * 2)[:p.get("max_line_len", 90)].strip()] + ["short"] + [LONG[:40]] * 3
            long[p["id"]] = "\n".join(lines)
    out.append(("long", long))
    empty = {p["id"]: "" for p in params if p["type"] == "text" and not p.get("required")}
    if empty:
        out.append(("empty", empty))
    for p in params:
        if p["type"] == "image":
            out.append((f"{p['id']}=png", {p["id"]: "upload.png"}))
        if p["type"] == "color":
            out.append(("light", {q["id"]: "#F4F1EA" if q["id"] in ("bg", "paper") else "#202020"
                                  for q in params if q["type"] == "color"}))
            break
    return out


def lint(binary, tid, label, params, workdir):
    d = Path(workdir) / tid
    d.mkdir(parents=True, exist_ok=True)
    src = rt.instantiate(tid, params, HERE)
    for name, path in rt.default_assets(tid, params, HERE).items():
        shutil.copyfile(path, d / name)
    tiny_png(d / "upload.png", 400, 200)
    f = d / f"{label.replace('=', '-')}.reel"
    f.write_text(src, encoding="utf-8")
    p = subprocess.run([binary, "lint", f.name, "--measure", "--all-formats", "--no-tts"], cwd=d,
                       capture_output=True, text=True, timeout=600)
    raw = p.stdout
    diags = json.loads(raw[raw.find("["):raw.rfind("]") + 1]) if "[" in raw else []
    return [x for x in diags if x["severity"] == "error"]


def test_lint_all_templates():
    binary = agentfx_bin()
    if not binary:
        print("skip: agentfx binary not found")
        return
    only = os.environ.get("TEMPLATES")
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        for tid in rt.list_templates(HERE):
            if only and tid not in only.split(","):
                continue
            for label, params in cases(rt.load_manifest(tid, HERE)):
                errs = lint(binary, tid, label, params, tmp)
                status = "ok" if not errs else f"{len(errs)} errors"
                print(f"  {tid:18} {label:24} {status}", flush=True)
                for e in errs[:5]:
                    failures.append(f"{tid} [{label}] {e.get('format')} {e['code']}: {e['message']}")
    assert not failures, "\n".join(failures)


if __name__ == "__main__":
    fails = 0
    for name, fn in list(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"PASS {name}")
            except AssertionError as e:
                fails += 1
                print(f"FAIL {name}: {e}")
    sys.exit(1 if fails else 0)
