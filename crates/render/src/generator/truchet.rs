//! Truchet tiles (parameters: `agentfx_ir::generator::KINDS`, "truchet"): quarter-circle arcs
//! about two opposite tile corners, glowing lines on a dark ground; `phase` turns the tiles.
use super::{Generator, Input, Real, fractal_view, palette};

// parameter indices (declaration order)
const RE: usize = 0;
const IM: usize = 1;
const ZOOM: usize = 2;
const TURN: usize = 3;
const WIDTH: usize = 4;
const PHASE: usize = 5;
const MODE: usize = 6;
const SPEED: usize = 7;
const OFFSET: usize = 8;

pub struct Truchet;

/// `t * t * (3 - 2 t)` of clamp((x - e0) / (e1 - e0)), with the kernel's fma.
pub fn smooth<T: Real>(e0: T, e1: T, x: T) -> T {
    let t = ((x - e0) / (e1 - e0)).clamp01();
    t * t * T::of(-2.0).fma(t, T::of(3.0))
}

/// Line intensity at distance d from a curve of width w: solid core + soft halo.
pub fn glow<T: Real>(d: T, w: T) -> T {
    let hw = T::of(0.6).fma(w, T::of(1e-4));
    let k = hw / (d + hw);
    T::of(0.55).fma(k * k, smooth(w, T::of(0.5) * w, d))
}

/// Summed line intensity at tile-space (x, y).
fn lines<T: Real>(g: &Input, x: T, y: T) -> T {
    let (cx, cy) = (x.cell(), y.cell());
    let s = g.seed as u32;
    let h = crate::look::hash(cx, cy, s);
    let delay = T::of(crate::look::unit(crate::look::hash(cx, cy, s.wrapping_add(1))) as f64) * T::of(0.6);
    let q = T::of(g.p[PHASE]) - delay;
    let turns = q.floor();
    let fr = ((q - turns) / T::of(0.4)).clamp01();
    let e = fr * fr * T::of(-2.0).fma(fr, T::of(3.0));
    let flip = (h ^ (turns.cell() as u32)) & 1;
    // rotation by a smooth angle 0..90 deg, rational (no trig): s = tan(angle / 4)
    let sv = e * T::of(0.414_213_56);
    let s2 = sv * sv;
    let den = T::of(1.0) + s2;
    let (c, sn) = ((T::of(1.0) - s2) / den, (sv + sv) / den);
    let (ax, ay) = (x - x.floor() - T::of(0.5), y - y.floor() - T::of(0.5));
    let rx = c.fma(ax, sn * ay);
    let ry = c.fma(ay, -(sn * ax));
    let k = if flip == 0 { T::of(0.5) } else { T::of(-0.5) };
    let w = T::of(g.p[WIDTH]);
    // distance to the nearer of the two corners (equal on the diagonal between them, so
    // continuous): each half of the tile draws the arcs about its own corner
    let len = |px: T, py: T| {
        let (dx, dy) = (rx - px, ry - py);
        dx.fma(dx, dy * dy).sqrt()
    };
    let (l1, l2) = (len(-k, T::of(-0.5)), len(k, T::of(0.5)));
    let l = if l1 < l2 { l1 } else { l2 };
    if g.p[MODE].round() as i32 >= 1 {
        glow((l - T::of(1.0 / 3.0)).abs(), w) + glow((l - T::of(2.0 / 3.0)).abs(), w)
    } else {
        glow((l - T::of(0.5)).abs(), w)
    }
}

impl Generator for Truchet {
    fn sample(&self, g: &Input, u: f64, v: f64) -> [f32; 4] {
        let i = if g.f32 { lines::<f32>(g, u as f32, v as f32) } else { lines::<f64>(g, u, v) as f32 };
        let (u, v) = (u as f32, v as f32);
        let pos = (g.p[SPEED] as f32).mul_add(0.045f32.mul_add(u, 0.03 * v), g.p[OFFSET] as f32);
        let c = palette(g.pal, pos);
        let b = palette(g.pal, pos + 0.5);
        let hot = 0.3 * (i - 0.8).max(0.0);
        let ch = |k: usize| c[k].mul_add(i, 0.07f32.mul_add(b[k], hot)).min(1.0);
        [ch(0), ch(1), ch(2), 1.0]
    }
    fn view(&self, p: &[f64]) -> [f64; 6] {
        fractal_view(p[RE], p[IM], p[ZOOM], 0.0, p[TURN], 4.0)
    }
    fn wgsl(&self) -> Option<&'static str> {
        Some(concat!(include_str!("common.wgsl"), include_str!("truchet.wgsl")))
    }
}
