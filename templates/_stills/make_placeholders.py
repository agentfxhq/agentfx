#!/usr/bin/env python3
"""Generate the default placeholder images of the still templates (flat illustrations, logo,
product, t-shirt). Dev tool, not used at render time; needs Pillow and numpy.

    python3 templates/_stills/make_placeholders.py            # writes into each template dir

Everything here is drawn from code (no stock photos), so the defaults carry no third-party rights.
"""
from __future__ import annotations

import math
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
FONTS = ROOT.parent / "assets" / "fonts"


def hexc(h: str) -> np.ndarray:
    h = h.lstrip("#")
    return np.array([int(h[i:i + 2], 16) for i in (0, 2, 4)], dtype=np.float32)


def vgrad(w, h, stops):
    """Vertical gradient from [(t, '#hex'), ...]."""
    ts = np.array([s[0] for s in stops])
    cs = np.stack([hexc(s[1]) for s in stops])
    y = np.linspace(0, 1, h)
    col = np.stack([np.interp(y, ts, cs[:, k]) for k in range(3)], axis=1)
    return np.repeat(col[:, None, :], w, axis=1)


def noise1d(n, seed, octaves=5, base=6):
    rng = np.random.default_rng(seed)
    out = np.zeros(n)
    amp, freq = 1.0, base
    for _ in range(octaves):
        pts = rng.uniform(-1, 1, freq + 3)
        x = np.linspace(0, freq, n)
        i = np.floor(x).astype(int)
        f = x - i
        f = f * f * (3 - 2 * f)
        out += amp * (pts[i] * (1 - f) + pts[i + 1] * f)
        amp *= 0.5
        freq *= 2
    return out / 1.9


def ridge(img, w, h, y0, amp, color, seed, octaves=5, base=5, sharp=False):
    """Fill everything below a noisy ridge line with `color` (with a soft vertical shade)."""
    line = y0 + amp * noise1d(w, seed, octaves, base)
    if sharp:
        line = y0 + amp * (np.abs(noise1d(w, seed, octaves, base)) * 2 - 0.6)
    yy = np.arange(h)[:, None]
    mask = (yy >= line[None, :]).astype(np.float32)
    # anti-alias the edge
    edge = np.clip(yy - line[None, :] + 0.5, 0, 1)
    mask = edge
    c = hexc(color)
    shade = np.clip((yy - line[None, :]) / (h * 0.5), 0, 1)[..., None]
    col = c * (1 - 0.18 * shade)
    img[:] = img * (1 - mask[..., None]) + col * mask[..., None]


def disc(img, cx, cy, r, color, glow=0.0, glow_color=None):
    h, w, _ = img.shape
    yy, xx = np.mgrid[0:h, 0:w]
    d = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2)
    if glow:
        g = np.exp(-np.clip(d - r, 0, None) / glow)[..., None] * 0.55
        gc = hexc(glow_color or color)
        img[:] = img * (1 - g) + gc * g
    a = np.clip(r - d + 0.5, 0, 1)[..., None]
    img[:] = img * (1 - a) + hexc(color) * a


