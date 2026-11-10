"""Screenshots of the playground for design review: python3 web/e2e/shots.py [out_dir] [--only name]

Needs the dev server (BASE, default http://127.0.0.1:8794) and an API behind it.
"""
import os
import sys

from playwright.sync_api import sync_playwright

BASE = os.environ.get('BASE', 'http://127.0.0.1:8794')
OUT = sys.argv[1] if len(sys.argv) > 1 and not sys.argv[1].startswith('--') else os.path.join(os.path.dirname(__file__), '..', 'docs', 'screenshots')
ONLY = sys.argv[sys.argv.index('--only') + 1] if '--only' in sys.argv else None
os.makedirs(OUT, exist_ok=True)

SHOTS = [
    # name, path, viewport, theme, action
    ('templates-dark', '/play?t=kinetic-title', (1440, 900), 'dark', 'pause'),
    ('templates-glitch-exact', '/play?t=glitch-text', (1440, 900), 'dark', 'pause'),
    ('templates-light', '/play?t=lyric-lines', (1440, 900), 'light', 'pause'),
    ('templates-tablet', '/play?t=music-visualizer', (820, 1180), 'dark', 'pause'),
    ('embed', '/play?t=glitch-text&embed=1', (1040, 600), 'dark', 'pause'),
    ('still-sizes', '/play?t=quote-card', (1440, 900), 'dark', 'still'),
    ('data-grid-de', '/play?t=bar-chart&lang=de', (1440, 900), 'light', 'pause'),
    ('embed-de-still', '/embed/quote-card?lang=de', (1040, 640), 'light', 'still'),
    ('studio-dark', '/studio', (1440, 900), 'dark', 'select-key'),
    ('studio-light', '/studio', (1440, 900), 'light', 'select-row'),
    ('studio-tablet', '/studio', (1024, 1366), 'dark', None),
]


def run():
    with sync_playwright() as p:
        b = p.chromium.launch()
        for name, path, (w, h), theme, action in SHOTS:
            if ONLY and ONLY not in name:
                continue
            ctx = b.new_context(viewport={'width': w, 'height': h}, device_scale_factor=1)
            ctx.add_init_script(f"localStorage.setItem('afx:theme', '{theme}')")
            pg = ctx.new_page()
            pg.on('console', lambda m: m.type == 'error' and print('  console:', m.text))
            pg.goto(BASE + path)
            pg.wait_for_selector('[data-testid=preview] iframe:not(.hidden)', timeout=30000)
            pg.wait_for_timeout(600)
            if action == 'pause':
                pg.click('[data-testid=play]')
                pg.wait_for_timeout(200)
                box = pg.locator('.scrub').bounding_box()
                pg.mouse.click(box['x'] + box['width'] * 0.42, box['y'] + box['height'] / 2)
                pg.wait_for_timeout(5000)
            elif action == 'still':
                pg.wait_for_selector('[data-testid=exact-frame]', timeout=30000)
                pg.locator('.size-card[data-size=story] .size-thumb').click()
                pg.wait_for_timeout(6000)
            elif action in ('select-key', 'select-row'):
                pg.wait_for_selector('[data-testid=key]')
                pg.wait_for_timeout(400)
                if action == 'select-key':
                    pg.locator('[data-testid=key]').nth(4).click()
                else:
                    pg.locator('[data-testid=row-label]').nth(1).click()
                box = pg.locator('[data-testid=ruler]').bounding_box()
                pg.mouse.click(box['x'] + 190, box['y'] + 10)
                pg.wait_for_timeout(1200)
            else:
                pg.wait_for_timeout(1500)
            pg.screenshot(path=os.path.join(OUT, f'{name}.png'))
            print('wrote', name)
            ctx.close()
        b.close()


if __name__ == '__main__':
    run()
