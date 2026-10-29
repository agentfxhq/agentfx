//! Text v2 (IR `TextFx` on `paint` content): the list's glyph runs are split into units
//! (char, word, line; found from the glyph data), optionally placed on a path, and animated
//! per unit by range / key selectors (opacity, translate, scale, rotate, blur, color). Fill
//! and stroke colors and the stroke width animate with `text.*` tracks.
//!
//! The renderer rewrites the display list per frame (`Prep::frame`) and draws the result
//! like any `paint` list, so clips, groups, gradients, LCD/mask text paths and culling stay
//! the ones of the list. The state that decides the pixels (`Prep::state`: per-unit amounts,
//! colors, path offset) is a pure function of t; the planner hashes it.
use crate::record;
use kurbo::{Affine, BezPath, Point, Rect, Vec2};
use peniko::FontData;
use agentfx_ir::{Brush, Draw, Easing, GlyphStroke, TextFx, TextSelector};
use std::hash::{Hash, Hasher};

/// Glyph reference: position in the list and per-glyph data in scene space.
struct G {
    item: usize,
    #[allow(dead_code)]
    k: usize,
    /// unit index per kind: [char, word, line]
    unit: [u32; 3],
    /// glyph origin (scene)
    origin: (f64, f64),
    /// ink box center (scene)
    center: (f64, f64),
    /// half the advance along the line (distance to the next glyph of the line, or half the ink)
    half: f64,
}

/// Flattened path with cumulative arc lengths (text on a path).
struct Poly {
    pts: Vec<Point>,
    cum: Vec<f64>,
}

impl Poly {
    fn new(d: &str) -> Option<Poly> {
        let p = BezPath::from_svg(d).ok()?;
        let mut pts: Vec<Point> = vec![];
        kurbo::flatten(p.elements().iter().copied(), 0.05, |el| match el {
            kurbo::PathEl::MoveTo(q) | kurbo::PathEl::LineTo(q) => pts.push(q),
            _ => {}
        });
        pts.dedup_by(|a, b| a.distance(*b) < 1e-9);
        if pts.len() < 2 {
            return None;
        }
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            cum.push(cum.last().unwrap() + w[0].distance(w[1]));
        }
        Some(Poly { pts, cum })
    }
    fn total(&self) -> f64 {
        *self.cum.last().unwrap()
    }
    /// Point and tangent angle (radians) at arc length s (clamped).
    fn at(&self, s: f64) -> (Point, f64) {
        let s = s.clamp(0.0, self.total());
        let i = match self.cum.binary_search_by(|v| v.partial_cmp(&s).unwrap()) {
            Ok(i) => i.min(self.pts.len() - 2),
            Err(i) => (i.max(1) - 1).min(self.pts.len() - 2),
        };
        let (a, b) = (self.pts[i], self.pts[i + 1]);
        let l = (self.cum[i + 1] - self.cum[i]).max(1e-12);
        let u = ((s - self.cum[i]) / l).clamp(0.0, 1.0);
        let p = a.lerp(b, u);
        // tangent: averaged over +-1.5 px so polyline corners do not flip glyphs
        let (p0, p1) = (self.pt(s - 1.5), self.pt(s + 1.5));
        let ang = if p0.distance(p1) > 1e-9 { (p1.y - p0.y).atan2(p1.x - p0.x) } else { (b.y - a.y).atan2(b.x - a.x) };
        (p, ang)
    }
    fn pt(&self, s: f64) -> Point {
        let s = s.clamp(0.0, self.total());
        let i = match self.cum.binary_search_by(|v| v.partial_cmp(&s).unwrap()) {
            Ok(i) => i.min(self.pts.len() - 2),
            Err(i) => (i.max(1) - 1).min(self.pts.len() - 2),
        };
        let l = (self.cum[i + 1] - self.cum[i]).max(1e-12);
        self.pts[i].lerp(self.pts[i + 1], ((s - self.cum[i]) / l).clamp(0.0, 1.0))
    }
}

pub struct Prep {
    pub items: Vec<Draw>,
    pub fx: TextFx,
    glyphs: Vec<G>,
    /// unit counts [char, word, line]
    units: [usize; 3],
    /// unit centers at rest (scene), per kind
    centers: [Vec<(f64, f64)>; 3],
    path: Option<Poly>,
    x0: f64,
    base_y: f64,
    /// a key selector: the state depends on t itself (not only on tracks)
    pub keyed: bool,
}

