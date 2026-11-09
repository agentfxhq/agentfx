"""The service core: job queue + worker pool, the CLI operations (parse, lint, timing, frame,
sheet, storyboard, render) on validated sources in sandboxed job dirs, render jobs with
progress, watermark, output store with signed expiring URLs, retention cleanup.

One scheduler, N worker slots (N = cores / threads per render). Slot i runs its processes as
uid AFX_SANDBOX_UID_BASE + i. Preview work (frame/sheet/storyboard/lint/timing/parse and the
checks before a render) always goes first, and the last slot never takes renders, so
previews stay fast while renders run.
"""
from __future__ import annotations

import asyncio
import collections
import glob
import hashlib
import hmac
import json
import logging
import os
import secrets
import shutil
import time
import zipfile
from dataclasses import asdict, dataclass, field
from pathlib import Path

from . import stdio as stdio_mod
from . import validate
from .assets import AssetStore, candidate_names
from .keys import Store
from .sandbox import Sandbox, SandboxError
from .validate import SourceError

log = logging.getLogger("agentfx.core")
JOB_ID = __import__("re").compile(r"^r[0-9a-f]{24}$")
VIDEO_OUTPUTS = ("mp4", "gif", "webp")
STILL_OUTPUTS = ("png", "jpg", "pdf")


class ServiceError(Exception):
    def __init__(self, message, code="error", status=400, extra=None):
        super().__init__(message)
        self.code, self.status, self.extra = code, status, extra or {}


def _err_from(e: Exception) -> ServiceError:
    if isinstance(e, ServiceError):
        return e
    if isinstance(e, SourceError):
        return ServiceError(str(e), e.code, 422)
    if isinstance(e, SandboxError):
        return ServiceError(str(e), e.code, 500)
    return ServiceError(f"internal error: {e}", "internal", 500)


@dataclass
class Unit:
    """One output file of a job: a source rendered as a video or a still."""
    source: str
    label: str | None = None      # size preset name
    row: int | None = None        # batch row index
    at: str = "0s"                # stills: frame time


@dataclass
class RenderJob:
    id: str
    client: str
    tier: str
    kind: str                     # video | still
    output: str                   # mp4|gif|webp | png|jpg|pdf
    quality: str
    format: str | None
    watermark: bool
    retention_h: float
    wall_s: float
    prefix: str = "agentfx"
    zip: bool = False             # batch: one zip of all files
    created: float = field(default_factory=time.time)
    status: str = "queued"
    progress: float = 0.0
    started: float | None = None
    finished: float | None = None
    error: str | None = None
    files: list = field(default_factory=list)     # [{"name", "size"?, "row"?}]
    failures: list = field(default_factory=list)  # batch rows that failed: [{"row", "size", "error"}]
    file: str | None = None       # the main file (single output or the zip)
    expires: float | None = None
    units: list = field(default_factory=list)     # not persisted

    def public(self, svc=None, base=None) -> dict:
        d = {"job_id": self.id, "status": self.status, "progress": round(self.progress, 3), "kind": self.kind,
             "output": "zip" if self.zip else self.output}
        if self.status == "done" and svc is not None:
            d["url"] = svc.file_url(base, self, self.file)
            d["expires_at"] = int(self.expires or 0)
            if len(self.files) > 1 or self.zip or any(f.get("size") for f in self.files):
                d["files"] = [{**f, "url": svc.file_url(base, self, f["name"])} for f in self.files]
            if self.failures:
                d["failures"] = self.failures
        if self.error:
            d["error"] = self.error
        if self.status == "queued":
            d["queued_s"] = round(time.time() - self.created, 1)
        return d


