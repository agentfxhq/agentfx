"""Static checks on untrusted Reel Script before anything is compiled or rendered.

The compiler resolves file references relative to the .reel file and passes absolute paths,
`http..`/`data:` values and `file://` markup through (crates/script/src/elements.rs `asset`,
`absolutize_src`, lower.rs image paints, design.rs `source_of`, s3d.rs textures/meshes,
textv2.rs fonts, timing.rs transcripts, audio.rs music, cloud.rs point files, ...). The
service therefore checks the parsed document (`agentfx parse --to json`) and rejects:

  * in file-reference positions (FILE_PROPS, positional args of FILE_ARG_KINDS, `image(..)`
    paints, src/href/url() in raw svg/html markup): anything but a plain file name from the
    job's asset set (`#id` element references and dot-less builtin names such as `sfx click`
    are allowed; they cannot leave the job dir);
  * everywhere else: `file:` URLs, absolute paths (`/x`, `\\x`, `~/x`, `C:\\x`) and `..`
    path segments, so that a file-loading property this list does not know still cannot
    reach outside the job dir;
  * in markup: URLs (other than xmlns), `<!DOCTYPE`/`<!ENTITY`, `<script>`, `<link>`,
    `<base>`, `<iframe>`, `<object>`, `<embed>`, `<meta>`, `@import`.

The checks are a first layer; the sandbox (own uid, no network, job dir) is the second.
"""
from __future__ import annotations

import re

FILE_PROPS = {"src", "shot", "hotspots", "texture", "tex", "lut", "audio", "file", "poster", "model",
              "points", "mask-src", "font-src"}
# kinds whose positional args can name a file (image logo.png, music bed.mp3, sfx pop.wav, ...)
FILE_ARG_KINDS = {"image", "video", "music", "sfx", "swap", "mesh", "device", "font", "captions",
                  "cloud", "design", "html", "svg"}
FILE_EXT = re.compile(r"^\S+\.(png|jpe?g|gif|webp|avif|bmp|svg|mp3|wav|m4a|aac|ogg|opus|flac|mp4|m4v|mov|"
                      r"webm|mkv|avi|obj|glb|gltf|ttf|otf|ttc|woff2?|json|cube|srt|vtt|ass|reel|txt|csv|html?)$",
                      re.I)
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
SAFE_ID = re.compile(r"^#[A-Za-z0-9_.:-]{1,128}$")
SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")
TOKEN_SPLIT = re.compile(r"[\s,()\[\]{}'\"=;|<>]+")
IMAGE_PAINT = re.compile(r"image\(\s*(?:'([^']*)'|\"([^\"]*)\"|([^\s)]*))", re.I)
MARKUP_ATTR = re.compile(r"(?i)(?<![\w-])(src|href|xlink:href|srcset|poster|data|action|formaction|background|ping|"
                         r"lowsrc|dynsrc|longdesc|codebase|archive|cite|manifest|icon)\s*=\s*"
                         r"(?:\"([^\"]*)\"|'([^']*)'|([^\s>]+))")
CSS_URL = re.compile(r"(?i)url\(\s*(?:\"([^\"]*)\"|'([^']*)'|([^)]*))\s*\)")
XMLNS = re.compile(r"(?i)\sxmlns(:[\w-]+)?\s*=\s*(\"[^\"]*\"|'[^']*')")
MARKUP_BANNED = ("<!doctype", "<!entity", "<!attlist", "<!element", "<?xml-stylesheet", "<script", "<link",
                 "<base", "<iframe", "<frame", "<object", "<embed", "<meta", "<portal", "@import", "file:",
                 "javascript:")
SIZE = re.compile(r"^\s*(\d{2,5})\s*[xX]\s*(\d{2,5})\s*$")


class SourceError(ValueError):
    """The source is rejected. `code` goes into the API error."""

    def __init__(self, message, code="invalid_source"):
        super().__init__(message)
        self.code = code


