//! Instanced point clouds (`Content::Cloud`): N points evaluated per frame from a compact
//! description (formations, per-point stagger, easing, seeded drift, optional projection)
//! and drawn by a splat routine into one premultiplied buffer. One paint unit for the
//! planner, whatever N is.
//!
//! - Load: every form's slots are generated (seeded), points are matched to slots
//!   (Hilbert order of source and target by default: neighbours stay neighbours), colors
//!   assigned, per-point start times computed.
//! - Frame (or motion-blur sample) at time t: point i starts at its last fully arrived form
//!   and blends towards every later form it has started (eased progress), so overlapping
//!   morphs never jump. Positions, size, opacity and color blend the same way.
//! - Schedule: conservative bounds per time from per-form boxes (no per-point work), and a
//!   signature that is constant while the cloud rests.
use anyhow::{Context, Result};
use agentfx_ir::{CloudContent, CloudShape, CloudText, CloudView, Easing};
use std::f64::consts::{PI, TAU};

// ---------------------------------------------------------------- deterministic randomness

/// SplitMix64 finalizer of (seed, a, b): uniform u64.
pub fn hash3(seed: u64, a: u64, b: u64) -> u64 {
    let mut z = seed ^ a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
/// Uniform in [0, 1).
pub fn rnd(seed: u64, a: u64, b: u64) -> f64 {
    (hash3(seed, a, b) >> 11) as f64 / (1u64 << 53) as f64
}
/// Standard normal (Box-Muller).
fn gauss(seed: u64, a: u64, b: u64) -> f64 {
    let u = rnd(seed, a, b * 2 + 1).max(1e-12);
    let v = rnd(seed, a, b * 2 + 2);
    (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
}

/// Hilbert index of (x, y) in a 1024 x 1024 grid.
pub fn hilbert(x: u32, y: u32) -> u32 {
    let (mut x, mut y) = (x.min(1023), y.min(1023));
    let mut d = 0u32;
    let mut s = 512u32;
    while s > 0 {
        let rx = u32::from(x & s != 0);
        let ry = u32::from(y & s != 0);
        d += s * s * ((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                x = 1023 - x;
                y = 1023 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    d
}

// ---------------------------------------------------------------- easing LUT

/// Easing sampled once (springs are costly to set up per call); steps stay exact.
struct EaseLut {
    exact: Option<Easing>,
    lut: Vec<f32>,
    lo: f64,
    hi: f64,
}
impl EaseLut {
    const N: usize = 1024;
    fn new(e: &Easing) -> EaseLut {
        let f = |u: f64| agentfx_ir::timing::ease(e, u, false);
        if matches!(e, Easing::Steps(..)) {
            return EaseLut { exact: Some(*e), lut: vec![], lo: 0.0, hi: 1.0 };
        }
        let lut: Vec<f32> = (0..=Self::N).map(|i| f(i as f64 / Self::N as f64) as f32).collect();
        let lo = lut.iter().copied().fold(0.0f32, f32::min) as f64;
        let hi = lut.iter().copied().fold(1.0f32, f32::max) as f64;
        EaseLut { exact: None, lut, lo, hi }
    }
    #[inline]
    fn at(&self, u: f64) -> f64 {
        if u <= 0.0 {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        if let Some(e) = &self.exact {
            return agentfx_ir::timing::ease(e, u, false);
        }
        let x = u * Self::N as f64;
        let i = x as usize;
        let f = x - i as f64;
        let a = self.lut[i] as f64;
        let b = self.lut[(i + 1).min(Self::N)] as f64;
        a + (b - a) * f
    }
}

// ---------------------------------------------------------------- forms

#[derive(Clone, Copy)]
struct Drift {
    amp: f64,
    k: f64,
    w: f64,
    ph: [f64; 3],
    dir: [[f64; 2]; 3],
}
impl Drift {
    fn new(d: &agentfx_ir::CloudDrift, seed: u64, form: u64) -> Drift {
        let ph = [0, 1, 2].map(|j| TAU * rnd(seed, 0xD1F7 + form, j));
        let dir = [0, 1, 2].map(|j| {
            let a = TAU * rnd(seed, 0xD1F8 + form, j);
            [a.cos(), a.sin()]
        });
        Drift { amp: d.amp, k: TAU / d.scale, w: TAU * d.speed, ph, dir }
    }
    /// Divergence-free displacement (curl of a sum of three plane waves), |d| <~ amp.
    #[inline]
    fn at(&self, x: f64, y: f64, t: f64) -> [f64; 3] {
        let tau = self.w * t;
        let k = self.k;
        let [d0, d1, d2] = self.dir;
        let a = k * (d0[0] * x + d0[1] * y) + tau + self.ph[0];
        let b = 1.3 * k * (d1[0] * x + d1[1] * y) - 0.8 * tau + self.ph[1];
        let g = 2.1 * k * (d2[0] * x + d2[1] * y) + 1.3 * tau + self.ph[2];
        let (sa, ca) = a.sin_cos();
        let (sb, cb) = b.sin_cos();
        let cg = g.cos();
        // psi = sin(a) cos(b) + 0.5 sin(g); grad psi:
        let px = ca * cb * k * d0[0] - sa * sb * 1.3 * k * d1[0] + 0.5 * cg * 2.1 * k * d2[0];
        let py = ca * cb * k * d0[1] - sa * sb * 1.3 * k * d1[1] + 0.5 * cg * 2.1 * k * d2[1];
        // curl (py, -px), normalized by the largest wave number term
        let s = self.amp / (2.2 * k);
        [py * s, -px * s, self.amp * 0.5 * sa * cb]
    }
}

enum Slots {
    Static(Vec<[f32; 3]>),
    Field { base: Vec<[f32; 3]>, rect: [f64; 4], flow: [f64; 2] },
    /// (r, theta, ox, oy, oz)
    Spiral { p: Vec<[f32; 5]>, c: [f64; 2], squash: f64, spin: f64, r0: f64 },
    Sphere { d: Vec<[f32; 3]>, c: [f64; 3], radius: f64, spin: f64 },
    /// (x, depth fraction 0..1)
    Terrain { g: Vec<[f32; 2]>, rect: [f64; 4], deep: bool, amp: f64, k: f64, w: f64, ph: [f64; 3] },
}

struct Form {
    slots: Slots,
    drift: Option<Drift>,
    dur: f64,
    ease: EaseLut,
    size: f64,
    opacity: f64,
    /// earliest / latest per-point start, latest arrival
    min_start: f64,
    max_start: f64,
    max_end: f64,
    /// positions depend on time (drift, flow, spin, waves)
    dynamic: bool,
    /// conservative 3D box of the slots over all times: [x0, y0, z0, x1, y1, z1]
    bbox: [f64; 6],
}

impl Form {
    #[inline]
    fn slot(&self, j: usize, t: f64) -> [f64; 3] {
        let p = match &self.slots {
            Slots::Static(v) => {
                let q = v[j];
                [q[0] as f64, q[1] as f64, q[2] as f64]
            }
            Slots::Field { base, rect, flow } => {
                let q = base[j];
                let mut x = q[0] as f64;
                let mut y = q[1] as f64;
                if flow[0] != 0.0 {
                    x = rect[0] + (x - rect[0] + flow[0] * t).rem_euclid(rect[2]);
                }
                if flow[1] != 0.0 {
                    y = rect[1] + (y - rect[1] + flow[1] * t).rem_euclid(rect[3]);
                }
                [x, y, q[2] as f64]
            }
            Slots::Spiral { p, c, squash, spin, r0 } => {
                let q = p[j];
                let r = q[0] as f64;
                let a = q[1] as f64 + spin * r0 / (r + r0) * t;
                let (s, co) = a.sin_cos();
                [c[0] + r * co + q[2] as f64, c[1] + squash * (r * s + q[3] as f64), q[4] as f64]
            }
            Slots::Sphere { d, c, radius, spin } => {
                let q = d[j];
                let (s, co) = (spin * t).sin_cos();
                let (x, z) = (q[0] as f64 * co - q[2] as f64 * s, q[0] as f64 * s + q[2] as f64 * co);
                [c[0] + radius * x, c[1] + radius * q[1] as f64, c[2] + radius * z]
            }
            Slots::Terrain { g, rect, deep, amp, k, w, ph } => {
                let q = g[j];
                let x = q[0] as f64;
                let f = q[1] as f64;
                let depth = f * rect[3];
                let tau = w * t;
                let h = 0.55 * (x * k + tau + ph[0]).sin() * (depth * k * 0.9 - 0.7 * tau + ph[1]).cos()
                    + 0.3 * ((0.5 * x - depth) * k * 1.4 - 1.4 * tau + ph[2]).sin()
                    + 0.15 * (((x - rect[0] - rect[2] / 2.0).hypot(depth - rect[3] / 2.0)) * k * 1.9 - 1.9 * tau).cos();
                if *deep {
                    [x, rect[1] - amp * h, -depth]
                } else {
                    [x, rect[1] + rect[3] - depth - amp * h, 0.0]
                }
            }
        };
        match &self.drift {
            Some(d) => {
                let o = d.at(p[0], p[1], t);
                [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
            }
            None => p,
        }
    }
}

fn bbox_of(pts: impl Iterator<Item = [f64; 3]>) -> [f64; 6] {
    let mut b = [f64::MAX, f64::MAX, f64::MAX, f64::MIN, f64::MIN, f64::MIN];
    for p in pts {
        for a in 0..3 {
            b[a] = b[a].min(p[a]);
            b[a + 3] = b[a + 3].max(p[a]);
        }
    }
    if b[0] > b[3] { [0.0; 6] } else { b }
}
fn grow(b: [f64; 6], m: [f64; 3]) -> [f64; 6] {
    [b[0] - m[0], b[1] - m[1], b[2] - m[2], b[3] + m[0], b[4] + m[1], b[5] + m[2]]
}
fn union6(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2]), a[3].max(b[3]), a[4].max(b[4]), a[5].max(b[5])]
}
/// Box of lerp(a, b, e) for a in A, b in B, e in [lo, hi] (bilinear: extremes at corners).
fn lerp_box(a: [f64; 6], b: [f64; 6], lo: f64, hi: f64) -> [f64; 6] {
    let mut o = [f64::MAX, f64::MAX, f64::MAX, f64::MIN, f64::MIN, f64::MIN];
    for ax in 0..3 {
        for e in [lo, hi, 0.0, 1.0] {
            for x in [a[ax], a[ax + 3]] {
                for y in [b[ax], b[ax + 3]] {
                    let v = x + (y - x) * e;
                    o[ax] = o[ax].min(v);
                    o[ax + 3] = o[ax + 3].max(v);
                }
            }
        }
    }
    o
}

/// Text sampler: N slot positions inside (or on the outline of) shaped text.
pub type TextSampler<'a> = dyn FnMut(&CloudText, usize, u64) -> Result<Vec<[f64; 2]>> + 'a;

fn gen_slots(shape: &CloudShape, n: usize, seed: u64, k: u64, text: &mut TextSampler) -> Result<(Slots, bool, [f64; 6])> {
    let s = seed ^ (k.wrapping_mul(0xA24B_AED4_963E_E407));
    Ok(match shape {
        CloudShape::Field { rect, depth, flow } => {
            let base: Vec<[f32; 3]> = (0..n as u64).map(|j| [(rect[0] + rnd(s, j, 0) * rect[2]) as f32, (rect[1] + rnd(s, j, 1) * rect[3]) as f32, ((rnd(s, j, 2) - 0.5) * depth) as f32]).collect();
            let b = [rect[0], rect[1], -depth / 2.0, rect[0] + rect[2], rect[1] + rect[3], depth / 2.0];
            (Slots::Field { base, rect: *rect, flow: *flow }, flow[0] != 0.0 || flow[1] != 0.0, b)
        }
        CloudShape::Grid { rect, cols, jitter } => {
            let cols = if *cols > 0 { *cols as usize } else { ((n as f64 * rect[2] / rect[3]).sqrt().round() as usize).max(1) };
            let rows = n.div_ceil(cols);
            let (cw, ch) = (rect[2] / cols as f64, rect[3] / rows as f64);
            let v: Vec<[f32; 3]> = (0..n)
                .map(|j| {
                    let (c, r) = ((j % cols) as f64, (j / cols) as f64);
                    let jx = jitter * (rnd(s, j as u64, 0) - 0.5) * cw;
                    let jy = jitter * (rnd(s, j as u64, 1) - 0.5) * ch;
                    [(rect[0] + (c + 0.5) * cw + jx) as f32, (rect[1] + (r + 0.5) * ch + jy) as f32, 0.0]
                })
                .collect();
            let b = bbox_of(v.iter().map(|p| [p[0] as f64, p[1] as f64, 0.0]));
            (Slots::Static(v), false, b)
        }
        CloudShape::Spiral { center, radius, arms, twist, spread, squash, spin } => {
            let rin = 0.03 * radius;
            let p: Vec<[f32; 5]> = (0..n as u64)
                .map(|j| {
                    let arm = (j % *arms as u64) as f64;
                    let u = rnd(s, j, 0).powf(0.95);
                    let r = rin + u * (radius - rin);
                    let th = arm * TAU / *arms as f64 + (r / rin).ln() * twist + 0.08 * gauss(s, j, 1);
                    let sc = spread * (0.12 * radius + r);
                    [r as f32, th as f32, (sc * gauss(s, j, 2)) as f32, (sc * gauss(s, j, 3)) as f32, (0.5 * sc * gauss(s, j, 4)) as f32]
                })
                .collect();
            let ext = p.iter().map(|q| (q[0] as f64) + (q[2] as f64).hypot(q[3] as f64)).fold(0.0, f64::max);
            let ez = p.iter().map(|q| (q[4] as f64).abs()).fold(0.0, f64::max);
            let b = [center[0] - ext, center[1] - ext * squash.abs(), -ez, center[0] + ext, center[1] + ext * squash.abs(), ez];
            (Slots::Spiral { p, c: *center, squash: *squash, spin: spin.to_radians(), r0: 0.15 * radius }, *spin != 0.0, b)
        }
        CloudShape::Gaussian { center, sigma } => {
            let v: Vec<[f32; 3]> = (0..n as u64).map(|j| [0, 1, 2].map(|a| (center[a] + sigma[a] * gauss(s, j, a as u64)) as f32)).collect();
            let b = bbox_of(v.iter().map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]));
            (Slots::Static(v), false, b)
        }
        CloudShape::Sphere { center, radius, spin } => {
            // Fibonacci lattice: even, deterministic
            let ga = PI * (3.0 - 5f64.sqrt());
            let d: Vec<[f32; 3]> = (0..n)
                .map(|j| {
                    let y = 1.0 - 2.0 * (j as f64 + 0.5) / n as f64;
                    let r = (1.0 - y * y).max(0.0).sqrt();
                    let a = ga * j as f64;
                    [(r * a.cos()) as f32, y as f32, (r * a.sin()) as f32]
                })
                .collect();
            let b = [center[0] - radius, center[1] - radius, center[2] - radius, center[0] + radius, center[1] + radius, center[2] + radius];
            (Slots::Sphere { d, c: *center, radius: *radius, spin: spin.to_radians() }, *spin != 0.0, b)
        }
        CloudShape::Terrain { rect, rows, amp, scale, speed, deep } => {
            let rows = if *rows > 0 { *rows as usize } else { ((n as f64 / 2.0).sqrt().round() as usize).max(1) };
            let cols = n.div_ceil(rows);
            let g: Vec<[f32; 2]> = (0..n)
                .map(|j| {
                    let (c, r) = (j % cols, j / cols);
                    let x = rect[0] + (c as f64 + 0.5) / cols as f64 * rect[2] + (rnd(s, j as u64, 0) - 0.5) * 0.3 * rect[2] / cols as f64;
                    let f = (r as f64 + 0.5 + (rnd(s, j as u64, 1) - 0.5) * 0.3) / rows as f64;
                    [x as f32, f as f32]
                })
                .collect();
            let ph = [0, 1, 2].map(|j| TAU * rnd(s, 0x7E44, j));
            let b = if *deep {
                [rect[0], rect[1] - amp, -rect[3], rect[0] + rect[2], rect[1] + amp, 0.0]
            } else {
                [rect[0], rect[1] - amp, 0.0, rect[0] + rect[2], rect[1] + rect[3] + amp, 0.0]
            };
            (Slots::Terrain { g, rect: *rect, deep: *deep, amp: *amp, k: TAU / scale, w: TAU * speed, ph }, *speed != 0.0, b)
        }
        CloudShape::Text(t) => {
            let mut pts = text(t, n, s).with_context(|| format!("text form {:?}", t.text))?;
            // reading order (left to right) for index staggers
            pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
            let v: Vec<[f32; 3]> = pts.iter().map(|p| [p[0] as f32, p[1] as f32, 0.0]).collect();
            let b = bbox_of(v.iter().map(|p| [p[0] as f64, p[1] as f64, 0.0]));
            (Slots::Static(v), false, b)
        }
        CloudShape::Points { points } => {
            let v: Vec<[f32; 3]> = (0..n).map(|j| {
                let p = &points[j % points.len()];
                [p[0] as f32, p[1] as f32, p.get(2).copied().unwrap_or(0.0) as f32]
            }).collect();
            let b = bbox_of(v.iter().map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]));
            (Slots::Static(v), false, b)
        }
    })
}

