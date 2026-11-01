//! L-systems (parameters: `agentfx_ir::generator::KINDS`, "lsystem"): a preset grammar
//! expanded `iter` times and drawn by a turtle as glowing polylines, fitted to the rect.
//! `grow` (0..1) draws it on by path distance from the root, so branching plants grow
//! outwards and curves trace themselves. CPU only (`cpu` override).
use super::raster::{color, layer, neon, px_per_unit, unit_to_px};
use super::{Generator, Input, palette};
use std::collections::BTreeMap;
use tiny_skia::{LineCap, LineJoin, Paint, PathBuilder, Stroke};

// parameter indices (declaration order)
const PRESET: usize = 0;
const ITER: usize = 1;
const GROW: usize = 2;
const WIDTH: usize = 3;
const RE: usize = 4;
const IM: usize = 5;
const ZOOM: usize = 6;
const TURN: usize = 7;
const SPEED: usize = 8;
const OFFSET: usize = 9;

/// Most symbols an expansion may reach (the iteration count stops short of it).
const MAX_LEN: usize = 1 << 21;

pub struct Preset {
    /// `agentfx_ir::generator::LSYSTEM_PRESETS` (test `presets_match_declaration`)
    #[cfg_attr(not(test), allow(dead_code))]
    pub name: &'static str,
    axiom: &'static str,
    rules: &'static [(u8, &'static str)],
    /// symbols that draw a step
    draw: &'static [u8],
    angle: f64,
    /// initial heading, degrees (0 = right, -90 = up)
    heading: f64,
    pub iter: u32,
}

/// The presets, in `preset=` order.
pub const PRESETS: &[Preset] = &[
    Preset { name: "koch", axiom: "F--F--F", rules: &[(b'F', "F+F--F+F")], draw: b"F", angle: 60.0, heading: 0.0, iter: 4 },
    Preset { name: "dragon", axiom: "FX", rules: &[(b'X', "X+YF+"), (b'Y', "-FX-Y")], draw: b"F", angle: 90.0, heading: 0.0, iter: 11 },
    Preset { name: "sierpinski", axiom: "A", rules: &[(b'A', "B-A-B"), (b'B', "A+B+A")], draw: b"AB", angle: 60.0, heading: 0.0, iter: 7 },
    Preset { name: "plant", axiom: "X", rules: &[(b'X', "F+[[X]-X]-F[-FX]+X"), (b'F', "FF")], draw: b"F", angle: 25.0, heading: -90.0, iter: 6 },
    Preset { name: "hilbert", axiom: "A", rules: &[(b'A', "+BF-AFA-FB+"), (b'B', "-AF+BFB-FA-")], draw: b"F", angle: 90.0, heading: 0.0, iter: 6 },
    Preset { name: "gosper", axiom: "A", rules: &[(b'A', "A-B--B+A++AA+B-"), (b'B', "+A-BB--B-A++A+B")], draw: b"AB", angle: 60.0, heading: 0.0, iter: 4 },
];

#[test]
fn presets_match_declaration() {
    let names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
    assert_eq!(names, agentfx_ir::generator::LSYSTEM_PRESETS);
}

fn expand(p: &Preset, n: u32) -> Vec<u8> {
    let mut s = p.axiom.as_bytes().to_vec();
    for _ in 0..n {
        let mut next = Vec::with_capacity(s.len() * 3);
        for &c in &s {
            match p.rules.iter().find(|r| r.0 == c) {
                Some(r) => next.extend_from_slice(r.1.as_bytes()),
                None => next.push(c),
            }
        }
        if next.len() > MAX_LEN {
            break;
        }
        s = next;
    }
    s
}

/// One drawn step: from, to, path distance from the root at its start, branch depth.
struct Seg {
    a: (f64, f64),
    b: (f64, f64),
    d: f64,
    depth: usize,
}

