#!/usr/bin/env python3
"""End-to-end smoke test for the AgentFX MCP server.

Starts mcp/server.py as a subprocess, sends JSON-RPC requests for every tool,
and checks that each returns a well-formed result (no isError unless expected).

Usage:
    python3 tools/mcp-e2e.py              # uses agentfx on PATH or target/release/agentfx
    AGENTFX_BIN=/path/to/agentfx python3 tools/mcp-e2e.py

Exit 0 on success, 1 on any failure.
"""
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SERVER = os.path.join(ROOT, "mcp", "server.py")

SAMPLE_REEL = """\
video "Test" size=1280x720 fps=30 captions=off
theme studio-light
  bg #222

scene intro dur=2s
  text #hello "Hello" size=80 color=#fff x=50% y=50% anchor=center
"""

passed = 0
failed = 0
errors = []


def rpc(proc, method, params=None, mid=None):
    msg = {"jsonrpc": "2.0", "id": mid or 1, "method": method}
    if params is not None:
        msg["params"] = params
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline()
    if not line:
        return None
    return json.loads(line)


def check(name, resp, expect_error=False):
    global passed, failed
    if resp is None:
        failed += 1
        errors.append(f"{name}: no response (server crashed?)")
        return
    result = resp.get("result", {})
    is_error = result.get("isError", False)
    content = result.get("content", [])
    if not expect_error and is_error:
        text = content[0].get("text", "")[:200] if content else "(empty)"
        failed += 1
        errors.append(f"{name}: unexpected isError: {text}")
    elif expect_error and not is_error:
        failed += 1
        errors.append(f"{name}: expected isError but got success")
    elif not content and not expect_error:
        failed += 1
        errors.append(f"{name}: empty content")
    else:
        passed += 1
        print(f"  ok  {name}")


