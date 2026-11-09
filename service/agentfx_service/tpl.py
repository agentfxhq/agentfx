"""Templates (docs/PLATFORM.md): manifests, sources and instantiation via
templates/render_template.py (the single implementation of substitution, imported here)."""
from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


class Templates:
    def __init__(self, root: Path):
        self.root = Path(root)
        self._mod = None

    @property
    def mod(self):
        if self._mod is None:
            p = self.root / "render_template.py"
            if not p.is_file():
                raise RuntimeError(f"{p} not found")
            spec = importlib.util.spec_from_file_location("render_template", p)
            m = importlib.util.module_from_spec(spec)
            sys.modules.setdefault("render_template", m)
            spec.loader.exec_module(m)
            self._mod = m
        return self._mod

    def ids(self) -> list[str]:
        try:
            return self.mod.list_templates(self.root)
        except RuntimeError:
            return []

    def manifest(self, tid: str) -> dict:
        return self.mod.load_manifest(tid, self.root)  # ValueError for unknown / invalid ids

    def source(self, tid: str) -> str:
        return (self.mod.template_dir(tid, self.root) / "template.reel").read_text(encoding="utf-8")

    def preview(self, tid: str) -> Path | None:
        try:
            d = self.mod.template_dir(tid, self.root)
        except ValueError:
            return None
        p = d / "preview.webp"
        return p if p.is_file() else None

    def instantiate(self, tid: str, params: dict) -> str:
        return self.mod.instantiate(tid, params, self.root)

    def asset_params(self, tid: str) -> set:
        return {p["id"] for p in self.manifest(tid).get("params", []) if p.get("type") in ("audio", "image")}


def dumps(o) -> str:
    return json.dumps(o, ensure_ascii=False)
