//! Power-n Mandelbulb (parameters: `agentfx_ir::generator::KINDS`, "mandelbulb"), raymarched
//! through its distance estimator `0.5 log r r / dr` (White / Nylander spherical power), orbit
//! trap min |z|^2. Camera, march and shading: `ray.rs`.
use super::ray::{self, M, Sdf, Shared, UK, V3, dot3};
use super::{Generator, Input};

// parameter indices (declaration order)
const POWER: usize = 0;
const ITER: usize = 6;
const SHARED: Shared = Shared { yaw: 1, pitch: 2, dist: 3, zoom: 4, steps: 7, halo: 8, fog: 9, occlusion: 10, rim: 11, speed: 12, offset: 13 };
const ZOOM: usize = 4;
const TURN: usize = 5;

pub struct Mandelbulb;

impl Sdf for Mandelbulb {
    /// uniforms UK: power, UK + 1: iterations
    fn de<T: M>(&self, u: &[T], p: V3<T>) -> (T, T, T) {
        let n = u[UK];
        let iter = u[UK + 1].f64() as u32;
        let one = T::of(1.0);
        let mut z = p;
        let mut dr = one;
        let mut r = T::of(0.0);
        let mut trap = T::of(1e10);
        for _ in 0..iter {
            let r2 = dot3(z, z);
            r = r2.root();
            if r > T::of(2.0) {
                break;
            }
            trap = trap.mn(r2);
            let lr = r.lg2();
            let rp = ((n - one) * lr).ex2();
            dr = (rp * n).fma(dr, one);
            let (st, ct) = T::atan2t(z[0].fma(z[0], z[1] * z[1]).root(), z[2]).sincost(n);
            let (sp, cp) = T::atan2t(z[1], z[0]).sincost(n);
            let zr = rp * r;
            let zs = zr * st;
            z = [zs.fma(cp, p[0]), zs.fma(sp, p[1]), zr.fma(ct, p[2])];
        }
        (T::of(0.346_573_6) * r.lg2() * r, dr.rcp(), trap)
    }
}

impl Generator for Mandelbulb {
    fn sample(&self, g: &Input, u: f64, v: f64) -> [f32; 4] {
        ray::sample(self, self.uniforms(g.p), g, u, v)
    }
    fn view(&self, p: &[f64]) -> [f64; 6] {
        ray::view(p[ZOOM], p[TURN])
    }
    fn uniforms(&self, p: &[f64]) -> Vec<f64> {
        // the bulb has radius about 1.2 (low powers grow); the sphere leaves room for the halo
        let mut u = ray::uniforms(p, &SHARED, 1.9 + 0.25 * (4.0 - p[POWER]).max(0.0));
        u.push(p[POWER]);
        u.push(p[ITER].round());
        u
    }
    fn wgsl(&self) -> Option<&'static str> {
        Some(concat!(include_str!("common.wgsl"), include_str!("ray.wgsl"), include_str!("mandelbulb.wgsl")))
    }
    fn cpu(&self, g: &Input, out: &mut [u8]) {
        ray::cpu(self, self.uniforms(g.p), g, out)
    }
}
