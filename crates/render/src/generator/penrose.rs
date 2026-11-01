//! Penrose P3 rhombus tiling (parameters: `agentfx_ir::generator::KINDS`, "penrose"), built by
//! deflating Robinson triangles from a sun of ten: each half-rhombus splits into two or three
//! smaller ones by the golden ratio. Thick and thin rhombi are filled from the palette (rings of
//! colour by distance from the centre) and outlined with glowing edges; `grow` reveals the
//! tiling outward from the centre. CPU only (`cpu` override).
use super::raster::{color, layer, neon, px_per_unit, smooth, unit_to_px};
use super::{Generator, Input, palette};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, PixmapPaint, Stroke, Transform};

// parameter indices (declaration order)
const DEPTH: usize = 0;
const GROW: usize = 1;
const WIDTH: usize = 2;
const RE: usize = 3;
const IM: usize = 4;
const ZOOM: usize = 5;
const TURN: usize = 6;
const SPEED: usize = 7;
const OFFSET: usize = 8;

const PHI: f64 = 1.618_033_988_749_895;

type P = (f64, f64);

/// A Robinson triangle: thin (false) or thick (true) half-rhombus; A is the apex, BC the
/// base shared with its mirror twin.
#[derive(Clone, Copy)]
struct Tri {
    thick: bool,
    a: P,
    b: P,
    c: P,
}

fn lerp(p: P, q: P, t: f64) -> P {
    (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t)
}

fn deflate(ts: &[Tri]) -> Vec<Tri> {
    let mut out = Vec::with_capacity(ts.len() * 3);
    for t in ts {
        if !t.thick {
            let p = lerp(t.a, t.b, 1.0 / PHI);
            out.push(Tri { thick: false, a: t.c, b: p, c: t.b });
            out.push(Tri { thick: true, a: p, b: t.c, c: t.a });
        } else {
            let q = lerp(t.b, t.a, 1.0 / PHI);
            let r = lerp(t.b, t.c, 1.0 / PHI);
            out.push(Tri { thick: true, a: r, b: t.c, c: t.a });
            out.push(Tri { thick: true, a: q, b: r, c: t.b });
            out.push(Tri { thick: false, a: r, b: q, c: t.a });
        }
    }
    out
}

/// The tiling after `n` deflations of a sun of radius `r`.
fn tiling(n: u32, r: f64) -> Vec<Tri> {
    let mut ts: Vec<Tri> = (0..10)
        .map(|i| {
            let ang = |k: f64| (k * std::f64::consts::PI / 10.0).sin_cos();
            let (s0, c0) = ang(2.0 * i as f64 - 1.0);
            let (s1, c1) = ang(2.0 * i as f64 + 1.0);
            let (mut b, mut c) = ((r * c0, r * s0), (r * c1, r * s1));
            if i % 2 == 0 {
                std::mem::swap(&mut b, &mut c);
            }
            Tri { thick: false, a: (0.0, 0.0), b, c }
        })
        .collect();
    for _ in 0..n {
        ts = deflate(&ts);
    }
    ts
}

pub struct Penrose;

