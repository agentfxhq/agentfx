import os
import shutil
import sys
from pathlib import Path

import pytest

SERVICE = Path(__file__).resolve().parents[1]
REPO = SERVICE.parent
sys.path.insert(0, str(SERVICE))


def find_bin():
    for c in (os.environ.get("AFX_BIN"), shutil.which("agentfx"), str(REPO / "target/release/agentfx"),
              # a git worktree shares the main checkout's build
              str(REPO.parents[2] / "target/release/agentfx") if ".claude" in REPO.parts else None):
        if c and os.path.isfile(c) and os.access(c, os.X_OK):
            return c
    return None


BIN = find_bin()
needs_cli = pytest.mark.skipif(BIN is None, reason="agentfx binary not found (set AFX_BIN)")


@pytest.fixture
def cfg(tmp_path, monkeypatch):
    monkeypatch.setenv("AFX_DATA_DIR", str(tmp_path / "data"))
    if BIN:
        monkeypatch.setenv("AFX_BIN", BIN)
    monkeypatch.setenv("AFX_HOME", str(REPO))
    # the service tests use two small fixture templates next to the real renderer
    tdir = tmp_path / "templates"
    tdir.mkdir()
    (tdir / "render_template.py").symlink_to(REPO / "templates" / "render_template.py")
    for t in (Path(__file__).parent / "fixtures" / "templates").iterdir():
        (tdir / t.name).symlink_to(t)
    monkeypatch.setenv("AFX_TEMPLATES_DIR", str(tdir))
    monkeypatch.setenv("AFX_WORKERS", "2")
    for k in list(os.environ):
        if k.startswith(("AFX_FREE_", "AFX_KEY_")):
            monkeypatch.delenv(k)
    from agentfx_service import config
    return config.load()


@pytest.fixture
def client(cfg):
    from starlette.testclient import TestClient
    from agentfx_service.app import build
    app = build(cfg)
    with TestClient(app) as c:
        c.svc = app.state.svc
        yield c


QUICKSTART = (REPO / "examples" / "quickstart.reel").read_text()
