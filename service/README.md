# AgentFX render service (P1)

Hosted render service for AgentFX: the HTTP API of [docs/PLATFORM.md](../docs/PLATFORM.md)
under `/api/v1`, and a remote MCP server (Streamable HTTP) at `/mcp` with the same tools as
the stdio server `mcp/server.py`. Python 3.11+, Starlette + uvicorn, an in-process job queue,
the `agentfx` CLI as a sandboxed subprocess. No Redis, no database server (SQLite for keys and
daily usage).

## Run

Production (Docker, Caddy in front, TLS for the domain):

```sh
cd service
AFX_DOMAIN=api.agentfx.dev AFX_PUBLIC_URL=https://api.agentfx.dev docker compose up -d --build
docker compose exec agentfx /opt/agentfx-service/venv/bin/python -m agentfx_service.keys mint --name "acme"
```

The image is `FROM ghcr.io/agentfxhq/agentfx:latest` (build arg `BASE`, e.g. an image built
from the repo `Dockerfile`), plus a venv with `requirements.txt`, the service, `mcp/`,
`docs/` and `templates/`. Build it from the repository root:
`docker build -f service/Dockerfile -t agentfx-service:dev .`

Development (no Docker, renders run as your user, no network isolation):

```sh
python3 -m venv .venv && .venv/bin/pip install -r service/requirements-dev.txt
cd service && AFX_BIN=../target/release/agentfx ../.venv/bin/uvicorn agentfx_service.app:app --port 8780
../.venv/bin/python -m pytest -q          # tests (need the agentfx binary; set AFX_BIN)
```

```sh
curl -s localhost:8780/api/v1/lint -d "{\"source\": $(jq -Rs . < ../examples/quickstart.reel)}"
curl -s localhost:8780/api/v1/renders -d '{"template": "hello-card", "sizes": ["instagram-post", "story"]}'
curl -s localhost:8780/api/v1/renders/<job_id>            # poll; done -> url / files[].url
claude mcp add --transport http agentfx https://api.agentfx.dev/mcp [--header "Authorization: Bearer afx_..."]
```

## Configuration (environment)

| Variable | Default | |
|---|---|---|
| `AFX_DATA_DIR` | `service/.data` (image: `/var/lib/agentfx`) | jobs/, outputs/, assets/, service.sqlite, signing.key |
| `AFX_BIN`, `AFX_HOME` | `agentfx` on PATH, repo | CLI and its data dir (`AGENTFX_HOME`) |
| `AFX_TEMPLATES_DIR` | `templates/` | |
| `AFX_PUBLIC_URL` | from the request | base of returned URLs |
| `AFX_WORKERS` / `AFX_RENDER_THREADS` | cores / 2, 2 | worker slots; threads per render (`RAYON_NUM_THREADS`) |
| `AFX_QUEUE_MAX` | 200 | 503 `busy` above |
| `AFX_SANDBOX_UID_BASE` | 0 (off; image: 20000) | slot i runs as uid base+i (needs root + CAP_SETUID) |
| `AFX_REQUIRE_SANDBOX` | 0 (image: 1) | refuse to start without the uid switch |
| `AFX_REQUIRE_NETWORK_ISOLATION` | 1 | entrypoint refuses to start without the iptables rule |
| `AFX_MEM_MB`, `AFX_FILE_MB`, `AFX_JOB_DISK_MB` | 4096, 512, 1024 | RLIMIT_AS, RLIMIT_FSIZE per process; job dir size |
| `AFX_NOFILE`, `AFX_NPROC`, `AFX_PREVIEW_WALL_S` | 1024, 512, 60 | |
| `AFX_MAX_AUDIO_MB` / `_IMAGE_MB` / `_SVG_KB` / `_FONT_MB` | 25 / 8 / 512 / 8 | upload caps |
| `AFX_SIGNING_KEY` | random, in `signing.key` | HMAC key of file URLs |
| `AFX_TRUST_PROXY` | 0 (compose: 1) | client IP from `X-Forwarded-For` (only behind Caddy) |
| `AFX_CORS_ORIGINS` | `*` | |
| `AFX_MCP_WAIT_S` | 50 | how long MCP render tools wait |
| `AFX_STORAGE_MAX_GB`, `AFX_JOB_OUTPUT_MB` | 50, 2048 | outputs + uploads on disk (new jobs/uploads get 503 above); files of one job |
| `AFX_FREE_<FIELD>`, `AFX_KEY_<FIELD>` | see `config.py` | tier limits, e.g. `AFX_FREE_DURATION_S=20`, `AFX_FREE_RENDERS_PER_DAY=10` |