class Scheduler:
    def __init__(self, workers: int, queue_max: int):
        self.n = max(1, workers)
        self.render_slots = max(1, self.n - 1) if self.n > 1 else 1
        self.queue_max = queue_max
        self.preview = collections.deque()
        self.render = collections.deque()
        self.cond = asyncio.Condition()
        self.renders_running = 0
        self.tasks = []

    def start(self):
        for slot in range(self.n):
            self.tasks.append(asyncio.create_task(self._worker(slot), name=f"worker-{slot}"))

    async def stop(self):
        for t in self.tasks:
            t.cancel()
        await asyncio.gather(*self.tasks, return_exceptions=True)

    def depth(self):
        return len(self.preview) + len(self.render)

    async def run_preview(self, fn):
        """Run fn(slot) (blocking code) on a worker slot, preview priority; returns its result."""
        if self.depth() >= self.queue_max:
            raise ServiceError("the render queue is full, try again shortly", "busy", 503)
        fut = asyncio.get_running_loop().create_future()
        async with self.cond:
            self.preview.append((fn, fut))
            self.cond.notify_all()
        return await fut

    async def add_render(self, coro_fn):
        if self.depth() >= self.queue_max:
            raise ServiceError("the render queue is full, try again shortly", "busy", 503)
        async with self.cond:
            self.render.append(coro_fn)
            self.cond.notify_all()

    def _can_render(self, slot):
        # the last slot is reserved for previews (when there is more than one)
        return (self.n == 1 or slot < self.n - 1) and self.renders_running < self.render_slots

    async def _worker(self, slot):
        while True:
            async with self.cond:
                await self.cond.wait_for(lambda: self.preview or (self.render and self._can_render(slot)))
                if self.preview:
                    item, kind = self.preview.popleft(), "preview"
                else:
                    item, kind = self.render.popleft(), "render"
                    self.renders_running += 1
            try:
                if kind == "preview":
                    fn, fut = item
                    if fut.cancelled():
                        continue
                    try:
                        res = await asyncio.to_thread(fn, slot)
                        if not fut.done():
                            fut.set_result(res)
                    except Exception as e:  # noqa: BLE001 - handed to the waiter
                        if not fut.done():
                            fut.set_exception(e)
                else:
                    try:
                        await item(slot)
                    except Exception:  # noqa: BLE001
                        log.exception("render job failed")
            finally:
                if kind == "render":
                    async with self.cond:
                        self.renders_running -= 1
                        self.cond.notify_all()


