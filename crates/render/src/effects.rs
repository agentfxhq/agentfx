//! Pixel effects on premultiplied RGBA8 buffers (the "raw Rust" part): gaussian blur
//! (three box passes, sigma-exact variance), tinted alpha (shadows, glows), integer
//! offsets, source-over, and alpha/luma mattes. All operate on `w x h` row-major
//! buffers; rows are processed in parallel.
use rayon::prelude::*;

/// Box sizes for three passes whose combined variance equals sigma^2 (Kovesi).
pub(crate) fn boxes(sigma: f64) -> [usize; 3] {
    let n = 3.0;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma - n * (wl * wl) as f64 - 4.0 * n * wl as f64 - 3.0 * n) / (-4.0 * wl as f64 - 4.0);
    let m = m_ideal.round() as i64;
    let mut out = [0usize; 3];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (if (i as i64) < m { wl } else { wu }).max(1) as usize;
    }
    out
}

/// One horizontal box pass, accumulating all four premultiplied channels together.
fn box_h(src: &[u8], dst: &mut [u8], w: usize, r: usize) {
    let norm = (1u64 << 32) / (2 * r as u64 + 1);
    dst.par_chunks_mut(w * 4).zip(src.par_chunks(w * 4)).for_each(|(d, s)| {
        let mut acc = [0u32; 4];
        for p in s.chunks_exact(4).take(r.saturating_add(1)) {
            for c in 0..4 {
                acc[c] += p[c] as u32;
            }
        }
        for x in 0..w {
            for c in 0..4 {
                d[x * 4 + c] = ((acc[c] as u64 * norm + (1 << 31)) >> 32).min(255) as u8;
            }
            if x + r + 1 < w {
                for c in 0..4 {
                    acc[c] += s[(x + r + 1) * 4 + c] as u32;
                }
            }
            if x >= r {
                for c in 0..4 {
                    acc[c] -= s[(x - r) * 4 + c] as u32;
                }
            }
        }
    });
}

#[cfg(test)]
/// One horizontal box pass of radius r over each row (edges = transparent).
fn box_h_reference(src: &[u8], dst: &mut [u8], w: usize, r: usize) {
    let norm = (1u64 << 32) / (2 * r as u64 + 1);
    dst.par_chunks_mut(w * 4).zip(src.par_chunks(w * 4)).for_each(|(d, s)| {
        for c in 0..4 {
            let mut acc: u64 = 0;
            // window [x - r, x + r]
            for x in 0..=r.min(w - 1) {
                acc += s[x * 4 + c] as u64;
            }
            for x in 0..w {
                d[x * 4 + c] = ((acc * norm + (1 << 31)) >> 32).min(255) as u8;
                let add = x + r + 1;
                if add < w {
                    acc += s[add * 4 + c] as u64;
                }
                if x >= r {
                    acc -= s[(x - r) * 4 + c] as u64;
                }
            }
        }
    });
}

fn transpose(src: &[u8], dst: &mut [u8], w: usize, h: usize) {
    // dst is h x w
    dst.par_chunks_mut(h * 4).enumerate().for_each(|(x, row)| {
        for y in 0..h {
            row[y * 4..y * 4 + 4].copy_from_slice(&src[(y * w + x) * 4..(y * w + x) * 4 + 4]);
        }
    });
}

