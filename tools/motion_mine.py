#!/usr/bin/env python3
"""agentfx motion mine: mine motion statistics from recorder output (L0/L2 motion tokens by measurement).

Input: recorder output dirs (each with model.json = composite tracks with fitted eases, and
probe.json = neutral element rects, from `recorder/src/lower.mjs --plan-only`).

For every composite track (translate / scale / rotate / opacity) of every element:
  1. sample the fitted piecewise curve per frame,
  2. split it into moves (maximal runs between rests),
  3. classify each move (enter / exit / move / emphasis) and measure duration, distance,
  4. fit the move's normalized progress against named easing curves (CSS/GSAP families, Material 3,
     springs by damping ratio) and keep the best family by RMSE,
  5. attach the element's size (probe rect area as % of the frame) and a role guess.
Then aggregates: durations by size class and role, ease families by move kind, stagger intervals
between sibling moves, overlap of consecutive moves, enter/exit pairs.

usage: motion_mine.py <rec-dir>... [--json out.json] [--md out.md] [--tokens tokens.json]
"""
import json, math, os, sys, statistics as st
from collections import Counter, defaultdict

# ---------------------------------------------------------------- easing library
def bez(x1, y1, x2, y2):
    def f(t):
        # solve x(s) = t by Newton + bisection, return y(s)
        lo, hi, s = 0.0, 1.0, t
        for _ in range(30):
            x = 3*(1-s)**2*s*x1 + 3*(1-s)*s*s*x2 + s**3
            if abs(x - t) < 1e-7: break
            if x < t: lo = s
            else: hi = s
            s = (lo + hi) / 2
        return 3*(1-s)**2*s*y1 + 3*(1-s)*s*s*y2 + s**3
    return f

def spring(zeta, settle=1.0):
    # unit step response of a damped spring normalized so it settles (2 % band) at t = 1
    w = 1.0
    # pick natural frequency so that envelope e^(-zeta w t) reaches 0.02 at t=1 (underdamped) or response ~0.98
    if zeta < 1:
        w = -math.log(0.02) / zeta
        wd = w * math.sqrt(1 - zeta*zeta)
        return lambda t: 1 - math.exp(-zeta*w*t) * (math.cos(wd*t) + zeta*w/wd*math.sin(wd*t))
    w = 5.83  # critically damped: (1 + w t) e^{-w t} = 0.02 at t = 1
    return lambda t: 1 - (1 + w*t) * math.exp(-w*t)

def pw(p, kind):
    if kind == 'in': return lambda t: t**p
    if kind == 'out': return lambda t: 1 - (1-t)**p
    return lambda t: (2**(p-1))*t**p if t < .5 else 1 - ((-2*t+2)**p)/2

CURVES = {'linear': lambda t: t}
for name, p in [('quad', 2), ('cubic', 3), ('quart', 4), ('quint', 5)]:
    for k in ['in', 'out', 'inout']:
        CURVES[f'{k}-{name}'] = pw(p, k)
CURVES.update({
    'in-sine': lambda t: 1 - math.cos(t*math.pi/2), 'out-sine': lambda t: math.sin(t*math.pi/2),
    'inout-sine': lambda t: -(math.cos(math.pi*t) - 1)/2,
    'in-expo': lambda t: 0 if t == 0 else 2**(10*t-10), 'out-expo': lambda t: 1 if t == 1 else 1 - 2**(-10*t),
    'inout-expo': lambda t: 0 if t == 0 else 1 if t == 1 else (2**(20*t-10)/2 if t < .5 else (2 - 2**(-20*t+10))/2),
    'out-back': bez(0.34, 1.56, 0.64, 1), 'in-back': bez(0.36, 0, 0.66, -0.56), 'inout-back': bez(0.68, -0.6, 0.32, 1.6),
    'out-circ': lambda t: math.sqrt(max(0, 1 - (t-1)**2)), 'in-circ': lambda t: 1 - math.sqrt(max(0, 1 - t*t)),
    # Material 3
    'm3-standard': bez(0.2, 0, 0, 1), 'm3-standard-decelerate': bez(0, 0, 0, 1), 'm3-standard-accelerate': bez(0.3, 0, 1, 1),
    'm3-emphasized-decelerate': bez(0.05, 0.7, 0.1, 1), 'm3-emphasized-accelerate': bez(0.3, 0, 0.8, 0.15),
    # CSS keywords
    'css-ease': bez(0.25, 0.1, 0.25, 1), 'css-ease-in-out': bez(0.42, 0, 0.58, 1),
})
for z in [0.3, 0.45, 0.6, 0.75, 0.9, 1.0]:
    CURVES[f'spring-z{z}'] = spring(z)

