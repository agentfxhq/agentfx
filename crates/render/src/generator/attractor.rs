//! Strange attractors as 2D density (parameters: `agentfx_ir::generator::KINDS`, "attractor"):
//! Clifford (x' = sin(a y) + c cos(a x), y' = sin(b x) + d cos(b y)), Peter de Jong
//! (x' = sin(a y) - cos(b x), y' = sin(c x) - cos(d y)) and the Lorenz flow (a b c = sigma rho
//! beta, RK4, d = orbit length) projected from 3D, `turn` spinning it about its vertical axis.
//! `density::CHAINS` seeded orbits plot into a shared histogram; the palette spans the orbit's
//! speeds (step length, Lorenz |velocity|), log-density tone mapping and glow give fine luminous
//! dust. CPU only.
//!
//! The framing comes from the system's analytic bounds (Clifford +-(1 + |c|), +-(1 + |d|);
//! de Jong +-2; Lorenz from rho), never from the orbit, so morphing a b c d does not jump.
use super::density::{lut, total, Hist, Rng, CHAINS};
use super::{Generator, Input};

const SYSTEM: usize = 0;
const A: usize = 1;
const B: usize = 2;
const C: usize = 3;
const D: usize = 4;
const SAMPLES: usize = 5;
const GAMMA: usize = 6;
const RE: usize = 7;
const IM: usize = 8;
const ZOOM: usize = 9;
const TURN: usize = 10;
const CSPEED: usize = 11;
const OFFSET: usize = 12;

/// Lorenz RK4 time step.
const DT: f64 = 0.002;
/// Lorenz orbit time per unit of `d`, over all chains.
const LORENZ_TIME: f64 = 100.0;
/// Restarts a chain may take after diverging before it gives up.
const RESTARTS: u32 = 16;

#[derive(Clone, Copy)]
enum Sys {
    Clifford,
    DeJong,
    Lorenz,
}

#[derive(Clone, Copy)]
struct Orbit {
    sys: Sys,
    a: f64,
    b: f64,
    c: f64,
    d: f64,
}

impl Orbit {
    /// Next point and its speed (step length, or |velocity| for Lorenz).
    #[inline]
    fn step(&self, p: [f64; 3]) -> ([f64; 3], f64) {
        let (a, b, c, d) = (self.a, self.b, self.c, self.d);
        match self.sys {
            Sys::Clifford => {
                let q = [(a * p[1]).sin() + c * (a * p[0]).cos(), (b * p[0]).sin() + d * (b * p[1]).cos(), 0.0];
                (q, (q[0] - p[0]).hypot(q[1] - p[1]))
            }
            Sys::DeJong => {
                let q = [(a * p[1]).sin() - (b * p[0]).cos(), (c * p[0]).sin() - (d * p[1]).cos(), 0.0];
                (q, (q[0] - p[0]).hypot(q[1] - p[1]))
            }
            Sys::Lorenz => {
                let f = |p: [f64; 3]| [a * (p[1] - p[0]), p[0] * (b - p[2]) - p[1], p[0] * p[1] - c * p[2]];
                let add = |p: [f64; 3], k: [f64; 3], s: f64| [p[0] + s * k[0], p[1] + s * k[1], p[2] + s * k[2]];
                let k1 = f(p);
                let dt = DT;
                let k2 = f(add(p, k1, 0.5 * dt));
                let k3 = f(add(p, k2, 0.5 * dt));
                let k4 = f(add(p, k3, dt));
                let q = [0, 1, 2].map(|i| p[i] + dt / 6.0 * (k1[i] + 2.0 * (k2[i] + k3[i]) + k4[i]));
                (q, (k1[0] * k1[0] + k1[1] * k1[1] + k1[2] * k1[2]).sqrt())
            }
        }
    }
    /// A seeded start, run onto the attractor.
    fn start(&self, r: &mut Rng) -> [f64; 3] {
        let (mut p, burn) = match self.sys {
            Sys::Lorenz => ([15.0 * r.sym(), 15.0 * r.sym(), self.b.abs().max(1.0) * (0.5 + 0.5 * r.unit())], 1500),
            _ => ([r.sym(), r.sym(), 0.0], 64),
        };
        for _ in 0..burn {
            p = self.step(p).0;
        }
        p
    }
}

fn ok(p: [f64; 3]) -> bool {
    p[0].abs() < 1e5 && p[1].abs() < 1e5 && p[2].abs() < 1e5 // false for NaN
}

/// The palette spans the orbit's speeds: 3rd to 97th percentile of a seeded pre-run.
fn speed_range(o: &Orbit, seed: u64) -> (f64, f64) {
    let mut r = Rng::new(seed, 0x5bee_d);
    let mut p = o.start(&mut r);
    let mut v = Vec::with_capacity(16_384);
    for _ in 0..16_384 {
        let (q, speed) = o.step(p);
        if !ok(q) {
            break;
        }
        p = q;
        v.push(speed);
    }
    if v.len() < 64 {
        return (0.0, 1.0);
    }
    v.sort_by(f64::total_cmp);
    let (lo, hi) = (v[v.len() * 3 / 100], v[v.len() * 97 / 100]);
    (lo, hi.max(lo + 1e-9))
}

pub struct Attractor;

impl Generator for Attractor {
    fn sample(&self, _g: &Input, _u: f64, _v: f64) -> [f32; 4] {
        [0.0; 4]
    }

