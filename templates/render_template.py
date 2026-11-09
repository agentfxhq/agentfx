#!/usr/bin/env python3
"""Instantiate an AgentFX template: validate params against manifest.json, substitute template.reel.

The single implementation of template substitution (docs/PLATFORM.md, "Templates"). Stdlib only;
the render service imports it:

    from render_template import instantiate, resolve_params, default_assets
    source = instantiate("glitch-text", {"text": "HELLO"}, "templates")

Command line:

    python3 templates/render_template.py <id> [--set k=v ...] [-o out.reel] [--copy-assets DIR]
    python3 templates/render_template.py --list

Template syntax (see docs/PLATFORM.md for the full contract):

    {{id}}                      the value (text escaped for a quoted Reel string)
    {{= expr}}                  arithmetic over numbers and number params: + - * / // % ( ),
                                min() max() round() abs() floor() ceil() clamp(v, lo, hi)
    {{#id}} ... {{/id}}         block shown when the param is non-empty / non-zero;
                                for a longtext param: repeated once per line (see below)
    {{^id}} ... {{/id}}         block shown when the param is empty / zero
    {{#id=v}} ... {{/id=v}}     block shown when the param equals v (enums); {{^id=v}} when not
    {{id.count}} {{id.end}}     longtext: number of lines, end time of the last line (s)

Inside a longtext loop: {{text}} (the line, escaped), {{n}} (1-based), {{i}} (0-based),
{{start}} {{end}} {{dur}} (seconds, plain numbers), {{#first}}/{{#last}} blocks, plus every
param and {{= expr}} (where n, i, start, end, dur are numbers).
"""
from __future__ import annotations

import argparse
import ast
import json
import math
import re
import shutil
import sys
from pathlib import Path

ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,47}$")
PARAM_ID_RE = re.compile(r"^[a-z][a-z0-9_]{0,31}$")
COLOR_RE = re.compile(r"^#[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$")
ASSET_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
TYPES = {"text", "longtext", "color", "number", "enum", "audio", "image", "times"}
ASSET_TYPES = {"audio", "image"}
AUDIO_EXT = {".mp3", ".wav", ".m4a", ".aac", ".ogg", ".flac"}
IMAGE_EXT = {".png", ".jpg", ".jpeg", ".svg"}
MAX_LONGTEXT_LINES = 64
MAX_LINE_LEN = 120

TAG_RE = re.compile(r"\{\{\s*([#^/=]?)\s*([^{}]*?)\s*\}\}")


# ------------------------------------------------------------------ manifest + params

def template_dir(template_id: str, templates_dir) -> Path:
    if not isinstance(template_id, str) or not ID_RE.match(template_id):
        raise ValueError(f"invalid template id {template_id!r}: use lowercase a-z, 0-9 and -")
    d = Path(templates_dir) / template_id
    if not (d / "manifest.json").is_file() or not (d / "template.reel").is_file():
        known = ", ".join(list_templates(templates_dir)) or "none"
        raise ValueError(f"unknown template {template_id!r} (known: {known})")
    return d


def list_templates(templates_dir) -> list[str]:
    root = Path(templates_dir)
    if not root.is_dir():
        return []
    return sorted(p.name for p in root.iterdir()
                  if ID_RE.match(p.name) and (p / "manifest.json").is_file() and (p / "template.reel").is_file())


def load_manifest(template_id: str, templates_dir) -> dict:
    d = template_dir(template_id, templates_dir)
    with open(d / "manifest.json", encoding="utf-8") as f:
        m = json.load(f)
    if m.get("id") != template_id:
        raise ValueError(f"manifest id {m.get('id')!r} does not match the directory {template_id!r}")
    seen = set()
    for p in m.get("params", []):
        pid, ptype = p.get("id"), p.get("type")
        if not isinstance(pid, str) or not PARAM_ID_RE.match(pid):
            raise ValueError(f"{template_id}: bad param id {pid!r}")
        if pid in seen:
            raise ValueError(f"{template_id}: param {pid!r} declared twice")
        seen.add(pid)
        if ptype not in TYPES:
            raise ValueError(f"{template_id}: param {pid!r} has unknown type {ptype!r}")
        if ptype == "enum" and not p.get("options"):
            raise ValueError(f"{template_id}: enum param {pid!r} needs options")
    return m


