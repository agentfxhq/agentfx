//! Print looks (X8): risograph plates, AM halftone, grain and paper, as node effects on
//! premultiplied RGBA8 offscreen buffers (`vraster::effect_pixmap`). Light effects (X6) are
//! in `light.rs`; `apply` dispatches both.
//!
//! Every random choice is a pure function of (layer-local pixel, seed[, frame]): integer
//! hashes, no RNG state, so a frame renders identically in any order, segment or thread.
//! Noise is anchored to the node (screen pixel minus the node's rounded translation): a
//! layer that moves carries its grain with it, and the translation-invariant effect cache
//! stays exact.
use rayon::prelude::*;
use std::sync::Arc;
use agentfx_ir::Effect;

/// Per-call context: where the buffer sits and what the node / frame are.
#[derive(Clone, Copy, Debug)]
pub struct FxCtx {
    /// Buffer origin in screen px.
    pub ox: i32,
    pub oy: i32,
    /// Node anchor in screen px (rounded translation of its chain): noise origin.
    pub ax: i32,
    pub ay: i32,
    /// Node scale (node px -> screen px).
    pub s: f64,
    /// Output frame (live grain).
    pub frame: u32,
}

/// Number of parameters per effect (`schedule::effect_params`); slot `TIME` holds the time
/// for time-dependent effects.
pub const NPARAM: usize = 8;
pub const TIME: usize = 7;

/// Extra ink (screen px) an effect adds around its subtree. `frame` = output size (light
/// effects reach at most across the frame).
pub fn margin(e: &Effect, p: &[f64; NPARAM], s: f64, frame: [f64; 2]) -> f64 {
    match e {
        Effect::Riso { drift, .. } => (p[0] + drift.abs()) * s + 2.0,
        Effect::Halftone { .. } => p[0] * s + 1.0,
        Effect::Grain { .. } | Effect::Sweep { .. } => 0.0,
        Effect::Paper { .. } => (3.0 * p[1] + 2.0) * s,
        Effect::Shadow { .. } => crate::light::shadow_reach(p, frame),
        Effect::Rays { .. } => crate::light::rays_reach(p, frame),
        _ => 0.0,
    }
}

/// Apply a look/light effect in place.
pub fn apply(e: &Effect, p: &[f64; NPARAM], buf: &mut [u8], w: usize, h: usize, cx: &FxCtx) {
    if w == 0 || h == 0 {
        return;
    }
    match e {
        Effect::Riso { inks, seed, drift, drift_speed, grain, density, paper, .. } => riso(buf, w, h, cx, &RisoCfg { inks, seed: *seed, misreg: p[0], drift: *drift, drift_speed: *drift_speed, t: p[TIME], grain: *grain, density: *density, paper: *paper }),
        Effect::Halftone { mode, ink, paper, .. } => halftone(buf, w, h, cx, p[0], p[1], mode == "cmyk", *ink, *paper),
        Effect::Grain { seed, live, size, .. } => grain(buf, w, h, cx, p[0], *seed, *live, *size),
        Effect::Paper { seed, color, .. } => paper(buf, w, h, cx, p[0], p[1], p[2], *seed, *color),
        Effect::Shadow { color, .. } => crate::light::shadow(buf, w, h, cx, p, *color),
        Effect::Sweep { color, .. } => crate::light::sweep(buf, w, h, p, *color),
        Effect::Rays { color, .. } => crate::light::rays(buf, w, h, cx, p, *color),
        Effect::Color { tint, levels, lut, .. } => {
            let cfg = crate::color::ColorCfg { brightness: p[0].max(0.0), saturate: p[1].max(0.0), contrast: p[2].max(0.0), hue: p[3], tint: *tint, tint_amount: p[4], levels: *levels, lut: lut.as_deref() };
            crate::color::apply(buf, w, h, &cfg)
        }
        Effect::Rgbsplit { .. } | Effect::Glitch { .. } | Effect::Vhs { .. } | Effect::Dither { .. } | Effect::Ascii { .. } => crate::digital::apply(e, p, buf, w, h, cx),
        _ => {}
    }
}

// ------------------------------------------------------------------ noise

/// 2D integer hash (lowbias32-style finalizer).
#[inline(always)]
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f) ^ 0x9e37_79b9;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
}

/// Uniform in [0, 1).
#[inline(always)]
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// Smooth value noise in [0, 1] at lattice spacing 1.
#[inline]
pub fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (ix, iy) = (xf as i32, yf as i32);
    let (tx, ty) = (x - xf, y - yf);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let a = unit(hash(ix, iy, seed));
    let b = unit(hash(ix + 1, iy, seed));
    let c = unit(hash(ix, iy + 1, seed));
    let d = unit(hash(ix + 1, iy + 1, seed));
    (a + (b - a) * sx) + ((c + (d - c) * sx) - (a + (b - a) * sx)) * sy
}

/// Fractal value noise in [0, 1], `oct` octaves starting at spacing `scale` px.
pub fn fbm(x: f32, y: f32, scale: f32, oct: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0 / scale);
    for o in 0..oct {
        sum += amp * value_noise(x * f, y * f, seed.wrapping_add(o * 7919));
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm
}

#[inline(always)]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn rgb(c: [f64; 4]) -> [f32; 3] {
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

