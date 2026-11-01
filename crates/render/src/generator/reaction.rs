//! Gray-Scott reaction-diffusion (parameters: `agentfx_ir::generator::KINDS`, "reaction") on a
//! torus grid (Karl Sims' formulation: 9-point Laplacian, dt 1). The grid covers the rect,
//! 180 rows at zoom 1; it starts from seeded colonies of V and runs `warmup + floor(t * speed)`
//! steps, rendered with bicubic sampling as glowing tissue: a dim tinted ground, bright bodies
//! and a luminous rim along the reaction front. CPU only.
//!
//! Purity: a frame is a function of (parameters, seed, t) alone: the state at step n is the
//! same whichever way it was reached. A small cache keeps the latest states per configuration
//! and continues from the nearest earlier one; every step is computed cell by cell from the
//! previous grid in a fixed order (any thread count), so a cached continuation is bit-identical
//! to a fresh simulation (test `scrub_equals_fresh_simulation`). Animating feed, kill,
//! diffusion or zoom changes the configuration, so such frames replay from step 0.
use super::{palette, Generator, Input};
use std::sync::Mutex;

const FEED: usize = 0;
const KILL: usize = 1;
const DU: usize = 2;
const DV: usize = 3;
const SPEED: usize = 4;
const WARMUP: usize = 5;
const ZOOM: usize = 6;
const CSPEED: usize = 7;
const OFFSET: usize = 8;

/// Most steps a frame runs (bounds a long scene or a huge `speed`).
const MAX_STEPS: u64 = 200_000;
/// Grid rows at zoom 1.
const ROWS: f64 = 180.0;
/// Most grid cells.
const MAX_CELLS: usize = 1_500_000;
/// Cached states (a few configurations rendered side by side).
const CACHE_LEN: usize = 6;

/// One simulation configuration (cache key).
#[derive(Clone, Copy, PartialEq)]
struct Cfg {
    cols: usize,
    rows: usize,
    feed: f32,
    kill: f32,
    du: f32,
    dv: f32,
    seed: u64,
}

struct State {
    cfg: Cfg,
    step: u64,
    u: Vec<f32>,
    v: Vec<f32>,
}

static CACHE: Mutex<Vec<State>> = Mutex::new(Vec::new());

fn hash(x: u32) -> u32 {
    crate::look::hash(x as i32, 0x5eed, 0x9e37_79b9)
}

/// The seeded start: U = 1 everywhere, soft colonies of V (about one per 2500 cells).
fn initial(c: &Cfg) -> (Vec<f32>, Vec<f32>) {
    let (cols, rows) = (c.cols, c.rows);
    let mut u = vec![1.0f32; cols * rows];
    let mut v = vec![0.0f32; cols * rows];
    let n = (cols * rows / 2500).max(3);
    let s = c.seed as u32;
    for i in 0..n as u32 {
        let h = hash(s ^ i.wrapping_mul(0x2c1b_3c6d));
        let (cx, cy) = ((h % cols as u32) as isize, (hash(h) % rows as u32) as isize);
        let r = 3 + (hash(h ^ 0xabcd) % 4) as isize;
        for y in cy - r..=cy + r {
            for x in cx - r..=cx + r {
                if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r {
                    let at = y.rem_euclid(rows as isize) as usize * cols + x.rem_euclid(cols as isize) as usize;
                    u[at] = 0.5;
                    v[at] = 0.25 + 0.05 * (hash(h ^ at as u32) & 0xffff) as f32 / 65535.0;
                }
            }
        }
    }
    (u, v)
}

/// Advance (u, v) by `n` steps.
fn run(c: &Cfg, u: &mut Vec<f32>, v: &mut Vec<f32>, n: u64) {
    use rayon::prelude::*;
    let (cols, rows) = (c.cols, c.rows);
    let mut nu = vec![0f32; u.len()];
    let mut nv = vec![0f32; v.len()];
    for _ in 0..n {
        let (cu, cv) = (&*u, &*v);
        nu.par_chunks_mut(cols).zip(nv.par_chunks_mut(cols)).enumerate().for_each(|(y, (ru, rv))| {
            let (ym, yp) = ((y + rows - 1) % rows * cols, (y + 1) % rows * cols);
            let y0 = y * cols;
            // one cell from its column neighbours xm, xp (the torus wraps only at the ends)
            let cell = |x: usize, xm: usize, xp: usize, ru: &mut [f32], rv: &mut [f32]| {
                let lap = |f: &[f32]| (f[y0 + xm] + f[y0 + xp] + f[ym + x] + f[yp + x]) * 0.2 + (f[ym + xm] + f[ym + xp] + f[yp + xm] + f[yp + xp]) * 0.05 - f[y0 + x];
                let (a, b) = (cu[y0 + x], cv[y0 + x]);
                let abb = a * b * b;
                ru[x] = (a + c.du * lap(cu) - abb + c.feed * (1.0 - a)).clamp(0.0, 1.0);
                rv[x] = (b + c.dv * lap(cv) + abb - (c.kill + c.feed) * b).clamp(0.0, 1.0);
            };
            cell(0, cols - 1, 1, ru, rv);
            for x in 1..cols - 1 {
                cell(x, x - 1, x + 1, ru, rv);
            }
            cell(cols - 1, cols - 2, 0, ru, rv);
        });
        std::mem::swap(u, &mut nu);
        std::mem::swap(v, &mut nv);
    }
}

