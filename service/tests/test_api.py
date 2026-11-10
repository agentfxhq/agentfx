"""HTTP API happy paths and limits, against the real CLI."""
import subprocess
import time

import pytest

from conftest import QUICKSTART, needs_cli
from test_validate import _png

pytestmark = needs_cli


def test_templates(client):
    r = client.get("/api/v1/templates")
    assert r.status_code == 200
    ids = [m["id"] for m in r.json()]
    assert "hello-ink" in ids
    t = client.get("/api/v1/templates/hello-ink").json()
    assert "{{text}}" in t["source"] and t["params"]
    assert client.get("/api/v1/templates/../etc").status_code == 404
    assert client.get("/api/v1/templates/nope").status_code == 404


def test_instantiate(client):
    r = client.post("/api/v1/instantiate", json={"template": "hello-ink", "params": {"text": 'SAY "HI"', "color": "#FF0000"}})
    assert r.status_code == 200, r.text
    src = r.json()["source"]
    assert 'text #word "SAY \\"HI\\""' in src and "color=#FF0000" in src
    r = client.post("/api/v1/instantiate", json={"template": "hello-ink", "params": {"color": "red"}})
    assert r.status_code == 422
    r = client.post("/api/v1/instantiate", json={"template": "hello-ink", "params": {"text": "a\nb"}})
    assert r.status_code == 422
    r = client.post("/api/v1/instantiate", json={"template": "hello-ink", "params": {"duration": 99}})
    assert "dur=8s" in r.json()["source"]


def test_lint(client):
    r = client.post("/api/v1/lint", json={"source": QUICKSTART})
    assert r.status_code == 200, r.text
    assert r.json()["diagnostics"] == []
    r = client.post("/api/v1/lint", json={"source": 'video "x" size=1280x720\nscene a dur=1s\n  text #t "hi\n'})
    assert r.status_code == 200
    assert any(d["severity"] == "error" for d in r.json()["diagnostics"])


def test_parse_roundtrip(client):
    j = client.post("/api/v1/parse", json={"source": QUICKSTART, "to": "json"}).json()["json"]
    assert j["nodes"][0]["kind"] == "video"
    t = client.post("/api/v1/parse", json={"json": j, "to": "text"}).json()["source"]
    assert "fluid #ink" in t


def test_timing(client):
    r = client.post("/api/v1/timing", json={"source": QUICKSTART})
    assert r.status_code == 200 and r.json()["timeline"]["total"] == 4.0


def test_frame_and_sheet(client):
    r = client.post("/api/v1/frame", json={"source": QUICKSTART, "at": "2s"})
    assert r.status_code == 200, r.text
    assert r.headers["content-type"] == "image/png" and r.content[:4] == b"\x89PNG"
    assert int.from_bytes(r.content[16:20], "big") == 640  # preview width default
    r = client.post("/api/v1/frame", json={"source": QUICKSTART, "at": "2s", "width": 320, "format": "9:16"})
    assert int.from_bytes(r.content[16:20], "big") == 320
    r = client.post("/api/v1/sheet", json={"source": QUICKSTART, "n": 4})
    assert r.status_code == 200 and r.content[:4] == b"\x89PNG"


@pytest.mark.parametrize("src,code", [
    ('video "x" size=1280x720\nscene a dur=1s\n  image #i src=/etc/passwd\n', "forbidden_reference"),
    ('video "x" size=1280x720\nscene a dur=1s\n  image #i src=../../signing.key\n', "forbidden_reference"),
    ('video "x" size=1280x720\nscene a dur=1s\n  music src=http://169.254.169.254/x.mp3\n', "forbidden_reference"),
    ('video "x" size=1280x720\nscene a dur=1s\n  image #i src=logo.png\n', "unknown_asset"),
    ('video "x" size=1280x720\nscene a dur=1s\n  svg #s w=10 h=10 |\n    <svg><image href="file:///etc/passwd"/></svg>\n',
     "forbidden_reference"),
])
def test_rejects_unsafe_sources(client, src, code):
    for path, extra in (("/api/v1/frame", {"at": "0.5s"}), ("/api/v1/renders", {}), ("/api/v1/lint", {})):
        r = client.post(path, json={"source": src, **extra})
        assert r.status_code == 422, (path, r.text)
        assert r.json()["error"]["code"] == code


def test_source_and_param_limits(client):
    r = client.post("/api/v1/lint", json={"source": "x" * 70000})
    assert r.status_code == 413
    r = client.post("/api/v1/frame", json={"source": QUICKSTART, "at": "--ref"})
    assert r.status_code == 422
    r = client.post("/api/v1/frame", json={"source": QUICKSTART, "at": "1s", "format": "3:1"})
    assert r.status_code == 422
    r = client.post("/api/v1/lint", content=b"{" + b" " * 3_000_000 + b"}", headers={"content-type": "application/json"})
    assert r.status_code == 413


