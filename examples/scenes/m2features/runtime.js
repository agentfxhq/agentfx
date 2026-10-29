// Scene runtime for recorded scenes (same contract as reel's scenes/_shared/reel.js):
// window.reel.ready resolves when fonts are loaded; window.reel.seek(t) pauses every
// animation at t seconds. Without ?reel=seek in the URL the scene plays live.
(() => {
  const meta = (n) => document.querySelector(`meta[name="${n}"]`)?.content;
  const duration = parseFloat(meta('reel-duration'));
  const fps = parseFloat(meta('reel-fps') || '30');
  // Chrome cancels CSS animations inside display:none subtrees and creates them afresh
  // (at currentTime 0) once the subtree is shown. So seek until no new animation appears:
  // each pass reveals one more nesting level of visibility ranges.
  function seek(t) {
    const seen = new Set();
    for (let pass = 0; pass < 64; pass++) {
      let fresh = false;
      for (const a of document.getAnimations()) {
        if (!seen.has(a)) { seen.add(a); fresh = true; }
        a.pause();
        a.currentTime = t * 1000;
      }
      if (!fresh) return;
    }
    throw new Error('reel: seek did not converge');
  }
  async function load() {
    await new Promise((r) => (document.readyState === 'loading' ? document.addEventListener('DOMContentLoaded', r) : r()));
    await Promise.all(Array.from(document.images).map((im) => im.decode().catch(() => {})));
    await document.fonts.ready;
    if (new URLSearchParams(location.search).has('reel')) { seek(1 / fps); seek(0); }
  }
  window.reel = { duration, fps, seek, ready: load() };
})();
