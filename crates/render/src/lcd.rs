//! LCD (subpixel) text antialiasing like Chrome/Skia on Linux.
//!
//! A glyph run is rasterized (grayscale coverage) at 3x horizontal resolution; FreeType's
//! default LCD filter (5-tap FIR 0x08 0x4D 0x56 0x4D 0x08 / 256) turns every three
//! subpixels into per-channel (R, G, B) coverage of one device pixel. vello_cpu has no
//! per-channel alpha, so the run is composited with two images over the (opaque)
//! destination: `dst *= 1 - a·cov` (multiply) and then `dst += color·a·cov` (plus), which
//! is exactly `dst = dst·(1 - a·cov_c) + color_c·a·cov_c` per channel.

/// FreeType `FT_LCD_FILTER_DEFAULT` weights (sum 256).
const FIR: [u32; 5] = [0x08, 0x4D, 0x56, 0x4D, 0x08];

/// Per-channel coverage (0..=255, `[r, g, b]` per pixel, row-major `w x h`) from a
/// subpixel coverage mask of width `3 * w + 4` (two subpixels of filter margin on the left
/// and right): output pixel `x`, channel `c` reads subpixels `3x + c .. 3x + c + 4`.
pub fn filter(sub: &[u8], w: usize, h: usize) -> Vec<[u8; 3]> {
    let sw = 3 * w + 4;
    assert_eq!(sub.len(), sw * h, "subpixel mask size");
    let mut out = vec![[0u8; 3]; w * h];
    for y in 0..h {
        let row = &sub[y * sw..(y + 1) * sw];
        for x in 0..w {
            let mut px = [0u8; 3];
            for (c, v) in px.iter_mut().enumerate() {
                let k = 3 * x + c;
                let s: u32 = (0..5).map(|t| FIR[t] * row[k + t] as u32).sum();
                *v = ((s + 128) >> 8).min(255) as u8;
            }
            out[y * w + x] = px;
        }
    }
    out
}

/// Skia's mask gamma (`SkMaskGamma`, `SkTMaskGamma_build_correcting_lut`): glyph coverage
/// is corrected per text-colour luminance so that blending in sRGB looks like blending in a
/// gamma space, plus a contrast boost that fades out for light text. Chrome on Linux builds
/// it with `SK_GAMMA_CONTRAST` 0.2 and `SK_GAMMA_EXPONENT` 1.2 (paint and device gamma) and
/// applies it to A8 (grayscale) masks too (`SK_GAMMA_APPLY_TO_A8`). Tables are indexed by
/// the colour channel (LCD: R, G, B separately) or luminance (A8) quantized to 3 bits, as
/// `SkTMaskGamma<3, 3, 3>`.
pub struct MaskGamma {
    tables: [[u8; 256]; 8],
}

impl MaskGamma {
    /// `contrast` (Skia `SK_GAMMA_CONTRAST`) and `gamma` (exponent for paint and device).
    pub fn new(contrast: f32, gamma: f32) -> Self {
        let mut tables = [[0u8; 256]; 8];
        for (i, t) in tables.iter_mut().enumerate() {
            // sk_t_scale255<3>: replicate the 3 bits into 8
            let i = i as u32;
            let lum = (i << 5) | (i << 2) | (i >> 1);
            build_correcting_lut(t, lum as u8, contrast, gamma);
        }
        Self { tables }
    }

    /// Identity correction (no gamma, no contrast).
    pub fn identity() -> Self {
        let mut t = [0u8; 256];
        for (i, v) in t.iter_mut().enumerate() { *v = i as u8; }
        Self { tables: [t; 8] }
    }

    /// Table for an 8-bit channel or luminance value.
    pub fn table(&self, v: u8) -> &[u8; 256] { &self.tables[(v >> 5) as usize] }

    /// Per-channel tables for an LCD run in straight sRGB colour `c` (0..1).
    pub fn lcd(&self, c: [f32; 4]) -> [&[u8; 256]; 3] {
        let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        [self.table(q(c[0])), self.table(q(c[1])), self.table(q(c[2]))]
    }

