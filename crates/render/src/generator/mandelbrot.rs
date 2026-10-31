//! Mandelbrot set (parameters: `agentfx_ir::generator::KINDS`, "mandelbrot"). The CPU runs
//! in f64 (zooms to about 1e12); the f32 GPU kernel resolves zooms to about 2e4.
use super::{Generator, Input, Real, escape, fractal_color, fractal_view};

// parameter indices (declaration order)
const RE: usize = 0;
const IM: usize = 1;
const ZOOM: usize = 2;
const DEPTH: usize = 3;
const TURN: usize = 4;
const ITER: usize = 5;
const BAILOUT: usize = 6;
const SPEED: usize = 7;
const OFFSET: usize = 8;

pub struct Mandelbrot;

/// (u, v): the point of the complex plane (`view`).
fn mu<T: Real>(g: &Input, u: f64, v: f64) -> Option<f32> {
    let p = |i: usize| T::of(g.p[i]);
    escape(T::of(0.0), T::of(0.0), T::of(u), T::of(v), g.p[ITER].round() as u32, p(BAILOUT) * p(BAILOUT))
}

impl Generator for Mandelbrot {
    fn sample(&self, g: &Input, u: f64, v: f64) -> [f32; 4] {
        let m = if g.f32 { mu::<f32>(g, u, v) } else { mu::<f64>(g, u, v) };
        fractal_color(g.pal, m, g.p[SPEED] as f32, g.p[OFFSET] as f32)
    }
    fn view(&self, p: &[f64]) -> [f64; 6] {
        fractal_view(p[RE], p[IM], p[ZOOM], p[DEPTH], p[TURN], 1.25)
    }
    fn wgsl(&self) -> Option<&'static str> {
        Some(concat!(include_str!("common.wgsl"), include_str!("mandelbrot.wgsl")))
    }
}