fn turtle(p: &Preset, s: &[u8]) -> Vec<Seg> {
    let da = p.angle.to_radians();
    let (mut x, mut y, mut h, mut d) = (0.0f64, 0.0f64, p.heading.to_radians(), 0.0f64);
    let mut stack = vec![];
    let mut out = vec![];
    for &c in s {
        match c {
            b'+' => h += da,
            b'-' => h -= da,
            b'[' => stack.push((x, y, h, d)),
            b']' => {
                if let Some(t) = stack.pop() {
                    (x, y, h, d) = t;
                }
            }
            c if p.draw.contains(&c) => {
                let (nx, ny) = (x + h.cos(), y + h.sin());
                out.push(Seg { a: (x, y), b: (nx, ny), d, depth: stack.len() });
                (x, y, d) = (nx, ny, d + 1.0);
            }
            _ => {}
        }
    }
    out
}

pub struct LSystem;

impl Generator for LSystem {
    fn sample(&self, _g: &Input, _u: f64, _v: f64) -> [f32; 4] {
        [0.0; 4]
    }
    fn cpu(&self, g: &Input, out: &mut [u8]) {
        let (w, h) = (g.w, g.h);
        let Some(mut core) = layer(w, h) else { return };
        let pr = &PRESETS[(g.p[PRESET].round().max(0.0) as usize).min(PRESETS.len() - 1)];
        let n = if g.p[ITER] >= 0.5 { g.p[ITER].round() as u32 } else { pr.iter };
        let segs = turtle(pr, &expand(pr, n));
        if segs.is_empty() {
            out[..w * h * 4].fill(0);
            return;
        }
        // turn the drawing, fit it into 92 % of the rect (so it stays in frame at any turn),
        // then zoom and offset
        let (sn, cs) = g.p[TURN].to_radians().sin_cos();
        let rot = |q: (f64, f64)| (cs * q.0 - sn * q.1, sn * q.0 + cs * q.1);
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for s in &segs {
            for q in [rot(s.a), rot(s.b)] {
                x0 = x0.min(q.0);
                y0 = y0.min(q.1);
                x1 = x1.max(q.0);
                y1 = y1.max(q.1);
            }
        }
        let fit = 0.92 * (2.0 * g.aspect / (x1 - x0).max(1e-9)).min(2.0 / (y1 - y0).max(1e-9)) * g.p[ZOOM];
        let (mx, my) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
        let map = |q: (f64, f64)| {
            let q = rot(q);
            (((q.0 - mx) * fit + g.p[RE]) as f32, ((q.1 - my) * fit + g.p[IM]) as f32)
        };
        let maxd = segs.iter().map(|s| s.d + 1.0).fold(0.0, f64::max);
        let vis = g.p[GROW].clamp(0.0, 1.0) * maxd;
        // one path per (colour bin, branch depth)
        const BINS: usize = 48;
        let mut paths: BTreeMap<(usize, usize), PathBuilder> = BTreeMap::new();
        for s in &segs {
            if s.d >= vis {
                continue;
            }
            let f = ((vis - s.d).min(1.0)) as f32;
            let a = map(s.a);
            let b = map(s.b);
            let b = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
            let bin = ((s.d / maxd) * BINS as f64) as usize;
            let pb = paths.entry((bin.min(BINS - 1), s.depth.min(8))).or_default();
            pb.move_to(a.0, a.1);
            pb.line_to(b.0, b.1);
        }
        let tr = unit_to_px(g.m);
        let width = g.p[WIDTH];
        let (cs_, co) = (g.p[SPEED] as f32, g.p[OFFSET] as f32);
        for ((bin, depth), pb) in paths {
            let Some(path) = pb.finish() else { continue };
            let c = palette(g.pal, cs_.mul_add(bin as f32 / BINS as f32, co));
            let mx = c[0].max(c[1]).max(c[2]).max(0.02);
            let c = [c[0] / mx, c[1] / mx, c[2] / mx, c[3]];
            let mut paint = Paint::default();
            paint.set_color(color(c, 1.0));
            paint.anti_alias = true;
            let stroke = Stroke { width: (width * 0.8f64.powi(depth as i32)) as f32, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
            core.stroke_path(&path, &paint, &stroke, tr, None);
        }
        neon(&core, out, w, h, width * px_per_unit(g.m), 0.8);
    }
}
