//! Skia distance-field text (SDFT) emulation.
//!
//! Chrome (Skia Ganesh, also on SwiftShader) draws *grayscale* text whose device matrix has
//! a scale other than 1 through signed distance fields instead of direct masks, when the
//! device text size is roughly 18–162 px (`SDFTControl::isSDFT`). LCD text never does.
//! Measured on headless Chrome on Linux (M4 probe): text inside an
//! `opacity < 1` subtree under a `scale(0.99)` / camera zoom is visibly softer and bolder
//! than the same text at `scale(1)`, boxes and lines in the same subtree are exact, and the
//! effect starts between 14 and 17 px.
//!
//! Skia's pipeline (`SkStrikeSpec::MakeSDFT`, `SkDistanceFieldGen`,
//! `GrDistanceFieldA8TextGeoProc`): the glyph is rasterized at a bucket size (32 / 72 /
//! 162 px), a signed distance field with 4 texels of padding is built from the coverage
//! (edge texels get a sub-texel distance from the coverage gradient), the field is sampled
//! bilinearly at the device pixels and turned into coverage with
//! `smoothstep(-0.65, 0.65, distance_in_device_px)`, plus a luminance-dependent distance
//! adjust that stands in for the mask gamma. This module does the same for a whole run.
//!
//! Known approximations (M4 review): one field per run rather than per glyph (glyphs closer
//! than `MAGNITUDE` texels merge), central-difference gradients instead of Skia's 3x3
//! kernel, scalar distance propagation instead of 8SSEDT vectors, 4-connected hard-edge
//! detection, no 8-bit quantization of the field, anisotropic scales rasterized
//! anisotropically, a constant distance adjust instead of Skia's table, and an upper size
//! limit of 162 px that may really be Skia's `fGlyphsAsPathsFontSize` (324 px; no visible
//! difference measured at 150-200 px). Runs above 4 Mtexels fall back to the mask path.

/// Smallest device text size that uses SDFT (Skia's default `kMinDistanceFieldFontSize` is
/// 18; measured 17 px soft, 14 px direct).
pub const MIN_DEVICE_SIZE: f32 = 16.5;
/// Largest device text size that uses SDFT (`kMaxDistanceFieldFontSize`).
pub const MAX_DEVICE_SIZE: f32 = 162.0;
/// `SK_DistanceFieldPad`: texels of padding around the raster.
pub const PAD: usize = 4;
/// `SK_DistanceFieldMagnitude`: distances are clamped to this many texels.
pub const MAGNITUDE: f32 = 4.0;
/// `kSdfDistanceFieldAAFactor`: half-width of the coverage ramp in device pixels.
pub const AA_FACTOR: f32 = 0.65;

/// Bucket text size (`kSmallDFFontSize` 32, `kMediumDFFontSize` 72, `kLargeDFFontSize` 162)
/// for a device text size.
pub fn bucket(device_size: f32) -> f32 {
    if device_size <= 32.0 {
        32.0
    } else if device_size <= 72.0 {
        72.0
    } else {
        162.0
    }
}

/// Does Skia draw this run as SDFT? `lcd_scene`: the document is LCD text (Chrome), the run
/// is grayscale (inside an isolation layer), and the device matrix scales by `sx`/`sy`.
pub fn applies(lcd_scene: bool, grayscale: bool, sx: f64, sy: f64, device_size: f32) -> bool {
    lcd_scene
        && grayscale
        && ((sx - 1.0).abs() > 1e-4 || (sy - 1.0).abs() > 1e-4)
        && (MIN_DEVICE_SIZE..=MAX_DEVICE_SIZE).contains(&device_size)
}

/// Sub-texel signed distance (negative inside) of an edge texel with coverage `a` (0..1)
/// and coverage gradient (`gx`, `gy`) (Gustavson & Strand, as `SkDistanceFieldGen`).
fn edge_distance(gx: f32, gy: f32, a: f32) -> f32 {
    if gx == 0.0 || gy == 0.0 {
        return 0.5 - a;
    }
    let len = (gx * gx + gy * gy).sqrt();
    let (mut gx, mut gy) = (gx.abs() / len, gy.abs() / len);
    if gx < gy {
        std::mem::swap(&mut gx, &mut gy);
    }
    let a1 = 0.5 * gy / gx;
    if a < a1 {
        0.5 * (gx + gy) - (2.0 * gx * gy * a).sqrt()
    } else if a < 1.0 - a1 {
        (0.5 - a) * gx
    } else {
        -0.5 * (gx + gy) + (2.0 * gx * gy * (1.0 - a)).sqrt()
    }
}

