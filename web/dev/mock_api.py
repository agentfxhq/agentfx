#!/usr/bin/env python3
"""Mock of the AgentFX HTTP API (docs/PLATFORM.md) for developing the web playground.

Stdlib only. Every endpoint shells out to the `agentfx` CLI in a throwaway job dir; renders run in a
background thread so progress polling works. Not hardened: no auth, no limits beyond a small
concurrency cap. For local development and the e2e tests only.

    python3 web/dev/mock_api.py [--port 8793] [--templates DIR]

Environment: AGENTFX_BIN (CLI path), AGENTFX_TEMPLATES (template dir), MOCK_TTS (default: none).
"""
import argparse
import csv
import io
import zipfile
import email.parser
import email.policy
import json
import mimetypes
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs, unquote

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, '..', '..'))
DATA = os.path.join(HERE, '.mock-data')
FIXTURES = os.path.join(HERE, 'fixtures', 'templates')
API = '/api/v1'
COLOR_RE = re.compile(r'^#[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$')
FORMATS = {'16:9', '9:16', '1:1', '4:5'}
CLI_SLOTS = threading.Semaphore(2)  # shared box: at most two CLI processes at once


def find_cli():
    env = os.environ.get('AGENTFX_BIN')
    if env:
        return env
    d = REPO
    for _ in range(8):  # worktrees live below the main checkout, which has the build
        cand = os.path.join(d, 'target', 'release', 'agentfx')
        if os.path.isfile(cand):
            return cand
        d = os.path.dirname(d)
    return shutil.which('agentfx') or 'agentfx'


CLI = find_cli()
TTS = os.environ.get('MOCK_TTS', 'none')


class ApiError(Exception):
    def __init__(self, status, code, message, **extra):
        super().__init__(message)
        self.status, self.code, self.message, self.extra = status, code, message, extra


HEAVY = {'render', 'frame', 'sheet'}
# on a shared box, heavy CLI work takes a machine-wide lock (MOCK_FLOCK=path, '' to disable)
FLOCK = os.environ.get('MOCK_FLOCK', '/tmp/claude-1000/agentfx-heavy.lock')


def run_cli(args, cwd, timeout=120):
    cmd = [CLI] + args
    if args and args[0] in HEAVY and FLOCK and shutil.which('flock') and os.path.isdir(os.path.dirname(FLOCK)):
        cmd = ['flock', FLOCK] + cmd
    with CLI_SLOTS:
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, timeout=timeout)
    return p.returncode, p.stdout.decode('utf-8', 'replace'), p.stderr.decode('utf-8', 'replace')


def json_prefix(text):
    """The first JSON value in text (the CLI prints a summary line after it)."""
    text = text.strip()
    if not text:
        return None
    try:
        return json.loads(text)
    except ValueError:
        pass
    try:
        val, _ = json.JSONDecoder().raw_decode(text)
        return val
    except ValueError:
        return None


# ---------------------------------------------------------------- templates

def templates_dir():
    env = os.environ.get('AGENTFX_TEMPLATES')
    if env:
        return env
    repo_t = os.path.join(REPO, 'templates')
    if os.path.isdir(repo_t) and any(os.path.isfile(os.path.join(repo_t, d, 'manifest.json'))
                                    for d in os.listdir(repo_t)):
        return repo_t
    return FIXTURES


def load_manifest(tid):
    if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,63}', tid or ''):
        raise ApiError(404, 'not_found', f'unknown template {tid!r}')
    path = os.path.join(templates_dir(), tid, 'manifest.json')
    if not os.path.isfile(path):
        raise ApiError(404, 'not_found', f'unknown template {tid!r}')
    with open(path, encoding='utf-8') as f:
        return json.load(f)


def _rt():
    """templates/render_template.py of the repo: SIZE_PRESETS / size_px (also for the fixtures)."""
    global _RT
    if _RT is None:
        import importlib.util
        spec = importlib.util.spec_from_file_location('render_template', os.path.join(REPO, 'templates', 'render_template.py'))
        _RT = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(_RT)
    return _RT


_RT = None


def size_px(m):
    """{name: "WxH"} for the manifest's `sizes` (a list of preset names)."""
    out = {}
    for n in m.get('sizes') or []:
        try:
            w, h = _rt().size_px(n)
        except ValueError:
            continue
        out[n] = f'{w}x{h}'
    return out


def with_manifest_extras(m):
    m.setdefault('kind', 'video')
    m['size_px'] = size_px(m)
    return m


def list_templates():
    out = []
    root = templates_dir()
    for d in sorted(os.listdir(root)):
        if os.path.isfile(os.path.join(root, d, 'manifest.json')):
            m = with_manifest_extras(load_manifest(d))
            m['preview_url'] = f'{API}/templates/{d}/preview'
            out.append(m)
    return out


