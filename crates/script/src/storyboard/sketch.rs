//! A pencil-and-marker canvas (tiny-skia, CPU). Every mark is geometry: a stroke is a filled
//! ribbon around a wobbly centre line (smooth seeded noise), tapered at both ends, drawn in one
//! or more passes; straight lines overshoot their end points; hatching is a set of such strokes
//! clipped to a region. All randomness is a pure function of (seed, key), so a board is
//! byte-identical for the same input and seed.
use super::hand;
use tiny_skia::{Color, FillRule, Mask, Paint, PathBuilder, Pixmap, Transform};

pub type P = (f64, f64);

#[derive(Clone, Copy, Debug)]
pub struct Ink {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}
impl Ink {
    pub const fn rgb(r: u8, g: u8, b: u8, a: f32) -> Ink {
        Ink { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a }
    }
    pub fn alpha(self, k: f64) -> Ink {
        Ink { a: (self.a as f64 * k).clamp(0.0, 1.0) as f32, ..self }
    }
}

pub const PAPER: Ink = Ink::rgb(0xF4, 0xF0, 0xE6, 1.0);
pub const PANEL: Ink = Ink::rgb(0xFB, 0xF9, 0xF3, 1.0);
pub const GRAPHITE: Ink = Ink::rgb(0x3A, 0x38, 0x36, 0.82);
pub const INK: Ink = Ink::rgb(0x1E, 0x1C, 0x1B, 0.92);
pub const RED: Ink = Ink::rgb(0xC8, 0x3A, 0x2E, 0.95);
pub const BLUE: Ink = Ink::rgb(0x2C, 0x5B, 0xA6, 0.95);