// ---------------------------------------------------------------- the cloud

/// One evaluated point (content space; `w` = projective depth, 1 without projection).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
    /// radius (content px; projection scale applied)
    pub r: f32,
    /// straight color and alpha (opacity x palette alpha)
    pub c: [f32; 3],
    pub a: f32,
    pub w: f32,
}

pub struct Cloud {
    pub n: usize,
    radius: f64,
    rfac: Vec<f32>,
    palette: Vec<[f32; 4]>,
    forms: Vec<Form>,
    /// per form, per point: slot index, start time, palette index
    slot: Vec<Vec<u32>>,
    start: Vec<Vec<f32>>,
    col: Vec<Vec<u8>>,
    proj: Vec<CloudView>,
    max_size: f64,
    /// added to the evaluation time (`CloudContent::time_offset`)
    toff: f64,
}

fn split_counts(n: usize, split: &[f64], ncol: usize) -> Vec<usize> {
    let w: Vec<f64> = if split.is_empty() { vec![1.0; ncol] } else { split.to_vec() };
    let total: f64 = w.iter().sum::<f64>().max(1e-12);
    let mut c: Vec<usize> = w.iter().map(|x| (x / total * n as f64).floor() as usize).collect();
    let mut rem = n - c.iter().sum::<usize>();
    // largest remainder
    let mut order: Vec<usize> = (0..w.len()).collect();
    order.sort_by(|&a, &b| {
        let fa = w[a] / total * n as f64 - c[a] as f64;
        let fb = w[b] / total * n as f64 - c[b] as f64;
        fb.total_cmp(&fa).then(a.cmp(&b))
    });
    for &i in order.iter().cycle() {
        if rem == 0 {
            break;
        }
        c[i] += 1;
        rem -= 1;
    }
    c
}