def esc_text(v):
    return str(v).replace('\\', '\\\\').replace('"', '\\"')


def coerce_params(manifest, params):
    vals, rows = {}, {}
    for p in manifest.get('params', []):
        pid, typ = p['id'], p['type']
        v = params.get(pid, p.get('default'))
        if typ == 'text':
            v = '' if v is None else str(v)
            if '\n' in v or '\r' in v:
                raise ApiError(400, 'bad_param', f'{pid}: newlines are not allowed in a text param')
            v = esc_text(v[: p.get('max_len', 200)])
        elif typ == 'longtext':
            v = '' if v is None else str(v)[: p.get('max_len', 4000)]
        elif typ == 'data':
            rows[pid] = parse_rows(v if v is not None else [], p)
            v = ''
        elif typ == 'color':
            v = str(v or '')
            if not COLOR_RE.match(v):
                raise ApiError(400, 'bad_param', f'{pid}: colour must look like #RRGGBB')
        elif typ == 'number':
            try:
                v = float(v)
            except (TypeError, ValueError):
                raise ApiError(400, 'bad_param', f'{pid}: not a number')
            v = min(max(v, p.get('min', v)), p.get('max', v))
            v = ('%g' % v)
        elif typ == 'enum':
            if v not in p.get('options', []):
                raise ApiError(400, 'bad_param', f'{pid}: must be one of {p.get("options")}')
        elif typ in ('audio', 'image'):
            if v and v in ASSETS:
                v = ASSETS[v]['name']
            elif v and not re.fullmatch(r'[\w.-]+', str(v)):
                raise ApiError(400, 'bad_param', f'{pid}: unknown asset')
            v = v or ''
        vals[pid] = '' if v is None else str(v)
    return vals, rows


def parse_rows(v, p):
    """A `data` param: a JSON array of row objects, or CSV text (header row optional)."""
    cols = [c['id'] for c in p.get('columns', [])]
    if isinstance(v, str):
        text = v.strip()
        if text.startswith('['):
            v = json.loads(text)
        else:
            rows = list(csv.reader(io.StringIO(text)))
            if not rows:
                return []
            head = [h.strip() for h in rows[0]]
            if cols and not set(head) & set(cols):  # no header row: positional columns
                head, body = cols, rows
            else:
                body = rows[1:]
            v = [{head[i]: c.strip() for i, c in enumerate(r) if i < len(head)} for r in body if any(c.strip() for c in r)]
    if not isinstance(v, list) or not all(isinstance(r, dict) for r in v):
        raise ApiError(400, 'bad_param', f'{p["id"]}: data must be a list of rows')
    ctype = {c['id']: c.get('type', 'text') for c in p.get('columns', [])}
    out = []
    for r in v[: p.get('max_rows', 200)]:
        row = {}
        for k, x in r.items():
            if ctype.get(k) == 'number':
                try:
                    row[k] = '%g' % float(x)
                except (TypeError, ValueError):
                    raise ApiError(400, 'bad_param', f'{p["id"]}: {k}={x!r} is not a number')
            else:
                row[k] = esc_text(re.sub(r'[\r\n]+', ' ', '' if x is None else str(x)))
        out.append(row)
    return out


def substitute(src, manifest, vals, rows=None):
    """Builtin substitution (the rules of templates/render_template.py, for the fixtures):
    {{id}} -> value; {{#id}}...{{/id}} repeats per non-empty line of a longtext param ({{line}},
    {{i}}) or per row of a data param ({{column}}, {{i}}); {{id.column}} -> that column as a list
    (`"a", "b"` for text columns, `1, 2` for number columns)."""
    rows = rows or {}
    ctypes = {p['id']: {c['id']: c.get('type', 'text') for c in p.get('columns', [])} for p in manifest.get('params', [])}

    def block(m):
        pid, body = m.group(1), m.group(2)
        out = []
        if pid in rows:
            for i, r in enumerate(rows[pid], 1):
                b = re.sub(r'\{\{([\w-]+)\}\}', lambda mm: r.get(mm.group(1), mm.group(0)), body)
                out.append(b.replace('{{i}}', str(i)))
            return ''.join(out)
        lines = [ln.strip() for ln in vals.get(pid, '').splitlines() if ln.strip()] or [' ']
        for i, ln in enumerate(lines, 1):
            out.append(body.replace('{{line}}', esc_text(ln)).replace('{{i}}', str(i)))
        return ''.join(out)
    src = re.sub(r'\{\{#([\w-]+)\}\}\n?(.*?)\{\{/\1\}\}\n?', block, src, flags=re.S)

    def column(m):
        pid, col = m.group(1), m.group(2)
        if pid not in rows:
            return m.group(0)
        if ctypes.get(pid, {}).get(col) == 'number':
            return ', '.join(r.get(col, '0') for r in rows[pid])
        return ', '.join('"%s"' % r.get(col, '') for r in rows[pid])
    src = re.sub(r'\{\{([\w-]+)\.([\w-]+)\}\}', column, src)
    return re.sub(r'\{\{([\w-]+)\}\}', lambda m: vals.get(m.group(1), m.group(0)), src)