// ---------------------------------------------------------------- randomness
pub fn mix(a: u64, b: u64) -> u64 {
    let mut z = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
/// Uniform [0, 1).
pub fn rnd(seed: u64, k: u64) -> f64 {
    (mix(seed, k) >> 11) as f64 / (1u64 << 53) as f64
}
/// Uniform [-1, 1).
pub fn rnds(seed: u64, k: u64) -> f64 {
    rnd(seed, k) * 2.0 - 1.0
}
/// Smooth 1D value noise in [-1, 1].
pub fn noise(seed: u64, x: f64) -> f64 {
    let i = x.floor();
    let f = x - i;
    let a = rnds(seed, i as i64 as u64);
    let b = rnds(seed, (i as i64 + 1) as u64);
    let s = f * f * (3.0 - 2.0 * f);
    a + (b - a) * s
}
/// FNV-1a of a string (stable seed per layer id).
pub fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[derive(Clone, Copy, Debug)]
pub struct Pen {
    pub ink: Ink,
    /// width px
    pub w: f64,
    /// amplitude of the centre-line wobble, px
    pub wobble: f64,
    /// max overshoot of straight lines past their ends, px
    pub overshoot: f64,
    pub passes: u32,
    /// 0 = constant width, 1 = pointed ends
    pub taper: f64,
    pub dash: Option<(f64, f64)>,
}
impl Pen {
    pub fn pencil(w: f64) -> Pen {
        Pen { ink: GRAPHITE, w, wobble: 0.9, overshoot: 5.0, passes: 2, taper: 0.6, dash: None }
    }
    pub fn ink(w: f64) -> Pen {
        Pen { ink: INK, w, wobble: 0.7, overshoot: 3.0, passes: 1, taper: 0.5, dash: None }
    }
    pub fn color(self, ink: Ink) -> Pen {
        Pen { ink, ..self }
    }
    pub fn width(self, w: f64) -> Pen {
        Pen { w, ..self }
    }
    pub fn passes(self, n: u32) -> Pen {
        Pen { passes: n, ..self }
    }
    pub fn dashed(self, on: f64, off: f64) -> Pen {
        Pen { dash: Some((on, off)), ..self }
    }
    pub fn wobble(self, w: f64) -> Pen {
        Pen { wobble: w, ..self }
    }
    pub fn overshoot(self, o: f64) -> Pen {
        Pen { overshoot: o, ..self }
    }
}

// ---------------------------------------------------------------- geometry helpers
pub fn dist(a: P, b: P) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}
pub fn poly_len(p: &[P]) -> f64 {
    p.windows(2).map(|w| dist(w[0], w[1])).sum()
}
/// Resample a polyline at a fixed step (keeps both ends).
pub fn resample(p: &[P], step: f64) -> Vec<P> {
    if p.len() < 2 {
        return p.to_vec();
    }
    let mut out = vec![p[0]];
    let mut carry = 0.0;
    for w in p.windows(2) {
        let (a, b) = (w[0], w[1]);
        let l = dist(a, b);
        if l < 1e-9 {
            continue;
        }
        let mut s = step - carry;
        while s <= l {
            let t = s / l;
            out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            s += step;
        }
        carry = l - (s - step);
    }
    let last = *p.last().unwrap();
    if dist(*out.last().unwrap(), last) > step * 0.25 {
        out.push(last);
    } else {
        *out.last_mut().unwrap() = last;
    }
    out
}
/// The first `frac` (0..1) of a polyline by length.
pub fn trim(p: &[P], from: f64, to: f64) -> Vec<P> {
    let l = poly_len(p);
    if l <= 0.0 || to <= from {
        return vec![];
    }
    let (s0, s1) = (from.clamp(0.0, 1.0) * l, to.clamp(0.0, 1.0) * l);
    let mut out = vec![];
    let mut s = 0.0;
    for w in p.windows(2) {
        let d = dist(w[0], w[1]);
        let (a, b) = (s, s + d);
        if b >= s0 && a <= s1 && d > 0.0 {
            let lerp = |x: f64| {
                let t = ((x - a) / d).clamp(0.0, 1.0);
                (w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t)
            };
            if out.is_empty() {
                out.push(lerp(s0.max(a)));
            }
            out.push(lerp(s1.min(b)));
        }
        s = b;
    }
    out
}
fn path_of(pts: &[P]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let mut it = pts.iter();
    let f = it.next()?;
    pb.move_to(f.0 as f32, f.1 as f32);
    for p in it {
        pb.line_to(p.0 as f32, p.1 as f32);
    }
    pb.close();
    pb.finish()
}
fn paths_of(polys: &[Vec<P>]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for poly in polys {
        let mut it = poly.iter();
        let Some(f) = it.next() else { continue };
        pb.move_to(f.0 as f32, f.1 as f32);
        for p in it {
            pb.line_to(p.0 as f32, p.1 as f32);
        }
        pb.close();
    }
    pb.finish()
}
pub fn rect_poly(r: [f64; 4]) -> Vec<P> {
    vec![(r[0], r[1]), (r[0] + r[2], r[1]), (r[0] + r[2], r[1] + r[3]), (r[0], r[1] + r[3])]
}
pub fn ellipse_poly(cx: f64, cy: f64, rx: f64, ry: f64, n: usize) -> Vec<P> {
    (0..n).map(|i| {
        let a = i as f64 / n as f64 * std::f64::consts::TAU;
        (cx + rx * a.cos(), cy + ry * a.sin())
    }).collect()
}
pub fn rounded_rect_poly(r: [f64; 4], rad: f64) -> Vec<P> {
    let rad = rad.min(r[2] / 2.0).min(r[3] / 2.0).max(0.0);
    if rad < 1.0 {
        return rect_poly(r);
    }
    let mut out = vec![];
    let corners = [(r[0] + r[2] - rad, r[1] + rad, -90.0), (r[0] + r[2] - rad, r[1] + r[3] - rad, 0.0), (r[0] + rad, r[1] + r[3] - rad, 90.0), (r[0] + rad, r[1] + rad, 180.0)];
    for (cx, cy, a0) in corners {
        for k in 0..=6 {
            let a = (a0 + 90.0 * k as f64 / 6.0f64).to_radians();
            out.push((cx + rad * a.cos(), cy + rad * a.sin()));
        }
    }
    out
}

// ---------------------------------------------------------------- canvas
pub struct Sketch {
    pub pm: Pixmap,
    /// clip for content marks (the current panel), None = whole page
    pub clip: Option<Mask>,
    pub w: u32,
    pub h: u32,
}

impl Sketch {
    pub fn new(w: u32, h: u32) -> Sketch {
        let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("storyboard page size");
        pm.fill(color(PAPER));
        Sketch { pm, clip: None, w, h }
    }

