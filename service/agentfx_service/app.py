"""HTTP API (docs/PLATFORM.md) + remote MCP at /mcp. Run: uvicorn agentfx_service.app:app"""
from __future__ import annotations

import contextlib
import json
import logging
import os

from starlette.applications import Starlette
from starlette.middleware import Middleware
from starlette.middleware.cors import CORSMiddleware
from starlette.requests import Request
from starlette.responses import FileResponse, JSONResponse, Response
from starlette.routing import Route

from . import config
from .core import Service, ServiceError
from .limits import Window, client_ip
from .mcp_http import RemoteMCP
from .tpl import Templates
from .validate import SourceError

log = logging.getLogger("agentfx.app")
VERSION = "0.1.1"


def error(code, message, status, **extra):
    return JSONResponse({"error": {"code": code, "message": message, **extra}}, status_code=status)


class BodyLimit:
    """Reject bodies over a size limit while they stream (Content-Length is checked first)."""

    def __init__(self, app, limit_for):
        self.app, self.limit_for = app, limit_for

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http":
            return await self.app(scope, receive, send)
        limit = self.limit_for(scope["path"])
        cl = dict(scope.get("headers") or []).get(b"content-length")
        if cl is not None:
            try:
                if int(cl) > limit:
                    return await error("too_large", f"request body over {limit >> 10} KB", 413)(scope, receive, send)
            except ValueError:
                return await error("invalid_request", "bad Content-Length", 400)(scope, receive, send)
        seen = 0

        async def limited():
            nonlocal seen
            msg = await receive()
            if msg["type"] == "http.request":
                seen += len(msg.get("body", b""))
                if seen > limit:
                    raise _TooLarge()
            return msg
        try:
            await self.app(scope, limited, send)
        except _TooLarge:
            with contextlib.suppress(Exception):
                await error("too_large", f"request body over {limit >> 10} KB", 413)(scope, receive, send)


class _TooLarge(Exception):
    pass