/// Gaussian blur with standard deviation `sigma` (pixels), in place.
///
/// Pixels outside the buffer are transparent. Large radii use a box-filtered
/// reduction and bilinear reconstruction, with their variance removed from sigma.
/// Downsampling begins at sigma ≈ 7.1, keeping reduced sigma at least 3.5.
/// Panics for sigma outside 0..=1,000,000 (including NaN), mismatched dimensions, or dimensions
/// exceeding the safe range of the box filter's 32-bit accumulators.
pub fn blur(buf: &mut [u8], w: usize, h: usize, sigma: f64) {
    assert!(sigma.is_finite() && sigma >= 0.0 && sigma <= 1_000_000.0, "unsupported blur sigma: {sigma}");
    assert_eq!(w.checked_mul(h).and_then(|n| n.checked_mul(4)), Some(buf.len()), "invalid RGBA dimensions");
    assert!(w.max(h) <= (u32::MAX / 255 - 1) as usize, "blur dimensions exceed accumulator range");
    if sigma < 0.2 || w == 0 || h == 0 {
        return;
    }
    let mut scale = 1usize;
    // Keep enough samples across the kernel to limit box-width quantization.
    // The 0.25 accounts for sampling variance in reduced-resolution pixels.
    while sigma / (scale * 2) as f64 >= (3.5_f64.powi(2) + 0.25).sqrt() {
        scale *= 2;
    }
    if scale == 1 {
        blur_full(buf, w, h, sigma);
        return;
    }
    let (sw, sh) = (w.div_ceil(scale), h.div_ceil(scale));
    let mut small = vec![0u8; sw * sh * 4];
    let area = (scale * scale) as u64;
    small.par_chunks_mut(sw * 4).enumerate().for_each(|(y, row)| {
        for x in 0..sw {
            let mut sum = [0u64; 4];
            for sy in y * scale..((y + 1) * scale).min(h) {
                for sx in x * scale..((x + 1) * scale).min(w) {
                    for c in 0..4 {
                        sum[c] += buf[(sy * w + sx) * 4 + c] as u64;
                    }
                }
            }
            for c in 0..4 {
                // Partial blocks include transparent pixels, not replicated edges.
                row[x * 4 + c] = ((sum[c] + area / 2) / area) as u8;
            }
        }
    });
    // Per-axis, phase-averaged variance: box (s²-1)/12 plus
    // center-aligned bilinear reconstruction (2s²+1)/12 = s²/4.
    let reduced_sigma = ((sigma / scale as f64).powi(2) - 0.25).sqrt();
    blur_full(&mut small, sw, sh, reduced_sigma);
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let fy = (y as f64 + 0.5) / scale as f64 - 0.5;
        let iy = fy.floor() as isize;
        let ty = fy - iy as f64;
        for x in 0..w {
            let fx = (x as f64 + 0.5) / scale as f64 - 0.5;
            let ix = fx.floor() as isize;
            let tx = fx - ix as f64;
            let mut value = [0.0; 4];
            for (sy, wy) in [(iy, 1.0 - ty), (iy + 1, ty)] {
                for (sx, wx) in [(ix, 1.0 - tx), (ix + 1, tx)] {
                    if sx >= 0 && sy >= 0 && (sx as usize) < sw && (sy as usize) < sh {
                        let i = (sy as usize * sw + sx as usize) * 4;
                        for c in 0..4 {
                            value[c] += small[i + c] as f64 * wx * wy;
                        }
                    }
                }
            }
            for c in 0..4 {
                row[x * 4 + c] = value[c].round() as u8;
            }
        }
    });
}

fn blur_full(buf: &mut [u8], w: usize, h: usize, sigma: f64) {
    let bx = boxes(sigma);
    let mut tmp = vec![0u8; buf.len()];
    box_h(buf, &mut tmp, w, (bx[0] - 1) / 2);
    box_h(&tmp, buf, w, (bx[1] - 1) / 2);
    box_h(buf, &mut tmp, w, (bx[2] - 1) / 2);
    transpose(&tmp, buf, w, h);
    box_h(buf, &mut tmp, h, (bx[0] - 1) / 2);
    box_h(&tmp, buf, h, (bx[1] - 1) / 2);
    box_h(buf, &mut tmp, h, (bx[2] - 1) / 2);
    transpose(&tmp, buf, h, w);
}

#[cfg(test)]
/// Gaussian blur with standard deviation `sigma` (pixels), in place.
fn blur_reference(buf: &mut [u8], w: usize, h: usize, sigma: f64) {
    if sigma < 0.2 || w == 0 || h == 0 {
        return;
    }
    let bx = boxes(sigma);
    let mut tmp = vec![0u8; buf.len()];
    for &b in &bx {
        box_h_reference(buf, &mut tmp, w, (b - 1) / 2);
        buf.copy_from_slice(&tmp);
    }
    let mut t = vec![0u8; buf.len()];
    transpose(buf, &mut t, w, h);
    let mut t2 = vec![0u8; buf.len()];
    for &b in &bx {
        box_h_reference(&t, &mut t2, h, (b - 1) / 2);
        t.copy_from_slice(&t2);
    }
    transpose(&t, buf, h, w);
}