class Service:
    def __init__(self, cfg):
        self.cfg = cfg
        cfg.data_dir.mkdir(parents=True, exist_ok=True)
        cfg.outputs_dir.mkdir(parents=True, exist_ok=True)
        self.sandbox = Sandbox(cfg)
        self.assets = AssetStore(cfg)
        self.store = Store(cfg.db_path)
        self.sched = Scheduler(cfg.workers, cfg.queue_max)
        self.jobs: dict[str, RenderJob] = {}
        self._cleanup_task = None
        self.watermark_font = self._find_font()
        self.stdio = stdio_mod.load(cfg.agentfx_home)
        from .tpl import Templates
        self.templates = Templates(cfg.templates_dir)

    def _find_font(self):
        for c in (os.path.join(self.cfg.agentfx_home, "assets", "fonts", "Inter.ttf"),
                  "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"):
            if os.path.isfile(c):
                return c
        return None

    async def start(self):
        for stale in self.cfg.jobs_dir.glob("*"):
            shutil.rmtree(stale, ignore_errors=True)
        self.sched.start()
        self._cleanup_task = asyncio.create_task(self._cleanup_loop())

    async def stop(self):
        if self._cleanup_task:
            self._cleanup_task.cancel()
        await self.sched.stop()

    # ------------------------------------------------------------ helpers

    def bin(self):
        return self.cfg.agentfx_bin

    def run_cli(self, slot, job_dir: Path, args: list, wall_s=None, env=None):
        return self.sandbox.run(slot, [self.bin(), *args], job_dir, wall_s or self.cfg.preview_wall_s, env)

    def check_source_size(self, source, tier):
        if not isinstance(source, str) or not source.strip():
            raise ServiceError("`source` (Reel Script text) is required", "invalid_request", 400)
        if len(source.encode("utf-8")) > tier.source_kb * 1024:
            raise ServiceError(f"source is larger than {tier.source_kb} KB", "limit_exceeded", 413)

    def prepare(self, slot, source: str, strict=True):
        """Blocking (worker thread): job dir with src.reel, parsed + validated, assets copied.
        Returns (job_dir, facts, parse_ok, parse_stderr). strict=False lets a source that does
        not parse through (lint reports the syntax errors itself)."""
        job_dir = self.sandbox.new_job_dir()
        try:
            (job_dir / "src.reel").write_text(source, encoding="utf-8")
            r = self.run_cli(slot, job_dir, ["parse", "src.reel", "--to", "json"])
            if r.killed:
                raise ServiceError("parsing took too long", "timeout", 422)
            try:
                doc = json.loads(r.stdout)
            except ValueError:
                doc = None
            if doc is None:
                raise ServiceError("the source does not parse: " + _first_error(r.stderr), "parse_error", 422,
                                   {"diagnostics": _json_or(r.stderr, [])})
            if r.code != 0 and strict:
                raise ServiceError("the source does not parse: " + _first_error(r.stderr), "parse_error", 422,
                                   {"diagnostics": _json_or(r.stderr, [])})
            known = self.assets.known(candidate_names(source))
            facts = validate.check_document(doc, known)
            self.assets.copy_into(facts["used"], job_dir)
            return job_dir, facts, r.code == 0, r.stderr
        except BaseException:
            self.sandbox.remove(job_dir)
            raise

    async def preview_op(self, fn):
        try:
            return await self.sched.run_preview(fn)
        except Exception as e:  # noqa: BLE001
            raise _err_from(e) from e

    # ------------------------------------------------------------ preview operations

    async def parse(self, tier, source=None, doc_json=None, to="json"):
        if to not in ("json", "text"):
            raise ServiceError("`to` must be json or text", "invalid_request")
        if doc_json is not None:
            text = json.dumps(doc_json)
            if len(text) > tier.source_kb * 1024 * 4:
                raise ServiceError("json is too large", "limit_exceeded", 413)

            def fn(slot):
                job_dir = self.sandbox.new_job_dir()
                try:
                    (job_dir / "src.json").write_text(text)
                    r = self.run_cli(slot, job_dir, ["parse", "src.json", "--to", to])
                    return r
                finally:
                    self.sandbox.remove(job_dir)
        else:
            self.check_source_size(source, tier)

            def fn(slot):
                job_dir = self.sandbox.new_job_dir()
                try:
                    (job_dir / "src.reel").write_text(source, encoding="utf-8")
                    return self.run_cli(slot, job_dir, ["parse", "src.reel", "--to", to])
                finally:
                    self.sandbox.remove(job_dir)
        r = await self.preview_op(fn)
        if r.code != 0 and not r.stdout.strip():
            raise ServiceError("parse failed: " + _first_error(r.stderr), "parse_error", 422,
                               {"diagnostics": _json_or(r.stderr, [])})
        if to == "json":
            out = {"json": _json_or(r.stdout, None)}
        else:
            out = {"source": r.stdout}
        if r.code != 0:
            out["diagnostics"] = _json_or(r.stderr, [])
        return out

    async def lint(self, tier, source, fmt=None, measure=True):
        self.check_source_size(source, tier)
        fmt = validate.check_format(fmt)

        def fn(slot):
            job_dir, _facts, _ok, _ = self.prepare(slot, source, strict=False)
            try:
                args = ["lint", "src.reel", "--no-tts"]
                if measure:
                    args += ["--measure"] + (["--format", fmt] if fmt else ["--all-formats"])
                elif fmt:
                    args += ["--format", fmt]
                return self.run_cli(slot, job_dir, args)
            finally:
                self.sandbox.remove(job_dir)
        r = await self.preview_op(fn)
        diags = _json_or(r.stdout, None)
        if diags is None:
            raise ServiceError("lint failed: " + r.stderr[-2000:], "lint_failed", 422)
        summary = (r.stderr.strip().splitlines() or [""])[-1]
        return {"diagnostics": diags, "summary": summary}

    async def timing(self, tier, source, fmt=None):
        self.check_source_size(source, tier)
        validate.check_format(fmt)

        def fn(slot):
            job_dir, *_ = self.prepare(slot, source)
            try:
                return self.run_cli(slot, job_dir, ["timing", "src.reel", "--no-tts"])
            finally:
                self.sandbox.remove(job_dir)
        r = await self.preview_op(fn)
        j = _json_or(r.stdout, None)
        if j is None:
            raise ServiceError("timing failed: " + _first_error(r.stderr), "compile_error", 422,
                               {"diagnostics": _json_or(r.stderr, [])})
        return j

    def _image_job(self, slot, source, args_fn, max_width=None, max_side=2160):
        job_dir, facts, *_ = self.prepare(slot, source)
        try:
            if validate.short_side(facts) > max_side:
                raise ServiceError(f"previews are limited to a {max_side}px short side", "limit_exceeded", 422)
            r = self.run_cli(slot, job_dir, args_fn("out.png"))
            if r.killed:
                raise ServiceError(f"preview stopped ({r.killed})", "timeout" if r.killed == "timeout" else "limit_exceeded", 422)
            png = job_dir / "out.png"
            if r.code != 0 or not png.is_file() or png.stat().st_size == 0:
                raise ServiceError("render failed: " + _first_error(r.stderr), "render_error", 422,
                                   {"diagnostics": _json_or(r.stderr, []), "stderr": r.stderr[-2000:]})
            if max_width:
                w = _png_width(png)
                if w and w > max_width:
                    s = self.sandbox.run(slot, ["ffmpeg", "-v", "error", "-y", "-i", "out.png", "-vf",
                                                f"scale={max_width}:-2:flags=lanczos", "small.png"], job_dir, 30)
                    if s.code == 0 and (job_dir / "small.png").is_file():
                        png = job_dir / "small.png"
            return png.read_bytes(), r.stderr
        finally:
            self.sandbox.remove(job_dir)

    async def frame(self, tier, source, at, fmt=None, width=None):
        self.check_source_size(source, tier)
        at = validate.check_at(at)
        fmt = validate.check_format(fmt)
        width = validate.check_int(width, "width", 64, 1920, 640)

        def args(o):
            return ["frame", "src.reel", "--at", at, "-o", o, "--no-tts"] + (["--format", fmt] if fmt else [])
        return await self.preview_op(lambda slot: self._image_job(slot, source, args, width))

    async def sheet(self, tier, source, n=None, fmt=None, cols=None, events=False):
        self.check_source_size(source, tier)
        fmt = validate.check_format(fmt)
        n = validate.check_int(n, "n", 1, 36, 12)
        cols = validate.check_int(cols, "cols", 1, 8, None)

        def args(o):
            a = ["sheet", "src.reel", "-o", o, "--no-tts"] + (["--events"] if events else ["--n", str(n)])
            return a + (["--cols", str(cols)] if cols else []) + (["--format", fmt] if fmt else [])
        return await self.preview_op(lambda slot: self._image_job(slot, source, args, 2400))

    # ------------------------------------------------------------ renders

    def client_active_renders(self, client):
        return sum(1 for j in self.jobs.values() if j.client == client and j.status in ("queued", "running"))

    def template_units(self, tier, template, params, sizes, output, rows=None):
        """Instantiate a template (once per batch row) for every size -> (kind, output, units)."""
        tpls = self.templates
        try:
            manifest = tpls.manifest(template)
            kind, output, plan = self.stdio.template_plan(manifest, sizes, output)
        except ValueError as e:
            raise ServiceError(str(e), "invalid_params", 422) from None
        rows = rows if rows is not None else [None]
        n = len(rows) * len(plan)
        if n > tier.max_outputs:
            raise ServiceError(f"at most {tier.max_outputs} files per job ({len(rows)} rows x {len(plan)} sizes)",
                               "limit_exceeded", 422)
        if kind == "video" and n > tier.video_outputs:
            raise ServiceError(f"at most {tier.video_outputs} videos per job on this tier", "limit_exceeded", 422)
        units = []
        for ri, row in enumerate(rows):
            p = dict(params or {})
            if row is not None:
                if not isinstance(row, dict):
                    raise ServiceError(f"row {ri}: must be an object of params", "invalid_params", 422)
                p.update(row)
            try:
                p = self.map_template_params(manifest, p)
                src = tpls.instantiate(template, p)
            except ValueError as e:
                raise ServiceError((f"row {ri}: " if row is not None else "") + str(e), "invalid_params", 422) from None
            for label, size in plan:
                s = self.stdio.set_video_size(src, size) if size else src
                self.check_source_size(s, tier)
                units.append(Unit(s, label if size else None, ri if row is not None else None,
                                  str(manifest.get("still_at", "0s"))))
        return kind, output, units, manifest

    def map_template_params(self, manifest, params):
        """Asset ids -> asset names for image/audio params; size/row caps for data params."""
        out = dict(params)
        for p in manifest.get("params", []):
            v = out.get(p["id"])
            if v is None:
                continue
            if p.get("type") in ("audio", "image") and isinstance(v, str):
                out[p["id"]] = self.assets.name_for(v)
            elif p.get("type") == "data":
                raw = v if isinstance(v, str) else json.dumps(v)
                if len(raw.encode("utf-8")) > 64 * 1024:
                    raise ValueError(f"param {p['id']!r}: data is larger than 64 KB")
                rows = v if isinstance(v, list) else raw.strip().count("\n")
                if (len(rows) if isinstance(rows, list) else rows) > 500:
                    raise ValueError(f"param {p['id']!r}: at most 500 rows")
        return out

    async def create_render(self, client, tier, source=None, fmt=None, output=None, quality="preview",
                            template=None, params=None, sizes=None, rows=None, at=None):
        """A render job: a raw source (video, or a still with output png/jpg/pdf), a template with
        sizes (one file per size), or a batch (rows x sizes, zipped)."""
        fmt = validate.check_format(fmt)
        if quality not in ("preview", "hd"):
            raise ServiceError("quality must be preview or hd", "invalid_request")
        if quality not in tier.qualities:
            raise ServiceError(f"quality={quality} needs an API key", "limit_exceeded", 403)
        if rows is not None:
            if not isinstance(rows, list) or not rows:
                raise ServiceError("rows must be a non-empty list of param objects", "invalid_request")
            if len(rows) > tier.batch_rows:
                raise ServiceError(f"at most {tier.batch_rows} rows per batch on this tier", "limit_exceeded", 422)
        prefix = "agentfx"
        if template is not None:
            if source is not None:
                raise ServiceError("send either `source` or `template`, not both", "invalid_request")
            if params is not None and not isinstance(params, dict):
                raise ServiceError("params must be an object", "invalid_request")
            kind, output, units, _m = self.template_units(tier, template, params, sizes, output, rows)
            prefix = template
        else:
            if sizes or rows is not None:
                raise ServiceError("sizes and rows need a `template`", "invalid_request")
            self.check_source_size(source, tier)
            output = output or "mp4"
            kind = "still" if output in STILL_OUTPUTS else "video"
            units = [Unit(source, at=validate.check_at(at or "0s"))]
        if output not in (STILL_OUTPUTS if kind == "still" else VIDEO_OUTPUTS):
            raise ServiceError(f"output must be one of {', '.join(STILL_OUTPUTS + VIDEO_OUTPUTS)}", "invalid_request")
        if self.client_active_renders(client) >= tier.concurrent_renders:
            raise ServiceError(f"at most {tier.concurrent_renders} render job(s) at a time per client; wait for the "
                               "running one", "too_many_renders", 429)
        if not self.store.take(client, "renders", tier.renders_per_day):
            raise ServiceError(f"daily render limit reached ({tier.renders_per_day} jobs/day)", "rate_limited", 429)
        if not self.store.take(client, "outputs", tier.outputs_per_day, len(units)):
            self.store.give_back(client, "renders")
            raise ServiceError(f"daily output limit reached ({tier.outputs_per_day} files/day)", "rate_limited", 429)
        jid = "r" + secrets.token_hex(12)
        job = RenderJob(id=jid, client=client, tier=tier.name, kind=kind, output=output, quality=quality, format=fmt,
                        watermark=tier.watermark, retention_h=tier.retention_h, wall_s=tier.render_wall_s,
                        prefix=prefix, zip=rows is not None, units=units)
        try:
            # check the first file now (fast failure with a 4xx); the worker checks every file
            await self.preview_op(lambda slot: self.sandbox.remove(self._unit_check(slot, job, units[0], tier)[0]))
        except Exception:
            self.store.give_back(client, "renders")
            self.store.give_back(client, "outputs", len(units))
            raise
        self.jobs[jid] = job
        try:
            await self.sched.add_render(lambda slot: self._render(job, slot, tier))
        except ServiceError:
            self.jobs.pop(jid, None)
            self.store.give_back(client, "renders")
            self.store.give_back(client, "outputs", len(units))
            raise
        return job

    def _unit_check(self, slot, job, unit, tier):
        """Blocking: prepare the unit's job dir and check the tier limits. Returns
        (job_dir, facts, duration_s)."""
        job_dir, facts, *_ = self.prepare(slot, unit.source)
        try:
            w, h = facts["size"] or (1920, 1080)
            if job.kind == "still":
                if max(w, h) > 8000 or w * h > tier.still_max_mp * 1e6:
                    raise ServiceError(f"stills are limited to 8000 px per side and {tier.still_max_mp:g} MP on this "
                                       f"tier ({w}x{h})", "limit_exceeded", 422)
                validate.check_at(unit.at)
                return job_dir, facts, 0.0
            ss = validate.short_side(facts)
            if ss > tier.max_short_side or max(w, h) > 3840:
                raise ServiceError(f"this tier renders video at most {tier.max_short_side}p; the source is {ss}p "
                                   f"(write `video ... size=1280x720`)", "limit_exceeded", 422)
            if facts["fps"] > 60:
                raise ServiceError("fps is limited to 60", "limit_exceeded", 422)
            r = self.run_cli(slot, job_dir, ["timing", "src.reel", "--no-tts"])
            t = _json_or(r.stdout, None)
            if not t or "timeline" not in t:
                raise ServiceError("the source does not compile: " + _first_error(r.stderr), "compile_error", 422,
                                   {"diagnostics": _json_or(r.stderr, [])})
            errs = [d for d in t.get("diagnostics", []) if d.get("severity") == "error"]
            if errs:
                raise ServiceError("the source has errors: " + errs[0].get("message", ""), "compile_error", 422,
                                   {"diagnostics": errs})
            duration = float(t["timeline"].get("total") or 0)
            if duration > tier.duration_s:
                raise ServiceError(f"this tier renders at most {tier.duration_s:g} s; the video is {duration:.1f} s",
                                   "limit_exceeded", 422)
            return job_dir, facts, duration
        except BaseException:
            self.sandbox.remove(job_dir)
            raise

    def _post_filter(self, job, facts):
        """ffmpeg filter for the free-tier watermark and the preview downscale, or None."""
        w, h = facts["size"] or (1920, 1080)
        f = []
        if job.kind == "video" and job.quality == "preview" and min(w, h) > 720:
            f.append("scale='if(gt(iw,ih),-2,720)':'if(gt(iw,ih),720,-2)':flags=lanczos")
            w, h = (round(w * 720 / h), 720) if w > h else (720, round(h * 720 / w))
        if job.watermark:
            short = min(w, h)
            font = f"fontfile={self.watermark_font}:" if self.watermark_font else ""
            f.append(f"drawtext={font}text='agentfx.dev':fontcolor=white@0.78:fontsize={max(10, round(short * 0.032))}:"
                     f"shadowcolor=black@0.45:shadowx=1:shadowy=1:x=w-tw-{round(short * 0.03)}:y=h-th-{round(short * 0.03)}")
        return ",".join(f) or None

    def _file_name(self, job, unit):
        parts = [job.prefix, job.id[1:9]] if not job.zip else [job.prefix]
        if unit.row is not None:
            parts.append(f"{unit.row + 1:03d}")
        if unit.label:
            parts.append(unit.label)
        return "-".join(parts)

    async def _render(self, job: RenderJob, slot: int, tier):
        job.status, job.started, job.progress = "running", time.time(), 0.01
        dest = self.cfg.outputs_dir / job.id
        dest.mkdir(mode=0o700, exist_ok=True)
        n = len(job.units)
        try:
            for i, unit in enumerate(job.units):
                base_p, span = i / n, 1 / n
                try:
                    name = await self._render_unit(job, unit, slot, tier, dest, base_p, span)
                    f = {"name": name}
                    if unit.label:
                        f["size"] = unit.label
                    if unit.row is not None:
                        f["row"] = unit.row
                    job.files.append(f)
                except ServiceError as e:
                    if not job.zip:
                        raise
                    job.failures.append({"row": unit.row, "size": unit.label, "error": str(e)[:500]})
                job.progress = max(job.progress, base_p + span)
            if job.zip:
                if not job.files:
                    raise ServiceError("every row failed: " + job.failures[0]["error"], "render_error")
                zname = f"{job.prefix}-{job.id[1:9]}.zip"
                await asyncio.to_thread(self._zip, dest, zname, job)
                job.file = zname
            else:
                job.file = job.files[0]["name"]
            job.finished = time.time()
            job.expires = job.finished + job.retention_h * 3600
            job.status, job.progress = "done", 1.0
        except ServiceError as e:
            job.status, job.error, job.finished = "error", str(e), time.time()
        except Exception as e:  # noqa: BLE001
            log.exception("render %s", job.id)
            job.status, job.error, job.finished = "error", f"internal error: {e}", time.time()
        finally:
            job.units = []
            if job.status == "error":
                job.expires = time.time() + 3600
            self._save_meta(job)

    def _zip(self, dest, zname, job):
        with zipfile.ZipFile(dest / zname, "w", zipfile.ZIP_STORED) as z:
            for f in job.files:
                z.write(dest / f["name"], f["name"])
            if job.failures:
                z.writestr("failures.json", json.dumps(job.failures, indent=1))
        for f in job.files:
            (dest / f["name"]).unlink(missing_ok=True)
        job.files = [{"name": zname, "contents": [f["name"] for f in job.files]}]

    async def _render_unit(self, job, unit, slot, tier, dest, base_p, span):
        job_dir, facts, duration = await asyncio.to_thread(self._unit_check, slot, job, unit, tier)
        try:
            stem = self._file_name(job, unit)
            post = self._post_filter(job, facts)
            if job.kind == "still":
                png = "still.png"
                r = await asyncio.to_thread(self.run_cli, slot, job_dir,
                                            ["frame", "src.reel", "--at", unit.at, "-o", png, "--no-tts"],
                                            min(job.wall_s, self.cfg.preview_wall_s * 3))
                if r.killed or r.code != 0 or not (job_dir / png).is_file():
                    raise ServiceError("render failed: " + (f"stopped ({r.killed})" if r.killed else _first_error(r.stderr)),
                                       "render_error")
                if post:
                    p = await asyncio.to_thread(self.sandbox.run, slot, ["ffmpeg", "-v", "error", "-y", "-i", png, "-vf",
                                                                         post, "still_wm.png"], job_dir, 60)
                    if p.code != 0:
                        raise ServiceError("watermark failed: " + p.stderr[-300:], "render_error")
                    png = "still_wm.png"
                run = (lambda argv: self.sandbox.run(slot, argv, job_dir, 120).code)
                final = await asyncio.to_thread(self.stdio.still_convert, str(job_dir / png), job.output, run)
                name = f"{stem}.{job.output}"
                shutil.copyfile(final, dest / name)
                return name
            status_dir = job_dir / ".status"
            status_dir.mkdir(exist_ok=True)
            name = f"{stem}.{job.output}"
            render_name = "raw.mp4" if post else name
            stop = asyncio.Event()
            started = time.time()

            async def poll():
                hi = 0.9 if post else 0.98
                while not stop.is_set():
                    p = _read_progress(status_dir)
                    if p is None:  # compiling, or no status file yet: estimate
                        p = min(0.3, (time.time() - started) / max(5.0, duration * 3) * 0.3)
                    job.progress = max(job.progress, base_p + span * min(hi, 0.02 + p * (hi - 0.02)))
                    try:
                        await asyncio.wait_for(stop.wait(), 1.0)
                    except asyncio.TimeoutError:
                        pass
            poller = asyncio.create_task(poll())
            try:
                deadline = time.monotonic() + job.wall_s
                args = ["render", "src.reel", "-o", render_name, "--no-tts"] + (["--format", job.format] if job.format else [])
                r = await asyncio.to_thread(self.run_cli, slot, job_dir, args, job.wall_s,
                                            {"AGENTFX_JOBS_DIR": str(status_dir)})
                if r.killed:
                    raise ServiceError(f"render stopped: {'wall clock limit' if r.killed == 'timeout' else 'disk limit'}",
                                       "limit_exceeded")
                if r.code != 0 or not (job_dir / render_name).is_file():
                    raise ServiceError("render failed: " + _first_error(r.stderr), "render_error")
                if post:
                    left = max(10.0, deadline - time.monotonic())
                    if job.output == "mp4":
                        cmd = ["ffmpeg", "-v", "error", "-y", "-i", render_name, "-vf", post, "-c:v", "libx264", "-preset",
                               "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-c:a", "copy", "-movflags", "+faststart",
                               name]
                    elif job.output == "gif":
                        cmd = ["ffmpeg", "-v", "error", "-y", "-i", render_name, "-filter_complex",
                               f"[0:v]fps=15,{post},split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer",
                               "-loop", "0", name]
                    else:
                        cmd = ["ffmpeg", "-v", "error", "-y", "-i", render_name, "-vf", f"fps=24,{post}", "-c:v", "libwebp",
                               "-quality", "75", "-loop", "0", "-an", name]
                    p = await asyncio.to_thread(self.sandbox.run, slot, cmd, job_dir, left)
                    if p.code != 0 or not (job_dir / name).is_file():
                        raise ServiceError("encoding failed: " + p.stderr[-300:], "render_error")
                shutil.copyfile(job_dir / name, dest / name)
                return name
            finally:
                stop.set()
                await poller
        finally:
            self.sandbox.remove(job_dir)

    def _save_meta(self, job):
        d = self.cfg.outputs_dir / job.id
        d.mkdir(mode=0o700, exist_ok=True)
        meta = asdict(job)
        meta.pop("units", None)
        (d / "job.json").write_text(json.dumps(meta))

    def get_job(self, jid) -> RenderJob | None:
        if not JOB_ID.match(jid or ""):
            return None
        j = self.jobs.get(jid)
        if j:
            return j
        try:
            d = json.loads((self.cfg.outputs_dir / jid / "job.json").read_text())
            return RenderJob(**d)
        except (OSError, ValueError, TypeError):
            return None

    async def wait_job(self, jid, timeout):
        t0 = time.monotonic()
        while time.monotonic() - t0 < timeout:
            j = self.get_job(jid)
            if not j or j.status in ("done", "error"):
                return j
            await asyncio.sleep(0.5)
        return self.get_job(jid)

    # ------------------------------------------------------------ signed URLs

    def sign(self, jid, name, exp):
        msg = f"{jid}/{name}/{int(exp)}".encode()
        return hmac.new(self.cfg.signing_key, msg, hashlib.sha256).hexdigest()[:32]

    def file_url(self, base, job: RenderJob, name=None):
        name = name or job.file
        if job.status != "done" or not name:
            return None
        exp = int(job.expires)
        return f"{base}/api/v1/files/{job.id}/{name}?exp={exp}&sig={self.sign(job.id, name, exp)}"

    def file_path(self, jid, name, exp, sig) -> Path | None:
        try:
            exp_i = int(exp)
        except (TypeError, ValueError):
            return None
        if exp_i < time.time() or not hmac.compare_digest(self.sign(jid, name, exp_i), str(sig or "")):
            return None
        job = self.get_job(jid)
        if not job or job.status != "done" or name not in {f["name"] for f in job.files}:
            return None
        p = self.cfg.outputs_dir / jid / name
        return p if p.is_file() else None

    # ------------------------------------------------------------ retention

    async def _cleanup_loop(self):
        while True:
            try:
                await asyncio.to_thread(self.cleanup)
            except Exception:  # noqa: BLE001
                log.exception("cleanup")
            await asyncio.sleep(600)

    def cleanup(self):
        now = time.time()
        for d in self.cfg.outputs_dir.iterdir():
            j = self.get_job(d.name)
            if j is None or (j.expires and j.expires < now and j.status in ("done", "error")):
                shutil.rmtree(d, ignore_errors=True)
                self.jobs.pop(d.name, None)
        for jid, j in list(self.jobs.items()):
            if j.status in ("done", "error") and j.expires and j.expires < now:
                self.jobs.pop(jid, None)
        self.assets.cleanup()