/// Signed distance field (texels, negative inside, clamped to ±`MAGNITUDE`) of an A8
/// coverage image `cov` (`w` × `h`, row-major, 0..=255).
pub fn distance_field(cov: &[u8], w: usize, h: usize) -> Vec<f32> {
    distance_field_impl(cov, w, h, true)
}

fn distance_field_impl(cov: &[u8], w: usize, h: usize, simd: bool) -> Vec<f32> {
    assert_eq!(cov.len(), w * h);
    let at = |x: isize, y: isize| -> f32 {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            0.0
        } else {
            cov[y as usize * w + x as usize] as f32 / 255.0
        }
    };
    // edge texels: partial coverage, or full/empty texels next to a texel on the other
    // side (hard edges); each gets its sub-texel signed distance (NaN = not an edge)
    let mut edge_d = vec![f32::NAN; w * h];
    // edge texel x positions per row, for the window search
    let mut rows: Vec<Vec<f32>> = vec![Vec::new(); h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let a = at(x, y);
            let inside = a >= 0.5;
            let partial = a > 0.0 && a < 1.0;
            // a hard edge between two full/empty texels: both are edge texels (a partial
            // neighbour carries the edge itself)
            let boundary = !partial
                && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| {
                    let n = at(x + dx, y + dy);
                    (n >= 0.5) != inside && (n == 0.0 || n == 1.0)
                });
            if partial || boundary {
                let gx = at(x + 1, y) - at(x - 1, y);
                let gy = at(x, y + 1) - at(x, y - 1);
                edge_d[y as usize * w + x as usize] = edge_distance(gx, gy, a);
                rows[y as usize].push(x as f32);
            }
        }
    }
    let r = MAGNITUDE.ceil() as isize + 1;
    let mut out = vec![0f32; w * h];
    for y in 0..h as isize {
        let mut start = 0;
        #[cfg(target_arch = "x86_64")]
        if simd && is_x86_feature_detected!("avx2") {
            start = unsafe { distance_row_avx2(cov, &edge_d, &rows, w, h, y as usize, &mut out) };
        }
        let _ = simd;
        for x in start..w as isize {
            let i = y as usize * w + x as usize;
            if !edge_d[i].is_nan() {
                out[i] = edge_d[i];
                continue;
            }
            // distance to the edge through the nearest edge texels: centre distance plus
            // that texel's own signed offset, seen from this side
            let sign = if at(x, y) >= 0.5 { -1.0 } else { 1.0 };
            let mut best = MAGNITUDE;
            for yy in (y - r).max(0)..=(y + r).min(h as isize - 1) {
                let dy = (yy - y) as f32;
                // rows are sorted by x: only the window [x - r, x + r]
                let row = &rows[yy as usize];
                let lo = row.partition_point(|&ex| ex < (x - r) as f32);
                for &ex in &row[lo..] {
                    let dx = ex - x as f32;
                    if dx > r as f32 {
                        break;
                    }
                    let d = (dx * dx + dy * dy).sqrt() + sign * edge_d[yy as usize * w + ex as usize];
                    if d < best {
                        best = d.max(0.0);
                    }
                }
            }
            out[i] = sign * best;
        }
    }
    out
}


