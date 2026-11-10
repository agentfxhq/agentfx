"""Template render API: stills in several sizes (png/jpg/pdf), data params, batch zips, MCP tools."""
import io
import json
import zipfile

from conftest import needs_cli
from test_api import _wait
from test_mcp import rpc

pytestmark = needs_cli


def _get(client, url):
    return client.get("/" + url.split("://", 1)[1].split("/", 1)[1])


def test_list_has_kind_and_sizes(client):
    ts = {t["id"]: t for t in client.get("/api/v1/templates").json()}
    assert ts["hello-card"]["kind"] == "still" and "story" in ts["hello-card"]["sizes"]
    assert ts["hello-ink"]["kind"] == "video"


def test_still_template_sizes(client):
    rows = [{"label": "Espresso", "value": "2.40"}, {"label": "Flat white", "value": "3.90"}]
    r = client.post("/api/v1/renders", json={"template": "hello-card", "params": {"title": "Menu", "rows": rows},
                                             "sizes": ["instagram-post", "story"], "output": "png"})
    assert r.status_code == 202, r.text
    s, _ = _wait(client, r.json()["job_id"])
    assert s["status"] == "done", s
    assert [f["size"] for f in s["files"]] == ["instagram-post", "story"]
    dims = []
    for f in s["files"]:
        png = _get(client, f["url"]).content
        assert png[:4] == b"\x89PNG"
        dims.append((int.from_bytes(png[16:20], "big"), int.from_bytes(png[20:24], "big")))
    assert dims == [(1080, 1080), (1080, 1920)]


def test_still_pdf_and_csv_data(client):
    csv = "label,value\nMon-Fri,9-18\nSat,10-14\n"
    r = client.post("/api/v1/renders", json={"template": "hello-card", "params": {"rows": csv},
                                             "sizes": ["instagram-post"], "output": "pdf"})
    assert r.status_code == 202, r.text
    s, _ = _wait(client, r.json()["job_id"])
    assert s["status"] == "done", s
    pdf = _get(client, s["url"])
    assert pdf.headers["content-type"] == "application/pdf" and pdf.content.startswith(b"%PDF-1.4")


def test_template_request_errors(client):
    def post(**b):
        return client.post("/api/v1/renders", json={"template": "hello-card", **b})
    assert post(sizes=["billboard"]).status_code == 422
    assert post(output="mp4").status_code == 422                   # a still template
    assert post(params={"rows": "x" * 70000}).status_code in (413, 422)
    assert post(params={"rows": [{"label": str(i)} for i in range(501)]}).status_code == 422
    assert client.post("/api/v1/renders", json={"template": "../etc"}).status_code == 422
    r = post(params={"rows": [{"label": "x", "value": "y"}]}, sizes=["a4-flyer"])
    assert r.status_code == 202  # 2480x3508 = 8.7 MP is within the free still limit
    _wait(client, r.json()["job_id"])


def test_batch_zip(client):
    rows = [{"title": f"Card {i}"} for i in range(3)]
    r = client.post("/api/v1/batch", json={"template": "hello-card", "rows": rows, "sizes": ["instagram-post"],
                                           "output": "jpg"})
    assert r.status_code == 202, r.text
    s, _ = _wait(client, r.json()["job_id"])
    assert s["status"] == "done", s
    z = zipfile.ZipFile(io.BytesIO(_get(client, s["url"]).content))
    names = sorted(z.namelist())
    assert names == ["hello-card-001-instagram-post.jpg", "hello-card-002-instagram-post.jpg",
                     "hello-card-003-instagram-post.jpg"]
    assert z.read(names[0])[:3] == b"\xff\xd8\xff"
    r = client.post("/api/v1/batch", json={"template": "hello-card", "rows": [{}] * 51})
    assert r.status_code == 422


def test_mcp_template_tools(client):
    res = rpc(client, "tools/call", {"name": "agentfx_list_templates", "arguments": {}})["result"]
    ids = {t["id"] for t in json.loads(res["content"][0]["text"])}
    assert {"hello-card", "hello-ink"} <= ids
    res = rpc(client, "tools/call", {"name": "agentfx_render_template", "arguments": {
        "template": "hello-card", "params": {"title": "MCP"}, "sizes": ["instagram-post"], "out_dir": "/etc"}})["result"]
    assert not res.get("isError"), res
    info = res["structuredContent"]
    if info["status"] == "done":
        assert res["content"][1]["type"] == "image"
    tools = {t["name"]: t for t in rpc(client, "tools/list")["result"]["tools"]}
    assert "out_dir" not in tools["agentfx_render_template"]["inputSchema"]["properties"]


def test_raw_source_still_with_format(client):
    from conftest import QUICKSTART
    r = client.post("/api/v1/renders", json={"source": QUICKSTART, "output": "png", "format": "9:16", "at": "2s"})
    assert r.status_code == 202, r.text
    s, _ = _wait(client, r.json()["job_id"])
    png = _get(client, s["url"]).content
    assert (int.from_bytes(png[16:20], "big"), int.from_bytes(png[20:24], "big")) == (720, 1280)


def test_storage_cap(client, monkeypatch):
    # the startup cleanup recounts stored_bytes in a thread, so lower the cap instead of faking usage
    monkeypatch.setattr(client.svc.cfg, "storage_max_gb", -1.0)
    r = client.post("/api/v1/renders", json={"template": "hello-card"})
    assert r.status_code == 503 and r.json()["error"]["code"] == "busy"