/// Write a print result: `c` = color over white (straight, 0..1) at layer coverage `a`.
/// With `paper`, the covered area is paper x c (opaque inside the layer); without, the ink
/// is transparent: alpha = 1 - min(c), premultiplied color = c - min(c) (exact over white,
/// a darkening over other colors).
#[inline(always)]
fn put(px: &mut [u8], c: [f32; 3], a: f32, paper: Option<[f32; 3]>) {
    match paper {
        Some(pp) => {
            for k in 0..3 {
                px[k] = ((pp[k] * c[k] * a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            px[3] = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        None => {
            let m = c[0].min(c[1]).min(c[2]).clamp(0.0, 1.0);
            let al = 1.0 - m;
            for k in 0..3 {
                px[k] = ((c[k] - m).clamp(0.0, al) * 255.0 + 0.5) as u8;
            }
            px[3] = (al * 255.0 + 0.5) as u8;
        }
    }
}

/// Straight color (0..1) and alpha of a premultiplied pixel.
#[inline(always)]
fn unpremul(p: &[u8]) -> ([f32; 3], f32) {
    let a = p[3] as f32 / 255.0;
    if p[3] == 0 {
        return ([1.0; 3], 0.0);
    }
    let inv = 1.0 / p[3] as f32;
    ([(p[0] as f32 * inv).min(1.0), (p[1] as f32 * inv).min(1.0), (p[2] as f32 * inv).min(1.0)], a)
}

/// Bilinear sample of an f32 plane (outside = 0), coordinates in pixel centres.
#[inline(always)]
pub(crate) fn sample_plane(pl: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let (tx, ty) = (fx - x0, fy - y0);
    let (ix, iy) = (x0 as isize, y0 as isize);
    let get = |x: isize, y: isize| if x < 0 || y < 0 || x >= w as isize || y >= h as isize { 0.0 } else { pl[y as usize * w + x as usize] };
    let a = get(ix, iy);
    let b = get(ix + 1, iy);
    let c = get(ix, iy + 1);
    let d = get(ix + 1, iy + 1);
    (a + (b - a) * tx) + ((c + (d - c) * tx) - (a + (b - a) * tx)) * ty
}

// ------------------------------------------------------------------ riso

pub struct RisoCfg<'a> {
    pub inks: &'a [[f64; 4]],
    pub seed: u64,
    pub misreg: f64,
    pub drift: f64,
    pub drift_speed: f64,
    pub t: f64,
    pub grain: f64,
    pub density: f64,
    pub paper: Option<[f64; 4]>,
}

/// Misregistration offset (node px) of plate `k` at time `t`: a fixed seeded vector of
/// length 0.4..1 x `misreg` plus a slow seeded wander of amplitude `drift`.
pub fn plate_offset(seed: u32, k: usize, misreg: f64, drift: f64, speed: f64, t: f64) -> (f64, f64) {
    let r = |i: i32| unit(hash(k as i32, i, seed)) as f64;
    let ang = r(1) * std::f64::consts::TAU;
    let mag = misreg * (0.4 + 0.6 * r(2));
    let (mut dx, mut dy) = (ang.cos() * mag, ang.sin() * mag);
    if drift != 0.0 {
        let tau = std::f64::consts::TAU;
        let w = speed * tau;
        dx += drift * (0.62 * (w * t + r(3) * tau).sin() + 0.38 * (1.73 * w * t + r(4) * tau).sin());
        dy += drift * (0.62 * (1.21 * w * t + r(5) * tau).sin() + 0.38 * (1.97 * w * t + r(6) * tau).sin());
    }
    (dx, dy)
}

/// Ink densities (0..1 per ink) that reproduce `c` (straight, relative to paper) with the
/// inks' optical densities `dd[k][ch]`: non-negative least squares on optical density
/// (Beer-Lambert), projected Gauss-Seidel.
#[inline]
fn separate(c: [f32; 3], dd: &[[f32; 3]], g: &[[f32; 4]; 4], out: &mut [f32; 4]) {
    let n = dd.len();
    let t = [-(c[0].max(0.02)).ln(), -(c[1].max(0.02)).ln(), -(c[2].max(0.02)).ln()];
    let mut b = [0f32; 4];
    for k in 0..n {
        b[k] = dd[k][0] * t[0] + dd[k][1] * t[1] + dd[k][2] * t[2];
    }
    *out = [0.0; 4];
    for _ in 0..8 {
        for k in 0..n {
            let mut r = b[k];
            for j in 0..n {
                if j != k {
                    r -= g[k][j] * out[j];
                }
            }
            out[k] = (r / g[k][k].max(1e-6)).clamp(0.0, 1.0);
        }
    }
}

/// Riso paper-tooth speckle (0..1) of plate seed `ps` at whole layer pixel (px, py).
#[inline(always)]
fn speckle_at(px: f32, py: f32, s: f32, ps: u32) -> f32 {
    let f = value_noise(px / (1.1 * s), py / (1.1 * s), ps ^ 0x5151);
    let f2 = unit(hash(px as i32, py as i32, ps ^ 0x7777));
    0.65 * smoothstep(0.35, 0.95, f) + 0.35 * f2
}

fn riso_cache_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| agentfx_ir::env::var("AGENTFX_RISO_CACHE").map_or(true, |v| v != "0"))
}

/// `speckle_at` over a rectangle of whole layer pixels.
pub(crate) struct Speckle {
    r: [i32; 4],
    v: Vec<f32>,
}

impl Speckle {
    #[inline(always)]
    fn at(&self, x: i32, y: i32) -> f32 {
        self.v[((y - self.r[1]) * self.r[2] + (x - self.r[0])) as usize]
    }
}

thread_local! {
    static SPECKLE: std::cell::RefCell<std::collections::HashMap<(u32, u32), Arc<Speckle>>> = Default::default();
}

/// The speckle plane of (`ps`, `s`) covering layer rect `need` ([x, y, w, h]), from the
/// thread's cache when it covers it; else computed with a 16 px margin (drift room).
fn speckle_plane(ps: u32, s: f32, need: [i32; 4]) -> Arc<Speckle> {
    let key = (ps, s.to_bits());
    let covers = |r: &[i32; 4]| need[0] >= r[0] && need[1] >= r[1] && need[0] + need[2] <= r[0] + r[2] && need[1] + need[3] <= r[1] + r[3];
    if let Some(p) = SPECKLE.with(|c| c.borrow().get(&key).filter(|p| covers(&p.r)).cloned()) {
        return p;
    }
    let _p = crate::prof::scope("fx.riso.speckle");
    const M: i32 = 16;
    let r = [need[0] - M, need[1] - M, need[2] + 2 * M, need[3] + 2 * M];
    let mut v = vec![0f32; (r[2] * r[3]) as usize];
    v.par_chunks_mut(r[2] as usize).enumerate().for_each(|(j, row)| {
        let py = (r[1] + j as i32) as f32;
        for (i, o) in row.iter_mut().enumerate() {
            *o = speckle_at((r[0] + i as i32) as f32, py, s, ps);
        }
    });
    let p = Arc::new(Speckle { r, v });
    SPECKLE.with(|c| {
        let mut c = c.borrow_mut();
        // bounded: a few planes per riso layer (one per ink)
        if c.len() >= 16 {
            c.clear();
        }
        c.insert(key, p.clone());
    });
    p
}

pub fn riso(buf: &mut [u8], w: usize, h: usize, cx: &FxCtx, cfg: &RisoCfg) {
    riso_impl(buf, w, h, cx, cfg, riso_cache_on())
}

fn riso_impl(buf: &mut [u8], w: usize, h: usize, cx: &FxCtx, cfg: &RisoCfg, cache: bool) {
    let n = cfg.inks.len().clamp(1, 4);
    let inks: Vec<[f32; 3]> = cfg.inks[..n].iter().map(|c| {
        // an ink with alpha < 1 prints lighter
        let a = c[3].clamp(0.0, 1.0) as f32;
        let r = rgb(*c);
        [1.0 - a * (1.0 - r[0]), 1.0 - a * (1.0 - r[1]), 1.0 - a * (1.0 - r[2])]
    }).collect();
    let dd: Vec<[f32; 3]> = inks.iter().map(|c| [-(c[0].max(0.03)).ln(), -(c[1].max(0.03)).ln(), -(c[2].max(0.03)).ln()]).collect();
    let mut g = [[0f32; 4]; 4];
    for i in 0..n {
        for j in 0..n {
            g[i][j] = (0..3).map(|c| dd[i][c] * dd[j][c]).sum();
        }
    }
    let paper = cfg.paper.map(rgb);
    let seed = cfg.seed as u32 ^ (cfg.seed >> 32) as u32;
    // density planes (coverage folded in)
    let np = w * h;
    let mut planes = vec![0f32; np * n];
    let mut alpha = vec![0f32; np];
    // ink separation memo: a direct-mapped table per worker (pure function of the pixel)
    const MEMO: usize = 4096;
    planes.par_chunks_mut(w * n).zip(alpha.par_chunks_mut(w)).enumerate().for_each_init(
        || vec![(u32::MAX, false, [0f32; 4]); MEMO],
        |memo, (y, (row, arow))| {
            let mut d = [0f32; 4];
            let mut last: Option<([u8; 4], [f32; 4])> = None;
            for x in 0..w {
                let px = &buf[(y * w + x) * 4..(y * w + x) * 4 + 4];
                let (c, a) = unpremul(px);
                arow[x] = a;
                if a <= 0.0 {
                    continue;
                }
                let key = [px[0], px[1], px[2], px[3]];
                match last {
                    Some((k, v)) if k == key => d = v,
                    _ => {
                        let packed = u32::from_le_bytes(key);
                        let slot = &mut memo[(packed.wrapping_mul(0x9E37_79B1) >> 20) as usize % MEMO];
                        if cache && slot.1 && slot.0 == packed {
                            d = slot.2;
                        } else {
                            let rel = match paper {
                                Some(pp) => [(c[0] / pp[0].max(0.02)).min(1.0), (c[1] / pp[1].max(0.02)).min(1.0), (c[2] / pp[2].max(0.02)).min(1.0)],
                                None => c,
                            };
                            separate(rel, &dd, &g, &mut d);
                            *slot = (packed, true, d);
                        }
                        last = Some((key, d));
                    }
                }
                for k in 0..n {
                    row[x * n + k] = d[k] * if paper.is_some() { 1.0 } else { a };
                }
            }
        },
    );
    // per-plate offsets in screen px
    let offs: Vec<(f32, f32)> = (0..n).map(|k| {
        let (dx, dy) = plate_offset(seed, k, cfg.misreg, cfg.drift, cfg.drift_speed, cfg.t);
        ((dx * cx.s) as f32, (dy * cx.s) as f32)
    }).collect();
    let (grain, dens) = (cfg.grain as f32, cfg.density as f32);
    let s = cx.s.max(1e-3) as f32;
    // low-frequency density variation: fbm on a coarse grid in layer coordinates (multiples of
    // G px, so partial buffers agree), bilinear per pixel
    const G: i32 = 8;
    let (l0x, l0y) = (cx.ox - cx.ax, cx.oy - cx.ay);
    let (i0, j0) = (l0x.div_euclid(G), l0y.div_euclid(G));
    let gw = (w as i32 / G + 3) as usize;
    let gh = (h as i32 / G + 3) as usize;
    let dgrid: Vec<f32> = if dens > 0.0 {
        (0..n * gw * gh).into_par_iter().map(|q| {
            let k = q / (gw * gh);
            let (gx, gy) = ((q % (gw * gh)) % gw, (q % (gw * gh)) / gw);
            let ps = seed.wrapping_add(k as u32 * 1013);
            fbm(((i0 + gx as i32) * G) as f32 / s, ((j0 + gy as i32) * G) as f32 / s, 160.0, 3, ps ^ 0xA5A5) - 0.5
        }).collect()
    } else {
        Vec::new()
    };
    // paper-tooth speckle per plate: a function of whole layer pixels only, so it is read
    // from a per-thread plane cache that outlives the frame (the drift moves the plates by
    // whole pixels inside the cached area); AGENTFX_RISO_CACHE=0 evaluates it per pixel
    let speckle: Vec<Option<Arc<Speckle>>> = (0..n)
        .map(|k| {
            if grain <= 0.0 || !cache {
                return None;
            }
            let (dx, dy) = offs[k];
            let (rx, ry) = (dx.round() as i32, dy.round() as i32);
            let ps = seed.wrapping_add(k as u32 * 1013);
            Some(speckle_plane(ps, s, [l0x - rx, l0y - ry, w as i32, h as i32]))
        })
        .collect();
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let ly_i = y as i32 + l0y;
        let gv = ly_i as f32 / G as f32 - j0 as f32;
        let (gj, ty) = (gv.floor() as usize, gv - gv.floor());
        for x in 0..w {
            let lx_i = x as i32 + l0x;
            let (lx, ly) = (lx_i as f32, ly_i as f32);
            let mut od = [0f32; 3];
            let mut any = false;
            for k in 0..n {
                let (dx, dy) = offs[k];
                // bilinear sample of plate k at (x - dx, y - dy)
                let fx = x as f32 - dx;
                let fy = y as f32 - dy;
                let x0 = fx.floor();
                let y0 = fy.floor();
                let (tx, tyy) = (fx - x0, fy - y0);
                let (ix, iy) = (x0 as isize, y0 as isize);
                let get = |xx: isize, yy: isize| if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize { 0.0 } else { planes[(yy as usize * w + xx as usize) * n + k] };
                let a0 = get(ix, iy);
                let b0 = get(ix + 1, iy);
                let c0 = get(ix, iy + 1);
                let d0 = get(ix + 1, iy + 1);
                let mut d = (a0 + (b0 - a0) * tx) + ((c0 + (d0 - c0) * tx) - (a0 + (b0 - a0) * tx)) * tyy;
                if d <= 0.0 {
                    continue;
                }
                any = true;
                let ps = seed.wrapping_add(k as u32 * 1013);
                if dens > 0.0 {
                    let gu = lx / G as f32 - i0 as f32;
                    let (gi, tx2) = (gu.floor() as usize, gu - gu.floor());
                    let base = k * gw * gh;
                    let at = |i: usize, j: usize| dgrid[base + j.min(gh - 1) * gw + i.min(gw - 1)];
                    let top = at(gi, gj) + (at(gi + 1, gj) - at(gi, gj)) * tx2;
                    let bot = at(gi, gj + 1) + (at(gi + 1, gj + 1) - at(gi, gj + 1)) * tx2;
                    d *= 1.0 + dens * 2.0 * (top + (bot - top) * ty);
                }
                if grain > 0.0 {
                    // fine speckle: ink missing in the paper tooth, slightly clumped; it lives
                    // on the plate (moves with its misregistration, whole pixels)
                    let (px_, py_) = (lx - dx.round(), ly - dy.round());
                    let sp = match &speckle[k] {
                        Some(pl) => pl.at(px_ as i32, py_ as i32),
                        None => speckle_at(px_, py_, s, ps),
                    };
                    d *= 1.0 - grain * sp;
                }
                let d = d.clamp(0.0, 1.0);
                for ch in 0..3 {
                    od[ch] += d * dd[k][ch];
                }
            }
            let c = [(-od[0]).exp(), (-od[1]).exp(), (-od[2]).exp()];
            let px = &mut row[x * 4..x * 4 + 4];
            match paper {
                Some(_) => {
                    let a = alpha[y * w + x];
                    if a > 0.0 {
                        put(px, c, a, paper);
                    }
                }
                None => {
                    if any {
                        put(px, c, 1.0, None);
                    } else {
                        px.copy_from_slice(&[0, 0, 0, 0]);
                    }
                }
            }
        }
    });
}