/// Search the union of eight adjacent pixel windows. Each lane keeps its own
/// sign, window mask, and best distance; edge texels bypass the search result.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn distance_row_avx2(cov: &[u8], edge_d: &[f32], rows: &[Vec<f32>], w: usize, h: usize, y: usize, out: &mut [f32]) -> isize {
    use std::arch::x86_64::*;
    unsafe {
        let end = w / 8 * 8;
        let r = MAGNITUDE.ceil() as usize + 1;
        let zero = _mm256_setzero_ps();
        let limit = _mm256_set1_ps(MAGNITUDE);
        for x in (0..end).step_by(8) {
            let i = y*w+x;
            let edges = _mm256_loadu_ps(edge_d.as_ptr().add(i));
            let is_edge = _mm256_cmp_ps::<_CMP_ORD_Q>(edges, edges);
            if _mm256_movemask_ps(is_edge) == 255 {
                _mm256_storeu_ps(out.as_mut_ptr().add(i), edges);
                continue;
            }
            let coverage = _mm256_cvtepu8_epi32(_mm_loadl_epi64(cov.as_ptr().add(i).cast()));
            // cov/255 >= 0.5 iff cov >= 128, for every byte.
            let inside = _mm256_cmpgt_epi32(coverage, _mm256_set1_epi32(127));
            let sign = _mm256_blendv_ps(_mm256_set1_ps(1.0), _mm256_set1_ps(-1.0), _mm256_castsi256_ps(inside));
            let xs = _mm256_setr_ps(x as f32, (x+1) as f32, (x+2) as f32, (x+3) as f32, (x+4) as f32, (x+5) as f32, (x+6) as f32, (x+7) as f32);
            let mut best = limit;
            for yy in y.saturating_sub(r)..=(y+r).min(h-1) {
                let dy = (yy as isize - y as isize) as f32;
                let dy2 = _mm256_set1_ps(dy*dy);
                let row = &rows[yy];
                let lo = row.partition_point(|&ex| ex < x as f32 - r as f32);
                for &ex in &row[lo..] {
                    if ex > (x+7+r) as f32 { break; }
                    let dx = _mm256_sub_ps(_mm256_set1_ps(ex), xs);
                    let within = _mm256_and_ps(_mm256_cmp_ps::<_CMP_GE_OQ>(dx, _mm256_set1_ps(-(r as f32))), _mm256_cmp_ps::<_CMP_LE_OQ>(dx, _mm256_set1_ps(r as f32)));
                    let d = _mm256_add_ps(_mm256_sqrt_ps(_mm256_add_ps(_mm256_mul_ps(dx, dx), dy2)), _mm256_mul_ps(sign, _mm256_set1_ps(edge_d[yy*w+ex as usize])));
                    let better = _mm256_and_ps(within, _mm256_cmp_ps::<_CMP_LT_OQ>(d, best));
                    best = _mm256_blendv_ps(best, _mm256_max_ps(d, zero), better);
                }
            }
            let result = _mm256_blendv_ps(_mm256_mul_ps(sign, best), edges, is_edge);
            _mm256_storeu_ps(out.as_mut_ptr().add(i), result);
        }
        end as isize
    }
}

/// Bilinear sample of the field at texel coordinates (`u`, `v`); texel centres are at
/// (i + 0.5, j + 0.5). Outside the field the distance is +`MAGNITUDE`.
pub fn sample(field: &[f32], w: usize, h: usize, u: f32, v: f32) -> f32 {
    let fx = u - 0.5;
    let fy = v - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let get = |x: f32, y: f32| -> f32 {
        if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
            MAGNITUDE
        } else {
            field[y as usize * w + x as usize]
        }
    };
    let a = get(x0, y0);
    let b = get(x0 + 1.0, y0);
    let c = get(x0, y0 + 1.0);
    let d = get(x0 + 1.0, y0 + 1.0);
    (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty
}

