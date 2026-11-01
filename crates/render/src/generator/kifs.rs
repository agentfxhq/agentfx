//! Kaleidoscopic IFS (parameters: `agentfx_ir::generator::KINDS`, "kifs"), Knighty's fold and
//! scale: per iteration rotate (twist), fold (Menger box folds, Sierpinski tetrahedral or
//! octahedral reflections), rotate (tilt), scale by `fold-scale` towards the offset c. The
//! distance is the final point's box (menger) or radial distance over the accumulated scale.
//! Rotation matrices are built on the host. Camera, march and shading: `ray.rs`.
use super::ray::{self, M, Sdf, Shared, UK, V3, dot3, mul3};
use super::{Generator, Input};

// parameter indices (declaration order)
const MODE: usize = 0;
const SCALE: usize = 1;
const TWIST: usize = 2;
const TILT: usize = 3;
const CX: usize = 4;
const ZOOM: usize = 10;
const TURN: usize = 11;
const ITER: usize = 12;
const SHARED: Shared = Shared { yaw: 7, pitch: 8, dist: 9, zoom: 10, steps: 13, halo: 14, fog: 15, occlusion: 16, rim: 17, speed: 18, offset: 19 };

// uniforms from UK: 0-8 twist matrix, 9-17 tilt matrix, 18 scale, 19-21 c (scale - 1),
// 22 iterations, 23 mode
const R1: usize = UK;
const R2: usize = UK + 9;
const S: usize = UK + 18;
const K: usize = UK + 19;
const N: usize = UK + 22;
const MD: usize = UK + 23;

pub struct Kifs;

/// (x, y) -> (-y, -x) when x + y < 0: the reflection in the plane x = -y.
fn flip<T: M>(a: T, b: T) -> (T, T) {
    if a + b < T::of(0.0) { (-b, -a) } else { (a, b) }
}

/// (x, y) -> (y, x) when x < y: the reflection in the plane x = y.
fn sort<T: M>(a: T, b: T) -> (T, T) {
    if a < b { (b, a) } else { (a, b) }
}

impl Sdf for Kifs {
    fn de<T: M>(&self, u: &[T], p: V3<T>) -> (T, T, T) {
        let (sc, k) = (u[S], [u[K], u[K + 1], u[K + 2]]);
        let iter = u[N].f64() as u32;
        let mode = u[MD].f64() as u32;
        let mut z = p;
        let mut s = T::of(1.0);
        let mut trap = T::of(1e10);
        for i in 0..iter {
            z = mul3(&u[R1..R1 + 9], z);
            let [mut x, mut y, mut w] = z;
            if mode == 1 {
                (x, y) = flip(x, y);
                (x, w) = flip(x, w);
                (y, w) = flip(y, w);
            } else if mode == 2 {
                (x, y) = flip(x, y);
                (x, w) = flip(x, w);
                (x, y) = sort(x, y);
                (x, w) = sort(x, w);
            } else {
                (x, y, w) = (x.abs(), y.abs(), w.abs());
                (x, y) = sort(x, y);
                (x, w) = sort(x, w);
                (y, w) = sort(y, w);
            }
            z = mul3(&u[R2..R2 + 9], [x, y, w]);
            if mode == 0 {
                // Menger: the z copies only where they lie above the middle layer
                let zz = sc * z[2];
                z = [sc.fma(z[0], -k[0]), sc.fma(z[1], -k[1]), if zz > T::of(0.5) * k[2] { sc.fma(z[2], -k[2]) } else { zz }];
            } else {
                z = [0, 1, 2].map(|j| sc.fma(z[j], -k[j]));
            }
            s = s * sc;
            let r2 = dot3(z, z);
            // orbit trap over the first three folds only: colour by the coarse cells (the deep
            // levels change from pixel to pixel)
            if i < 3 {
                trap = trap.mn(r2);
            }
            if r2 > T::of(1e4) {
                break;
            }
        }
        let r2 = dot3(z, z);
        let d = if mode == 0 { z[0].abs().mx(z[1].abs()).mx(z[2].abs()) - T::of(1.0) } else { r2.fma(r2.rsqrt(), T::of(-1.0)) };
        (d, s.rcp(), trap * T::of(0.15))
    }
}

/// Row-major rotation by `deg` about axis 1 (y, vertical) or 0 (x).
fn rot(axis: usize, deg: f64) -> [f64; 9] {
    let (s, c) = deg.to_radians().sin_cos();
    if axis == 1 { [c, 0.0, s, 0.0, 1.0, 0.0, -s, 0.0, c] } else { [1.0, 0.0, 0.0, 0.0, c, -s, 0.0, s, c] }
}

impl Generator for Kifs {
    fn sample(&self, g: &Input, u: f64, v: f64) -> [f32; 4] {
        ray::sample(self, self.uniforms(g.p), g, u, v)
    }
    fn view(&self, p: &[f64]) -> [f64; 6] {
        ray::view(p[ZOOM], p[TURN])
    }
    fn uniforms(&self, p: &[f64]) -> Vec<f64> {
        let c = [p[CX], p[CX + 1], p[CX + 2]];
        let cl = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
        let mut u = ray::uniforms(p, &SHARED, 1.6 * cl.max(0.5) + 0.2);
        u.extend(rot(1, p[TWIST]));
        u.extend(rot(0, p[TILT]));
        u.push(p[SCALE]);
        u.extend(c.map(|v| v * (p[SCALE] - 1.0)));
        u.push(p[ITER].round());
        u.push(p[MODE].round());
        u
    }
    fn wgsl(&self) -> Option<&'static str> {
        Some(concat!(include_str!("common.wgsl"), include_str!("ray.wgsl"), include_str!("kifs.wgsl")))
    }
    fn cpu(&self, g: &Input, out: &mut [u8]) {
        ray::cpu(self, self.uniforms(g.p), g, out)
    }
}