// ------------------------------------------------------------------ halftone

/// CMYK screens: ink colors and angles relative to `angle - 45`.
const CMYK: [([f32; 3], f32); 4] = [([0.0, 0.62, 0.89], 15.0), ([0.90, 0.10, 0.52], 75.0), ([1.0, 0.92, 0.05], 0.0), ([0.13, 0.12, 0.14], 45.0)];

#[allow(clippy::too_many_arguments)]
pub fn halftone(buf: &mut [u8], w: usize, h: usize, cx: &FxCtx, cell: f64, angle: f64, cmyk: bool, ink: [f64; 4], paper: Option<[f64; 4]>) {
    let cell = (cell * cx.s).max(1.0) as f32;
    // source averaged over about a cell (dot = mean tone of its cell)
    let mut src = buf.to_vec();
    crate::effects::blur(&mut src, w, h, (cell as f64 * 0.33).max(0.3));
    let ns = if cmyk { 4 } else { 1 };
    // tone per screen per pixel of the averaged source
    let mut tone = vec![0f32; w * h * ns];
    tone.par_chunks_mut(w * ns).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (c, a) = unpremul(&src[(y * w + x) * 4..(y * w + x) * 4 + 4]);
            if cmyk {
                let k = 1.0 - c[0].max(c[1]).max(c[2]);
                let d = (1.0 - k).max(1e-4);
                row[x * 4] = (1.0 - c[0] - k) / d * a;
                row[x * 4 + 1] = (1.0 - c[1] - k) / d * a;
                row[x * 4 + 2] = (1.0 - c[2] - k) / d * a;
                row[x * 4 + 3] = k * a;
            } else {
                let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                row[x] = (1.0 - l) * a;
            }
        }
    });
    drop(src);
    let screens: Vec<([f32; 3], f32)> = if cmyk {
        CMYK.iter().map(|(c, a)| (*c, a + angle as f32 - 45.0)).collect()
    } else {
        let a = ink[3].clamp(0.0, 1.0) as f32;
        let r = rgb(ink);
        vec![([1.0 - a * (1.0 - r[0]), 1.0 - a * (1.0 - r[1]), 1.0 - a * (1.0 - r[2])], angle as f32)]
    };
    let paper = paper.map(rgb);
    let alpha: Vec<f32> = buf.par_chunks(4).map(|p| p[3] as f32 / 255.0).collect();
    let trig: Vec<(f32, f32)> = screens.iter().map(|(_, a)| (a.to_radians().cos(), a.to_radians().sin())).collect();
    let inv_pi = 1.0 / std::f32::consts::PI;
    let (l0x, l0y) = ((cx.ox - cx.ax) as f32, (cx.oy - cx.ay) as f32);
    // dot radius per cell and screen: the tone sampled once at each cell centre
    struct Grid {
        u0: i32,
        v0: i32,
        gw: usize,
        gh: usize,
        r: Vec<f32>,
    }
    let grids: Vec<Grid> = (0..screens.len()).map(|si| {
        let (co, sn) = trig[si];
        let corners = [(l0x, l0y), (l0x + w as f32, l0y), (l0x, l0y + h as f32), (l0x + w as f32, l0y + h as f32)];
        let us: Vec<f32> = corners.iter().map(|(x, y)| (x * co + y * sn) / cell).collect();
        let vs: Vec<f32> = corners.iter().map(|(x, y)| (-x * sn + y * co) / cell).collect();
        let u0 = us.iter().cloned().fold(f32::INFINITY, f32::min).floor() as i32 - 2;
        let v0 = vs.iter().cloned().fold(f32::INFINITY, f32::min).floor() as i32 - 2;
        let gw = (us.iter().cloned().fold(f32::NEG_INFINITY, f32::max).floor() as i32 + 3 - u0) as usize;
        let gh = (vs.iter().cloned().fold(f32::NEG_INFINITY, f32::max).floor() as i32 + 3 - v0) as usize;
        let r: Vec<f32> = (0..gw * gh).into_par_iter().map(|q| {
            let (cu, cv) = ((u0 + (q % gw) as i32) as f32 + 0.5, (v0 + (q / gw) as i32) as f32 + 0.5);
            let gx = (cu * co - cv * sn) * cell - l0x;
            let gy = (cu * sn + cv * co) * cell - l0y;
            let k = sample_plane_n(&tone, w, h, ns, si, gx, gy);
            if k <= 0.002 { -1.0 } else { cell * (k.min(1.0) * inv_pi).sqrt() * 1.02 }
        }).collect();
        Grid { u0, v0, gw, gh, r }
    }).collect();
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            // layer-local position of the pixel centre
            let lx = x as f32 + l0x + 0.5;
            let ly = y as f32 + l0y + 0.5;
            let mut c = [1f32; 3];
            let mut cov_any = 0f32;
            for (si, (ink, _)) in screens.iter().enumerate() {
                let (co, sn) = trig[si];
                let g = &grids[si];
                // into the rotated cell grid
                let u = (lx * co + ly * sn) / cell;
                let v = (-lx * sn + ly * co) / cell;
                let (iu, iv) = (u.floor() as i32 - g.u0, v.floor() as i32 - g.v0);
                let mut cov = 0f32;
                for dj in -1..=1 {
                    let j = iv + dj;
                    if j < 0 || j as usize >= g.gh {
                        continue;
                    }
                    for di in -1..=1 {
                        let i = iu + di;
                        if i < 0 || i as usize >= g.gw {
                            continue;
                        }
                        let r = g.r[j as usize * g.gw + i as usize];
                        if r <= 0.0 {
                            continue;
                        }
                        let du = (u - (g.u0 + i) as f32 - 0.5) * cell;
                        let dv = (v - (g.v0 + j) as f32 - 0.5) * cell;
                        let d2 = du * du + dv * dv;
                        if d2 >= (r + 0.5) * (r + 0.5) {
                            continue;
                        }
                        cov = cov.max((r - d2.sqrt() + 0.5).clamp(0.0, 1.0));
                    }
                }
                if cov > 0.0 {
                    cov_any = cov_any.max(cov);
                    for ch in 0..3 {
                        c[ch] *= 1.0 - cov * (1.0 - ink[ch]);
                    }
                }
            }
            let px = &mut row[x * 4..x * 4 + 4];
            match paper {
                Some(_) => {
                    let a = alpha[y * w + x];
                    // paper inside the layer, plus dots that spill past its edge
                    put(px, c, a.max(cov_any), paper);
                }
                None => put(px, c, 1.0, None),
            }
        }
    });
}

