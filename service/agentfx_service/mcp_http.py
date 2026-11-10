"""Remote MCP: Streamable HTTP transport (MCP 2025-06-18; also answers 2025-11-25, 2025-03-26
and 2024-11-05 clients) at /mcp, stateless, JSON responses (no SSE stream, no sessions).

Tools and handlers are the stdio server's (mcp/server.py: TOOLS and call()), with the
filesystem arguments removed from the schemas (`file`, `assets_dir`, `out`; a hosted server
cannot read or write the caller's paths) and agentfx_critique left out (it reads local files
and spends the operator's Anthropic key). The CLI runs through the service's sandbox and job
queue; agentfx_render becomes a render job and returns its URL.
"""
from __future__ import annotations

import asyncio
import base64
import copy
import json

from . import stdio as stdio_mod
from . import validate
from .core import ServiceError
from .validate import SourceError

SUPPORTED = ("2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05")
LATEST = "2025-06-18"
REMOVED_ARGS = ("file", "assets_dir", "out", "out_dir")
EXCLUDED = {"agentfx_critique"}
IMAGE_TOOLS = {"agentfx_frame", "agentfx_sheet", "agentfx_storyboard"}


def load_stdio_server(home=None):
    return stdio_mod.load(home)


def remote_tools(stdio):
    out = []
    for t in stdio.TOOLS:
        if t["name"] in EXCLUDED:
            continue
        t = copy.deepcopy(t)
        schema = t["inputSchema"]
        props = schema.setdefault("properties", {})
        had_source = "source" in props
        for k in REMOVED_ARGS:
            props.pop(k, None)
        req = [r for r in schema.get("required", []) if r in props]
        if had_source:
            props["source"] = {"type": "string", "description": "Reel Script text. Assets: upload with POST "
                               "/api/v1/assets and reference the returned name (no paths or URLs)."}
            req = ["source"] + [r for r in req if r != "source"]
        if t["name"] == "agentfx_render":
            props.pop("alpha", None)
            props["output"] = {"type": "string", "enum": ["mp4", "gif", "webp"], "default": "mp4"}
            props["quality"] = {"type": "string", "enum": ["preview", "hd"], "default": "preview",
                                "description": "hd needs an API key"}
            t["description"] = ("Render the video on the AgentFX render service. Returns the job status and, "
                                "when it finishes within ~50 s, the download URL; else poll the status_url "
                                "(GET, same auth).")
        if t["name"] == "agentfx_render_template":
            t["description"] += (" Hosted: returns the job status and the download URLs (one per size) when done "
                                 "within ~50 s, else a status_url to poll; image/audio params take an asset id or "
                                 "name from POST /api/v1/assets.")
            props["quality"] = {"type": "string", "enum": ["preview", "hd"], "default": "preview",
                                "description": "videos: hd needs an API key"}
        if req:
            schema["required"] = req
        else:
            schema.pop("required", None)
        out.append(t)
    return out