    /// Paper tooth: per-pixel grain plus faint horizontal fibres (deterministic).
    pub fn paper_grain(&mut self, seed: u64) {
        let w = self.w as usize;
        let data = self.pm.data_mut();
        for (i, px) in data.chunks_exact_mut(4).enumerate() {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            let g = rnds(seed, i as u64) * 3.2 + noise(seed ^ 0x51, x / 23.0 + (y / 3.0).floor() * 17.31) * 2.2;
            for c in px.iter_mut().take(3) {
                *c = (*c as f64 + g).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    pub fn clip_rect(&mut self, r: Option<[f64; 4]>) {
        self.clip = r.and_then(|r| {
            let mut m = Mask::new(self.w, self.h)?;
            let p = path_of(&rect_poly(r))?;
            m.fill_path(&p, FillRule::Winding, true, Transform::identity());
            Some(m)
        });
    }

    fn fill_polys(&mut self, polys: &[Vec<P>], ink: Ink, mask: Option<&Mask>) {
        let Some(p) = paths_of(polys) else { return };
        let mut paint = Paint::default();
        paint.set_color(color(ink));
        paint.anti_alias = true;
        let m = mask.or(self.clip.as_ref());
        self.pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), m);
    }

    /// Flat fill of a region (marker wash, paper knock-out).
    pub fn fill(&mut self, region: &[Vec<P>], ink: Ink) {
        self.fill_polys(region, ink, None);
    }

    /// One pen stroke along an open polyline.
    pub fn stroke(&mut self, pts: &[P], pen: &Pen, seed: u64) {
        self.stroke_masked(pts, pen, seed, None)
    }

    fn stroke_masked(&mut self, pts: &[P], pen: &Pen, seed: u64, mask: Option<&Mask>) {
        if pts.len() < 2 || pen.w <= 0.0 {
            return;
        }
        for pass in 0..pen.passes.max(1) {
            let ps = mix(seed, 0x5eed + pass as u64);
            let (dx, dy) = if pass == 0 { (0.0, 0.0) } else { (rnds(ps, 1) * pen.w * 0.7, rnds(ps, 2) * pen.w * 0.7) };
            let shifted: Vec<P> = pts.iter().map(|p| (p.0 + dx, p.1 + dy)).collect();
            let pen_k = if pass == 0 { *pen } else { Pen { ink: pen.ink.alpha(0.5), w: pen.w * 0.75, ..*pen } };
            let segs: Vec<Vec<P>> = match pen.dash {
                Some((on, off)) => dashes(&shifted, on, off, ps),
                None => vec![shifted],
            };
            let ribbons: Vec<Vec<P>> = segs.iter().enumerate().filter_map(|(k, s)| ribbon(s, &pen_k, mix(ps, k as u64))).collect();
            self.fill_polys(&ribbons, pen_k.ink, mask);
        }
    }

    /// Closed outline (the end overlaps the start a little, as a hand would).
    pub fn stroke_closed(&mut self, poly: &[P], pen: &Pen, seed: u64) {
        if poly.len() < 3 {
            return;
        }
        let n = poly.len();
        let start = (rnd(seed, 77) * n as f64) as usize % n;
        let mut pts: Vec<P> = (0..=n).map(|i| poly[(start + i) % n]).collect();
        // overlap: continue a bit past the start
        let extra = resample(&[poly[start % n], poly[(start + 1) % n]], 2.0);
        let ov = (pen.overshoot * (0.6 + 0.8 * rnd(seed, 78))).max(2.0);
        let mut acc = 0.0;
        for w in extra.windows(2) {
            acc += dist(w[0], w[1]);
            if acc > ov {
                break;
            }
            pts.push(w[1]);
        }
        self.stroke(&pts, pen, seed);
    }

    /// A straight line that overshoots its end points.
    pub fn line(&mut self, a: P, b: P, pen: &Pen, seed: u64) {
        let l = dist(a, b).max(1e-6);
        let (ux, uy) = ((b.0 - a.0) / l, (b.1 - a.1) / l);
        let o0 = pen.overshoot * (0.2 + 0.8 * rnd(seed, 11)) * (l / 40.0).min(1.0);
        let o1 = pen.overshoot * (0.2 + 0.8 * rnd(seed, 12)) * (l / 40.0).min(1.0);
        let a2 = (a.0 - ux * o0, a.1 - uy * o0);
        let b2 = (b.0 + ux * o1, b.1 + uy * o1);
        // a slight bow
        let bow = rnds(seed, 13) * (l * 0.006).min(2.0);
        let mid = ((a2.0 + b2.0) / 2.0 - uy * bow, (a2.1 + b2.1) / 2.0 + ux * bow);
        let pts = hand::catmull_rom(&[a2, mid, b2], 8);
        self.stroke(&pts, pen, seed);
    }

    /// Hand-drawn rectangle: four separate strokes with overshoot.
    pub fn rect(&mut self, r: [f64; 4], pen: &Pen, seed: u64) {
        let j = |k: u64| rnds(seed, k) * pen.wobble * 0.8;
        let c = [(r[0] + j(1), r[1] + j(2)), (r[0] + r[2] + j(3), r[1] + j(4)), (r[0] + r[2] + j(5), r[1] + r[3] + j(6)), (r[0] + j(7), r[1] + r[3] + j(8))];
        for i in 0..4 {
            self.line(c[i], c[(i + 1) % 4], pen, mix(seed, 100 + i as u64));
        }
    }

    pub fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, pen: &Pen, seed: u64) {
        let n = ((rx + ry) * 0.5).clamp(12.0, 90.0) as usize;
        let a0 = rnd(seed, 3) * std::f64::consts::TAU;
        let over = 0.18 + 0.2 * rnd(seed, 4);
        let pts: Vec<P> = (0..=n + (n as f64 * over / std::f64::consts::TAU) as usize + 2).map(|i| {
            let a = a0 + i as f64 / n as f64 * std::f64::consts::TAU;
            let k = 1.0 + 0.025 * noise(seed, i as f64 / n as f64 * 5.0) + 0.012 * (i as f64 / n as f64);
            (cx + rx * k * a.cos(), cy + ry * k * a.sin())
        }).collect();
        self.stroke(&pts, pen, seed);
    }