#[inline(always)]
fn sample_plane_n(pl: &[f32], w: usize, h: usize, n: usize, k: usize, x: f32, y: f32) -> f32 {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let (tx, ty) = (fx - x0, fy - y0);
    let (ix, iy) = (x0 as isize, y0 as isize);
    let get = |x: isize, y: isize| if x < 0 || y < 0 || x >= w as isize || y >= h as isize { 0.0 } else { pl[(y as usize * w + x as usize) * n + k] };
    let a = get(ix, iy);
    let b = get(ix + 1, iy);
    let c = get(ix, iy + 1);
    let d = get(ix + 1, iy + 1);
    (a + (b - a) * tx) + ((c + (d - c) * tx) - (a + (b - a) * tx)) * ty
}

// ------------------------------------------------------------------ grain

/// Monochrome grain inside the alpha: +-amount, triangular distribution, `size` px grains
/// (value noise above 1 px). AVX2 build of the row loop when available.
#[allow(clippy::too_many_arguments)]
pub fn grain(buf: &mut [u8], w: usize, _h: usize, cx: &FxCtx, amount: f64, seed: u64, live: bool, size: f64) {
    if amount <= 0.0 {
        return;
    }
    let mut sd = seed as u32 ^ (seed >> 32) as u32;
    if live {
        sd = sd.wrapping_add(cx.frame.wrapping_mul(0x9E37_79B1));
    }
    let amt = (amount * 255.0) as f32;
    let size = (size * cx.s).max(0.25) as f32;
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let ly = y as i32 + cx.oy - cx.ay;
        let x0 = cx.ox - cx.ax;
        grain_row(row, w, x0, ly, sd, amt, size);
    });
}

