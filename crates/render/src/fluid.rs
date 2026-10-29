//! 2D fluid (X5): stable fluids (Jos Stam) on a reduced grid, simulated once and sequentially,
//! cached per frame as u8 coverage at grid resolution, so frame n is a pure function of n and
//! every painter (parallel segments, single frames) sees identical fields.
//!
//! Step k simulates time `k / fps` (emitters active at that time) and yields frame k:
//! forces (pour jets, stirs, injected node coverage, turbulence, buoyancy), vorticity
//! confinement, viscosity (implicit, Jacobi), pressure projection (multigrid V-cycles, warm start),
//! semi-Lagrangian velocity advection, MacCormack dye advection (clamped), dissipation.
//! Grid units: one cell, velocities in cells/s. Fields are SoA `f32` rows; the hot loops are
//! plain element-wise row passes (auto-vectorized; an AVX2 copy is selected at run time,
//! without FMA contraction, so results are bit-identical across the two paths).
use anyhow::Result;
use rayon::prelude::*;
use agentfx_ir::{FluidContent, FluidEmitter, NodeId};
use std::sync::{Arc, Mutex};

/// Coverage below half a u8 step is invisible: dye under this (x density) is "gone".
const COV_EPS: f64 = 0.5 / 255.0;
/// Feature sizes (scene px) of the turbulence and the source kick noise.
const TURB_SCALE: f64 = 220.0;
const KICK_SCALE: f64 = 70.0;

pub struct Fluid {
    pub c: FluidContent,
    pub gw: usize,
    pub gh: usize,
    fps: f64,
    /// first frame with dye, last frame with visible dye (None = until the end)
    first: Option<usize>,
    settle: Option<usize>,
    /// simulation frames available (scene frames, plus the time offset)
    max_frames: usize,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    frames: Vec<Arc<Vec<u8>>>,
    sim: Option<Sim>,
    pub sim_s: f64,
    pub mask_s: f64,
}

/// Simulation state (grid, SoA).
pub struct Sim {
    w: usize,
    h: usize,
    u: Vec<f32>,
    v: Vec<f32>,
    d: Vec<f32>,
    p: Vec<f32>,
    t0: Vec<f32>,
    t1: Vec<f32>,
    t2: Vec<f32>,
    t3: Vec<f32>,
}

/// Dye coverage masks of the source emitters active at a step, in emitter order.
pub type Masks = Vec<Option<Vec<f32>>>;

impl Fluid {
    pub fn new(c: &FluidContent, fps: f64, frames: usize) -> Fluid {
        let (gw, gh) = agentfx_ir::validate::fluid_grid(c);
        let mut f = Fluid { c: c.clone(), gw, gh, fps, first: None, settle: None, max_frames: frames.max(1) + (c.time_offset * fps).round().max(0.0) as usize + 1, cache: Mutex::new(Cache::default()) };
        f.plan();
        f
    }

    /// Analytic visibility window: dye only enters through pours and sources, advection never
    /// raises the maximum and dissipation shrinks it by `q` per frame, so after the last
    /// injection the field is below the visible threshold within a known number of frames.
    fn plan(&mut self) {
        let fps = self.fps;
        let mut first: Option<usize> = None;
        let mut last = 0usize;
        let mut dmax = 0.0;
        for e in &self.c.emitters {
            let (a, b) = e.window();
            let (fa, fb) = step_window(a, b, fps);
            let amount = match e {
                FluidEmitter::Pour { rate, .. } => rate * (fb.saturating_sub(fa)) as f64 / fps,
                FluidEmitter::Source { amount, .. } => *amount,
                FluidEmitter::Stir { .. } => continue,
            };
            if amount <= 0.0 || fb <= fa {
                continue;
            }
            first = Some(first.map_or(fa, |x: usize| x.min(fa)));
            last = last.max(fb - 1);
            dmax += amount;
        }
        self.first = first;
        let thr = -(1.0 - COV_EPS).ln() / self.c.density;
        let q = self.c.dissipation.powf(30.0 / fps);
        self.settle = match first {
            None => Some(0),
            Some(_) if q < 1.0 && dmax > thr => Some(last + ((thr / dmax).ln() / q.ln()).ceil().max(0.0) as usize + 1),
            Some(_) if dmax <= thr => Some(last + 1),
            Some(_) => None,
        };
    }

    /// Simulation frame position shown at simulation time `s` (None: nothing visible).
    pub fn frame_pos(&self, s: f64) -> Option<f64> {
        let first = self.first?;
        let p = (s * self.fps).clamp(0.0, (self.max_frames - 1) as f64);
        if (p.ceil() as usize) < first {
            return None;
        }
        if let Some(st) = self.settle {
            if p.floor() as usize > st {
                return None;
            }
        }
        // snap near-integer positions (track rounding) to the frame
        let r = p.round();
        Some(if (p - r).abs() < 1e-4 { r } else { p })
    }

    pub fn bounds(&self, s: f64) -> Option<[f64; 4]> {
        self.frame_pos(s).map(|_| self.c.rect)
    }