class RemoteMCP:
    def __init__(self, svc, version="0.1.1"):
        self.svc = svc
        self.stdio = load_stdio_server(svc.cfg.agentfx_home)
        self.tools = remote_tools(self.stdio)
        self.by_name = {t["name"]: t for t in self.tools}
        self.version = version

    # ---------------------------------------------------------------- JSON-RPC

    async def handle(self, body, ctx) -> list | dict | None:
        """One POST body -> response payload (None = 202, nothing to send)."""
        if isinstance(body, list):
            if not body:
                return _err(None, -32600, "empty batch")
            outs = [r for r in [await self.one(m, ctx) for m in body] if r is not None]
            return outs or None
        return await self.one(body, ctx)

    async def one(self, msg, ctx):
        if not isinstance(msg, dict) or msg.get("jsonrpc") != "2.0":
            return _err(msg.get("id") if isinstance(msg, dict) else None, -32600, "invalid JSON-RPC message")
        if "method" not in msg:
            return None  # a response from the client; we never send requests
        mid, method, params = msg.get("id"), msg["method"], msg.get("params") or {}
        if "id" not in msg:
            return None  # notification (notifications/initialized, cancelled, ...)
        if not isinstance(params, dict):
            return _err(mid, -32602, "params must be an object")
        try:
            if method == "initialize":
                want = params.get("protocolVersion")
                res = {"protocolVersion": want if want in SUPPORTED else LATEST,
                       "capabilities": {"tools": {"listChanged": False}},
                       "serverInfo": {"name": "agentfx", "title": "AgentFX render service", "version": self.version},
                       "instructions": "AgentFX: write Reel Script, check it with agentfx_lint and agentfx_frame, "
                                       "render with agentfx_render (returns a job URL). Start with agentfx_spec."}
            elif method == "ping":
                res = {}
            elif method == "tools/list":
                res = {"tools": self.tools}
            elif method == "tools/call":
                name = params.get("name")
                if name not in self.by_name:
                    return _err(mid, -32602, f"unknown tool {name!r}")
                args = params.get("arguments") or {}
                if not isinstance(args, dict):
                    return _err(mid, -32602, "arguments must be an object")
                res = await self.call(name, args, ctx)
            elif method in ("resources/list", "prompts/list"):
                return _err(mid, -32601, f"method {method} not supported")
            else:
                return _err(mid, -32601, f"method {method} not supported")
            return {"jsonrpc": "2.0", "id": mid, "result": res}
        except Exception as e:  # noqa: BLE001 - tool errors are results
            return {"jsonrpc": "2.0", "id": mid, "result": _tool_error(_msg(e))}

    # ---------------------------------------------------------------- tools

    async def call(self, name, args, ctx):
        svc, tier = self.svc, ctx["tier"]
        if name == "agentfx_render":
            return await self.render(args, ctx)
        if name == "agentfx_render_template":
            return await self.render_template(args, ctx)
        if name == "agentfx_list_templates":
            items = await asyncio.to_thread(self.stdio.template_list, str(svc.cfg.templates_dir))
            return {"content": [{"type": "text", "text": json.dumps(items, indent=1)}]}
        heavy = name in IMAGE_TOOLS
        ctx["rate"](heavy)
        a = self.clean_args(name, args)
        if "source" in self.by_name[name]["inputSchema"]["properties"]:
            svc.check_source_size(a.get("source"), tier)

        def fn(slot):
            job_dir = None
            try:
                if "source" in a:
                    job_dir, facts, *_ = svc.prepare(slot, a["source"], strict=(name != "agentfx_lint"))
                    if heavy and validate.short_side(facts) > 2160:
                        raise ServiceError("previews are limited to a 2160px short side", "limit_exceeded")
                else:
                    job_dir = svc.sandbox.new_job_dir()

                def runner(cmd, cwd=None):
                    r = svc.run_cli(slot, job_dir, cmd)
                    if r.killed:
                        return 1, "", f"stopped: {r.killed} limit"
                    return r.code, r.stdout, r.stderr
                return self.stdio.call(name, a, run_cli=runner, workdir=str(job_dir))
            finally:
                if job_dir is not None:
                    svc.sandbox.remove(job_dir)
        if name in ("agentfx_spec", "agentfx_design_guide", "agentfx_lens"):
            content = await asyncio.to_thread(self.stdio.call, name, a)  # file reads only
        else:
            content = await svc.preview_op(fn)
        is_error = any(c.pop("_isError", False) for c in content if isinstance(c, dict))
        # never leak job dir paths to the client
        jobs = str(svc.cfg.jobs_dir)
        for c in content:
            if c.get("type") == "text":
                c["text"] = c["text"].replace(jobs, "<job>")
        res = {"content": content}
        if is_error:
            res["isError"] = True
        return res

    def clean_args(self, name, args):
        """Only schema arguments, validated; filesystem arguments are dropped."""
        props = self.by_name[name]["inputSchema"].get("properties", {})
        a = {k: v for k, v in args.items() if k in props}
        if "at" in props:
            if "at" not in a:
                raise SourceError("`at` is required", "invalid_request")
            a["at"] = validate.check_at(a["at"])
        if "format" in a:
            a["format"] = validate.check_format(a["format"])
            if a["format"] is None:
                a.pop("format")
        for k, lo, hi in (("n", 1, 36), ("cols", 1, 8), ("seed", 0, 2**31 - 1)):
            if k in a:
                a[k] = validate.check_int(a[k], k, lo, hi, None)
        for k in ("measure", "events", "md", "no_tts"):
            if k in a:
                a[k] = bool(a[k])
        if "no_tts" in props:
            a["no_tts"] = True
        if "name" in a:
            a["name"] = str(a["name"])
            if not validate.SAFE_NAME.match(a["name"]) or ".." in a["name"]:
                raise SourceError("unknown lens", "invalid_request")
        if "section" in a:
            a["section"] = str(a["section"])[:64]
        if "source" in props and not isinstance(a.get("source"), str):
            raise SourceError("`source` (Reel Script text) is required", "invalid_request")
        return a

    async def render(self, args, ctx):
        svc = self.svc
        a = self.clean_args("agentfx_render", args)
        job = await svc.create_render(ctx["client"], ctx["tier"], source=a["source"], fmt=a.get("format"),
                                      output=a.get("output", "mp4"), quality=a.get("quality", "preview"))
        return await self._job_result(job, ctx)

    async def render_template(self, args, ctx):
        svc = self.svc
        a = {k: v for k, v in args.items() if k in ("template", "params", "sizes", "output", "quality")}
        if not isinstance(a.get("template"), str):
            raise SourceError("`template` is required", "invalid_request")
        job = await svc.create_render(ctx["client"], ctx["tier"], output=a.get("output"),
                                      quality=a.get("quality", "preview"), template=a["template"],
                                      params=a.get("params"), sizes=a.get("sizes"))
        return await self._job_result(job, ctx, images=True)

    async def _job_result(self, job, ctx, images=False):
        svc = self.svc
        job = await svc.wait_job(job.id, svc.cfg.mcp_wait_s) or job
        base = ctx["base"]
        info = job.public(svc, base)
        info["status_url"] = f"{base}/api/v1/renders/{job.id}"
        if job.status == "error":
            return _tool_error(f"render failed: {job.error}\n{json.dumps(info)}")
        content = []
        if job.status == "done":
            urls = [f["url"] for f in info.get("files", [])] or [info["url"]]
            text = "rendered:\n" + "\n".join(urls) + "\n"
            if images and job.output == "png":
                # small stills come back as images too (like agentfx_frame)
                for f in job.files[:4]:
                    p = svc.cfg.outputs_dir / job.id / f["name"]
                    if p.is_file() and p.stat().st_size <= 2 * 1024 * 1024:
                        content.append({"type": "image", "data": base64.b64encode(p.read_bytes()).decode(),
                                        "mimeType": "image/png"})
        else:
            text = f"render {job.status} ({info['progress']:.0%}); poll {info['status_url']}\n"
        return {"content": [{"type": "text", "text": text + json.dumps(info)}] + content, "structuredContent": info}


def _msg(e):
    if isinstance(e, (ServiceError, SourceError)):
        extra = getattr(e, "extra", None) or {}
        d = extra.get("diagnostics")
        return str(e) + (("\n" + json.dumps(d)[:4000]) if d else "")
    return f"error: {e}"


def _tool_error(text):
    return {"content": [{"type": "text", "text": text}], "isError": True}


def _err(mid, code, message):
    return {"jsonrpc": "2.0", "id": mid, "error": {"code": code, "message": message}}