fn grain_row(row: &mut [u8], w: usize, x0: i32, ly: i32, sd: u32, amt: f32, size: f32) {
    #[cfg(target_arch = "x86_64")]
    if size <= 1.0 && std::is_x86_feature_detected!("avx2") {
        // SAFETY: AVX2 is present (checked above).
        unsafe { grain_row_fine_avx2(row, w, x0, ly, sd, amt) };
        return;
    }
    if size <= 1.0 {
        grain_row_fine(row, w, x0, ly, sd, amt);
        return;
    }
    for x in 0..w {
        let lx = (x as i32 + x0) as f32;
        let n = value_noise(lx / size, ly as f32 / size, sd) + value_noise(lx / size, ly as f32 / size, sd ^ 0x3c3c) - 1.0;
        add_grain(&mut row[x * 4..x * 4 + 4], n * 1.6 * amt);
    }
}

#[inline(always)]
fn add_grain(px: &mut [u8], d: f32) {
    let a = px[3] as f32;
    if a == 0.0 {
        return;
    }
    let d = d * a / 255.0;
    for c in px[..3].iter_mut() {
        *c = (*c as f32 + d).clamp(0.0, a).round() as u8;
    }
}

#[inline(always)]
fn grain_row_fine_body(row: &mut [u8], w: usize, x0: i32, ly: i32, sd: u32, amt: f32) {
    // per-pixel hashes are integer ops in a straight loop: LLVM vectorizes it under AVX2
    let mut noise = vec![0f32; w];
    for (x, v) in noise.iter_mut().enumerate() {
        let lx = x as i32 + x0;
        *v = unit(hash(lx, ly, sd)) + unit(hash(lx, ly, sd ^ 0x3c3c)) - 1.0;
    }
    for (px, n) in row.chunks_exact_mut(4).zip(&noise) {
        add_grain(px, n * amt);
    }
}