def save(img, path, size=None):
    im = Image.fromarray(np.clip(img, 0, 255).astype(np.uint8))
    if size:
        im = im.resize(size, Image.LANCZOS)
    path.parent.mkdir(parents=True, exist_ok=True)
    im = im.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.FLOYDSTEINBERG)
    im.save(path, optimize=True)
    print(path.relative_to(ROOT), path.stat().st_size // 1024, "KB")


SCENES = {
    # name: sky stops, sun, layers (y0, amp, color, seed, sharp)
    "dusk": dict(sky=[(0, "#1B1446"), (0.45, "#6B2E6E"), (0.7, "#E0645A"), (1, "#F6B26B")],
                 sun=(0.68, 0.55, 0.09, "#FFE3A3"), layers=[(0.58, 0.07, "#8A3B6A", 3, True), (0.68, 0.06, "#5A2556", 4, True),
                                                            (0.78, 0.05, "#331843", 5, False), (0.9, 0.03, "#1A0E2B", 6, False)]),
    "alpine": dict(sky=[(0, "#9FD3F2"), (0.6, "#DDEFF7"), (1, "#F4F8FA")], sun=None,
                   layers=[(0.42, 0.16, "#E9EEF3", 11, True), (0.56, 0.1, "#7FA3B8", 12, True), (0.72, 0.05, "#3F6E5A", 13, False),
                           (0.86, 0.04, "#244A3B", 14, False)]),
    "desert": dict(sky=[(0, "#F7C873"), (0.5, "#F9E0B0"), (1, "#FCEBD0")], sun=(0.3, 0.3, 0.08, "#FFF5E1"),
                   layers=[(0.6, 0.04, "#E8A15E", 21, False), (0.7, 0.05, "#D9824A", 22, False), (0.82, 0.04, "#B65F35", 23, False)]),
    "night": dict(sky=[(0, "#060B26"), (0.6, "#16245C"), (1, "#2C4A8A")], sun=(0.75, 0.22, 0.05, "#F2F0E6"),
                  layers=[(0.66, 0.08, "#1C2C5E", 31, True), (0.76, 0.05, "#121E45", 32, False), (0.88, 0.03, "#0A1230", 33, False)],
                  stars=True),
    "forest": dict(sky=[(0, "#CFE7D2"), (0.6, "#EAF4E4"), (1, "#F6F9EE")], sun=(0.62, 0.3, 0.07, "#FFF6D8"),
                   layers=[(0.5, 0.06, "#9CC5A1", 41, False), (0.62, 0.07, "#5E9C72", 42, True), (0.74, 0.05, "#2F6B4A", 43, True),
                           (0.86, 0.04, "#1A4632", 44, False)]),
    "ocean": dict(sky=[(0, "#FF9A8B"), (0.35, "#FFC3A0"), (0.55, "#FFE1C4"), (1, "#FFE1C4")], sun=(0.5, 0.53, 0.1, "#FFF1D6"),
                  layers=[(0.6, 0.004, "#3B7EA1", 51, False), (0.75, 0.006, "#2D6585", 52, False), (0.88, 0.006, "#1F4A66", 53, False)]),
    "lavender": dict(sky=[(0, "#B8A6E8"), (0.6, "#E3D7F4"), (1, "#F6EEF8")], sun=(0.25, 0.42, 0.06, "#FFFFFF"),
                     layers=[(0.6, 0.05, "#9C86D6", 61, False), (0.7, 0.04, "#7A63BF", 62, False), (0.82, 0.03, "#5A449E", 63, False),
                             (0.93, 0.02, "#3D2E73", 64, False)]),
    "canyon": dict(sky=[(0, "#2E5A88"), (0.55, "#7FB0D6"), (1, "#CFE3EE")], sun=None,
                   layers=[(0.45, 0.12, "#C9643A", 71, True), (0.6, 0.1, "#A54B2A", 72, True), (0.76, 0.06, "#7A3420", 73, False),
                           (0.9, 0.03, "#4E2216", 74, False)]),
    "mint": dict(sky=[(0, "#0C2A1F"), (0.55, "#17845A"), (1, "#8FE0BB")], sun=(0.5, 0.62, 0.12, "#E8FFF4"),
                 layers=[(0.66, 0.06, "#10603F", 81, True), (0.78, 0.05, "#0C4A31", 82, False), (0.9, 0.03, "#082E1F", 83, False)]),
}


def landscape(name, w=1200, h=900):
    sc = SCENES[name]
    img = vgrad(w, h, sc["sky"])
    if sc.get("stars"):
        rng = np.random.default_rng(7)
        for _ in range(140):
            x, y = rng.uniform(0, w), rng.uniform(0, h * 0.6)
            r = rng.uniform(0.6, 1.8)
            disc(img, x, y, r, "#FFFFFF")
    if sc["sun"]:
        sx, sy, sr, scol = sc["sun"]
        disc(img, sx * w, sy * h, sr * h, scol, glow=h * 0.06)
    for (y0, amp, col, seed, sharp) in sc["layers"]:
        ridge(img, w, h, y0 * h, amp * h, col, seed, sharp=sharp)
    return img


def logo(path, size=640):
    """A neutral logo mark: rounded gradient tile with a white spark, transparent background."""
    s = size * 4
    im = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    grad = vgrad(s, s, [(0, "#3CBF87"), (1, "#17845A")])
    tile = Image.fromarray(grad.astype(np.uint8)).convert("RGBA")
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, s - 1, s - 1], radius=int(s * 0.24), fill=255)
    im.paste(tile, (0, 0), mask)
    d = ImageDraw.Draw(im)
    c, r1, r2 = s / 2, s * 0.32, s * 0.085
    pts = []
    for k in range(8):
        a = -math.pi / 2 + k * math.pi / 4
        r = r1 if k % 2 == 0 else r2
        pts.append((c + r * math.cos(a), c + r * math.sin(a)))
    d.polygon(pts, fill=(255, 255, 255, 255))
    im = im.resize((size, size), Image.LANCZOS)
    im.save(path, optimize=True)
    print(path.relative_to(ROOT), path.stat().st_size // 1024, "KB")


def product(path, w=900, h=1100):
    """A phone product shot on transparent background (front, slight shadow)."""
    k = 3
    W, H = w * k, h * k
    im = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    # shadow
    sh = Image.new("L", (W, H), 0)
    ImageDraw.Draw(sh).rounded_rectangle([int(W * 0.24), int(H * 0.1), int(W * 0.8), int(H * 0.96)], radius=int(W * 0.09), fill=130)
    sh = sh.filter(ImageFilter.GaussianBlur(W * 0.03))
    im.paste((0, 0, 0, 255), (0, 0), sh)
    body = [int(W * 0.2), int(H * 0.04), int(W * 0.76), int(H * 0.92)]
    d = ImageDraw.Draw(im)
    d.rounded_rectangle(body, radius=int(W * 0.09), fill=(30, 32, 38, 255))
    d.rounded_rectangle([body[0] + 6 * k, body[1] + 6 * k, body[2] - 6 * k, body[3] - 6 * k], radius=int(W * 0.082), fill=(12, 12, 16, 255))
    scr = [body[0] + 16 * k, body[1] + 16 * k, body[2] - 16 * k, body[3] - 16 * k]
    sw, shh = scr[2] - scr[0], scr[3] - scr[1]
    g = vgrad(sw, shh, [(0, "#FF6A3D"), (0.45, "#C63D96"), (1, "#3B2A8F")])
    # soft blobs on the wallpaper
    disc(g, sw * 0.3, shh * 0.35, sw * 0.35, "#FFB45E", glow=sw * 0.2)
    disc(g, sw * 0.75, shh * 0.7, sw * 0.3, "#6A5BFF", glow=sw * 0.2)
    g = np.array(Image.fromarray(np.clip(g, 0, 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(sw * 0.06)))
    wall = Image.fromarray(g).convert("RGBA")
    m = Image.new("L", (sw, shh), 0)
    ImageDraw.Draw(m).rounded_rectangle([0, 0, sw - 1, shh - 1], radius=int(W * 0.068), fill=255)
    im.paste(wall, (scr[0], scr[1]), m)
    # island + clock
    d.rounded_rectangle([int(W * 0.4), scr[1] + 14 * k, int(W * 0.56), scr[1] + 40 * k], radius=13 * k, fill=(0, 0, 0, 255))
    try:
        f = ImageFont.truetype(str(FONTS / "Inter.ttf"), 92 * k)
        f.set_variation_by_axes([600, 14]) if hasattr(f, "set_variation_by_axes") else None
        d.text(((scr[0] + scr[2]) / 2, scr[1] + shh * 0.2), "9:41", font=f, fill=(255, 255, 255, 235), anchor="mm")
    except Exception:
        pass
    # glass highlight
    hl = Image.new("L", (W, H), 0)
    ImageDraw.Draw(hl).polygon([(body[0], body[1]), (int(W * 0.55), body[1]), (body[0], int(H * 0.55))], fill=28)
    hl = hl.filter(ImageFilter.GaussianBlur(W * 0.02))
    clip = Image.new("L", (W, H), 0)
    ImageDraw.Draw(clip).rounded_rectangle(scr, radius=int(W * 0.068), fill=255)
    hl = Image.fromarray((np.array(hl).astype(np.float32) * (np.array(clip) / 255.0)).astype(np.uint8))
    im.paste((255, 255, 255, 255), (0, 0), hl)
    im = im.resize((w, h), Image.LANCZOS)
    im.save(path, optimize=True)
    print(path.relative_to(ROOT), path.stat().st_size // 1024, "KB")


def tshirt(path, w=1400, h=1400, color=(244, 243, 239)):
    """Front view t-shirt on transparent background with soft fabric shading."""
    k = 2
    W, H = w * k, h * k
    def P(x, y):
        return (x * W, y * H)
    outline = [P(0.36, 0.07), P(0.43, 0.06), P(0.47, 0.1), P(0.5, 0.11), P(0.53, 0.1), P(0.57, 0.06), P(0.64, 0.07),
               P(0.83, 0.15), P(0.95, 0.33), P(0.83, 0.41), P(0.77, 0.33), P(0.78, 0.93), P(0.5, 0.95), P(0.22, 0.93),
               P(0.23, 0.33), P(0.17, 0.41), P(0.05, 0.33), P(0.17, 0.15)]
    mask = Image.new("L", (W, H), 0)
    ImageDraw.Draw(mask).polygon(outline, fill=255)
    mask = mask.filter(ImageFilter.GaussianBlur(k * 1.2))
    base = np.zeros((H, W, 3), np.float32) + np.array(color, np.float32)
    # shading: darker at the sides and under the sleeves, folds
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)
    xn, yn = xx / W, yy / H
    shade = 1 - 0.10 * np.clip(np.abs(xn - 0.5) / 0.28, 0, 1) ** 2
    for (fx, fy, fr, amt) in [(0.3, 0.55, 0.12, -0.05), (0.68, 0.62, 0.1, -0.05), (0.5, 0.82, 0.18, -0.03), (0.42, 0.3, 0.1, 0.03)]:
        shade += amt * np.exp(-(((xn - fx) / fr) ** 2 + ((yn - fy) / (fr * 2.2)) ** 2))
    # collar
    col_mask = Image.new("L", (W, H), 0)
    ImageDraw.Draw(col_mask).arc([int(0.43 * W), int(0.02 * H), int(0.57 * W), int(0.17 * H)], 20, 160, fill=255, width=int(0.014 * W))
    col_mask = np.array(col_mask.filter(ImageFilter.GaussianBlur(k * 2))) / 255.0
    shade -= 0.08 * col_mask
    inner = Image.new("L", (W, H), 0)
    ImageDraw.Draw(inner).polygon([P(0.43, 0.06), P(0.47, 0.1), P(0.5, 0.11), P(0.53, 0.1), P(0.57, 0.06), P(0.5, 0.075)], fill=255)
    shade -= 0.25 * np.array(inner.filter(ImageFilter.GaussianBlur(k * 3))) / 255.0
    rgb = np.clip(base * shade[..., None], 0, 255)
    # drop shadow
    shadow = mask.filter(ImageFilter.GaussianBlur(W * 0.02))
    out = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    sh = Image.fromarray((np.array(shadow) * 0.35).astype(np.uint8))
    out.paste((0, 0, 0, 255), (int(W * 0.006), int(H * 0.012)), sh)
    shirt = Image.fromarray(rgb.astype(np.uint8)).convert("RGBA")
    out.paste(shirt, (0, 0), mask)
    out = out.resize((w, h), Image.LANCZOS)
    out.save(path, optimize=True)
    print(path.relative_to(ROOT), path.stat().st_size // 1024, "KB")


def main():
    which = set(sys.argv[1:])

    def want(x):
        return not which or x in which
    if want("collage"):
        for k, name in enumerate(["dusk", "alpine", "desert", "night", "forest", "ocean", "lavender", "canyon", "mint"], 1):
            save(landscape(name), ROOT / "collage" / f"photo-{k}.png", (960, 720))
    if want("flyer"):
        save(landscape("dusk", 1200, 1200), ROOT / "flyer" / "photo.png", (1000, 1000))
    if want("social-set"):
        logo(ROOT / "social-set" / "logo.png")
        logo(ROOT / "banner" / "logo.png")
    if want("youtube-thumbnail"):
        product(ROOT / "youtube-thumbnail" / "subject.png")
    if want("mockup"):
        tshirt(ROOT / "mockup" / "tshirt.png")
        save(landscape("ocean", 1200, 1200), ROOT / "mockup" / "artwork.png", (900, 900))
    if want("certificate"):
        logo(ROOT / "certificate" / "logo.png", 400)


if __name__ == "__main__":
    main()