def family(name):
    if name.startswith('spring'): return 'spring'
    if name.startswith('m3-'): return 'm3'
    if name == 'linear': return 'linear'
    for k in ['inout', 'out', 'in']:
        if name.startswith(k + '-'): return k
    return 'other'

def fit(prog):
    """prog: normalized progress samples at t = i/(n-1). Returns (best name, rmse, ranked list)."""
    n = len(prog)
    if n < 3: return ('linear', 0.0, [])
    ts = [i/(n-1) for i in range(n)]
    res = []
    for name, f in CURVES.items():
        e = math.sqrt(sum((f(t) - p)**2 for t, p in zip(ts, prog)) / n)
        res.append((e, name))
    res.sort()
    return res[0][1], res[0][0], res[:3]

# ---------------------------------------------------------------- track sampling
def ease_fn(e):
    if e == 'linear' or e is None: return lambda t: t
    if isinstance(e, dict) and 'cubic' in e: return bez(*e['cubic'])
    if e == 'hold' or (isinstance(e, dict) and 'hold' in e): return lambda t: 0.0
    return lambda t: t

def sample(keys, N):
    """per-frame values (list of tuples) from recorder keys [{f, v:[..], ease}]"""
    out = []
    ks = sorted(keys, key=lambda k: k['f'])
    j = 0
    for f in range(N):
        while j + 1 < len(ks) and ks[j+1]['f'] <= f: j += 1
        a = ks[j]
        if f <= a['f'] or j + 1 >= len(ks):
            out.append(tuple(a['v'])); continue
        b = ks[j+1]
        u = (f - a['f']) / (b['f'] - a['f'])
        y = ease_fn(a['ease'])(u)
        out.append(tuple(av + (bv - av)*y for av, bv in zip(a['v'], b['v'])))
    return out

EPS = {'translate': 0.25, 'scale': 0.002, 'rotate': 0.1, 'opacity': 0.004}

def scalar(prop, v, ref):
    if prop == 'translate':
        return math.dist(v[:2], ref[:2]) if len(v) >= 2 else abs(v[0] - ref[0])
    return v[0]

def moves(prop, vals, fps):
    """maximal runs of frame-to-frame change > EPS (a 1-frame rest does not split a move)"""
    eps = EPS[prop]
    ch = [False] + [max(abs(a - b) for a, b in zip(vals[i], vals[i-1])) > eps for i in range(1, len(vals))]
    runs, i = [], 1
    while i < len(vals):
        if ch[i]:
            s = i - 1
            e = i
            while e + 1 < len(vals) and (ch[e+1] or (e + 2 < len(vals) and ch[e+2])): e += 1
            runs.append((s, e)); i = e + 1
        else:
            i += 1
    out = []
    for s, e in runs:
        if e - s < 2: continue
        a, b = vals[s], vals[e]
        seg = vals[s:e+1]
        if prop == 'translate':
            # progress along the straight line from a to b (projection), distance in px
            d = math.dist(a[:2], b[:2])
            if d < 1: continue
            ux, uy = (b[0]-a[0])/d, (b[1]-a[1])/d
            prog = [((v[0]-a[0])*ux + (v[1]-a[1])*uy)/d for v in seg]
            mag = d
        else:
            d = b[0] - a[0]
            if abs(d) < eps*3: continue
            prog = [(v[0]-a[0])/d for v in seg]
            mag = d
        out.append({'prop': prop, 's': s, 'e': e, 'dur': (e - s)/fps, 'from': a, 'to': b, 'mag': mag, 'prog': prog})
    return out