impl Generator for Penrose {
    fn sample(&self, _g: &Input, _u: f64, _v: f64) -> [f32; 4] {
        [0.0; 4]
    }
    fn cpu(&self, g: &Input, out: &mut [u8]) {
        let (w, h) = (g.w, g.h);
        let (Some(mut fill), Some(mut lines)) = (layer(w, h), layer(w, h)) else { return };
        let zoom = g.p[ZOOM];
        // the sun covers the rect (corner radius), the view magnifies it
        let reach = (g.aspect * g.aspect + 1.0).sqrt();
        let r = 1.05 * reach;
        let ts = tiling(g.p[DEPTH].round().clamp(0.0, 10.0) as u32, r);
        let (sn, cs) = g.p[TURN].to_radians().sin_cos();
        let map = |q: P| {
            let (x, y) = (q.0 * zoom, q.1 * zoom);
            ((cs * x - sn * y + g.p[RE]) as f32, (sn * x + cs * y + g.p[IM]) as f32)
        };
        let tr = unit_to_px(g.m);
        let width = g.p[WIDTH];
        let grow = g.p[GROW].clamp(0.0, 1.0);
        let (spd, off) = (g.p[SPEED] as f32, g.p[OFFSET] as f32);
        let mut paint = Paint::default();
        paint.anti_alias = true;
        let stroke = Stroke { width: width as f32, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        // fills: colour by kind and distance from the centre (in sun radii), fading in with grow
        const BINS: usize = 64;
        let mut fills: Vec<Option<PathBuilder>> = (0..BINS * 2).map(|_| None).collect();
        let mut edges: Vec<Option<PathBuilder>> = (0..BINS).map(|_| None).collect();
        let mut alpha = vec![0.0f64; BINS];
        for t in &ts {
            let m = ((t.a.0 + t.b.0 + t.c.0) / 3.0, (t.a.1 + t.b.1 + t.c.1) / 3.0);
            let d = (m.0 * m.0 + m.1 * m.1).sqrt() / r;
            let bin = ((d * BINS as f64) as usize).min(BINS - 1);
            alpha[bin] = smooth(grow * 1.15 - 0.15, grow * 1.15, d);
            let (a, b, c) = (map(t.a), map(t.b), map(t.c));
            let pb = fills[bin * 2 + t.thick as usize].get_or_insert_with(PathBuilder::new);
            pb.move_to(a.0, a.1);
            pb.line_to(b.0, b.1);
            pb.line_to(c.0, c.1);
            pb.close();
            // edges CA and AB (BC is shared with the twin half and not an edge of the rhombus)
            let pe = edges[bin].get_or_insert_with(PathBuilder::new);
            pe.move_to(c.0, c.1);
            pe.line_to(a.0, a.1);
            pe.line_to(b.0, b.1);
        }
        for (i, pb) in fills.into_iter().enumerate() {
            let (bin, thick) = (i / 2, i % 2 == 1);
            let a = 1.0 - alpha[bin];
            let Some(path) = pb.and_then(|p| p.finish()) else { continue };
            if a <= 0.0 {
                continue;
            }
            let d = bin as f32 / BINS as f32;
            let c = palette(g.pal, spd.mul_add(1.2 * d + if thick { 0.0 } else { 0.5 }, off));
            let k = if thick { 0.6 } else { 1.0 };
            paint.set_color(color([c[0] * k, c[1] * k, c[2] * k, c[3]], a));
            // aliased: the AA seams between the two halves of a rhombus would show; the
            // glowing edges cover the tile boundaries
            paint.anti_alias = false;
            fill.fill_path(&path, &paint, FillRule::Winding, tr, None);
            paint.anti_alias = true;
        }
        for (bin, pe) in edges.into_iter().enumerate() {
            let a = if width > 0.0 { 1.0 - alpha[bin] } else { 0.0 };
            let Some(path) = pe.and_then(|p| p.finish()) else { continue };
            if a <= 0.0 {
                continue;
            }
            let c = palette(g.pal, spd.mul_add(1.2 * bin as f32 / BINS as f32 + 0.25, off));
            let mx = c[0].max(c[1]).max(c[2]).max(0.02);
            paint.set_color(color([c[0] / mx, c[1] / mx, c[2] / mx, c[3]], a));
            lines.stroke_path(&path, &paint, &stroke, tr, None);
        }
        neon(&lines, out, w, h, width * px_per_unit(g.m), 0.7);
        // fills under the glowing edges
        let Some(mut dst) = tiny_skia::PixmapMut::from_bytes(&mut out[..w * h * 4], w as u32, h as u32) else { return };
        let pp = PixmapPaint { blend_mode: tiny_skia::BlendMode::DestinationOver, ..PixmapPaint::default() };
        dst.draw_pixmap(0, 0, fill.as_ref(), &pp, Transform::identity(), None);
    }
}