    pub fn sig(&self, s: f64) -> u64 {
        match self.frame_pos(s) {
            None => 0,
            Some(p) => p.to_bits() ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    #[allow(dead_code)]
    pub fn first_frame(&self) -> Option<usize> {
        self.first
    }
    #[allow(dead_code)]
    pub fn settle_frame(&self) -> Option<usize> {
        self.settle
    }

    /// Bytes held by the frame cache so far, sim seconds and mask seconds.
    #[allow(dead_code)]
    pub fn stats(&self) -> (usize, usize, f64, f64) {
        let c = self.cache.lock().unwrap();
        (c.frames.len(), c.frames.len() * self.gw * self.gh, c.sim_s, c.mask_s)
    }

    /// Coverage (0..1) at frame position `p` (lerp of the two cached frames), simulating up to
    /// them first. `mask(emitter index, node, sim time)` rasterizes a source node's coverage
    /// at grid resolution; it is called with the cache locked (sequential).
    pub fn coverage(&self, p: f64, mask: &mut dyn FnMut(usize, NodeId, f64) -> Result<Vec<f32>>) -> Result<Vec<f32>> {
        let k0 = p.floor() as usize;
        let fr = (p - k0 as f64) as f32;
        let k1 = if fr > 0.0 { k0 + 1 } else { k0 };
        let (a, b) = {
            let mut c = self.cache.lock().unwrap();
            self.ensure(&mut c, k1, mask)?;
            (c.frames[k0].clone(), c.frames[k1].clone())
        };
        let mut out = vec![0f32; self.gw * self.gh];
        if k1 == k0 {
            for (o, &x) in out.iter_mut().zip(a.iter()) {
                *o = x as f32 / 255.0;
            }
        } else {
            for ((o, &x), &y) in out.iter_mut().zip(a.iter()).zip(b.iter()) {
                *o = (x as f32 + (y as f32 - x as f32) * fr) / 255.0;
            }
        }
        Ok(out)
    }

    fn ensure(&self, c: &mut Cache, k: usize, mask: &mut dyn FnMut(usize, NodeId, f64) -> Result<Vec<f32>>) -> Result<()> {
        let k = k.min(self.max_frames - 1);
        while c.frames.len() <= k {
            let step = c.frames.len();
            let t = step as f64 / self.fps;
            let tm = std::time::Instant::now();
            let mut masks: Masks = vec![None; self.c.emitters.len()];
            for (i, e) in self.c.emitters.iter().enumerate() {
                if let FluidEmitter::Source { node, .. } = e {
                    if active(e, step, self.fps) {
                        let _p = crate::prof::scope("fluid.mask");
                        masks[i] = Some(mask(i, *node, t)?);
                    }
                }
            }
            c.mask_s += tm.elapsed().as_secs_f64();
            let ts = std::time::Instant::now();
            let _p = crate::prof::scope_px("fluid.sim", (self.gw * self.gh) as u64);
            let sim = c.sim.get_or_insert_with(|| Sim::new(self.gw, self.gh));
            sim.step(&self.c, step, self.fps, &masks);
            let frame = Arc::new(sim.coverage_u8(self.c.density));
            c.sim_s += ts.elapsed().as_secs_f64();
            c.frames.push(frame);
        }
        Ok(())
    }
}

impl Drop for Fluid {
    fn drop(&mut self) {
        let c = self.cache.lock().unwrap();
        if !c.frames.is_empty() && agentfx_ir::env::var("AGENTFX_QUIET").is_err() {
            let k = c.frames.len();
            eprintln!(
                "fluid {}x{} grid: {k} frames simulated, sim {:.2} s ({:.1} ms/frame), source masks {:.2} s, cache {:.1} MB",
                self.gw, self.gh, c.sim_s, c.sim_s * 1e3 / k as f64, c.mask_s, (k * self.gw * self.gh) as f64 / 1048576.0
            );
        }
    }
}

/// Steps [a, b) during which an emitter with window [start, end) is active (step k at k/fps).
fn step_window(start: f64, end: f64, fps: f64) -> (usize, usize) {
    let a = (start * fps - 1e-6).ceil().max(0.0) as usize;
    let mut b = (end * fps - 1e-6).ceil().max(0.0) as usize;
    if b <= a && end > start {
        // shorter than a step: one step at the first frame time not before start
        b = a + 1;
    }
    (a, b)
}

fn active(e: &FluidEmitter, step: usize, fps: f64) -> bool {
    let (a, b) = e.window();
    let (sa, sb) = step_window(a, b, fps);
    step >= sa && step < sb
}

// ---------------------------------------------------------------- noise

fn hash3(x: i32, y: i32, z: i32, seed: u64) -> f32 {
    let mut h = seed ^ 0x51_7cc1_b727_220a;
    for v in [x as u32 as u64, y as u32 as u64, z as u32 as u64] {
        h = (h ^ v).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        h ^= h >> 29;
    }
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 32;
    (h & 0xff_ffff) as f32 / 0xff_ffff as f32 * 2.0 - 1.0
}

/// Smooth value noise in (x, y, z), -1..1.
fn vnoise(x: f32, y: f32, z: f32, seed: u64) -> f32 {
    let (xi, yi, zi) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (x - xi, y - yi, z - zi);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy, sz) = (s(fx), s(fy), s(fz));
    let (xi, yi, zi) = (xi as i32, yi as i32, zi as i32);
    let mut acc = [0f32; 2];
    for (k, a) in acc.iter_mut().enumerate() {
        let z = zi + k as i32;
        let c00 = hash3(xi, yi, z, seed);
        let c10 = hash3(xi + 1, yi, z, seed);
        let c01 = hash3(xi, yi + 1, z, seed);
        let c11 = hash3(xi + 1, yi + 1, z, seed);
        let a0 = c00 + (c10 - c00) * sx;
        let a1 = c01 + (c11 - c01) * sx;
        *a = a0 + (a1 - a0) * sy;
    }
    acc[0] + (acc[1] - acc[0]) * sz
}

/// Two-octave potential for curl noise.
fn potential(x: f32, y: f32, z: f32, seed: u64) -> f32 {
    vnoise(x, y, z, seed) + 0.5 * vnoise(x * 2.03 + 17.1, y * 2.03 - 5.3, z * 1.7, seed ^ 0xabc)
}

/// Divergence-free noise field (curl of `potential`) on the grid, per cell `(cx, cy)`; `scale`
/// = feature size in cells. Unit-ish magnitude.
fn curl_field(w: usize, h: usize, scale: f32, z: f32, seed: u64, cx: &mut [f32], cy: &mut [f32]) {
    let inv = 1.0 / scale.max(1.0);
    // potential on cell corners (w+1 x h+1), then centred differences per cell
    let pw = w + 1;
    let pot: Vec<f32> = (0..(h + 1) * pw).into_par_iter().map(|i| potential((i % pw) as f32 * inv, (i / pw) as f32 * inv, z, seed)).collect();
    let k = scale.max(1.0) * 0.5;
    cx.par_chunks_mut(w).zip(cy.par_chunks_mut(w)).enumerate().for_each(|(y, (rx, ry))| {
        for x in 0..w {
            let p00 = pot[y * pw + x];
            let p10 = pot[y * pw + x + 1];
            let p01 = pot[(y + 1) * pw + x];
            let p11 = pot[(y + 1) * pw + x + 1];
            let dpdy = (p01 + p11 - p00 - p10) * 0.5;
            let dpdx = (p10 + p11 - p00 - p01) * 0.5;
            rx[x] = dpdy * k;
            ry[x] = -dpdx * k;
        }
    });
}

// ---------------------------------------------------------------- kernels

#[inline(always)]
fn bilerp(f: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let x0 = (x as usize).min(w - 2);
    let y0 = (y as usize).min(h - 2);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let i = y0 * w + x0;
    let a = f[i] + (f[i + 1] - f[i]) * fx;
    let b = f[i + w] + (f[i + w + 1] - f[i + w]) * fx;
    a + (b - a) * fy
}

#[inline(always)]
fn minmax4(f: &[f32], w: usize, h: usize, x: f32, y: f32) -> (f32, f32) {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let x0 = (x as usize).min(w - 2);
    let y0 = (y as usize).min(h - 2);
    let i = y0 * w + x0;
    let (a, b, c, d) = (f[i], f[i + 1], f[i + w], f[i + w + 1]);
    (a.min(b).min(c.min(d)), a.max(b).max(c.max(d)))
}

/// out = src advected back along (u, v) * dt (semi-Lagrangian).
fn advect(src: &[f32], u: &[f32], v: &[f32], w: usize, h: usize, dt: f32, out: &mut [f32]) {
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            *o = bilerp(src, w, h, x as f32 - dt * u[i], y as f32 - dt * v[i]);
        }
    });
}