    /// Table for an A8 run: Skia uses the luminance of the colour, computed in the gamma
    /// space (`SkScalerContextRec::setLuminanceColor` / `computeLuminance`).
    pub fn a8(&self, c: [f32; 4], gamma: f32) -> &[u8; 256] {
        let lin = |x: f32| x.clamp(0.0, 1.0).powf(gamma);
        let l = 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
        self.table((l.powf(1.0 / gamma) * 255.0).round() as u8)
    }
}

fn build_correcting_lut(table: &mut [u8; 256], src_i: u8, contrast: f32, gamma: f32) {
    let to_luma = |v: f32| if gamma == 1.0 { v } else { v.powf(gamma) };
    let from_luma = |v: f32| if gamma == 1.0 { v } else { v.powf(1.0 / gamma) };
    let apply_contrast = |a: f32, c: f32| a + (1.0 - a) * c * a;
    let src = src_i as f32 / 255.0;
    let lin_src = to_luma(src);
    // Skia guesses the destination as the perceptual inverse of the source
    let dst = 1.0 - src;
    let lin_dst = to_luma(dst);
    let adjusted = contrast * lin_dst;
    for (i, v) in table.iter_mut().enumerate() {
        let raw = i as f32 / 255.0;
        let srca = apply_contrast(raw, adjusted);
        let out = if (src - dst).abs() < 1.0 / 256.0 {
            srca
        } else {
            let lin_out = lin_src * srca + (1.0 - srca) * lin_dst;
            (from_luma(lin_out) - dst) / (src - dst)
        };
        *v = (255.0 * out).round().clamp(0.0, 255.0) as u8;
    }
}

/// The two premultiplied RGBA images that composite the run: `mul` (multiply blend,
/// opaque: `1 - a·cov_c`) and `add` (plus: `color_c · a · cov_c`, alpha = max channel).
/// `color` is straight sRGB 0..1 with alpha.
fn images_scalar(cov: &[[u8; 3]], color: [f32; 4], luts: [&[u8; 256]; 3]) -> (Vec<u8>, Vec<u8>) {
    let mut mul = Vec::with_capacity(cov.len() * 4);
    let mut add = Vec::with_capacity(cov.len() * 4);
    let a = color[3].clamp(0.0, 1.0);
    for p in cov {
        let mut m = [255u8; 4];
        let mut s = [0u8; 4];
        let mut amax = 0u8;
        for c in 0..3 {
            let k = luts[c][p[c] as usize] as f32 / 255.0 * a;
            m[c] = crate::pixel::round_u8((1.0 - k) * 255.0);
            s[c] = crate::pixel::round_u8(color[c].clamp(0.0, 1.0) * k * 255.0);
            amax = amax.max(crate::pixel::round_u8(k * 255.0));
        }
        s[3] = amax.max(s[0]).max(s[1]).max(s[2]);
        mul.extend_from_slice(&m);
        add.extend_from_slice(&s);
    }
    (mul, add)
}

#[cfg(test)]
fn images_reference(cov: &[[u8; 3]], color: [f32; 4], luts: [&[u8; 256]; 3]) -> (Vec<u8>, Vec<u8>) {
    let mut mul = Vec::with_capacity(cov.len() * 4);
    let mut add = Vec::with_capacity(cov.len() * 4);
    let a = color[3].clamp(0.0, 1.0);
    for p in cov {
        let mut m = [255u8; 4];
        let mut s = [0u8; 4];
        let mut amax = 0u8;
        for c in 0..3 {
            let k = luts[c][p[c] as usize] as f32 / 255.0 * a;
            m[c] = ((1.0 - k) * 255.0).round() as u8;
            s[c] = (color[c].clamp(0.0, 1.0) * k * 255.0).round() as u8;
            amax = amax.max((k * 255.0).round() as u8);
        }
        s[3] = amax.max(s[0]).max(s[1]).max(s[2]);
        mul.extend_from_slice(&m);
        add.extend_from_slice(&s);
    }
    (mul, add)
}

