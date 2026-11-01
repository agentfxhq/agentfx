//! Shared chaos-game density buffer for the orbit kinds (flame, attractor): many seeded chains
//! plot their points into one histogram of hit counts and summed colour, then a log-density
//! tone map turns it into a glowing, transparent image.
//!
//! Determinism: the chains are a fixed decomposition of the sample budget (`CHAINS`, each with
//! its own seeded PRNG), and every hit adds integers with atomic adds. Integer addition is
//! commutative, so the histogram (and the image) is identical for any thread count or schedule.
//!
//! Resolution independence: the budget is samples per device pixel of the layer rect, and the
//! tone map measures density against the mean of the lit pixels, not the brightest pixel, so
//! a larger output or budget only makes the image smoother.
use super::raster::{bloom, invert};
use super::{palette, Input};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// Independent chains the budget is split into (fixed: part of the image's definition).
pub const CHAINS: u64 = 64;
/// Hard ceiling of samples per image (keeps a huge layer or `samples=` bounded).
const MAX_TOTAL: f64 = 1.2e8;

/// SplitMix64: a small seeded PRNG for the chains (never per-pixel noise).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64, stream: u64) -> Rng {
        let mut r = Rng(seed ^ stream.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xd1b5_4a32_d192_ed03);
        r.next();
        r
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
    /// Uniform in [-1, 1).
    pub fn sym(&mut self) -> f64 {
        2.0 * self.unit() - 1.0
    }
}

/// Total samples for a budget of `per_px` samples per device pixel of the layer rect.
pub fn total(g: &Input, per_px: f64) -> u64 {
    let det = (g.m[0] * g.m[3] - g.m[1] * g.m[2]).abs().max(1e-300);
    let rect_px = 4.0 * g.aspect / det;
    (per_px * rect_px).clamp(CHAINS as f64 * 64.0, MAX_TOTAL) as u64
}

/// 256-entry colour table for a palette coordinate 0..1: palette at `speed * c + offset`,
/// brightness-normalised (the hue comes from the palette, the brightness from the density).
pub fn lut(pal: &[[f32; 4]], speed: f32, offset: f32) -> Vec<[u32; 3]> {
    (0..256)
        .map(|i| {
            let c = palette(pal, speed.mul_add(i as f32 / 255.0, offset));
            let mx = c[0].max(c[1]).max(c[2]).max(0.05);
            [0, 1, 2].map(|k| ((c[k] / mx).clamp(0.0, 1.0) * 255.0).round() as u32)
        })
        .collect()
}

/// Hit counts and summed colour per device pixel.
pub struct Hist {
    w: usize,
    h: usize,
    /// unit space -> pixel
    px: [f64; 6],
    hits: Vec<AtomicU32>,
    rgb: Vec<AtomicU32>,
}

impl Hist {
    pub fn new(g: &Input) -> Hist {
        let n = g.w * g.h;
        Hist { w: g.w, h: g.h, px: invert(g.m), hits: (0..n).map(|_| AtomicU32::new(0)).collect(), rgb: (0..3 * n).map(|_| AtomicU32::new(0)).collect() }
    }

    /// Plot a unit-space point with a table colour.
    #[inline]
    pub fn add(&self, u: f64, v: f64, c: [u32; 3]) {
        let p = &self.px;
        let x = p[0] * u + p[2] * v + p[4];
        let y = p[1] * u + p[3] * v + p[5];
        // also rejects NaN
        if !(x >= 0.0 && y >= 0.0 && x < self.w as f64 && y < self.h as f64) {
            return;
        }
        let i = y as usize * self.w + x as usize;
        self.hits[i].fetch_add(1, Relaxed);
        self.rgb[3 * i].fetch_add(c[0], Relaxed);
        self.rgb[3 * i + 1].fetch_add(c[1], Relaxed);
        self.rgb[3 * i + 2].fetch_add(c[2], Relaxed);
    }