def instantiate(tid, params, size=None):
    """`size`: a preset name of the manifest's `sizes` (stills are laid out per size)."""
    manifest = load_manifest(tid)
    if size is not None and size not in size_px(manifest):
        raise ApiError(400, 'bad_size', f'template {tid!r} has no size {size!r}')
    tdir = os.path.join(templates_dir(), tid)
    script = os.path.join(templates_dir(), 'render_template.py')
    if os.path.isfile(script) and templates_dir() != FIXTURES:
        args = [sys.executable, script, tid]
        for k, v in (params or {}).items():
            if k in ASSETS_BY_TYPE_PARAM(manifest) and v in ASSETS:
                v = ASSETS[v]['name']
            if isinstance(v, (list, dict)):
                v = json.dumps(v)
            args += ['--set', f'{k}={v}']
        if size:
            args += ['--size', size]
        p = subprocess.run(args, capture_output=True, timeout=30)
        if p.returncode != 0:
            raise ApiError(400, 'bad_param', p.stderr.decode('utf-8', 'replace').strip()[-400:])
        return p.stdout.decode('utf-8')
    with open(os.path.join(tdir, 'template.reel'), encoding='utf-8') as f:
        src = f.read()
    vals, rows = coerce_params(manifest, params or {})
    return with_size(substitute(src, manifest, vals, rows), size_px(manifest)[size] if size else None)


def ASSETS_BY_TYPE_PARAM(manifest):
    return {p['id'] for p in manifest.get('params', []) if p['type'] in ('audio', 'image')}


# ---------------------------------------------------------------- assets and job dirs

ASSETS = {}  # asset_id -> {name, path}
JOBS = {}    # job_id -> {status, progress, ...}


def new_job_dir(source):
    if not isinstance(source, str) or not source.strip():
        raise ApiError(400, 'bad_request', 'source is required')
    if len(source.encode('utf-8')) > 64 * 1024:
        raise ApiError(413, 'too_large', 'source is larger than 64 KB')
    jid = uuid.uuid4().hex[:16]
    d = os.path.join(DATA, 'jobs', jid)
    os.makedirs(d)
    with open(os.path.join(d, 'source.reel'), 'w', encoding='utf-8') as f:
        f.write(source)
    # assets referenced by name: uploads first, then files shipped with templates
    pool = {a['name']: a['path'] for a in ASSETS.values()}
    root = templates_dir()
    for t in os.listdir(root):
        td = os.path.join(root, t)
        if os.path.isdir(td):
            for fn in os.listdir(td):
                if not fn.endswith(('.reel', '.json', '.md', '.py')) and fn not in pool:
                    pool[fn] = os.path.join(td, fn)
    for name, path in pool.items():
        if name in source:
            os.symlink(path, os.path.join(d, name))
    return jid, d


SIZE_RE = re.compile(r'^(\d{2,5})x(\d{2,5})$')


def parse_size(size):
    m = SIZE_RE.match(str(size))
    if not m or not (16 <= int(m[1]) <= 8000 and 16 <= int(m[2]) <= 8000):
        raise ApiError(400, 'bad_size', 'size must look like 1080x1920 (16..8000 px)')
    return int(m[1]), int(m[2])


def with_size(source, size):
    """`size: "WxH"` (API addition). Like formats (SPEC §11) the layout keeps the script's short
    side: the header gets the requested aspect at the base short side, so `when portrait` etc.
    apply and px sizes keep their meaning; stills are then scaled to the exact pixels."""
    if not size:
        return source
    w, h = parse_size(size)
    lines = source.split('\n')
    for i, ln in enumerate(lines):
        if re.match(r'\s*video\b', ln):
            m = re.search(r'\bsize=(\d+)x(\d+)', ln)
            short = min(int(m[1]), int(m[2])) if m else 1080
            k = short / min(w, h)
            lw, lh = round(w * k / 2) * 2, round(h * k / 2) * 2
            if m:
                ln = re.sub(r'\bsize=\S+', f'size={lw}x{lh}', ln, count=1)
            else:
                ln = ln.rstrip() + f' size={lw}x{lh}'
            # a size override replaces the multi-format list
            lines[i] = re.sub(r'\s+formats=\[[^\]]*\]', '', ln)
            return '\n'.join(lines)
    return f'video size={w}x{h}\n' + source


def src_of(body):
    return with_size(body.get('source'), body.get('size'))


def fmt_args(body):
    if body.get('size'):
        return []
    f = body.get('format')
    if f in (None, ''):
        return []
    if f not in FORMATS:
        raise ApiError(400, 'bad_format', f'format must be one of {sorted(FORMATS)}')
    return ['--format', f]