/// Values of everything animatable at one time.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct State {
    /// per animator: amount per unit (its kind's count)
    pub amounts: Vec<Vec<f64>>,
    pub fill: Option<[f64; 4]>,
    pub stroke: Option<[f64; 4]>,
    pub stroke_w: Option<f64>,
    pub path_offset: f64,
}

impl State {
    pub fn hash_into(&self, h: &mut impl Hasher) {
        for a in &self.amounts {
            for v in a {
                v.to_bits().hash(h);
            }
        }
        for c in [self.fill, self.stroke].iter().flatten() {
            c.map(f64::to_bits).hash(h);
        }
        self.stroke_w.map(f64::to_bits).hash(h);
        self.path_offset.to_bits().hash(h);
    }
}

fn kind(by: &str) -> usize {
    match by {
        "word" => 1,
        "line" => 2,
        _ => 0,
    }
}

fn mat(m: &[f64; 6]) -> Affine {
    Affine::new(*m)
}

/// Space-like glyph ids of a font (U+0020, U+00A0, U+3000, U+2009, tab).
fn space_ids(f: Option<&FontData>) -> Vec<u32> {
    use skrifa::MetadataProvider;
    let Some(f) = f else { return vec![] };
    let Ok(r) = skrifa::FontRef::from_index(f.data.data(), f.index) else { return vec![] };
    let cm = r.charmap();
    [' ', '\u{a0}', '\u{3000}', '\u{2009}', '\t'].iter().filter_map(|c| cm.map(*c)).map(|g| g.to_u32()).collect()
}

impl Prep {
    pub fn new(items: &[Draw], fx: Option<&TextFx>, fonts: &[FontData]) -> Prep {
        let fx = fx.cloned().unwrap_or_default();
        let fl = |f: usize| fonts.get(f).cloned();
        let mut glyphs: Vec<G> = vec![];
        let (mut ci, mut wi, mut li) = (0u32, 0u32, 0u32);
        // a word starts at the first non-space glyph after a space or a line break; spaces
        // belong to the unit before them
        let (mut started, mut brk) = (false, false);
        let mut prev_y: Option<(f64, f64)> = None;
        for (item, d) in items.iter().enumerate() {
            let Draw::Glyphs { font, size, coords, transform, glyph_transform, glyphs: gl, .. } = d else { continue };
            let fd = fl(*font);
            let spaces = space_ids(fd.as_ref());
            let m = mat(transform);
            let gt = glyph_transform.map(|g| mat(&g));
            let scale = m.as_coeffs()[3].abs().max(1e-9);
            for (k, &(id, x, y)) in gl.iter().enumerate() {
                let o = m * Point::new(x as f64, y as f64);
                let space = spaces.contains(&id);
                if let Some((py, psz)) = prev_y {
                    if (o.y - py).abs() > 0.3 * psz {
                        li += 1;
                        brk = true;
                    }
                }
                prev_y = Some((o.y, *size as f64 * scale));
                if space {
                    brk = true;
                } else {
                    if started {
                        ci += 1;
                        if brk {
                            wi += 1;
                        }
                    }
                    started = true;
                    brk = false;
                }
                let b = m.transform_rect_bbox(record::glyph_bounds(fd.as_ref(), *size, coords, &[(id, x, y)], gt).inflate(-1.0, -1.0));
                let c = if b.width() > 0.0 && b.height() > 0.0 { b.center() } else { o };
                glyphs.push(G { item, k, unit: [ci, wi, li], origin: (o.x, o.y), center: (c.x, c.y), half: (b.width() / 2.0).max(0.0) });
            }
        }
        // lines holding only spaces leave gaps: renumber densely
        for kd in 0..3 {
            let mut seen: Vec<u32> = glyphs.iter().map(|g| g.unit[kd]).collect();
            seen.sort();
            seen.dedup();
            for g in &mut glyphs {
                g.unit[kd] = seen.binary_search(&g.unit[kd]).unwrap() as u32;
            }
        }
        let units = [0, 1, 2].map(|kd| glyphs.iter().map(|g| g.unit[kd] as usize + 1).max().unwrap_or(0));
        // advance: distance to the next glyph origin on the same line
        for i in 0..glyphs.len() {
            if let Some(nx) = glyphs.get(i + 1) {
                if nx.unit[2] == glyphs[i].unit[2] && nx.origin.0 > glyphs[i].origin.0 {
                    glyphs[i].half = (nx.origin.0 - glyphs[i].origin.0) / 2.0;
                }
            }
        }
        let centers = [0, 1, 2].map(|kd| {
            let mut r: Vec<Option<Rect>> = vec![None; units[kd]];
            for g in &glyphs {
                let b = Rect::from_center_size((g.center.0, g.center.1), (0.0, 0.0));
                let u = g.unit[kd] as usize;
                r[u] = Some(r[u].map_or(b, |x| x.union(b)));
            }
            r.into_iter().map(|b| b.map_or((0.0, 0.0), |b| (b.center().x, b.center().y))).collect()
        });
        let x0 = glyphs.iter().map(|g| g.origin.0).fold(f64::INFINITY, f64::min);
        let base_y = glyphs.first().map(|g| g.origin.1).unwrap_or(0.0);
        let path = fx.path.as_ref().and_then(|p| Poly::new(&p.d));
        let keyed = fx.animators.iter().any(|a| matches!(a.selector, TextSelector::Keys { .. }));
        Prep { items: items.to_vec(), fx, glyphs, units, centers, path, x0: if x0.is_finite() { x0 } else { 0.0 }, base_y, keyed }
    }

