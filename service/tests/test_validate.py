"""Path / reference validation and upload sniffing (no CLI needed)."""
import struct
import zlib

import pytest

from agentfx_service import validate
from agentfx_service.assets import AssetStore, sniff
from agentfx_service.validate import SourceError, check_document, check_markup

ASSETS = {"a0123456789abcdef01234567.png", "bed.mp3"}


def doc(*children, header=None):
    return {"reel": "0", "nodes": [{"kind": "video", "args": ["t"], "props": header or {"size": "1280x720"}},
                                   {"kind": "scene", "args": ["s"], "props": {"dur": "2s"}, "children": list(children)}]}


def node(kind, args=(), body=None, **props):
    n = {"kind": kind, "args": list(args), "props": props}
    if body is not None:
        n["body"] = body
    return n


@pytest.mark.parametrize("n", [
    node("image", ["#i"], src="/etc/passwd"),
    node("image", ["#i"], src="../secret.png"),
    node("image", ["#i"], src="sub/../../x.png"),
    node("image", ["#i"], src="http://169.254.169.254/latest"),
    node("image", ["#i"], src="https://example.com/a.png"),
    node("image", ["#i"], src="file:///etc/passwd"),
    node("image", ["#i"], src="data:image/png;base64,AAAA"),
    node("image", ["#i"], src="~/x.png"),
    node("image", ["#i"], src="C:\\x.png"),
    node("image", ["#i"], src="//host/x.png"),
    node("image", ["#i", "/etc/hosts"]),
    node("music", ["../../bed.mp3"]),
    node("video", ["#v", "http://x/y.mp4"]),
    node("sfx", ["/usr/share/sounds/x.wav"]),
    node("text", ["#t", "hi"], fill="image(/etc/passwd)"),
    node("text", ["#t", "hi"], fill="image('../a.png' cover)"),
    node("card", ["#c"], skin="/etc/x"),
    node("design", [], lens="../../../etc"),
    node("text", ["#t", "x"], font="file:///etc/fonts/a.ttf"),
    node("svg", ["#s"], body='<svg xmlns="http://www.w3.org/2000/svg"><image href="/etc/passwd"/></svg>'),
    node("svg", ["#s"], body='<svg><image xlink:href="file:///etc/passwd"/></svg>'),
    node("svg", ["#s"], body='<svg><image href="http://evil/x.png"/></svg>'),
    node("svg", ["#s"], body='<!DOCTYPE svg [<!ENTITY x SYSTEM "file:///etc/passwd">]><svg>&x;</svg>'),
    node("html", ["#h"], body='<div style="background:url(/etc/passwd)"></div>'),
    node("html", ["#h"], body='<link rel="stylesheet" href="x.css">'),
    node("html", ["#h"], body='<style>@import "x.css";</style>'),
    node("html", ["#h"], body="<img src='../../x.png'>"),
    node("html", ["#h"], body="<img src=../../x.png>"),
    node("svg", ["#s"], src="/etc/passwd"),
    node("mesh", ["#m"], src="model.obj"),          # unknown asset
    node("image", ["#i"], src="other.png"),          # unknown asset
    node("device", ["#d"], shot="../shot.png"),
    node("swap", ["#app", "next.png"]),               # unknown asset
    node("captions", [], src="talk.srt"),
    node("cloud", ["#c"], points="/tmp/p.json"),
])
def test_rejects(n):
    with pytest.raises(SourceError):
        check_document(doc(n), ASSETS)


@pytest.mark.parametrize("n", [
    node("image", ["#i"], src="a0123456789abcdef01234567.png"),
    node("image", ["#i", "a0123456789abcdef01234567.png", "cover"]),
    node("music", ["bed.mp3"], vol=0.25),
    node("sfx", ["click"]),
    node("screen", ["#crt"], src="#ui"),
    node("text", ["#t", "50/50 split, see https://agentfx.dev and /r/agentfx"], sub="a / b", rate="+6%"),
    node("fluid", ["#ink"], res="1/4", at="50%,50%"),
    node("text", ["#t", "hi"], fill="image(a0123456789abcdef01234567.png cover)"),
    node("svg", ["#s"], body='<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">'
                             '<use href="#a"/><image href="a0123456789abcdef01234567.png"/>'
                             '<image href="data:image/png;base64,AAAA"/></svg>'),
    node("path", ["#p"], d="M0 160 C240 0 480 0 720 160"),
])
def test_accepts(n):
    facts = check_document(doc(n), ASSETS)
    assert facts["size"] == (1280, 720)


def test_header_title_with_dots_is_not_a_file():
    check_document(doc(header={"size": "1920x1080"}), ASSETS)
    d = doc()
    d["nodes"][0]["args"] = ["AgentFX v0.1 release.notes"]
    check_document(d, ASSETS)


def test_used_assets_are_collected():
    f = check_document(doc(node("image", ["#i"], src="a0123456789abcdef01234567.png"), node("music", ["bed.mp3"])), ASSETS)
    assert f["used"] == ASSETS


def test_short_side():
    assert validate.short_side({"size": (1280, 720)}) == 720
    assert validate.short_side({"size": None}) == 1080


@pytest.mark.parametrize("at", ["--ref", "-1", "", "a b", "x" * 80, "../x"])
def test_bad_at(at):
    with pytest.raises(SourceError):
        validate.check_at(at)


@pytest.mark.parametrize("at", ["2.5s", "12", "intro:hook", "beat:4"])
def test_good_at(at):
    assert validate.check_at(at) == at


def test_markup_upload_rules():
    check_markup('<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>', "svg", set())
    with pytest.raises(SourceError):
        check_markup('<svg><script>alert(1)</script></svg>', "svg", set())
    with pytest.raises(SourceError):
        check_markup('<svg><image href="a.png"/></svg>', "svg", set())


def _png(w, h):
    raw = b"".join(b"\x00" + b"\x00\x00\x00" * w for _ in range(h))
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + \
        chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def test_sniff():
    assert sniff(_png(2, 2)) == ("image", "png")
    assert sniff(b"\xff\xd8\xff\xe0" + b"\0" * 20) == ("image", "jpg")
    assert sniff(b"ID3\x04" + b"\0" * 20) == ("audio", "mp3")
    assert sniff(b"RIFF\0\0\0\0WAVEfmt ") == ("audio", "wav")
    assert sniff(b"wOF2" + b"\0" * 20) == ("font", "woff2")
    assert sniff(b'<svg xmlns="http://www.w3.org/2000/svg"></svg>') == ("svg", "svg")
    assert sniff(b"#EXTM3U\nhttp://evil/x") is None
    assert sniff(b"<html><body>hi</body></html>") is None
    assert sniff(b"\x7fELF\x02\x01") is None


def test_upload_caps(cfg):
    st = AssetStore(cfg)
    with pytest.raises(SourceError):
        st.check(_png(8001, 10))           # side over the cap
    with pytest.raises(SourceError):
        st.check(b"GIF89a" + b"\0" * 10)  # unsupported type
    with pytest.raises(SourceError):
        st.check(b'<svg><image href="file:///etc/passwd"/></svg>')
    meta = st.put(_png(4, 4), "ip:1.2.3.4", 1)
    assert meta["name"].endswith(".png") and st.path(meta["name"]).is_file()
    assert st.path("a" + "0" * 24 + ".png") is None
    assert st.name_for(meta["asset_id"]) == meta["name"]
