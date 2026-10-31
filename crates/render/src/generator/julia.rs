//! Julia set z -> z^2 + c (parameters: `agentfx_ir::generator::KINDS`, "julia").
use super::{Generator, Input, Real, escape, fractal_color, fractal_view};

// parameter indices (declaration order)
const CX: usize = 0;
const CY: usize = 1;
const RE: usize = 2;
const IM: usize = 3;
const ZOOM: usize = 4;
const DEPTH: usize = 5;
const TURN: usize = 6;
const ITER: usize = 7;
const BAILOUT: usize = 8;
const SPEED: usize = 9;
const OFFSET: usize = 10;

pub struct Julia;

/// (u, v): the point of the complex plane (`view`).
fn mu<T: Real>(g: &Input, u: f64, v: f64) -> Option<f32> {
    let p = |i: usize| T::of(g.p[i]);
    escape(T::of(u), T::of(v), p(CX), p(CY), g.p[ITER].round() as u32, p(BAILOUT) * p(BAILOUT))
}

impl Generator for Julia {
    fn sample(&self, g: &Input, u: f64, v: f64) -> [f32; 4] {
        let m = if g.f32 { mu::<f32>(g, u, v) } else { mu::<f64>(g, u, v) };
        fractal_color(g.pal, m, g.p[SPEED] as f32, g.p[OFFSET] as f32)
    }
    fn view(&self, p: &[f64]) -> [f64; 6] {
        fractal_view(p[RE], p[IM], p[ZOOM], p[DEPTH], p[TURN], 1.3)
    }
    fn wgsl(&self) -> Option<&'static str> {
        Some(concat!(include_str!("common.wgsl"), include_str!("julia.wgsl")))
    }
}