def gc_jobs(max_age=3600):
    root = os.path.join(DATA, 'jobs')
    if not os.path.isdir(root):
        return
    now = time.time()
    for j in os.listdir(root):
        p = os.path.join(root, j)
        try:
            if now - os.path.getmtime(p) > max_age and JOBS.get(j, {}).get('status') not in ('queued', 'running'):
                shutil.rmtree(p, ignore_errors=True)
        except OSError:
            pass


# ---------------------------------------------------------------- endpoints

def ep_parse(body):
    to = body.get('to', 'json')
    if 'json' in body and body['json'] is not None:
        jid, d = new_job_dir('x')
        src_path = os.path.join(d, 'in.json')
        with open(src_path, 'w', encoding='utf-8') as f:
            json.dump(body['json'], f)
    else:
        jid, d = new_job_dir(src_of(body))
        src_path = os.path.join(d, 'source.reel')
    rc, out, err = run_cli(['parse', src_path, '--to', to], d)
    shutil.rmtree(d, ignore_errors=True)
    diags = json_prefix(err) if err.strip().startswith('[') else None
    if rc != 0 or not out.strip():
        raise ApiError(422, 'parse_error', (diags or [{}])[0].get('message', err.strip()[-300:]) if diags else err.strip()[-300:] or 'parse failed',
                       diagnostics=diags or [])
    if to == 'json':
        return {'json': json.loads(out)}
    return {'source': out}


def ep_lint(body):
    jid, d = new_job_dir(src_of(body))
    rc, out, err = run_cli(['lint', 'source.reel', '--tts', TTS] + fmt_args(body), d)
    shutil.rmtree(d, ignore_errors=True)
    diags = json_prefix(out)
    if diags is None:
        diags = json_prefix(err) or []
    return {'diagnostics': diags}


def ep_timing(body):
    jid, d = new_job_dir(src_of(body))
    rc, out, err = run_cli(['timing', 'source.reel', '--tts', TTS] + fmt_args(body), d)
    shutil.rmtree(d, ignore_errors=True)
    data = json_prefix(out)
    if data is None:
        raise ApiError(422, 'compile_error', err.strip()[-300:] or 'timing failed', diagnostics=json_prefix(err) or [])
    return data


def scale_png(path, width):
    ff = shutil.which('ffmpeg')
    if not ff or not width:
        return path
    out = path[:-4] + f'-{width}.png'
    p = subprocess.run([ff, '-v', 'error', '-y', '-i', path, '-vf', f'scale={int(width)}:-2:flags=lanczos', out],
                       capture_output=True, timeout=30)
    return out if p.returncode == 0 and os.path.isfile(out) else path


def ep_frame(body):
    at = str(body.get('at', '0s'))
    if not re.fullmatch(r'[\w.:+\-@]+', at):
        raise ApiError(400, 'bad_request', 'bad `at`')
    jid, d = new_job_dir(src_of(body))
    try:
        rc, out, err = run_cli(['frame', 'source.reel', '--at', at, '--tts', TTS, '-o', 'frame.png'] + fmt_args(body), d)
        png = os.path.join(d, 'frame.png')
        if rc != 0 or not os.path.isfile(png):
            raise ApiError(422, 'compile_error', 'the script has errors; fix them first', diagnostics=json_prefix(err) or [])
        width = int(body.get('width') or 640)
        with open(scale_png(png, min(max(width, 64), 1920)), 'rb') as f:
            return f.read()
    finally:
        shutil.rmtree(d, ignore_errors=True)


def ep_sheet(body):
    jid, d = new_job_dir(src_of(body))
    try:
        n = int(body.get('n') or 12)
        rc, out, err = run_cli(['sheet', 'source.reel', '--n', str(min(max(n, 1), 24)), '--tts', TTS, '-o', 'sheet.png'] + fmt_args(body), d, timeout=300)
        png = os.path.join(d, 'sheet.png')
        if rc != 0 or not os.path.isfile(png):
            raise ApiError(422, 'compile_error', 'the script has errors; fix them first', diagnostics=json_prefix(err) or [])
        with open(png, 'rb') as f:
            return f.read()
    finally:
        shutil.rmtree(d, ignore_errors=True)


FILE_URL_RE = re.compile(r'file://(/[^)\'"\s]+)')


