"""Browser end-to-end tests of the playground (Python Playwright, one headless Chromium).

    python3 web/dev/mock_api.py &            # or the real service
    AGENTFX_API=http://127.0.0.1:8793 npx vite --port 8794 &
    flock /tmp/claude-1000/agentfx-heavy.lock python3 web/e2e/e2e.py   # on the shared box

BASE (default http://127.0.0.1:8794). Exits non-zero on the first failure.
"""
import os
import re
import sys
import time
import traceback
import urllib.request

from playwright.sync_api import Page, expect, sync_playwright

BASE = os.environ.get('BASE', 'http://127.0.0.1:8794')
SHOTS = os.environ.get('E2E_SHOTS')  # optional dir for failure screenshots
expect.set_options(timeout=30000)


def frame_html(pg: Page) -> str:
    return pg.locator('[data-testid=preview] iframe:not(.hidden)').get_attribute('srcdoc') or ''


def box_aspect(pg: Page) -> float:
    b = pg.locator('[data-testid=preview]').bounding_box()
    return b['width'] / b['height']


def fetch(url: str) -> bytes:
    with urllib.request.urlopen(url if url.startswith('http') else BASE + url, timeout=60) as r:
        return r.read()


def wait_until(fn, timeout=30, msg='condition'):
    t0 = time.time()
    while time.time() - t0 < timeout:
        v = fn()
        if v:
            return v
        time.sleep(0.25)
    raise AssertionError(f'timed out waiting for {msg}')


def test_template_flow(pg: Page):
    pg.goto(BASE + '/play?t=kinetic-title')
    pg.wait_for_selector('[data-testid=preview] iframe:not(.hidden)')
    expect(pg.locator('.tcard[aria-current=true]')).to_contain_text('Kinetic title')
    # change text -> the live preview reloads with it
    inp = pg.locator('[data-param=headline]')
    inp.fill('Hello e2e')
    wait_until(lambda: 'Hello e2e' in frame_html(pg), msg='preview with the new text')
    # switch format -> the stage becomes portrait
    assert box_aspect(pg) > 1.5
    pg.click('[data-format="9:16"]')
    wait_until(lambda: box_aspect(pg) < 0.7, msg='portrait preview')
    wait_until(lambda: 'Hello e2e' in frame_html(pg) and 'width:720px;height:1280px' in frame_html(pg).replace(' ', ''), msg='9:16 html')
    # render -> progress -> download
    pg.click('[data-testid=render]')
    expect(pg.locator('[data-testid=download]')).to_be_visible(timeout=120000)
    href = pg.locator('[data-testid=download]').get_attribute('href')
    data = fetch(href)
    assert len(data) > 5000 and data[4:8] == b'ftyp', f'not an mp4 ({len(data)} bytes)'
    print('  rendered', len(data), 'bytes from', href)


def test_gallery_and_embed(pg: Page):
    pg.goto(BASE + '/play')
    pg.click('[data-template=glitch-text]')
    expect(pg).to_have_url(re.compile(r't=glitch-text'))
    pg.goto(BASE + '/play?t=glitch-text&embed=1')
    pg.wait_for_selector('[data-testid=preview] iframe:not(.hidden)')
    assert pg.locator('.header').count() == 0 and pg.locator('[data-testid=gallery]').count() == 0
    expect(pg.locator('[data-param=text]')).to_have_value('SIGNAL')
    # paused on a native-only effect -> the exact native frame replaces the live preview
    pg.click('[data-testid=play]')
    expect(pg.locator('[data-testid=exact-frame].on')).to_be_visible(timeout=60000)


def test_still_sizes(pg: Page):
    pg.goto(BASE + '/play?t=quote-card&lang=de')
    expect(pg.locator('[data-testid=render]')).to_contain_text('Bilder erstellen')
    expect(pg.locator('[data-testid=sizes] .size-card')).to_have_count(4)
    pg.locator('.size-card[data-size=a4-flyer] .size-meta').click()
    pg.click('[data-output=pdf]')
    pg.click('[data-testid=render]')
    expect(pg.locator('[data-testid=file-list] li')).to_have_count(2, timeout=120000)
    data = fetch(pg.locator('[data-testid=download]').get_attribute('href'))
    assert data[:2] == b'PK', 'expected a zip'
    print('  zip', len(data), 'bytes')


