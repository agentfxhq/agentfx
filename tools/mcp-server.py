#!/usr/bin/env python3
"""Compatibility entry point: the MCP server lives in mcp/server.py (tools named agentfx_*)."""
import os
import runpy

here = os.path.dirname(os.path.abspath(__file__))
runpy.run_path(os.path.join(os.path.dirname(here), "mcp", "server.py"), run_name="__main__")