def check_ref(value: str, where: str, assets: set) -> str | None:
    """A value in a file-reference position. Returns the asset name it uses, or None."""
    v = str(value).strip()
    if not v:
        return None
    if v.startswith("#"):
        if not SAFE_ID.match(v):
            raise SourceError(f"{where}: invalid element reference {v!r}")
        return None
    if "\x00" in v:
        raise SourceError(f"{where}: NUL byte in a file reference")
    if SCHEME.match(v) or v.startswith("//"):
        raise SourceError(f"{where}: URLs are not allowed ({v[:80]!r}); upload the file to /api/v1/assets "
                          "and use the returned name", "forbidden_reference")
    if "/" in v or "\\" in v or v.startswith("~") or ".." in v:
        raise SourceError(f"{where}: paths are not allowed ({v[:80]!r}); use the plain name returned by "
                          "/api/v1/assets", "forbidden_reference")
    if "." not in v:
        return None  # builtin name (e.g. `sfx click`); resolves inside the job dir at most
    if not SAFE_NAME.match(v):
        raise SourceError(f"{where}: invalid file name {v[:80]!r}", "forbidden_reference")
    if v not in assets:
        raise SourceError(f"{where}: unknown asset {v!r}: upload it to /api/v1/assets and use the "
                          "returned name", "unknown_asset")
    return v


def check_loose(value: str, where: str, assets: set, used: set):
    """Any other string value: no file URLs, absolute paths or `..`; `image(..)` paints are refs."""
    v = str(value)
    if "\x00" in v:
        raise SourceError(f"{where}: NUL byte")
    if "file:" in v.lower():
        raise SourceError(f"{where}: file: URLs are not allowed", "forbidden_reference")
    for tok in TOKEN_SPLIT.split(v):
        if not tok:
            continue
        if len(tok) > 1 and tok[0] in "/\\" or tok.startswith("~/") or re.match(r"^[A-Za-z]:[\\/]", tok):
            raise SourceError(f"{where}: absolute paths are not allowed ({tok[:80]!r})", "forbidden_reference")
        if ".." in re.split(r"[\\/]", tok) and ("/" in tok or "\\" in tok or tok == ".."):
            raise SourceError(f"{where}: `..` is not allowed ({tok[:80]!r})", "forbidden_reference")
    for m in IMAGE_PAINT.finditer(v):
        ref = next(g for g in m.groups() if g is not None)
        name = check_ref(ref, where + " image()", assets)
        if name:
            used.add(name)


def check_markup(body: str, where: str, assets: set, used: set | None = None):
    """Raw svg/html markup (inline bodies and uploaded .svg files)."""
    used = used if used is not None else set()
    if "\x00" in body:
        raise SourceError(f"{where}: NUL byte in markup")
    stripped = XMLNS.sub(" ", body)
    low = stripped.lower()
    for bad in MARKUP_BANNED:
        if bad in low:
            raise SourceError(f"{where}: `{bad}` is not allowed in markup", "forbidden_reference")
    if "://" in low:
        raise SourceError(f"{where}: URLs are not allowed in markup (only xmlns)", "forbidden_reference")
    refs = [next(g for g in m.groups()[1:] if g is not None) for m in MARKUP_ATTR.finditer(stripped)]
    refs += [next(g for g in m.groups() if g is not None) for m in CSS_URL.finditer(stripped)]
    for r in refs:
        r = r.strip()
        if r.startswith("#") or r == "":
            continue
        if r.lower().startswith("data:image/") and "," in r:
            continue
        name = check_ref(r, where, assets)
        if name:
            used.add(name)
    return used


def _pathlike(s: str) -> bool:
    low = s.lower()
    return ("/" in s or "\\" in s or s.startswith("~") or ".." in re.split(r"[\\/]", s)
            or re.match(r"^[a-z]:", low) is not None or "://" in s
            or low.startswith(("file:", "data:", "http:", "https:", "ftp:")))