    /// Unit counts [char, word, line].
    #[allow(dead_code)]
    pub fn units(&self) -> [usize; 3] {
        self.units
    }

    /// The animatable state at t. `num(prop, base)` / `col(prop)` read the node's tracks.
    pub fn state(&self, t: f64, num: &dyn Fn(&str, f64) -> f64, col: &dyn Fn(&str) -> Option<[f64; 4]>) -> State {
        let mut amounts = Vec::with_capacity(self.fx.animators.len());
        for (i, a) in self.fx.animators.iter().enumerate() {
            let n = self.units[kind(&a.by)];
            let p = |f: &str, b: f64| num(&format!("animators.{i}.{f}"), b);
            let v: Vec<f64> = match &a.selector {
                TextSelector::Range { start, end, offset, shape, amount, order, seed } => {
                    let (s, e, o, am) = (p("start", *start), p("end", *end), p("offset", *offset), p("amount", *amount));
                    let perm = order_perm(order, *seed, n);
                    (0..n).map(|u| am * range_amount(shape, s.min(e) + o, s.max(e) + o, perm[u], n)).collect()
                }
                TextSelector::Keys { units, easing, amount } => {
                    let am = p("amount", *amount);
                    (0..n).map(|u| am * key_amount(&units[u.min(units.len() - 1)], easing, t)).collect()
                }
            };
            // quantize: 1/4096 of an amount is invisible and keeps rest frames bit-equal
            amounts.push(v.into_iter().map(|x| (x * 4096.0).round() / 4096.0).collect());
        }
        let po = self.fx.path.as_ref().map_or(0.0, |p| num("text.path-offset", p.offset));
        let sw = num("text.stroke-width", f64::NAN);
        State { amounts, fill: col("text.fill"), stroke: col("text.stroke"), stroke_w: sw.is_finite().then_some(sw.max(0.0)), path_offset: (po * 64.0).round() / 64.0 }
    }

    /// True when the state leaves the list unchanged (draw the converted base list).
    pub fn is_rest(&self, st: &State) -> bool {
        self.path.is_none() && st.fill.is_none() && st.stroke.is_none() && st.stroke_w.is_none() && st.amounts.iter().all(|a| a.iter().all(|v| *v == 0.0))
    }

    /// Per-glyph placement on the path (scene-space transform applied after the run's), or
    /// None = hidden (off the path).
    fn place(&self, g: &G, offset: f64) -> Option<Affine> {
        let Some(p) = &self.path else { return Some(Affine::IDENTITY) };
        let mid = g.origin.0 - self.x0 + offset + g.half;
        if mid < 0.0 || mid > p.total() {
            return None;
        }
        let (pm, ang) = p.at(mid);
        let (s, c) = ang.sin_cos();
        let d = g.origin.1 - self.base_y;
        let o = Point::new(pm.x - c * g.half - s * d, pm.y - s * g.half + c * d);
        Some(Affine::translate(o.to_vec2()) * Affine::rotate(ang) * Affine::translate((-g.origin.0, -g.origin.1)))
    }

