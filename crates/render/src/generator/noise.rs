//! Domain-warped fBm (parameters: `agentfx_ir::generator::KINDS`, "noise"): Inigo Quilez's
//! two-level warp `f = fbm(p + w r)`, `r = fbm2(p + w q)`, `q = fbm2(p + t flow)`.
use super::{Generator, Input, Real, fbm, fractal_view, palette};

// parameter indices (declaration order)
const RE: usize = 0;
const IM: usize = 1;
const ZOOM: usize = 2;
const TURN: usize = 3;
const WARP: usize = 4;
const OCT: usize = 5;
const FLOW: usize = 6;
const SPEED: usize = 7;
const OFFSET: usize = 8;

pub struct Noise;

/// (f, r.x, r.y): the warped value and the second-level warp (for colouring).
fn warp<T: Real>(g: &Input, x: T, y: T) -> (T, T, T) {
    let s = g.seed as u32;
    let oct = (g.p[OCT].round() as u32).clamp(1, 8);
    let w = T::of(g.p[WARP]);
    let tt = T::of(g.t) * T::of(g.p[FLOW]);
    let qx = fbm(x + tt, y, oct, s);
    let qy = fbm(x + T::of(5.2), tt.fma(T::of(-0.7), y + T::of(1.3)), oct, s.wrapping_add(1));
    let rx = fbm(w.fma(qx, x + T::of(1.7)), w.fma(qy, y + T::of(9.2)), oct, s.wrapping_add(2));
    let ry = fbm(w.fma(qx, x + T::of(8.3)), w.fma(qy, y + T::of(2.8)), oct, s.wrapping_add(3));
    let f = fbm(w.fma(rx, x), w.fma(ry, y), oct, s.wrapping_add(4));
    (f, rx, ry)
}

/// Colour of a warped value (the tail of `sample` in noise.wgsl): palette by f and r.x, dark
/// valleys, glossy highlights where r.y crosses zero.
fn color(pal: &[[f32; 4]], f: f32, rx: f32, ry: f32, speed: f32, offset: f32) -> [f32; 4] {
    let c = palette(pal, speed.mul_add(0.8f32.mul_add(f, 0.8 * rx), offset));
    let l = 0.7f32.mul_add(f, 0.5).clamp(0.0, 1.0);
    let r = (-2.5f32).mul_add(ry.abs(), 1.0).clamp(0.0, 1.0);
    let k = 0.6f32.mul_add(r * r * r, (0.8 * l).mul_add(l, 0.2));
    [(c[0] * k).min(1.0), (c[1] * k).min(1.0), (c[2] * k).min(1.0), c[3]]
}

impl Generator for Noise {
    fn sample(&self, g: &Input, u: f64, v: f64) -> [f32; 4] {
        let (f, rx, ry) = if g.f32 {
            warp::<f32>(g, u as f32, v as f32)
        } else {
            let (f, q, r) = warp::<f64>(g, u, v);
            (f as f32, q as f32, r as f32)
        };
        color(g.pal, f, rx, ry, g.p[SPEED] as f32, g.p[OFFSET] as f32)
    }
    fn view(&self, p: &[f64]) -> [f64; 6] {
        fractal_view(p[RE], p[IM], p[ZOOM], 0.0, p[TURN], 0.7)
    }
    fn wgsl(&self) -> Option<&'static str> {
        Some(concat!(include_str!("common.wgsl"), include_str!("noise.wgsl")))
    }
}