    /// Parallel hatching clipped to a region (angle in degrees, spacing px).
    pub fn hatch(&mut self, region: &[Vec<P>], angle: f64, spacing: f64, pen: &Pen, seed: u64) {
        let Some(path) = paths_of(region) else { return };
        let mut mask = match &self.clip {
            Some(c) => c.clone(),
            None => {
                let Some(mut m) = Mask::new(self.w, self.h) else { return };
                m.data_mut().fill(255);
                m
            }
        };
        mask.intersect_path(&path, FillRule::Winding, true, Transform::identity());
        let pts: Vec<&P> = region.iter().flatten().collect();
        if pts.is_empty() {
            return;
        }
        let x0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        let x1 = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
        let y0 = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
        let y1 = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let r = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt() / 2.0 + 4.0;
        let a = angle.to_radians();
        let (ux, uy) = (a.cos(), a.sin());
        let (nx, ny) = (-uy, ux);
        let spacing = spacing.max(1.5);
        let n = (r / spacing).ceil() as i64;
        let p = Pen { overshoot: 0.0, passes: 1, ..*pen };
        for k in -n..=n {
            let d = k as f64 * spacing + rnds(seed, k as u64 + 900) * spacing * 0.25;
            let (px, py) = (cx + nx * d, cy + ny * d);
            // hatch strokes are quick: they start and stop a little early/late
            let l0 = r * (1.0 + 0.1 * rnds(seed, k as u64 + 300));
            let l1 = r * (1.0 + 0.1 * rnds(seed, k as u64 + 600));
            let line = resample(&[(px - ux * l0, py - uy * l0), (px + ux * l1, py + uy * l1)], 6.0);
            self.stroke_masked(&line, &p, mix(seed, k as u64 + 4000), Some(&mask));
        }
    }

    pub fn dot(&mut self, c: P, r: f64, ink: Ink) {
        self.fill(&[ellipse_poly(c.0, c.1, r, r, 8)], ink);
    }

    /// Hand lettering; `anchor` 0 = left, 0.5 = centre, 1 = right of (x, baseline y). Returns the width.
    pub fn text(&mut self, s: &str, x: f64, y: f64, cap: f64, pen: &Pen, seed: u64, anchor: f64) -> f64 {
        self.text_rot(s, x, y, cap, pen, seed, anchor, 0.0)
    }