def _strings(v):
    if isinstance(v, str):
        yield v
    elif isinstance(v, list):
        for x in v:
            yield from _strings(x)
    elif isinstance(v, dict):
        for x in v.values():
            yield from _strings(x)


def check_document(doc: dict, assets: set) -> dict:
    """Check a parsed document (`agentfx parse --to json`). Returns facts the service needs:
    {"used": set of asset names, "size": (w, h) or None, "fps": float}."""
    used: set = set()
    facts = {"used": used, "size": None, "fps": 30.0}
    if not isinstance(doc, dict) or not isinstance(doc.get("nodes"), list):
        raise SourceError("could not read the parsed document")

    def walk(nodes, depth, path):
        if not isinstance(nodes, list):
            return
        for i, n in enumerate(nodes):
            if not isinstance(n, dict):
                continue
            kind = str(n.get("kind", ""))
            line = (n.get("span") or {}).get("line") if isinstance(n.get("span"), dict) else None
            where = f"{kind}" + (f" (line {line})" if line else "") if kind else path
            if depth == 0 and kind == "video":
                props = n.get("props") or {}
                m = SIZE.match(str(props.get("size", "")))
                if m:
                    facts["size"] = (int(m.group(1)), int(m.group(2)))
                try:
                    facts["fps"] = float(props.get("fps", 30))
                except (TypeError, ValueError):
                    pass
                header = True
            else:
                header = False
            for k, v in (n.get("props") or {}).items():
                key = str(k).lower()
                if key in FILE_PROPS:
                    for s in _strings(v):
                        name = check_ref(s, f"{where} {k}=", assets)
                        if name:
                            used.add(name)
                else:
                    for s in _strings(v):
                        check_loose(s, f"{where} {k}=", assets, used)
            args = [a for a in (n.get("args") or []) if isinstance(a, str)]
            if not header:
                for a in args:
                    s = a.strip()
                    # a file-like argument (known extension) must be an asset; in kinds that load
                    # positional files anything path- or URL-like is rejected as well
                    if FILE_EXT.match(s) or (kind in FILE_ARG_KINDS and _pathlike(s)):
                        name = check_ref(s, f"{where} argument", assets)
                        if name:
                            used.add(name)
            if isinstance(n.get("body"), str):
                check_markup(n["body"], f"{where} body", assets, used)
            for k, v in n.items():
                if k in ("kind", "args", "props", "children", "body", "span"):
                    continue
                for s in _strings(v):
                    check_loose(s, f"{where} {k}", assets, used)
            walk(n.get("children"), depth + 1, where)

    walk(doc["nodes"], 0, "document")
    return facts


def short_side(facts) -> int:
    """Output short side in px: the header size= short side, else the default 1080 (the
    multi-format compile keeps the short side; spec section 11)."""
    s = facts.get("size")
    return min(s) if s else 1080


# ------------------------------------------------------------------ request params

AT = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_:.\-]{0,63}$")
FORMATS = ("16:9", "9:16", "1:1", "4:5")


def check_at(at) -> str:
    s = str(at).strip()
    if not AT.match(s):
        raise SourceError("`at` must be an anchor, scene:anchor or a time like 2.5s", "invalid_request")
    return s


def check_format(fmt) -> str | None:
    if fmt in (None, ""):
        return None
    if fmt not in FORMATS:
        raise SourceError(f"format must be one of {', '.join(FORMATS)}", "invalid_request")
    return fmt


def check_int(v, name, lo, hi, default):
    if v is None:
        return default
    if isinstance(v, bool) or not isinstance(v, (int, float, str)):
        raise SourceError(f"{name} must be an integer", "invalid_request")
    try:
        x = int(v)
    except ValueError:
        raise SourceError(f"{name} must be an integer", "invalid_request") from None
    if not lo <= x <= hi:
        raise SourceError(f"{name} must be between {lo} and {hi}", "invalid_request")
    return x
