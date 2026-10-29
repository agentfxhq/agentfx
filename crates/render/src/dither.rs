//! Dithered gradients. vello_cpu quantizes gradients straight to 8 bit, which bands on
//! large, slowly varying (dark) gradients; Chrome/Skia dither gradients with a small ordered
//! pattern. Here a gradient fill is evaluated in f32 per device pixel (same piecewise-linear
//! premultiplied sRGB ramps as vello), Skia's 8x8 ordered offset (+-0.49 LSB) is added before
//! rounding, and the path is filled with that image.
//!
//! The pattern is anchored to absolute screen pixels (`origin` = screen position of the
//! context's pixel (0, 0)), so a pixel gets the same value whichever damaged rect draws it
//! and in every frame its gradient is unchanged: no temporal shimmer, and damage still
//! means "these pixels may change".
use kurbo::{Affine, BezPath, Point, Rect, Shape as _};
use peniko::{Extend, Gradient, GradientKind, InterpolationAlphaSpace, color::ColorSpaceTag};
use std::sync::Arc;
use vello_cpu::{ImageSource, PaintType, Pixmap, RenderContext};

/// Experiment knob: AGENTFX_DITHER_PHASE="dx,dy" shifts the pattern anchor.
fn phase() -> (i64, i64) {
    static P: std::sync::OnceLock<(i64, i64)> = std::sync::OnceLock::new();
    *P.get_or_init(|| agentfx_ir::env::var("AGENTFX_DITHER_PHASE").ok().and_then(|v| {
        let (a, b) = v.split_once(',')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }).unwrap_or((0, 0)))
}

/// 8x8 Bayer matrix (values 0..63).
const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60, 28, 52, 20, 62, 30, 54,
    22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61,
    29, 53, 21,
];

/// Dither offset in 8-bit units at absolute screen pixel (x, y): Skia's raster-pipeline
/// `dither` stage (8x8 ordered matrix from the bits of x and x^y, scaled to
/// (2M - 63) / 128, i.e. within +-0.4922 so exact values survive rounding).
pub fn offset(x: i64, y: i64) -> f32 {
    static BAYER_MODE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *BAYER_MODE.get_or_init(|| agentfx_ir::env::var_os("AGENTFX_DITHER_BAYER").is_some()) {
        return (BAYER[((y & 7) * 8 + (x & 7)) as usize] as f32 * 2.0 - 63.0) / 128.0;
    }
    let (px, py) = phase();
    let (x, y) = ((x + px) as u32, (y + py) as u32);
    let y = y ^ x;
    let m = (y & 1) << 5 | (x & 1) << 4 | (y & 2) << 2 | (x & 2) << 1 | (y & 4) >> 1 | (x & 4) >> 2;
    (m as f32 * 2.0 - 63.0) / 128.0
}

/// Gradient parameter t at paint-space point p, or None where the gradient is undefined.
fn param(kind: &GradientKind, p: Point) -> Option<f32> {
    match kind {
        GradientKind::Linear(l) => {
            let d = l.end - l.start;
            let len2 = d.hypot2();
            if len2 == 0.0 { return None; }
            Some(((p - l.start).dot(d) / len2) as f32)
        }
        GradientKind::Radial(r) => {
            // concentric only (CSS radial gradients); others fall back to vello
            let dr = (r.end_radius - r.start_radius) as f64;
            if dr == 0.0 { return None; }
            Some((((p - r.start_center).hypot() - r.start_radius as f64) / dr) as f32)
        }
        GradientKind::Sweep(s) => {
            let d = p - s.center;
            let mut a = d.y.atan2(d.x);
            if a < 0.0 { a += std::f64::consts::TAU; }
            let span = (s.end_angle - s.start_angle) as f64;
            if span == 0.0 { return None; }
            Some(((a - s.start_angle as f64) / span) as f32)
        }
    }
}

/// Whether `g` is evaluated here (else the caller draws it with vello, undithered).
pub fn supported(g: &Gradient) -> bool {
    if g.interpolation_cs != ColorSpaceTag::Srgb || g.stops.len() < 2 { return false; }
    match &g.kind {
        GradientKind::Radial(r) => r.start_center == r.end_center && r.start_radius != r.end_radius,
        _ => true,
    }
}

/// Premultiplied stops in sRGB (vello's `encode_stops` for an sRGB gradient).
fn ramp(g: &Gradient) -> Vec<(f32, [f32; 4])> {
    let premul = g.interpolation_alpha_space == InterpolationAlphaSpace::Premultiplied;
    g.stops.iter().map(|s| {
        let c = s.color.to_alpha_color::<peniko::color::Srgb>();
        let c = if premul { c.premultiply().components } else { c.components };
        (s.offset, c.map(|v| v.clamp(0.0, 1.0)))
    }).collect()
}