    /// Lettering rotated by `rot` degrees about (x, y).
    #[allow(clippy::too_many_arguments)]
    pub fn text_rot(&mut self, s: &str, x: f64, y: f64, cap: f64, pen: &Pen, seed: u64, anchor: f64, rot: f64) -> f64 {
        let u = cap / 6.0;
        let wu = hand::width_units(s);
        let w = wu * u;
        let (sr, cr) = rot.to_radians().sin_cos();
        let place = |px: f64, py: f64| -> P {
            // local (px, py) relative to the text origin; baseline at py = 0
            (x + px * cr - py * sr, y + px * sr + py * cr)
        };
        let mut cx = -anchor * w;
        let mut first = true;
        let slant = 0.07;
        let pen = Pen { w: (cap * 0.105).clamp(0.9, 4.5).min(pen.w.max(0.9)), overshoot: 0.0, ..*pen };
        for (i, c) in s.chars().enumerate() {
            if c == ' ' {
                cx += hand::SPACE * u;
                first = true;
                continue;
            }
            let Some((adv, strokes)) = hand::glyph(c).or_else(|| hand::glyph('?')) else { continue };
            if !first {
                cx += hand::GAP * u;
            }
            first = false;
            let gs = mix(seed, i as u64 + 1);
            let sc = 1.0 + 0.06 * rnds(gs, 1);
            let base = 0.045 * cap * rnds(gs, 2);
            let gr = (2.2 * rnds(gs, 3)).to_radians();
            let (gsn, gcs) = gr.sin_cos();
            let ox = cx;
            for (k, st) in strokes.iter().enumerate() {
                let pts: Vec<P> = st.iter().map(|&(gx, gy)| {
                    // glyph space: origin at baseline-left, y up negative
                    let lx = gx * u * sc;
                    let ly = (gy - 6.0) * u * sc;
                    let lx = lx - ly * slant;
                    let (rx, ry) = (lx * gcs - ly * gsn, lx * gsn + ly * gcs);
                    place(ox + rx, ry + base)
                }).collect();
                if pts.len() == 2 && dist(pts[0], pts[1]) < 1.2 * u {
                    // dots: a round dab
                    let c = ((pts[0].0 + pts[1].0) / 2.0, (pts[0].1 + pts[1].1) / 2.0);
                    self.dot(c, (pen.w * 0.8).max(0.9), pen.ink);
                    continue;
                }
                let pts = resample(&pts, (u * 0.8).max(1.0));
                self.stroke(&pts, &pen, mix(gs, 50 + k as u64));
            }
            cx += adv * u;
        }
        w
    }

    /// A storyboard "fat arrow": a hollow ribbon along the path with a triangular head,
    /// filled with `fill` and outlined with `pen`.
    pub fn fat_arrow(&mut self, path: &[P], width: f64, fill: Ink, pen: &Pen, seed: u64) {
        let path = resample(path, 3.0);
        if path.len() < 2 {
            return;
        }
        let total = poly_len(&path);
        let head_len = (width * 2.0).min(total * 0.55);
        let head_w = width * 2.1;
        // body ends where the head starts
        let body = trim(&path, 0.0, (total - head_len * 0.85) / total);
        if body.len() < 2 {
            return;
        }
        let n = body.len();
        let mut left = vec![];
        let mut right = vec![];
        let bl = poly_len(&body);
        let mut s = 0.0;
        for i in 0..n {
            if i > 0 {
                s += dist(body[i - 1], body[i]);
            }
            let (a, b) = (body[i.saturating_sub(1)], body[(i + 1).min(n - 1)]);
            let l = dist(a, b).max(1e-6);
            let (nx, ny) = (-(b.1 - a.1) / l, (b.0 - a.0) / l);
            // tail narrower than the neck
            let hw = width * 0.5 * (0.45 + 0.55 * (s / bl.max(1e-6)).min(1.0).sqrt());
            left.push((body[i].0 + nx * hw, body[i].1 + ny * hw));
            right.push((body[i].0 - nx * hw, body[i].1 - ny * hw));
        }
        let tip = *path.last().unwrap();
        let neck = *body.last().unwrap();
        let l = dist(neck, tip).max(1e-6);
        let (ux, uy) = ((tip.0 - neck.0) / l, (tip.1 - neck.1) / l);
        let (nx, ny) = (-uy, ux);
        let back = (tip.0 - ux * head_len, tip.1 - uy * head_len);
        let h1 = (back.0 + nx * head_w / 2.0, back.1 + ny * head_w / 2.0);
        let h2 = (back.0 - nx * head_w / 2.0, back.1 - ny * head_w / 2.0);
        let mut outline: Vec<P> = left.clone();
        outline.push(h1);
        outline.push(tip);
        outline.push(h2);
        outline.extend(right.iter().rev());
        // a soft drop shadow, then the fill, then the outline in pieces (as drawn by hand)
        let shadow: Vec<P> = outline.iter().map(|p| (p.0 + 2.0, p.1 + 2.5)).collect();
        self.fill(&[shadow], Ink { a: 0.18, ..INK });
        self.fill(&[outline.clone()], fill);
        let mut l_side = left.clone();
        l_side.push(h1);
        l_side.push(tip);
        let mut r_side = right.clone();
        r_side.push(h2);
        r_side.push(tip);
        self.stroke(&l_side, pen, mix(seed, 1));
        self.stroke(&r_side, pen, mix(seed, 2));
        self.line(left[0], right[0], &Pen { overshoot: 1.0, ..*pen }, mix(seed, 3));
    }

