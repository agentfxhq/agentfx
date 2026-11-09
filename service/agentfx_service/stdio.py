"""Load mcp/server.py (the stdio MCP server): its tool table, handlers and the stdlib helpers
(set_video_size, template_plan, still_convert, jpeg_to_pdf) are shared with the service."""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

_cache = {}


def load(home=None):
    """mcp/server.py from AGENTFX_HOME (the image installs it there), else from this repo."""
    cands = ([Path(home) / "mcp" / "server.py"] if home else []) + [Path(__file__).resolve().parents[2] / "mcp" / "server.py"]
    p = next((c for c in cands if c.is_file()), cands[-1])
    if p in _cache:
        return _cache[p]
    spec = importlib.util.spec_from_file_location("agentfx_mcp_stdio", p)
    m = importlib.util.module_from_spec(spec)
    sys.modules["agentfx_mcp_stdio"] = m
    spec.loader.exec_module(m)
    if home:
        m.HOME = str(home)  # the handlers read docs/design/templates from AGENTFX_HOME
    _cache[p] = m
    return m
