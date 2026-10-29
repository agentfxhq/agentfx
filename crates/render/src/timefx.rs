//! Time effects (X3): a node's subtree evaluated at other times than the frame's.
//! Frame n is a pure function of n, so any subtree can be rendered at any time t.
//!
//! - slit-scan: screen row/column p shows the subtree at `t - span * u(p)` (bands of `band`
//!   px, cross-faded between neighbouring time samples);
//! - echo: trailing copies at `t - k * span`, decaying opacity, optional tint and blend;
//! - time displacement: per-pixel `u` from a map (radial, linear, noise, a layer's luma),
//!   quantized to `levels` time samples and cross-faded.
//!
//! This module holds the pure parts (sample times, maps, pixel compositing); the painter
//! side lives in `timefx_paint.rs` (a child of `vraster`). The schedule hashes the subtree at
//! `sig_times` (damage) and sweeps the ink over them.
use agentfx_ir::{BlendMode, Scene, TimeFx, TimeMap};

/// `span` at time t (track `time.span`, else the IR value).
pub fn span_at(scene: &Scene, tracks: &[usize], tf: &TimeFx, t: f64) -> f64 {
    crate::schedule::number_at(scene, tracks, "time.span", t, tf.span())
}

/// Spans below this are "off" (the plain subtree).
pub const SPAN_EPS: f64 = 1e-6;

/// Number of time samples (levels) of a slit-scan / displace effect; echo: copies + 1.
pub fn levels(scene: &Scene, tf: &TimeFx) -> usize {
    match tf {
        TimeFx::Slitscan { axis, range, band, .. } => {
            let (a, b) = slit_range(scene, axis, range);
            ((b - a).abs() / band.max(1.0)).ceil().max(1.0) as usize + 1
        }
        TimeFx::Echo { count, .. } => *count as usize + 1,
        TimeFx::Displace { levels, .. } => (*levels).max(2) as usize,
    }
}

pub fn slit_range(scene: &Scene, axis: &str, range: &Option<[f64; 2]>) -> (f64, f64) {
    match range {
        // b < a: the delay grows towards a (reversed)
        Some(r) => (r[0], if (r[1] - r[0]).abs() < 1.0 { r[0] + 1.0 } else { r[1] }),
        None => (0.0, if axis == "x" { scene.width as f64 } else { scene.height as f64 }),
    }
}

/// Time of level `l` (0 = the frame time).
pub fn level_time(tf: &TimeFx, t: f64, span: f64, l: usize, nlev: usize) -> f64 {
    match tf {
        TimeFx::Echo { .. } => t - span * l as f64,
        _ => t - span * l as f64 / (nlev - 1).max(1) as f64,
    }
}

/// Times at which the schedule hashes the subtree (besides the frame time itself): every
/// echo copy, every displace level, and for slit-scans at most two samples per frame
/// interval of the window (motions shorter than half a frame inside the window may be
/// missed by damage; the raster samples every band).
pub fn sig_times(scene: &Scene, tracks: &[usize], tf: &TimeFx, t: f64) -> Vec<f64> {
    let span = span_at(scene, tracks, tf, t);
    if span.abs() < SPAN_EPS {
        return vec![];
    }
    let nlev = levels(scene, tf);
    match tf {
        TimeFx::Slitscan { .. } => {
            let m = nlev.min((span.abs() * scene.fps as f64 * 2.0).ceil() as usize + 2).max(2);
            (1..m).map(|j| t - span * j as f64 / (m - 1) as f64).collect()
        }
        _ => (1..nlev).map(|l| level_time(tf, t, span, l, nlev)).collect(),
    }
}

/// Frame whose content selection (switch/sequence) applies at time `ts`.
pub fn frame_of(ts: f64, fps: f64, frames: usize) -> usize {
    ((ts * fps + 1e-6).floor().max(0.0) as usize).min(frames.saturating_sub(1))
}