/// Drop shadow: the alpha tinted with `color`, blurred, shifted by (dx, dy), under `buf`.
pub fn drop_shadow(buf: &mut [u8], w: usize, h: usize, color: [f64; 4], sigma: f64, dx: i64, dy: i64) {
    let mut sh = tint_alpha(buf, color, 1.0);
    blur(&mut sh, w, h, sigma);
    let mut sh = shift(&sh, w, h, dx, dy);
    over(&mut sh, buf);
    buf.copy_from_slice(&sh);
}

/// Glow: the alpha tinted with `color` x `strength`, blurred, under `buf`.
pub fn glow(buf: &mut [u8], w: usize, h: usize, color: [f64; 4], strength: f64, sigma: f64) {
    let mut g = tint_alpha(buf, color, strength);
    blur(&mut g, w, h, sigma);
    over(&mut g, buf);
    buf.copy_from_slice(&g);
}

/// `color` (straight RGBA 0..1) x the buffer's alpha x `strength`, premultiplied.
pub fn tint_alpha(buf: &[u8], color: [f64; 4], strength: f64) -> Vec<u8> {
    let a = (color[3] * strength).clamp(0.0, 1.0);
    let k = [color[0] * a, color[1] * a, color[2] * a, a].map(|v| (v.clamp(0.0, 1.0) * 65536.0) as u32);
    let mut out = vec![0u8; buf.len()];
    out.par_chunks_mut(1 << 16).zip(buf.par_chunks(1 << 16)).for_each(|(o, b)| {
        for (o, p) in o.chunks_exact_mut(4).zip(b.chunks_exact(4)) {
            let al = p[3] as u32;
            for c in 0..4 {
                o[c] = ((al * k[c] + 32768) >> 16).min(255) as u8;
            }
        }
    });
    out
}

/// Shift by integer pixels (content moving out of the buffer is dropped).
pub fn shift(buf: &[u8], w: usize, h: usize, dx: i64, dy: i64) -> Vec<u8> {
    let mut out = vec![0u8; buf.len()];
    out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let sy = y as i64 - dy;
        if sy < 0 || sy >= h as i64 {
            return;
        }
        for x in 0..w {
            let sx = x as i64 - dx;
            if sx >= 0 && sx < w as i64 {
                let s = (sy as usize * w + sx as usize) * 4;
                row[x * 4..x * 4 + 4].copy_from_slice(&buf[s..s + 4]);
            }
        }
    });
    out
}

/// `top` over `under` (premultiplied), result in `under`.
pub fn over(under: &mut [u8], top: &[u8]) {
    let px = |u: &mut [u8], t: &[u8]| {
        let ia = 255 - t[3] as u32;
        for c in 0..4 {
            u[c] = (t[c] as u32 + (u[c] as u32 * ia + 127) / 255).min(255) as u8;
        }
    };
    if under.len() < PAR_MIN {
        under.chunks_exact_mut(4).zip(top.chunks_exact(4)).for_each(|(u, t)| px(u, t));
    } else {
        // 64 KB work items (per-pixel items cost more to schedule than to compute)
        under.par_chunks_mut(1 << 16).zip(top.par_chunks(1 << 16)).for_each(|(u, t)| u.chunks_exact_mut(4).zip(t.chunks_exact(4)).for_each(|(u, t)| px(u, t)));
    }
}

/// Buffers (bytes) below this run per-pixel passes serially: rayon's dispatch costs more
/// than the pass on the small offscreens of per-layer effects (same arithmetic).
pub const PAR_MIN: usize = 1 << 18;

/// Multiply `buf` by the matte's coverage: alpha or Rec. 709 luma (of the unpremultiplied
/// color, times alpha), optionally inverted.
pub fn matte(buf: &mut [u8], matte: &[u8], luma: bool, invert: bool) {
    let px = |p: &mut [u8], m: &[u8]| {
        let mut v = if luma {
            // premultiplied rgb: luma * alpha directly
            (0.2126 * m[0] as f64 + 0.7152 * m[1] as f64 + 0.0722 * m[2] as f64).round().min(255.0) as u32
        } else {
            m[3] as u32
        };
        if invert {
            v = 255 - v;
        }
        for c in p.iter_mut() {
            *c = ((*c as u32 * v + 127) / 255) as u8;
        }
    };
    if buf.len() < PAR_MIN {
        buf.chunks_exact_mut(4).zip(matte.chunks_exact(4)).for_each(|(p, m)| px(p, m));
    } else {
        buf.par_chunks_mut(1 << 16).zip(matte.par_chunks(1 << 16)).for_each(|(b, m)| b.chunks_exact_mut(4).zip(m.chunks_exact(4)).for_each(|(p, m)| px(p, m)));
    }
}

