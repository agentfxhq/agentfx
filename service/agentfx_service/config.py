"""Service configuration from the environment (prefix AFX_). See service/README.md."""
from __future__ import annotations

import dataclasses
import os
import shutil
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]


def _env(name, default, cast=str):
    v = os.environ.get(name)
    if v is None or v == "":
        return default
    if cast is bool:
        return v.strip().lower() in ("1", "true", "yes", "on")
    return cast(v)


@dataclass
class Tier:
    """Limits of one tier. `free` applies to anonymous requests (keyed by IP), `key` to API keys
    (a key row can override single fields, see keys.py)."""
    name: str
    source_kb: int
    duration_s: float
    max_short_side: int          # output resolution cap (short side, px)
    qualities: tuple             # allowed render qualities
    renders_per_day: int
    previews_per_min: int        # frame/sheet/storyboard (and lint/timing/parse) per minute
    concurrent_renders: int      # queued + running renders per client
    retention_h: float           # outputs and uploads are deleted after this
    watermark: bool
    render_wall_s: float         # wall clock cap per render job
    uploads_per_day: int
    upload_mb_per_day: int
    outputs_per_day: int         # files produced per day (template sizes x batch rows count each)
    max_outputs: int             # files per job (sizes x rows)
    video_outputs: int           # video files per job
    batch_rows: int              # rows per /batch job
    still_max_mp: float          # megapixels per still (each side is capped at 8000 px anyway)

    def with_overrides(self, o: dict | None) -> "Tier":
        if not o:
            return self
        fields = {f.name for f in dataclasses.fields(self)}
        clean = {k: (tuple(v) if k == "qualities" else v) for k, v in o.items() if k in fields and k != "name"}
        return dataclasses.replace(self, **clean)


def _tier(prefix: str, **d) -> Tier:
    vals = {}
    for k, v in d.items():
        env = f"AFX_{prefix.upper()}_{k.upper()}"
        if k == "qualities":
            vals[k] = tuple(x.strip() for x in _env(env, ",".join(v)).split(",") if x.strip())
        else:
            vals[k] = _env(env, v, type(v))
    return Tier(name=prefix, **vals)


@dataclass
class Config:
    data_dir: Path
    agentfx_bin: str
    agentfx_home: str
    templates_dir: Path
    public_url: str              # e.g. https://api.agentfx.dev ('' = derive from the request)
    workers: int
    render_threads: int
    queue_max: int
    # sandbox
    sandbox_uid_base: int        # 0 = no uid switch (development); slot i runs as uid base+i
    require_sandbox: bool        # refuse to start when the uid switch is not possible
    mem_mb: int                  # RLIMIT_AS per process
    file_mb: int                 # RLIMIT_FSIZE per process
    job_disk_mb: int             # total size of one job dir (watchdog)
    nofile: int
    nproc: int                   # RLIMIT_NPROC (only with the uid switch: it counts per uid)
    preview_wall_s: float        # wall clock cap for frame/sheet/lint/... jobs
    # uploads
    max_audio_mb: int
    max_image_mb: int
    max_svg_kb: int
    max_font_mb: int
    # misc
    signing_key: bytes
    trust_proxy: bool            # take the client IP from X-Forwarded-For (only behind Caddy)
    mcp_wait_s: float            # agentfx_render over MCP waits this long for short renders
    free: Tier = field(default=None)
    key: Tier = field(default=None)

    @property
    def jobs_dir(self):
        return self.data_dir / "jobs"

    @property
    def outputs_dir(self):
        return self.data_dir / "outputs"

    @property
    def assets_dir(self):
        return self.data_dir / "assets"

    @property
    def db_path(self):
        return self.data_dir / "service.sqlite"


def _find_bin():
    for c in (os.environ.get("AFX_BIN"), os.environ.get("AGENTFX_BIN"), shutil.which("agentfx"),
              str(REPO / "target" / "release" / "agentfx")):
        if c and os.path.isfile(c) and os.access(c, os.X_OK):
            return c
    return "agentfx"


def _signing_key(data_dir: Path) -> bytes:
    k = os.environ.get("AFX_SIGNING_KEY")
    if k:
        return k.encode()
    p = data_dir / "signing.key"
    if not p.exists():
        data_dir.mkdir(parents=True, exist_ok=True)
        fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(os.urandom(32).hex())
    return p.read_text().strip().encode()


def load() -> Config:
    data_dir = Path(_env("AFX_DATA_DIR", str(REPO / "service" / ".data"))).resolve()
    cores = os.cpu_count() or 2
    threads = _env("AFX_RENDER_THREADS", 2, int)
    home = _env("AFX_HOME", os.environ.get("AGENTFX_HOME") or str(REPO))
    cfg = Config(
        data_dir=data_dir,
        agentfx_bin=_find_bin(),
        agentfx_home=home,
        templates_dir=Path(_env("AFX_TEMPLATES_DIR", str(REPO / "templates"))),
        public_url=_env("AFX_PUBLIC_URL", "").rstrip("/"),
        workers=_env("AFX_WORKERS", max(1, cores // max(1, threads)), int),
        render_threads=threads,
        queue_max=_env("AFX_QUEUE_MAX", 200, int),
        sandbox_uid_base=_env("AFX_SANDBOX_UID_BASE", 0, int),
        require_sandbox=_env("AFX_REQUIRE_SANDBOX", False, bool),
        mem_mb=_env("AFX_MEM_MB", 4096, int),
        file_mb=_env("AFX_FILE_MB", 512, int),
        job_disk_mb=_env("AFX_JOB_DISK_MB", 1024, int),
        nofile=_env("AFX_NOFILE", 1024, int),
        nproc=_env("AFX_NPROC", 512, int),
        preview_wall_s=_env("AFX_PREVIEW_WALL_S", 60.0, float),
        max_audio_mb=_env("AFX_MAX_AUDIO_MB", 25, int),
        max_image_mb=_env("AFX_MAX_IMAGE_MB", 8, int),
        max_svg_kb=_env("AFX_MAX_SVG_KB", 512, int),
        max_font_mb=_env("AFX_MAX_FONT_MB", 8, int),
        signing_key=_signing_key(data_dir),
        trust_proxy=_env("AFX_TRUST_PROXY", False, bool),
        mcp_wait_s=_env("AFX_MCP_WAIT_S", 50.0, float),
    )
    cfg.free = _tier("free", source_kb=64, duration_s=20.0, max_short_side=720, qualities=("preview",),
                     renders_per_day=10, previews_per_min=60, concurrent_renders=1, retention_h=24.0,
                     watermark=True, render_wall_s=300.0, uploads_per_day=30, upload_mb_per_day=100,
                     outputs_per_day=200, max_outputs=100, video_outputs=3, batch_rows=50, still_max_mp=12.0)
    cfg.key = _tier("key", source_kb=256, duration_s=180.0, max_short_side=1080, qualities=("preview", "hd"),
                    renders_per_day=500, previews_per_min=600, concurrent_renders=3, retention_h=168.0,
                    watermark=False, render_wall_s=1800.0, uploads_per_day=500, upload_mb_per_day=2000,
                    outputs_per_day=20000, max_outputs=2000, video_outputs=50, batch_rows=1000, still_max_mp=64.0)
    return cfg