/// Precompute the exact scalar arithmetic for each possible coverage byte.
/// Each add entry stores its channel in the low byte and alpha in the next byte.
pub fn images(cov: &[[u8; 3]], color: [f32; 4], luts: [&[u8; 256]; 3]) -> (Vec<u8>, Vec<u8>) {
    #[cfg(target_arch = "x86_64")]
    if cov.len() >= 256 && is_x86_feature_detected!("avx2") {
        let mut mt = [[0i32; 256]; 3];
        let mut st = [[0i32; 256]; 3];
        let a = color[3].clamp(0.0, 1.0);
        for c in 0..3 {
            for i in 0..256 {
                let k = luts[c][i] as f32 / 255.0 * a;
                mt[c][i] = crate::pixel::round_u8((1.0 - k) * 255.0) as i32;
                let s = crate::pixel::round_u8(color[c].clamp(0.0, 1.0) * k * 255.0);
                let alpha = crate::pixel::round_u8(k * 255.0).max(s);
                st[c][i] = s as i32 | ((alpha as i32) << 8);
            }
        }
        let mut mul = vec![0; cov.len() * 4];
        let mut add = vec![0; cov.len() * 4];
        // Runtime feature check above; buffers have exactly four bytes per pixel.
        unsafe { images_avx2(cov, &mt, &st, &mut mul, &mut add); }
        return (mul, add);
    }
    images_scalar(cov, color, luts)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn images_avx2(cov: &[[u8; 3]], mt: &[[i32; 256]; 3], st: &[[i32; 256]; 3], mul: &mut [u8], add: &mut [u8]) {
    use std::arch::x86_64::*;
    unsafe {
        let mut x = 0;
        while x + 8 <= cov.len() {
            let mut m = _mm256_set1_epi32(0xff000000u32 as i32);
            let mut s = _mm256_setzero_si256();
            let mut a = _mm256_setzero_si256();
            for c in 0..3 {
                let indices: [i32; 8] = std::array::from_fn(|i| cov[x+i][c] as i32);
                let indices = _mm256_loadu_si256(indices.as_ptr().cast());
                let mc = _mm256_i32gather_epi32::<4>(mt[c].as_ptr(), indices);
                let sc = _mm256_i32gather_epi32::<4>(st[c].as_ptr(), indices);
                a = _mm256_max_epi32(a, _mm256_srli_epi32::<8>(sc));
                let shift = _mm256_set1_epi32(c as i32 * 8);
                m = _mm256_or_si256(m, _mm256_sllv_epi32(mc, shift));
                s = _mm256_or_si256(s, _mm256_sllv_epi32(_mm256_and_si256(sc, _mm256_set1_epi32(255)), shift));
            }
            s = _mm256_or_si256(s, _mm256_slli_epi32::<24>(a));
            _mm256_storeu_si256(mul.as_mut_ptr().add(x*4).cast(), m);
            _mm256_storeu_si256(add.as_mut_ptr().add(x*4).cast(), s);
            x += 8;
        }
        for i in x..cov.len() {
            let mut a = 0;
            for c in 0..3 {
                let k = cov[i][c] as usize;
                mul[i*4+c] = mt[c][k] as u8;
                add[i*4+c] = st[c][k] as u8;
                a = a.max((st[c][k] >> 8) as u8);
            }
            mul[i*4+3] = 255;
            add[i*4+3] = a;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simd_matches_original_random() {
        let mut rng = crate::pixel::Rng(42);
        for len in [0, 1, 7, 8, 9, 255, 256, 257, 263, 1023, 4096] {
            for _ in 0..24 {
                let cov: Vec<[u8; 3]> = (0..len).map(|_| std::array::from_fn(|_| (rng.next() >> 24) as u8)).collect();
                let luts: [[u8; 256]; 3] = std::array::from_fn(|_| std::array::from_fn(|_| (rng.next() >> 24) as u8));
                let color = std::array::from_fn(|_| rng.unit() * 1.4 - 0.2);
                let tables = [&luts[0], &luts[1], &luts[2]];
                let expected = images_reference(&cov, color, tables);
                assert_eq!(images_scalar(&cov, color, tables), expected);
                assert_eq!(images(&cov, color, tables), expected);
            }
        }
    }

    #[test]
    #[ignore = "manual release microbenchmark"]
    fn bench_pixels() {
        use std::{hint::black_box, time::Instant};
        let mut rng = crate::pixel::Rng(42);
        let cov: Vec<[u8; 3]> = (0..65536).map(|_| std::array::from_fn(|_| (rng.next() >> 24) as u8)).collect();
        let gamma = MaskGamma::new(0.2, 1.2);
        let color = [0.12, 0.72, 0.9, 0.65];
        let luts = gamma.lcd(color);
        for (label, f) in [("original", images_reference as fn(_, _, _) -> _), ("scalar", images_scalar), ("avx2", images)] {
            let t = Instant::now();
            for _ in 0..500 { black_box(f(black_box(&cov), black_box(color), luts)); }
            eprintln!("lcd {label}: {:?}", t.elapsed());
        }
    }

    #[test]
    fn fir_spreads_one_subpixel_and_keeps_full_coverage() {
        // w = 2 pixels: 10 subpixels (2 margin each side)
        let mut sub = vec![0u8; 10];
        sub[2 + 1] = 255; // subpixel 1 (green of pixel 0)
        let cov = filter(&sub, 2, 1);
        // red of pixel 0 reads subpixels 0..5 -> weight 0x4D on index 3
        assert_eq!(cov[0][0], ((0x4D * 255 + 128) >> 8) as u8);
        assert_eq!(cov[0][1], ((0x56 * 255 + 128) >> 8) as u8);
        assert_eq!(cov[0][2], ((0x4D * 255 + 128) >> 8) as u8);
        let full = vec![255u8; 10];
        assert_eq!(filter(&full, 2, 1), vec![[255, 255, 255]; 2]);
    }
    #[test]
    fn mask_gamma_matches_skia_shape() {
        let g = MaskGamma::new(0.2, 1.2);
        for t in &g.tables {
            assert_eq!(t[0], 0);
            assert_eq!(t[255], 255);
            assert!(t.windows(2).all(|w| w[0] <= w[1]), "monotonic");
        }
        // white text on (guessed) black: no contrast, coverage^(1/1.2) -> bolder
        let w = g.table(255);
        assert_eq!(w[128], ((128.0f32 / 255.0).powf(1.0 / 1.2) * 255.0).round() as u8);
        // black text: contrast 0.2 * 1 boosts mid coverage; gamma thins it back a little
        let b = g.table(0);
        let a: f32 = 128.0 / 255.0;
        let s = a + (1.0 - a) * 0.2 * a;
        let expect = ((1.0 - ((1.0 - s)).powf(1.0 / 1.2)) * 255.0).round() as u8;
        assert_eq!(b[128], expect);
        // identity when contrast 0, gamma 1
        let i = MaskGamma::new(0.0, 1.0);
        assert!(i.tables.iter().all(|t| t.iter().enumerate().all(|(k, v)| *v as usize == k)));
    }

    #[test]
    fn compositing_math_matches_per_channel_blend() {
        let cov = vec![[255u8, 128, 0]];
        let id = MaskGamma::identity();
        let lut = id.table(0);
        let (mul, add) = images(&cov, [0.0, 0.0, 0.0, 1.0], [lut; 3]);
        // black text: dst * (1 - cov)
        assert_eq!(&mul[..3], &[0, 127, 255]);
        assert_eq!(&add[..3], &[0, 0, 0]);
        let (mul, add) = images(&cov, [1.0, 1.0, 1.0, 0.5], [lut; 3]);
        assert_eq!(&mul[..3], &[128, 191, 255]);
        assert_eq!(&add[..3], &[128, 64, 0]);
    }
}
