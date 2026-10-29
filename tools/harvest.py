#!/usr/bin/env python3
"""agentfx design harvest (aggregation): computed-style records of style frames -> a design spec (design.reel).

Input: the JSON written by `agentfx design extract` (Blitz/Stylo computed styles per element).
Output: `design "Name" source=harvest` with colors (roles), bg, type (families, base, ratio, sizes,
weights, tracking, case, line heights), space (unit, scale), layout (radius, margin), surface,
elevation, texture, and an evidence comment. Never invents values: a line without evidence is omitted.

usage: harvest.py <extract.json> [-o design.reel] [--name N] [--json stats.json]
"""
import colorsys, json, math, sys
from collections import Counter, defaultdict
from pathlib import Path


def hexc(c):
    r, g, b, a = c
    s = "#%02x%02x%02x" % (round(r * 255), round(g * 255), round(b * 255))
    return s if a >= 0.999 else s + "%02x" % round(a * 255)


def lum(c):
    def ch(v):
        return v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4
    return 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])


def contrast(a, b):
    la, lb = lum(a), lum(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


def lab(c):
    # sRGB -> CIE Lab (D65)
    def lin(v):
        return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4
    r, g, b = (lin(v) for v in c[:3])
    x = (r * 0.4124 + g * 0.3576 + b * 0.1805) / 0.95047
    y = r * 0.2126 + g * 0.7152 + b * 0.0722
    z = (r * 0.0193 + g * 0.1192 + b * 0.9505) / 1.08883
    f = lambda t: t ** (1 / 3) if t > 0.008856 else 7.787 * t + 16 / 116
    return (116 * f(y) - 16, 500 * (f(x) - f(y)), 200 * (f(y) - f(z)))


def de(a, b):
    return math.dist(lab(a), lab(b))


def over(c, under):
    a = c[3]
    return [c[i] * a + under[i] * (1 - a) for i in range(3)] + [1.0]


def sat(c):
    h, l, s = colorsys.rgb_to_hls(*c[:3])
    return s * (1 - abs(2 * l - 1)), h * 360, l


class Clusters:
    """merge near-identical colors (dE < 6), keep weight and evidence"""
    def __init__(self):
        self.items = []  # [color, weight, count]

    def add(self, c, w):
        for it in self.items:
            if de(it[0], c) < 6 and abs(it[0][3] - c[3]) < 0.15:
                it[1] += w
                it[2] += 1
                return
        self.items.append([list(c), w, 1])

    def ranked(self):
        return sorted(self.items, key=lambda it: -it[1])


def main():
    a = sys.argv[1:]

    def opt(k, d=None):
        if k in a:
            i = a.index(k)
            v = a[i + 1]
            del a[i:i + 2]
            return v
        return d

    out, name, jout = opt("-o"), opt("--name", "Harvested"), opt("--json")
    data = json.load(open(a[0]))
    frames = data["frames"]
    W = frames[0]["w"] if frames else 1920
    H = frames[0]["h"] if frames else 1080
    els = [e for f in frames for e in f["elements"]]
    # effective (opaque) background under each element: nearest ancestor-ish = largest earlier box containing it
    bgs, text, borders, surf = Clusters(), Clusters(), Clusters(), Clusters()
    grads = []
    for f in frames:
        stack = []  # (x, y, w, h, color)
        page_bg = [1, 1, 1, 1]
        for e in f["elements"]:
            area = e["w"] * e["h"]
            if e["tag"] in ("html", "body") and e["bg"][3] > 0.5:
                page_bg = e["bg"]
            # find the background behind this element
            under = page_bg
            for (x, y, w, h, c) in stack:
                if x <= e["x"] + 1 and y <= e["y"] + 1 and x + w >= e["x"] + e["w"] - 1 and y + h >= e["y"] + e["h"] - 1:
                    under = c
            if e["bg"][3] > 0.02 and area > 0:
                eff = over(e["bg"], under)
                full = area >= 0.6 * W * H
                (bgs if full else surf).add(e["bg"] if not full else eff, area)
                stack.append((e["x"], e["y"], e["w"], e["h"], eff))
            for g in e["grads"]:
                if g["stops"]:
                    grads.append((area, g, area >= 0.6 * W * H))
                    if area >= 0.6 * W * H:
                        for s in g["stops"]:
                            bgs.add(over(s["c"], under), area / len(g["stops"]))
            if e["chars"] > 0 and e["color"][3] > 0.3:
                text.add(e["color"], e["chars"])
                e["_under"] = under
            if max(e["border"]) >= 1 and e["border_color"][3] > 0.05:
                borders.add(e["border_color"], max(e["border"]) * (e["w"] + e["h"]) * 2)
    stats = {"frames": [f["file"] for f in frames], "elements": len(els)}
    roles = {}
    bg_r = bgs.ranked()
    bg = bg_r[0][0] if bg_r else [1, 1, 1, 1]
    roles["bg"] = bg
    if len(bg_r) > 1 and de(bg_r[1][0], bg) > 3:
        roles["bg2"] = bg_r[1][0]
    surf_r = [s for s in surf.ranked() if de(s[0], bg) > 2 and sat(s[0])[0] < 0.35]
    if surf_r:
        roles["surface"] = surf_r[0][0]
        if len(surf_r) > 1:
            roles["surface2"] = surf_r[1][0]
    br = borders.ranked()
    if br:
        roles["border"] = br[0][0]
    tx = text.ranked()
    greys = [t for t in tx if sat(t[0])[0] < 0.2]
    if greys:
        roles["text"] = max(greys[:3], key=lambda t: (t[1] > 0.3 * greys[0][1]) * contrast(t[0], bg))[0] if len(greys) > 1 else greys[0][0]
        rest = [t for t in greys if de(t[0], roles["text"]) > 6]
        rest.sort(key=lambda t: -t[1])
        if rest:
            roles["muted"] = rest[0][0]
            low = [t for t in rest[1:] if 1.5 < contrast(t[0], bg) < contrast(roles["muted"], bg)]
            if low:
                roles["faint"] = low[0][0]
    elif tx:
        roles["text"] = tx[0][0]
    # accents: saturated colors anywhere (text, small fills, gradient stops), weighted by saturation
    acc = Clusters()
    for t in tx:
        s, h, l = sat(t[0])
        if s > 0.15:
            acc.add(t[0], t[1] * 50 * s)
    for s_ in surf.ranked():
        s, h, l = sat(s_[0])
        if s > 0.15:
            acc.add(s_[0][:3] + [1.0], math.sqrt(s_[1]) * s)
    for area, g, full in grads:
        if not full:
            for st in g["stops"]:
                s, h, l = sat(st["c"])
                if s > 0.15:
                    acc.add(st["c"][:3] + [1.0], math.sqrt(area) * s / 2)
    ar = acc.ranked()
    if ar:
        roles["accent"] = ar[0][0]
        h0 = sat(ar[0][0])[1]
        nxt = [x for x in ar[1:] if min(abs(sat(x[0])[1] - h0), 360 - abs(sat(x[0])[1] - h0)) >= 25]
        roles["accent2"] = nxt[0][0] if nxt else ar[0][0]
        dark = lum(bg) < 0.2
        # the highlight-text token: the accent that reads best on the ground
        cands = [roles["accent"], roles["accent2"]]
        roles["mint"] = max(cands, key=lambda c: contrast(c, bg))
    allc = [c for c in [roles.get(k) for k in roles] if c]
    roles["paper"] = max(allc, key=lum)
    roles["ink"] = min(allc, key=lum)
    stats["colors"] = {k: hexc(v) for k, v in roles.items()}
    stats["clusters"] = {"bg": [(hexc(c), round(w), n) for c, w, n in bg_r[:4]], "text": [(hexc(c), round(w), n) for c, w, n in tx[:6]], "accent": [(hexc(c), round(w, 1), n) for c, w, n in ar[:5]]}
    # ---------------------------------------------------------------- type
    tels = [e for e in els if e["chars"] > 0]
    fam_big, fam_body, mono = Counter(), Counter(), None
    for e in tels:
        if e["size"] >= 64:
            fam_big[e["family"]] += e["chars"] * e["size"]
        if e["size"] < 48:
            fam_body[e["family"]] += e["chars"]
        if "mono" in e["family"].lower():
            mono = e["family"]
    typ = {}
    if fam_big:
        typ["display"] = fam_big.most_common(1)[0][0]
    if fam_body:
        typ["body"] = [f for f, _ in fam_body.most_common() if f != mono][0] if any(f != mono for f in fam_body) else fam_body.most_common(1)[0][0]
    if mono:
        typ["mono"] = mono
    sizes = Counter()
    for e in tels:
        sizes[round(e["size"])] += e["chars"]
    body_sizes = Counter()
    for e in tels:
        if not e["upper"] and e["ls_em"] < 0.03 and e["weight"] < 600:
            body_sizes[round(e["size"])] += e["chars"]
    body_cands = {s: c for s, c in body_sizes.items() if 24 <= s <= 48} or {s: c for s, c in sizes.items() if 24 <= s <= 48}
    base = max(body_cands, key=body_cands.get) if body_cands else None
    distinct = sorted(sizes)
    ratio, err = None, None
    if base and len(distinct) >= 3:
        # steps above the base: distinct sizes merged within 12 %, weighted (>= 2 % of the characters or a display size)
        tot = sum(sizes.values())
        above = sorted(s for s in distinct if s > base * 1.08 and (sizes[s] >= 0.02 * tot or s >= 64))
        steps = []
        for s_ in above:
            if not steps or s_ > steps[-1] * 1.12:
                steps.append(s_)
        if steps:
            top = steps[-1]
            n = len(steps)
            ratio = round(min(1.7, max(1.1, (top / base) ** (1 / n))), 3)
            err = math.sqrt(sum((math.log(s_) - math.log(base * ratio ** (i + 1))) ** 2 for i, s_ in enumerate(steps)) / n)
    if base:
        typ["base"] = base
    if ratio:
        typ["ratio"] = ratio
        near = lambda target: min(distinct, key=lambda s: abs(math.log(s / target)))
        for k_, p in [("display", 4), ("h1", 3), ("h2", 2), ("h3", 1), ("small", -1)]:
            v = near(base * ratio ** p)
            if abs(math.log(v / (base * ratio ** p))) < 0.12:
                typ[k_ + "_px"] = v
    labels = [e for e in tels if e["upper"] or e["ls_em"] > 0.03]
    if labels:
        ls = Counter(round(e["size"]) for e in labels)
        typ["label_px"] = ls.most_common(1)[0][0]
        typ["label-case"] = "upper" if sum(e["upper"] for e in labels) * 2 >= len(labels) else "none"
        typ["label-tracking"] = round(sorted(e["ls_em"] for e in labels)[len(labels) // 2], 3)
        typ["label-weight"] = round(sorted(e["weight"] for e in labels)[len(labels) // 2])
    disp = [e for e in tels if e["size"] >= 64]
    if disp:
        typ["display-weight"] = round(Counter(e["weight"] for e in disp).most_common(1)[0][0])
        typ["display-tracking"] = round(sorted(e["ls_em"] for e in disp)[len(disp) // 2], 3)
        lhs = [e["lh"] for e in disp if e["lh"]]
        if lhs:
            typ["lh-display"] = round(sorted(lhs)[len(lhs) // 2], 2)
    heads = [e for e in tels if 40 <= e["size"] < 64 and e["weight"] >= 550]
    if heads:
        typ["heading-weight"] = round(Counter(e["weight"] for e in heads).most_common(1)[0][0])
    bodies = [e for e in tels if base and abs(e["size"] - base) <= 2]
    if bodies:
        typ["body-weight"] = round(Counter(e["weight"] for e in bodies).most_common(1)[0][0])
        lhs = [e["lh"] for e in bodies if e["lh"]]
        if lhs:
            typ["lh-body"] = round(sorted(lhs)[len(lhs) // 2], 2)
    stats["type"] = {**typ, "ratio_fit_error": round(err, 4) if err is not None else None, "sizes": dict(sorted(sizes.items()))}
    # ---------------------------------------------------------------- space
    vals = []
    for e in els:
        vals += [v for v in e["pad"] + e["gap"] if v and v > 0.5]
        vals += [v for v in e["margin"] if v and v > 0.5]
    space = {}
    if vals:
        r = [round(v) for v in vals]
        space["unit"] = 8 if sum(1 for v in r if v % 8 == 0) >= 0.6 * len(r) else 4
        space["on_grid_pct"] = round(100 * sum(1 for v in r if v % space["unit"] == 0) / len(r))
        space["scale"] = sorted(v for v, _ in Counter(r).most_common(7))
    content = [e for e in els if e["chars"] > 0 or (e["bg"][3] > 0.05 and e["w"] * e["h"] < 0.6 * W * H)]
    if content:
        left = min(e["x"] for e in content if e["x"] >= 0)
        space["margin_pct"] = round(left / W * 100, 1)
    stats["space"] = space
    # ---------------------------------------------------------------- shape, surface, elevation, texture
    surfaces = [e for e in els if e["bg"][3] > 0.02 and 0.002 * W * H < e["w"] * e["h"] < 0.6 * W * H]
    framed = [e for e in els if max(e["border"]) >= 1 and 0.002 * W * H < e["w"] * e["h"] < 0.6 * W * H]
    shp = {}
    rads = Counter(round(max(e["radius"])) for e in surfaces + framed)
    if rads:
        big = [(r, c) for r, c in rads.most_common() if r < 500]
        shp["radius"] = big[0][0] if big else 999
        if any(r >= 500 for r in rads):
            shp["radius_pill"] = True
        small = [r for r, c in big if r < shp["radius"]]
        if small:
            shp["radius_sm"] = small[0]
    shadowed = [e for e in els if any(not s["inset"] for s in e["shadows"])]
    sh_r = Counter(", ".join(f"{s['x']:g} {s['y']:g} {s['blur']:g} {hexc(s['c'])}" for s in e["shadows"] if not s["inset"]) for e in shadowed)
    n_sh = len(set(id(e) for e in shadowed))
    n_border = len(framed)
    translucent = [e for e in surfaces if e["bg"][3] < 0.5]
    tinted = [e for e in surfaces if sat(e["bg"])[0] > 0.3 and e["bg"][3] < 0.5]
    n_s = max(1, len(surfaces) + len([e for e in framed if e["bg"][3] <= 0.02]))
    if n_sh >= 0.3 * n_s:
        style = "elevated"
    elif len(tinted) >= 0.4 * n_s:
        style = "tinted"
    elif len(translucent) >= 0.4 * n_s and lum(bg) < 0.2:
        style = "glass"
    elif n_border >= 0.5 * n_s and n_sh == 0:
        style = "outline"
    else:
        style = "flat"
    shp["surface"] = style
    bw = Counter(max(e["border"]) for e in framed)
    if bw:
        shp["border"] = bw.most_common(1)[0][0]
    if sh_r:
        shp["shadow"] = sh_r.most_common(1)[0][0]
    full_grads = [g for area, g, full in grads if full]
    tex = {"bg": full_grads[0]["kind"] if full_grads else "solid"}
    stats["shape"] = shp
    stats["texture"] = tex
    # ---------------------------------------------------------------- write
    L = [f"// harvested from: {', '.join(Path(f).name for f in stats['frames'])} ({len(frames)} frame{'s' if len(frames) != 1 else ''}) by agentfx design harvest",
         f'design "{name}" source=harvest base={"dark" if lum(bg) < 0.2 else "light"}']
    order = ["bg", "bg2", "surface", "surface2", "border", "text", "muted", "faint", "mint", "accent", "accent2", "paper", "ink"]
    L.append("  colors " + " ".join(f"{k}={hexc(roles[k])}" for k in order if k in roles))
    if full_grads:
        g = full_grads[0]
        stops = ", ".join(f"{hexc(s['c'])} {round(s['p'] * 100) if s['p'] is not None else round(i / max(1, len(g['stops']) - 1) * 100)}%" for i, s in enumerate(g["stops"]))
        L.append(f"  bg linear({round(g['angle'] or 180)}deg, {stops})" if g["kind"] == "linear" else f"  bg radial(at 50% 40%, {stops})")
    else:
        L.append(f"  bg {hexc(bg)}")
    fparts = [f'{k}="{typ[k]}"' for k in ["display", "body", "mono"] if k in typ]
    if fparts:
        # fonts on their own line: `display=`/`body=` are also size keys (one line may not repeat a key)
        L.append("  type " + " ".join(fparts))
    tparts = []
    for k in ["base", "ratio"]:
        if k in typ:
            tparts.append(f"{k}={typ[k]:g}")
    for k in ["display", "h1", "h2", "h3", "small"]:
        if k + "_px" in typ:
            tparts.append(f"{k}={typ[k + '_px']}")
    if base:
        tparts.append(f"body={base}")
    if "label_px" in typ:
        tparts.append(f"label={typ['label_px']}")
    for k in ["display-weight", "heading-weight", "body-weight", "label-weight", "display-tracking", "label-tracking", "label-case", "lh-display", "lh-body"]:
        if k in typ:
            tparts.append(f"{k}={typ[k]}")
    if tparts:
        L.append("  type " + " ".join(tparts))
    if space.get("scale"):
        L.append(f"  space unit={space['unit']} scale=[{', '.join(str(v) for v in space['scale'])}]")
    lay = []
    if "radius" in shp:
        lay.append(f"radius={shp['radius']}")
    if "radius_sm" in shp:
        lay.append(f"radius-sm={shp['radius_sm']}")
    if "margin_pct" in space and 2 <= space["margin_pct"] <= 15:
        lay.append(f"safe={space['margin_pct']:g}%")
    if lay:
        L.append("  layout " + " ".join(lay))
    L.append(f"  surface style={shp['surface']}" + (f" border={shp['border']:g}" if "border" in shp else ""))
    L.append(f'  elevation shadow="{shp["shadow"]}"' if "shadow" in shp else "  elevation shadow=none")
    L.append(f"  texture bg={tex['bg']}")
    ev = f"  // evidence: text {stats['colors'].get('text')} on {sum(e['chars'] for e in tels)} chars; accent {stats['colors'].get('accent', '-')}; "
    ev += f"type ratio fit error {stats['type']['ratio_fit_error']}; {n_sh} shadowed, {n_border} bordered of {len(surfaces)} surfaces"
    L.append(ev)
    s = "\n".join(L) + "\n"
    if out:
        Path(out).write_text(s)
    if jout:
        Path(jout).write_text(json.dumps(stats, indent=1))
    print(s, end="")


if __name__ == "__main__":
    main()
