"""Playground additions (docs/PLATFORM.md, P2): /preview, /fonts, /catalog, `size` on preview ops,
`size_px` on templates."""
import struct

from conftest import QUICKSTART, needs_cli

SIMPLE = 'video "x" size=1280x720 captions=off\nscene a dur=1s bg=#101010\n  text #t "Hello" size=80 color=#FFFFFF x=50% y=50% anchor=center\n'


def _png_size(b):
    assert b[:8] == b"\x89PNG\r\n\x1a\n"
    return struct.unpack(">II", b[16:24])


def test_size_px(client):
    ts = {t["id"]: t for t in client.get("/api/v1/templates").json()}
    assert ts["hello-card"]["sizes"] == ["instagram-post", "story", "a4-flyer"]
    assert ts["hello-card"]["size_px"] == {"instagram-post": "1080x1080", "story": "1080x1920", "a4-flyer": "2480x3508"}
    assert ts["hello-ink"]["size_px"] == {}
    t = client.get("/api/v1/templates/hello-card").json()
    assert t["size_px"]["story"] == "1080x1920"


def test_instantiate_size(client):
    r = client.post("/api/v1/instantiate", json={"template": "hello-card", "size": "story"})
    assert r.status_code == 200, r.text
    assert r.json()["source"].startswith('video "Hello card" size=1080x1920')
    r = client.post("/api/v1/instantiate", json={"template": "hello-card", "size": "x-post"})
    assert r.status_code == 422


def test_font_whitelist(client):
    r = client.get("/api/v1/fonts/Inter.ttf")
    assert r.status_code == 200
    assert r.headers["access-control-allow-origin"] == "*"
    assert r.headers["content-type"] == "font/ttf"
    for bad in ("fonts.json", "..%2Fsecret.ttf", "OFL.txt", ".Inter.ttf", "nope.ttf"):
        assert client.get(f"/api/v1/fonts/{bad}").status_code == 404, bad


def test_size_validation(client):
    for bad in ("big", "10x10", "9000x100", 1080):
        r = client.post("/api/v1/frame", json={"source": SIMPLE, "at": "0.5s", "size": bad})
        assert r.status_code in (400, 422), (bad, r.text)
    # free tier: 12 MP per still
    r = client.post("/api/v1/frame", json={"source": SIMPLE, "at": "0.5s", "size": "4000x4000"})
    assert r.status_code == 422 and r.json()["error"]["code"] == "limit_exceeded"


@needs_cli
def test_frame_sheet_size(client):
    r = client.post("/api/v1/frame", json={"source": SIMPLE, "at": "0.5s", "size": "1080x1920", "width": 270})
    assert r.status_code == 200, r.text
    w, h = _png_size(r.content)
    assert (w, h) == (270, 480)
    # an A4 page (short side 2480) is fine with an explicit size (the megapixel cap applies)
    r = client.post("/api/v1/frame", json={"source": SIMPLE, "at": "0.5s", "size": "2480x3508", "width": 320})
    assert r.status_code == 200, r.text
    r = client.post("/api/v1/sheet", json={"source": SIMPLE, "n": 2, "size": "1080x1080"})
    assert r.status_code == 200, r.text


@needs_cli
def test_lint_timing_size(client):
    r = client.post("/api/v1/lint", json={"source": SIMPLE, "size": "1080x1920"})
    assert r.status_code == 200, r.text
    assert isinstance(r.json()["diagnostics"], list)
    r = client.post("/api/v1/timing", json={"source": SIMPLE, "size": "1080x1920"})
    assert r.status_code == 200, r.text


@needs_cli
def test_preview_page(client):
    r = client.post("/api/v1/preview", json={"source": QUICKSTART})
    assert r.status_code == 200, r.text
    assert r.headers["content-type"].startswith("text/html")
    html = r.text
    assert "window.__reel" in html or "__reel" in html
    assert "file://" not in html
    assert "/api/v1/fonts/Inter.ttf" in html
    r = client.post("/api/v1/preview", json={"source": SIMPLE, "size": "1080x1920"})
    assert r.status_code == 200, r.text
    assert "1080px" in r.text and "1920px" in r.text


@needs_cli
def test_preview_inlines_job_assets(client, tmp_path):
    from test_validate import _png
    up = client.post("/api/v1/assets", files={"file": ("a.png", _png(8, 8), "image/png")})
    assert up.status_code == 201, up.text
    name = up.json()["name"]
    src = SIMPLE + f"  image #i src={name} x=10 y=10 w=100 h=100\n"
    r = client.post("/api/v1/preview", json={"source": src})
    assert r.status_code == 200, r.text
    assert "file://" not in r.text and "data:image/png;base64," in r.text


@needs_cli
def test_catalog(client):
    r = client.get("/api/v1/catalog")
    assert r.status_code == 200, r.text
    j = r.json()
    assert j and client.get("/api/v1/catalog").json() == j


def test_legacy_size_map_is_listed(client, cfg):
    import json
    d = cfg.templates_dir / "legacy-card"
    d.mkdir()
    (d / "template.reel").write_text('video "x" size=1080x1080\nscene a dur=1s\n  text #t "{{title}}"\n')
    (d / "manifest.json").write_text(json.dumps({"id": "legacy-card", "kind": "still", "title": "L", "description": "",
                                                  "tags": [], "formats": [], "sizes": {"story": "1080x1920"},
                                                  "params": [{"id": "title", "type": "text", "label": "T", "default": "x"}]}))
    t = client.get("/api/v1/templates/legacy-card").json()
    assert t["sizes"] == ["story"] and t["size_px"] == {"story": "1080x1920"}