def ep_preview(body):
    """API addition: the HTML backend for a live in-browser preview (window.__reel.seek/play)."""
    jid, d = new_job_dir(src_of(body))
    rc, out, err = run_cli(['html', 'source.reel', '--no-embed', '--tts', TTS, '-o', 'preview.html'] + fmt_args(body), d)
    path = os.path.join(d, 'preview.html')
    if not os.path.isfile(path):
        raise ApiError(422, 'compile_error', err.strip()[-300:] or 'html failed', diagnostics=json_prefix(err) or [])
    with open(path, encoding='utf-8') as f:
        html = f.read()

    def rewrite(m):
        p = unquote(m.group(1))
        base = os.path.basename(p)
        if '/fonts/' in p and os.path.isfile(p):
            FONTS[base] = p
            return f'{API}/fonts/{base}'
        real = os.path.realpath(p)
        if os.path.isfile(real):
            name = base
            if not os.path.exists(os.path.join(d, name)):
                os.symlink(real, os.path.join(d, name))
            return f'{API}/files/{jid}/{name}'
        return m.group(0)
    html = FILE_URL_RE.sub(rewrite, html)
    os.remove(path)
    JOBS.setdefault(jid, {'status': 'preview', 'dir': d})
    return html.encode('utf-8')


FONTS = {}


def ep_assets(handler):
    ctype = handler.headers.get('Content-Type', '')
    length = int(handler.headers.get('Content-Length') or 0)
    if length > 30 * 1024 * 1024:
        raise ApiError(413, 'too_large', 'asset larger than 30 MB')
    raw = handler.rfile.read(length)
    msg = email.parser.BytesParser(policy=email.policy.HTTP).parsebytes(
        b'Content-Type: ' + ctype.encode() + b'\r\n\r\n' + raw)
    for part in msg.iter_parts():
        if part.get_param('name', header='content-disposition') == 'file':
            fname = part.get_filename() or 'upload.bin'
            ext = os.path.splitext(fname)[1].lower()
            if ext not in ('.mp3', '.wav', '.ogg', '.m4a', '.png', '.jpg', '.jpeg', '.webp', '.svg', '.ttf', '.otf', '.woff', '.woff2'):
                raise ApiError(415, 'bad_type', f'unsupported file type {ext}')
            aid = uuid.uuid4().hex[:12]
            stem = re.sub(r'[^\w-]+', '-', os.path.splitext(fname)[0]).strip('-')[:40] or 'asset'
            name = f'{stem}-{aid[:6]}{ext}'
            ad = os.path.join(DATA, 'assets', aid)
            os.makedirs(ad)
            path = os.path.join(ad, name)
            with open(path, 'wb') as f:
                f.write(part.get_payload(decode=True))
            ASSETS[aid] = {'name': name, 'path': path}
            return {'asset_id': aid, 'name': name}
    raise ApiError(400, 'bad_request', 'multipart field `file` missing')


VIDEO_OUT = ('mp4', 'gif', 'webp')
STILL_OUT = ('png', 'jpg', 'pdf')


def jpeg_pdf(jpg_bytes, w, h, dpi=300):
    """A one-page PDF that shows a JPEG at `dpi` (stdlib only)."""
    pw, ph = w * 72 / dpi, h * 72 / dpi
    content = f'q {pw:.2f} 0 0 {ph:.2f} 0 0 cm /Im0 Do Q'.encode()
    objs = [
        b'<< /Type /Catalog /Pages 2 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2f} {ph:.2f}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>'.encode(),
        f'<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {len(jpg_bytes)} >>\nstream\n'.encode() + jpg_bytes + b'\nendstream',
        f'<< /Length {len(content)} >>\nstream\n'.encode() + content + b'\nendstream',
    ]
    out = bytearray(b'%PDF-1.4\n')
    offs = []
    for i, o in enumerate(objs, 1):
        offs.append(len(out))
        out += f'{i} 0 obj\n'.encode() + o + b'\nendobj\n'
    xref = len(out)
    out += f'xref\n0 {len(objs) + 1}\n0000000000 65535 f \n'.encode()
    for o in offs:
        out += f'{o:010d} 00000 n \n'.encode()
    out += f'trailer << /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode()
    return bytes(out)


