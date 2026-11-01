//! Fractal flames (parameters: `agentfx_ir::generator::KINDS`, "flame"): a seeded iterated
//! function system of 3 to 5 weighted affine maps, each followed by a blend of two classic
//! flame variations (linear, sinusoidal, spherical, swirl, horseshoe, polar, heart), with
//! optional n-fold rotational symmetry. The chaos game runs as `density::CHAINS` seeded chains
//! into a shared histogram; a running colour coordinate (blended towards each map's colour)
//! picks the palette, log-density tone mapping and glow turn the orbit into silk. CPU only.
//!
//! `morph` turns every map's linear part by `morph` whole turns (each map one way or the other),
//! so animating it 0 -> 1 is a seamless loop. The framing (robust percentiles of pre-runs pooled
//! over the whole loop) does not depend on `morph`, so the flame never jumps while it morphs.
use super::density::{lut, total, Hist, Rng, CHAINS};
use super::{Generator, Input};
use std::f64::consts::{PI, TAU};

const SAMPLES: usize = 0;
const GAMMA: usize = 1;
const MORPH: usize = 2;
const SYMMETRY: usize = 3;
const RE: usize = 4;
const IM: usize = 5;
const ZOOM: usize = 6;
const TURN: usize = 7;
const CSPEED: usize = 8;
const OFFSET: usize = 9;

/// Chaos-game iterations a chain discards before it plots.
const BURN: usize = 24;
/// Restarts a chain may take after leaving the plane (NaN / far away) before it gives up.
const RESTARTS: u32 = 16;

#[derive(Clone, Copy)]
struct Xf {
    /// x' = a x + b y + e, y' = c x + d y + f
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
    /// cumulative selection weight (normalised, last = 1)
    cum: f64,
    var: [(u8, f64); 2],
    /// this map's palette coordinate
    col: f64,
}

/// The seeded maps, linear parts turned by `morph` turns.
fn xforms(seed: u64, morph: f64) -> Vec<Xf> {
    // whole turns are exact: morph 1 is morph 0
    let morph = morph.rem_euclid(1.0);
    let mut r = Rng::new(seed, 0xf1a3e);
    let n = 3 + (r.next() % 3) as usize;
    let mut v = Vec::with_capacity(n);
    let mut sum = 0.0;
    for i in 0..n {
        let spin = if r.next() & 1 == 0 { 1.0 } else { -1.0 };
        let angle = r.sym() * PI + morph * TAU * spin;
        let scale = 0.5 + 0.38 * r.unit();
        let squash = 0.55 + 0.45 * r.unit();
        let shear = 0.35 * r.sym();
        let (sn, cs) = angle.sin_cos();
        // rotation * [[scale, shear], [0, scale * squash]]
        let (m00, m01, m11) = (scale, shear * scale, scale * squash);
        let w1 = 0.55 + 0.45 * r.unit();
        let v1 = (r.next() % 7) as u8;
        let v2 = (r.next() % 7) as u8;
        let weight = 0.35 + r.unit();
        sum += weight;
        v.push(Xf {
            a: cs * m00,
            b: cs * m01 - sn * m11,
            c: sn * m00,
            d: sn * m01 + cs * m11,
            e: 0.9 * r.sym(),
            f: 0.9 * r.sym(),
            cum: sum,
            var: [(v1, w1), (v2, 1.0 - w1)],
            col: if n > 1 { i as f64 / (n - 1) as f64 } else { 0.5 },
        });
    }
    for x in &mut v {
        x.cum /= sum;
    }
    v
}

/// One flam3 variation of (x, y).
#[inline]
fn variation(k: u8, x: f64, y: f64) -> (f64, f64) {
    let r2 = x * x + y * y;
    match k {
        0 => (x, y),
        1 => (x.sin(), y.sin()),
        2 => {
            let i = 1.0 / (r2 + 1e-9);
            (x * i, y * i)
        }
        3 => {
            let (sn, cs) = r2.sin_cos();
            (x * sn - y * cs, x * cs + y * sn)
        }
        4 => {
            let i = 1.0 / (r2.sqrt() + 1e-9);
            ((x - y) * (x + y) * i, 2.0 * x * y * i)
        }
        5 => (x.atan2(y) / PI, r2.sqrt() - 1.0),
        _ => {
            let r = r2.sqrt();
            let (sn, cs) = (x.atan2(y) * r).sin_cos();
            (r * sn, -r * cs)
        }
    }
}

/// The IFS with its symmetry: one chain's state and step.
struct Game<'a> {
    xf: &'a [Xf],
    /// n-fold rotations (index 0 = identity)
    rot: Vec<(f64, f64)>,
}

impl Game<'_> {
    fn new(xf: &[Xf], symmetry: usize) -> Game<'_> {
        Game { xf, rot: (0..symmetry.max(1)).map(|j| (TAU * j as f64 / symmetry.max(1) as f64).sin_cos()).collect() }
    }
    /// One chaos-game step of (x, y, colour coordinate).
    #[inline]
    fn step(&self, r: &mut Rng, p: (f64, f64, f64)) -> (f64, f64, f64) {
        let pick = r.unit();
        let t = self.xf.iter().find(|t| pick < t.cum).unwrap_or(&self.xf[self.xf.len() - 1]);
        let (u, v) = (t.a * p.0 + t.b * p.1 + t.e, t.c * p.0 + t.d * p.1 + t.f);
        let (mut x, mut y) = (0.0, 0.0);
        for (k, w) in t.var {
            if w > 0.0 {
                let q = variation(k, u, v);
                x += w * q.0;
                y += w * q.1;
            }
        }
        if self.rot.len() > 1 {
            let (sn, cs) = self.rot[(r.next() % self.rot.len() as u64) as usize];
            (x, y) = (cs * x - sn * y, sn * x + cs * y);
        }
        (x, y, 0.5 * (p.2 + t.col))
    }
    fn start(&self, r: &mut Rng) -> (f64, f64, f64) {
        let mut p = (r.sym(), r.sym(), 0.5);
        for _ in 0..BURN {
            p = self.step(r, p);
        }
        p
    }
}