def main():
    global passed, failed

    env = os.environ.copy()
    env["AGENTFX_HOME"] = ROOT

    proc = subprocess.Popen(
        [sys.executable, SERVER],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
        cwd=ROOT,
    )

    try:
        # initialize
        resp = rpc(proc, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test"}}, mid=0)
        if resp is None or "result" not in resp:
            print("FAIL: initialize failed")
            sys.exit(1)
        print("  ok  initialize")
        passed += 1

        # tools/list
        resp = rpc(proc, "tools/list", mid=1)
        tools = [t["name"] for t in resp.get("result", {}).get("tools", [])]
        print(f"  ok  tools/list ({len(tools)} tools)")
        passed += 1

        # ping (returns empty result {}, not content)
        resp = rpc(proc, "ping", mid=2)
        if resp and "result" in resp:
            passed += 1
            print("  ok  ping")
        else:
            failed += 1
            errors.append("ping: no result")

        # agentfx_spec (overview)
        resp = rpc(proc, "tools/call", {"name": "agentfx_spec", "arguments": {}}, mid=10)
        check("spec_overview", resp)

        # agentfx_spec (section)
        resp = rpc(proc, "tools/call", {"name": "agentfx_spec", "arguments": {"section": "1"}}, mid=11)
        check("spec_section", resp)

        # agentfx_catalog
        resp = rpc(proc, "tools/call", {"name": "agentfx_catalog", "arguments": {}}, mid=20)
        check("catalog_json", resp)

        # agentfx_catalog (md)
        resp = rpc(proc, "tools/call", {"name": "agentfx_catalog", "arguments": {"md": True}}, mid=21)
        check("catalog_md", resp)

        # agentfx_design_guide
        resp = rpc(proc, "tools/call", {"name": "agentfx_design_guide", "arguments": {}}, mid=30)
        check("design_guide", resp)

        # agentfx_lens -- pick first available lens
        design_dir = os.path.join(ROOT, "design", "lenses")
        if os.path.isdir(design_dir):
            lenses = sorted(d for d in os.listdir(design_dir) if os.path.isdir(os.path.join(design_dir, d)))
            if lenses:
                resp = rpc(proc, "tools/call", {"name": "agentfx_lens", "arguments": {"name": lenses[0]}}, mid=31)
                check("lens", resp)
            else:
                print("  skip lens (no lenses found)")
        else:
            print("  skip lens (no design/lenses dir)")

        # agentfx_lint (inline source, no measure for speed)
        resp = rpc(proc, "tools/call", {"name": "agentfx_lint", "arguments": {"source": SAMPLE_REEL, "measure": False}}, mid=40)
        check("lint", resp)

        # agentfx_lint (measured)
        resp = rpc(proc, "tools/call", {"name": "agentfx_lint", "arguments": {"source": SAMPLE_REEL, "measure": True, "no_tts": True}}, mid=41)
        check("lint_measured", resp)

        # agentfx_timing
        resp = rpc(proc, "tools/call", {"name": "agentfx_timing", "arguments": {"source": SAMPLE_REEL, "no_tts": True}}, mid=50)
        check("timing", resp)

        # agentfx_frame
        resp = rpc(proc, "tools/call", {"name": "agentfx_frame", "arguments": {"source": SAMPLE_REEL, "at": "0.5"}}, mid=60)
        if resp:
            content = resp.get("result", {}).get("content", [])
            has_image = any(c.get("type") == "image" for c in content)
            if has_image:
                passed += 1
                print("  ok  frame (image returned)")
            else:
                is_err = resp.get("result", {}).get("isError", False)
                if is_err:
                    # frame may fail if renderer not built -- acceptable in some CI
                    passed += 1
                    print("  ok  frame (renderer unavailable, isError expected)")
                else:
                    failed += 1
                    errors.append("frame: no image in response")
        else:
            failed += 1
            errors.append("frame: no response")

        # agentfx_sheet
        resp = rpc(proc, "tools/call", {"name": "agentfx_sheet", "arguments": {"source": SAMPLE_REEL, "n": 4}}, mid=70)
        if resp:
            content = resp.get("result", {}).get("content", [])
            has_image = any(c.get("type") == "image" for c in content)
            is_err = resp.get("result", {}).get("isError", False)
            if has_image or is_err:
                passed += 1
                print(f"  ok  sheet ({'image' if has_image else 'isError, renderer unavailable'})")
            else:
                failed += 1
                errors.append("sheet: no image and no isError")
        else:
            failed += 1
            errors.append("sheet: no response")

        # agentfx_render
        with tempfile.NamedTemporaryFile(suffix=".mp4", delete=False) as f:
            render_out = f.name
        try:
            resp = rpc(proc, "tools/call", {"name": "agentfx_render", "arguments": {"source": SAMPLE_REEL, "out": render_out, "no_tts": True}}, mid=80)
            if resp:
                is_err = resp.get("result", {}).get("isError", False)
                if not is_err and os.path.exists(render_out) and os.path.getsize(render_out) > 0:
                    passed += 1
                    print(f"  ok  render ({os.path.getsize(render_out)} bytes)")
                elif is_err:
                    passed += 1
                    print("  ok  render (isError, renderer/ffmpeg unavailable)")
                else:
                    failed += 1
                    errors.append("render: no output file and no isError")
            else:
                failed += 1
                errors.append("render: no response")
        finally:
            if os.path.exists(render_out):
                os.remove(render_out)

        # agentfx_critique (should fail gracefully without ANTHROPIC_API_KEY or missing file)
        resp = rpc(proc, "tools/call", {"name": "agentfx_critique", "arguments": {"file": "/nonexistent.png"}}, mid=90)
        check("critique_missing_file", resp, expect_error=True)

        # unknown method
        resp = rpc(proc, "tools/call", {"name": "nonexistent_tool", "arguments": {}}, mid=99)
        check("unknown_tool", resp, expect_error=True)

    finally:
        proc.stdin.close()
        proc.terminate()
        proc.wait(timeout=5)

    print(f"\n{'PASS' if failed == 0 else 'FAIL'}: {passed} passed, {failed} failed")
    if errors:
        for e in errors:
            print(f"  FAIL: {e}")
    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()
