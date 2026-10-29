// Renders the fictional UI states of app.html to ../<state>.png (2560x1296, DPR 2) plus
// ../<state>.json hotspots ({"id":[x,y,w,h]} in shot pixels), and picker.html to the
// crop test fixture. Needs Playwright with Chromium:  node examples/assets/ui/src/shoot.mjs
// (set NODE_PATH if playwright is installed elsewhere).
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const require = createRequire(import.meta.url);
const { chromium } = require('playwright');
const here = path.dirname(fileURLToPath(import.meta.url));
const out = path.join(here, '..');
const STATES = ['changes', 'content', 'hot', 'drag', 'wide', 'pub', 'pub-menu', 'pub-menu-staging',
  'pub-staging', 'pub-hover', 'pub-busy', 'pub-sent', 'pg-start', 'pg-diff', 'pg-push', 'pg-confirm', 'pg-done'];
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 648 }, deviceScaleFactor: 2 });
for (const s of STATES) {
  await page.goto('file://' + path.join(here, 'app.html'));
  await page.evaluate(async (s) => { await document.fonts.ready; window.setState(s); }, s);
  const hot = await page.evaluate(() => {
    const o = {};
    for (const e of document.querySelectorAll('[data-hot]')) {
      const r = e.getBoundingClientRect();
      if (r.width < 1 || r.height < 1 || e.closest('.hidden')) continue;
      o[e.dataset.hot] = [r.left, r.top, r.width, r.height].map((v) => Math.round(v * 2));
    }
    return o;
  });
  await page.screenshot({ path: path.join(out, s + '.png') });
  (await import('node:fs')).writeFileSync(path.join(out, s + '.json'), JSON.stringify(hot) + '\n');
}
const p2 = await browser.newPage({ viewport: { width: 1440, height: 810 }, deviceScaleFactor: 2 });
await p2.goto('file://' + path.join(here, 'picker.html'));
await p2.evaluate(() => document.fonts.ready);
await p2.screenshot({ path: path.join(here, '../../../../tests/fixtures/assets/picker.png') });
await browser.close();