def render_one(d, source, name, output, at, fargs, target=None):
    """Render `source` in job dir d to d/name; returns an error message or None."""
    err = ''
    src_file = f'{os.path.splitext(name)[0]}.reel'
    with open(os.path.join(d, src_file), 'w', encoding='utf-8') as f:
        f.write(source)
    if output in VIDEO_OUT:
        rc, out, err = run_cli(['render', src_file, '--tts', TTS, '-o', name] + fargs, d, timeout=900)
    else:
        png = name if output == 'png' else name + '.png'
        rc, out, err = run_cli(['frame', src_file, '--at', at or '0s', '--tts', TTS, '-o', png] + fargs, d)
        if rc == 0 and target:  # exact pixel size of a still
            ff = shutil.which('ffmpeg') or 'ffmpeg'
            p = subprocess.run([ff, '-v', 'error', '-y', '-i', os.path.join(d, png), '-vf', f'scale={target[0]}:{target[1]}:flags=lanczos', os.path.join(d, 'scaled-' + png)], capture_output=True, timeout=120)
            if p.returncode == 0:
                os.replace(os.path.join(d, 'scaled-' + png), os.path.join(d, png))
        if rc == 0 and output != 'png':
            ff = shutil.which('ffmpeg') or 'ffmpeg'
            jpg = name if output == 'jpg' else name + '.jpg'
            p = subprocess.run([ff, '-v', 'error', '-y', '-i', os.path.join(d, png), '-q:v', '2', os.path.join(d, jpg)], capture_output=True, timeout=60)
            rc = p.returncode
            if rc == 0 and output == 'pdf':
                m = re.search(r'size=(\d+)x(\d+)', source)
                w, h = target or ((int(m[1]), int(m[2])) if m else (1080, 1080))
                with open(os.path.join(d, jpg), 'rb') as f:
                    data = jpeg_pdf(f.read(), w, h)
                with open(os.path.join(d, name), 'wb') as f:
                    f.write(data)
    if rc == 0 and os.path.isfile(os.path.join(d, name)):
        return None
    diags = json_prefix(err)
    return (diags[0]['message'] if isinstance(diags, list) and diags else err.strip()[-300:]) or 'render failed'


def parse_sizes(manifest, sizes):
    """`sizes`: preset names from the manifest (None: the template's own size) -> [(name, WxH)]."""
    if not sizes:
        return [(None, None)]
    px = size_px(manifest)
    if not isinstance(sizes, list) or len(sizes) > 12 or len(set(map(str, sizes))) != len(sizes):
        raise ApiError(400, 'bad_size', 'sizes must be a list of up to 12 distinct preset names')
    for n in sizes:
        if n not in px:
            raise ApiError(400, 'bad_size', f'unknown size {n!r} (sizes: {", ".join(px) or "none"})')
    return [(n, px[n]) for n in sizes]


def start_job(outputs, output, zipped=False):
    """outputs: [(file name, source, at, fargs, target px, size name, row)]; renders them in a
    thread. A batch (zipped) is zipped and its `url` is the zip; otherwise `url` = first file."""
    jid = uuid.uuid4().hex[:16]
    d = os.path.join(DATA, 'jobs', jid)
    os.makedirs(d)
    job = {'status': 'queued', 'progress': 0.0, 'dir': d, 'started': time.time(), 'done_n': 0, 'n': len(outputs)}
    JOBS[jid] = job
    per = 3.0 if output in VIDEO_OUT else 1.2

    def work():
        job['status'] = 'running'
        job['expected'] = per * len(outputs)
        files = []
        for name, source, at, fargs, target, label, row in outputs:
            # link the assets the source references (same rules as new_job_dir)
            _, tmp = new_job_dir(source)
            for fn in os.listdir(tmp):
                if fn != 'source.reel' and not os.path.exists(os.path.join(d, fn)):
                    os.symlink(os.path.realpath(os.path.join(tmp, fn)), os.path.join(d, fn))
            shutil.rmtree(tmp, ignore_errors=True)
            err = render_one(d, source, name, output, at, fargs, target if output in STILL_OUT else None)
            if err:
                job.update(status='error', error=f'{name}: {err}')
                return
            f = {'name': name, 'url': f'{API}/files/{jid}/{name}'}
            if label:
                f['size'] = label
            if row is not None:
                f['row'] = row
            files.append(f)
            job['done_n'] += 1
        url = files[0]['url']
        if zipped:
            with zipfile.ZipFile(os.path.join(d, 'agentfx.zip'), 'w', zipfile.ZIP_DEFLATED) as z:
                for f in files:
                    z.write(os.path.join(d, f['name']), f['name'])
            url = f'{API}/files/{jid}/agentfx.zip'
        job.update(status='done', progress=1.0, files=files, url=url)
    threading.Thread(target=work, daemon=True).start()
    return jid


def ep_render_start(body):
    """A raw `source` (one file) or a `template` with `params` and `sizes` (one file per size)."""
    output = body.get('output')
    tid = body.get('template')
    if tid is not None:
        if body.get('source') is not None:
            raise ApiError(400, 'bad_request', 'send either `source` or `template`, not both')
        m = load_manifest(tid)
        still = m.get('kind') == 'still'
        output = output or ('png' if still else 'mp4')
        check_output(output, still)
        at = str(m.get('still_at') or '0s')
        outs = []
        for name, wh in parse_sizes(m, body.get('sizes')):
            src = instantiate(tid, body.get('params') or {}, name)
            fname = f'{tid}-{name}.{output}' if name else f'{tid}.{output}'
            outs.append((fname, src, at, [] if name else fmt_args(body), parse_size(wh) if wh else None, name, None))
        return {'job_id': start_job(outs, output)}
    if body.get('sizes'):
        raise ApiError(400, 'bad_request', 'sizes need a `template`')
    output = output or 'mp4'
    check_output(output, None)
    source = body.get('source')
    new_job_dir(source)  # validates (size, length)
    at = str(body.get('at') or '0s')
    if not re.fullmatch(r'[\w.:+\-@]+', at):
        raise ApiError(400, 'bad_request', 'bad `at`')
    return {'job_id': start_job([(f'agentfx.{output}', source, at, fmt_args(body), None, None, None)], output)}