/// Palette index per slot from a color rule.
fn color_slots(rule: &agentfx_ir::CloudColor, pos: &[[f64; 3]], seed: u64, k: u64, ncol: usize) -> Vec<u8> {
    let n = pos.len();
    let b = bbox_of(pos.iter().copied());
    let c = rule.center.unwrap_or([(b[0] + b[3]) / 2.0, (b[1] + b[4]) / 2.0]);
    let (ds, dc) = rule.angle.to_radians().sin_cos();
    let key = |j: usize| -> f64 {
        let p = pos[j];
        match rule.by.as_str() {
            "index" => j as f64,
            "x" => p[0],
            "y" => p[1],
            "z" => p[2],
            "distance" => (p[0] - c[0]).hypot(p[1] - c[1]),
            "angle" => (p[1] - c[1]).atan2(p[0] - c[0]),
            "dir" => {
                let along = p[0] * dc + p[1] * ds;
                let across = -p[0] * ds + p[1] * dc;
                along + rule.wave.map(|w| w[0] * (across * TAU / w[1].max(1.0)).sin()).unwrap_or(0.0)
            }
            _ => rnd(seed, 0xC010 + k, j as u64),
        }
    };
    let keys: Vec<f64> = (0..n).map(key).collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| keys[a].total_cmp(&keys[b]).then(a.cmp(&b)));
    let counts = split_counts(n, &rule.split, if rule.split.is_empty() { ncol } else { rule.split.len() });
    let mut out = vec![0u8; n];
    let mut it = order.into_iter();
    for (ci, &cnt) in counts.iter().enumerate() {
        for j in it.by_ref().take(cnt) {
            out[j] = ci as u8;
        }
    }
    out
}

/// Sort keys for matching: Hilbert index of 2D positions, each set normalized to its own
/// box (a moved or rescaled copy of a set maps onto itself; left goes to left).
fn curve_keys(v: &[[f64; 2]]) -> Vec<u32> {
    let mut bb = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in v {
        bb = [bb[0].min(p[0]), bb[1].min(p[1]), bb[2].max(p[0]), bb[3].max(p[1])];
    }
    let (sx, sy) = ((bb[2] - bb[0]).max(1e-6), (bb[3] - bb[1]).max(1e-6));
    v.iter().map(|p| hilbert(((p[0] - bb[0]) / sx * 1023.0) as u32, ((p[1] - bb[1]) / sy * 1023.0) as u32)).collect()
}

/// Local improvement of a matching (sources in curve order `a`, target per source `out`):
/// swap targets of nearby sources when that lowers the summed squared travel (removes most
/// crossings; squared costs favour parallel paths).
fn refine(src: &[[f64; 2]], dst: &[[f64; 2]], a: &[usize], out: &mut [usize]) {
    let d2 = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]) * (p[0] - q[0]) + (p[1] - q[1]) * (p[1] - q[1]);
    let n = a.len();
    for pass in 0..6 {
        let win = [4, 16, 64, 16, 64, 8][pass];
        let mut swaps = 0;
        for x in 0..n {
            let i = a[x];
            for y in x + 1..(x + win).min(n) {
                let j = a[y];
                let now = d2(src[i], dst[out[i]]) + d2(src[j], dst[out[j]]);
                let alt = d2(src[i], dst[out[j]]) + d2(src[j], dst[out[i]]);
                if alt < now - 1e-9 {
                    out.swap(i, j);
                    swaps += 1;
                }
            }
        }
        if swaps == 0 {
            break;
        }
    }
}

/// Match sources (points) to targets (slots) of equal count; returns target per source.
fn assign(mode: &str, src: &[[f64; 2]], dst: &[[f64; 2]], seed: u64, k: u64) -> Vec<usize> {
    let n = src.len();
    let mut a: Vec<usize> = (0..n).collect();
    let mut b: Vec<usize> = (0..n).collect();
    let curve = !matches!(mode, "index" | "random" | "angle");
    match mode {
        "index" => {}
        "random" => {
            let r: Vec<u64> = (0..n as u64).map(|j| hash3(seed, 0xA551 + k, j)).collect();
            b.sort_by_key(|&j| (r[j], j));
        }
        "angle" => {
            let cen = |v: &[[f64; 2]]| {
                let s = v.iter().fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
                [s[0] / v.len().max(1) as f64, s[1] / v.len().max(1) as f64]
            };
            let (ca, cb) = (cen(src), cen(dst));
            let ang = |p: &[f64; 2], c: [f64; 2]| ((p[1] - c[1]).atan2(p[0] - c[0]), (p[0] - c[0]).hypot(p[1] - c[1]));
            a.sort_by(|&x, &y| ang(&src[x], ca).partial_cmp(&ang(&src[y], ca)).unwrap_or(std::cmp::Ordering::Equal).then(x.cmp(&y)));
            b.sort_by(|&x, &y| ang(&dst[x], cb).partial_cmp(&ang(&dst[y], cb)).unwrap_or(std::cmp::Ordering::Equal).then(x.cmp(&y)));
        }
        _ => {
            let (ka, kb) = (curve_keys(src), curve_keys(dst));
            a.sort_by_key(|&j| (ka[j], j));
            b.sort_by_key(|&j| (kb[j], j));
        }
    }
    let mut out = vec![0usize; n];
    for (&x, &y) in a.iter().zip(&b) {
        out[x] = y;
    }
    if curve {
        refine(src, dst, &a, &mut out);
    }
    out
}