fn grain_row_fine(row: &mut [u8], w: usize, x0: i32, ly: i32, sd: u32, amt: f32) {
    grain_row_fine_body(row, w, x0, ly, sd, amt)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn grain_row_fine_avx2(row: &mut [u8], w: usize, x0: i32, ly: i32, sd: u32, amt: f32) {
    grain_row_fine_body(row, w, x0, ly, sd, amt)
}

// ------------------------------------------------------------------ paper

/// Ink bleed + rough edges (blur, then a noise-perturbed threshold of the alpha) and paper
/// texture (fibers darken slightly, tooth lets `color` show through).
#[allow(clippy::too_many_arguments)]
pub fn paper(buf: &mut [u8], w: usize, h: usize, cx: &FxCtx, texture: f64, bleed: f64, rough: f64, seed: u64, color: [f64; 4]) {
    let sd = seed as u32 ^ (seed >> 32) as u32;
    let s = cx.s.max(1e-3) as f32;
    let (texture, rough) = (texture.max(0.0) as f32, rough.clamp(0.0, 1.0) as f32);
    let pc = rgb(color);
    let edges = bleed > 0.0 || rough > 0.0;
    let blurred = if edges {
        let mut b = buf.to_vec();
        crate::effects::blur(&mut b, w, h, (bleed * cx.s).max(0.8));
        Some(b)
    } else {
        None
    };
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let lx = (x as i32 + cx.ox - cx.ax) as f32;
            let ly = (y as i32 + cx.oy - cx.ay) as f32;
            let i = (y * w + x) * 4;
            let px = &mut row[x * 4..x * 4 + 4];
            let (mut c, mut a) = unpremul(px);
            if let Some(b) = &blurred {
                let (cb, ab) = unpremul(&b[i..i + 4]);
                if ab <= 0.003 && a <= 0.0 {
                    continue;
                }
                let n = fbm(lx / s, ly / s, 7.0, 3, sd ^ 0xED6E) - 0.5;
                let th = 0.5 + rough * 0.7 * n;
                let a2 = smoothstep(th - 0.09, th + 0.09, ab);
                if a < 0.5 {
                    c = cb;
                }
                a = a2;
            }
            if a <= 0.0 {
                px.copy_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            if texture > 0.0 {
                // fibers: long thin streaks; tooth: sparse pits
                let fib = value_noise(lx / (2.0 * s), ly / (26.0 * s), sd ^ 0xF1B) * 0.6 + value_noise(lx / (31.0 * s), ly / (2.4 * s), sd ^ 0xF2B) * 0.4;
                let dark = 1.0 - texture * 0.22 * smoothstep(0.45, 0.95, fib);
                let tooth = smoothstep(0.72, 0.97, value_noise(lx / (1.6 * s), ly / (1.6 * s), sd ^ 0x7007)) * texture;
                for ch in 0..3 {
                    c[ch] = (c[ch] * dark) * (1.0 - tooth) + pc[ch] * tooth;
                }
            }
            for ch in 0..3 {
                px[ch] = ((c[ch] * a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            px[3] = (a * 255.0 + 0.5) as u8;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> FxCtx {
        FxCtx { ox: 0, oy: 0, ax: 0, ay: 0, s: 1.0, frame: 0 }
    }
    fn poster(w: usize, h: usize) -> Vec<u8> {
        let mut b = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let p = if (x / 16 + y / 16) % 2 == 0 { [230, 40, 120, 255] } else if x > w / 2 { [0, 90, 190, 255] } else { [0, 0, 0, 0] };
                b[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&p);
            }
        }
        b
    }
    fn valid(b: &[u8]) -> bool {
        b.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
    }

    #[test]
    fn separation_reproduces_ink_colors() {
        let inks = [[1.0, 0.28, 0.69, 1.0], [0.0, 0.47, 0.75, 1.0], [1.0, 0.91, 0.0, 1.0]];
        let dd: Vec<[f32; 3]> = inks.iter().map(|c| [-((c[0] as f32).max(0.03)).ln(), -((c[1] as f32).max(0.03)).ln(), -((c[2] as f32).max(0.03)).ln()]).collect();
        let mut g = [[0f32; 4]; 4];
        for i in 0..3 {
            for j in 0..3 {
                g[i][j] = (0..3).map(|c| dd[i][c] * dd[j][c]).sum();
            }
        }
        let mut d = [0f32; 4];
        separate([1.0, 1.0, 1.0], &dd, &g, &mut d);
        assert!(d.iter().all(|&v| v < 1e-3), "paper = no ink: {d:?}");
        separate([0.0, 0.47, 0.75], &dd, &g, &mut d);
        assert!(d[1] > 0.9 && d[0] < 0.2 && d[2] < 0.2, "pure blue plate: {d:?}");
    }

    #[test]
    fn looks_are_deterministic_and_valid() {
        let (w, h) = (96, 64);
        let inks = vec![[1.0, 0.28, 0.69, 1.0], [0.0, 0.47, 0.75, 1.0], [1.0, 0.91, 0.0, 1.0]];
        let effects = [
            Effect::Riso { inks: inks.clone(), seed: 3, misreg: 2.5, drift: 1.0, drift_speed: 0.3, grain: 0.4, density: 0.3, paper: None },
            Effect::Riso { inks, seed: 3, misreg: 2.5, drift: 0.0, drift_speed: 0.3, grain: 0.4, density: 0.3, paper: Some([0.96, 0.94, 0.9, 1.0]) },
            Effect::Halftone { cell: 9.0, angle: 45.0, mode: "mono".into(), ink: [0.1, 0.1, 0.1, 1.0], paper: None },
            Effect::Halftone { cell: 7.0, angle: 30.0, mode: "cmyk".into(), ink: [0.1, 0.1, 0.1, 1.0], paper: Some([1.0; 4]) },
            Effect::Grain { amount: 0.2, seed: 1, live: true, size: 1.0 },
            Effect::Grain { amount: 0.2, seed: 1, live: false, size: 2.5 },
            Effect::Paper { texture: 0.6, bleed: 2.0, rough: 0.7, seed: 4, color: [0.97, 0.95, 0.9, 1.0] },
        ];
        for e in &effects {
            let mut p = [0f64; NPARAM];
            for (i, f) in e.look_fields().iter().enumerate() {
                p[i] = e.look_field(f);
            }
            p[TIME] = 0.4;
            let mut a = poster(w, h);
            let mut b = poster(w, h);
            apply(e, &p, &mut a, w, h, &cx());
            apply(e, &p, &mut b, w, h, &cx());
            assert_eq!(a, b, "{e:?} deterministic");
            assert!(valid(&a), "{e:?} premultiplied");
            assert_ne!(a, poster(w, h), "{e:?} changes pixels");
        }
    }

    #[test]
    fn riso_speckle_cache_is_bitwise_the_direct_evaluation() {
        // drifting plates over many frames and node offsets: cached planes (reused and
        // regrown) give the per-pixel speckle exactly
        let (w, h) = (120, 70);
        let inks = vec![[1.0, 0.28, 0.69, 1.0], [0.0, 0.47, 0.75, 1.0], [1.0, 0.91, 0.0, 1.0], [0.1, 0.1, 0.1, 1.0]];
        for (i, t) in [0.0, 0.13, 0.5, 1.7, 3.3, 3.31, 9.0].into_iter().enumerate() {
            for paper in [None, Some([0.96, 0.94, 0.9])] {
                let cfg = RisoCfg { inks: &inks, seed: 11, misreg: 4.0, drift: 3.0, drift_speed: 0.5, t, grain: 0.5, density: 0.3, paper: paper.map(|p: [f64; 3]| [p[0], p[1], p[2], 1.0]) };
                let c = FxCtx { ox: 7 * i as i32 - 20, oy: 3 - 5 * i as i32, ax: 2, ay: -1, s: [1.0, 1.0, 2.0][i % 3], frame: i as u32 };
                let (mut a, mut b) = (poster(w, h), poster(w, h));
                riso_impl(&mut a, w, h, &c, &cfg, false);
                riso_impl(&mut b, w, h, &c, &cfg, true);
                assert_eq!(a, b, "t {t}");
            }
        }
    }

    #[test]
    fn live_grain_changes_per_frame_static_does_not() {
        let (w, h) = (32, 32);
        let run = |live: bool, frame: u32| {
            let mut b = vec![128u8; w * h * 4];
            b.chunks_exact_mut(4).for_each(|p| p[3] = 255);
            grain(&mut b, w, h, &FxCtx { frame, ..cx() }, 0.2, 9, live, 1.0);
            b
        };
        assert_eq!(run(false, 0), run(false, 5));
        assert_ne!(run(true, 0), run(true, 5));
        assert_eq!(run(true, 5), run(true, 5));
    }

    #[test]
    fn grain_avx2_matches_scalar() {
        let w = 77;
        let mut a: Vec<u8> = (0..w * 4).map(|i| if i % 4 == 3 { 200 } else { (i * 7 % 200) as u8 }).collect();
        let mut b = a.clone();
        grain_row_fine(&mut a, w, -13, 5, 77, 40.0);
        grain_row(&mut b, w, -13, 5, 77, 40.0, 1.0);
        assert_eq!(a, b);
    }

    #[test]
    fn noise_anchored_to_the_node() {
        // the same layer one pixel further right (buffer and anchor move together) gives the
        // same pixels: grain/paper texture travel with the layer
        let (w, h) = (40, 30);
        let base = poster(w, h);
        let e = Effect::Paper { texture: 0.8, bleed: 1.0, rough: 0.5, seed: 2, color: [1.0; 4] };
        let p = [0.8, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut a = base.clone();
        let mut b = base.clone();
        apply(&e, &p, &mut a, w, h, &FxCtx { ox: 10, oy: 4, ax: 10, ay: 4, ..cx() });
        apply(&e, &p, &mut b, w, h, &FxCtx { ox: 11, oy: 4, ax: 11, ay: 4, ..cx() });
        assert_eq!(a, b);
    }

    #[test]
    fn plate_offsets_seeded_and_bounded() {
        for k in 0..3 {
            let (dx, dy) = plate_offset(7, k, 3.0, 0.0, 0.25, 0.0);
            let m = (dx * dx + dy * dy).sqrt();
            assert!((1.2 - 1e-9..=3.0 + 1e-9).contains(&m), "{m}");
            assert_eq!(plate_offset(7, k, 3.0, 2.0, 0.25, 1.3), plate_offset(7, k, 3.0, 2.0, 0.25, 1.3));
            let (ex, ey) = plate_offset(7, k, 3.0, 2.0, 0.25, 1.3);
            assert!(((ex - dx).powi(2) + (ey - dy).powi(2)).sqrt() <= 2.0 * 2f64.sqrt() + 1e-9);
        }
        assert_ne!(plate_offset(7, 0, 3.0, 0.0, 0.25, 0.0), plate_offset(8, 0, 3.0, 0.0, 0.25, 0.0));
    }
}