Tier fields: `source_kb, duration_s, max_short_side, qualities, renders_per_day,
previews_per_min, concurrent_renders, retention_h, watermark, render_wall_s, uploads_per_day,
upload_mb_per_day, outputs_per_day, max_outputs, video_outputs, batch_rows, still_max_mp`.
A key can override any of them: `keys mint --name x --limits '{"duration_s": 300}'`.

## Limits (defaults)

| | free (per IP) | key |
|---|---|---|
| source | 64 KB | 256 KB |
| video | 20 s, 720p short side, preview | 180 s, 1080p, preview/hd |
| stills | 12 MP, 8000 px per side | 64 MP |
| jobs | 10/day, 1 at a time, 100 files/job, 3 videos/job, 200 files/day | 500/day, 3 at a time |
| batch | 50 rows | 1000 rows |
| frame/sheet | 60/min (other calls 240/min) | 600/min |
| uploads | 30/day, 100 MB/day | 500/day, 2 GB/day |
| retention | 24 h | 7 d |
| watermark | yes | no |

`quality=preview` scales videos above 720p down to 720p (post pass); `hd` keeps the source size.

## Security

Untrusted Reel Script runs through two layers.

1. Static checks (`validate.py`) on `agentfx parse --to json` before anything compiles: file
   references (`src=`, `shot=`, `texture=`, `lut=`, `audio=`, `hotspots=`, positional files of
   image/video/music/sfx/mesh/..., `image(..)` paints, src/href/url() in svg/html markup) must be
   names in the job's asset set; everywhere else `file:` URLs, absolute paths and `..` segments
   are rejected; markup may not contain URLs (other than xmlns), DOCTYPE/ENTITY, script, link,
   base, iframe, object, embed, meta or @import. Request values that reach the CLI argv (`at`,
   `format`, `n`, `cols`, ...) are validated, so nothing can smuggle in a flag.
2. The sandbox (`sandbox.py`, `docker/entrypoint.sh`): a throwaway job dir (0700) per request
   holding only the source and copies of the referenced assets; worker slot i runs its processes
   as uid 20000+i (no supplementary groups, umask 077); prlimit sets RLIMIT_AS 4 GB, CPU,
   FSIZE 512 MB, NOFILE, NPROC, CORE 0; a watchdog kills the process group on the wall clock or
   when the job dir passes 1 GB; the environment is rebuilt from scratch (no service secrets,
   `AGENTFX_TTS=none`, HOME/cache inside the job dir); an iptables/ip6tables owner rule rejects
   all traffic of uids 20000-20255 (installed by the entrypoint, which then drops NET_ADMIN from
   the bounding set). The service runs as root in the container with only CHOWN, DAC_OVERRIDE,
   FOWNER, SETUID, SETGID, KILL (+ NET_ADMIN and SETPCAP for the entrypoint, which removes both from
   the bounding set before uvicorn starts), no-new-privileges, a read-only
   root filesystem and private data files (umask 077; the data dir is 0711 so slot uids reach
   only their own job dir).

Outputs are served only through HMAC-signed, expiring URLs; job and asset ids are 96-bit random.

Not enforced / known gaps:
- Without Docker (development) there is no uid switch and no network isolation; renders run as
  the service user. `AFX_REQUIRE_SANDBOX=1` makes that a startup error.
- The renderer can read world-readable files of the image (fonts, the agentfx data dir, system
  files) if a file-loading property is missed by the static checks: there is no mount namespace
  or Landlock (blocked by Docker's default seccomp profile / not in the Python stdlib). Nothing
  secret is world-readable in the image.
- Memory is RLIMIT_AS per process plus the container `mem_limit`; there is no per-job cgroup.
  Container-level `pids_limit` and NPROC per uid bound fork bombs.
- Asset names are capabilities: anyone who knows a name can use the asset until it expires.
- Rate limits are per process (in memory) except the daily counters (SQLite); run one service
  process per host. IPv6 clients are bucketed per /64.
- Templates are trusted code from the repository (their sources pass the same checks anyway).

## Ops

- Health: `GET /healthz` (workers, queue depth, running renders, whether the uid sandbox is on).
- Logs: `docker compose logs -f agentfx` (uvicorn access log + job errors). Caddy logs to stdout.
- Keys: `python -m agentfx_service.keys mint|list|revoke` inside the container.
- Retention: a cleanup task deletes expired outputs and uploads every 10 minutes; stale job
  dirs are removed at startup. Jobs that were queued or running during a restart are lost
  (finished jobs survive: `outputs/<job>/job.json`).
- Capacity: one render slot per `AFX_RENDER_THREADS` cores, the last slot is reserved for
  previews (frame/sheet/lint), so previews stay fast while renders queue.
- Upgrades: rebuild the image (new CLI via `BASE`), `docker compose up -d`; the data volume keeps
  keys, usage and outputs.
