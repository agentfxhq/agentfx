// reel scene runtime: makes a scene a pure function of time.
// - Waits for fonts, images and tracks.json before frame 0 (window.reel.ready).
// - window.reel.seek(t): pauses every CSS animation at t seconds and applies tracks.json.
// - Without ?reel=seek in the URL the scene plays live (CSS animations run on their
//   own, tracks are driven by requestAnimationFrame), so it can be previewed in a browser.
(() => {
  const meta = (n) => document.querySelector(`meta[name="${n}"]`)?.content;
  const duration = parseFloat(meta('reel-duration'));
  const fps = parseFloat(meta('reel-fps') || '30');
  const tracksUrl = meta('reel-tracks');
  let tracks = [];

  // Segment easing for tracks.json keys ("ease" describes the segment arriving at a key).
  const bez = (x1, y1, x2, y2) => (x) => {
    // Newton + bisection, same scheme as Chrome's gfx::CubicBezier.
    const cx = 3 * x1, bx = 3 * (x2 - x1) - cx, ax = 1 - cx - bx;
    const cy = 3 * y1, by = 3 * (y2 - y1) - cy, ay = 1 - cy - by;
    const sx = (t) => ((ax * t + bx) * t + cx) * t;
    const sy = (t) => ((ay * t + by) * t + cy) * t;
    const dx = (t) => (3 * ax * t + 2 * bx) * t + cx;
    let t = x;
    for (let i = 0; i < 4; i++) {
      const e = sx(t) - x;
      if (Math.abs(e) < 1e-7) return sy(t);
      const d = dx(t);
      if (Math.abs(d) < 1e-7) break;
      t -= e / d;
    }
    let lo = 0, hi = 1; t = x;
    while (lo < hi) {
      const v = sx(t);
      if (Math.abs(v - x) < 1e-7) break;
      if (x > v) lo = t; else hi = t;
      t = (lo + hi) / 2;
      if (hi - lo < 1e-12) break;
    }
    return sy(t);
  };
  const EASE = {
    linear: (x) => x,
    ease: bez(0.25, 0.1, 0.25, 1),
    'ease-in': bez(0.42, 0, 1, 1),
    'ease-out': bez(0, 0, 0.58, 1),
    'ease-in-out': bez(0.42, 0, 0.58, 1),
  };
  const easeFn = (s) => {
    if (!s || EASE[s]) return EASE[s || 'linear'];
    let m = /^cubic-bezier\(([^)]*)\)$/.exec(s);
    if (m) { const [a, b, c, d] = m[1].split(',').map(Number); return bez(a, b, c, d); }
    m = /^steps\((\d+)\)$/.exec(s);
    if (m) { const n = +m[1]; return (x) => Math.floor(x * n) / n; }
    throw new Error(`reel: unsupported track ease ${s}`);
  };

  function trackValue(tr, t) {
    const k = tr.keys;
    if (t <= k[0].t) return k[0].v;
    if (t >= k[k.length - 1].t) return k[k.length - 1].v;
    for (let i = 1; i < k.length; i++) {
      if (t < k[i].t) {
        const p = easeFn(k[i].ease)((t - k[i - 1].t) / (k[i].t - k[i - 1].t));
        return k[i - 1].v + (k[i].v - k[i - 1].v) * p;
      }
    }
    return k[k.length - 1].v;
  }

  function applyTracks(t) {
    for (const tr of tracks) {
      const n = Math.floor(trackValue(tr, t) + 1e-9);
      const s = Array.from(tr.text).slice(0, Math.max(0, n)).join('');
      if (tr.el.textContent !== s) tr.el.textContent = s;
    }
  }

  function seek(t) {
    for (const a of document.getAnimations()) {
      a.pause();
      a.currentTime = t * 1000;
    }
    applyTracks(t);
  }

  async function load() {
    if (tracksUrl) {
      const j = await (await fetch(tracksUrl)).json();
      tracks = j.tracks.map((tr) => {
        if (tr.property !== 'text') throw new Error(`reel: unsupported track property ${tr.property}`);
        const els = document.querySelectorAll(tr.selector);
        if (els.length !== 1) throw new Error(`reel: track selector ${tr.selector} matches ${els.length}`);
        return { ...tr, el: els[0] };
      });
    }
    await Promise.all(Array.from(document.images).map((im) => im.decode().catch(() => {
      throw new Error(`reel: image failed ${im.src}`);
    })));
    await document.fonts.ready;
    const live = !new URLSearchParams(location.search).has('reel');
    if (live) {
      const t0 = performance.now();
      const tick = () => {
        const t = Math.min((performance.now() - t0) / 1000, duration);
        applyTracks(t);
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    } else {
      seek(0);
    }
  }

  window.reel = { duration, fps, seek, ready: load() };
})();