    /// A thin open arrow (pencil line plus a two-stroke head).
    pub fn thin_arrow(&mut self, path: &[P], pen: &Pen, head: f64, seed: u64) {
        if path.len() < 2 {
            return;
        }
        self.stroke(path, pen, seed);
        let tip = *path.last().unwrap();
        // direction from a point ~head back along the path
        let back = trim(path, (1.0 - head * 1.5 / poly_len(path).max(1e-6)).max(0.0), 1.0);
        let from = back.first().copied().unwrap_or(path[path.len() - 2]);
        let l = dist(from, tip).max(1e-6);
        let (ux, uy) = ((tip.0 - from.0) / l, (tip.1 - from.1) / l);
        for (k, sgn) in [(1u64, 1.0), (2, -1.0)] {
            let a = (25.0f64 + 6.0 * rnds(seed, k)).to_radians() * sgn;
            let (c, s) = (a.cos(), a.sin());
            let (dx, dy) = (-(ux * c - uy * s), -(ux * s + uy * c));
            let end = (tip.0 + dx * head, tip.1 + dy * head);
            self.stroke(&[tip, end], &Pen { taper: 0.8, ..*pen }, mix(seed, 10 + k));
        }
    }

    pub fn save_png(&self, path: &std::path::Path) -> anyhow::Result<()> {
        self.pm.save_png(path).map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))
    }
}

fn color(i: Ink) -> Color {
    Color::from_rgba(i.r.clamp(0.0, 1.0), i.g.clamp(0.0, 1.0), i.b.clamp(0.0, 1.0), i.a.clamp(0.0, 1.0)).unwrap_or(Color::BLACK)
}

fn dashes(p: &[P], on: f64, off: f64, seed: u64) -> Vec<Vec<P>> {
    let l = poly_len(p);
    if l <= 0.0 {
        return vec![];
    }
    let mut out = vec![];
    let mut s = rnd(seed, 9) * off;
    let mut k = 0u64;
    while s < l {
        let len = on * (0.8 + 0.4 * rnd(seed, 100 + k));
        let seg = trim(p, s / l, ((s + len) / l).min(1.0));
        if seg.len() >= 2 {
            out.push(seg);
        }
        s += len + off * (0.8 + 0.4 * rnd(seed, 200 + k));
        k += 1;
    }
    out
}

/// The filled outline of a wobbly, tapered stroke along `pts`.
fn ribbon(pts: &[P], pen: &Pen, seed: u64) -> Option<Vec<P>> {
    let rs = resample(pts, 2.0);
    let total = poly_len(&rs);
    if rs.len() < 2 || total < 0.3 {
        // a dab
        let c = pts.first()?;
        return Some(ellipse_poly(c.0, c.1, pen.w * 0.55, pen.w * 0.5, 8));
    }
    let n = rs.len();
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    let mut s = 0.0;
    let tl = (pen.w * 3.0 + 3.0).min(total / 2.0).max(0.5);
    for i in 0..n {
        if i > 0 {
            s += dist(rs[i - 1], rs[i]);
        }
        let (a, b) = (rs[i.saturating_sub(1)], rs[(i + 1).min(n - 1)]);
        let l = dist(a, b).max(1e-9);
        let (nx, ny) = (-(b.1 - a.1) / l, (b.0 - a.0) / l);
        let wob = noise(seed, s / 26.0) * pen.wobble + noise(seed ^ 0x77, s / 6.5) * pen.wobble * 0.22;
        let e = s.min(total - s);
        let k = (e / tl).clamp(0.0, 1.0);
        let tf = 1.0 - pen.taper * (1.0 - k.sqrt());
        let pressure = 1.0 + 0.2 * noise(seed ^ 0x33, s / 37.0);
        let hw = (pen.w * 0.5 * tf * pressure).max(0.18);
        let c = (rs[i].0 + nx * wob, rs[i].1 + ny * wob);
        left.push((c.0 + nx * hw, c.1 + ny * hw));
        right.push((c.0 - nx * hw, c.1 - ny * hw));
    }
    left.extend(right.into_iter().rev());
    Some(left)
}
