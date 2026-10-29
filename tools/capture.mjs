// Chrome bridge for reel: frames, contact sheets, layout measurement, full renders.
// Input: one JSON object on stdin ({cmd, html, w, h, ...}); output: JSON on stdout.
// The page exposes window.__reel.seek(t) (Web Animations seeking), so every frame is a pure function of t.
import { chromium } from 'playwright-core';
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const req = JSON.parse(fs.readFileSync(0, 'utf8'));
const exe = process.env.AGENTFX_CHROME || process.env.REEL_CHROME || [
  `${os.homedir()}/.cache/ms-playwright/chromium_headless_shell-1228/chrome-headless-shell-linux64/chrome-headless-shell`,
].find((p) => fs.existsSync(p));

const browser = await chromium.launch({ executablePath: exe, args: ['--allow-file-access-from-files', '--disable-web-security', '--font-render-hinting=none', '--disable-lcd-text', '--force-color-profile=srgb'] });
async function open(scale = 1) {
  const page = await browser.newPage({ viewport: { width: req.w, height: req.h }, deviceScaleFactor: scale });
  await page.goto('file://' + req.html, { waitUntil: 'load', timeout: 120000 });
  await page.evaluate(() => window.__reel.ready);
  return page;
}
async function shot(page, t, type = 'png') {
  await page.evaluate((t) => window.__reel.seek(t), t);
  return page.screenshot({ type, quality: type === 'jpeg' ? 92 : undefined, clip: { x: 0, y: 0, width: req.w, height: req.h } });
}
const out = {};
try {
  if (req.cmd === 'frames') {
    const page = await open();
    for (let i = 0; i < req.times.length; i++) fs.writeFileSync(req.out[i], await shot(page, req.times[i]));
    out.files = req.out;
  } else if (req.cmd === 'measure') {
    const page = await open();
    Object.assign(out, { samples: await page.evaluate((times) => window.__reel.measureAll(times), req.times) });
  } else if (req.cmd === 'sheet') {
    const page = await open();
    const cells = [];
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'reel-sheet-'));
    for (let i = 0; i < req.times.length; i++) {
      const b = await shot(page, req.times[i], 'jpeg');
      let ref = null;
      if (req.ref) {
        const f = path.join(tmp, `r${i}.jpg`);
        execFileSync(req.ffmpeg || 'ffmpeg', ['-v', 'error', '-y', '-ss', String(req.times[i]), '-i', req.ref, '-frames:v', '1', '-q:v', '3', f]);
        ref = fs.existsSync(f) ? fs.readFileSync(f).toString('base64') : null;
        if (ref) fs.unlinkSync(f);
      }
      cells.push({ img: b.toString('base64'), ref, label: req.labels?.[i] ?? req.times[i].toFixed(2) + 's' });
    }
    fs.rmdirSync(tmp);
    const cols = req.ref ? 2 : req.cols || 4;
    const cw = req.ref ? 760 : Math.round(1900 / cols);
    const ch = Math.round(cw * req.h / req.w);
    const html = `<html><body style="margin:0;background:#111;font:600 15px Inter,sans-serif;color:#ddd">
      <div style="display:grid;grid-template-columns:repeat(${req.ref ? 1 : cols},auto);gap:10px;padding:10px">
      ${cells.map((c) => req.ref
        ? `<div style="display:flex;gap:8px;align-items:flex-start"><div><img src="data:image/jpeg;base64,${c.ref}" style="width:${cw}px;height:${ch}px;display:block;object-fit:contain;background:#000"><div style="padding:3px 0">reference · ${c.label}</div></div><div><img src="data:image/jpeg;base64,${c.img}" style="width:${cw}px;height:${ch}px;display:block"><div style="padding:3px 0">reel · ${c.label}</div></div></div>`
        : `<div><img src="data:image/jpeg;base64,${c.img}" style="width:${cw}px;height:${ch}px;display:block"><div style="padding:3px 0">${c.label}</div></div>`).join('')}
      </div></body></html>`;
    const sp = await browser.newPage({ viewport: { width: req.ref ? 2 * cw + 30 : 1920, height: 600 } });
    await sp.setContent(html);
    await sp.screenshot({ path: req.out, fullPage: true });
    out.file = req.out;
  } else if (req.cmd === 'geom') {
    // Layout geometry for direct mode (reel-ir): every layer's box in scene coordinates with all
    // animation off and all transform channels at identity, plus text metrics. Rasters (raw layers,
    // rich text, and with --compat m0 all text and painted boxes) are cut out IN PLACE: everything
    // else is hidden, page background transparent, 2x device scale.
    const page = await open(2);
    const geom = await page.evaluate(() => {
      const st = document.createElement('style'); st.id = 'geomcss';
      st.textContent = '.L{animation:none!important;--r-x:0px!important;--r-y:0px!important;--r-z:0px!important;--r-px:0!important;--r-py:0!important;--r-scale:1!important;--r-sx:1!important;--r-sy:1!important;--r-rot:0deg!important;--r-rotx:0deg!important;--r-roty:0deg!important;--r-magic:1!important;--r-opacity:1!important;--r-reveal:1!important;--r-line-h:1!important;--r-blur:0px!important;--r-glow:0px!important}'
        + 'html.iso,html.iso body,html.iso #stage{background:transparent!important}html.iso *{visibility:hidden!important}html.iso .iso-t,html.iso .iso-t *{visibility:visible!important}html.iso .iso-box *{visibility:hidden!important}html.iso .grain{display:none!important}';
      document.head.appendChild(st);
      const out = {};
      for (const el of document.querySelectorAll('.L[id]')) {
        const r = el.getBoundingClientRect(); const cs = getComputedStyle(el);
        const p = (k) => parseFloat(cs[k]) || 0;
        const o = { r: [r.left, r.top, r.width, r.height] };
        const hasText = [...el.childNodes].some((n) => (n.nodeType === 3 && n.textContent.trim()) || (n.nodeType === 1 && n.tagName === 'SPAN'));
        if (hasText) {
          o.c = [r.left + p('paddingLeft') + p('borderLeftWidth'), r.top + p('paddingTop') + p('borderTopWidth'), el.clientWidth - p('paddingLeft') - p('paddingRight')];
          o.text = el.innerText; o.fs = p('fontSize'); o.fw = parseFloat(cs.fontWeight) || 400; o.ls = cs.letterSpacing === 'normal' ? 0 : p('letterSpacing');
          o.lh = cs.lineHeight === 'normal' ? 1.21 : p('lineHeight') / p('fontSize'); o.color = cs.color; o.align = cs.textAlign; o.font = cs.fontFamily;
          o.rich = el.querySelector('span') !== null; o.upper = cs.textTransform === 'uppercase';
        }
        out[el.id] = o;
      }
      return out;
    });
    const rasters = {};
    const cut = async (id, box) => {
      const g = geom[id]; if (!g) return;
      const m = box ? 90 : 4;
      const x0 = Math.max(0, Math.floor(g.r[0] - m)), y0 = Math.max(0, Math.floor(g.r[1] - m));
      const x1 = Math.min(req.w, Math.ceil(g.r[0] + g.r[2] + m)), y1 = Math.min(req.h, Math.ceil(g.r[1] + g.r[3] + m));
      if (x1 - x0 < 1 || y1 - y0 < 1) return;
      await page.evaluate(([id, box]) => { document.documentElement.classList.add('iso'); const e = document.getElementById(id); e.classList.add('iso-t'); if (box) e.classList.add('iso-box'); }, [id, box]);
      const file = path.join(req.outdir, `${id}${box ? '.box' : ''}.png`);
      await page.screenshot({ path: file, omitBackground: true, clip: { x: x0, y: y0, width: x1 - x0, height: y1 - y0 } });
      await page.evaluate((id) => { document.documentElement.classList.remove('iso'); const e = document.getElementById(id); e.classList.remove('iso-t', 'iso-box'); }, id);
      rasters[id + (box ? '#box' : '')] = { file, rect: [x0, y0, x1 - x0, y1 - y0] };
    };
    for (const id of req.raster || []) await cut(id, false);
    for (const id of req.raster_box || []) await cut(id, true);
    Object.assign(out, { geom, rasters });
  } else if (req.cmd === 'render') {
    const fps = req.fps, n = Math.round(req.duration * fps), jobs = Math.max(1, Math.min(req.jobs || 4, 8));
    const per = Math.ceil(n / jobs);
    const dir = fs.mkdtempSync(path.join(path.dirname(req.out), '.reel-seg-'));
    const t0 = Date.now();
    const segs = [];
    await Promise.all(Array.from({ length: jobs }, async (_, j) => {
      const a = j * per, b = Math.min(n, a + per);
      if (a >= b) return;
      const seg = path.join(dir, `seg${j}.mp4`);
      segs[j] = seg;
      const ff = spawn(req.ffmpeg || 'ffmpeg', ['-v', 'error', '-y', '-f', 'image2pipe', '-framerate', String(fps), '-c:v', 'mjpeg', '-i', '-',
        '-c:v', 'libx264', '-preset', 'medium', '-crf', '18', '-pix_fmt', 'yuv420p', '-threads', '1', seg], { stdio: ['pipe', 'inherit', 'inherit'] });
      const page = await open();
      for (let f = a; f < b; f++) {
        const buf = await shot(page, f / fps, 'jpeg');
        if (!ff.stdin.write(buf)) await new Promise((r) => ff.stdin.once('drain', r));
      }
      ff.stdin.end();
      await new Promise((r) => ff.on('close', r));
      await page.close();
    }));
    const list = path.join(dir, 'list.txt');
    fs.writeFileSync(list, segs.filter(Boolean).map((s) => `file '${s}'`).join('\n'));
    const args = ['-v', 'error', '-y', '-f', 'concat', '-safe', '0', '-i', list];
    if (req.audio) args.push('-i', req.audio, '-c:a', 'aac', '-b:a', '192k', '-shortest');
    args.push('-c:v', 'copy', '-movflags', '+faststart', req.out);
    execFileSync(req.ffmpeg || 'ffmpeg', args);
    for (const s of segs.filter(Boolean)) fs.unlinkSync(s);
    fs.unlinkSync(list);
    fs.rmdirSync(dir);
    Object.assign(out, { file: req.out, frames: n, jobs, seconds: (Date.now() - t0) / 1000, fps_rendered: n / ((Date.now() - t0) / 1000) });
  }
} finally {
  await browser.close();
}
process.stdout.write(JSON.stringify(out));
