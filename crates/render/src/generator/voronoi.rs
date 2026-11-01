//! Voronoi cells (parameters: `agentfx_ir::generator::KINDS`, "voronoi"): one seed per lattice
//! cell drifting on a smooth triangle-wave path (no trig, so the f32 CPU matches the kernel),
//! exact distance to the cell border (Inigo Quilez's two-pass search), palette by cell.
use super::truchet::{glow, smooth};
use super::{Generator, Input, Real, fractal_view, palette};

// parameter indices (declaration order)
const RE: usize = 0;
const IM: usize = 1;
const ZOOM: usize = 2;
const TURN: usize = 3;
const JITTER: usize = 4;
const FLOW: usize = 5;
const MODE: usize = 6;
const WIDTH: usize = 7;
const SPEED: usize = 8;
const OFFSET: usize = 9;

pub struct Voronoi;

/// Smoothed triangle wave 0..1..0 of period 1.
fn wave<T: Real>(x: T) -> T {
    let t = (T::of(2.0).fma(x.fract(), T::of(-1.0))).abs();
    t * t * T::of(-2.0).fma(t, T::of(3.0))
}

/// The seed of lattice cell (cx, cy) at time tt, in cell coordinates (0..1).
fn seed_pt<T: Real>(cx: i32, cy: i32, s: u32, jit: T, tt: T) -> (T, T) {
    let a = T::of(crate::look::unit(crate::look::hash(cx, cy, s)) as f64);
    let b = T::of(crate::look::unit(crate::look::hash(cx, cy, s.wrapping_add(1))) as f64);
    let r = T::of(0.6).fma(a, T::of(0.7));
    let px = wave(r.fma(tt, a));
    let py = wave(T::of(1.3).fma(r * tt, b));
    (jit.fma(px - T::of(0.5), T::of(0.5)), jit.fma(py - T::of(0.5), T::of(0.5)))
}

pub struct Cell<T> {
    /// distance to the nearest border
    pub border: T,
    /// distance to the nearest seed
    pub f1: T,
    /// distance to the second nearest seed (3x3 neighbourhood)
    pub f2: T,
    pub id: (i32, i32),
}

fn cells<T: Real>(g: &Input, x: T, y: T) -> Cell<T> {
    let s = g.seed as u32;
    let jit = T::of(g.p[JITTER]);
    let tt = T::of(g.t) * T::of(g.p[FLOW]);
    let (nx, ny) = (x.cell(), y.cell());
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let (mut md, mut m2) = (T::of(8.0), T::of(8.0));
    let (mut mr, mut mg) = ((T::of(0.0), T::of(0.0)), (0, 0));
    for j in -1..=1 {
        for i in -1..=1 {
            let (ox, oy) = seed_pt(nx + i, ny + j, s, jit, tt);
            let rx = T::of(i as f64) + ox - fx;
            let ry = T::of(j as f64) + oy - fy;
            let d = rx.fma(rx, ry * ry);
            if d < md {
                m2 = md;
                md = d;
                mr = (rx, ry);
                mg = (i, j);
            } else if d < m2 {
                m2 = d;
            }
        }
    }
    let mut bd = T::of(8.0);
    for j in -2..=2 {
        for i in -2..=2 {
            let (gx, gy) = (mg.0 + i, mg.1 + j);
            let (ox, oy) = seed_pt(nx + gx, ny + gy, s, jit, tt);
            let rx = T::of(gx as f64) + ox - fx;
            let ry = T::of(gy as f64) + oy - fy;
            let (ex, ey) = (rx - mr.0, ry - mr.1);
            let e2 = ex.fma(ex, ey * ey);
            if e2 > T::of(1e-5) {
                let l = e2.sqrt();
                let d = (T::of(0.5) * (mr.0 + rx)).fma(ex / l, T::of(0.5) * (mr.1 + ry) * (ey / l));
                if d < bd {
                    bd = d;
                }
            }
        }
    }
    Cell { border: bd, f1: md.sqrt(), f2: m2.sqrt(), id: (nx + mg.0, ny + mg.1) }
}

/// Colour of a cell sample (`vo_color` in voronoi.wgsl).
fn color(g: &Input, c: &Cell<f32>, x: f32, y: f32) -> [f32; 4] {
    let s = g.seed as u32;
    let (speed, offset, w) = (g.p[SPEED] as f32, g.p[OFFSET] as f32, g.p[WIDTH] as f32);
    let hid = crate::look::unit(crate::look::hash(c.id.0, c.id.1, s.wrapping_add(2)));
    let mode = g.p[MODE].round() as i32;
    let edge = smooth(w, 0.5 * w, c.border);
    let rgb: [f32; 3] = match mode {
        // glowing edges on black, colour sweeping across the rect
        1 => {
            let hw = 4.0f32.mul_add(w, 1e-4);
            let q = hw / (c.border + hw);
            let i = 0.6f32.mul_add(q * q, glow(c.border, w));
            let p = palette(g.pal, speed.mul_add(0.04f32.mul_add(x, 0.025 * y), offset));
            let f = palette(g.pal, speed.mul_add(hid, offset));
            let hot = 0.3 * (i - 0.8).max(0.0);
            [0, 1, 2].map(|k| p[k].mul_add(i, 0.12f32.mul_add(f[k], hot)).min(1.0))
        }
        // crystal facets: F2 - F1 ridges
        2 => {
            let r = c.f2 - c.f1;
            let p = palette(g.pal, speed.mul_add(0.6f32.mul_add(r, 0.5 * hid), offset));
            let l = 0.35f32.mul_add(r.min(1.0), (-0.65f32).mul_add(edge, 0.65));
            let l = 0.25f32.mul_add(edge, l);
            [0, 1, 2].map(|k| (p[k] * l).min(1.0))
        }
        // shaded cells with dark seams
        _ => {
            let p = palette(g.pal, speed.mul_add(hid, offset));
            let l = (-0.9f32).mul_add(c.f1.min(1.0), 1.15) * (-0.85f32).mul_add(edge, 1.0);
            [0, 1, 2].map(|k| (p[k] * l).min(1.0))
        }
    };
    [rgb[0], rgb[1], rgb[2], 1.0]
}

impl Generator for Voronoi {
    fn sample(&self, g: &Input, u: f64, v: f64) -> [f32; 4] {
        let c = if g.f32 {
            cells::<f32>(g, u as f32, v as f32)
        } else {
            let c = cells::<f64>(g, u, v);
            Cell { border: c.border as f32, f1: c.f1 as f32, f2: c.f2 as f32, id: c.id }
        };
        color(g, &c, u as f32, v as f32)
    }
    fn view(&self, p: &[f64]) -> [f64; 6] {
        fractal_view(p[RE], p[IM], p[ZOOM], 0.0, p[TURN], 4.0)
    }
    fn wgsl(&self) -> Option<&'static str> {
        Some(concat!(include_str!("common.wgsl"), include_str!("voronoi.wgsl")))
    }
}