    fn cpu(&self, g: &Input, out: &mut [u8]) {
        use rayon::prelude::*;
        let sys = match g.p[SYSTEM].round() as i64 {
            1 => Sys::DeJong,
            2 => Sys::Lorenz,
            _ => Sys::Clifford,
        };
        let n = total(g, g.p[SAMPLES]);
        let o = Orbit { sys, a: g.p[A], b: g.p[B], c: g.p[C], d: g.p[D] };
        // steps per chain, and points plotted per step: the maps plot every iterate; Lorenz
        // integrates a fixed orbit length (d x 100 time units in all, resolution independent)
        // and spreads the samples along it, so more samples draw the same strands smoother
        // instead of filling them into a sheet
        let (steps, sub) = match sys {
            Sys::Lorenz => {
                let steps = ((g.p[D].abs().max(0.05) * LORENZ_TIME / DT) as u64 / CHAINS).max(1);
                (steps, (n / CHAINS / steps).clamp(1, 64))
            }
            _ => (n / CHAINS, 1),
        };
        let (sn, cs) = g.p[TURN].to_radians().sin_cos();
        // system space -> unit space: half-extents (hx, hy) and centre of the analytic bounds,
        // fitted to 0.94 of the rect
        let (hx, hy, cz) = match sys {
            Sys::Clifford => (1.0 + o.c.abs(), 1.0 + o.d.abs(), 0.0),
            Sys::DeJong => (2.0, 2.0, 0.0),
            Sys::Lorenz => {
                let rho = o.b.abs().max(1.0);
                (0.75 * rho, 0.95 * rho, 0.86 * (rho - 1.0).max(0.0))
            }
        };
        let (lo, hi) = speed_range(&o, g.seed);
        let s = 0.94 * (g.aspect / hx).min(1.0 / hy) * g.p[ZOOM];
        let (re, im) = (g.p[RE], g.p[IM]);
        let tbl = lut(g.pal, g.p[CSPEED] as f32, g.p[OFFSET] as f32);
        let hist = Hist::new(g);
        (0..CHAINS).into_par_iter().for_each(|ci| {
            let mut r = Rng::new(g.seed, ci + 1);
            let mut p = o.start(&mut r);
            let mut fails = 0;
            for _ in 0..steps {
                let (q, speed) = o.step(p);
                if !ok(q) {
                    // diverging coefficients: restart a few times, then give the chain up
                    fails += 1;
                    if fails > RESTARTS {
                        break;
                    }
                    p = o.start(&mut r);
                    continue;
                }
                let col = tbl[(((speed - lo) / (hi - lo)).clamp(0.0, 1.0) * 255.0) as usize];
                for k in 1..=sub {
                    // points along the step p -> q (the maps: q itself)
                    let f = k as f64 / sub as f64;
                    let m = [0, 1, 2].map(|i| p[i] + (q[i] - p[i]) * f);
                    // system (x up) -> unit (v down); Lorenz: spin about z, then z is up
                    let (x, y) = match sys {
                        Sys::Lorenz => (cs * m[0] - sn * m[1], m[2] - cz),
                        _ => (cs * m[0] - sn * m[1], sn * m[0] + cs * m[1]),
                    };
                    hist.add(x * s + re, -y * s + im, col);
                }
                p = q;
            }
        });
        hist.tone(g, g.p[GAMMA], 1.0, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::tests::input;

    fn render(over: &[(&str, f64)]) -> Vec<u8> {
        let k = agentfx_ir::generator::kind("attractor").unwrap();
        let pal = vec![[1.0, 0.0, 1.0, 1.0], [0.0, 1.0, 1.0, 1.0]];
        let mut p = vec![];
        let g = input(k, &mut p, &pal, over, 96, 64);
        let mut out = vec![0; 96 * 64 * 4];
        Attractor.cpu(&g, &mut out);
        out
    }

    #[test]
    fn every_system_draws_inside_the_rect() {
        for over in [&[("system", 0.0)][..], &[("system", 1.0), ("a", 1.4), ("b", -2.3), ("c", 2.4), ("d", -2.1)], &[("system", 2.0), ("a", 10.0), ("b", 28.0), ("c", 8.0 / 3.0)]] {
            let out = render(&[over, &[("samples", 6.0)]].concat());
            assert!(out.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "{over:?} premultiplied");
            let lit = out.chunks_exact(4).filter(|p| p[3] > 8).count();
            assert!(lit > 96 * 64 / 30, "{over:?}: only {lit} lit pixels");
            // the analytic bounds hold: at zoom 0.5 the orbit stays inside the middle half, so
            // the outer rows and columns stay dark (glow included)
            let out = render(&[over, &[("samples", 6.0), ("zoom", 0.5)]].concat());
            let a = |x: usize, y: usize| out[(y * 96 + x) * 4 + 3];
            let edge = (0..96).filter(|&x| a(x, 0) > 16 || a(x, 63) > 16).count() + (0..64).filter(|&y| a(0, y) > 16 || a(95, y) > 16).count();
            assert_eq!(edge, 0, "{over:?}: {edge} lit border pixels");
        }
    }

    #[test]
    fn diverging_coefficients_render_empty_not_panic() {
        let out = render(&[("system", 2.0), ("a", -8.0), ("b", 50.0), ("c", -5.0)]);
        assert_eq!(out.len(), 96 * 64 * 4);
    }
}