    /// The display list at state `st`.
    pub fn frame(&self, st: &State) -> Vec<Draw> {
        if self.is_rest(st) {
            return self.items.clone();
        }
        // per glyph: placement
        let place: Vec<Option<Affine>> = self.glyphs.iter().map(|g| self.place(g, st.path_offset)).collect();
        // unit centers (placed glyph box centers averaged when on a path)
        let centers: [Vec<(f64, f64)>; 3] = if self.path.is_some() {
            [0, 1, 2].map(|kd| {
                let mut acc = vec![(0.0, 0.0, 0usize); self.units[kd]];
                for (g, pl) in self.glyphs.iter().zip(&place) {
                    if let Some(m) = pl {
                        let c = *m * Point::new(g.center.0, g.center.1);
                        let a = &mut acc[g.unit[kd] as usize];
                        a.0 += c.x;
                        a.1 += c.y;
                        a.2 += 1;
                    }
                }
                acc.into_iter().map(|(x, y, n)| if n > 0 { (x / n as f64, y / n as f64) } else { (0.0, 0.0) }).collect()
            })
        } else {
            self.centers.clone()
        };
        // glyph index range per item
        let mut first: Vec<Option<usize>> = vec![None; self.items.len()];
        for (gi, g) in self.glyphs.iter().enumerate() {
            if first[g.item].is_none() {
                first[g.item] = Some(gi);
            }
        }
        let mut out = Vec::with_capacity(self.items.len() + 8);
        for (ii, d) in self.items.iter().enumerate() {
            let Draw::Glyphs { font, size, coords, embolden, hint, brush, transform, glyph_transform, glyphs, stroke } = d else {
                out.push(d.clone());
                continue;
            };
            let Some(g0) = first[ii] else {
                out.push(d.clone());
                continue;
            };
            // fill / stroke overrides
            let fill = match st.fill {
                Some(c) => Brush::Solid(c.map(|v| v as f32)),
                None => brush.clone(),
            };
            let stroke: Option<GlyphStroke> = match (stroke, st.stroke_w, st.stroke) {
                (Some(s), w, c) => Some(GlyphStroke { width: w.unwrap_or(s.width), brush: c.map(|c| Brush::Solid(c.map(|v| v as f32))).unwrap_or(s.brush.clone()), ..s.clone() }),
                (None, Some(w), c) if w > 0.0 => Some(GlyphStroke { width: w, brush: Brush::Solid(c.unwrap_or([0.0, 0.0, 0.0, 1.0]).map(|v| v as f32)), join: "round".into(), over: false }),
                _ => None,
            };
            let stroke = stroke.filter(|s| s.width > 0.0);
            let m = mat(transform);
            // consecutive glyphs with the same per-glyph state share one run
            let mut k = 0;
            while k < glyphs.len() {
                let gi = g0 + k;
                let g = &self.glyphs[gi];
                let us = self.unit_state(st, g, &centers);
                let pl = place[gi];
                let mut j = k + 1;
                if self.path.is_none() {
                    while j < glyphs.len() && self.unit_state(st, &self.glyphs[g0 + j], &centers) == us {
                        j += 1;
                    }
                }
                let (Some(pl), true) = (pl, us.opacity > 0.0) else {
                    k = j;
                    continue;
                };
                let tm = us.m * pl * m;
                let mut run_fill = fill.clone();
                let mut overlay: Option<Brush> = None;
                if let Some((c, a)) = us.color {
                    match &run_fill {
                        Brush::Solid(b) => {
                            let b = b.map(|v| v as f64);
                            run_fill = Brush::Solid([0, 1, 2, 3].map(|i| (b[i] + (c[i] - b[i]) * a) as f32));
                        }
                        _ => overlay = Some(Brush::Solid([c[0] as f32, c[1] as f32, c[2] as f32, (c[3] * a) as f32])),
                    }
                }
                let mut run_stroke = stroke.clone();
                let layer = us.opacity < 1.0 && (run_stroke.is_some() || overlay.is_some() || !matches!(run_fill, Brush::Solid(_)));
                if us.opacity < 1.0 && !layer {
                    if let Brush::Solid(c) = &mut run_fill {
                        c[3] *= us.opacity as f32;
                    }
                }
                let blur = us.blur > 0.01;
                if blur {
                    out.push(Draw::Push { clip: None, transform: agentfx_ir::MAT_IDENTITY, alpha: 1.0, blend: Default::default(), filter: Some(agentfx_ir::DrawFilter::Blur { std_dev: us.blur }), even_odd: false });
                }
                if layer {
                    out.push(Draw::Push { clip: None, transform: agentfx_ir::MAT_IDENTITY, alpha: us.opacity as f32, blend: Default::default(), filter: None, even_odd: false });
                }
                let sub: Vec<(u32, f32, f32)> = glyphs[k..j].to_vec();
                if let Some(s) = &mut run_stroke {
                    if let (Brush::Solid(c), false) = (&mut s.brush, layer) {
                        c[3] *= us.opacity as f32;
                    }
                }
                out.push(Draw::Glyphs { font: *font, size: *size, coords: coords.clone(), embolden: *embolden, hint: *hint, brush: run_fill, transform: tm.as_coeffs(), glyph_transform: *glyph_transform, glyphs: sub.clone(), stroke: run_stroke });
                if let Some(b) = overlay {
                    out.push(Draw::Glyphs { font: *font, size: *size, coords: coords.clone(), embolden: *embolden, hint: *hint, brush: b, transform: tm.as_coeffs(), glyph_transform: *glyph_transform, glyphs: sub, stroke: None });
                }
                if layer {
                    out.push(Draw::Pop);
                }
                if blur {
                    out.push(Draw::Pop);
                }
                k = j;
            }
        }
        out
    }