/// Coverage (0..1) of a device pixel from its sampled distance (texels), `k` texels per
/// device pixel and the distance adjust (texels, positive = bolder).
pub fn coverage(distance: f32, k: f32, adjust: f32) -> f32 {
    let aw = AA_FACTOR * k;
    // Skia: distance is positive inside; smoothstep(-afwidth, afwidth, distance)
    let d = -distance + adjust;
    let t = ((d + aw) / (2.0 * aw)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Distance adjust (texels at the bucket size) for text of straight-sRGB colour `c`:
/// Skia's `DistanceAdjustTable` makes dark text bolder and fades the boost out for light
/// text, like the mask-gamma contrast it replaces. Approximation: a constant per
/// luminance ramp, not Skia's table; `AGENTFX_SDF_ADJUST` sets the value for black text. The
/// default is 0: on a recorded dialog frame the text's ink matches Chrome's at 0
/// (195.5 vs 199.9) and is 11 % too heavy at 0.25.
pub fn adjust(c: [f32; 4]) -> f32 {
    let lum = 0.2126 * c[0].clamp(0.0, 1.0) + 0.7152 * c[1].clamp(0.0, 1.0) + 0.0722 * c[2].clamp(0.0, 1.0);
    let dark: f32 = agentfx_ir::env::var("AGENTFX_SDF_ADJUST").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    dark * (1.0 - lum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simd_matches_scalar_random() {
        let mut rng = crate::pixel::Rng(567);
        for w in [0, 1, 7, 8, 9, 15, 16, 17, 33, 129] {
            for h in [0, 1, 2, 7, 19] {
                for mode in 0..5 {
                    let cov: Vec<u8> = (0..w*h).map(|_| match mode {
                        0 => 0, 1 => 255,
                        2 => if rng.next() & 1024 == 0 { 0 } else { 255 },
                        3 => (rng.next() >> 24) as u8,
                        _ => if rng.next() & 31 == 0 { (rng.next() >> 24) as u8 } else { 0 },
                    }).collect();
                    let scalar = distance_field_impl(&cov, w, h, false);
                    let simd = distance_field(&cov, w, h);
                    assert_eq!(simd.iter().map(|f| f.to_bits()).collect::<Vec<_>>(), scalar.iter().map(|f| f.to_bits()).collect::<Vec<_>>(), "{w}x{h}, mode {mode}");
                }
            }
        }
    }

    #[test]
    #[ignore = "manual release microbenchmark"]
    fn bench_pixels() {
        use std::{hint::black_box, time::Instant};
        let (w, h) = (1024, 64);
        let cov: Vec<u8> = (0..w*h).map(|i| {
            let (x, y) = (i % w % 24, i / w);
            if (6..=17).contains(&x) && (12..52).contains(&y) { if x == 6 || x == 17 { 128 } else { 255 } } else { 0 }
        }).collect();
        for simd in [false, true] {
            let t = Instant::now();
            for _ in 0..100 { black_box(distance_field_impl(black_box(&cov), w, h, simd)); }
            eprintln!("sdft simd={simd}: {:?}", t.elapsed());
        }
    }

    #[test]
    fn field_signs_and_edges() {
        // a 12x12 image with a filled 6x6 square in the middle, hard edges
        let (w, h) = (12, 12);
        let mut cov = vec![0u8; w * h];
        for y in 3..9 {
            for x in 3..9 {
                cov[y * w + x] = 255;
            }
        }
        let f = distance_field(&cov, w, h);
        assert!(f[5 * w + 5] < 0.0, "inside is negative");
        assert!(f[0] > 0.0 && f[0] <= MAGNITUDE, "outside is positive and clamped");
        // hard edge at x = 3.0: boundary texels sit half a texel from it, the next ones 1.5
        assert!((f[5 * w + 2] - 0.5).abs() < 1e-5, "{}", f[5 * w + 2]);
        assert!((f[5 * w + 3] + 0.5).abs() < 1e-5, "{}", f[5 * w + 3]);
        assert!((f[5 * w + 1] - 1.5).abs() < 1e-5, "{}", f[5 * w + 1]);
        assert!((f[5 * w + 4] + 1.5).abs() < 1e-5, "{}", f[5 * w + 4]);
        // bilinear sample across the edge crosses zero at the edge
        let d = sample(&f, w, h, 3.0, 5.5);
        assert!(d.abs() < 1e-5, "{d}");
        // a half-covered edge column: the edge is in the middle of the texel
        let mut cov2 = cov.clone();
        for y in 3..9 {
            cov2[y * w + 2] = 128;
        }
        let f2 = distance_field(&cov2, w, h);
        assert!(f2[5 * w + 2].abs() < 0.01, "{}", f2[5 * w + 2]);
        assert!((f2[5 * w + 1] - 1.0).abs() < 0.02, "{}", f2[5 * w + 1]);
    }

    #[test]
    fn partial_texel_edge() {
        // coverage 0.25 in an edge texel with a horizontal gradient: edge is 0.25 texel inside
        assert!((edge_distance(1.0, 0.0, 0.25) - 0.25).abs() < 1e-6);
        assert!((edge_distance(0.0, 1.0, 0.75) + 0.25).abs() < 1e-6);
    }

    #[test]
    fn coverage_ramp() {
        let k = 2.0;
        assert!((coverage(0.0, k, 0.0) - 0.5).abs() < 1e-6);
        assert_eq!(coverage(2.0 * k, k, 0.0), 0.0);
        assert_eq!(coverage(-2.0 * k, k, 0.0), 1.0);
        // a positive adjust makes the edge bolder
        assert!(coverage(0.0, k, 0.3) > 0.5);
    }

    #[test]
    fn applies_rules() {
        assert!(applies(true, true, 1.12, 1.12, 18.0));
        assert!(!applies(true, true, 1.0, 1.0, 18.0), "scale 1 is a direct mask");
        assert!(!applies(true, false, 1.12, 1.12, 18.0), "LCD text never");
        assert!(!applies(false, true, 1.12, 1.12, 18.0), "grayscale scenes keep their masks");
        assert!(!applies(true, true, 1.12, 1.12, 12.0), "small text is a direct mask");
        assert!(!applies(true, true, 2.0, 2.0, 200.0), "huge text is drawn as paths");
    }
}