def check_output(output, still):
    allowed = STILL_OUT if still else VIDEO_OUT if still is False else VIDEO_OUT + STILL_OUT
    if output not in allowed:
        raise ApiError(400, 'bad_request', f'output must be one of {allowed}')


def ep_batch(body):
    """One output per row of params (x sizes), zipped; each row overrides the shared `params`."""
    tid = body.get('template')
    m = load_manifest(tid)
    rows = body.get('rows')
    if isinstance(rows, str):
        rows = parse_rows(rows, {'id': 'rows'})
        rows = [{k: v.replace('\\"', '"').replace('\\\\', '\\') for k, v in r.items()} for r in rows]
    if not isinstance(rows, list) or not rows or len(rows) > 100 or not all(isinstance(r, dict) for r in rows):
        raise ApiError(400, 'bad_request', 'rows must be a list of 1..100 param objects (or CSV text)')
    still = m.get('kind') == 'still'
    output = body.get('output') or ('png' if still else 'mp4')
    check_output(output, still)
    at = str(m.get('still_at') or '0s')
    shared = body.get('params') or {}
    sizes = parse_sizes(m, body.get('sizes'))
    outs = []
    for i, row in enumerate(rows):
        for name, wh in sizes:
            src = instantiate(tid, {**shared, **row}, name)
            outs.append((f'{tid}-{i + 1:03d}{"-" + name if name else ""}.{output}', src, at, [], parse_size(wh) if wh else None, name, i))
    if len(outs) > 200:
        raise ApiError(413, 'too_large', 'too many outputs (rows x sizes > 200)')
    return {'job_id': start_job(outs, output, zipped=True)}


def ep_render_status(jid):
    job = JOBS.get(jid)
    if not job or job.get('status') == 'preview':
        raise ApiError(404, 'not_found', 'unknown job')
    if job['status'] == 'running':
        el = time.time() - job['started']
        exp = job.get('expected', 5)
        guess = min(0.95, 1 - 0.5 ** (el / exp * 1.6))
        job['progress'] = round(max(guess, job['done_n'] / max(1, job['n']) * 0.95), 3)
    return {k: job[k] for k in ('status', 'progress', 'url', 'files', 'error') if k in job}


CATALOG = None


def ep_catalog():
    global CATALOG
    if CATALOG is None:
        rc, out, err = run_cli(['catalog'], DATA)
        CATALOG = json.loads(out)
    return CATALOG


# ---------------------------------------------------------------- HTTP

