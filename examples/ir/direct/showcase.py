#!/usr/bin/env python3
"""Direct-mode showcase for reel M1 (no HTML, no Chrome): writes
examples/direct/showcase-16x9.ir.json and showcase-9x16.ir.json.

Uses the M1 IR features: per-glyph text tracks with stagger + spring easing,
glow and blur node effects with animated radius, drop shadow, an alpha matte
(hidden text node as matte source), path draw-on (trim-end), motion blur via
sub-frame samples, gradient brushes. 8 s at 30 fps.
"""
import json, math, os, sys

FONT = "../../scenes/_shared/Inter.ttf"
SPRING = {"spring": {"mass": 1, "stiffness": 140, "damping": 13, "velocity": 0}}
SPRING_SOFT = {"spring": {"mass": 1, "stiffness": 90, "damping": 15, "velocity": 0}}
EASE_OUT = {"cubic-bezier": [0.16, 1, 0.3, 1]}
EASE_IO = {"cubic-bezier": [0.65, 0, 0.35, 1]}


def build(W, H):
    nodes, tracks = [], []

    def node(name, content=None, children=None, **kw):
        n = {"name": name, "children": children or []}
        if content is not None:
            n["content"] = content
        n.update(kw)
        nodes.append(n)
        return len(nodes) - 1

    def track(n, prop, delay, dur, keys, easing="linear", fill="both", stagger=None):
        t = {"node": n, "property": prop, "delay": delay, "duration": dur, "fill": fill, "easing": easing,
             "keyframes": [{"offset": o, "value": v} for o, v in keys]}
        if stagger:
            t["stagger"] = stagger
        tracks.append(t)

    tf = lambda *fns: {"transform": list(fns)}
    num = lambda v: {"number": v}
    portrait = H > W
    cx, cy = W / 2, H / 2
    s = min(W, H) / 1080  # layout scale

    root = node("root")
    # background: diagonal gradient (path fill)
    bg = node("background", {"type": "path", "d": f"M0 0 H{W} V{H} H0 Z",
                              "fill": {"linear": {"start": [0, 0], "end": [W, H], "stops": [
                                  {"offset": 0, "color": [0.03, 0.06, 0.09, 1]},
                                  {"offset": 0.55, "color": [0.05, 0.14, 0.12, 1]},
                                  {"offset": 1, "color": [0.07, 0.30, 0.21, 1]}]}}})
    # blurred drifting orbs (blur effect, animated radius)
    orb_r = 260 * s
    orb1 = node("orb green (blur)", {"type": "rect", "rect": {"rect": [cx - orb_r, cy - orb_r, 2 * orb_r, 2 * orb_r], "radius": [orb_r] * 4},
                                      "color": [0.10, 0.60, 0.40, 0.55]},
                origin=[cx, cy], effects=[{"type": "blur", "radius": 90 * s}])
    track(orb1, "transform", 0, 8, [(0, tf({"translate": [-0.32 * W, -0.18 * H]})), (1, tf({"translate": [0.22 * W, -0.05 * H]}))], EASE_IO)
    track(orb1, "opacity", 0, 1.2, [(0, num(0)), (1, num(1))], EASE_OUT)
    orb2 = node("orb blue (blur)", {"type": "rect", "rect": {"rect": [cx - orb_r * 0.7, cy - orb_r * 0.7, 1.4 * orb_r, 1.4 * orb_r], "radius": [orb_r * 0.7] * 4},
                                     "color": [0.15, 0.35, 0.85, 0.45]},
                origin=[cx, cy], effects=[{"type": "blur", "radius": 70 * s}])
    track(orb2, "transform", 0, 8, [(0, tf({"translate": [0.30 * W, 0.25 * H]})), (1, tf({"translate": [-0.10 * W, 0.20 * H]}))], EASE_IO)
    track(orb2, "effects.0.radius", 0, 8, [(0, num(110 * s)), (0.5, num(40 * s)), (1, num(110 * s))], EASE_IO)

    # title: per-glyph spring entrance + glow (animated radius / strength)
    ts = (270 if portrait else 230) * s
    ty = 0.15 * H if portrait else 40 * s
    title = node("title (glyph springs + glow)", {"type": "text", "text": "reel", "font": 0, "size": ts, "weight": 820,
                                                  "letter_spacing": -0.03 * ts, "pos": [0, ty], "max_width": W, "align": "center",
                                                  "brush": {"linear": {"start": [cx - ts, 0], "end": [cx + ts, 0], "stops": [
                                                      {"offset": 0, "color": [0.62, 1.0, 0.82, 1]}, {"offset": 1, "color": [1, 1, 1, 1]}]}}},
                 origin=[cx, ty + ts * 0.6],
                 effects=[{"type": "glow", "radius": 0, "color": [0.35, 1.0, 0.70, 1], "strength": 0.9}])
    track(title, "glyph.transform", 0.5, 1.0, [(0, tf({"translate": [0, 120 * s]}, {"scale": [0.4, 0.4]}, {"rotate": -18})),
                                               (1, tf({"translate": [0, 0]}, {"scale": [1, 1]}, {"rotate": 0}))], SPRING, stagger={"each": 0.09})
    track(title, "glyph.opacity", 0.5, 0.3, [(0, num(0)), (1, num(1))], "linear", stagger={"each": 0.09})
    track(title, "effects.0.radius", 1.3, 2.2, [(0, num(0)), (0.35, num(34 * s)), (1, num(14 * s))], EASE_OUT)
    track(title, "effects.0.strength", 1.3, 2.2, [(0, num(0)), (0.35, num(1.4)), (1, num(0.7))], EASE_OUT)

    # draw-on underline (trim-end), gradient stroke, round caps
    uw = (0.70 if portrait else 0.42) * W
    uy = ty + ts * 1.2
    x0 = cx - uw / 2
    pts = []
    for i in range(0, 61):
        x = x0 + uw * i / 60
        y = uy + math.sin(i / 60 * math.pi * 3) * 14 * s
        pts.append(f"{'M' if i == 0 else 'L'}{x:.2f} {y:.2f}")
    line = node("underline (path draw-on)", {"type": "path", "d": " ".join(pts), "trim": [0, 0],
                                             "stroke": {"width": 12 * s, "cap": "round", "join": "round",
                                                        "brush": {"linear": {"start": [x0, 0], "end": [x0 + uw, 0], "stops": [
                                                            {"offset": 0, "color": [0.23, 0.95, 0.62, 1]}, {"offset": 1, "color": [0.35, 0.65, 1.0, 1]}]}}}})
    track(line, "trim-end", 1.0, 1.4, [(0, num(0)), (1, num(1))], EASE_OUT)
    track(line, "trim-start", 6.6, 0.9, [(0, num(0)), (1, num(1))], EASE_IO, fill="forwards")

    # typewriter subtitle (glyph.opacity steps, staggered)
    ss = 40 * s
    sub_y = uy + 60 * s
    sub_text = "native raster · HTML + direct mode" if not portrait else "native raster\nHTML + direct mode"
    sub = node("subtitle (typewriter)", {"type": "text", "text": sub_text, "font": 0, "size": ss, "weight": 500,
                                        "line_height": 1.35, "pos": [0, sub_y], "max_width": W, "align": "center",
                                        "brush": {"solid": [0.78, 0.92, 0.86, 1]}})
    track(sub, "glyph.opacity", 2.1, 0.001, [(0, num(0)), (1, num(1))], {"steps": [1, "jump-end"]}, stagger={"each": 0.035})

    # card: slides in with a soft spring, de-blurs (effects.0 radius), drop shadow
    cw, ch = (0.80 * W, 300 * s) if portrait else (0.46 * W, 240 * s)
    cy0 = 0.45 * H if portrait else 450 * s
    card = node("card (de-blur + drop shadow)", None, [], origin=[cx, cy0 + ch / 2],
                effects=[{"type": "blur", "radius": 0}, {"type": "drop-shadow", "dx": 0, "dy": 18 * s, "radius": 28 * s, "color": [0, 0, 0, 0.45]}])
    card_bg = node("card bg", {"type": "rect", "rect": {"rect": [cx - cw / 2, cy0, cw, ch], "radius": [28 * s] * 4}, "color": [1, 1, 1, 0.09]})
    card_line = node("card border", {"type": "path", "d": f"M{cx - cw / 2 + 28 * s} {cy0} H{cx + cw / 2 - 28 * s}",
                                     "stroke": {"width": 2, "brush": {"solid": [1, 1, 1, 0.25]}}})
    card_txt = node("card text", {"type": "text", "text": "blur · glow · matte · trim", "font": 0, "size": 46 * s, "weight": 650,
                                  "pos": [0, cy0 + ch * 0.20], "max_width": W, "align": "center", "brush": {"solid": [1, 1, 1, 1]}})
    card_txt2 = node("card text 2", {"type": "text", "text": "motion blur · springs · per-glyph tracks", "font": 0, "size": 30 * s, "weight": 450,
                                     "pos": [0, cy0 + ch * 0.55], "max_width": W, "align": "center", "brush": {"solid": [0.75, 0.85, 0.82, 1]}})
    nodes[card]["children"] = [card_bg, card_line, card_txt, card_txt2]
    track(card, "transform", 2.6, 1.3, [(0, tf({"translate": [0, 160 * s]}, {"scale": [0.92, 0.92]})), (1, tf({"translate": [0, 0]}, {"scale": [1, 1]}))], SPRING_SOFT)
    track(card, "opacity", 2.6, 0.5, [(0, num(0)), (1, num(1))], EASE_OUT)
    track(card, "effects.0.radius", 2.6, 1.0, [(0, num(26 * s)), (1, num(0))], EASE_OUT)

    # matte: a moving gradient band shown only through the (hidden) word "MATTE"
    ms = (170 if portrait else 150) * s
    my = cy0 + ch + (130 if portrait else 30) * s
    matte_src = node("matte source (hidden text)", {"type": "text", "text": "MATTE", "font": 0, "size": ms, "weight": 900,
                                                    "letter_spacing": 0.04 * ms, "pos": [0, my], "max_width": W, "align": "center",
                                                    "brush": {"solid": [1, 1, 1, 1]}}, hidden=True)
    band_w = W * 0.9
    band = node("band", {"type": "path", "d": f"M{cx - band_w} {my - 20} H{cx + band_w} V{my + ms * 1.3} H{cx - band_w} Z",
                         "fill": {"linear": {"start": [cx - band_w, 0], "end": [cx + band_w, 0], "extend": "reflect", "stops": [
                             {"offset": 0, "color": [0.10, 0.45, 0.32, 1]}, {"offset": 0.45, "color": [0.40, 1.0, 0.75, 1]},
                             {"offset": 0.5, "color": [1, 1, 1, 1]}, {"offset": 0.55, "color": [0.40, 0.75, 1.0, 1]},
                             {"offset": 1, "color": [0.12, 0.25, 0.55, 1]}]}}})
    track(band, "transform", 4.0, 3.5, [(0, tf({"translate": [-band_w * 0.55, 0]})), (1, tf({"translate": [band_w * 0.55, 0]}))], EASE_IO)
    matte = node("matte target", None, [band], effects=[{"type": "matte", "source": matte_src, "mode": "alpha"}], origin=[cx, my])
    track(matte, "opacity", 4.0, 0.6, [(0, num(0)), (1, num(1))], EASE_OUT)
    track(matte, "transform", 4.0, 0.9, [(0, tf({"scale": [0.85, 0.85]})), (1, tf({"scale": [1, 1]}))], SPRING)
    # the matte source moves with the target (same transform) so the word scales in
    track(matte_src, "transform", 4.0, 0.9, [(0, tf({"scale": [0.85, 0.85]})), (1, tf({"scale": [1, 1]}))], SPRING)
    nodes[matte_src]["origin"] = [cx, my]

    # motion-blurred comet: fast arc across the frame, rotating, 12 sub-frame samples
    br = 26 * s
    comet = node("comet (motion blur)", None, [], origin=[cx, cy], motion_blur={"samples": 12, "shutter": 1.0})
    ball = node("ball", {"type": "rect", "rect": {"rect": [cx - br, cy - br, 2 * br, 2 * br], "radius": [br] * 4}, "color": [1, 0.85, 0.35, 1]})
    tail = node("ball square", {"type": "rect", "rect": {"rect": [cx + br * 1.5, cy - br * 0.5, br, br], "radius": [3] * 4}, "color": [0.35, 0.95, 0.65, 1]})
    nodes[comet]["children"] = [ball, tail]
    keys = []
    ease_io = lambda x: 4 * x ** 3 if x < 0.5 else 1 - (-2 * x + 2) ** 3 / 2
    for i in range(0, 17):
        u = ease_io(i / 16)
        x = (-0.6 + 1.2 * u) * W
        y = (-0.25 * H) + math.sin(u * math.pi) * (-0.12 * H) + (0.62 * H if portrait else 0.32 * H)
        keys.append((i / 16, tf({"translate": [x, y]}, {"rotate": 540 * u})))
    # positions pre-eased per keyframe, linear segments: smooth velocity (CSS eases each segment)
    track(comet, "transform", 5.2, 1.3, keys, "linear")
    nodes[root]["children"] = [bg, orb1, orb2, matte_src, line, title, sub, card, matte, comet]
    return {"version": 1, "width": W, "height": H, "fps": 30, "frames": 240, "nodes": nodes, "tracks": tracks,
            "assets": [], "fonts": [{"path": FONT}]}


if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    for name, (w, h) in {"16x9": (1920, 1080), "9x16": (1080, 1920)}.items():
        p = os.path.join(here, f"showcase-{name}.ir.json")
        json.dump(build(w, h), open(p, "w"), indent=1)
        print(p)