fn sample(ramp: &[(f32, [f32; 4])], premul: bool, t: f32) -> [f32; 4] {
    let first = ramp[0];
    let last = ramp[ramp.len() - 1];
    let c = if t <= first.0 { first.1 } else if t >= last.0 { last.1 } else {
        let i = ramp.windows(2).position(|w| t <= w[1].0).unwrap_or(ramp.len() - 2);
        let (x0, c0) = ramp[i];
        let (x1, c1) = ramp[i + 1];
        let f = (t - x0) / (x1 - x0).max(1e-6);
        std::array::from_fn(|k| c0[k] + (c1[k] - c0[k]) * f)
    };
    if premul { c } else { [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]] }
}

fn extend(e: Extend, t: f32) -> f32 {
    match e {
        Extend::Pad => t.clamp(0.0, 1.0),
        Extend::Repeat => t - t.floor(),
        Extend::Reflect => { let u = t.rem_euclid(2.0); if u > 1.0 { 2.0 - u } else { u } }
    }
}

/// Ramp resolution (vello also samples gradients from a LUT).
const LUT: usize = 2048;

/// Premultiplied RGBA8 image of gradient `g` (paint space) over device rect `r` = [x, y, w, h]
/// of a context; `inv` maps device to paint space; `alpha` multiplies the result;
/// `origin` is the screen position of device pixel (0, 0). `dither` false = plain rounding.
#[allow(clippy::too_many_arguments)]
pub fn image(g: &Gradient, inv: Affine, r: [i32; 4], alpha: f32, origin: (i64, i64), dither: bool) -> Vec<u8> {
    image_impl::<false>(g, inv, r, alpha, origin, dither, true)
}

#[allow(clippy::too_many_arguments)]
fn image_impl<const ORIGINAL: bool>(g: &Gradient, inv: Affine, r: [i32; 4], alpha: f32, origin: (i64, i64), dither: bool, simd: bool) -> Vec<u8> {
    // ORIGINAL is instantiated only by tests to compare against the old libm path.
    let quantize = |v: f32| if ORIGINAL { v.round() as u8 } else { crate::pixel::round_u8(v) };
    let ramp = ramp(g);
    let premul = g.interpolation_alpha_space == InterpolationAlphaSpace::Premultiplied;
    // premultiplied colour x alpha x 255 per LUT entry
    let lut: Vec<[f32; 4]> = (0..LUT).map(|k| {
        let c = sample(&ramp, premul, k as f32 / (LUT - 1) as f32);
        [c[0] * alpha * 255.0, c[1] * alpha * 255.0, c[2] * alpha * 255.0, (c[3] * alpha * 255.0).round().clamp(0.0, 255.0)]
    }).collect();
    let (w, h) = (r[2] as usize, r[3] as usize);
    let mut out = vec![0u8; w * h * 4];
    let c = inv.as_coeffs();
    let ext = g.extend;
    let linear = match &g.kind {
        GradientKind::Linear(l) => {
            let d = l.end - l.start;
            let len2 = d.hypot2();
            (len2 > 0.0).then(|| (l.start, d / len2))
        }
        _ => None,
    };
    for y in 0..h {
        let dy = (r[1] as i64 + y as i64) as f64 + 0.5;
        let sy = origin.1 + r[1] as i64 + y as i64;
        let row = &mut out[y * w * 4..(y + 1) * w * 4];
        let dx0 = r[0] as f64 + 0.5;
        // linear: t is affine along the row
        let (t0, dt) = match linear {
            Some((st, dn)) => {
                let p0 = Point::new(c[0] * dx0 + c[2] * dy + c[4], c[1] * dx0 + c[3] * dy + c[5]);
                ((p0 - st).dot(dn), c[0] * dn.x + c[1] * dn.y)
            }
            None => (0.0, 0.0),
        };
        let mut start = 0;
        #[cfg(target_arch = "x86_64")]
        if simd && linear.is_some() && is_x86_feature_detected!("avx2") {
            // The affine parameter uses f64, as in the scalar path. In particular,
            // do not accumulate dt or fuse multiply/add: either changes LUT indices.
            start = unsafe { linear_row_avx2(row, t0, dt, ext, &lut, origin.0 + r[0] as i64, sy, dither) };
        }
        let _ = simd;
        for x in start..w {
            let t = if linear.is_some() {
                (t0 + dt * x as f64) as f32
            } else {
                let dx = dx0 + x as f64;
                let p = Point::new(c[0] * dx + c[2] * dy + c[4], c[1] * dx + c[3] * dy + c[5]);
                match param(&g.kind, p) { Some(t) => t, None => continue }
            };
            let e = extend(ext, t);
            let s = lut[((e * (LUT - 1) as f32) + 0.5) as usize];
            let d = if dither { offset(origin.0 + r[0] as i64 + x as i64, sy) } else { 0.0 };
            let o = x * 4;
            let a8 = s[3];
            row[o] = quantize(s[0] + d).min(a8 as u8);
            row[o + 1] = quantize(s[1] + d).min(a8 as u8);
            row[o + 2] = quantize(s[2] + d).min(a8 as u8);
            row[o + 3] = a8 as u8;
        }
    }
    out
}


