//! Flow field (parameters: `agentfx_ir::generator::KINDS`, "flow"): particles streaking along
//! the streamlines of a divergence-free curl-noise field, drawn as additive glowing trails.
//! CPU only (`cpu` override): every particle's life is a pure function of (seed, index, t), so
//! any frame renders on its own.
use super::raster::{color, layer, neon, px_per_unit, smooth, unit_to_px};
use super::{Generator, Input, fbm, palette};
use tiny_skia::{BlendMode, LineCap, Paint, PathBuilder, Stroke};

// parameter indices (declaration order)
const RE: usize = 0;
const IM: usize = 1;
const ZOOM: usize = 2;
const TURN: usize = 3;
const COUNT: usize = 4;
const TRAIL: usize = 5;
const FLOW: usize = 6;
const SPEED: usize = 7;
const WIDTH: usize = 8;
const CSPEED: usize = 9;
const OFFSET: usize = 10;

/// Integration step, unit space.
const DS: f64 = 0.01;
/// Share of steady drift mixed into the curl direction.
const DRIFT: f64 = 0.85;

pub struct Flow;

fn rnd(i: u32, k: i32, salt: u32, seed: u32) -> f64 {
    crate::look::unit(crate::look::hash(i as i32, k, seed ^ salt.wrapping_mul(0x9e37_79b9))) as f64
}

struct Field {
    m: [f64; 4],
    c: [f64; 2],
    ft: f64,
    seed: u32,
}

impl Field {
    fn new(g: &Input) -> Field {
        let s = 0.6 / g.p[ZOOM];
        let (sn, cs) = g.p[TURN].to_radians().sin_cos();
        Field { m: [s * cs, s * sn, -s * sn, s * cs], c: [g.p[RE], g.p[IM]], ft: g.p[FLOW] * g.t, seed: g.seed as u32 }
    }
    /// Stream function at unit point (u, v).
    fn psi(&self, u: f64, v: f64) -> f64 {
        let x = self.m[0] * u + self.m[2] * v + self.c[0];
        let y = self.m[1] * u + self.m[3] * v + self.c[1];
        fbm(x + self.ft, y - 0.6 * self.ft, 2, self.seed) + 0.3 * fbm(2.3 * y + 4.1 - 0.8 * self.ft, 2.3 * x - 1.7, 2, self.seed.wrapping_add(11))
    }
    /// Unit-length curl direction (rotated gradient of psi).
    fn dir(&self, u: f64, v: f64) -> (f64, f64) {
        let e = 2e-3;
        let du = self.psi(u + e, v) - self.psi(u - e, v);
        let dv = self.psi(u, v + e) - self.psi(u, v - e);
        let n = (du * du + dv * dv).sqrt().max(1e-12);
        // a steady drift along the field's x axis turns closed eddies into streams
        let (vx, vy) = (dv / n + DRIFT * self.m[0] / self.m[0].hypot(self.m[1]), -du / n + DRIFT * self.m[1] / self.m[0].hypot(self.m[1]));
        let n = (vx * vx + vy * vy).sqrt().max(1e-12);
        (vx / n, vy / n)
    }
}

impl Generator for Flow {
    fn sample(&self, _g: &Input, _u: f64, _v: f64) -> [f32; 4] {
        [0.0; 4]
    }
    fn cpu(&self, g: &Input, out: &mut [u8]) {
        use rayon::prelude::*;
        let (w, h) = (g.w, g.h);
        let Some(mut core) = layer(w, h) else { return };
        let tr = unit_to_px(g.m);
        let ppu = px_per_unit(g.m);
        let field = Field::new(g);
        let seed = g.seed as u32;
        let n = g.p[COUNT].round().max(1.0) as u32;
        let trail = g.p[TRAIL];
        let speed = g.p[SPEED];
        let width = g.p[WIDTH];
        let (cs, co) = (g.p[CSPEED] as f32, g.p[OFFSET] as f32);
        let (eu, ev) = (g.aspect + 0.4, 1.4);
        // trails, computed in parallel, drawn in index order (deterministic)
        let trails: Vec<(Vec<(f64, f64)>, [f32; 4], f64)> = (0..n)
            .into_par_iter()
            .map(|i| {
                let life = 1.2 + 2.2 * rnd(i, 0, 1, seed);
                let a = if speed > 0.0 { g.t * speed / life } else { 0.0 } + rnd(i, 0, 2, seed);
                let k = a.floor();
                let f = a - k;
                let k = k as i32;
                let mut p = ((2.0 * rnd(i, k, 3, seed) - 1.0) * eu, (2.0 * rnd(i, k, 4, seed) - 1.0) * ev);
                let head = f * life;
                let tail = (head - trail).max(0.0);
                let nh = (head / DS) as usize;
                let nt = (tail / DS) as usize;
                let c = palette(g.pal, cs.mul_add(0.5 * field.psi(p.0 * 0.5, p.1 * 0.5) as f32 + 0.5, co));
                let mx = c[0].max(c[1]).max(c[2]).max(0.02);
                let c = [c[0] / mx, c[1] / mx, c[2] / mx, c[3]];
                let env = smooth(0.0, 0.12, f) * (1.0 - smooth(0.82, 1.0, f));
                let mut pts = Vec::with_capacity(nh - nt + 1);
                for s in 0..=nh {
                    if s >= nt {
                        pts.push(p);
                    }
                    let (dx, dy) = field.dir(p.0, p.1);
                    // midpoint step
                    let (mx2, my2) = field.dir(p.0 + 0.5 * DS * dx, p.1 + 0.5 * DS * dy);
                    p = (p.0 + DS * mx2, p.1 + DS * my2);
                }
                (pts, c, env)
            })
            .collect();
        let stroke = Stroke { width: width as f32, line_cap: LineCap::Butt, ..Stroke::default() };
        for (pts, c, env) in &trails {
            if pts.len() < 2 || *env <= 0.0 {
                continue;
            }
            // fade from tail to head in short chunks
            let chunks = ((pts.len() - 1) / 4).clamp(1, 24);
            for j in 0..chunks {
                let i0 = j * (pts.len() - 1) / chunks;
                let i1 = (j + 1) * (pts.len() - 1) / chunks;
                let fade = ((j + 1) as f64 / chunks as f64).powf(1.6);
                let mut b = PathBuilder::new();
                b.move_to(pts[i0].0 as f32, pts[i0].1 as f32);
                for q in &pts[i0 + 1..=i1] {
                    b.line_to(q.0 as f32, q.1 as f32);
                }
                let Some(path) = b.finish() else { continue };
                let mut paint = Paint::default();
                paint.set_color(color(*c, 0.9 * env * fade));
                paint.blend_mode = BlendMode::Plus;
                paint.anti_alias = true;
                core.stroke_path(&path, &paint, &stroke, tr, None);
            }
        }
        neon(&core, out, w, h, width * ppu, 1.0);
    }
}