def _clean_text(s: str, pid: str, max_len: int | None, allow_newlines=False) -> str:
    if not isinstance(s, str):
        s = str(s)
    if not allow_newlines and ("\n" in s or "\r" in s):
        raise ValueError(f"param {pid!r}: newlines are not allowed in a text param (use one line)")
    # Control characters out; tabs become spaces.
    s = "".join(" " if c == "\t" else c for c in s if c in "\n" or ord(c) >= 32)
    if max_len is not None and len(s) > max_len:
        raise ValueError(f"param {pid!r}: {len(s)} characters, at most {max_len} allowed")
    return s


def _markup_safe(s: str) -> str:
    """Reel text runs treat **, {..} and `..` as markup: neutralise them in user text."""
    s = s.replace("**", "*").replace("`", "'").replace("{", "(").replace("}", ")")
    return " ".join(s.split())


def _escape(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def _number(v, pid):
    if isinstance(v, bool):
        raise ValueError(f"param {pid!r}: expected a number, got {v!r}")
    if isinstance(v, (int, float)):
        x = float(v)
    else:
        try:
            x = float(str(v).strip())
        except ValueError:
            raise ValueError(f"param {pid!r}: expected a number, got {v!r}") from None
    if not math.isfinite(x):
        raise ValueError(f"param {pid!r}: expected a finite number, got {v!r}")
    return x


def _times(v, pid) -> list[tuple[float, float | None]]:
    """Per-line times: a JSON list of numbers or [start, end] pairs, or a string
    "0, 2.5, 5.1" / "0-2.2, 2.5-4.9" (seconds). Empty = none."""
    if v is None or v == "":
        return []
    items = v
    if isinstance(v, str):
        s = v.strip()
        if s.startswith("["):
            try:
                items = json.loads(s)
            except json.JSONDecodeError as e:
                raise ValueError(f"param {pid!r}: invalid JSON list ({e.msg})") from None
        else:
            items = [x for x in re.split(r"[,\s;]+", s) if x]
    if not isinstance(items, list):
        raise ValueError(f"param {pid!r}: expected a list of times")
    out = []
    for it in items:
        if isinstance(it, str) and re.match(r"^\d+(\.\d+)?-\d+(\.\d+)?$", it):
            a, b = it.split("-")
            it = [a, b]
        if isinstance(it, (list, tuple)):
            if len(it) != 2:
                raise ValueError(f"param {pid!r}: a time pair needs [start, end], got {it!r}")
            a, b = _number(it[0], pid), _number(it[1], pid)
            if b <= a:
                raise ValueError(f"param {pid!r}: end {b:g} must be after start {a:g}")
            out.append((a, b))
        else:
            out.append((_number(it, pid), None))
    for (a, _), (b, _) in zip(out, out[1:]):
        if b < a:
            raise ValueError(f"param {pid!r}: times must not decrease ({a:g} then {b:g})")
    if out and out[0][0] < 0:
        raise ValueError(f"param {pid!r}: times must be >= 0")
    if len(out) > MAX_LONGTEXT_LINES:
        raise ValueError(f"param {pid!r}: at most {MAX_LONGTEXT_LINES} times")
    return out


def resolve_params(manifest: dict, params: dict | None) -> dict:
    """Validate user params against the manifest and fill defaults. Returns {id: value} with
    typed values (str for text/colour/enum/asset, float for number, list[str] for longtext,
    list[(start, end|None)] for times). Raises ValueError with a clear message."""
    params = dict(params or {})
    decl = {p["id"]: p for p in manifest.get("params", [])}
    unknown = sorted(set(params) - set(decl))
    if unknown:
        raise ValueError(f"unknown param(s) {', '.join(unknown)} for template {manifest.get('id')!r} "
                         f"(known: {', '.join(decl)})")
    out = {}
    for pid, p in decl.items():
        t = p["type"]
        given = pid in params and params[pid] is not None
        v = params[pid] if given else p.get("default")
        if t == "text":
            v = _markup_safe(_clean_text(v if v is not None else "", pid, p.get("max_len")))
            if p.get("required") and not v:
                raise ValueError(f"param {pid!r} is required")
        elif t == "longtext":
            v = _clean_text(v if v is not None else "", pid, p.get("max_len"), allow_newlines=True)
            lines = [_markup_safe(x) for x in v.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
            lines = [x for x in lines if x]
            max_lines = min(int(p.get("max_lines", MAX_LONGTEXT_LINES)), MAX_LONGTEXT_LINES)
            if len(lines) > max_lines:
                raise ValueError(f"param {pid!r}: {len(lines)} lines, at most {max_lines} allowed")
            max_line = int(p.get("max_line_len", MAX_LINE_LEN))
            for k, line in enumerate(lines, 1):
                if len(line) > max_line:
                    raise ValueError(f"param {pid!r}: line {k} has {len(line)} characters, at most {max_line}")
            if p.get("required") and not lines:
                raise ValueError(f"param {pid!r} is required (at least one line)")
            v = lines
        elif t == "color":
            v = "" if v is None else str(v).strip()
            if not v and not p.get("required") and p.get("default") in (None, ""):
                pass
            elif not COLOR_RE.match(v):
                raise ValueError(f"param {pid!r}: colour must look like #RRGGBB or #RRGGBBAA, got {v!r}")
            v = v.upper()
        elif t == "number":
            if v is None:
                raise ValueError(f"param {pid!r} needs a number")
            x = _number(v, pid)
            if "min" in p:
                x = max(float(p["min"]), x)
            if "max" in p:
                x = min(float(p["max"]), x)
            v = x
        elif t == "enum":
            opts = [str(o) for o in p["options"]]
            v = str(v) if v is not None else ""
            if v not in opts:
                raise ValueError(f"param {pid!r}: {v!r} is not one of {', '.join(opts)}")
        elif t in ASSET_TYPES:
            v = "" if v is None else str(v).strip()
            if v:
                if not ASSET_RE.match(v) or ".." in v:
                    raise ValueError(f"param {pid!r}: asset name {v!r} must be a plain file name "
                                     "(letters, digits, . _ -; no paths, no URLs)")
                ext = Path(v).suffix.lower()
                allowed = AUDIO_EXT if t == "audio" else IMAGE_EXT
                if p.get("accept"):
                    allowed = {str(x).lower() for x in p["accept"]} & allowed
                if ext not in allowed:
                    raise ValueError(f"param {pid!r}: {ext or 'no extension'} is not a supported "
                                     f"{t} type ({', '.join(sorted(allowed))})")
            elif p.get("required"):
                raise ValueError(f"param {pid!r} is required (upload a file)")
        elif t == "times":
            v = _times(v, pid)
        out[pid] = v
    _line_timing(manifest, decl, out)
    return out


def _line_timing(manifest, decl, values):
    """Longtext params with a "timing" block get per-line start/end times (see PLATFORM.md)."""
    values["__timing__"] = {}
    for pid, p in decl.items():
        if p["type"] != "longtext" or "timing" not in p:
            continue
        tm = p["timing"]

        def num(key, default):
            ref = tm.get(key)
            if ref is None:
                return default
            if isinstance(ref, (int, float)):
                return float(ref)
            if ref not in values or decl[ref]["type"] != "number":
                raise ValueError(f"{manifest.get('id')}: timing.{key} of {pid!r} must name a number param")
            return float(values[ref])

        bpm = num("bpm", 120.0)
        if bpm <= 0:
            raise ValueError(f"param bpm must be > 0, got {bpm:g}")
        beat = 60.0 / bpm
        offset = num("offset", 0.0)
        beats = num("beats", 4.0)
        gap = num("gap", 0.0)
        step = max(beats * beat, 0.05)
        ref = tm.get("times")
        given = values.get(ref, []) if ref else []
        lines = values[pid]
        spans = []
        t = offset
        for k in range(len(lines)):
            if k < len(given):
                start, end = given[k]
            elif given and k == len(given):
                # lines past the given times continue on the beat grid, from the next beat
                last_s, last_e = given[-1]
                base = last_e if last_e is not None else last_s + step
                start, end = offset + math.ceil(round((base - offset) / beat, 6)) * beat, None
            else:
                start, end = t, None
            spans.append([start, end])
            t = start + step
        for k, sp in enumerate(spans):
            if sp[1] is None:
                nxt = spans[k + 1][0] if k + 1 < len(spans) else sp[0] + step
                sp[1] = max(sp[0] + 0.05, nxt - gap)
        values["__timing__"][pid] = [tuple(s) for s in spans]


# ------------------------------------------------------------------ expressions

_FUNCS = {"min": min, "max": max, "round": round, "abs": abs, "floor": math.floor, "ceil": math.ceil,
          "clamp": lambda v, lo, hi: max(lo, min(hi, v))}


def _eval(expr: str, env: dict) -> float:
    try:
        tree = ast.parse(expr, mode="eval")
    except SyntaxError:
        raise ValueError(f"template expression {{{{= {expr}}}}} is not valid") from None

    def ev(n):
        if isinstance(n, ast.Expression):
            return ev(n.body)
        if isinstance(n, ast.Constant) and isinstance(n.value, (int, float)) and not isinstance(n.value, bool):
            return n.value
        if isinstance(n, ast.Name):
            if n.id in env and isinstance(env[n.id], (int, float)):
                return env[n.id]
            raise ValueError(f"template expression {{{{= {expr}}}}}: {n.id!r} is not a number")
        if isinstance(n, ast.UnaryOp) and isinstance(n.op, (ast.USub, ast.UAdd)):
            v = ev(n.operand)
            return -v if isinstance(n.op, ast.USub) else v
        if isinstance(n, ast.BinOp):
            a, b = ev(n.left), ev(n.right)
            ops = {ast.Add: lambda: a + b, ast.Sub: lambda: a - b, ast.Mult: lambda: a * b,
                   ast.Div: lambda: a / b, ast.FloorDiv: lambda: a // b, ast.Mod: lambda: a % b}
            f = ops.get(type(n.op))
            if f is None:
                raise ValueError(f"template expression {{{{= {expr}}}}}: operator not allowed")
            try:
                return f()
            except ZeroDivisionError:
                raise ValueError(f"template expression {{{{= {expr}}}}}: division by zero") from None
        if isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id in _FUNCS and not n.keywords:
            return _FUNCS[n.func.id](*[ev(a) for a in n.args])
        raise ValueError(f"template expression {{{{= {expr}}}}}: only numbers, params, + - * / // % "
                         "and min max round abs floor ceil clamp are allowed")
    return ev(tree)


def _fmt(x) -> str:
    if isinstance(x, bool):
        return "1" if x else "0"
    if isinstance(x, int):
        return str(x)
    x = float(x)
    if abs(x - round(x)) < 1e-9:
        return str(int(round(x)))
    return f"{x:.4f}".rstrip("0").rstrip(".")


# ------------------------------------------------------------------ substitution

def _parse(src: str):
    """Tokens -> tree: list of str | ("var", name) | ("expr", e) | ("sec", inv, name, children)."""
    root, stack, pos = [], [], 0
    cur = root
    for m in TAG_RE.finditer(src):
        sigil, name = m.group(1), m.group(2)
        start, end = m.start(), m.end()
        if sigil in ("#", "^", "/"):
            # standalone section tags (alone on their line) take the whole line with them
            ls = src.rfind("\n", 0, start) + 1
            le = src.find("\n", end)
            le = len(src) if le < 0 else le
            if not src[ls:start].strip() and not src[end:le].strip() and ls >= pos:
                start, end = ls, min(le + 1, len(src))
        if start > pos:
            cur.append(src[pos:start])
        pos = end
        if sigil in ("#", "^"):
            node = ("sec", sigil == "^", name, [])
            cur.append(node)
            stack.append((name, cur, src.count("\n", 0, m.start()) + 1))
            cur = node[3]
        elif sigil == "/":
            if not stack or stack[-1][0] != name:
                line = src.count("\n", 0, m.start()) + 1
                want = stack[-1][0] if stack else None
                raise ValueError(f"template line {line}: {{{{/{name}}}}} closes "
                                 + (f"{{{{#{want}}}}}" if want else "nothing"))
            _, cur, _ = stack.pop()
        elif sigil == "=":
            cur.append(("expr", name))
        else:
            cur.append(("var", name))
    if stack:
        name, _, line = stack[-1]
        raise ValueError(f"template line {line}: {{{{#{name}}}}} is never closed")
    cur.append(src[pos:])
    return root


def _render(nodes, values, decl, scope) -> str:
    out = []
    for n in nodes:
        if isinstance(n, str):
            out.append(n)
        elif n[0] == "var":
            out.append(_lookup(n[1], values, decl, scope))
        elif n[0] == "expr":
            out.append(_fmt(_eval(n[1], _numeric_env(values, decl, scope))))
        else:
            _, inv, name, kids = n
            if "=" in name:
                pid, want = (x.strip() for x in name.split("=", 1))
                show = (_raw(pid, values, decl, scope) == want) != inv
                if show:
                    out.append(_render(kids, values, decl, scope))
                continue
            if not inv and decl.get(name, {}).get("type") == "longtext" and name not in scope:
                lines = values[name]
                spans = values["__timing__"].get(name) or [(float(k), float(k + 1)) for k in range(len(lines))]
                for k, line in enumerate(lines):
                    s, e = spans[k]
                    loop = {"text": _escape(line), "len": len(line), "n": k + 1, "i": k, "start": s, "end": e, "dur": e - s,
                            "first": k == 0, "last": k == len(lines) - 1}
                    out.append(_render(kids, values, decl, {**scope, **loop}))
                continue
            truthy = _truthy(_raw(name, values, decl, scope))
            if truthy != inv:
                out.append(_render(kids, values, decl, scope))
    return "".join(out)


def _raw(name, values, decl, scope):
    if name in scope:
        return scope[name]
    if name in values and name in decl:
        return values[name]
    raise ValueError(f"template uses unknown name {name!r} (params: {', '.join(decl)})")


def _truthy(v) -> bool:
    if isinstance(v, (list, tuple, str)):
        return len(v) > 0
    return bool(v)


def _ramp(args, values, decl, scope) -> str:
    """{{ramp a b n}}: n colours from colour a to colour b (OKLab-free, linear sRGB mix),
    comma-separated, for colors=[...] lists. a/b are colour params or #hex literals."""
    mirror = args[3:] == ["mirror"]
    if mirror:
        args = args[:3]
    if len(args) != 3:
        raise ValueError("{{ramp a b n [mirror]}} takes two colours, a count and optionally mirror")

    def col(x):
        v = x if x.startswith("#") else _raw(x, values, decl, scope)
        if not isinstance(v, str) or not COLOR_RE.match(v):
            raise ValueError(f"{{{{ramp}}}}: {x!r} is not a colour")
        return [int(v[k:k + 2], 16) for k in (1, 3, 5)]
    a, b = col(args[0]), col(args[1])
    n = int(args[2]) if args[2].isdigit() else int(_eval(args[2], _numeric_env(values, decl, scope)))
    if not 2 <= n <= 256:
        raise ValueError("{{ramp a b n}}: n must be 2..256")

    def lin(c):
        c /= 255
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4

    def srgb(c):
        c = c * 12.92 if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055
        return max(0, min(255, round(c * 255)))
    out = []
    for k in range(n):
        f = k / (n - 1)
        if mirror:  # a -> b -> a (symmetric visualizers: bass in the middle / at 12 o'clock)
            f = 1 - abs(2 * k / n - 1) if n > 2 else f
        out.append("#" + "".join(f"{srgb(lin(x) + (lin(y) - lin(x)) * f):02X}" for x, y in zip(a, b)))
    return ", ".join(out)


def _lookup(name, values, decl, scope) -> str:
    if name.startswith("ramp "):
        return _ramp(name.split()[1:], values, decl, scope)
    if "." in name:
        pid, attr = name.split(".", 1)
        if decl.get(pid, {}).get("type") == "longtext" and attr in ("count", "end"):
            lines = values[pid]
            if attr == "count":
                return str(len(lines))
            spans = values["__timing__"].get(pid) or [(0.0, float(len(lines)))]
            return _fmt(spans[-1][1] if spans else 0)
        if decl.get(pid, {}).get("type") == "color" and attr == "rgb":
            return values[pid][:7]
        raise ValueError(f"template uses unknown name {name!r}")
    if name in scope:
        v = scope[name]
        return v if isinstance(v, str) else _fmt(v)
    v = _raw(name, values, decl, scope)
    t = decl[name]["type"]
    if t == "text":
        return _escape(v)
    if t == "number":
        return _fmt(v)
    if t in ("longtext", "times"):
        raise ValueError(f"template uses {{{{{name}}}}} directly; a {t} param is only used in a "
                         f"{{{{#{name}}}}} block" + (" or as {{" + name + ".count}}" if t == "longtext" else ""))
    return str(v)


def _numeric_env(values, decl, scope):
    env = {k: v for k, v in values.items() if k in decl and decl[k]["type"] == "number"}
    for pid, p in decl.items():
        if p["type"] == "longtext":
            env[f"{pid}_count"] = len(values[pid])
            spans = values["__timing__"].get(pid)
            env[f"{pid}_end"] = spans[-1][1] if spans else 0.0
        if p["type"] == "text":
            env[f"{pid}_len"] = len(values[pid])
    env.update({k: v for k, v in scope.items() if isinstance(v, (int, float)) and not isinstance(v, bool)})
    return env


def render_source(src: str, manifest: dict, params: dict | None) -> str:
    """Substitute an in-memory template source (for tests and editors)."""
    values = resolve_params(manifest, params)
    decl = {p["id"]: p for p in manifest.get("params", [])}
    out = _render(_parse(src), values, decl, {})
    if "{{" in out or "}}" in out:
        line = out[:out.find("{{") if "{{" in out else out.find("}}")].count("\n") + 1
        raise ValueError(f"unresolved template tag in output line {line}")
    return out


def instantiate(template_id: str, params: dict | None, templates_dir) -> str:
    """Return the Reel Script source of a template with params applied (raises ValueError)."""
    manifest = load_manifest(template_id, templates_dir)
    src = (template_dir(template_id, templates_dir) / "template.reel").read_text(encoding="utf-8")
    return render_source(src, manifest, params)


def default_assets(template_id: str, params: dict | None, templates_dir) -> dict[str, Path]:
    """Files from the template's own directory that the instantiated source references (default
    audio/images that the user did not replace): {name: path}. The service copies them into the
    job dir next to the instantiated source; user uploads are mapped by the service itself."""
    manifest = load_manifest(template_id, templates_dir)
    values = resolve_params(manifest, params)
    d = template_dir(template_id, templates_dir)
    out = {}
    for p in manifest.get("params", []):
        if p["type"] in ASSET_TYPES:
            v = values[p["id"]]
            if v and (d / v).is_file():
                out[v] = d / v
    for name in manifest.get("assets", []):
        if (d / name).is_file():
            out[name] = d / name
    return out


def _coerce_cli(manifest, k, v):
    decl = {p["id"]: p for p in manifest.get("params", [])}
    if decl.get(k, {}).get("type") == "longtext":
        if v.startswith("@"):
            return Path(v[1:]).read_text(encoding="utf-8")
        return v.replace("\\n", "\n")
    return v


def main(argv=None):
    here = Path(__file__).resolve().parent
    ap = argparse.ArgumentParser(description="Instantiate an AgentFX template (docs/PLATFORM.md).")
    ap.add_argument("id", nargs="?", help="template id (directory name)")
    ap.add_argument("--set", action="append", default=[], metavar="K=V",
                    help="param value; longtext: \\n separates lines, @file reads a file")
    ap.add_argument("-o", "--out", help="write the source here (default: stdout)")
    ap.add_argument("--templates", default=str(here), help="templates directory")
    ap.add_argument("--copy-assets", metavar="DIR", help="copy the default assets the source uses into DIR")
    ap.add_argument("--list", action="store_true", help="list template ids")
    a = ap.parse_args(argv)
    if a.list or not a.id:
        print("\n".join(list_templates(a.templates)))
        return 0
    try:
        manifest = load_manifest(a.id, a.templates)
        params = {}
        for kv in a.set:
            if "=" not in kv:
                raise ValueError(f"--set {kv!r}: use key=value")
            k, v = kv.split("=", 1)
            params[k.strip()] = _coerce_cli(manifest, k.strip(), v)
        src = instantiate(a.id, params, a.templates)
        if a.copy_assets:
            dest = Path(a.copy_assets)
            dest.mkdir(parents=True, exist_ok=True)
            for name, path in default_assets(a.id, params, a.templates).items():
                if not (dest / name).exists() or (dest / name).resolve() != path.resolve():
                    shutil.copyfile(path, dest / name)
    except ValueError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    if a.out:
        Path(a.out).write_text(src, encoding="utf-8")
    else:
        sys.stdout.write(src)
    return 0


if __name__ == "__main__":
    sys.exit(main())