/// One weighted Jacobi sweep of the Poisson equation `(sum of neighbours - 4 p) / h2 = b`
/// (Neumann walls: a missing neighbour is the cell itself).
#[inline(always)]
fn jacobi_row(p: &[f32], b: &[f32], w: usize, h: usize, y: usize, h2: f32, om: f32, out: &mut [f32]) {
    let ym = y.saturating_sub(1);
    let yp = (y + 1).min(h - 1);
    let (r, rm, rp) = (&p[y * w..y * w + w], &p[ym * w..ym * w + w], &p[yp * w..yp * w + w]);
    let bv = &b[y * w..y * w + w];
    if w == 1 {
        out[0] = r[0] + om * ((r[0] * 2.0 + rm[0] + rp[0] - h2 * bv[0]) * 0.25 - r[0]);
        return;
    }
    out[0] = r[0] + om * ((r[0] + r[1] + rm[0] + rp[0] - h2 * bv[0]) * 0.25 - r[0]);
    for x in 1..w - 1 {
        out[x] = r[x] + om * ((r[x - 1] + r[x + 1] + rm[x] + rp[x] - h2 * bv[x]) * 0.25 - r[x]);
    }
    out[w - 1] = r[w - 1] + om * ((r[w - 2] + r[w - 1] + rm[w - 1] + rp[w - 1] - h2 * bv[w - 1]) * 0.25 - r[w - 1]);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn jacobi_row_avx2(p: &[f32], b: &[f32], w: usize, h: usize, y: usize, h2: f32, om: f32, out: &mut [f32]) {
    jacobi_row(p, b, w, h, y, h2, om, out)
}

#[allow(clippy::too_many_arguments)]
fn jacobi_w(p: &[f32], b: &[f32], w: usize, h: usize, h2: f32, om: f32, out: &mut [f32], avx2: bool) {
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        #[cfg(target_arch = "x86_64")]
        if avx2 {
            // SAFETY: AVX2 support was detected at run time.
            unsafe { jacobi_row_avx2(p, b, w, h, y, h2, om, row) };
            return;
        }
        let _ = avx2;
        jacobi_row(p, b, w, h, y, h2, om, row);
    });
}

