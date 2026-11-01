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
    upsample(&small, sw, sh, scale, buf, 0, 0, w, h);
}

/// `blur` of a `w x h` buffer that is never held in full: the input arrives in tiles (on the
/// reduction grid), the output is produced for one crop rectangle only. Bitwise the
/// corresponding pixels of `blur` (same reduction sums, small blur and reconstruction), for
/// huge, mostly invisible rasters (a glow on a layer scaled far beyond the frame).
pub struct TiledBlur {
    w: usize,
    h: usize,
    scale: usize,
    sw: usize,
    sh: usize,
    sigma: f64,
    sums: Vec<u64>,
}

impl TiledBlur {
    /// None when `blur` would not reduce (sigma below the downsampling limit).
    pub fn new(w: usize, h: usize, sigma: f64) -> Option<Self> {
        if !(sigma.is_finite() && sigma >= 0.2 && sigma <= 1_000_000.0) || w == 0 || h == 0 || w.max(h) > (u32::MAX / 255 - 1) as usize {
            return None;
        }
        let scale = Self::scale_for(sigma);
        if scale == 1 {
            return None;
        }
        let (sw, sh) = (w.div_ceil(scale), h.div_ceil(scale));
        Some(TiledBlur { w, h, scale, sw, sh, sigma, sums: vec![0; sw * sh * 4] })
    }

    /// The reduction factor `blur` uses for `sigma` (tiles must start on multiples of it).
    pub fn scale_for(sigma: f64) -> usize {
        let mut scale = 1usize;
        while sigma / (scale * 2) as f64 >= (3.5_f64.powi(2) + 0.25).sqrt() {
            scale *= 2;
        }
        scale
    }

    pub fn scale(&self) -> usize {
        self.scale
    }

    /// Add the input tile `tile` (`tw x th`, RGBA8 premultiplied) at (`tx`, `ty`) of the
    /// full buffer; `tx`, `ty` multiples of the scale, every pixel added once.
    pub fn add(&mut self, tile: &[u8], tx: usize, ty: usize, tw: usize, th: usize) {
        assert!(tx % self.scale == 0 && ty % self.scale == 0 && tx + tw <= self.w && ty + th <= self.h && tile.len() == tw * th * 4);
        let (sc, sw) = (self.scale, self.sw);
        let (bx0, by0) = (tx / sc, ty / sc);
        let (bx1, by1) = ((tx + tw).div_ceil(sc), (ty + th).div_ceil(sc));
        let rows = &mut self.sums[by0 * sw * 4..by1 * sw * 4];
        rows.par_chunks_mut(sw * 4).enumerate().for_each(|(j, row)| {
            let by = by0 + j;
            for bx in bx0..bx1 {
                let mut sum = [0u64; 4];
                for y in (by * sc).max(ty)..((by + 1) * sc).min(ty + th) {
                    let r = &tile[(y - ty) * tw * 4..][..tw * 4];
                    for x in (bx * sc).max(tx)..((bx + 1) * sc).min(tx + tw) {
                        for c in 0..4 {
                            sum[c] += r[(x - tx) * 4 + c] as u64;
                        }
                    }
                }
                for c in 0..4 {
                    row[bx * 4 + c] += sum[c];
                }
            }
        });
    }

    /// The blurred pixels of crop rect (`cx`, `cy`, `cw`, `ch`) into `out` (`cw x ch`).
    pub fn finish(self, out: &mut [u8], cx: usize, cy: usize, cw: usize, ch: usize) {
        assert!(cx + cw <= self.w && cy + ch <= self.h && out.len() == cw * ch * 4);
        let (scale, sw, sh) = (self.scale, self.sw, self.sh);
        let area = (scale * scale) as u64;
        // Partial blocks include transparent pixels, not replicated edges.
        let mut small: Vec<u8> = self.sums.iter().map(|&v| ((v + area / 2) / area) as u8).collect();
        let reduced_sigma = ((self.sigma / scale as f64).powi(2) - 0.25).sqrt();
        blur_full(&mut small, sw, sh, reduced_sigma);
        upsample(&small, sw, sh, scale, out, cx, cy, cw, ch);
    }
}