impl Cloud {
    pub fn new(c: &CloudContent, text: &mut TextSampler) -> Result<Cloud> {
        let n = c.count as usize;
        let seed = c.seed;
        let ncol = c.palette.len().max(1);
        let proj = c.projection.clone();
        let rfac: Vec<f32> = (0..n as u64).map(|i| (1.0 + c.radius_jitter * (2.0 * rnd(seed, 0x5123, i) - 1.0)) as f32).collect();
        let mut forms: Vec<Form> = Vec::with_capacity(c.forms.len());
        let mut slot: Vec<Vec<u32>> = vec![];
        let mut start: Vec<Vec<f32>> = vec![];
        let mut col: Vec<Vec<u8>> = vec![];
        // screen-ish 2D position for matching (projected when there is a projection)
        let view = |p: [f64; 3], t: f64| -> [f64; 2] {
            if proj.is_empty() {
                [p[0], p[1]]
            } else {
                let m = proj_at(&proj, t);
                let w = m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11];
                let w = if w.abs() < 1e-6 { 1e-6 } else { w };
                [(m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3]) / w, (m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7]) / w]
            }
        };
        let mut prev_pos: Vec<[f64; 2]> = vec![];
        let mut prev_col: Vec<u8> = vec![];
        let mut prev_start: Vec<f32> = vec![];
        for (k, f) in c.forms.iter().enumerate() {
            let (slots, moving, bbox) = gen_slots(&f.shape, n, seed, k as u64, text)?;
            let drift = f.drift.as_ref().filter(|d| d.amp != 0.0).map(|d| Drift::new(d, seed, k as u64));
            let dm = drift.map(|d| d.amp * 1.2).unwrap_or(0.0);
            let mut form = Form {
                slots,
                drift,
                dur: if k == 0 { 0.0 } else { f.dur },
                ease: EaseLut::new(&f.easing),
                size: f.size,
                opacity: f.opacity,
                min_start: 0.0,
                max_start: 0.0,
                max_end: 0.0,
                dynamic: moving || drift.is_some(),
                bbox: grow(bbox, [dm, dm, dm * 0.6]),
            };
            let t_arrive = if k == 0 { 0.0 } else { f.at + f.dur };
            let pos3: Vec<[f64; 3]> = (0..n).map(|j| form.slot(j, t_arrive)).collect();
            let pos2: Vec<[f64; 2]> = pos3.iter().map(|p| view(*p, t_arrive)).collect();
            // slot colors
            let slot_col: Option<Vec<u8>> = match &f.color {
                Some(rule) => Some(color_slots(rule, &pos3, seed, k as u64, ncol)),
                None if k == 0 => Some(if c.weights.is_empty() {
                    vec![0u8; n]
                } else {
                    color_slots(&agentfx_ir::CloudColor { by: "random".into(), split: c.weights.clone(), angle: 0.0, center: None, wave: None }, &pos3, seed, 0, ncol)
                }),
                None => None,
            };
            // matching
            let (sl, cl): (Vec<u32>, Vec<u8>) = if k == 0 {
                ((0..n as u32).collect(), slot_col.clone().unwrap())
            } else {
                let mut tgt = vec![0u32; n];
                let mut grouped = false;
                if let Some(sc) = &slot_col {
                    // color-preserving groups when the counts agree
                    let cnt = |v: &[u8]| {
                        let mut h = vec![0usize; 256];
                        v.iter().for_each(|&x| h[x as usize] += 1);
                        h
                    };
                    if cnt(sc) == cnt(&prev_col) {
                        grouped = true;
                        for ci in 0..ncol.min(256) as u8 {
                            let srcs: Vec<usize> = (0..n).filter(|&i| prev_col[i] == ci).collect();
                            let dsts: Vec<usize> = (0..n).filter(|&j| sc[j] == ci).collect();
                            if srcs.is_empty() {
                                continue;
                            }
                            let a: Vec<[f64; 2]> = srcs.iter().map(|&i| prev_pos[i]).collect();
                            let b: Vec<[f64; 2]> = dsts.iter().map(|&j| pos2[j]).collect();
                            let m = assign(&f.assign, &a, &b, seed, k as u64);
                            for (x, &i) in srcs.iter().enumerate() {
                                tgt[i] = dsts[m[x]] as u32;
                            }
                        }
                    }
                }
                if !grouped {
                    let m = assign(&f.assign, &prev_pos, &pos2, seed, k as u64);
                    for i in 0..n {
                        tgt[i] = m[i] as u32;
                    }
                }
                let cl: Vec<u8> = match &slot_col {
                    Some(sc) => (0..n).map(|i| sc[tgt[i] as usize]).collect(),
                    None => prev_col.clone(),
                };
                (tgt, cl)
            };
            // per-point start times
            let st: Vec<f32> = if k == 0 {
                vec![f32::MIN; n]
            } else {
                let sb = bbox_of(pos3.iter().copied());
                let cen = [(sb[0] + sb[3]) / 2.0, (sb[1] + sb[4]) / 2.0];
                let maxd = pos3.iter().map(|p| (p[0] - cen[0]).hypot(p[1] - cen[1])).fold(1e-9, f64::max);
                (0..n)
                    .map(|i| {
                        let j = sl[i] as usize;
                        let p = pos3[j];
                        let key = match f.stagger_by.as_str() {
                            "random" => rnd(seed, 0x57A6 + k as u64, i as u64),
                            "distance" => (p[0] - cen[0]).hypot(p[1] - cen[1]) / maxd,
                            "x" => (p[0] - sb[0]) / (sb[3] - sb[0]).max(1e-9),
                            "y" => (p[1] - sb[1]) / (sb[4] - sb[1]).max(1e-9),
                            "angle" => ((p[1] - cen[1]).atan2(p[0] - cen[0]) + PI) / TAU,
                            _ => j as f64 / (n.max(2) - 1) as f64,
                        };
                        let d = if f.stagger >= 0.0 { f.stagger * key } else { -f.stagger * (1.0 - key) };
                        // a point never starts a form before the previous one
                        ((f.at + d) as f32).max(prev_start[i])
                    })
                    .collect()
            };
            if k > 0 {
                form.min_start = st.iter().copied().fold(f32::MAX, f32::min) as f64;
                form.max_start = st.iter().copied().fold(f32::MIN, f32::max) as f64;
                form.max_end = form.max_start + form.dur;
            } else {
                form.min_start = f64::MIN;
                form.max_start = f64::MIN;
                form.max_end = f64::MIN;
            }
            prev_pos = (0..n).map(|i| pos2[sl[i] as usize]).collect();
            prev_col = cl.clone();
            prev_start = st.clone();
            forms.push(form);
            slot.push(sl);
            start.push(st);
            col.push(cl);
        }
        let max_size = forms.iter().map(|f| f.size).fold(0.0, f64::max) * (1.0 + c.radius_jitter);
        Ok(Cloud { n, radius: c.radius, rfac, palette: c.palette.clone(), forms, slot, start, col, proj, max_size, toff: c.time_offset })
    }

    #[allow(dead_code)]
    pub fn has_projection(&self) -> bool {
        !self.proj.is_empty()
    }

    /// Evaluate all points at time t into `out` (content space, projected if any; far to
    /// near with a projection). Parallel over chunks, order preserved.
    pub fn eval(&self, t: f64, out: &mut Vec<Pt>) {
        use rayon::prelude::*;
        let t = t + self.toff;
        let m = (!self.proj.is_empty()).then(|| proj_at(&self.proj, t));
        let parts: Vec<Vec<Pt>> = (0..self.n).into_par_iter().step_by(4096).map(|i0| {
            let mut v = Vec::with_capacity(4096);
            for i in i0..(i0 + 4096).min(self.n) {
                if let Some(p) = self.point(i, t, m.as_ref()) {
                    v.push(p);
                }
            }
            v
        }).collect();
        out.clear();
        out.reserve(self.n);
        for p in parts {
            out.extend(p);
        }
        if m.is_some() {
            // far to near (painter's order); stable, so equal depths keep index order
            out.par_sort_by(|a, b| b.w.total_cmp(&a.w));
        }
    }

    #[inline]
    fn point(&self, i: usize, t: f64, m: Option<&[f64; 12]>) -> Option<Pt> {
        let nf = self.forms.len();
        let tf = t as f32;
        // last form the point has fully arrived at
        let mut k0 = 0;
        for k in (1..nf).rev() {
            let s = self.start[k][i];
            if tf >= s && (t - s as f64) >= self.forms[k].dur {
                k0 = k;
                break;
            }
        }
        let f0 = &self.forms[k0];
        let mut p = f0.slot(self.slot[k0][i] as usize, t);
        let mut size = f0.size;
        let mut op = f0.opacity;
        let pc = self.palette[self.col[k0][i] as usize % self.palette.len()];
        let mut c = [pc[0] as f64, pc[1] as f64, pc[2] as f64, pc[3] as f64];
        for k in k0 + 1..nf {
            let s = self.start[k][i] as f64;
            if t < s {
                break;
            }
            let f = &self.forms[k];
            let u = if f.dur > 0.0 { (t - s) / f.dur } else { 1.0 };
            let e = f.ease.at(u);
            let q = f.slot(self.slot[k][i] as usize, t);
            for a in 0..3 {
                p[a] += (q[a] - p[a]) * e;
            }
            size += (f.size - size) * e;
            op += (f.opacity - op) * e.clamp(0.0, 1.0);
            let qc = self.palette[self.col[k][i] as usize % self.palette.len()];
            let ec = e.clamp(0.0, 1.0);
            for a in 0..4 {
                c[a] += (qc[a] as f64 - c[a]) * ec;
            }
        }
        let r = self.radius * size.max(0.0) * self.rfac[i] as f64;
        let a = (op.clamp(0.0, 1.0) * c[3]) as f32;
        if a <= 0.0 || r <= 0.0 {
            return None;
        }
        let (x, y, w, rs) = match m {
            None => (p[0], p[1], 1.0, r),
            Some(m) => {
                let w = m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11];
                if w <= 1e-3 {
                    return None;
                }
                let x = (m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3]) / w;
                let y = (m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7]) / w;
                (x, y, w, r * proj_scale(m, x, y, w))
            }
        };
        Some(Pt { x: x as f32, y: y as f32, r: rs as f32, c: [c[0] as f32, c[1] as f32, c[2] as f32], a, w: w as f32 })
    }

    /// Forms that can affect the frame at t: from the last form every point has reached.
    fn live_forms(&self, t: f64) -> (usize, usize) {
        let nf = self.forms.len();
        let mut k0 = 0;
        for k in (1..nf).rev() {
            if t >= self.forms[k].max_end {
                k0 = k;
                break;
            }
        }
        let mut k1 = k0;
        for k in k0 + 1..nf {
            if t >= self.forms[k].min_start {
                k1 = k;
            }
        }
        (k0, k1)
    }

    /// Conservative content-space ink at t (no per-point work).
    pub fn bounds(&self, t: f64) -> Option<[f64; 4]> {
        let t = t + self.toff;
        let (k0, k1) = self.live_forms(t);
        let mut b = self.forms[k0].bbox;
        for k in k0 + 1..=k1 {
            let f = &self.forms[k];
            b = union6(b, lerp_box(b, f.bbox, f.ease.lo, f.ease.hi));
        }
        let r = self.radius * self.max_size + 1.0;
        if self.proj.is_empty() {
            return Some([b[0] - r, b[1] - r, b[3] - b[0] + 2.0 * r, b[4] - b[1] + 2.0 * r]);
        }
        let m = proj_at(&self.proj, t);
        let mut o = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        let mut smax: f64 = 0.0;
        for cx in [b[0], b[3]] {
            for cy in [b[1], b[4]] {
                for cz in [b[2], b[5]] {
                    let w = m[8] * cx + m[9] * cy + m[10] * cz + m[11];
                    if w <= 1e-3 {
                        // the box reaches behind the camera: unbounded
                        return Some([-1e6, -1e6, 2e6, 2e6]);
                    }
                    let x = (m[0] * cx + m[1] * cy + m[2] * cz + m[3]) / w;
                    let y = (m[4] * cx + m[5] * cy + m[6] * cz + m[7]) / w;
                    smax = smax.max(proj_scale(&m, x, y, w));
                    o = [o[0].min(x), o[1].min(y), o[2].max(x), o[3].max(y)];
                }
            }
        }
        let r = r * smax.max(1.0);
        Some([o[0] - r, o[1] - r, o[2] - o[0] + 2.0 * r, o[3] - o[1] + 2.0 * r])
    }

    /// Signature of the cloud's pixels at t: constant while every point rests in a static form.
    pub fn sig(&self, t: f64) -> u64 {
        use std::hash::{Hash, Hasher};
        let t = t + self.toff;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let (k0, k1) = self.live_forms(t);
        let resting = k1 == k0 && !self.forms[k0].dynamic;
        if resting {
            k0.hash(&mut h);
        } else {
            t.to_bits().hash(&mut h);
        }
        if !self.proj.is_empty() {
            for v in proj_at(&self.proj, t) {
                v.to_bits().hash(&mut h);
            }
        }
        h.finish()
    }
}