#[allow(dead_code)]
fn jacobi(p: &[f32], div: &[f32], w: usize, h: usize, out: &mut [f32], avx2: bool) {
    jacobi_w(p, div, w, h, 1.0, 1.0, out, avx2)
}

/// Residual `b - A p` of the Poisson equation at spacing^2 `h2`.
fn residual(p: &[f32], b: &[f32], w: usize, h: usize, h2: f32, out: &mut [f32]) {
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let ym = y.saturating_sub(1);
        let yp = (y + 1).min(h - 1);
        for x in 0..w {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(w - 1);
            let i = y * w + x;
            let s = p[y * w + xm] + p[y * w + xp] + p[ym * w + x] + p[yp * w + x] - 4.0 * p[i];
            row[x] = b[i] - s / h2;
        }
    });
}

/// Multigrid V-cycle for `A p = b` on a w x h cell-centred grid (spacing^2 `h2`): weighted
/// Jacobi smoothing, 2x2 averaging restriction, bilinear prolongation. Deterministic.
#[allow(clippy::too_many_arguments)]
fn vcycle(p: &mut Vec<f32>, b: &[f32], w: usize, h: usize, h2: f32, avx2: bool, scratch: &mut Vec<f32>) {
    const NU: usize = 3;
    const OM: f32 = 0.8;
    scratch.resize(w * h, 0.0);
    if w <= 4 || h <= 4 {
        for _ in 0..60 {
            jacobi_w(p, b, w, h, h2, OM, scratch, avx2);
            std::mem::swap(p, scratch);
        }
        return;
    }
    for _ in 0..NU {
        jacobi_w(p, b, w, h, h2, OM, scratch, avx2);
        std::mem::swap(p, scratch);
    }
    residual(p, b, w, h, h2, scratch);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let r = &*scratch;
    let rc: Vec<f32> = (0..cw * ch).into_par_iter().map(|i| {
        let (cx, cy) = (i % cw, i / cw);
        let mut s = 0.0;
        let mut n = 0.0;
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (x, y) = (2 * cx + dx, 2 * cy + dy);
            if x < w && y < h {
                s += r[y * w + x];
                n += 1.0;
            }
        }
        s / n
    }).collect();
    let mut ec = vec![0f32; cw * ch];
    let mut sc = vec![];
    vcycle(&mut ec, &rc, cw, ch, h2 * 4.0, avx2, &mut sc);
    p.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let gy = (y as f32 + 0.5) * 0.5 - 0.5;
        for (x, v) in row.iter_mut().enumerate() {
            let gx = (x as f32 + 0.5) * 0.5 - 0.5;
            *v += if cw >= 2 && ch >= 2 { bilerp(&ec, cw, ch, gx, gy) } else { ec[0] };
        }
    });
    scratch.resize(w * h, 0.0);
    for _ in 0..NU {
        jacobi_w(p, b, w, h, h2, OM, scratch, avx2);
        std::mem::swap(p, scratch);
    }
}

/// In-place separable 1-2-1 blur (clamped edges), `tmp` scratch of the same size.
fn blur121(f: &mut [f32], tmp: &mut [f32], w: usize, h: usize) {
    {
        let src = &*f;
        tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let r = &src[y * w..y * w + w];
            for x in 0..w {
                row[x] = 0.25 * (r[x.saturating_sub(1)] + 2.0 * r[x] + r[(x + 1).min(w - 1)]);
            }
        });
    }
    let src = &*tmp;
    f.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (ym, yp) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            row[x] = 0.25 * (src[ym * w + x] + 2.0 * src[y * w + x] + src[yp * w + x]);
        }
    });
}

fn has_avx2() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::is_x86_feature_detected!("avx2") && agentfx_ir::env::var("AGENTFX_FLUID_NO_AVX2").is_err()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

impl Sim {
    pub fn new(w: usize, h: usize) -> Sim {
        let z = || vec![0f32; w * h];
        Sim { w, h, u: z(), v: z(), d: z(), p: z(), t0: z(), t1: z(), t2: z(), t3: z() }
    }

    #[allow(dead_code)]
    pub fn dye(&self) -> &[f32] {
        &self.d
    }

    pub fn coverage_u8(&self, density: f64) -> Vec<u8> {
        let k = density as f32;
        self.d.par_iter().map(|&d| ((1.0 - (-k * d.max(0.0)).exp()) * 255.0 + 0.5).clamp(0.0, 255.0) as u8).collect()
    }

