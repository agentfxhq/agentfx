"""Remote MCP over Streamable HTTP: raw JSON-RPC round trip, and the official MCP client."""
import asyncio
import base64
import socket
import threading
import time

import pytest

from conftest import QUICKSTART, needs_cli

H = {"Accept": "application/json, text/event-stream", "MCP-Protocol-Version": "2025-06-18"}


def rpc(client, method, params=None, mid=1, headers=H):
    r = client.post("/mcp", json={"jsonrpc": "2.0", "id": mid, "method": method, "params": params or {}}, headers=headers)
    assert r.status_code == 200, r.text
    return r.json()


def test_initialize_and_list(client):
    init = rpc(client, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                      "clientInfo": {"name": "t", "version": "1"}}, headers={"Accept": H["Accept"]})
    assert init["result"]["protocolVersion"] == "2025-06-18"
    assert "tools" in init["result"]["capabilities"]
    r = client.post("/mcp", json={"jsonrpc": "2.0", "method": "notifications/initialized"}, headers=H)
    assert r.status_code == 202
    tools = rpc(client, "tools/list")["result"]["tools"]
    names = {t["name"] for t in tools}
    import agentfx_service.mcp_http as m
    stdio = m.load_stdio_server()
    assert names == {t["name"] for t in stdio.TOOLS} - {"agentfx_critique"}
    for t in tools:
        props = t["inputSchema"].get("properties", {})
        assert not {"file", "assets_dir", "out"} & set(props), t["name"]
    render = next(t for t in tools if t["name"] == "agentfx_render")
    assert render["inputSchema"]["required"] == ["source"]


def test_protocol_errors(client):
    assert client.get("/mcp").status_code == 405
    r = client.post("/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "ping"}, headers={"MCP-Protocol-Version": "1999-01-01"})
    assert r.status_code == 400
    assert rpc(client, "nope/nope")["error"]["code"] == -32601
    r = client.post("/mcp", content=b"{not json", headers=H)
    assert r.status_code == 400 and r.json()["error"]["code"] == -32700
    assert rpc(client, "tools/call", {"name": "agentfx_critique", "arguments": {"file": "/etc/passwd"}})["error"]["code"] == -32602
    assert client.post("/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "ping"},
                       headers={**H, "Authorization": "Bearer afx_bad"}).status_code == 401


@needs_cli
def test_lint_and_frame_calls(client):
    res = rpc(client, "tools/call", {"name": "agentfx_lint", "arguments": {"source": QUICKSTART}})["result"]
    assert not res.get("isError") and res["content"][0]["text"].startswith("[]")
    res = rpc(client, "tools/call", {"name": "agentfx_frame", "arguments": {"source": QUICKSTART, "at": "2s"}})["result"]
    img = res["content"][0]
    assert img["type"] == "image" and base64.b64decode(img["data"])[:4] == b"\x89PNG"
    # the stdio `file` argument is ignored: the inline source is required
    res = rpc(client, "tools/call", {"name": "agentfx_lint", "arguments": {"file": "/etc/passwd"}})["result"]
    assert res["isError"]
    res = rpc(client, "tools/call", {"name": "agentfx_frame", "arguments": {
        "source": 'video "x" size=1280x720\nscene a dur=1s\n  image #i src=/etc/passwd\n', "at": "0.5s"}})["result"]
    assert res["isError"] and "not allowed" in res["content"][0]["text"]
    res = rpc(client, "tools/call", {"name": "agentfx_spec", "arguments": {}})["result"]
    assert "Reel Script" in res["content"][0]["text"]
    res = rpc(client, "tools/call", {"name": "agentfx_lens", "arguments": {"name": "../../etc"}})["result"]
    assert res["isError"]


@needs_cli
def test_render_call_returns_job_url(client):
    src = QUICKSTART.replace("dur=4s", "dur=1s")
    res = rpc(client, "tools/call", {"name": "agentfx_render", "arguments": {"source": src}})["result"]
    assert not res.get("isError"), res
    info = res["structuredContent"]
    assert info["status_url"].endswith(info["job_id"])
    assert info["status"] in ("done", "queued", "running")
    if info["status"] == "done":
        assert "/api/v1/files/" in info["url"]


def _free_port():
    for p in range(8781, 8799):
        with socket.socket() as s:
            try:
                s.bind(("127.0.0.1", p))
                return p
            except OSError:
                continue
    pytest.skip("no free port in 8781-8798")


@needs_cli
def test_official_mcp_client(cfg):
    pytest.importorskip("mcp")
    import uvicorn
    from mcp.client.session import ClientSession
    from mcp.client.streamable_http import streamable_http_client
    from agentfx_service.app import build

    port = _free_port()
    server = uvicorn.Server(uvicorn.Config(build(cfg), host="127.0.0.1", port=port, log_level="warning"))
    t = threading.Thread(target=server.run, daemon=True)
    t.start()
    for _ in range(100):
        if server.started:
            break
        time.sleep(0.1)

    async def go():
        async with streamable_http_client(f"http://127.0.0.1:{port}/mcp") as streams:
            read, write = streams[0], streams[1]
            async with ClientSession(read, write) as s:
                await s.initialize()
                tools = await s.list_tools()
                assert "agentfx_lint" in {t.name for t in tools.tools}
                r = await s.call_tool("agentfx_lint", {"source": QUICKSTART})
                assert not r.is_error if hasattr(r, "is_error") else not r.isError
                return r
    try:
        asyncio.run(go())
    finally:
        server.should_exit = True
        t.join(10)
