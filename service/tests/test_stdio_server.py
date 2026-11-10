"""`python3 mcp/server.py` (stdio) still works after the refactor."""
import json
import os
import subprocess
import sys

from conftest import BIN, QUICKSTART, REPO, needs_cli


@needs_cli
def test_stdio_roundtrip():
    msgs = [{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
             "params": {"name": "agentfx_lint", "arguments": {"source": QUICKSTART, "no_tts": True}}},
            {"jsonrpc": "2.0", "id": 4, "method": "tools/call",
             "params": {"name": "agentfx_frame", "arguments": {"source": QUICKSTART, "at": "1s"}}}]
    r = subprocess.run([sys.executable, str(REPO / "mcp" / "server.py")], input="\n".join(json.dumps(m) for m in msgs) + "\n",
                       capture_output=True, text=True, timeout=120, env={**os.environ, "AGENTFX_BIN": BIN})
    out = [json.loads(l) for l in r.stdout.splitlines()]
    assert [o["id"] for o in out] == [1, 2, 3, 4]
    assert any(t["name"] == "agentfx_critique" for t in out[1]["result"]["tools"])
    assert out[2]["result"]["content"][0]["text"].startswith("[]")
    assert out[3]["result"]["content"][0]["type"] == "image"