/// Map value u in 0..1 at screen pixel centre (x, y) (radial, linear, noise; layer maps are
/// rendered by the painter).
pub fn map_u(map: &TimeMap, x: f64, y: f64) -> f32 {
    match map {
        TimeMap::Radial { center, radius } => ((x - center[0]).hypot(y - center[1]) / radius).clamp(0.0, 1.0) as f32,
        TimeMap::Linear { from, to } => {
            let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
            (((x - from[0]) * dx + (y - from[1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0) as f32
        }
        TimeMap::Noise { scale, seed, octaves } => {
            let (mut sum, mut amp, mut norm, mut fr) = (0.0, 1.0, 0.0, 1.0 / scale);
            for o in 0..*octaves {
                sum += amp * value_noise(x * fr, y * fr, seed.wrapping_add(o as u64 * 0x9E37_79B9));
                norm += amp;
                amp *= 0.5;
                fr *= 2.0;
            }
            // fbm of value noise concentrates around 0.5: stretch to use the whole span
            (((sum / norm) - 0.5) * 1.8 + 0.5).clamp(0.0, 1.0) as f32
        }
        TimeMap::Layer { .. } => 0.0,
    }
}

fn hash2(ix: i64, iy: i64, seed: u64) -> f64 {
    // SplitMix64 of the lattice point
    let mut z = (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

fn value_noise(x: f64, y: f64, seed: u64) -> f64 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let s = |v: f64| v * v * (3.0 - 2.0 * v);
    let (sx, sy) = (s(fx), s(fy));
    let (ix, iy) = (x0 as i64, y0 as i64);
    let a = hash2(ix, iy, seed);
    let b = hash2(ix + 1, iy, seed);
    let c = hash2(ix, iy + 1, seed);
    let d = hash2(ix + 1, iy + 1, seed);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

/// Recolour a premultiplied buffer: rgb = tint.rgb * alpha, everything scaled by `k`
/// (`tint` None: only the scale).
pub fn tint_scale(buf: &mut [u8], tint: Option<[f64; 4]>, k: f64) {
    match tint {
        Some(c) => {
            let ka = k * c[3];
            let m = [c[0] * ka, c[1] * ka, c[2] * ka, ka];
            for p in buf.chunks_exact_mut(4) {
                let a = p[3] as f64;
                for j in 0..4 {
                    p[j] = (a * m[j] + 0.5).min(255.0) as u8;
                }
            }
        }
        None => {
            if k >= 1.0 {
                return;
            }
            let k = (k * 65536.0) as u32;
            for v in buf.iter_mut() {
                *v = ((*v as u32 * k + 32768) >> 16) as u8;
            }
        }
    }
}

/// Composite premultiplied `src` (sw x sh) onto `dst` (dw wide) at (ox, oy), with `mode`
/// (normal, plus, multiply, screen; others composite normally).
#[allow(clippy::too_many_arguments)]
pub fn blend_into(dst: &mut [u8], dw: usize, dh: usize, src: &[u8], sw: usize, sh: usize, ox: i64, oy: i64, mode: BlendMode) {
    let x0 = ox.max(0) as usize;
    let y0 = oy.max(0) as usize;
    let x1 = ((ox + sw as i64).min(dw as i64)).max(0) as usize;
    let y1 = ((oy + sh as i64).min(dh as i64)).max(0) as usize;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let div = |v: u32| ((v + 128 + ((v + 128) >> 8)) >> 8).min(255);
    for y in y0..y1 {
        let sy = (y as i64 - oy) as usize;
        let srow = &src[(sy * sw + (x0 as i64 - ox) as usize) * 4..(sy * sw + (x1 as i64 - ox) as usize) * 4];
        let drow = &mut dst[(y * dw + x0) * 4..(y * dw + x1) * 4];
        for (d, s) in drow.chunks_exact_mut(4).zip(srow.chunks_exact(4)) {
            let sa = s[3] as u32;
            if sa == 0 && mode != BlendMode::Multiply {
                continue;
            }
            let da = d[3] as u32;
            match mode {
                BlendMode::Plus => {
                    for j in 0..4 {
                        d[j] = (d[j] as u32 + s[j] as u32).min(255) as u8;
                    }
                }
                BlendMode::Multiply => {
                    for j in 0..3 {
                        let (sc, dc) = (s[j] as u32, d[j] as u32);
                        d[j] = (div(sc * dc) + div(sc * (255 - da)) + div(dc * (255 - sa))).min(255) as u8;
                    }
                    d[3] = (sa + da - div(sa * da)).min(255) as u8;
                }
                BlendMode::Screen => {
                    for j in 0..4 {
                        let (sc, dc) = (s[j] as u32, d[j] as u32);
                        d[j] = (sc + dc - div(sc * dc)).min(255) as u8;
                    }
                }
                _ => {
                    for j in 0..4 {
                        d[j] = (s[j] as u32 + div(d[j] as u32 * (255 - sa))).min(255) as u8;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_seeded_and_in_range() {
        let m = TimeMap::Noise { scale: 80.0, seed: 3, octaves: 3 };
        let a: Vec<f32> = (0..200).map(|i| map_u(&m, i as f64 * 7.3, i as f64 * 3.1)).collect();
        let b: Vec<f32> = (0..200).map(|i| map_u(&m, i as f64 * 7.3, i as f64 * 3.1)).collect();
        assert_eq!(a, b);
        assert!(a.iter().all(|v| (0.0..=1.0).contains(v)));
        let m2 = TimeMap::Noise { scale: 80.0, seed: 4, octaves: 3 };
        assert!((0..200).any(|i| map_u(&m2, i as f64 * 7.3, i as f64 * 3.1) != a[i]));
    }

    #[test]
    fn blend_modes() {
        let mut d = vec![0u8, 0, 128, 128];
        blend_into(&mut d, 1, 1, &[100, 0, 0, 100], 1, 1, 0, 0, BlendMode::Plus);
        assert_eq!(d, vec![100, 0, 128, 228]);
        let mut d = vec![0u8, 0, 0, 0];
        blend_into(&mut d, 1, 1, &[10, 20, 30, 40], 1, 1, 0, 0, BlendMode::Normal);
        assert_eq!(d, vec![10, 20, 30, 40]);
        let mut d = vec![255u8, 255, 255, 255];
        blend_into(&mut d, 1, 1, &[255, 0, 0, 255], 1, 1, 0, 0, BlendMode::Multiply);
        assert_eq!(d, vec![255, 0, 0, 255]);
    }

    #[test]
    fn echo_level_times() {
        let tf = TimeFx::Echo { count: 3, span: 0.1, decay: 0.5, blend: BlendMode::Normal, tint: vec![] };
        assert!((level_time(&tf, 1.0, 0.1, 2, 4) - 0.8).abs() < 1e-12);
        let tf = TimeFx::Displace { map: TimeMap::Radial { center: [0.0, 0.0], radius: 10.0 }, span: 1.0, levels: 5 };
        assert!((level_time(&tf, 1.0, 1.0, 4, 5) - 0.0).abs() < 1e-12);
        assert!((map_u(&TimeMap::Radial { center: [0.0, 0.0], radius: 10.0 }, 5.0, 0.0) - 0.5).abs() < 1e-6);
    }
}