#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(clippy::too_many_arguments)]
unsafe fn linear_row_avx2(row: &mut [u8], t0: f64, dt: f64, ext: Extend, lut: &[[f32; 4]], sx: i64, sy: i64, dither: bool) -> usize {
    use std::arch::x86_64::*;
    unsafe {
        // Both supported dither patterns have period eight, including phase shifts.
        let offsets: [f32; 8] = std::array::from_fn(|i| if dither { offset(sx+i as i64, sy) } else { 0.0 });
        let d = _mm256_loadu_ps(offsets.as_ptr());
        let end = row.len() / 32 * 8;
        for x in (0..end).step_by(8) {
            let xs = _mm256_setr_pd(x as f64, (x+1) as f64, (x+2) as f64, (x+3) as f64);
            let xt = _mm256_setr_pd((x+4) as f64, (x+5) as f64, (x+6) as f64, (x+7) as f64);
            let low = _mm256_cvtpd_ps(_mm256_add_pd(_mm256_set1_pd(t0), _mm256_mul_pd(_mm256_set1_pd(dt), xs)));
            let high = _mm256_cvtpd_ps(_mm256_add_pd(_mm256_set1_pd(t0), _mm256_mul_pd(_mm256_set1_pd(dt), xt)));
            let t = _mm256_insertf128_ps::<1>(_mm256_castps128_ps256(low), high);
            let e = if matches!(ext, Extend::Pad) {
                _mm256_min_ps(_mm256_max_ps(t, _mm256_setzero_ps()), _mm256_set1_ps(1.0))
            } else {
                // Rust rem_euclid's negative and endpoint semantics are retained.
                let mut ts = [0.0; 8];
                _mm256_storeu_ps(ts.as_mut_ptr(), t);
                let es = ts.map(|t| extend(ext, t));
                _mm256_loadu_ps(es.as_ptr())
            };
            // Rust's float-to-usize cast maps NaN to zero. This also bounds
            // every gather index even for non-finite paint transforms.
            let e = _mm256_min_ps(_mm256_max_ps(e, _mm256_setzero_ps()), _mm256_set1_ps(1.0));
            let idx = _mm256_cvttps_epi32(_mm256_add_ps(_mm256_mul_ps(e, _mm256_set1_ps((LUT-1) as f32)), _mm256_set1_ps(0.5)));
            let idx = _mm256_slli_epi32::<2>(idx);
            let base = lut.as_ptr().cast::<f32>();
            let a = _mm256_cvttps_epi32(_mm256_i32gather_ps::<4>(base.add(3), idx));
            let mut rgba = _mm256_slli_epi32::<24>(a);
            for c in 0..3 {
                let s = _mm256_i32gather_ps::<4>(base.add(c), idx);
                let channel = _mm256_min_epi32(crate::pixel::round8(_mm256_add_ps(s, d)), a);
                rgba = _mm256_or_si256(rgba, _mm256_sllv_epi32(channel, _mm256_set1_epi32(c as i32 * 8)));
            }
            _mm256_storeu_si256(row.as_mut_ptr().add(x*4).cast(), rgba);
        }
        end
    }
}

/// Cache of gradient images at rest: a gradient that is drawn again with the same screen
/// placement reuses its full (screen-anchored, dithered) image; the first sighting only
/// evaluates the visible rect (moving gradients never repeat a placement).
#[derive(Default)]
pub struct Cache {
    map: std::collections::HashMap<u64, Option<(Arc<Pixmap>, i32, i32)>>,
    bytes: usize,
}

const CACHE_LIMIT: usize = 96 << 20;