/// Projection matrix at t (piecewise linear, clamped).
pub fn proj_at(keys: &[CloudView], t: f64) -> [f64; 12] {
    if t <= keys[0].t {
        return keys[0].m;
    }
    let i = keys.partition_point(|k| k.t <= t);
    if i >= keys.len() {
        return keys[keys.len() - 1].m;
    }
    let (a, b) = (&keys[i - 1], &keys[i]);
    let u = ((t - a.t) / (b.t - a.t).max(1e-12)).clamp(0.0, 1.0);
    let mut m = a.m;
    for j in 0..12 {
        m[j] += (b.m[j] - a.m[j]) * u;
    }
    m
}

/// Size factor of a sphere at screen (x, y) with depth w: sqrt of the product of the
/// projection Jacobian's singular values (= focal / depth at the center).
fn proj_scale(m: &[f64; 12], x: f64, y: f64, w: f64) -> f64 {
    let j0 = [(m[0] - x * m[8]) / w, (m[1] - x * m[9]) / w, (m[2] - x * m[10]) / w];
    let j1 = [(m[4] - y * m[8]) / w, (m[5] - y * m[9]) / w, (m[6] - y * m[10]) / w];
    let cx = j0[1] * j1[2] - j0[2] * j1[1];
    let cy = j0[2] * j1[0] - j0[0] * j1[2];
    let cz = j0[0] * j1[1] - j0[1] * j1[0];
    (cx * cx + cy * cy + cz * cz).sqrt().sqrt()
}

// ---------------------------------------------------------------- splat