fn ok(p: (f64, f64, f64)) -> bool {
    p.0.abs() < 1e6 && p.1.abs() < 1e6 // false for NaN
}

/// Framing: robust centre (median) and half-extent (97th percentile) of pre-runs pooled over
/// eight morph phases, so one framing holds for the whole morph loop.
fn frame(seed: u64, symmetry: usize, aspect: f64) -> (f64, f64, f64) {
    const PHASES: usize = 8;
    const PER: usize = 6000;
    let mut xs = Vec::with_capacity(PHASES * PER);
    let mut ys = Vec::with_capacity(PHASES * PER);
    for k in 0..PHASES {
        let xf = xforms(seed, k as f64 / PHASES as f64);
        let game = Game::new(&xf, symmetry);
        let mut r = Rng::new(seed, 0xf4a3e + k as u64);
        let mut p = game.start(&mut r);
        for _ in 0..PER {
            p = game.step(&mut r, p);
            if !ok(p) {
                p = game.start(&mut r);
                continue;
            }
            xs.push(p.0);
            ys.push(p.1);
        }
    }
    if xs.len() < 100 {
        return (0.0, 0.0, 1.0);
    }
    let median = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let (cx, cy) = (median(&xs), median(&ys));
    let mut ext: Vec<f64> = xs.iter().zip(&ys).map(|(x, y)| ((x - cx).abs() / aspect).max((y - cy).abs())).collect();
    ext.sort_by(f64::total_cmp);
    let e = ext[ext.len() * 97 / 100].max(1e-6);
    (cx, cy, e)
}

pub struct Flame;

impl Generator for Flame {
    fn sample(&self, _g: &Input, _u: f64, _v: f64) -> [f32; 4] {
        [0.0; 4]
    }

    fn cpu(&self, g: &Input, out: &mut [u8]) {
        use rayon::prelude::*;
        let symmetry = g.p[SYMMETRY].round().clamp(1.0, 12.0) as usize;
        let xf = xforms(g.seed, g.p[MORPH]);
        let game = Game::new(&xf, symmetry);
        let (cx, cy, ext) = frame(g.seed, symmetry, g.aspect);
        // flame space -> unit space: fit 0.85 of the half-height (room for the morph to wander),
        // then zoom, turn, pan
        let s = 0.85 / ext * g.p[ZOOM];
        let (sn, cs) = g.p[TURN].to_radians().sin_cos();
        let (re, im) = (g.p[RE], g.p[IM]);
        let tbl = lut(g.pal, g.p[CSPEED] as f32, g.p[OFFSET] as f32);
        let hist = Hist::new(g);
        let n = total(g, g.p[SAMPLES]);
        (0..CHAINS).into_par_iter().for_each(|ci| {
            let mut r = Rng::new(g.seed, ci + 1);
            let mut p = game.start(&mut r);
            let mut fails = 0;
            for _ in 0..n / CHAINS {
                p = game.step(&mut r, p);
                if !ok(p) {
                    // a diverging system: restart a few times, then give the chain up
                    fails += 1;
                    if fails > RESTARTS {
                        break;
                    }
                    p = game.start(&mut r);
                    continue;
                }
                let (x, y) = ((p.0 - cx) * s, (p.1 - cy) * s);
                // flame y grows upwards, unit v downwards
                let (u, v) = (cs * x - sn * y + re, -(sn * x + cs * y) + im);
                hist.add(u, v, tbl[(p.2.clamp(0.0, 1.0) * 255.0) as usize]);
            }
        });
        hist.tone(g, g.p[GAMMA], 1.0, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::tests::input;

    fn render(over: &[(&str, f64)], w: usize, h: usize) -> Vec<u8> {
        let k = agentfx_ir::generator::kind("flame").unwrap();
        let pal = vec![[1.0, 0.0, 1.0, 1.0], [0.0, 1.0, 1.0, 1.0]];
        let mut p = vec![];
        let g = input(k, &mut p, &pal, over, w, h);
        let mut out = vec![0; w * h * 4];
        Flame.cpu(&g, &mut out);
        out
    }

    #[test]
    fn flame_has_density_and_premultiplied_bounds() {
        let out = render(&[("samples", 4.0)], 96, 64);
        assert!(out.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "premultiplied");
        let lit = out.chunks_exact(4).filter(|p| p[3] > 8).count();
        assert!(lit > 96 * 64 / 20, "only {lit} lit pixels");
        assert!(out.chunks_exact(4).any(|p| p[3] < 255), "transparent background");
    }

    #[test]
    fn morph_turns_loop_and_symmetry_changes_the_image() {
        let a = render(&[("samples", 2.0)], 64, 48);
        assert_eq!(a, render(&[("samples", 2.0), ("morph", 1.0)], 64, 48), "a whole turn is the same flame");
        assert_ne!(a, render(&[("samples", 2.0), ("morph", 0.25)], 64, 48));
        assert_ne!(a, render(&[("samples", 2.0), ("symmetry", 3.0)], 64, 48));
    }
}