/// Average of `n` premultiplied buffers accumulated in `acc` (u16 sums per channel).
pub fn accumulate(acc: &mut [u16], buf: &[u8]) {
    acc.par_chunks_mut(4096).zip(buf.par_chunks(4096)).for_each(|(a, b)| {
        for (x, y) in a.iter_mut().zip(b) {
            *x += *y as u16;
        }
    });
}

/// `acc += k * buf` (the sum of `k` identical samples; integer, so exact), serial below
/// 1 MB (rayon's dispatch costs more than the adds for the small offscreens of per-layer
/// motion blur).
pub fn accumulate_weighted(acc: &mut [u16], buf: &[u8], k: u16) {
    let add = |a: &mut [u16], b: &[u8]| {
        for (x, y) in a.iter_mut().zip(b) {
            *x += *y as u16 * k;
        }
    };
    if acc.len() < 1 << 20 {
        add(acc, buf);
    } else {
        acc.par_chunks_mut(1 << 16).zip(buf.par_chunks(1 << 16)).for_each(|(a, b)| add(a, b));
    }
}

/// `average` serial below 1 MB (see `accumulate_weighted`); same rounding.
pub fn average_small(acc: &[u16], n: u32) -> Vec<u8> {
    if acc.len() >= 1 << 20 {
        return average(acc, n);
    }
    acc.iter().map(|&v| ((v as u32 + n / 2) / n).min(255) as u8).collect()
}