/// The state after `step` steps; `cache` continues from (and updates) the state cache.
fn state(c: &Cfg, step: u64, cache: bool) -> (Vec<f32>, Vec<f32>) {
    let found = if cache {
        let m = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        m.iter().filter(|s| s.cfg == *c && s.step <= step).max_by_key(|s| s.step).map(|s| (s.step, s.u.clone(), s.v.clone()))
    } else {
        None
    };
    let (from, mut u, mut v) = found.unwrap_or_else(|| {
        let (u, v) = initial(c);
        (0, u, v)
    });
    run(c, &mut u, &mut v, step - from);
    if cache && step > from {
        let mut m = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        // replace this configuration's state at `from` (the one continued), else the oldest
        if let Some(i) = m.iter().position(|s| s.cfg == *c && s.step == from) {
            m.remove(i);
        } else if m.len() >= CACHE_LEN {
            m.remove(0);
        }
        m.push(State { cfg: *c, step, u: u.clone(), v: v.clone() });
    }
    (u, v)
}

/// Catmull-Rom weights for fraction t.
#[inline]
fn cubic(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [0.5 * (-t3 + 2.0 * t2 - t), 0.5 * (3.0 * t3 - 5.0 * t2 + 2.0), 0.5 * (-3.0 * t3 + 4.0 * t2 + t), 0.5 * (t3 - t2)]
}

/// Bicubic sample of a torus field at grid point (x, y) (cell centres at integers).
#[inline]
fn bicubic(f: &[f32], cols: usize, rows: usize, x: f64, y: f64) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (wx, wy) = (cubic((x - fx) as f32), cubic((y - fy) as f32));
    let (ix, iy) = (fx as i64, fy as i64);
    let mut acc = 0.0;
    for (j, wyj) in wy.iter().enumerate() {
        let row = (iy + j as i64 - 1).rem_euclid(rows as i64) as usize * cols;
        let mut r = 0.0;
        for (i, wxi) in wx.iter().enumerate() {
            r += wxi * f[row + (ix + i as i64 - 1).rem_euclid(cols as i64) as usize];
        }
        acc += wyj * r;
    }
    acc
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub struct Reaction;

impl Reaction {
    fn cfg(g: &Input) -> Cfg {
        let mut rows = (ROWS / g.p[ZOOM]).round().max(16.0);
        let mut cols = (rows * g.aspect).round().max(16.0);
        let cells = rows * cols;
        if cells > MAX_CELLS as f64 {
            let k = (MAX_CELLS as f64 / cells).sqrt();
            rows = (rows * k).floor().max(16.0);
            cols = (cols * k).floor().max(16.0);
        }
        Cfg { cols: cols as usize, rows: rows as usize, feed: g.p[FEED] as f32, kill: g.p[KILL] as f32, du: g.p[DU] as f32, dv: g.p[DV] as f32, seed: g.seed }
    }

    fn steps(g: &Input) -> u64 {
        (g.p[WARMUP].round().max(0.0) + (g.t.max(0.0) * g.p[SPEED]).floor()).min(MAX_STEPS as f64) as u64
    }

