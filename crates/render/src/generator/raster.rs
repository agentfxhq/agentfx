//! Helpers for the vector / grid kinds that override `Generator::cpu` and rasterize geometry
//! in unit space with tiny-skia (flow, lsystem, penrose, life).
use tiny_skia::{Pixmap, Transform};

/// Inverse of a pixel -> unit mapping `[a, b, c, d, e, f]`: unit -> pixel.
pub fn invert(m: [f64; 6]) -> [f64; 6] {
    let det = m[0] * m[3] - m[1] * m[2];
    let det = if det.abs() < 1e-300 { 1e-300 } else { det };
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    [a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])]
}

/// tiny-skia transform drawing unit-space geometry into the pixel buffer of `g.m`.
pub fn unit_to_px(m: [f64; 6]) -> Transform {
    let i = invert(m);
    Transform::from_row(i[0] as f32, i[1] as f32, i[2] as f32, i[3] as f32, i[4] as f32, i[5] as f32)
}

/// Device pixels per unit-space unit (the mapping's mean scale).
pub fn px_per_unit(m: [f64; 6]) -> f64 {
    1.0 / (m[0] * m[3] - m[1] * m[2]).abs().sqrt().max(1e-300)
}

/// A scratch layer the size of the output.
pub fn layer(w: usize, h: usize) -> Option<Pixmap> {
    Pixmap::new(w as u32, h as u32)
}

/// Glow: add a blurred copy of `src` (three box passes of radius `r` px, about a Gaussian)
/// times `k` onto `dst` (both premultiplied RGBA8, `w x h`), saturating.
pub fn bloom(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: f64, k: f32) {
    let r = r.round().clamp(1.0, 200.0) as usize;
    let mut a: Vec<f32> = src[..w * h * 4].iter().map(|&v| v as f32).collect();
    let mut tmp = vec![0f32; a.len()];
    for _ in 0..3 {
        box_pass(&a, &mut tmp, w, h, r, true);
        box_pass(&tmp, &mut a, w, h, r, false);
    }
    for (d, s) in dst[..w * h * 4].iter_mut().zip(&a) {
        *d = (*d as f32 + s * k).min(255.0) as u8;
    }
}

/// One running-sum box blur of radius r along x (`horiz`) or y, edges clamped to zero.
fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horiz: bool) {
    let (len, lines) = if horiz { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| if horiz { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let inv = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let mut acc = [0f32; 4];
        for i in 0..r.min(len) {
            let o = at(line, i);
            for k in 0..4 {
                acc[k] += src[o + k];
            }
        }
        for i in 0..len {
            if i + r < len {
                let o = at(line, i + r);
                for k in 0..4 {
                    acc[k] += src[o + k];
                }
            }
            let o = at(line, i);
            for k in 0..4 {
                dst[o + k] = acc[k] * inv;
            }
            if i >= r {
                let o = at(line, i - r);
                for k in 0..4 {
                    acc[k] -= src[o + k];
                }
            }
        }
    }
}

/// Polynomial smoothstep.
pub fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A tiny-skia colour from straight RGBA 0..1 times `a`.
pub fn color(c: [f32; 4], a: f64) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba(c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), (c[3] as f64 * a).clamp(0.0, 1.0) as f32).unwrap_or(tiny_skia::Color::TRANSPARENT)
}

/// Write `core` into `out` with a two-radius bloom (line width `wpx` device px): the neon
/// look of the line kinds.
pub fn neon(core: &Pixmap, out: &mut [u8], w: usize, h: usize, wpx: f64, k: f32) {
    let n = w * h * 4;
    out[..n].copy_from_slice(&core.data()[..n]);
    bloom(core.data(), out, w, h, (3.0 * wpx).max(2.0), 1.3 * k);
    bloom(core.data(), out, w, h, (12.0 * wpx).max(6.0), 1.0 * k);
}