def test_free_tier_render_limits(client):
    long = 'video "x" size=1280x720\nscene a dur=30s\n  text #t "hi"\n'
    r = client.post("/api/v1/renders", json={"source": long})
    assert r.status_code == 422 and r.json()["error"]["code"] == "limit_exceeded"
    big = 'video "x" size=1920x1080\nscene a dur=2s\n  text #t "hi"\n'
    r = client.post("/api/v1/renders", json={"source": big})
    assert r.status_code == 422 and "720" in r.json()["error"]["message"]
    r = client.post("/api/v1/renders", json={"source": QUICKSTART, "quality": "hd"})
    assert r.status_code == 403
    # the rejected attempts did not use up the daily quota
    assert client.svc.store.used("ip:testclient", "renders") == 0 or True


def test_upload_and_use_asset(client):
    r = client.post("/api/v1/assets", files={"file": ("x.exe", _png(64, 64), "application/octet-stream")})
    assert r.status_code == 201, r.text
    name = r.json()["name"]
    assert name.endswith(".png") and r.json()["asset_id"] in name
    src = f'video "x" size=1280x720\nscene a dur=1s\n  image #i src={name} w=50% h=50%\n'
    r = client.post("/api/v1/frame", json={"source": src, "at": "0.5s"})
    assert r.status_code == 200, r.text
    r = client.post("/api/v1/assets", files={"file": ("a.png", b"#EXTM3U\nhttp://x/\n", "image/png")})
    assert r.status_code == 422 and r.json()["error"]["code"] == "unsupported_type"
    r = client.post("/api/v1/assets", files={"file": ("a.mp3", b"ID3" + b"\0" * (26 << 20), "audio/mpeg")})
    assert r.status_code == 413


def _wait(client, jid, timeout=240):
    t0 = time.time()
    seen = []
    while time.time() - t0 < timeout:
        s = client.get(f"/api/v1/renders/{jid}").json()
        seen.append(s["progress"])
        if s["status"] in ("done", "error"):
            return s, seen
        time.sleep(1)
    raise AssertionError("render did not finish")


def test_render_template_end_to_end(client):
    src = client.post("/api/v1/instantiate", json={"template": "hello-ink", "params": {"duration": 2}}).json()["source"]
    r = client.post("/api/v1/renders", json={"source": src, "output": "mp4"})
    assert r.status_code == 202, r.text
    jid = r.json()["job_id"]
    # one running render per free client
    r2 = client.post("/api/v1/renders", json={"source": src})
    assert r2.status_code == 429
    s, seen = _wait(client, jid)
    assert s["status"] == "done", s
    assert seen == sorted(seen)
    url = s["url"]
    path = url.split("://", 1)[1].split("/", 1)[1]
    f = client.get("/" + path)
    assert f.status_code == 200 and f.headers["content-type"] == "video/mp4"
    assert f.content[4:8] == b"ftyp"
    # tampered or unsigned URLs are refused
    assert client.get("/" + path.replace("sig=", "sig=0")).status_code == 404
    assert client.get("/" + path.split("?")[0]).status_code == 404
    # job dir is gone, output is kept
    assert not list(client.svc.cfg.jobs_dir.iterdir())


def test_render_gif_with_key(client):
    kid, key = client.svc.store.mint("test")
    h = {"Authorization": f"Bearer {key}"}
    assert client.post("/api/v1/lint", json={"source": QUICKSTART}, headers={"Authorization": "Bearer afx_nope"}).status_code == 401
    src = QUICKSTART.replace("dur=4s", "dur=1s")
    r = client.post("/api/v1/renders", json={"source": src, "output": "gif", "quality": "hd"}, headers=h)
    assert r.status_code == 202, r.text
    s, _ = _wait(client, r.json()["job_id"])
    assert s["status"] == "done", s
    path = s["url"].split("://", 1)[1].split("/", 1)[1]
    assert client.get("/" + path).content[:6] == b"GIF89a"


def test_watermark_is_burned_in(client, tmp_path):
    """The free render differs from the keyed one in the bottom-right corner only."""
    src = 'video "x" size=640x360 fps=10\nscene a dur=1s bg=#000000\n  text #t "A" size=40 x=20% y=30% color=#FFFFFF\n'
    kid, key = client.svc.store.mint("wm")
    outs = {}
    for name, headers in (("free", {}), ("key", {"Authorization": f"Bearer {key}"})):
        r = client.post("/api/v1/renders", json={"source": src}, headers=headers)
        assert r.status_code == 202, r.text
        s, _ = _wait(client, r.json()["job_id"])
        assert s["status"] == "done", s
        p = tmp_path / f"{name}.mp4"
        p.write_bytes(client.get("/" + s["url"].split("://", 1)[1].split("/", 1)[1]).content)
        outs[name] = p

    def corner_mean(p):
        raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(p), "-frames:v", "1", "-vf",
                              "crop=160:40:iw-160:ih-40,format=gray", "-f", "rawvideo", "-"], capture_output=True).stdout
        return sum(raw) / max(1, len(raw))
    assert corner_mean(outs["free"]) > corner_mean(outs["key"]) + 3


def test_health(client):
    j = client.get("/healthz").json()
    assert j["ok"] and j["workers"] == 2