    /// Advance by one frame: step `k` at simulation time k/fps.
    pub fn step(&mut self, c: &FluidContent, k: usize, fps: f64, masks: &Masks) {
        let (w, h) = (self.w, self.h);
        let dt = (1.0 / fps) as f32;
        let t = k as f64 / fps;
        let res = c.res.clamp(0.05, 1.0);
        let gx = |x: f64| ((x - c.rect[0]) * res - 0.5) as f32;
        let gy = |y: f64| ((y - c.rect[1]) * res - 0.5) as f32;
        let avx2 = has_avx2();
        // ---- forces
        for (ei, e) in c.emitters.iter().enumerate() {
            if !active(e, k, fps) {
                continue;
            }
            match e {
                FluidEmitter::Pour { at, dir, speed, radius, rate, wobble, start, .. } => {
                    let wob = if *wobble != 0.0 { wobble * vnoise((t - start) as f32 * 1.7, 0.5, ei as f32 * 3.1, c.seed ^ 0x707) as f64 } else { 0.0 };
                    let a = (dir + wob).to_radians();
                    let (tu, tv) = ((speed * res * a.cos()) as f32, (speed * res * a.sin()) as f32);
                    let (cx, cy, r) = (gx(at[0]), gy(at[1]), (radius * res).max(0.5) as f32);
                    let amt = (*rate as f32) * dt;
                    self.splat(cx, cy, r, |d, u, v, wgt| {
                        *d += amt * wgt;
                        *u += (tu - *u) * wgt;
                        *v += (tv - *v) * wgt;
                    });
                }
                FluidEmitter::Stir { at, radius, strength, .. } => {
                    let (cx, cy, r) = (gx(at[0]), gy(at[1]), (radius * res).max(0.5) as f32);
                    let s = *strength as f32;
                    let pull = (dt * 8.0).min(1.0);
                    let (ww, hh) = (w, h);
                    let r2 = r * r;
                    let x0 = ((cx - 3.0 * r).floor().max(0.0)) as usize;
                    let x1 = ((cx + 3.0 * r).ceil().min((ww - 1) as f32)) as usize;
                    let y0 = ((cy - 3.0 * r).floor().max(0.0)) as usize;
                    let y1 = ((cy + 3.0 * r).ceil().min((hh - 1) as f32)) as usize;
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                            let wgt = (-(dx * dx + dy * dy) / r2).exp() * pull;
                            let i = y * ww + x;
                            self.u[i] += (-s * dy - self.u[i]) * wgt;
                            self.v[i] += (s * dx - self.v[i]) * wgt;
                        }
                    }
                }
                FluidEmitter::Source { start, end, amount, speed, dir, noise, .. } => {
                    let Some(m) = masks.get(ei).and_then(|m| m.as_ref()) else { continue };
                    let (sa, sb) = step_window(*start, *end, fps);
                    let share = (*amount / (sb - sa).max(1) as f64) as f32;
                    let a = dir.to_radians();
                    let (bu, bv) = ((speed * res * a.cos()) as f32, (speed * res * a.sin()) as f32);
                    let nz = (*noise * res) as f32;
                    if nz != 0.0 {
                        curl_field(w, h, (KICK_SCALE * res) as f32, ei as f32 * 7.3 + 0.37, c.seed ^ 0x5eed, &mut self.t0, &mut self.t1);
                    }
                    let first = k == sa;
                    for i in 0..w * h {
                        let mi = m[i];
                        if mi <= 0.0 {
                            continue;
                        }
                        self.d[i] += share * mi;
                        // velocity: set inside the coverage on the first step, nudged after
                        let wgt = if first { mi } else { mi * (dt * 4.0).min(1.0) };
                        let (tu, tv) = if nz != 0.0 { (bu + nz * self.t0[i], bv + nz * self.t1[i]) } else { (bu, bv) };
                        self.u[i] += (tu - self.u[i]) * wgt;
                        self.v[i] += (tv - self.v[i]) * wgt;
                    }
                }
            }
        }
        if c.turbulence > 0.0 {
            curl_field(w, h, (TURB_SCALE * res) as f32, (t * 0.35) as f32, c.seed, &mut self.t0, &mut self.t1);
            let a = (c.turbulence * res) as f32 * dt;
            for i in 0..w * h {
                self.u[i] += a * self.t0[i];
                self.v[i] += a * self.t1[i];
            }
        }
        if c.buoyancy != 0.0 {
            let b = (c.buoyancy * res) as f32 * dt;
            for i in 0..w * h {
                self.v[i] -= b * self.d[i].min(4.0);
            }
        }
        // ---- vorticity confinement
        if c.vorticity > 0.0 {
            let (u, v) = (&self.u, &self.v);
            let curl = &mut self.t2;
            curl.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                let ym = y.saturating_sub(1);
                let yp = (y + 1).min(h - 1);
                for x in 0..w {
                    let xm = x.saturating_sub(1);
                    let xp = (x + 1).min(w - 1);
                    row[x] = 0.5 * ((v[y * w + xp] - v[y * w + xm]) - (u[yp * w + x] - u[ym * w + x]));
                }
            });
            // smooth the curl (two 1-2-1 tent passes each way): confinement fed by grid-scale
            // curl amplifies the grid's own noise into sawtooth edges
            for _ in 0..2 {
                blur121(&mut self.t2, &mut self.t3, w, h);
            }
            let eps = c.vorticity as f32 * dt;
            let curl = &self.t2;
            self.u.par_chunks_mut(w).zip(self.v.par_chunks_mut(w)).enumerate().for_each(|(y, (ru, rv))| {
                if y == 0 || y == h - 1 {
                    return;
                }
                for x in 1..w - 1 {
                    let i = y * w + x;
                    let nx = 0.5 * (curl[i + 1].abs() - curl[i - 1].abs());
                    let ny = 0.5 * (curl[i + w].abs() - curl[i - w].abs());
                    let l = (nx * nx + ny * ny).sqrt() + 1e-5;
                    let wv = curl[i];
                    ru[x] += eps * (ny / l) * wv;
                    rv[x] -= eps * (nx / l) * wv;
                }
            });
        }
        // ---- viscosity: implicit diffusion, a = nu dt N^2 (N = grid width)
        let a = (c.viscosity * (1.0 / fps) * (w * w) as f64) as f32;
        if a > 1e-4 {
            for comp in 0..2 {
                let f0 = if comp == 0 { self.u.clone() } else { self.v.clone() };
                let iters = 12;
                for _ in 0..iters {
                    let cur = if comp == 0 { &self.u } else { &self.v };
                    let out = &mut self.t3;
                    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                        let ym = y.saturating_sub(1);
                        let yp = (y + 1).min(h - 1);
                        for x in 0..w {
                            let xm = x.saturating_sub(1);
                            let xp = (x + 1).min(w - 1);
                            let s = cur[y * w + xm] + cur[y * w + xp] + cur[ym * w + x] + cur[yp * w + x];
                            row[x] = (f0[y * w + x] + a * s) / (1.0 + 4.0 * a);
                        }
                    });
                    if comp == 0 {
                        std::mem::swap(&mut self.u, &mut self.t3);
                    } else {
                        std::mem::swap(&mut self.v, &mut self.t3);
                    }
                }
            }
        }
        self.walls();
        // ---- projection
        {
            let (u, v) = (&self.u, &self.v);
            self.t2.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                let ym = y.saturating_sub(1);
                let yp = (y + 1).min(h - 1);
                for x in 0..w {
                    let xm = x.saturating_sub(1);
                    let xp = (x + 1).min(w - 1);
                    row[x] = 0.5 * (u[y * w + xp] - u[y * w + xm] + v[yp * w + x] - v[ym * w + x]);
                }
            });
        }
        for p in self.p.iter_mut() {
            *p *= 0.8;
        }
        for _ in 0..c.iterations.clamp(1, 20) {
            vcycle(&mut self.p, &self.t2, w, h, 1.0, avx2, &mut self.t3);
        }
        {
            let p = &self.p;
            self.u.par_chunks_mut(w).zip(self.v.par_chunks_mut(w)).enumerate().for_each(|(y, (ru, rv))| {
                let ym = y.saturating_sub(1);
                let yp = (y + 1).min(h - 1);
                for x in 0..w {
                    let xm = x.saturating_sub(1);
                    let xp = (x + 1).min(w - 1);
                    ru[x] -= 0.5 * (p[y * w + xp] - p[y * w + xm]);
                    rv[x] -= 0.5 * (p[yp * w + x] - p[ym * w + x]);
                }
            });
        }
        self.walls();
        // ---- advect velocity (semi-Lagrangian)
        advect(&self.u, &self.u, &self.v, w, h, dt, &mut self.t0);
        advect(&self.v, &self.u, &self.v, w, h, dt, &mut self.t1);
        std::mem::swap(&mut self.u, &mut self.t0);
        std::mem::swap(&mut self.v, &mut self.t1);
        // velocity was advected with itself: use the new field for the dye (stable either way)
        // ---- advect dye (MacCormack, clamped to the forward sample's neighbourhood)
        advect(&self.d, &self.u, &self.v, w, h, dt, &mut self.t2);
        advect(&self.t2, &self.u, &self.v, w, h, -dt, &mut self.t3);
        {
            let mc: f32 = 0.5;
            let (d, fwd, back, u, v) = (&self.d, &self.t2, &self.t3, &self.u, &self.v);
            self.t0.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    let i = y * w + x;
                    let (lo, hi) = minmax4(d, w, h, x as f32 - dt * u[i], y as f32 - dt * v[i]);
                    *o = (fwd[i] + mc * (d[i] - back[i])).clamp(lo, hi);
                }
            });
        }
        std::mem::swap(&mut self.d, &mut self.t0);
        // ---- dissipation
        let q = c.dissipation.powf(30.0 / fps) as f32;
        let dr = c.drag.powf(30.0 / fps) as f32;
        self.d.par_iter_mut().for_each(|x| *x *= q);
        if dr < 1.0 {
            self.u.par_iter_mut().for_each(|x| *x *= dr);
            self.v.par_iter_mut().for_each(|x| *x *= dr);
        }
    }

    fn walls(&mut self) {
        let (w, h) = (self.w, self.h);
        for y in 0..h {
            self.u[y * w] = 0.0;
            self.u[y * w + w - 1] = 0.0;
        }
        for x in 0..w {
            self.v[x] = 0.0;
            self.v[(h - 1) * w + x] = 0.0;
        }
    }

    fn splat(&mut self, cx: f32, cy: f32, r: f32, mut f: impl FnMut(&mut f32, &mut f32, &mut f32, f32)) {
        let (w, h) = (self.w, self.h);
        let r2 = r * r;
        let x0 = (cx - 3.0 * r).floor().max(0.0) as usize;
        let x1 = (cx + 3.0 * r).ceil().min((w - 1) as f32).max(0.0) as usize;
        let y0 = (cy - 3.0 * r).floor().max(0.0) as usize;
        let y1 = (cy + 3.0 * r).ceil().min((h - 1) as f32).max(0.0) as usize;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let wgt = (-(dx * dx + dy * dy) / r2).exp();
                if wgt < 1e-4 {
                    continue;
                }
                let i = y * w + x;
                let (mut d, mut u, mut v) = (self.d[i], self.u[i], self.v[i]);
                f(&mut d, &mut u, &mut v, wgt);
                self.d[i] = d;
                self.u[i] = u;
                self.v[i] = v;
            }
        }
    }
}