/// Bilinear reconstruction of the reduced buffer for the rows/columns
/// [cy, cy + ch) x [cx, cx + cw) of the full buffer (shared by `blur` and `TiledBlur`).
#[allow(clippy::too_many_arguments)]
fn upsample(small: &[u8], sw: usize, sh: usize, scale: usize, out: &mut [u8], cx: usize, cy: usize, cw: usize, ch: usize) {
    debug_assert_eq!(out.len(), cw * ch * 4);
    if cw == 0 || ch == 0 {
        return;
    }
    // the column taps are the same for every row (computed once, the arithmetic per pixel
    // is unchanged), and pixels whose four taps are all transparent stay 0 (as the
    // weighted sum would)
    let cols: Vec<(isize, f64)> = (cx..cx + cw).map(|x| {
        let fx = (x as f64 + 0.5) / scale as f64 - 0.5;
        let ix = fx.floor() as isize;
        (ix, fx - ix as f64)
    }).collect();
    out.par_chunks_mut(cw * 4).enumerate().for_each(|(j, row)| {
        let y = cy + j;
        let fy = (y as f64 + 0.5) / scale as f64 - 0.5;
        let iy = fy.floor() as isize;
        let ty = fy - iy as f64;
        let tap = |sx: isize, sy: isize| -> Option<&[u8]> {
            (sx >= 0 && sy >= 0 && (sx as usize) < sw && (sy as usize) < sh).then(|| &small[(sy as usize * sw + sx as usize) * 4..][..4])
        };
        let zero = |p: Option<&[u8]>| p.is_none_or(|p| p == [0, 0, 0, 0]);
        for (x, &(ix, tx)) in cols.iter().enumerate() {
            let taps = [tap(ix, iy), tap(ix + 1, iy), tap(ix, iy + 1), tap(ix + 1, iy + 1)];
            if taps.iter().all(|&t| zero(t)) {
                row[x * 4..x * 4 + 4].fill(0);
                continue;
            }
            let mut value = [0.0; 4];
            for (k, wy) in [(0, 1.0 - ty), (2, ty)] {
                for (i, wx) in [(0, 1.0 - tx), (1, tx)] {
                    if let Some(p) = taps[k + i] {
                        for c in 0..4 {
                            value[c] += p[c] as f64 * wx * wy;
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
    fn tiled_blur_matches_blur_crop() {
        let (w, h) = (301, 233);
        let src = synthetic(w, h, 9);
        for sigma in [7.5, 15.0, 40.0, 130.0] {
            let mut full = src.clone();
            blur(&mut full, w, h, sigma);
            let mut tb = TiledBlur::new(w, h, sigma).unwrap();
            let t = tb.scale() * 3;
            for ty in (0..h).step_by(t) {
                for tx in (0..w).step_by(t) {
                    let (tw, th) = (t.min(w - tx), t.min(h - ty));
                    let tile: Vec<u8> = (ty..ty + th).flat_map(|y| src[(y * w + tx) * 4..(y * w + tx + tw) * 4].to_vec()).collect();
                    tb.add(&tile, tx, ty, tw, th);
                }
            }
            let (cx, cy, cw, ch) = (37, 50, 190, 101);
            let mut out = vec![0u8; cw * ch * 4];
            tb.finish(&mut out, cx, cy, cw, ch);
            for y in 0..ch {
                assert_eq!(&out[y * cw * 4..(y + 1) * cw * 4], &full[((cy + y) * w + cx) * 4..((cy + y) * w + cx + cw) * 4], "sigma {sigma}, row {y}");
            }
        }
        assert!(TiledBlur::new(100, 100, 5.0).is_none());
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