/// Draw points (content space) through affine `m` ([a, b, c, d, e, f], content -> buffer
/// pixels) into a premultiplied RGBA8 buffer of `w` x `h`, "over" in order. Discs with an
/// analytic coverage ramp (`r + 0.5 - distance`, clamped; the interior needs no square root),
/// sub-pixel points as area-preserving bilinear dots. Integer blending, exact /255 rounding.
///
/// Points are binned into bands of rows (in order, so every pixel sees the same sequence as a
/// plain loop: identical result) and the bands are drawn in parallel: cache-local and
/// multi-core.
pub fn splat(pts: &[Pt], m: [f64; 6], buf: &mut [u8], w: usize, h: usize) {
    use rayon::prelude::*;
    const BAND: usize = 32;
    if w == 0 || h == 0 {
        return;
    }
    let s = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
    let (wf, hf) = (w as f32, h as f32);
    let dev: Vec<Dev> = pts
        .iter()
        .filter_map(|p| {
            let (x, y) = (p.x as f64, p.y as f64);
            let cx = (m[0] * x + m[2] * y + m[4]) as f32;
            let cy = (m[1] * x + m[3] * y + m[5]) as f32;
            let r = p.r * s as f32;
            if !(cx.is_finite() && cy.is_finite()) || cx + r + 1.0 < 0.0 || cy + r + 1.0 < 0.0 || cx - r - 1.0 > wf || cy - r - 1.0 > hf {
                return None;
            }
            let col = [(p.c[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u32, (p.c[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u32, (p.c[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u32];
            Some(Dev { cx, cy, r, col, a: p.a.clamp(0.0, 1.0) })
        })
        .collect();
    let nb = h.div_ceil(BAND);
    let span = |d: &Dev| -> (usize, usize) {
        let y0 = ((d.cy - d.r - 1.0).floor().max(0.0) as usize / BAND).min(nb - 1);
        let y1 = ((d.cy + d.r + 1.0).ceil().max(0.0) as usize / BAND).min(nb - 1);
        (y0, y1)
    };
    let mut start = vec![0u32; nb + 1];
    for d in &dev {
        let (a, b) = span(d);
        for k in a..=b {
            start[k + 1] += 1;
        }
    }
    for k in 0..nb {
        start[k + 1] += start[k];
    }
    let mut fill = start.clone();
    let mut idx = vec![0u32; start[nb] as usize];
    for (i, d) in dev.iter().enumerate() {
        let (a, b) = span(d);
        for k in a..=b {
            idx[fill[k] as usize] = i as u32;
            fill[k] += 1;
        }
    }
    buf.par_chunks_mut(w * 4 * BAND).enumerate().for_each(|(b, band)| {
        let rows = band.len() / (w * 4);
        let y_off = b * BAND;
        for &i in &idx[start[b] as usize..start[b + 1] as usize] {
            draw_point(&dev[i as usize], band, w, y_off, rows);
        }
    });
}

struct Dev {
    cx: f32,
    cy: f32,
    r: f32,
    col: [u32; 3],
    a: f32,
}

/// One point into the band of rows `[y_off, y_off + rows)` (`band` holds just those rows).
#[inline]
fn draw_point(d: &Dev, band: &mut [u8], w: usize, y_off: usize, rows: usize) {
    let (cx, cy, r, col, a) = (d.cx, d.cy, d.r, d.col, d.a);
    let wf = w as f32;
    let (ylo, yhi) = (y_off as f32, (y_off + rows) as f32);
    if r < 0.6 {
        // tiny: total coverage = disc area, bilinear over the 4 nearest pixel centers
        let cov = (PI as f32 * r * r).min(1.0) * a;
        let (fx, fy) = (cx - 0.5, cy - 0.5);
        let (ix, iy) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - ix, fy - iy);
        for (dx, dy, wgt) in [(0, 0, (1.0 - tx) * (1.0 - ty)), (1, 0, tx * (1.0 - ty)), (0, 1, (1.0 - tx) * ty), (1, 1, tx * ty)] {
            let (px, py) = (ix as i64 + dx, iy as i64 + dy);
            if px >= 0 && (px as usize) < w && py >= y_off as i64 && (py as usize) < y_off + rows {
                let i = ((py as usize - y_off) * w + px as usize) * 4;
                over8(&mut band[i..i + 4], col, (cov * wgt * 255.0 + 0.5) as u32);
            }
        }
        return;
    }
    let a255 = a * 255.0;
    let full = (a255 + 0.5) as u32;
    let ro = r + 0.5;
    let ri = r - 0.5;
    let (ro2, ri2) = (ro * ro, if ri > 0.0 { ri * ri } else { -1.0 });
    let y0 = (cy - ro).floor().max(ylo) as usize;
    let y1 = (cy + ro).ceil().min(yhi).max(ylo) as usize;
    for py in y0..y1 {
        let dy = py as f32 + 0.5 - cy;
        let dy2 = dy * dy;
        if dy2 >= ro2 {
            continue;
        }
        // pixel centers inside the outer radius: |px + 0.5 - cx| < half
        let half = (ro2 - dy2).sqrt();
        let xa = (cx - half - 0.5).ceil().max(0.0) as usize;
        let xb = ((cx + half - 0.5).floor() + 1.0).clamp(0.0, wf) as usize;
        if xa >= xb {
            continue;
        }
        // fully covered interior span (distance <= r - 0.5)
        let (ia, ib) = if dy2 < ri2 {
            let hi = (ri2 - dy2).sqrt();
            ((cx - hi - 0.5).ceil().max(xa as f32) as usize, (((cx + hi - 0.5).floor() + 1.0).max(0.0) as usize).min(xb))
        } else {
            (xb, xb)
        };
        let o = (py - y_off) * w * 4;
        let row = &mut band[o..o + w * 4];
        for px in xa..xb {
            let al = if px >= ia && px < ib {
                full
            } else {
                let dx = px as f32 + 0.5 - cx;
                let cov = (ro - (dx * dx + dy2).sqrt()).clamp(0.0, 1.0);
                (cov * a255 + 0.5) as u32
            };
            over8(&mut row[px * 4..px * 4 + 4], col, al);
        }
    }
}

#[inline(always)]
fn div255(x: u32) -> u32 {
    (x + 128 + ((x + 128) >> 8)) >> 8
}

/// Premultiplied "over" of straight color `col` with alpha `al` (0..255).
#[inline(always)]
fn over8(d: &mut [u8], col: [u32; 3], al: u32) {
    if al == 0 {
        return;
    }
    if al >= 255 {
        d[0] = col[0] as u8;
        d[1] = col[1] as u8;
        d[2] = col[2] as u8;
        d[3] = 255;
        return;
    }
    let inv = 255 - al;
    d[0] = div255(col[0] * al + d[0] as u32 * inv) as u8;
    d[1] = div255(col[1] * al + d[1] as u32 * inv) as u8;
    d[2] = div255(col[2] * al + d[2] as u32 * inv) as u8;
    d[3] = (al + div255(d[3] as u32 * inv)) as u8;
}

// ---------------------------------------------------------------- text sampling

/// Sample `n` points inside (hex grid) or on the outline of a coverage mask (`alpha`, `mw` x
/// `mh`, pixel (x, y) covers scene `[ox + x / s, oy + y / s]`).
pub fn sample_mask(alpha: &[u8], mw: usize, mh: usize, ox: f64, oy: f64, s: f64, n: usize, outline: bool, seed: u64) -> Vec<[f64; 2]> {
    let inside = |x: f64, y: f64| -> bool {
        let (ix, iy) = (x as isize, y as isize);
        ix >= 0 && iy >= 0 && (ix as usize) < mw && (iy as usize) < mh && alpha[iy as usize * mw + ix as usize] >= 128
    };
    let mut pts: Vec<[f64; 2]> = vec![];
    if outline {
        for y in 0..mh {
            for x in 0..mw {
                if alpha[y * mw + x] < 128 {
                    continue;
                }
                let edge = x == 0 || y == 0 || x + 1 == mw || y + 1 == mh || alpha[y * mw + x - 1] < 128 || alpha[y * mw + x + 1] < 128 || alpha[(y - 1) * mw + x] < 128 || alpha[(y + 1) * mw + x] < 128;
                if edge {
                    pts.push([x as f64 + 0.5, y as f64 + 0.5]);
                }
            }
        }
    } else {
        let grid = |sp: f64| -> Vec<[f64; 2]> {
            let mut out = vec![];
            let mut row = 0;
            let mut y = sp / 2.0;
            while y < mh as f64 {
                let mut x = sp / 2.0 + if row % 2 == 1 { sp / 2.0 } else { 0.0 };
                while x < mw as f64 {
                    if inside(x, y) {
                        out.push([x, y]);
                    }
                    x += sp;
                }
                y += sp * 0.866;
                row += 1;
            }
            out
        };
        // largest spacing with at least n points
        let (mut lo, mut hi) = (0.25f64, (mw.max(mh) as f64).max(1.0));
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if grid(mid).len() >= n {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        pts = grid(lo);
        let j = 0.18 * lo;
        for (i, p) in pts.iter_mut().enumerate() {
            p[0] += j * (2.0 * rnd(seed, 0x7E47, i as u64) - 1.0);
            p[1] += j * (2.0 * rnd(seed, 0x7E48, i as u64) - 1.0);
        }
    }
    if pts.is_empty() {
        return vec![[ox, oy]; n];
    }
    // exactly n: drop a seeded random subset (holes spread evenly), or cycle with jitter
    let m = pts.len();
    let mut out: Vec<[f64; 2]> = if m >= n {
        let mut idx: Vec<usize> = (0..m).collect();
        idx.sort_by_key(|&i| (hash3(seed, 0x7E49, i as u64), i));
        let mut keep: Vec<usize> = idx.into_iter().take(n).collect();
        keep.sort_unstable();
        keep.into_iter().map(|i| pts[i]).collect()
    } else {
        (0..n).map(|i| {
            let p = pts[i % m];
            if i < m { p } else { [p[0] + rnd(seed, 0x7E4A, i as u64) - 0.5, p[1] + rnd(seed, 0x7E4B, i as u64) - 0.5] }
        }).collect()
    };
    for p in &mut out {
        *p = [ox + p[0] / s, oy + p[1] / s];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentfx_ir::{CloudColor, CloudDrift, CloudForm};

    fn form(at: f64, shape: CloudShape) -> CloudForm {
        CloudForm { at, dur: 1.0, stagger: 0.5, stagger_by: "index".into(), easing: Easing::Linear, assign: "curve".into(), shape, drift: None, size: 1.0, opacity: 1.0, color: None }
    }
    fn no_text(_: &CloudText, _: usize, _: u64) -> Result<Vec<[f64; 2]>> {
        anyhow::bail!("no text")
    }
    fn content(n: u32) -> CloudContent {
        CloudContent {
            count: n,
            seed: 7,
            radius: 2.0,
            radius_jitter: 0.2,
            palette: vec![[1.0, 1.0, 1.0, 1.0], [1.0, 0.3, 0.2, 1.0]],
            weights: vec![0.7, 0.3],
            forms: vec![
                CloudForm { drift: Some(CloudDrift { amp: 8.0, scale: 200.0, speed: 0.3 }), ..form(0.0, CloudShape::Field { rect: [0.0, 0.0, 800.0, 600.0], depth: 0.0, flow: [0.0, 0.0] }) },
                CloudForm { easing: Easing::Spring { mass: 1.0, stiffness: 150.0, damping: 15.0, velocity: 0.0 }, ..form(1.0, CloudShape::Spiral { center: [400.0, 300.0], radius: 250.0, arms: 3, twist: 1.45, spread: 0.1, squash: 0.7, spin: 20.0 }) },
                CloudForm { color: Some(CloudColor { by: "y".into(), split: vec![0.7, 0.3], angle: 0.0, center: None, wave: None }), ..form(3.0, CloudShape::Grid { rect: [100.0, 100.0, 600.0, 400.0], cols: 0, jitter: 0.0 }) },
            ],
            projection: vec![],
            time_offset: 0.0,
        }
    }

    #[test]
    fn deterministic() {
        let c = content(3000);
        let a = Cloud::new(&c, &mut no_text).unwrap();
        let b = Cloud::new(&c, &mut no_text).unwrap();
        for t in [0.0, 0.5, 1.3, 2.2, 3.4, 9.0] {
            let (mut pa, mut pb) = (vec![], vec![]);
            a.eval(t, &mut pa);
            b.eval(t, &mut pb);
            assert_eq!(pa, pb, "t={t}");
            assert_eq!(a.sig(t), b.sig(t));
        }
        // a different seed moves the points
        let mut c2 = c.clone();
        c2.seed = 8;
        let d = Cloud::new(&c2, &mut no_text).unwrap();
        let (mut pa, mut pd) = (vec![], vec![]);
        a.eval(0.0, &mut pa);
        d.eval(0.0, &mut pd);
        assert_ne!(pa, pd);
    }

    #[test]
    fn assignment_is_a_permutation_and_keeps_neighbours() {
        let c = content(4096);
        let cl = Cloud::new(&c, &mut no_text).unwrap();
        for k in 0..cl.forms.len() {
            let mut seen = vec![false; cl.n];
            for &j in &cl.slot[k] {
                assert!(!seen[j as usize]);
                seen[j as usize] = true;
            }
        }
        // grid -> grid (same slots): curve matching is the identity on positions
        let src: Vec<[f64; 2]> = (0..1024).map(|i| [(i % 32) as f64, (i / 32) as f64]).collect();
        let m = assign("curve", &src, &src, 1, 1);
        assert!(m.iter().enumerate().all(|(i, &j)| i == j));
        // total travel of curve matching is far below random matching
        let dst: Vec<[f64; 2]> = (0..1024).map(|i| [(i % 32) as f64 * 1.1 + 3.0, (i / 32) as f64 * 0.9 + 2.0]).collect();
        let travel = |m: &[usize]| m.iter().enumerate().map(|(i, &j)| (src[i][0] - dst[j][0]).hypot(src[i][1] - dst[j][1])).sum::<f64>();
        let (mc, mr) = (assign("curve", &src, &dst, 1, 1), assign("random", &src, &dst, 1, 1));
        assert!(travel(&mc) * 3.0 < travel(&mr), "{} vs {}", travel(&mc), travel(&mr));
        // a rescaled, moved copy maps point i to its own copy (shape-relative matching)
        assert!(mc.iter().enumerate().all(|(i, &j)| i == j));
        // different shapes: refinement leaves no improving swap among curve neighbours
        let ring: Vec<[f64; 2]> = (0..1024).map(|i| { let a = i as f64 * 0.00614; [300.0 + 200.0 * a.cos(), 200.0 + 150.0 * a.sin()] }).collect();
        let m = assign("curve", &src, &ring, 1, 1);
        let d2 = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
        let cost = |m: &[usize]| m.iter().enumerate().map(|(i, &j)| d2(src[i], ring[j])).sum::<f64>();
        let mut unrefined = vec![0; 1024];
        let (ka, kb) = (curve_keys(&src), curve_keys(&ring));
        let mut a: Vec<usize> = (0..1024).collect();
        let mut b = a.clone();
        a.sort_by_key(|&j| (ka[j], j));
        b.sort_by_key(|&j| (kb[j], j));
        for (&x, &y) in a.iter().zip(&b) { unrefined[x] = y; }
        assert!(cost(&m) < cost(&unrefined));
    }

    #[test]
    fn colors_follow_rules_and_groups_keep_colors() {
        let c = content(2000);
        let cl = Cloud::new(&c, &mut no_text).unwrap();
        let count = |k: usize, ci: u8| cl.col[k].iter().filter(|&&x| x == ci).count();
        assert_eq!(count(0, 1), 600);
        // form 2 splits by y with the same counts: grouped matching, every point keeps its color
        assert_eq!(cl.col[2], cl.col[0]);
        // and the red points are the bottom 30 % of the grid
        let f = &cl.forms[2];
        let ys: Vec<f64> = (0..cl.n).map(|i| f.slot(cl.slot[2][i] as usize, 10.0)[1]).collect();
        let max_white = (0..cl.n).filter(|&i| cl.col[2][i] == 0).map(|i| ys[i]).fold(f64::MIN, f64::max);
        let min_red = (0..cl.n).filter(|&i| cl.col[2][i] == 1).map(|i| ys[i]).fold(f64::MAX, f64::min);
        assert!(max_white <= min_red);
    }

    #[test]
    fn morph_is_continuous_and_lands_on_slots() {
        let c = content(500);
        let cl = Cloud::new(&c, &mut no_text).unwrap();
        let mut prev = vec![];
        cl.eval(0.9, &mut prev);
        let mut cur = vec![];
        let mut t = 0.9;
        while t < 5.0 {
            t += 1.0 / 240.0;
            cl.eval(t, &mut cur);
            let jump = prev.iter().zip(&cur).map(|(a, b)| (a.x - b.x).hypot(a.y - b.y)).fold(0.0f32, f32::max);
            assert!(jump < 40.0, "jump {jump} at t={t}");
            std::mem::swap(&mut prev, &mut cur);
        }
        // at rest in the grid: exactly on the slots, and the signature is constant
        cl.eval(6.0, &mut cur);
        for (i, p) in cur.iter().enumerate() {
            let q = cl.forms[2].slot(cl.slot[2][i] as usize, 6.0);
            assert!((p.x as f64 - q[0]).abs() < 1e-3 && (p.y as f64 - q[1]).abs() < 1e-3);
        }
        assert_eq!(cl.sig(6.0), cl.sig(7.5));
        assert_ne!(cl.sig(1.5), cl.sig(1.6));
    }

    #[test]
    fn time_offset_shifts_evaluation() {
        let c = content(400);
        let mut c2 = c.clone();
        c2.time_offset = 1.25;
        let (a, b) = (Cloud::new(&c, &mut no_text).unwrap(), Cloud::new(&c2, &mut no_text).unwrap());
        let (mut pa, mut pb) = (vec![], vec![]);
        a.eval(2.0, &mut pa);
        b.eval(0.75, &mut pb);
        assert_eq!(pa, pb);
        assert_eq!(a.bounds(2.0), b.bounds(0.75));
    }

    #[test]
    fn bounds_contain_every_point() {
        let mut c = content(1500);
        c.forms[1].stagger = 1.5;
        let cl = Cloud::new(&c, &mut no_text).unwrap();
        let mut pts = vec![];
        for i in 0..80 {
            let t = i as f64 * 0.07;
            cl.eval(t, &mut pts);
            let b = cl.bounds(t).unwrap();
            for p in &pts {
                assert!(p.x as f64 - p.r as f64 >= b[0] - 1e-3 && p.x as f64 + p.r as f64 <= b[0] + b[2] + 1e-3, "t={t} x={} b={b:?}", p.x);
                assert!(p.y as f64 - p.r as f64 >= b[1] - 1e-3 && p.y as f64 + p.r as f64 <= b[1] + b[3] + 1e-3, "t={t} y={} b={b:?}", p.y);
            }
        }
    }

    #[test]
    fn projection_scales_and_sorts() {
        let mut c = content(800);
        c.forms = vec![form(0.0, CloudShape::Sphere { center: [400.0, 300.0, 0.0], radius: 200.0, spin: 30.0 })];
        // T(c) * perspective(1000) * T(-c)
        let d = 1000.0;
        let (cx, cy) = (400.0, 300.0);
        c.projection = vec![CloudView { t: 0.0, m: [1.0, 0.0, -cx / d, 0.0, 0.0, 1.0, -cy / d, 0.0, 0.0, 0.0, -1.0 / d, 1.0] }];
        let cl = Cloud::new(&c, &mut no_text).unwrap();
        let mut pts = vec![];
        cl.eval(0.5, &mut pts);
        assert!(pts.windows(2).all(|w| w[0].w >= w[1].w));
        // near points (z > 0, w < 1) are larger than far ones
        let near = pts.last().unwrap();
        let far = pts.first().unwrap();
        assert!(near.r > far.r && near.w < far.w);
        let b = cl.bounds(0.5).unwrap();
        for p in &pts {
            assert!(p.x as f64 >= b[0] && p.x as f64 <= b[0] + b[2] && p.y as f64 >= b[1] && p.y as f64 <= b[1] + b[3]);
        }
    }

    #[test]
    fn splat_covers_disc_area() {
        let (w, h) = (64, 64);
        let mut buf = vec![0u8; w * h * 4];
        let pt = Pt { x: 20.3, y: 30.7, r: 6.0, c: [1.0, 1.0, 1.0], a: 1.0, w: 1.0 };
        splat(&[pt], [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut buf, w, h);
        let cov: f64 = buf.chunks(4).map(|p| p[3] as f64 / 255.0).sum();
        assert!((cov - PI * 36.0).abs() < 1.5, "{cov}");
        // tiny points keep their area
        let mut buf = vec![0u8; w * h * 4];
        splat(&[Pt { r: 0.3, ..pt }], [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut buf, w, h);
        let cov: f64 = buf.chunks(4).map(|p| p[3] as f64 / 255.0).sum();
        assert!((cov - PI * 0.09).abs() < 0.02, "{cov}");
    }

    #[test]
    fn mask_sampling_counts() {
        let (mw, mh) = (200, 100);
        let mut a = vec![0u8; mw * mh];
        for y in 20..80 {
            for x in 30..170 {
                a[y * mw + x] = 255;
            }
        }
        for outline in [false, true] {
            let p = sample_mask(&a, mw, mh, 10.0, 5.0, 2.0, 1000, outline, 3);
            assert_eq!(p.len(), 1000);
            assert!(p.iter().all(|q| q[0] >= 10.0 + 14.0 && q[0] <= 10.0 + 86.0 && q[1] >= 5.0 + 9.0 && q[1] <= 5.0 + 41.0));
            assert_eq!(p, sample_mask(&a, mw, mh, 10.0, 5.0, 2.0, 1000, outline, 3));
        }
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    /// `cargo test --release -p reel cloud::bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn eval_and_splat_100k() {
        let path = agentfx_ir::env::var("AGENTFX_CLOUD_BENCH").expect("AGENTFX_CLOUD_BENCH=<ir.json>");
        let scene: agentfx_ir::Scene = agentfx_ir::Scene::load(std::path::Path::new(&path)).unwrap();
        let c = scene.nodes.iter().find_map(|n| match &n.content { Some(agentfx_ir::Content::Cloud(c)) => Some(c.clone()), _ => None }).unwrap();
        let mut no_text = |_: &CloudText, n: usize, _: u64| -> Result<Vec<[f64; 2]>> { Ok((0..n).map(|i| [(i % 300) as f64 * 5.0, (i / 300) as f64 * 3.0]).collect()) };
        let cl = Cloud::new(&c, &mut no_text).unwrap();
        let mut pts = vec![];
        let mut buf = vec![0u8; 1920 * 1080 * 4];
        for t in [0.5, 1.6, 4.5, 7.5, 10.5, 13.0] {
            let t0 = std::time::Instant::now();
            for _ in 0..10 {
                cl.eval(t, &mut pts);
            }
            let te = t0.elapsed().as_secs_f64() / 10.0;
            let t1 = std::time::Instant::now();
            for _ in 0..10 {
                buf.fill(0);
                splat(&pts, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut buf, 1920, 1080);
            }
            let ts = t1.elapsed().as_secs_f64() / 10.0;
            let area: f64 = pts.iter().map(|p| PI * (p.r as f64 + 0.5).powi(2)).sum();
            eprintln!("t={t}: {} pts, {:.1} Mpx, eval {:.2} ms, splat {:.2} ms", pts.len(), area / 1e6, te * 1e3, ts * 1e3);
        }
    }
}