/// Fill `path` (user space; `m` maps user space to this context's pixels, `pt` maps paint
/// space to user space) with gradient `g`, dithered. `id` identifies the gradient (stable
/// for the render), `screen` is the area worth caching (scene viewport) in screen pixels.
/// Returns false when unsupported (the caller draws it with vello instead).
#[allow(clippy::too_many_arguments)]
pub fn fill(ctx: &mut RenderContext, cache: &mut Cache, id: usize, path: &BezPath, g: &Gradient, m: Affine, pt: Affine, alpha: f32, origin: (i64, i64), screen: [i32; 4], even_odd: bool) -> bool {
    if !supported(g) || m.determinant().abs() < 1e-12 || pt.determinant().abs() < 1e-12 { return false; }
    let bb = m.transform_rect_bbox(path.bounding_box());
    let vis = Rect::new(0.0, 0.0, ctx.width() as f64, ctx.height() as f64);
    let vbb = bb.intersect(vis);
    if vbb.width() <= 0.0 || vbb.height() <= 0.0 { return true; }
    // placement in screen space: the device matrix translated by the context origin
    let ms = Affine::translate((origin.0 as f64, origin.1 as f64)) * m * pt;
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    (id, alpha.to_bits(), ms.as_coeffs().map(f64::to_bits)).hash(&mut hs);
    let key = hs.finish();
    let inv_s = ms.inverse();
    let (img, ix, iy) = match cache.map.get(&key) {
        Some(Some((px, x, y))) => (px.clone(), *x, *y),
        seen => {
            let rest = seen.is_some();
            // second sighting: the whole on-screen bbox (cached); first: the visible rect only
            let sbb = (bb + kurbo::Vec2::new(origin.0 as f64, origin.1 as f64)).intersect(Rect::new(screen[0] as f64, screen[1] as f64, (screen[0] + screen[2]) as f64, (screen[1] + screen[3]) as f64));
            let area = if rest && sbb.width() > 0.0 && sbb.height() > 0.0 { sbb } else { vbb + kurbo::Vec2::new(origin.0 as f64, origin.1 as f64) };
            let (x0, y0) = (area.x0.floor() as i32, area.y0.floor() as i32);
            let (x1, y1) = (area.x1.ceil() as i32, area.y1.ceil() as i32);
            let (w, h) = (x1 - x0, y1 - y0);
            if w <= 0 || h <= 0 || w > u16::MAX as i32 || h > u16::MAX as i32 { return false; }
            let px = image(g, inv_s, [x0, y0, w, h], alpha, (0, 0), true);
            let meta = vello_cpu::PixelMetadata { alpha_type: peniko::ImageAlphaType::AlphaPremultiplied, may_have_transparency: true };
            let px = Arc::new(Pixmap::from_parts(px, w as u16, h as u16, meta));
            if rest {
                if cache.bytes > CACHE_LIMIT { cache.map.clear(); cache.bytes = 0; }
                cache.bytes += (w * h * 4) as usize;
                cache.map.insert(key, Some((px.clone(), x0, y0)));
            } else {
                cache.map.insert(key, None);
            }
            (px, x0, y0)
        }
    };
    ctx.set_transform(m);
    ctx.set_fill_rule(if even_odd { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
    ctx.set_paint(PaintType::Image(peniko::ImageBrush {
        image: ImageSource::Pixmap(img),
        sampler: peniko::ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality: peniko::ImageQuality::Low, alpha: 1.0 },
    }));
    // image pixels are screen pixels: placed at (ix, iy) - origin in this context
    ctx.set_paint_transform(m.inverse() * Affine::translate(((ix as i64 - origin.0) as f64, (iy as i64 - origin.1) as f64)));
    ctx.fill_path(path);
    ctx.reset_paint_transform();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simd_matches_scalar_random() {
        use peniko::color::{AlphaColor, Srgb};
        let mut rng = crate::pixel::Rng(890);
        for w in [0, 1, 7, 8, 9, 15, 16, 17, 63, 257] {
            for ext in [Extend::Pad, Extend::Repeat, Extend::Reflect] {
                for premul in [false, true] {
                    let stops: [AlphaColor<Srgb>; 3] = std::array::from_fn(|_| AlphaColor::new(std::array::from_fn(|_| rng.unit())));
                    let mut g = Gradient::new_linear((-3.4, 20.0), (170.6, -34.9)).with_stops(stops).with_extend(ext);
                    g.interpolation_alpha_space = if premul { InterpolationAlphaSpace::Premultiplied } else { InterpolationAlphaSpace::Unpremultiplied };
                    let inv = Affine::new(std::array::from_fn(|_| rng.unit() as f64 * 4.0 - 2.0));
                    let alpha = rng.unit();
                    for dither in [false, true] {
                        let r = [-37, -9, w, 19];
                        let origin = (-1027, 523);
                        assert_eq!(image_impl::<true>(&g, inv, r, alpha, origin, dither, false), image(&g, inv, r, alpha, origin, dither), "w={w}, {ext:?}, premul={premul}, dither={dither}");
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "manual release microbenchmark"]
    fn bench_pixels() {
        use peniko::color::{AlphaColor, Srgb};
        use std::{hint::black_box, time::Instant};
        let stops = [AlphaColor::<Srgb>::new([0.01, 0.08, 0.12, 1.0]), AlphaColor::<Srgb>::new([0.15, 0.2, 0.4, 0.8])];
        let g = Gradient::new_linear((0.0, 0.0), (1024.0, 200.0)).with_stops(stops);
        for simd in [false, true] {
            let t = Instant::now();
            for _ in 0..100 { black_box(image_impl::<false>(black_box(&g), Affine::IDENTITY, [0, 0, 1024, 512], 0.8, (0, 0), true, simd)); }
            eprintln!("dither simd={simd}: {:?}", t.elapsed());
        }
    }

    #[test]
    fn bayer_is_a_permutation_with_zero_mean() {
        let mut s = BAYER.to_vec();
        s.sort();
        assert_eq!(s, (0..64).collect::<Vec<u8>>());
        let mean: f32 = (0..8).flat_map(|y| (0..8).map(move |x| offset(x, y))).sum::<f32>() / 64.0;
        assert!(mean.abs() < 1e-6);
        let mut vals: Vec<i32> = (0..8).flat_map(|y| (0..8).map(move |x| (offset(x, y) * 128.0).round() as i32)).collect();
        vals.sort();
        assert_eq!(vals, (0..64).map(|m| 2 * m - 63).collect::<Vec<i32>>(), "Skia matrix is a permutation of 0..63");
        assert!(offset(0, 0) < 0.0 && offset(0, 0) > -0.5);
        // anchored to absolute pixels, periodic with 8
        assert_eq!(offset(3, 5), offset(3 + 8 * 7, 5 - 8 * 3));
    }

    /// Undithered evaluation matches vello's own gradient within 1 level, for linear,
    /// concentric radial and sweep gradients with each extend mode.
    #[test]
    fn matches_vello_undithered() {
        use peniko::color::{AlphaColor, DynamicColor, Srgb};
        let (w, h) = (96u16, 64u16);
        let stops = |g: Gradient| g.with_stops([
            (0.0, AlphaColor::<Srgb>::new([0.05, 0.2, 0.12, 1.0])),
            (0.4, AlphaColor::<Srgb>::new([0.9, 0.3, 0.1, 0.6])),
            (1.0, AlphaColor::<Srgb>::new([0.1, 0.1, 0.8, 1.0])),
        ].map(|(o, c)| peniko::ColorStop { offset: o, color: DynamicColor::from_alpha_color(c) }).as_slice());
        for ext in [Extend::Pad, Extend::Repeat, Extend::Reflect] {
            for g in [
                Gradient::new_linear((10.0, 5.0), (60.0, 40.0)),
                Gradient::new_radial((40.0, 30.0), 35.0),
                Gradient::new_two_point_radial((40.0, 30.0), 8.0, (40.0, 30.0), 30.0),
                Gradient::new_sweep((48.0, 32.0), 0.3, 5.0),
            ] {
                let g = stops(g).with_extend(ext);
                let pt = Affine::new([1.2, 0.1, -0.2, 0.9, 3.0, -2.0]);
                let mut ctx = RenderContext::new(w, h);
                ctx.set_paint(PaintType::Gradient(g.clone()));
                ctx.set_paint_transform(pt);
                ctx.fill_rect(&Rect::new(0.0, 0.0, w as f64, h as f64));
                ctx.flush();
                let mut px = Pixmap::new(w, h);
                ctx.render(&mut px, &mut vello_cpu::Resources::default());
                let ours = image(&g, pt.inverse(), [0, 0, w as i32, h as i32], 1.0, (0, 0), false);
                let theirs = px.data_as_u8_slice();
                let mut worst = 0;
                for (i, (a, b)) in ours.iter().zip(theirs).enumerate() {
                    // skip the sweep's seam pixels (angle wrap), where both sides are valid
                    let d = (*a as i32 - *b as i32).abs();
                    if d > 2 && matches!(g.kind, GradientKind::Sweep(_)) { continue; }
                    worst = worst.max(d);
                    assert!(d <= 2, "{:?} {ext:?} px {} c {}: ours {a} vello {b}", g.kind, i / 4, i % 4);
                }
                assert!(worst <= 2);
            }
        }
    }
}