# ------------------------------------------------------------------ utils

def _json_or(text, default):
    try:
        return json.loads(text)
    except (ValueError, TypeError):
        # stderr can carry a JSON array followed by a summary line
        t = (text or "").strip()
        end = t.rfind("]")
        if t.startswith("[") and end > 0:
            try:
                return json.loads(t[:end + 1])
            except ValueError:
                pass
        return default


def _first_error(stderr: str) -> str:
    d = _json_or(stderr, None)
    if isinstance(d, list):
        for x in d:
            if isinstance(x, dict) and x.get("severity") == "error":
                return f"{x.get('code')}: {x.get('message')} (line {x.get('line')})"
    lines = [l for l in (stderr or "").strip().splitlines() if l.strip()]
    return lines[-1][:500] if lines else "no output"


def _read_progress(status_dir: Path):
    best = None
    for f in glob.glob(str(status_dir / "*.json")):
        try:
            with open(f) as fh:
                v = json.load(fh)
            done, total = v.get("frames_done"), v.get("frames_total")
            if total:
                p = min(1.0, done / total)
                best = p if best is None else max(best, p)
        except (OSError, ValueError, TypeError):
            continue
    return best


def _png_width(p: Path):
    with open(p, "rb") as f:
        h = f.read(24)
    if len(h) == 24 and h[12:16] == b"IHDR":
        return int.from_bytes(h[16:20], "big")
    return None