class Handler(BaseHTTPRequestHandler):
    server_version = 'agentfx-mock/0.1'

    def log_message(self, fmt, *args):
        if os.environ.get('MOCK_QUIET') != '1':
            sys.stderr.write('mock %s\n' % (fmt % args))

    def cors(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Headers', 'Content-Type, Authorization')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')

    def send(self, status, data, ctype='application/json', extra=None):
        if isinstance(data, (dict, list)):
            data = json.dumps(data).encode('utf-8')
        elif isinstance(data, str):
            data = data.encode('utf-8')
        self.send_response(status)
        self.send_header('Content-Type', ctype)
        self.send_header('Content-Length', str(len(data)))
        self.cors()
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != 'HEAD':
            self.wfile.write(data)

    def body(self):
        n = int(self.headers.get('Content-Length') or 0)
        if n > 2 * 1024 * 1024:
            raise ApiError(413, 'too_large', 'request too large')
        try:
            return json.loads(self.rfile.read(n) or b'{}')
        except ValueError:
            raise ApiError(400, 'bad_json', 'request body is not JSON')

    def do_OPTIONS(self):
        self.send_response(204)
        self.cors()
        self.end_headers()

    def handle_err(self, fn):
        try:
            fn()
        except ApiError as e:
            err = {'code': e.code, 'message': e.message}
            err.update(e.extra)
            self.send(e.status, {'error': err})
        except subprocess.TimeoutExpired:
            self.send(504, {'error': {'code': 'timeout', 'message': 'the CLI timed out'}})
        except BrokenPipeError:
            pass
        except Exception as e:  # noqa: BLE001 - a dev mock: report everything
            self.send(500, {'error': {'code': 'internal', 'message': repr(e)}})

    def do_GET(self):
        self.handle_err(self._get)

    def do_HEAD(self):
        self.handle_err(self._get)

    def do_POST(self):
        self.handle_err(self._post)

    def _get(self):
        u = urlparse(self.path)
        path = u.path
        if not path.startswith(API):
            raise ApiError(404, 'not_found', 'not found')
        p = path[len(API):].strip('/').split('/')
        if p == ['templates']:
            return self.send(200, list_templates())
        if len(p) == 2 and p[0] == 'templates':
            m = with_manifest_extras(load_manifest(p[1]))
            with open(os.path.join(templates_dir(), p[1], 'template.reel'), encoding='utf-8') as f:
                m['source'] = f.read()
            return self.send(200, m)
        if len(p) == 3 and p[0] == 'templates' and p[2] == 'preview':
            return self.template_preview(p[1])
        if p == ['catalog']:
            return self.send(200, ep_catalog(), extra={'Cache-Control': 'max-age=3600'})
        if len(p) == 2 and p[0] == 'renders':
            return self.send(200, ep_render_status(p[1]))
        if len(p) == 2 and p[0] == 'fonts' and p[1] in FONTS:
            return self.send_file(FONTS[p[1]], {'Cache-Control': 'max-age=86400'})
        if len(p) == 3 and p[0] == 'files':
            jid, name = p[1], unquote(p[2])
            if not re.fullmatch(r'[0-9a-f]{16}', jid) or '/' in name or name.startswith('.'):
                raise ApiError(404, 'not_found', 'not found')
            fp = os.path.join(DATA, 'jobs', jid, name)
            if not os.path.isfile(fp):
                raise ApiError(404, 'not_found', 'not found')
            extra = {}
            if 'download' in parse_qs(u.query):
                extra['Content-Disposition'] = f'attachment; filename="{name}"'
            return self.send_file(fp, extra)
        raise ApiError(404, 'not_found', 'not found')

    def send_file(self, fp, extra=None):
        ctype = mimetypes.guess_type(fp)[0] or 'application/octet-stream'
        with open(fp, 'rb') as f:
            data = f.read()
        self.send(200, data, ctype, extra)

    def template_preview(self, tid):
        load_manifest(tid)
        tdir = os.path.join(templates_dir(), tid)
        for name in ('preview.webp', 'preview.gif', 'preview.png'):
            if os.path.isfile(os.path.join(tdir, name)):
                return self.send_file(os.path.join(tdir, name), {'Cache-Control': 'max-age=600'})
        cache = os.path.join(DATA, 'previews', f'{tid}.png')
        if not os.path.isfile(cache):
            m = load_manifest(tid)
            src = instantiate(tid, {})
            at = m.get('still_at') or '%.2fs' % (float(m.get('duration_s') or 4) * 0.45)
            size = next(iter((m.get('sizes') or {}).values()), None) if m.get('kind') == 'still' else None
            png = ep_frame({'source': src, 'at': at, 'width': 480, 'size': size})
            os.makedirs(os.path.dirname(cache), exist_ok=True)
            with open(cache, 'wb') as f:
                f.write(png)
        return self.send_file(cache, {'Cache-Control': 'max-age=600'})

    def _post(self):
        path = urlparse(self.path).path
        p = path[len(API):].strip('/') if path.startswith(API) else None
        if p == 'assets':
            return self.send(200, ep_assets(self))
        body = self.body()
        if p == 'instantiate':
            return self.send(200, {'source': instantiate(body.get('template'), body.get('params') or {}, body.get('size') or None)})
        if p == 'parse':
            return self.send(200, ep_parse(body))
        if p == 'lint':
            return self.send(200, ep_lint(body))
        if p == 'timing':
            return self.send(200, ep_timing(body))
        if p == 'frame':
            return self.send(200, ep_frame(body), 'image/png')
        if p == 'sheet':
            return self.send(200, ep_sheet(body), 'image/png')
        if p == 'preview':
            return self.send(200, ep_preview(body), 'text/html; charset=utf-8')
        if p == 'renders':
            return self.send(202, ep_render_start(body))
        if p == 'batch':
            return self.send(202, ep_batch(body))
        raise ApiError(404, 'not_found', 'not found')


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--port', type=int, default=int(os.environ.get('MOCK_PORT', 8793)))
    ap.add_argument('--host', default='127.0.0.1')
    ap.add_argument('--templates')
    a = ap.parse_args()
    if a.templates:
        os.environ['AGENTFX_TEMPLATES'] = os.path.abspath(a.templates)
    os.makedirs(os.path.join(DATA, 'jobs'), exist_ok=True)
    gc_jobs(0)
    srv = ThreadingHTTPServer((a.host, a.port), Handler)
    srv.daemon_threads = True
    sys.stderr.write(f'agentfx mock API on http://{a.host}:{a.port}{API} (cli {CLI}, templates {templates_dir()})\n')

    def janitor():
        while True:
            time.sleep(300)
            gc_jobs()
    threading.Thread(target=janitor, daemon=True).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == '__main__':
    main()