def test_data_grid(pg: Page):
    pg.goto(BASE + '/play?t=bar-chart')
    pg.wait_for_selector('[data-testid=preview] iframe:not(.hidden)')
    grid = pg.locator('[data-testid=data-rows]')
    grid.locator('input').nth(1).fill('77')
    wait_until(lambda: '77' in frame_html(pg), msg='edited value in the chart')
    pg.click('[data-testid=paste-csv]')
    pg.fill('[data-testid=csv-text]', 'label,value\nNorth,5\nSouth,9\nEast,3')
    pg.click('[data-testid=apply-csv]')
    expect(grid.locator('.dg-cell')).to_have_count(6)
    wait_until(lambda: 'South' in frame_html(pg), msg='pasted rows in the chart')


def test_studio(pg: Page):
    pg.goto(BASE + '/studio')
    pg.wait_for_selector('[data-testid=key]')
    pg.wait_for_selector('[data-testid=preview] iframe:not(.hidden)')
    code = lambda: pg.locator('.cm-content').inner_text()
    assert 'anim #bar sx 0.6s:0 1.4s:1~out-back' in code()
    # drag the bar's second keyframe 30 px to the right
    key = pg.locator('[data-key="bar.sx@1.4"]')
    b = key.bounding_box()
    x, y = b['x'] + b['width'] / 2, b['y'] + b['height'] / 2
    pg.mouse.move(x, y)
    pg.mouse.down()
    pg.mouse.move(x + 15, y, steps=4)
    pg.mouse.move(x + 30, y, steps=4)
    pg.mouse.up()
    new = wait_until(lambda: re.search(r'anim #bar sx 0\.6s:0 ([\d.]+)s:1~out-back', code()) and code(), msg='code after drag')
    t = float(re.search(r'anim #bar sx 0\.6s:0 ([\d.]+)s:1~out-back', new).group(1))
    assert t > 1.45, f'key did not move ({t})'
    assert '// Welcome to AgentFX Studio' in new, 'comments must survive a timeline edit'
    print('  key moved to', t)
    # undo restores the text
    pg.click('[data-testid=undo]')
    wait_until(lambda: 'anim #bar sx 0.6s:0 1.4s:1~out-back' in code(), msg='undo')
    # select a key, change its easing in the inspector
    pg.locator('[data-key="bar.sx@1.4"]').click()
    pg.locator('[data-testid=ease-grid] button', has_text='out-expo').first.click()
    wait_until(lambda: '1.4s:1~out-expo' in code(), msg='easing edit')
    # a lint error shows inline and in the list, with a quick fix
    pg.locator('.cm-content').click()
    pg.keyboard.press('Control+End')
    pg.keyboard.type('\n  @0.5s pulse #dott')
    expect(pg.locator('[data-testid=error-count]')).to_be_visible()
    expect(pg.locator('.cm-lintRange-error').first).to_be_visible()
    expect(pg.locator('[data-testid=diag]').first).to_contain_text('#dott')
    pg.locator('[data-testid=quick-fix]').first.click()
    wait_until(lambda: '@0.5s pulse #dot\n' in code() + '\n', msg='quick fix applied')
    expect(pg.locator('[data-testid=error-count]')).to_have_count(0)


TESTS = [test_template_flow, test_gallery_and_embed, test_still_sizes, test_data_grid, test_studio]


def main():
    only = sys.argv[1] if len(sys.argv) > 1 else None
    failed = 0
    with sync_playwright() as p:
        b = p.chromium.launch()
        try:
            for fn in TESTS:
                if only and only not in fn.__name__:
                    continue
                ctx = b.new_context(viewport={'width': 1440, 'height': 900})
                pg = ctx.new_page()
                errors = []
                pg.on('pageerror', lambda e: errors.append(str(e)))
                t0 = time.time()
                try:
                    fn(pg)
                    assert not errors, f'page errors: {errors}'
                    print(f'PASS {fn.__name__} ({time.time() - t0:.1f}s)')
                except Exception:
                    failed += 1
                    print(f'FAIL {fn.__name__}')
                    traceback.print_exc()
                    if SHOTS:
                        pg.screenshot(path=os.path.join(SHOTS, fn.__name__ + '.png'))
                finally:
                    ctx.close()
        finally:
            b.close()
    print('failed:', failed)
    sys.exit(1 if failed else 0)


if __name__ == '__main__':
    main()