    /// Log-density tone map into premultiplied RGBA8 with glow. Density is measured against
    /// the mean over the lit pixels (so a compact attractor and a frame-filling flame expose
    /// alike, at any resolution and budget); `gamma` > 1 lifts faint filaments, `glow` is the
    /// bloom strength.
    ///
    /// Sparse pixels are smoothed (a light density estimation): counts and colour are blended
    /// towards a 5x5 binomial blur of themselves by `4 / (4 + hits)`, which turns the speckle of
    /// a few stray hits into haze and leaves dense filaments sharp.
    pub fn tone(&self, g: &Input, gamma: f64, glow: f32, out: &mut [u8]) {
        use rayon::prelude::*;
        // `REF` x the mean lit density reaches full brightness
        const REF: f64 = 24.0;
        let norm = 1.0 / (1.0 + REF).ln();
        let inv_g = 1.0 / gamma.max(0.05);
        let (w, h) = (self.w, self.h);
        // [hits, r, g, b] as f32, then the separable 5x5 binomial blur of it
        let raw: Vec<[f32; 4]> = (0..w * h)
            .into_par_iter()
            .map(|i| [self.hits[i].load(Relaxed) as f32, self.rgb[3 * i].load(Relaxed) as f32, self.rgb[3 * i + 1].load(Relaxed) as f32, self.rgb[3 * i + 2].load(Relaxed) as f32])
            .collect();
        const B: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
        let pass = |src: &[[f32; 4]], horiz: bool| -> Vec<[f32; 4]> {
            (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = ((i % w) as isize, (i / w) as isize);
                    let mut acc = [0f32; 4];
                    for (j, wt) in B.iter().enumerate() {
                        let d = j as isize - 2;
                        let (sx, sy) = if horiz { (x + d, y) } else { (x, y + d) };
                        if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                            let s = src[sy as usize * w + sx as usize];
                            for c in 0..4 {
                                acc[c] += wt * s[c];
                            }
                        }
                    }
                    acc
                })
                .collect()
        };
        let soft = pass(&pass(&raw, true), false);
        let (sum, lit) = raw.par_iter().map(|p| (p[0] as f64, (p[0] > 0.0) as u64)).reduce(|| (0.0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        let k = lit.max(1) as f64 / sum.max(1.0);
        let mut core = vec![0u8; w * h * 4];
        core.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let i = y * w + x;
                let mix = 4.0 / (4.0 + raw[i][0]);
                let px: [f32; 4] = [0, 1, 2, 3].map(|c| raw[i][c] + (soft[i][c] - raw[i][c]) * mix);
                let n = px[0];
                if n <= 1e-4 {
                    continue;
                }
                let lum = ((1.0 + n as f64 * k).ln() * norm).min(1.0).powf(inv_g) as f32;
                // the densest cores burn towards white
                let white = 0.4 * lum.powi(8);
                let d = crate::look::unit(crate::look::hash(g.org.0 + x as i32, g.org.1 + y as i32, g.seed as u32));
                let o = &mut row[x * 4..x * 4 + 4];
                for c in 0..3 {
                    let avg = (px[c + 1] / (255.0 * n)).min(1.0);
                    let v = (avg + (1.0 - avg) * white) * lum;
                    o[c] = (v * 255.0 + d).clamp(0.0, 255.0) as u8;
                }
                o[3] = (lum * 255.0 + d).clamp(0.0, 255.0) as u8;
            }
        });
        let n = core.len();
        out[..n].copy_from_slice(&core);
        if glow > 0.0 {
            // radii in device pixels, relative to the layer's size so a preview and a full
            // render glow alike
            let ppu = 1.0 / (g.m[0] * g.m[3] - g.m[1] * g.m[2]).abs().sqrt().max(1e-300);
            bloom(&core, out, w, h, (0.008 * ppu).max(1.0), 0.32 * glow);
            bloom(&core, out, w, h, (0.04 * ppu).max(2.0), 0.28 * glow);
        }
    }
}