def kind_of(m, rest):
    p, a, b = m['prop'], m['from'], m['to']
    if p == 'opacity':
        if a[0] < 0.05 and b[0] > 0.5: return 'enter'
        if b[0] < 0.05 and a[0] > 0.5: return 'exit'
        return 'emphasis'
    if p == 'scale':
        if abs(b[0] - 1) < 0.02 and abs(a[0] - 1) > 0.02: return 'enter' if a[0] < 1 else 'settle'
        if abs(a[0] - 1) < 0.02 and b[0] < 0.2: return 'exit'
        return 'emphasis' if abs(b[0]-1) < 0.05 else 'move'
    if p == 'translate':
        da = math.dist(a[:2], rest[:2]); db = math.dist(b[:2], rest[:2])
        if db < 1 and da > 1: return 'enter'
        if da < 1 and db > 1: return 'exit'
        return 'move'
    return 'move'

def main():
    args = sys.argv[1:]
    def opt(k):
        if k in args:
            i = args.index(k); v = args[i+1]; del args[i:i+2]; return v
    jout, mdout, tokout = opt('--json'), opt('--md'), opt('--tokens')
    allm = []
    sources = {}
    for d in args:
        model = json.load(open(os.path.join(d, 'model.json')))
        probe = json.load(open(os.path.join(d, 'probe.json'))) if os.path.exists(os.path.join(d, 'probe.json')) else {'frames': {}}
        meta, fps, N = model['meta'], model['fps'], model['N']
        W, H = meta.get('width', 1920), meta.get('height', 1080)
        name = meta.get('comp') if meta.get('kind') == 'remotion' else os.path.basename(meta.get('project', d).rstrip('/'))
        src = os.path.basename(d.rstrip('/'))
        sources[src] = {'name': name, 'kind': meta.get('kind'), 'frames': N, 'fps': fps}
        pf = sorted(int(k) for k in probe['frames'].keys())
        nodes = {n['uid']: n for n in model['nodes']}
        def rect(uid, f):
            best = None
            for k in pf:
                if k <= f or best is None:
                    r = probe['frames'][str(k)].get(str(uid))
                    if r and r != '0': best = r
                if k > f and best: break
            if not best: return None
            x, y, w, h = [float(v) for v in best.split(',')[:4]]
            return (x, y, w, h)
        def role(n, r):
            kids = [nodes.get(k) for k in n.get('kids', [])]
            has_text = any(k and k.get('text') for k in kids)
            area = (r[2]*r[3]/(W*H)) if r else 0
            if area > 0.25: return 'stage'
            if has_text and len(kids) <= 3: return 'text'
            if area < 0.004: return 'small'
            return 'block'
        for n in model['nodes']:
            comp = n.get('comp') or {}
            for prop, keys in comp.items():
                if prop not in EPS: continue
                vals = sample(keys, N)
                rest = vals[-1] if prop != 'translate' else (0, 0)
                for m in moves(prop, vals, fps):
                    r = rect(n['uid'], m['s'])
                    area = (r[2]*r[3]/(W*H)*100) if r else None
                    best, err, top = fit(m['prog'])
                    m.update({'src': src, 'uid': n['uid'], 'kind': kind_of(m, rest), 'area_pct': area,
                              'size_px': (math.sqrt(r[2]*r[3]) if r else None), 'role': role(n, r), 'ease': best, 'family': family(best), 'rmse': round(err, 4),
                              'top3': [t[1] for t in top], 't': m['s']/fps, 'parent': n.get('parent')})
                    del m['prog']
                    allm.append(m)
    # ---------------------------------------------------------- aggregate
    def q(v):
        v = sorted(x for x in v if x is not None)
        if not v: return None
        k = lambda p: v[min(len(v)-1, int(round(p*(len(v)-1))))]
        return {'n': len(v), 'p25': round(k(.25), 3), 'median': round(k(.5), 3), 'p75': round(k(.75), 3), 'min': round(v[0], 3), 'max': round(v[-1], 3)}
    good = [m for m in allm if m['rmse'] < 0.06]
    agg = {'sources': sources, 'moves': len(allm), 'fitted_below_0.06': len(good)}
    agg['dur_by_kind'] = {k: q([m['dur'] for m in allm if m['kind'] == k]) for k in ['enter', 'exit', 'move', 'emphasis', 'settle']}
    agg['dur_by_kind_prop'] = {f'{k}/{p}': q([m['dur'] for m in allm if m['kind'] == k and m['prop'] == p]) for k in ['enter', 'exit', 'move'] for p in EPS}
    def sclass(a):
        if a is None: return None
        return 'small(<1%)' if a < 1 else 'medium(1-10%)' if a < 10 else 'large(>=10%)'
    agg['dur_by_size'] = {s: q([m['dur'] for m in allm if sclass(m['area_pct']) == s and m['kind'] in ('enter', 'move')]) for s in ['small(<1%)', 'medium(1-10%)', 'large(>=10%)']}
    agg['dur_by_role'] = {r: q([m['dur'] for m in allm if m['role'] == r and m['kind'] in ('enter', 'move')]) for r in ['text', 'small', 'block', 'stage']}
    agg['translate_px_by_kind'] = {k: q([abs(m['mag']) for m in allm if m['prop'] == 'translate' and m['kind'] == k]) for k in ['enter', 'exit', 'move']}
    agg['scale_from_enter'] = q([m['from'][0] for m in allm if m['prop'] == 'scale' and m['kind'] == 'enter'])
    fam = defaultdict(Counter); names = defaultdict(Counter)
    for m in good:
        fam[m['kind']][m['family']] += 1; names[m['kind']][m['ease']] += 1
    agg['family_by_kind'] = {k: dict(v.most_common()) for k, v in fam.items()}
    agg['ease_by_kind'] = {k: dict(v.most_common(8)) for k, v in names.items()}
    # per source: dominant ease of enters
    agg['per_source'] = {s: {'moves': sum(1 for m in allm if m['src'] == s), 'enter_ease': Counter(m['ease'] for m in good if m['src'] == s and m['kind'] == 'enter').most_common(3),
                             'enter_dur_median': (q([m['dur'] for m in allm if m['src'] == s and m['kind'] == 'enter']) or {}).get('median')} for s in sources}
    # staggers: per source, starts of enters (one per element, opacity or translate) that follow the
    # previous enter within 0.5 s form a chain; the start-to-start intervals are stagger intervals
    stag = []
    for s_ in sources:
        ens = sorted([m for m in allm if m['src'] == s_ and m['kind'] in ('enter',) and m['prop'] in ('opacity', 'translate', 'scale')], key=lambda m: m['s'])
        seen, starts = set(), []
        for m in ens:
            if m['uid'] in seen: continue
            seen.add(m['uid']); starts.append(m['s'] / sources[s_]['fps'])
        for x, y in zip(starts, starts[1:]):
            if 0 < y - x <= 0.5: stag.append(y - x)
    agg['stagger_s'] = q(stag)
    agg['stagger_hist_ms'] = dict(sorted(Counter(int(round(x*1000/33.33))*33 for x in stag).items()))
    # overlap: consecutive enters of different elements in one source, next start as fraction of previous duration
    ov = []
    for s in sources:
        ens = sorted([m for m in allm if m['src'] == s and m['kind'] == 'enter' and m['prop'] in ('opacity', 'translate')], key=lambda m: m['s'])
        seen = set()
        uniq = []
        for m in ens:
            if m['uid'] in seen: continue
            seen.add(m['uid']); uniq.append(m)
        for a, b in zip(uniq, uniq[1:]):
            if a['e'] > b['s'] >= a['s'] and a['e'] > a['s']:
                ov.append((b['s'] - a['s']) / (a['e'] - a['s']))
    agg['overlap_fraction'] = q(ov)
    # enter/exit pairs on the same element and prop
    pairs = []
    for (src, uid, prop), ms in groupby3(allm).items():
        en = [m for m in ms if m['kind'] == 'enter']; ex = [m for m in ms if m['kind'] == 'exit']
        if en and ex: pairs.append(ex[0]['dur'] / en[0]['dur'])
    agg['exit_over_enter'] = q(pairs)
    agg['exit_ease_family'] = dict(Counter(m['family'] for m in good if m['kind'] == 'exit').most_common())
    if jout:
        json.dump({'aggregate': agg, 'moves': allm}, open(jout, 'w'), indent=1, default=str)
    print(json.dumps(agg, indent=1, default=str))

def groupby3(allm):
    g = defaultdict(list)
    for m in allm: g[(m['src'], m['uid'], m['prop'])].append(m)
    return g

if __name__ == '__main__':
    main()