// ---------------------------------------------------------------- display

/// Mitchell-Netravali (B = C = 1/3) weights for fractional offset `t` (taps -1..2): a bicubic
/// upsample with a slight blur, which hides the grid.
#[inline(always)]
fn mitchell(t: f32) -> [f32; 4] {
    let k = |x: f32| {
        let x = x.abs();
        if x < 1.0 {
            (7.0 * x * x * x - 12.0 * x * x + 16.0 / 3.0) / 6.0
        } else if x < 2.0 {
            (-7.0 / 3.0 * x * x * x + 12.0 * x * x - 20.0 * x + 32.0 / 3.0) / 6.0
        } else {
            0.0
        }
    };
    [k(1.0 + t), k(t), k(1.0 - t), k(2.0 - t)]
}

/// Paint coverage field `cov` (gw x gh, grid cell centers at content (rect.xy + (i+0.5)/res))
/// into the premultiplied RGBA buffer `out` (w x h device px at device origin (ox, oy)),
/// through `inv`: device -> content affine [a b c d e f]. Axis-aligned: separable Mitchell;
/// otherwise bilinear.
#[allow(clippy::too_many_arguments)]
pub fn paint(cov: &[f32], gw: usize, gh: usize, c: &FluidContent, inv: [f64; 6], ox: f64, oy: f64, out: &mut [u8], w: usize, h: usize, as_matte: bool) {
    debug_assert_eq!(out.len(), w * h * 4);
    let res = c.res.clamp(0.05, 1.0);
    let col = [c.color[0] as f32, c.color[1] as f32, c.color[2] as f32, c.color[3] as f32];
    let shade = c.shade as f32;
    // `edge`: drawn normally only the rim (coverage below edge, fading out above it); as a matte
    // source the full coverage (the revealed layer fills the stain's core)
    let edge = c.edge as f32;
    let ss = |a: f32, b: f32, x: f32| {
        let u = ((x - a) / (b - a)).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    };
    // device pixel center -> grid coordinate
    let gxy = |dx: f64, dy: f64| -> (f32, f32) {
        let (x, y) = (dx + ox + 0.5, dy + oy + 0.5);
        let cxp = inv[0] * x + inv[2] * y + inv[4];
        let cyp = inv[1] * x + inv[3] * y + inv[5];
        (((cxp - c.rect[0]) * res - 0.5) as f32, ((cyp - c.rect[1]) * res - 0.5) as f32)
    };
    let px = |a: f32| -> [u8; 4] {
        let a = a.clamp(0.0, 1.0);
        if a <= 0.0 {
            return [0; 4];
        }
        let s = 1.0 - shade * a * a;
        let a = if edge > 0.0 && edge < 1.0 {
            if as_matte {
                // the revealed layer: opaque from just above the rim's middle on
                ss(0.5 * edge, (edge + 0.15).min(1.0), a)
            } else {
                // an opaque rim (coverage 0.5 edge .. edge) fading out over the core
                ss(0.0, 0.5 * edge, a) * (1.0 - ss(edge, (edge + 0.35).min(1.0), a))
            }
        } else {
            a
        };
        if a <= 0.0 {
            return [0; 4];
        }
        let al = a * col[3];
        [(col[0] * s * al * 255.0 + 0.5) as u8, (col[1] * s * al * 255.0 + 0.5) as u8, (col[2] * s * al * 255.0 + 0.5) as u8, (al * 255.0 + 0.5) as u8]
    };
    let at = |x: isize, y: isize| cov[(y.clamp(0, gh as isize - 1) as usize) * gw + x.clamp(0, gw as isize - 1) as usize];
    if inv[1] == 0.0 && inv[2] == 0.0 {
        // separable: column taps, then rows
        let cols: Vec<(isize, [f32; 4])> = (0..w).map(|x| {
            let (g, _) = gxy(x as f64, 0.0);
            let i = g.floor();
            (i as isize, mitchell(g - i))
        }).collect();
        let _ = h;
        out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            let (_, g) = gxy(0.0, y as f64);
            let j = g.floor();
            let wy = mitchell(g - j);
            let j = j as isize;
            // the four grid rows blended vertically first (gw values), then horizontal taps
            let mut line = vec![0f32; gw];
            for (k, wk) in wy.iter().enumerate() {
                let r = (j - 1 + k as isize).clamp(0, gh as isize - 1) as usize;
                let src = &cov[r * gw..r * gw + gw];
                for (l, s) in line.iter_mut().zip(src) {
                    *l += wk * s;
                }
            }
            for (x, p) in row.chunks_exact_mut(4).enumerate() {
                let (i, wx) = cols[x];
                let mut a = 0.0;
                for (k, wk) in wx.iter().enumerate() {
                    a += wk * line[(i - 1 + k as isize).clamp(0, gw as isize - 1) as usize];
                }
                p.copy_from_slice(&px(a));
            }
        });
    } else {
        out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for (x, p) in row.chunks_exact_mut(4).enumerate() {
                let (gx, gy) = gxy(x as f64, y as f64);
                if gx < -1.0 || gy < -1.0 || gx > gw as f32 || gy > gh as f32 {
                    p.copy_from_slice(&[0; 4]);
                    continue;
                }
                let (i, j) = (gx.floor(), gy.floor());
                let (fx, fy) = (gx - i, gy - j);
                let (i, j) = (i as isize, j as isize);
                let a0 = at(i, j) + (at(i + 1, j) - at(i, j)) * fx;
                let a1 = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * fx;
                p.copy_from_slice(&px(a0 + (a1 - a0) * fy));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> FluidContent {
        serde_json::from_value(serde_json::json!({
            "rect": [0, 0, 320, 180], "res": 0.25, "vorticity": 12, "turbulence": 300, "seed": 3, "color": [0.9, 0.27, 0.18, 1],
            "emitters": [
                {"type": "pour", "start": 0, "end": 0.5, "at": [80, 150], "dir": -80, "speed": 500, "radius": 10, "rate": 6, "wobble": 15},
                {"type": "stir", "start": 0.2, "end": 0.6, "at": [160, 90], "radius": 40, "strength": 4}
            ]
        })).unwrap()
    }

    #[test]
    fn deterministic_and_frame_independent_of_request_order() {
        let c = content();
        let a = Fluid::new(&c, 30.0, 30);
        let b = Fluid::new(&c, 30.0, 30);
        let mut no = |_: usize, _: NodeId, _: f64| -> Result<Vec<f32>> { unreachable!() };
        // b is asked for a late frame first (one sequential simulation either way)
        let late_b = b.coverage(20.0, &mut no).unwrap();
        for k in 0..=20 {
            let ca = a.coverage(k as f64, &mut no).unwrap();
            if k == 20 {
                assert_eq!(ca, late_b);
            }
        }
        let s: f32 = late_b.iter().sum();
        assert!(s > 1.0, "dye visible");
    }

    #[test]
    fn avx2_and_scalar_paths_match() {
        let (w, h) = (97, 53);
        let p: Vec<f32> = (0..w * h).map(|i| hash3(i as i32, 1, 2, 7)).collect();
        let d: Vec<f32> = (0..w * h).map(|i| hash3(i as i32, 3, 4, 9) * 0.3).collect();
        let (mut a, mut b) = (vec![0f32; w * h], vec![0f32; w * h]);
        jacobi(&p, &d, w, h, &mut a, false);
        jacobi(&p, &d, w, h, &mut b, has_avx2());
        assert_eq!(a, b);
    }

    #[test]
    fn settles_and_bounds_vanish() {
        let mut c = content();
        c.dissipation = 0.9;
        let f = Fluid::new(&c, 30.0, 300);
        let st = f.settle_frame().expect("settles");
        assert!(st > 15 && st < 200, "settle {st}");
        assert!(f.bounds(0.0).is_some());
        assert!(f.bounds((st + 2) as f64 / 30.0).is_none());
        // the simulated field really is invisible there
        let mut no = |_: usize, _: NodeId, _: f64| -> Result<Vec<f32>> { unreachable!() };
        let cov = f.coverage((st + 1) as f64, &mut no).unwrap();
        assert!(cov.iter().all(|&x| x == 0.0));
        // nothing before the first pour
        c.emitters[0] = FluidEmitter::Pour { start: 1.0, end: 1.5, at: [80.0, 150.0], dir: 0.0, speed: 100.0, radius: 10.0, rate: 3.0, wobble: 0.0 };
        let f = Fluid::new(&c, 30.0, 300);
        assert!(f.bounds(0.9).is_none());
        assert!(f.bounds(1.0).is_some());
        assert_ne!(f.sig(1.0), f.sig(1.0 + 1.0 / 30.0));
        assert_eq!(f.sig(0.5), f.sig(0.6));
    }

    #[test]
    fn projection_reduces_divergence() {
        let c = content();
        let mut s = Sim::new(80, 45);
        for k in 0..6 {
            s.step(&c, k, 30.0, &vec![None; 2]);
        }
        let (w, h) = (s.w, s.h);
        let mut div = 0.0f64;
        let mut mag = 0.0f64;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                div += (0.5 * (s.u[i + 1] - s.u[i - 1] + s.v[i + w] - s.v[i - w])).abs() as f64;
                mag += (s.u[i].abs() + s.v[i].abs()) as f64;
            }
        }
        assert!(mag > 0.0 && div / mag < 0.1, "div/mag {}", div / mag);
    }
}