    fn unit_state(&self, st: &State, g: &G, centers: &[Vec<(f64, f64)>; 3]) -> UnitState {
        let mut o = 1.0;
        let mut m = Affine::IDENTITY;
        let mut blur = 0.0;
        let mut color: Option<([f64; 4], f64)> = None;
        for (a, am) in self.fx.animators.iter().zip(&st.amounts) {
            let kd = kind(&a.by);
            let u = g.unit[kd] as usize;
            let v = am.get(u).copied().unwrap_or(0.0);
            if v == 0.0 {
                continue;
            }
            if let Some(op) = a.opacity {
                o *= (1.0 + (op - 1.0) * v).clamp(0.0, 1.0);
            }
            let (cx, cy) = centers[kd].get(u).copied().unwrap_or(g.center);
            let mut local = Affine::IDENTITY;
            if a.scale.is_some() || a.rotate.is_some() {
                let s = a.scale.unwrap_or([1.0, 1.0]);
                let r = a.rotate.unwrap_or(0.0) * v;
                local = Affine::translate((cx, cy)) * Affine::rotate(r.to_radians()) * Affine::scale_non_uniform(1.0 + (s[0] - 1.0) * v, 1.0 + (s[1] - 1.0) * v) * Affine::translate((-cx, -cy));
            }
            if let Some(tr) = a.translate {
                local = Affine::translate(Vec2::new(tr[0] * v, tr[1] * v)) * local;
            }
            m = local * m;
            if let Some(b) = a.blur {
                blur += b * v;
            }
            if let Some(c) = a.color {
                color = Some(match color {
                    None => (c, v.clamp(0.0, 1.0)),
                    Some((c0, a0)) => {
                        let w = v.clamp(0.0, 1.0);
                        ([0, 1, 2, 3].map(|i| c0[i] + (c[i] - c0[i]) * w), a0.max(w))
                    }
                });
            }
        }
        UnitState { opacity: (o * 255.0).round() / 255.0, m, blur, color }
    }
}

#[derive(PartialEq)]
struct UnitState {
    opacity: f64,
    m: Affine,
    blur: f64,
    color: Option<([f64; 4], f64)>,
}