def build(cfg=None) -> Starlette:
    cfg = cfg or config.load()
    svc = Service(cfg)
    templates = Templates(cfg.templates_dir)
    mcp = RemoteMCP(svc, VERSION)
    heavy_win, light_win, upload_win = Window(), Window(), Window()

    def base_url(req: Request):
        if cfg.public_url:
            return cfg.public_url
        proto = req.headers.get("x-forwarded-proto") if cfg.trust_proxy else None
        return f"{proto or req.url.scheme}://{req.headers.get('host', req.url.netloc)}"

    def auth(req: Request):
        """(client id, tier) or raises ServiceError(401)."""
        h = req.headers.get("authorization", "")
        if h:
            if not h.lower().startswith("bearer "):
                raise ServiceError("Authorization must be `Bearer <key>`", "unauthorized", 401)
            k = svc.store.lookup(h[7:].strip())
            if not k:
                raise ServiceError("invalid or revoked API key", "unauthorized", 401)
            return "key:" + k["id"], cfg.key.with_overrides(k.get("limits"))
        return "ip:" + client_ip(req.scope.get("client"), req.headers, cfg.trust_proxy), cfg.free

    def rate(client, tier, heavy):
        if heavy:
            ok = heavy_win.allow(client, tier.previews_per_min)
        else:
            ok = light_win.allow(client, tier.previews_per_min * 4)
        if not ok:
            raise ServiceError(f"rate limit: {tier.previews_per_min}/min for frames and sheets"
                               if heavy else "rate limit, slow down", "rate_limited", 429)

    async def body(req: Request) -> dict:
        try:
            b = await req.json()
        except (ValueError, UnicodeDecodeError):
            raise ServiceError("body must be JSON", "invalid_request", 400) from None
        if not isinstance(b, dict):
            raise ServiceError("body must be a JSON object", "invalid_request", 400)
        return b

    def handler(fn):
        async def wrapped(req: Request):
            try:
                return await fn(req)
            except ServiceError as e:
                headers = {"Retry-After": "60"} if e.status == 429 else None
                r = error(e.code, str(e), e.status, **e.extra)
                if headers:
                    r.headers.update(headers)
                if e.status == 401:
                    r.headers["WWW-Authenticate"] = 'Bearer realm="agentfx"'
                return r
            except SourceError as e:
                return error(e.code, str(e), 422)
            except Exception as e:  # noqa: BLE001
                log.exception("unhandled")
                return error("internal", f"internal error ({type(e).__name__})", 500)
        return wrapped

    # ------------------------------------------------------------ templates

    @handler
    async def list_templates(req):
        auth(req)
        out = []
        for tid in templates.ids():
            try:
                m = templates.manifest(tid)
            except (ValueError, OSError):
                continue
            m = dict(m)
            m.setdefault("kind", "video")
            m["preview_url"] = f"{base_url(req)}/api/v1/templates/{tid}/preview.webp" if templates.preview(tid) else None
            out.append(m)
        return JSONResponse(out)

    @handler
    async def get_template(req):
        auth(req)
        tid = req.path_params["id"]
        try:
            m = dict(templates.manifest(tid))
            m.setdefault("kind", "video")
            m["source"] = templates.source(tid)
        except ValueError as e:
            raise ServiceError(str(e), "not_found", 404) from None
        m["preview_url"] = f"{base_url(req)}/api/v1/templates/{tid}/preview.webp" if templates.preview(tid) else None
        return JSONResponse(m)

    @handler
    async def template_preview(req):
        p = templates.preview(req.path_params["id"])
        if not p:
            raise ServiceError("no preview", "not_found", 404)
        return FileResponse(p, media_type="image/webp", headers={"Cache-Control": "public, max-age=3600"})

    @handler
    async def instantiate(req):
        client, tier = auth(req)
        rate(client, tier, False)
        b = await body(req)
        tid, params = b.get("template"), b.get("params") or {}
        if not isinstance(params, dict):
            raise ServiceError("params must be an object", "invalid_request")
        try:
            params = svc.map_template_params(templates.manifest(tid), params)
            size = b.get("size")
            if size:
                _k, _o, plan = svc.stdio.template_plan(templates.manifest(tid), [size], None, templates.mod)
                src = svc.stdio.set_video_size(templates.instantiate(tid, params, size), plan[0][1])
            else:
                src = templates.instantiate(tid, params)
        except ValueError as e:
            raise ServiceError(str(e), "invalid_params", 422) from None
        return JSONResponse({"source": src})

    # ------------------------------------------------------------ CLI operations

    @handler
    async def parse(req):
        client, tier = auth(req)
        rate(client, tier, False)
        b = await body(req)
        to = b.get("to") or ("text" if "json" in b and "source" not in b else "json")
        if "json" in b and "source" not in b:
            return JSONResponse(await svc.parse(tier, doc_json=b["json"], to=to))
        return JSONResponse(await svc.parse(tier, source=b.get("source"), to=to))

    @handler
    async def lint(req):
        client, tier = auth(req)
        rate(client, tier, False)
        b = await body(req)
        return JSONResponse(await svc.lint(tier, b.get("source"), b.get("format"), bool(b.get("measure", True))))

    @handler
    async def timing(req):
        client, tier = auth(req)
        rate(client, tier, False)
        b = await body(req)
        return JSONResponse(await svc.timing(tier, b.get("source"), b.get("format")))

    @handler
    async def frame(req):
        client, tier = auth(req)
        rate(client, tier, True)
        b = await body(req)
        if "at" not in b:
            raise ServiceError("`at` is required (anchor, scene:anchor or seconds like 2.5s)", "invalid_request")
        png, _ = await svc.frame(tier, b.get("source"), b["at"], b.get("format"), b.get("width"))
        return Response(png, media_type="image/png")

    @handler
    async def sheet(req):
        client, tier = auth(req)
        rate(client, tier, True)
        b = await body(req)
        png, _ = await svc.sheet(tier, b.get("source"), b.get("n"), b.get("format"), b.get("cols"), bool(b.get("events")))
        return Response(png, media_type="image/png")

    # ------------------------------------------------------------ assets

    @handler
    async def upload(req):
        client, tier = auth(req)
        if not upload_win.allow(client, 20):
            raise ServiceError("rate limit: 20 uploads/min", "rate_limited", 429)
        ctype = req.headers.get("content-type", "")
        if not ctype.startswith("multipart/form-data"):
            raise ServiceError("send multipart/form-data with a `file` field", "invalid_request", 415)
        form = await req.form(max_files=1, max_fields=4)
        f = form.get("file")
        if f is None or isinstance(f, str):
            raise ServiceError("missing `file` field", "invalid_request")
        data = await f.read(svc.assets.max_upload() + 1)
        await form.close()
        if len(data) > svc.assets.max_upload():
            raise ServiceError("file too large", "too_large", 413)
        if svc.stored_bytes > cfg.storage_max_gb * 1024 ** 3:
            raise ServiceError("the service is out of storage, try again later", "busy", 503)
        if not svc.store.take(client, "uploads", tier.uploads_per_day):
            raise ServiceError(f"daily upload limit reached ({tier.uploads_per_day}/day)", "rate_limited", 429)
        if not svc.store.take(client, "upload_bytes", tier.upload_mb_per_day * 1024 * 1024, len(data)):
            svc.store.give_back(client, "uploads")
            raise ServiceError(f"daily upload volume reached ({tier.upload_mb_per_day} MB/day)", "rate_limited", 429)
        try:
            meta = svc.assets.put(data, client, tier.retention_h)
            svc.stored_bytes += len(data)
        except SourceError:
            svc.store.give_back(client, "uploads")
            svc.store.give_back(client, "upload_bytes", len(data))
            raise
        return JSONResponse({"asset_id": meta["asset_id"], "name": meta["name"], "kind": meta["kind"],
                             "size": meta["size"], "expires_at": int(meta["expires"])}, status_code=201)

    # ------------------------------------------------------------ renders

    @handler
    async def create_render(req):
        client, tier = auth(req)
        rate(client, tier, False)
        b = await body(req)
        job = await svc.create_render(client, tier, source=b.get("source"), fmt=b.get("format"), output=b.get("output"),
                                      quality=b.get("quality") or "preview", template=b.get("template"),
                                      params=b.get("params"), sizes=b.get("sizes"), at=b.get("at"))
        return accepted(req, job)

    @handler
    async def batch(req):
        client, tier = auth(req)
        rate(client, tier, False)
        b = await body(req)
        if not b.get("template"):
            raise ServiceError("`template` is required", "invalid_request")
        job = await svc.create_render(client, tier, output=b.get("output"), quality=b.get("quality") or "preview",
                                      template=b["template"], params=b.get("params"), sizes=b.get("sizes"),
                                      rows=b.get("rows"))
        return accepted(req, job)

    def accepted(req, job):
        d = job.public()
        d["status_url"] = f"{base_url(req)}/api/v1/renders/{job.id}"
        return JSONResponse(d, status_code=202, headers={"Location": d["status_url"]})

    @handler
    async def get_render(req):
        job = svc.get_job(req.path_params["job_id"])
        if not job:
            raise ServiceError("unknown job", "not_found", 404)
        return JSONResponse(job.public(svc, base_url(req)))

    @handler
    async def get_file(req):
        p = svc.file_path(req.path_params["job_id"], req.path_params["name"], req.query_params.get("exp"),
                          req.query_params.get("sig"))
        if not p:
            raise ServiceError("unknown, expired or unsigned file URL", "not_found", 404)
        media = {".mp4": "video/mp4", ".gif": "image/gif", ".webp": "image/webp", ".png": "image/png",
                 ".jpg": "image/jpeg", ".pdf": "application/pdf", ".zip": "application/zip"}.get(p.suffix, "application/octet-stream")
        return FileResponse(p, media_type=media, filename=p.name,
                            headers={"Cache-Control": "private, max-age=3600", "X-Content-Type-Options": "nosniff"})

    # ------------------------------------------------------------ MCP + health

    async def mcp_endpoint(req: Request):
        if req.method != "POST":
            return Response(status_code=405, headers={"Allow": "POST"})
        v = req.headers.get("mcp-protocol-version")
        if v and v not in ("2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"):
            return error("unsupported_protocol_version", f"MCP-Protocol-Version {v} is not supported", 400)
        try:
            client, tier = auth(req)
        except ServiceError as e:
            r = error(e.code, str(e), 401)
            r.headers["WWW-Authenticate"] = 'Bearer realm="agentfx"'
            return r
        try:
            payload = json.loads(await req.body())
        except (ValueError, UnicodeDecodeError):
            return JSONResponse({"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "parse error"}},
                                status_code=400)
        ctx = {"client": client, "tier": tier, "base": base_url(req), "rate": lambda heavy: rate(client, tier, heavy)}
        out = await mcp.handle(payload, ctx)
        if out is None:
            return Response(status_code=202)
        return JSONResponse(out)

    async def health(req):
        return JSONResponse({"ok": True, "version": VERSION, "workers": svc.sched.n, "queue": svc.sched.depth(),
                             "renders_running": svc.sched.renders_running, "sandbox_uids": svc.sandbox.uid_switch})

    routes = [
        Route("/healthz", health),
        Route("/api/v1/templates", list_templates),
        Route("/api/v1/templates/{id}", get_template),
        Route("/api/v1/templates/{id}/preview.webp", template_preview),
        Route("/api/v1/instantiate", instantiate, methods=["POST"]),
        Route("/api/v1/parse", parse, methods=["POST"]),
        Route("/api/v1/lint", lint, methods=["POST"]),
        Route("/api/v1/timing", timing, methods=["POST"]),
        Route("/api/v1/frame", frame, methods=["POST"]),
        Route("/api/v1/sheet", sheet, methods=["POST"]),
        Route("/api/v1/assets", upload, methods=["POST"]),
        Route("/api/v1/renders", create_render, methods=["POST"]),
        Route("/api/v1/batch", batch, methods=["POST"]),
        Route("/api/v1/renders/{job_id}", get_render),
        Route("/api/v1/files/{job_id}/{name}", get_file),
        Route("/mcp", mcp_endpoint, methods=["GET", "POST", "DELETE"]),
    ]

    @contextlib.asynccontextmanager
    async def lifespan(app):
        await svc.start()
        try:
            yield
        finally:
            await svc.stop()

    json_limit = max(cfg.free.source_kb, cfg.key.source_kb) * 1024 * 4 + 16384
    upload_limit = svc.assets.max_upload() + 65536

    def limit_for(path):
        return upload_limit if path.startswith("/api/v1/assets") else json_limit

    origins = [o.strip() for o in os.environ.get("AFX_CORS_ORIGINS", "*").split(",") if o.strip()]
    app = Starlette(routes=routes, lifespan=lifespan, middleware=[
        Middleware(CORSMiddleware, allow_origins=origins, allow_methods=["GET", "POST"],
                   allow_headers=["Authorization", "Content-Type", "MCP-Protocol-Version", "Mcp-Session-Id"],
                   expose_headers=["Location", "Retry-After"], max_age=600),
        Middleware(BodyLimit, limit_for=limit_for),
    ])
    app.state.svc = svc
    return app


def __getattr__(name):
    # `uvicorn agentfx_service.app:app` builds the app on first access
    if name == "app":
        global app
        logging.basicConfig(level=os.environ.get("AFX_LOG_LEVEL", "INFO"))
        app = build()
        return app
    raise AttributeError(name)
