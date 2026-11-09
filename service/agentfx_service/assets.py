"""Uploaded assets: type sniffing (magic bytes, never the client's file name or content type),
size caps, image dimension caps, SVG markup checks; flat store `assets/<id>/<id>.<ext>`.

Sources reference an asset by its name (`a1b2...e.png`). Template default assets (files next
to a template.reel, e.g. a demo song) form a read-only library usable by every source.
"""
from __future__ import annotations

import json
import re
import secrets
import shutil
import struct
import time
from pathlib import Path

from .validate import SourceError, check_markup

ASSET_ID = re.compile(r"^a[0-9a-f]{24}$")
NAME = re.compile(r"^(a[0-9a-f]{24})\.([a-z0-9]{2,5})$")
MAX_PIXELS = 40_000_000
MAX_SIDE = 8000
LIBRARY_EXT = {".mp3", ".wav", ".m4a", ".ogg", ".flac", ".png", ".jpg", ".jpeg", ".svg", ".ttf", ".otf",
               ".woff", ".woff2"}


class UploadError(SourceError):
    pass


def sniff(data: bytes) -> tuple[str, str] | None:
    """(kind, ext) from the content, or None."""
    h = data[:64]
    if h.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image", "png"
    if h.startswith(b"\xff\xd8\xff"):
        return "image", "jpg"
    if h.startswith(b"ID3") or (len(h) > 2 and h[0] == 0xFF and (h[1] & 0xE0) == 0xE0):
        return "audio", "mp3"
    if h.startswith(b"RIFF") and h[8:12] == b"WAVE":
        return "audio", "wav"
    if h.startswith(b"OggS"):
        return "audio", "ogg"
    if h.startswith(b"fLaC"):
        return "audio", "flac"
    if h[4:8] == b"ftyp" and h[8:12] in (b"M4A ", b"M4B "):
        return "audio", "m4a"
    if h.startswith((b"\x00\x01\x00\x00", b"true")):
        return "font", "ttf"
    if h.startswith(b"OTTO"):
        return "font", "otf"
    if h.startswith(b"wOFF"):
        return "font", "woff"
    if h.startswith(b"wOF2"):
        return "font", "woff2"
    t = data[:4096].lstrip(b"\xef\xbb\xbf \t\r\n")
    if t.startswith((b"<?xml", b"<svg", b"<!--")) and b"<svg" in data[:65536]:
        return "svg", "svg"
    return None


def png_size(data: bytes):
    if len(data) < 24 or data[12:16] != b"IHDR":
        return None
    return struct.unpack(">II", data[16:24])


def jpeg_size(data: bytes):
    i, n = 2, len(data)
    while i + 9 < n:
        if data[i] != 0xFF:
            i += 1
            continue
        marker = data[i + 1]
        if marker in (0xD8, 0x01) or 0xD0 <= marker <= 0xD7:
            i += 2
            continue
        seg = struct.unpack(">H", data[i + 2:i + 4])[0]
        if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
            h, w = struct.unpack(">HH", data[i + 5:i + 9])
            return w, h
        i += 2 + seg
    return None


class AssetStore:
    def __init__(self, cfg):
        self.cfg = cfg
        self.root = cfg.assets_dir
        self.root.mkdir(parents=True, exist_ok=True)
        self.library: dict[str, Path] = {}
        self.load_library()

    def load_library(self):
        lib = {}
        root = self.cfg.templates_dir
        if root.is_dir():
            for t in sorted(root.iterdir()):
                if not t.is_dir() or t.name.startswith((".", "_")):
                    continue
                for f in sorted(t.iterdir()):
                    if f.is_file() and f.suffix.lower() in LIBRARY_EXT and re.match(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$", f.name) \
                            and not f.name.startswith("preview."):
                        lib.setdefault(f.name, f)
        self.library = lib

    def cap(self, kind: str) -> int:
        c = self.cfg
        return {"audio": c.max_audio_mb << 20, "image": c.max_image_mb << 20, "svg": c.max_svg_kb << 10,
                "font": c.max_font_mb << 20}[kind]

    def max_upload(self) -> int:
        c = self.cfg
        return max(c.max_audio_mb << 20, c.max_image_mb << 20, c.max_svg_kb << 10, c.max_font_mb << 20)

    def check(self, data: bytes) -> tuple[str, str]:
        t = sniff(data)
        if not t:
            raise UploadError("unsupported file type: upload PNG/JPEG images, SVG, MP3/WAV/OGG/FLAC/M4A audio "
                              "or TTF/OTF/WOFF/WOFF2 fonts", "unsupported_type")
        kind, ext = t
        if len(data) > self.cap(kind):
            raise UploadError(f"{kind} files are limited to {self.cap(kind) >> 10} KB", "too_large")
        if kind == "image":
            wh = png_size(data) if ext == "png" else jpeg_size(data)
            if not wh:
                raise UploadError("could not read the image size", "unsupported_type")
            w, h = wh
            if w <= 0 or h <= 0 or w > MAX_SIDE or h > MAX_SIDE or w * h > MAX_PIXELS:
                raise UploadError(f"images are limited to {MAX_SIDE}x{MAX_SIDE} and {MAX_PIXELS // 1_000_000} MP", "too_large")
        if kind == "svg":
            try:
                text = data.decode("utf-8")
            except UnicodeDecodeError:
                raise UploadError("SVG must be UTF-8", "unsupported_type") from None
            check_markup(text, "uploaded svg", set())
        return kind, ext

    def put(self, data: bytes, client: str, retention_h: float) -> dict:
        kind, ext = self.check(data)
        aid = "a" + secrets.token_hex(12)
        name = f"{aid}.{ext}"
        d = self.root / aid
        d.mkdir(mode=0o700)
        with open(d / name, "wb") as f:
            f.write(data)
        meta = {"asset_id": aid, "name": name, "kind": kind, "size": len(data), "client": client,
                "created": time.time(), "expires": time.time() + retention_h * 3600}
        (d / "meta.json").write_text(json.dumps(meta))
        return meta

    def path(self, name: str) -> Path | None:
        """File of an asset name (uploaded or library), or None."""
        m = NAME.match(name)
        if m:
            p = self.root / m.group(1) / name
            if p.is_file():
                meta = self.meta(m.group(1))
                if meta and meta.get("expires", 0) > time.time():
                    return p
            return None
        return self.library.get(name)

    def meta(self, aid: str) -> dict | None:
        if not ASSET_ID.match(aid):
            return None
        try:
            return json.loads((self.root / aid / "meta.json").read_text())
        except (OSError, ValueError):
            return None

    def name_for(self, value: str) -> str:
        """An asset id or name (template params) -> the name, if known; else the value."""
        if ASSET_ID.match(value or ""):
            m = self.meta(value)
            if m:
                return m["name"]
        return value

    def known(self, names) -> set:
        return {n for n in names if self.path(n)}

    def copy_into(self, names, job_dir: Path):
        for n in names:
            p = self.path(n)
            if p is None:
                raise SourceError(f"unknown asset {n!r}", "unknown_asset")
            shutil.copyfile(p, job_dir / n)

    def cleanup(self):
        now = time.time()
        for d in self.root.iterdir():
            m = self.meta(d.name)
            if m is None or m.get("expires", 0) < now:
                shutil.rmtree(d, ignore_errors=True)


def candidate_names(source: str) -> set:
    """Names in the source that could be assets (before parsing): every token that looks like
    a file name. The validator decides which ones are actually referenced."""
    return set(re.findall(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\.[A-Za-z0-9]{2,5}", source))