    fn draw(g: &Input, c: &Cfg, u: &[f32], v: &[f32], out: &mut [u8]) {
        use rayon::prelude::*;
        let (cols, rows) = (c.cols, c.rows);
        let (cs, co) = (g.p[CSPEED] as f32, g.p[OFFSET] as f32);
        let m = g.m;
        // unit (u in +-aspect, v in +-1) -> grid (cell centres at integers)
        let (kx, ky) = (cols as f64 / (2.0 * g.aspect), rows as f64 / 2.0);
        out.par_chunks_mut(g.w * 4).enumerate().take(g.h).for_each(|(y, row)| {
            for x in 0..g.w {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let uu = m[0] * px + m[2] * py + m[4];
                let vv = m[1] * px + m[3] * py + m[5];
                let (gx, gy) = ((uu + g.aspect) * kx - 0.5, (vv + 1.0) * ky - 0.5);
                let b = (bicubic(v, cols, rows, gx, gy) / 0.42).clamp(0.0, 1.0);
                let a = bicubic(u, cols, rows, gx, gy).clamp(0.0, 1.0);
                let body = smooth(0.32, 0.62, b);
                let rim = (-((b - 0.36) * (1.0 / 0.11)).powi(2)).exp();
                // hue: chemistry plus a slow diagonal sweep across the rect
                let sweep = 0.22 * (uu / g.aspect) as f32 + 0.14 * vv as f32;
                let pc = palette(g.pal, cs.mul_add(0.18 + 0.5 * b + 0.2 * (1.0 - a) + sweep, co));
                let mx = pc[0].max(pc[1]).max(pc[2]).max(0.05);
                let hue = [pc[0] / mx, pc[1] / mx, pc[2] / mx];
                let ground = palette(g.pal, cs.mul_add(0.5 + sweep, co));
                let light = 0.62 * body + 0.95 * rim;
                let white = 0.35 * rim * rim;
                let d = crate::look::unit(crate::look::hash(g.org.0 + x as i32, g.org.1 + y as i32, g.seed as u32));
                let o = &mut row[x * 4..x * 4 + 4];
                for k in 0..3 {
                    let col = 0.07 * ground[k] * (1.0 - body) + (hue[k] + (1.0 - hue[k]) * white) * light;
                    o[k] = (col.min(1.0) * 255.0 + d).clamp(0.0, 255.0) as u8;
                }
                o[3] = 255;
            }
        });
    }
}

impl Generator for Reaction {
    fn sample(&self, _g: &Input, _u: f64, _v: f64) -> [f32; 4] {
        [0.0; 4]
    }

    fn cpu(&self, g: &Input, out: &mut [u8]) {
        let c = Reaction::cfg(g);
        let (u, v) = state(&c, Reaction::steps(g), true);
        Reaction::draw(g, &c, &u, &v, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::tests::input;

    fn pal() -> Vec<[f32; 4]> {
        vec![[0.0, 0.0, 0.05, 1.0], [1.0, 0.1, 0.7, 1.0], [0.0, 0.9, 1.0, 1.0]]
    }

    /// Frame t rendered after earlier frames (cache continuation), out of order, and as a fresh
    /// simulation: identical bytes.
    #[test]
    fn scrub_equals_fresh_simulation() {
        let k = agentfx_ir::generator::kind("reaction").unwrap();
        let pal = pal();
        let mut p = vec![];
        let mut g = input(k, &mut p, &pal, &[("speed", 120.0), ("warmup", 40.0), ("zoom", 4.0)], 80, 48);
        g.seed = 0x5c12b;
        let frame = |g: &Input| {
            let mut o = vec![0u8; 80 * 48 * 4];
            Reaction.cpu(g, &mut o);
            o
        };
        let fresh = |g: &Input| {
            let c = Reaction::cfg(g);
            let (u, v) = state(&c, Reaction::steps(g), false);
            let mut o = vec![0u8; 80 * 48 * 4];
            Reaction::draw(g, &c, &u, &v, &mut o);
            o
        };
        let mut seq = vec![];
        for t in [0.0, 0.25, 0.5, 1.0, 0.75, 1.5, 0.5] {
            g.t = t;
            seq.push((t, frame(&g)));
        }
        for (t, f) in &seq {
            g.t = *t;
            assert_eq!(*f, fresh(&g), "t = {t}");
        }
        assert_ne!(seq[0].1, seq[5].1, "the pattern evolves");
        // the same step (floor(t * speed)) is the same image
        g.t = 1.0 + 0.4 / 120.0;
        assert_eq!(frame(&g), seq[3].1);
    }

    #[test]
    fn bounds_and_opaque() {
        let k = agentfx_ir::generator::kind("reaction").unwrap();
        let pal = pal();
        let mut p = vec![];
        let mut g = input(k, &mut p, &pal, &[("warmup", 300.0), ("zoom", 3.0)], 64, 40);
        g.t = 0.5;
        let mut o = vec![0u8; 64 * 40 * 4];
        Reaction.cpu(&g, &mut o);
        assert!(o.chunks_exact(4).all(|p| p[3] == 255));
        assert!(o.chunks_exact(4).any(|p| p[0].max(p[1]).max(p[2]) > 100), "bright tissue");
        let c = Reaction::cfg(&g);
        let (u, v) = state(&c, 300, false);
        assert!(u.iter().chain(&v).all(|x| (0.0..=1.0).contains(x)));
        // a huge zoom-out stays within the cell budget
        let mut q = g.p.to_vec();
        q[ZOOM] = 0.01;
        g.p = &q;
        let c = Reaction::cfg(&g);
        assert!(c.cols * c.rows <= MAX_CELLS);
    }
}