pub fn average(acc: &[u16], n: u32) -> Vec<u8> {
    acc.par_iter().map(|&v| ((v as u32 + n / 2) / n).min(255) as u8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn synthetic(w: usize, h: usize, margin: usize) -> Vec<u8> {
        let mut image = vec![0; w * h * 4];
        for y in margin..h - margin {
            for x in margin..w - margin {
                let a = ((x - margin) * 200 / (w - 2 * margin) + 55) as u8;
                let p = if x < w / 2 && y < h / 2 {
                    [a, a / 3, a / 5, a]
                } else if (x as isize - (w * 2 / 3) as isize).pow(2)
                    + (y as isize - (h * 2 / 3) as isize).pow(2) < (w / 7).pow(2) as isize {
                    [a / 4, a, a / 2, a]
                } else if x % 19 < 2 || y % 23 < 2 {
                    [a / 2, a / 3, a, a]
                } else {
                    [0; 4]
                };
                image[(y * w + x) * 4..(y * w + x + 1) * 4].copy_from_slice(&p);
            }
        }
        image
    }

    fn alpha_mass(image: &[u8]) -> u64 {
        image.chunks_exact(4).map(|p| p[3] as u64).sum()
    }

    #[test]
    fn fast_blur_quality() {
        // Odd dimensions exercise partial downsampling blocks. The border case
        // also checks transparent edges with content touching all four sides.
        for margin in [0, 200] {
            let (w, h) = (801, 773);
            let original = synthetic(w, h, margin);
            for sigma in [8.0, 20.0, 60.0] {
                let mut reference = original.clone();
                let mut fast = original.clone();
                blur_reference(&mut reference, w, h, sigma);
                blur(&mut fast, w, h, sigma);
                let mse = fast.iter().zip(&reference).map(|(&a, &b)| {
                    (a as f64 - b as f64).powi(2)
                }).sum::<f64>() / fast.len() as f64;
                let psnr = 10.0 * (255.0 * 255.0 / mse).log10();
                eprintln!("sigma {sigma}, margin {margin}: PSNR {psnr:.2} dB");
                assert!(psnr >= 40.0, "sigma {sigma}, margin {margin}: PSNR {psnr}");
                assert!(fast.chunks_exact(4).all(|p| p[..3].iter().all(|&c| c <= p[3])));
                // Transparent boundaries intentionally lose mass. Compare against
                // the reference there, and against the input for padded content.
                let expected = if margin == 0 { alpha_mass(&reference) } else { alpha_mass(&original) };
                let error = (alpha_mass(&fast) as f64 / expected as f64 - 1.0).abs();
                assert!(error <= 0.02, "sigma {sigma}, margin {margin}: mass error {error}");
            }
        }
    }

    #[test]
    fn full_blur_matches_reference() {
        for (w, h) in [(1, 1), (1, 31), (37, 1), (33, 27)] {
            let original: Vec<u8> = (0..w * h * 4).map(|i| (i * 73 % 256) as u8).collect();
            for sigma in [0.2, 1.0, 3.0, 6.0, 60.0] {
                let mut reference = original.clone();
                let mut actual = original.clone();
                blur_reference(&mut reference, w, h, sigma);
                blur_full(&mut actual, w, h, sigma);
                assert_eq!(actual, reference, "{w}x{h}, sigma {sigma}");
            }
        }
    }

    #[test]
    fn blur_degenerate_and_invalid_inputs() {
        blur(&mut [], 0, 3, 8.0);
        for (w, h) in [(1, 1), (1, 37), (35, 1), (3, 5)] {
            let mut pixels = vec![255; w * h * 4];
            blur(&mut pixels, w, h, 60.0);
            assert!(pixels.chunks_exact(4).all(|p| p.iter().all(|c| *c == p[3])));
        }
        for sigma in [f64::NAN, f64::INFINITY, -1.0, 1e20] {
            assert!(std::panic::catch_unwind(|| blur(&mut [0; 4], 1, 1, sigma)).is_err());
        }
        assert!(std::panic::catch_unwind(|| blur(&mut [0; 4], 2, 1, 1.0)).is_err());
    }

    #[test]
    #[ignore = "manual release timing on a shared machine"]
    fn blur_timing() {
        use std::time::Instant;
        let (w, h) = (1200, 1200);
        let original = synthetic(w, h, 100);
        // Warm both implementations and report the median of five interleaved runs.
        let mut reference_ms = Vec::new();
        let mut fast_ms = Vec::new();
        for iteration in 0..6 {
            for (implementation, timings) in [
                (blur_reference as fn(&mut [u8], usize, usize, f64), &mut reference_ms),
                (blur as fn(&mut [u8], usize, usize, f64), &mut fast_ms),
            ] {
                let mut pixels = original.clone();
                let start = Instant::now();
                implementation(&mut pixels, w, h, 90.0);
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(&pixels);
                if iteration > 0 {
                    timings.push(elapsed);
                }
            }
        }
        reference_ms.sort_by(f64::total_cmp);
        fast_ms.sort_by(f64::total_cmp);
        eprintln!("1200x1200 sigma 90: before {:.2} ms, after {:.2} ms, speedup {:.2}x ({} Rayon threads)",
            reference_ms[2], fast_ms[2], reference_ms[2] / fast_ms[2], rayon::current_num_threads());
    }

    #[test]
    fn blur_preserves_mass_and_spreads() {
        let (w, h) = (64, 64);
        let mut b = vec![0u8; w * h * 4];
        for y in 28..36 {
            for x in 28..36 {
                b[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let before: u64 = b.iter().map(|&v| v as u64).sum();
        blur(&mut b, w, h, 3.0);
        let after: u64 = b.iter().map(|&v| v as u64).sum();
        assert!((after as f64 - before as f64).abs() / (before as f64) < 0.03, "{before} {after}");
        assert!(b[(32 * w + 22) * 4 + 3] > 0, "spread");
        assert!(b[(32 * w + 32) * 4 + 3] < 255, "center attenuated");
    }
    #[test]
    fn variance_matches_sigma() {
        for s in [1.0, 2.5, 8.0] {
            let bx = boxes(s);
            let var: f64 = bx.iter().map(|&b| ((b * b) as f64 - 1.0) / 12.0).sum();
            assert!((var.sqrt() - s).abs() < 0.35, "sigma {s}: {}", var.sqrt());
        }
    }
    #[test]
    fn matte_and_over() {
        let mut c = vec![200u8, 100, 50, 255];
        matte(&mut c, &[0, 0, 0, 128], false, false);
        assert_eq!(c, vec![100, 50, 25, 128]);
        let mut u = vec![0u8, 0, 255, 255];
        over(&mut u, &[128, 0, 0, 128]);
        assert_eq!(u, vec![128, 0, 127, 255]);
    }
}
