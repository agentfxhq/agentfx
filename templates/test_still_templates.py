#!/usr/bin/env python3
"""Tests for the still templates (pytest, or run directly: python3 templates/test_still_templates.py).

1. Unit tests of the still/table additions to render_template.py: sizes, data params (JSON,
   CSV, limits), table blocks, expressions, bindings, batch.
2. Every still template: instantiate at every declared size with defaults and with edge inputs
   (max-length text, missing optional images, 1 and max table rows, every enum value).
3. With the agentfx binary ($AGENTFX_BIN, `agentfx` on PATH or <repo>/target/release/agentfx):
   `agentfx lint --measure --no-tts` at every size, no errors and no warnings except the video
   reading-time rules (STILL_IGNORED: reading-time and scale-taste rules for video).
4. With AGENTFX_STILL_RENDER=1: one frame per size (`agentfx frame --at <still_at>`), checked for
   the exact pixel size. Frames run one at a time under the shared heavy-job lock
   ($AGENTFX_HEAVY_LOCK, default /tmp/claude-1000/agentfx-heavy.lock when its dir exists).
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

STILL_IGNORED = {"W_TEXT_DENSITY", "W_WORDY", "W_READ_SIZE", "I_LINE_LENGTH", "W_SCALE_FILL"}
LONG = ("The quick brown fox jumps over the lazy dog while the band plays on and on, "
        "and everybody stays until the very last song of the night is over")


def agentfx_bin():
    for c in (os.environ.get("AGENTFX_BIN"), shutil.which("agentfx"), str(HERE.parent / "target/release/agentfx")):
        if c and Path(c).is_file():
            return c
    return None


def still_templates():
    out = []
    for tid in rt.list_templates(HERE):
        m = rt.load_manifest(tid, HERE)
        if m.get("kind") == "still":
            out.append(tid)
    return out


def tiny_png(path, w=64, h=48, rgba=(40, 120, 230, 255)):
    raw = b"".join(b"\x00" + bytes(rgba) * w for _ in range(h))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)) \
        + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
    Path(path).write_bytes(png)


def png_size(path):
    b = Path(path).read_bytes()[:24]
    return struct.unpack(">II", b[16:24])


# ------------------------------------------------------------------ unit tests

M = {"id": "t", "kind": "still", "sizes": ["instagram-post", "story", "leaderboard"], "params": [
    {"id": "title", "type": "text", "default": "Hi", "max_len": 40},
    {"id": "logo", "type": "image", "default": "", "accept": [".png"]},
    {"id": "layout", "type": "enum", "default": "grid", "options": ["grid", "mosaic"]},
    {"id": "bg", "type": "color", "default": "#101010"},
    {"id": "rows", "type": "data", "max_rows": 5,
     "columns": [{"id": "section"}, {"id": "item", "required": True}, {"id": "price", "type": "number"},
                 {"id": "img", "type": "image"}],
     "default": [{"section": "A", "item": "Soup", "price": 4.5, "img": "a.png"},
                 {"section": "A", "item": "Tea \"hot\"", "price": 3},
                 {"section": "B", "item": "Cake **x**", "price": 5}]},
]}


def r(src, size=None, **params):
    return rt.render_source(src, M, params, size)


def test_sizes():
    assert r("{{W}}x{{H}} {{S}} {{DW}}x{{DH}} {{SIZE}}") == "1080x1080 1 1080x1080 instagram-post"
    assert r("{{W}}x{{H}} {{DW}}x{{DH}}", "story") == "1080x1920 1080x1920"
    out = r("{{W}}x{{H}} {{S}} {{= DW*S}} {{= DH}}", "leaderboard")
    assert out.startswith("728x90 0.0833 ") and float(out.split()[2]) >= 728 - 0.01
    assert r("{{#= SIZE_story}}S{{/= SIZE_story}}{{^= SIZE_story}}-{{/= SIZE_story}}", "story") == "S"
    assert r("{{W}}x{{H}}", "640x480") == "640x480"
    for bad in ("huge", "10x10", "9000x9000", "../x"):
        try:
            r("{{W}}", bad)
            raise AssertionError(bad)
        except ValueError:
            pass
    assert rt.size_px("a4-flyer") == (2480, 3508)
    # the compiler rounds odd frame sizes up to even, so presets must be even to come out exact
    assert all(w % 2 == 0 and h % 2 == 0 for w, h in rt.SIZE_PRESETS.values())


def test_data_json_csv_and_blocks():
    src = "{{#rows}}{{#@changed.section}}[{{.section}}]{{/@changed.section}}{{@n}}.{{.item}}={{= row_price*2}}{{#.img}}@{{.img}}{{/.img}}{{^@last}},{{/@last}}{{/rows}}"
    assert r(src) == '[A]1.Soup=9@a.png,2.Tea \\"hot\\"=6,[B]3.Cake *x*=10'
    csv = "section;item;price\nX;Pie;2.5\nX;Jam;1"
    assert r(src, rows=csv) == "[X]1.Pie=5,2.Jam=2"
    assert r(src, rows='[{"item": "Only", "price": "7"}]') == "[]1.Only=14"
    assert r(src, rows=[["S", "Arr", 1]]) == "[S]1.Arr=2"
    assert r("{{rows.count}} {{= rows_count}} {{= rows_groups_section}} {{= rows_filled_img}} {{= rows_max_price}}") == "3 3 2 1 5"
    assert r("{{#rows}}x{{/rows}}{{^rows}}empty{{/rows}}", rows=[]) == "empty"


def test_data_validation():
    bad = [([{"item": "a"}] * 6, "at most 5"), ([{"price": 1}], "required"), ([{"item": "a", "price": "x"}], "number"),
           ([{"item": "a", "img": "../x.png"}], "plain file name"), ([{"item": "a", "img": "x.gif"}], "image type"),
           ("[{", "invalid JSON"), ('"just a string"', "list of rows"), (["x"], "object"),
           ("item\n" + "x" * 70000, "KB")]
    for v, msg in bad:
        try:
            r("{{rows.count}}", rows=v)
            raise AssertionError(f"accepted {v!r}")
        except ValueError as e:
            assert msg in str(e), (msg, str(e))
    big = {"id": "t2", "params": [{"id": "rows", "type": "data", "columns": [{"id": "a"}]}]}
    assert rt.render_source("{{rows.count}}", big, {"rows": [{"a": str(k)} for k in range(500)]}) == "500"
    try:
        rt.render_source("{{rows.count}}", big, {"rows": [{"a": "1"}] * 501})
        raise AssertionError("501 rows accepted")
    except ValueError as e:
        assert "at most 500" in str(e)


def test_expressions_and_bindings():
    assert r("{{: a = 2}}{{: b = a*3 + (1 if a > 1 else 0)}}{{b}} {{= sqrt(16)}} {{= 3 if 1 < 2 <= 2 else 4}}") == "7 4 3"
    assert r("{{#= W > 1000 and not (H < 10)}}big{{/= W > 1000 and not (H < 10)}}") == "big"
    assert r("{{#layout=grid}}{{: z = 5}}{{z}}{{/layout=grid}}") == "5"
    try:
        r("{{#layout=grid}}{{: z = 5}}{{/layout=grid}}{{z}}")   # bindings stay in their block
        raise AssertionError("binding leaked")
    except ValueError:
        pass
    try:
        r("{{: title = 1}}")
        raise AssertionError("param shadowed")
    except ValueError:
        pass
    assert r("{{= layout_grid}} {{= layout_mosaic}} {{= logo_len}} {{= title_len}}") == "1 0 0 2"
    assert r("{{= round(bg_luma*1000)}}") == "5"
    assert r("{{= round(bg_luma*100)}}", bg="#FFFFFF") == "100"
    try:
        r("{{= __import__('os')}}")
        raise AssertionError("call allowed")
    except ValueError:
        pass


def test_required_number_cell_and_default_assets():
    m = {"id": "t", "params": [{"id": "rows", "type": "data", "columns": [{"id": "v", "type": "number", "required": True}]}]}
    try:
        rt.render_source("{{rows.count}}", m, {"rows": [{"v": ""}]})
        raise AssertionError("missing required number accepted")
    except ValueError as e:
        assert "required" in str(e)
    # a supplied image named like a packaged default is an upload: not copied over
    assert "logo.png" in rt.default_assets("social-set", {}, HERE)
    assert "logo.png" not in rt.default_assets("social-set", {"logo": "logo.png"}, HERE)
    assert "photo-1.png" not in rt.default_assets("collage", {"photos": [{"src": "photo-1.png"}, {"src": "x.png"}]}, HERE)


def test_batch_params():
    m = dict(M, batch={"param": "rows", "index": "n"}, params=M["params"] + [{"id": "n", "type": "number", "default": 1, "min": 1}])
    sets = rt.batch_params(m, {})
    assert [s["n"] for s in sets] == [1, 2, 3]
    assert rt.batch_params(m, {"rows": []}) == [{"rows": []}]
    assert rt.batch_params(M, {"title": "x"}) == [{"title": "x"}]
    for bad in (4, 1.5):
        try:
            rt.resolve_params(m, {"n": bad})
            raise AssertionError(f"batch index {bad} accepted")
        except ValueError as e:
            assert "out of range" in str(e)


def test_video_templates_unchanged():
    """A manifest without sizes keeps the old behaviour (no size names, no size argument needed)."""
    m = {"id": "v", "params": [{"id": "t", "type": "text", "default": "x"}]}
    assert rt.render_source("{{t}}", m, {}) == "x"
    try:
        rt.render_source("{{W}}", m, {})
        raise AssertionError("W without sizes")
    except ValueError:
        pass


# ------------------------------------------------------------------ templates

def edge_variants(m):
    """(tag, params) pairs: defaults, max-length text, no optional images, 1 and max table rows,
    every enum value."""
    decl = {p["id"]: p for p in m["params"]}
    out = [("default", {})]
    longp = {}
    for p in m["params"]:
        if p["type"] == "text" and p.get("max_len"):
            longp[p["id"]] = (LONG * 3)[: p["max_len"]].strip() or "x"
    out.append(("long", longp))
    noimg = {p["id"]: "" for p in m["params"] if p["type"] == "image" and not p.get("required")}
    if noimg:
        out.append(("noimage", noimg))
    for p in m["params"]:
        if p["type"] == "data":
            rows = rt.resolve_params(m, {})[p["id"]]
            if not rows:   # e.g. a batch table: synthesize rows from the columns
                rows = [{c["id"]: (f"Sample {c['id']} {k + 1}" if c.get("type", "text") == "text" else 1)
                         for c in p["columns"] if c.get("type", "text") in ("text", "number")} for k in range(3)]
            lo = max(1, int(p.get("min_rows", 1)))
            out.append((f"{p['id']}-min", {p["id"]: rows[:lo] if len(rows) >= lo else rows}))
            hi = int(p.get("max_rows", rt.MAX_DATA_ROWS))
            # max rows with the default cells (in order, so sections stay grouped)
            big = [dict(rows[k * len(rows) // hi]) for k in range(hi)]
            mx = {p["id"]: big}
            if (m.get("batch") or {}).get("param") == p["id"]:
                mx[m["batch"]["index"]] = hi   # render the last row of a full batch
            out.append((f"{p['id']}-max", mx))
            # the default rows with every text cell at its max length
            longrows = [dict(x) for x in rows]
            for row in longrows:
                for c in p["columns"]:
                    if c.get("type", "text") == "text" and row.get(c["id"]) and c.get("max_len"):
                        row[c["id"]] = (row[c["id"]] + " " + LONG)[: c["max_len"]].strip()
            out.append((f"{p['id']}-long", {p["id"]: longrows}))
        if p["type"] == "enum":
            for o in p["options"]:
                if o != p["default"]:
                    out.append((f"{p['id']}={o}", {p["id"]: o}))
    return [(t, {k: v for k, v in ps.items() if k in decl}) for t, ps in out]


def test_manifests_and_assets():
    ids = still_templates()
    assert len(ids) >= 9, ids
    for tid in ids:
        m = rt.load_manifest(tid, HERE)
        d = HERE / tid
        assert m["sizes"] and all(rt.size_px(s) for s in m["sizes"]), tid
        prev = [p for p in (d / "preview.webp", d / "preview.png") if p.is_file()]
        assert prev and prev[0].stat().st_size <= 600 * 1024, f"{tid}: preview missing or > 600 KB"
        assert (d / "README.md").is_file(), tid
        for name, path in rt.default_assets(tid, {}, HERE).items():
            assert path.is_file() and path.suffix == ".png", (tid, name)
        if m.get("batch"):
            decl = {p["id"]: p for p in m["params"]}
            assert decl[m["batch"]["param"]]["type"] == "data" and decl[m["batch"]["index"]]["type"] == "number"


def test_instantiate_every_size_and_edge():
    for tid in still_templates():
        m = rt.load_manifest(tid, HERE)
        for tag, params in edge_variants(m):
            for size in m["sizes"]:
                src = rt.instantiate(tid, params, HERE, size)
                w, h = rt.size_px(size)
                assert f"size={w}x{h}" in src, (tid, size)
                assert "{{" not in src and "}}" not in src, (tid, tag, size)
                assert "layer #page" in src, (tid, size)


def _lint(binary, path):
    p = subprocess.run([binary, "lint", str(path), "--measure", "--no-tts"], capture_output=True, text=True, timeout=300)
    try:
        diags = json.loads(p.stdout or "[]")
    except json.JSONDecodeError:
        raise AssertionError(f"{path}: lint output is not JSON: {p.stdout[:400]} {p.stderr[:400]}")
    return [d for d in diags if d.get("severity") in ("error", "warning") and d.get("code") not in STILL_IGNORED]


def _jobdir(tid, params, tmp):
    d = Path(tmp) / tid
    d.mkdir(parents=True, exist_ok=True)
    # packaged defaults, plus the same files standing in for uploads a test variant names
    files = {**rt.default_assets(tid, {}, HERE), **rt.default_assets(tid, params, HERE)}
    for name, path in files.items():
        shutil.copyfile(path, d / name)
    return d


def test_lint_every_size():
    binary = agentfx_bin()
    if not binary:
        print("skip lint: no agentfx binary")
        return
    only = os.environ.get("STILL_TEMPLATES")
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        for tid in still_templates():
            if only and tid not in only.split(","):
                continue
            m = rt.load_manifest(tid, HERE)
            for tag, params in edge_variants(m):
                d = _jobdir(tid, params, tmp)
                for size in m["sizes"]:
                    f = d / f"{size}-{tag.replace('=', '-')}.reel"
                    f.write_text(rt.instantiate(tid, params, HERE, size))
                    for x in _lint(binary, f):
                        failures.append(f"{tid} [{tag}] {size}: {x['code']} {x.get('id', '')} {x['message'][:140]}")
    assert not failures, "\n".join(failures)


VIDEO_FROM_DATA = ["chart-animated"]   # video templates built on the table params


def test_video_data_templates_lint():
    binary = agentfx_bin()
    if not binary:
        print("skip lint: no agentfx binary")
        return
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        for tid in VIDEO_FROM_DATA:
            m = rt.load_manifest(tid, HERE)
            assert m.get("kind", "video") == "video" and m.get("formats"), tid
            for tag, params in edge_variants(m):
                d = _jobdir(tid, params, tmp)
                f = d / f"{tag.replace('=', '-')}.reel"
                f.write_text(rt.instantiate(tid, params, HERE))
                p = subprocess.run([binary, "lint", str(f), "--measure", "--all-formats", "--no-tts"],
                                   capture_output=True, text=True, timeout=300)
                diags = json.loads(p.stdout[p.stdout.index("["):] if "[" in p.stdout else "[]")
                for x in diags:
                    if x.get("severity") in ("error", "warning") and not (tag == "long" and x["code"] in ("W_TEXT_DENSITY", "W_WORDY")):
                        failures.append(f"{tid} [{tag}] {x.get('format', '')}: {x['code']} {x.get('id', '')} {x['message'][:140]}")
    assert not failures, "\n".join(failures)


def test_render_one_frame_per_size():
    binary = agentfx_bin()
    if not binary or os.environ.get("AGENTFX_STILL_RENDER") != "1":
        print("skip render: set AGENTFX_STILL_RENDER=1 (and have the agentfx binary)")
        return
    import fcntl
    lock_path = os.environ.get("AGENTFX_HEAVY_LOCK", "/tmp/claude-1000/agentfx-heavy.lock")
    lock = open(lock_path, "w") if Path(lock_path).parent.is_dir() else None
    only = os.environ.get("STILL_TEMPLATES")
    with tempfile.TemporaryDirectory() as tmp:
        if lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            for tid in still_templates():
                if only and tid not in only.split(","):
                    continue
                m = rt.load_manifest(tid, HERE)
                d = _jobdir(tid, {}, tmp)
                for size in m["sizes"]:
                    f = d / f"{size}.reel"
                    f.write_text(rt.instantiate(tid, {}, HERE, size))
                    png = d / f"{size}.png"
                    p = subprocess.run(["nice", "-n", "19", binary, "frame", str(f), "--at", m.get("still_at", "0s"), "-o", str(png)],
                                       capture_output=True, text=True, timeout=900)
                    assert png.is_file(), f"{tid} {size}: {p.stdout[-600:]} {p.stderr[-600:]}"
                    assert png_size(png) == rt.size_px(size), (tid, size, png_size(png))
        finally:
            if lock:
                fcntl.flock(lock, fcntl.LOCK_UN)


if __name__ == "__main__":
    fails = 0
    for name, fn in list(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except Exception as e:  # noqa: BLE001
                fails += 1
                print(f"FAIL {name}: {e}")
    sys.exit(1 if fails else 0)