/// AE range selector amount of unit `i` of `n` for the range [s, e] (fractions).
pub fn range_amount(shape: &str, s: f64, e: f64, i: usize, n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let (a, b) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
    let c = (a + b) / 2.0;
    let w = e - s;
    let u = if w > 1e-12 { (c - s) / w } else if c >= s { 1.0 } else { 0.0 };
    let inside = (0.0..=1.0).contains(&u) && w > 1e-12;
    match shape {
        "ramp-up" => u.clamp(0.0, 1.0),
        "ramp-down" => 1.0 - u.clamp(0.0, 1.0),
        "triangle" if inside => 1.0 - (2.0 * u - 1.0).abs(),
        "round" if inside => (1.0 - (2.0 * u - 1.0).powi(2)).max(0.0).sqrt(),
        "smooth" if inside => 0.5 - 0.5 * (std::f64::consts::TAU * u).cos(),
        "triangle" | "round" | "smooth" => 0.0,
        _ => ((b.min(e) - a.max(s)) / (b - a)).clamp(0.0, 1.0),
    }
}

/// Amount from per-unit keys `[[t, v], ...]` at t (held outside, eased between).
pub fn key_amount(keys: &[[f64; 2]], easing: &Easing, t: f64) -> f64 {
    let Some(first) = keys.first() else { return 0.0 };
    if t <= first[0] {
        return first[1];
    }
    for w in keys.windows(2) {
        if t < w[1][0] {
            let span = (w[1][0] - w[0][0]).max(1e-9);
            let p = agentfx_ir::timing::ease(easing, ((t - w[0][0]) / span).clamp(0.0, 1.0), false);
            return w[0][1] + (w[1][1] - w[0][1]) * p;
        }
    }
    keys.last().unwrap()[1]
}

/// Unit -> slot permutation for the range selector's order.
fn order_perm(order: &str, seed: u64, n: usize) -> Vec<usize> {
    match order {
        "reverse" => (0..n).map(|i| n - 1 - i).collect(),
        "random" => {
            let mut v: Vec<usize> = (0..n).collect();
            let mut s = seed ^ 0x9E37_79B9_7F4A_7C15;
            let mut next = || {
                s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = s;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^ (z >> 31)
            };
            for i in (1..n).rev() {
                let j = (next() % (i as u64 + 1)) as usize;
                v.swap(i, j);
            }
            v
        }
        _ => (0..n).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_shapes() {
        // square: partial coverage of the edge unit
        assert_eq!(range_amount("square", 0.0, 0.5, 0, 4), 1.0);
        assert_eq!(range_amount("square", 0.0, 0.5, 2, 4), 0.0);
        assert!((range_amount("square", 0.0, 0.375, 1, 4) - 0.5).abs() < 1e-12);
        // ramp-up: 0 before, 1 after
        assert_eq!(range_amount("ramp-up", 0.5, 1.0, 0, 4), 0.0);
        assert_eq!(range_amount("ramp-up", 0.0, 0.25, 3, 4), 1.0);
        assert!((range_amount("ramp-down", 0.0, 1.0, 1, 4) - 0.625).abs() < 1e-12);
        // bells: 0 outside, 1 at the middle
        assert_eq!(range_amount("smooth", 2.0, 3.0, 1, 4), 0.0);
        assert!((range_amount("triangle", 0.0, 1.0, 1, 2) - 0.5).abs() < 1e-12);
        assert!((range_amount("smooth", 0.0, 1.0, 0, 2) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn keys_hold_and_ease() {
        let k = [[1.0, 1.0], [2.0, 0.0]];
        assert_eq!(key_amount(&k, &Easing::Linear, 0.0), 1.0);
        assert_eq!(key_amount(&k, &Easing::Linear, 1.5), 0.5);
        assert_eq!(key_amount(&k, &Easing::Linear, 3.0), 0.0);
    }

    #[test]
    fn random_order_is_a_seeded_permutation() {
        let a = order_perm("random", 7, 20);
        let mut s = a.clone();
        s.sort();
        assert_eq!(s, (0..20).collect::<Vec<_>>());
        assert_eq!(a, order_perm("random", 7, 20));
        assert_ne!(a, order_perm("random", 8, 20));
    }

    #[test]
    fn poly_points_and_tangents() {
        let p = Poly::new("M0 0 L100 0 L100 100").unwrap();
        assert!((p.total() - 200.0).abs() < 1e-9);
        let (q, a) = p.at(50.0);
        assert!((q.x - 50.0).abs() < 1e-9 && q.y.abs() < 1e-9 && a.abs() < 1e-9);
        let (q, a) = p.at(150.0);
        assert!((q.y - 50.0).abs() < 1e-9 && (a - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    }
}
