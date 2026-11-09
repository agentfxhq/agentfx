#!/usr/bin/env python3
"""Instantiate an AgentFX template (docs/PLATFORM.md, "Templates"). Stdlib only.

Minimal implementation by the render service package (P1); the templates package (P3) owns
this file and extends it (longtext blocks etc.). The service imports:
list_templates, load_manifest, template_dir, instantiate.

    python3 templates/render_template.py <id> [--set k=v ...] [-o out.reel]
    python3 templates/render_template.py --list
"""
from __future__ import annotations

import argparse
import csv
import io
import json
import math
import re
import sys
from pathlib import Path

ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,47}$")
COLOR_RE = re.compile(r"^#[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$")
ASSET_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
TAG_RE = re.compile(r"\{\{\s*([a-z][a-z0-9_]{0,31})\s*\}\}")
BLOCK_RE = re.compile(r"\{\{#([a-z][a-z0-9_]{0,31})\}\}(.*?)\{\{/\1\}\}", re.S)
MAX_DATA_ROWS = 500
MAX_DATA_BYTES = 64 * 1024


def _esc(v) -> str:
    return " ".join(str(v).split()).replace("\\", "\\\\").replace('"', '\\"')


def parse_data(v, pid="data") -> list:
    """A `data` param: JSON array of objects (or a JSON string of one), or CSV text with a
    header row -> list of {column: str}. At most 500 rows / 64 KB."""
    if v is None or v == "":
        return []
    raw = v if isinstance(v, str) else json.dumps(v)
    if len(raw.encode("utf-8")) > MAX_DATA_BYTES:
        raise ValueError(f"param {pid!r}: data is larger than 64 KB")
    if isinstance(v, str):
        t = v.strip()
        if t.startswith("["):
            v = json.loads(t)
        else:
            v = list(csv.DictReader(io.StringIO(t)))
    if not isinstance(v, list) or not all(isinstance(r, dict) for r in v):
        raise ValueError(f"param {pid!r}: data must be a list of objects or CSV with a header row")
    if len(v) > MAX_DATA_ROWS:
        raise ValueError(f"param {pid!r}: at most {MAX_DATA_ROWS} rows")
    rows = []
    for r in v:
        row = {}
        for k, x in r.items():
            k = re.sub(r"[^a-z0-9_]", "_", str(k).strip().lower())[:32]
            if not k or not k[0].isalpha():
                raise ValueError(f"param {pid!r}: bad column name {k!r}")
            row[k] = "" if x is None else str(x)
        rows.append(row)
    return rows


def template_dir(template_id, templates_dir) -> Path:
    if not isinstance(template_id, str) or not ID_RE.match(template_id):
        raise ValueError(f"invalid template id {template_id!r}")
    d = Path(templates_dir) / template_id
    if not (d / "manifest.json").is_file() or not (d / "template.reel").is_file():
        raise ValueError(f"unknown template {template_id!r}")
    return d


def list_templates(templates_dir) -> list:
    root = Path(templates_dir)
    if not root.is_dir():
        return []
    return sorted(p.name for p in root.iterdir()
                  if ID_RE.match(p.name) and (p / "manifest.json").is_file() and (p / "template.reel").is_file())


def load_manifest(template_id, templates_dir) -> dict:
    with open(template_dir(template_id, templates_dir) / "manifest.json", encoding="utf-8") as f:
        return json.load(f)


def resolve_params(manifest: dict, params: dict | None) -> dict:
    params = dict(params or {})
    decl = {p["id"]: p for p in manifest.get("params", [])}
    unknown = sorted(set(params) - set(decl))
    if unknown:
        raise ValueError(f"unknown param(s): {', '.join(unknown)}")
    out = {}
    for pid, p in decl.items():
        t = p["type"]
        v = params.get(pid, p.get("default"))
        if t in ("text", "longtext"):
            v = "" if v is None else str(v)
            if t == "text" and ("\n" in v or "\r" in v):
                raise ValueError(f"param {pid!r}: newlines are not allowed")
            if p.get("max_len") and len(v) > p["max_len"]:
                raise ValueError(f"param {pid!r}: at most {p['max_len']} characters")
            v = v.replace("\\", "\\\\").replace('"', '\\"')
        elif t == "color":
            v = str(v or "")
            if not COLOR_RE.match(v):
                raise ValueError(f"param {pid!r}: colour must look like #RRGGBB")
        elif t == "number":
            try:
                x = float(v)
            except (TypeError, ValueError):
                raise ValueError(f"param {pid!r}: expected a number") from None
            if not math.isfinite(x):
                raise ValueError(f"param {pid!r}: expected a finite number")
            x = min(float(p.get("max", x)), max(float(p.get("min", x)), x))
            v = str(int(x)) if x == int(x) else f"{x:g}"
        elif t == "enum":
            if str(v) not in [str(o) for o in p.get("options", [])]:
                raise ValueError(f"param {pid!r}: must be one of {p.get('options')}")
            v = str(v)
        elif t == "data":
            out[pid] = parse_data(v, pid)
            continue
        elif t in ("audio", "image"):
            v = "" if v is None else str(v)
            if v and (not ASSET_RE.match(v) or ".." in v):
                raise ValueError(f"param {pid!r}: must be a plain asset name")
        out[pid] = v
    return out


def instantiate(template_id, params, templates_dir) -> str:
    manifest = load_manifest(template_id, templates_dir)
    src = (template_dir(template_id, templates_dir) / "template.reel").read_text(encoding="utf-8")
    values = resolve_params(manifest, params)

    def block(m):
        rows = values.get(m.group(1))
        if not isinstance(rows, list):
            raise ValueError(f"{{{{#{m.group(1)}}}}} needs a data param")
        out = []
        for i, row in enumerate(rows):
            scope = {"i": str(i), "n": str(i + 1), **{k: _esc(x) for k, x in row.items()}}
            out.append(TAG_RE.sub(lambda t: scope.get(t.group(1), t.group(0)), m.group(2)))
        return "".join(out)
    src = BLOCK_RE.sub(block, src)

    def sub(m):
        if m.group(1) not in values:
            raise ValueError(f"template uses unknown param {m.group(1)!r}")
        return values[m.group(1)]
    return TAG_RE.sub(sub, src)


def main(argv=None):
    here = Path(__file__).resolve().parent
    ap = argparse.ArgumentParser(description="Instantiate an AgentFX template.")
    ap.add_argument("id", nargs="?")
    ap.add_argument("--set", action="append", default=[], metavar="K=V")
    ap.add_argument("-o", "--out")
    ap.add_argument("--templates", default=str(here))
    ap.add_argument("--list", action="store_true")
    a = ap.parse_args(argv)
    if a.list or not a.id:
        print("\n".join(list_templates(a.templates)))
        return 0
    try:
        params = dict(kv.split("=", 1) for kv in a.set)
        src = instantiate(a.id, params, a.templates)
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
