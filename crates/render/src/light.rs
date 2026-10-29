//! Light on flat design (X6): soft shadows from a spherical light by the subtree's signed
//! distance field, a soft light sweep, and god rays through the subtree's holes. Node
//! effects on premultiplied RGBA8 offscreen buffers, dispatched by `look::apply`.
//!
//! Model: the page is the plane z = 0; a shadowing layer is a flat sheet at `height` above
//! it; the light is a sphere of radius R at (lx, ly, lz) in scene px. A receiver point p on
//! the page looks at the light through the sheet plane at
//!   q = L + (p - L) * (lz - h) / lz
//! and the light's disc seen from p covers a disc of radius r = R * h / lz around q in that
//! plane. The occluded fraction of that disc is estimated from the sheet's signed distance
//! field at q (a one-step cone trace: exact for a straight edge, smooth elsewhere), so the
//! penumbra widens as the sheet rises or the light grows, and a moving light sweeps it.
use crate::look::{FxCtx, NPARAM};
use rayon::prelude::*;

/// Largest distance from (lx, ly) to a corner of the frame.
fn far_corner(lx: f64, ly: f64, frame: [f64; 2]) -> f64 {
    [(0.0, 0.0), (frame[0], 0.0), (0.0, frame[1]), (frame[0], frame[1])]
        .iter()
        .map(|(x, y)| ((x - lx).powi(2) + (y - ly).powi(2)).sqrt())
        .fold(0.0, f64::max)
}

/// Shadow parameters `[lx, ly, lz, radius, height, strength]` -> (lz clamped above the
/// sheet, height, penumbra radius in the sheet plane).
pub(crate) fn shadow_geom(p: &[f64; NPARAM]) -> (f64, f64, f64) {
    let h = p[4].max(0.0);
    let lz = p[2].max(h + 1.0);
    (lz, h, p[3].max(0.0) * h / lz)
}

/// How far (screen px) a shadow reaches beyond its caster: the largest offset of q -> p
/// over the frame plus the penumbra on the page.
pub fn shadow_reach(p: &[f64; NPARAM], frame: [f64; 2]) -> f64 {
    let (lz, h, r) = shadow_geom(p);
    let k = h / (lz - h);
    // p - q = (q - L) * h / (lz - h); |q - L| <= the far corner (casters on screen)
    k * far_corner(p[0], p[1], frame) + r * lz / (lz - h) + 2.0
}

/// How far rays stream past their holes: samples run `length` of the way to the light.
pub fn rays_reach(p: &[f64; NPARAM], frame: [f64; 2]) -> f64 {
    let len = p[2].clamp(0.0, 0.95);
    far_corner(p[0], p[1], frame) * len / (1.0 - len) + 2.0
}

// ------------------------------------------------------------------ distance field

/// 1D squared Euclidean distance transform (Felzenszwalb & Huttenlocher), in place.
/// Entries >= 1e19 are "no feature" (lines without any stay at infinity).
fn edt_1d(f: &mut [f32], v: &mut [usize], z: &mut [f64], d: &mut [f32]) {
    let n = f.len();
    let mut k: isize = -1;
    for q in 0..n {
        if f[q] >= 1e19 {
            continue;
        }
        let fq = f[q] as f64 + (q * q) as f64;
        loop {
            if k < 0 {
                k = 0;
                v[0] = q;
                z[0] = f64::NEG_INFINITY;
                z[1] = f64::INFINITY;
                break;
            }
            let vk = v[k as usize];
            let s = (fq - (f[vk] as f64 + (vk * vk) as f64)) / (2.0 * (q as f64 - vk as f64));
            if s <= z[k as usize] {
                k -= 1;
                continue;
            }
            k += 1;
            v[k as usize] = q;
            z[k as usize] = s;
            z[k as usize + 1] = f64::INFINITY;
            break;
        }
    }
    if k < 0 {
        return;
    }
    let mut j = 0usize;
    for q in 0..n {
        while z[j + 1] < q as f64 {
            j += 1;
        }
        let dq = q as f32 - v[j] as f32;
        d[q] = dq * dq + f[v[j]];
    }
    f.copy_from_slice(d);
}

/// Squared distance (px^2) from every pixel to the nearest pixel where `feature` is true.
fn edt(feature: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e20;
    let mut g: Vec<f32> = feature.iter().map(|&b| if b { 0.0 } else { INF }).collect();
    // columns (via a transposed copy), then rows
    let mut t = vec![INF; w * h];
    t.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for y in 0..h {
            col[y] = g[y * w + x];
        }
        let (mut v, mut z, mut d) = (vec![0usize; h], vec![0f64; h + 1], vec![0f32; h]);
        edt_1d(col, &mut v, &mut z, &mut d);
    });
    g.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            row[x] = t[x * h + y];
        }
        let (mut v, mut z, mut d) = (vec![0usize; w], vec![0f64; w + 1], vec![0f32; w]);
        edt_1d(row, &mut v, &mut z, &mut d);
    });
    g
}

/// Signed distance field (px, negative inside) of a coverage plane (0..1): exact
/// Euclidean distances between pixel centres, half a pixel to the edge, and partial
/// coverage as a sub-pixel offset at the edge (as SDFT edge texels).
pub fn sdf(cov: &[f32], w: usize, h: usize) -> Vec<f32> {
    let inside: Vec<bool> = cov.iter().map(|&a| a >= 0.5).collect();
    let outside: Vec<bool> = inside.iter().map(|b| !b).collect();
    let d_in = edt(&inside, w, h); // for outside pixels: distance to the shape
    let d_out = edt(&outside, w, h); // for inside pixels: distance to the outside
    cov.par_iter()
        .zip(d_in.par_iter().zip(d_out.par_iter()))
        .map(|(&a, (&di, &do_))| {
            if a > 0.02 && a < 0.98 {
                0.5 - a
            } else if a >= 0.5 {
                -(do_.sqrt() - 0.5)
            } else {
                di.sqrt() - 0.5
            }
        })
        .collect()
}

/// Bilinear SDF sample in pixel-centre coordinates; outside the field: far away.
#[inline(always)]
fn sample_sdf(f: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let fx = x - 0.5;
    let fy = y - 0.5;
    if fx < 0.0 || fy < 0.0 || fx > (w - 1) as f32 || fy > (h - 1) as f32 {
        return 1e6;
    }
    let x0 = fx.floor();
    let y0 = fy.floor();
    let (tx, ty) = (fx - x0, fy - y0);
    let (ix, iy) = (x0 as usize, y0 as usize);
    let (ix1, iy1) = ((ix + 1).min(w - 1), (iy + 1).min(h - 1));
    let a = f[iy * w + ix];
    let b = f[iy * w + ix1];
    let c = f[iy1 * w + ix];
    let d = f[iy1 * w + ix1];
    (a + (b - a) * tx) + ((c + (d - c) * tx) - (a + (b - a) * tx)) * ty
}

/// Alpha plane averaged over `ds` x `ds` blocks.
pub(crate) fn alpha_plane(buf: &[u8], w: usize, h: usize, ds: usize) -> (Vec<f32>, usize, usize) {
    let (sw, sh) = (w.div_ceil(ds), h.div_ceil(ds));
    let mut out = vec![0f32; sw * sh];
    out.par_chunks_mut(sw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut sum = 0u32;
            for yy in y * ds..((y + 1) * ds).min(h) {
                for xx in x * ds..((x + 1) * ds).min(w) {
                    sum += buf[(yy * w + xx) * 4 + 3] as u32;
                }
            }
            *o = sum as f32 / (255 * ds * ds) as f32;
        }
    });
    (out, sw, sh)
}

// ------------------------------------------------------------------ shadow

/// Soft shadow behind the subtree (`p` = [lx, ly, lz, radius, height, strength]).
pub fn shadow(buf: &mut [u8], w: usize, h: usize, cx: &FxCtx, p: &[f64; NPARAM], color: [f64; 4]) {
    let (lz, ht, r) = shadow_geom(p);
    let strength = (color[3] * p[5]).clamp(0.0, 1.0) as f32;
    if strength <= 0.0 || ht <= 0.0 {
        return;
    }
    // light in buffer px
    let (lx, ly) = ((p[0] - cx.ox as f64) as f32, (p[1] - cx.oy as f64) as f32);
    let k = ((lz - ht) / lz) as f32;
    // a coarser field for wide penumbras (the ramp hides the resolution)
    let ds = ((r / 3.0).floor() as usize).clamp(1, 4);
    let field_p = crate::prof::scope("fx.shadow.sdf");
    let (cov, sw, sh) = alpha_plane(buf, w, h, ds);
    let field = sdf(&cov, sw, sh);
    drop(field_p);
    let rr = (r as f32).max(0.6);
    let inv_ds = 1.0 / ds as f32;
    let col = [color[0] as f32, color[1] as f32, color[2] as f32];
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let py = y as f32 + 0.5;
        for x in 0..w {
            let px = x as f32 + 0.5;
            let qx = lx + (px - lx) * k;
            let qy = ly + (py - ly) * k;
            let d = sample_sdf(&field, sw, sh, qx * inv_ds, qy * inv_ds) * ds as f32;
            if d >= rr {
                continue;
            }
            // occluded fraction of the light's disc
            let t = ((rr - d) / (2.0 * rr)).clamp(0.0, 1.0);
            let occ = t * t * (3.0 - 2.0 * t) * strength;
            let o = &mut row[x * 4..x * 4 + 4];
            // subtree over the shadow
            let ia = 1.0 - o[3] as f32 / 255.0;
            if ia <= 0.0 {
                continue;
            }
            let sa = occ * ia;
            for c in 0..3 {
                o[c] = (o[c] as f32 + col[c] * sa * 255.0 + 0.5).min(255.0) as u8;
            }
            o[3] = (o[3] as f32 + sa * 255.0 + 0.5).min(255.0) as u8;
        }
    });
}

// ------------------------------------------------------------------ sweep

/// Bounding box (buffer px) of pixels with alpha >= `min`.
fn alpha_bbox(buf: &[u8], w: usize, h: usize, min: u8) -> Option<[f32; 4]> {
    let rows: Vec<Option<(usize, usize)>> = buf
        .par_chunks(w * 4)
        .map(|row| {
            let first = row.chunks_exact(4).position(|p| p[3] >= min)?;
            let last = row.chunks_exact(4).rposition(|p| p[3] >= min)?;
            Some((first, last))
        })
        .collect();
    let y0 = rows.iter().position(|r| r.is_some())?;
    let y1 = rows.iter().rposition(|r| r.is_some())?;
    let x0 = rows.iter().flatten().map(|r| r.0).min()?;
    let x1 = rows.iter().flatten().map(|r| r.1).max()?;
    let _ = h;
    Some([x0 as f32, y0 as f32, x1 as f32 + 1.0, y1 as f32 + 1.0])
}

/// A soft band of light (`p` = [pos, angle, width, strength]) screened onto the subtree.
pub fn sweep(buf: &mut [u8], w: usize, h: usize, p: &[f64; NPARAM], color: [f64; 4]) {
    let Some(bb) = alpha_bbox(buf, w, h, 8) else { return };
    let (pos, ang, width, strength) = (p[0] as f32, (p[1] as f32).to_radians(), p[2].max(0.5) as f32, (p[3] * color[3]).max(0.0) as f32);
    if strength <= 0.0 {
        return;
    }
    let (ux, uy) = (ang.cos(), ang.sin());
    let proj = [(bb[0], bb[1]), (bb[2], bb[1]), (bb[0], bb[3]), (bb[2], bb[3])].map(|(x, y)| x * ux + y * uy);
    let smin = proj.iter().cloned().fold(f32::INFINITY, f32::min);
    let smax = proj.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    // pos 0: the band is fully before the ink, 1: fully past it
    let sc = smin - 2.0 * width + pos * (smax - smin + 4.0 * width);
    let tint = [color[0] as f32, color[1] as f32, color[2] as f32];
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let py = y as f32 + 0.5;
        for x in 0..w {
            let o = &mut row[x * 4..x * 4 + 4];
            if o[3] == 0 {
                continue;
            }
            let s = (x as f32 + 0.5) * ux + py * uy;
            let d = (s - sc) / width;
            if d.abs() > 3.5 {
                continue;
            }
            let k = strength * (-d * d).exp();
            let a = o[3] as f32;
            // screen in premultiplied form: c + k*tint*a - c*k*tint
            for c in 0..3 {
                let v = o[c] as f32;
                o[c] = (v + k * tint[c] * (a - v) + 0.5).min(a) as u8;
            }
        }
    });
}

// ------------------------------------------------------------------ rays

/// AGENTFX_RAYS=v1: march every sample (reference); default skips samples whose bilinear
/// taps are all zero (`march_rays`, same result).
fn rays_v1() -> bool {
    static V1: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V1.get_or_init(|| agentfx_ir::env::var("AGENTFX_RAYS").is_ok_and(|v| v == "v1"))
}

/// Reference ray march: every pixel of the half-resolution plane averages `RAY_SAMPLES`
/// decaying mask samples on its way to the light.
#[allow(clippy::too_many_arguments)]
fn march_rays_ref(mask: &[f32], sw: usize, sh: usize, lx: f32, ly: f32, len: f32, decay: f32, norm: f32, cx: &FxCtx, lum: &mut [f32]) {
    lum.par_chunks_mut(sw).enumerate().for_each(|(y, row)| {
        let py = y as f32 + 0.5;
        for (x, o) in row.iter_mut().enumerate() {
            let px = x as f32 + 0.5;
            let (dx, dy) = ((lx - px) * len / RAY_SAMPLES as f32, (ly - py) * len / RAY_SAMPLES as f32);
            // a seeded sub-step offset per pixel: long rays show fine noise instead of
            // sample banding (blurred away below)
            let j = crate::look::unit(crate::look::hash(x as i32 + cx.ox, y as i32 + cx.oy, 0x5EED));
            let (mut sx, mut sy) = (px + dx * j, py + dy * j);
            let (mut sum, mut wgt) = (0f32, 1f32);
            for _ in 0..RAY_SAMPLES {
                sum += crate::look::sample_plane(mask, sw, sh, sx, sy) * wgt;
                wgt *= decay;
                sx += dx;
                sy += dy;
            }
            *o = sum / norm;
        }
    });
}

/// Block size (mask px) of the occupancy grid of `march_rays`.
const RAY_BLOCK: usize = 8;

/// The ray march of `rays` with empty space skipped. A sample whose four bilinear taps are
/// zero adds exactly 0 to the sum, so it is not evaluated: a block grid marks blocks whose
/// pixels (plus one px right and below, the bilinear footprint) hold any non-zero mask, and
/// a ray whose whole path crosses only empty blocks (summed-area test of its bounding box)
/// is 0. Sample positions and weights advance exactly as in the full march, so every sum
/// is bitwise the reference.
#[allow(clippy::too_many_arguments)]
fn march_rays(mask: &[f32], sw: usize, sh: usize, lx: f32, ly: f32, len: f32, decay: f32, norm: f32, cx: &FxCtx, lum: &mut [f32]) {
    let b = RAY_BLOCK;
    let (bw, bh) = (sw.div_ceil(b), sh.div_ceil(b));
    let mut occ = vec![false; bw * bh];
    for y in 0..sh {
        for x in 0..sw {
            if mask[y * sw + x] != 0.0 {
                // pixel (x, y) is a tap of samples with ix in {x-1, x}, iy in {y-1, y}
                for (xx, yy) in [(x, y), (x.wrapping_sub(1), y), (x, y.wrapping_sub(1)), (x.wrapping_sub(1), y.wrapping_sub(1))] {
                    if xx < sw && yy < sh {
                        occ[(yy / b) * bw + xx / b] = true;
                    }
                }
            }
        }
    }
    // summed-area table over blocks
    let mut sat = vec![0u32; (bw + 1) * (bh + 1)];
    for y in 0..bh {
        let mut run = 0u32;
        for x in 0..bw {
            run += occ[y * bw + x] as u32;
            sat[(y + 1) * (bw + 1) + x + 1] = sat[y * (bw + 1) + x + 1] + run;
        }
    }
    let any = |x0: f32, y0: f32, x1: f32, y1: f32| -> bool {
        // blocks touched by sample index ranges [floor(x - 0.5), floor(x - 0.5) + 1]; pad 1 px
        let bx0 = (((x0 - 1.5).floor().max(0.0)) as usize / b).min(bw);
        let by0 = (((y0 - 1.5).floor().max(0.0)) as usize / b).min(bh);
        let bx1 = ((((x1 + 1.5).floor().max(-1.0) + 1.0) as usize).div_ceil(b)).min(bw);
        let by1 = ((((y1 + 1.5).floor().max(-1.0) + 1.0) as usize).div_ceil(b)).min(bh);
        if bx1 <= bx0 || by1 <= by0 {
            return false;
        }
        sat[by1 * (bw + 1) + bx1] + sat[by0 * (bw + 1) + bx0] > sat[by0 * (bw + 1) + bx1] + sat[by1 * (bw + 1) + bx0]
    };
    lum.par_chunks_mut(sw).enumerate().for_each(|(y, row)| {
        let py = y as f32 + 0.5;
        for (x, o) in row.iter_mut().enumerate() {
            let px = x as f32 + 0.5;
            let (dx, dy) = ((lx - px) * len / RAY_SAMPLES as f32, (ly - py) * len / RAY_SAMPLES as f32);
            let j = crate::look::unit(crate::look::hash(x as i32 + cx.ox, y as i32 + cx.oy, 0x5EED));
            let (mut sx, mut sy) = (px + dx * j, py + dy * j);
            // the path's extent (the incremental positions stay within a few ulps of it)
            let (ex, ey) = (sx + dx * RAY_SAMPLES as f32, sy + dy * RAY_SAMPLES as f32);
            if !any(sx.min(ex), sy.min(ey), sx.max(ex), sy.max(ey)) {
                *o = 0.0 / norm;
                continue;
            }
            let (mut sum, mut wgt) = (0f32, 1f32);
            for _ in 0..RAY_SAMPLES {
                let (ix, iy) = ((sx - 0.5).floor(), (sy - 0.5).floor());
                // all four taps outside the plane or in an empty block: the sample is 0
                let live = ix >= -1.0 && iy >= -1.0 && ix < sw as f32 && iy < sh as f32 && (ix < 0.0 || iy < 0.0 || occ[(iy as usize / b) * bw + ix as usize / b]);
                if live {
                    sum += crate::look::sample_plane(mask, sw, sh, sx, sy) * wgt;
                }
                wgt *= decay;
                sx += dx;
                sy += dy;
            }
            *o = sum / norm;
        }
    });
}

const RAY_SAMPLES: usize = 96;

/// God rays (`p` = [lx, ly, length, strength]): light behind the subtree passes its holes
/// (transparent pixels inside its ink box) and is smeared away from the light. Computed at
/// half resolution (rays are soft), added over the subtree.
pub fn rays(buf: &mut [u8], w: usize, h: usize, cx: &FxCtx, p: &[f64; NPARAM], color: [f64; 4]) {
    let strength = (p[3] * color[3]).max(0.0) as f32;
    let len = p[2].clamp(0.0, 0.95) as f32;
    if strength <= 0.0 || len <= 0.0 {
        return;
    }
    let Some(bb) = alpha_bbox(buf, w, h, 128) else { return };
    // hole mask at half resolution
    let ds = 2usize;
    let (cov, sw, sh) = alpha_plane(buf, w, h, ds);
    let hb = [bb[0] / ds as f32, bb[1] / ds as f32, bb[2] / ds as f32, bb[3] / ds as f32];
    let mask: Vec<f32> = cov
        .par_iter()
        .enumerate()
        .map(|(i, &a)| {
            let (x, y) = ((i % sw) as f32 + 0.5, (i / sw) as f32 + 0.5);
            if x >= hb[0] && x <= hb[2] && y >= hb[1] && y <= hb[3] { 1.0 - a } else { 0.0 }
        })
        .collect();
    let (lx, ly) = (((p[0] - cx.ox as f64) / ds as f64) as f32, ((p[1] - cx.oy as f64) / ds as f64) as f32);
    let decay = 0.975f32;
    let norm: f32 = (0..RAY_SAMPLES).map(|i| decay.powi(i as i32)).sum();
    let mut lum = vec![0f32; sw * sh];
    {
        let _p = crate::prof::scope("fx.rays.march");
        if rays_v1() {
            march_rays_ref(&mask, sw, sh, lx, ly, len, decay, norm, cx, &mut lum);
        } else {
            march_rays(&mask, sw, sh, lx, ly, len, decay, norm, cx, &mut lum);
        }
    }
    // soften the jitter (1 px at half resolution), then a soft exposure curve
    let mut tmp: Vec<u8> = lum.iter().flat_map(|&v| { let b = (v.min(1.0) * 255.0 + 0.5) as u8; [b, b, b, b] }).collect();
    crate::effects::blur(&mut tmp, sw, sh, 1.2);
    for (o, p) in lum.iter_mut().zip(tmp.chunks_exact(4)) {
        *o = 1.0 - (-1.8 * p[3] as f32 / 255.0).exp();
    }
    let tint = [color[0] as f32, color[1] as f32, color[2] as f32];
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let v = crate::look::sample_plane(&lum, sw, sh, (x as f32 + 0.5) / ds as f32, (y as f32 + 0.5) / ds as f32);
            let k = (v * strength).min(1.0);
            if k <= 0.0 {
                continue;
            }
            let o = &mut row[x * 4..x * 4 + 4];
            // additive light: alpha grows toward opaque, color adds
            let a = o[3] as f32 / 255.0;
            let na = (a + k * (1.0 - a)).min(1.0);
            for c in 0..3 {
                o[c] = ((o[c] as f32 / 255.0 + k * tint[c]).min(na) * 255.0 + 0.5) as u8;
            }
            o[3] = (na * 255.0 + 0.5) as u8;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::FxCtx;

    fn cx() -> FxCtx {
        FxCtx { ox: 0, oy: 0, ax: 0, ay: 0, s: 1.0, frame: 0 }
    }

    #[test]
    fn sdf_of_a_disc() {
        let (w, h) = (101, 101);
        let cov: Vec<f32> = (0..w * h).map(|i| {
            let (x, y) = ((i % w) as f32 - 50.0, (i / w) as f32 - 50.0);
            if (x * x + y * y).sqrt() <= 20.0 { 1.0 } else { 0.0 }
        }).collect();
        let f = sdf(&cov, w, h);
        let at = |x: usize, y: usize| f[y * w + x];
        assert!((at(50, 50) + 20.0).abs() < 1.5, "centre {}", at(50, 50));
        assert!((at(90, 50) - 19.5).abs() < 1.5, "outside {}", at(90, 50));
        assert!((at(0, 0) - (50f32 * 2f32.sqrt() - 20.0)).abs() < 1.5, "corner {}", at(0, 0));
    }

    #[test]
    fn edt_brute_force() {
        let (w, h) = (23, 17);
        let feat: Vec<bool> = (0..w * h).map(|i| (i * 7919) % 13 == 0).collect();
        let d = edt(&feat, w, h);
        for y in 0..h {
            for x in 0..w {
                let mut best = f32::INFINITY;
                for yy in 0..h {
                    for xx in 0..w {
                        if feat[yy * w + xx] {
                            best = best.min(((x as f32 - xx as f32).powi(2) + (y as f32 - yy as f32).powi(2)) as f32);
                        }
                    }
                }
                assert_eq!(d[y * w + x], best, "{x},{y}");
            }
        }
    }

    fn square(w: usize, h: usize, r: [usize; 4]) -> Vec<u8> {
        let mut b = vec![0u8; w * h * 4];
        for y in r[1]..r[3] {
            for x in r[0]..r[2] {
                b[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        b
    }

    #[test]
    fn shadow_falls_away_from_the_light_and_softens_with_height() {
        let (w, h) = (200, 120);
        let run = |lx: f64, height: f64| {
            let mut b = square(w, h, [80, 40, 120, 80]);
            let p = [lx, 60.0, 400.0, 40.0, height, 1.0, 0.0, 0.0];
            shadow(&mut b, w, h, &cx(), &p, [0.0, 0.0, 0.0, 0.8]);
            b
        };
        let a = |b: &Vec<u8>, x: usize, y: usize| b[(y * w + x) * 4 + 3];
        // light on the left: shadow on the right of the square, not on the left
        let b = run(-200.0, 30.0);
        assert!(a(&b, 125, 60) > 100, "right side shadowed: {}", a(&b, 125, 60));
        assert!(a(&b, 74, 60) < 10, "left side lit: {}", a(&b, 74, 60));
        // moving the light moves the shadow
        let b2 = run(400.0, 30.0);
        assert!(a(&b2, 74, 60) > 100 && a(&b2, 125, 60) < 10);
        // a higher sheet: wider penumbra (the shadow edge is softer)
        let lo = run(-200.0, 10.0);
        let hi = run(-200.0, 60.0);
        let ramp = |b: &Vec<u8>| (120..200).filter(|&x| (5..200).contains(&a(b, x, 60))).count();
        assert!(ramp(&hi) > ramp(&lo), "{} {}", ramp(&hi), ramp(&lo));
        // the caster itself is untouched (drawn over its shadow)
        assert_eq!(&b[(60 * w + 100) * 4..(60 * w + 100) * 4 + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn rays_stream_through_holes_away_from_the_light() {
        let (w, h) = (240, 120);
        // a plate with a hole
        let mut b = square(w, h, [40, 20, 120, 100]);
        for y in 50..70 {
            for x in 70..90 {
                b[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
        }
        let p = [0.0, 60.0, 0.8, 1.0, 0.0, 0.0, 0.0, 0.0];
        rays(&mut b, w, h, &cx(), &p, [1.0, 0.9, 0.6, 1.0]);
        let a = |x: usize, y: usize| b[(y * w + x) * 4 + 3];
        assert!(a(80, 60) > 60, "hole lit {}", a(80, 60));
        assert!(a(170, 60) > 20, "ray past the plate {}", a(170, 60));
        assert!(a(170, 10) < 5, "no ray off-axis {}", a(170, 10));
        assert!(a(20, 60) < 5, "no ray toward the light {}", a(20, 60));
    }

    #[test]
    fn skipping_ray_march_is_bitwise_the_reference() {
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        for (case, &(sw, sh)) in [(64usize, 40usize), (97, 61), (300, 170), (17, 9)].iter().enumerate() {
            // sparse holes, some on the borders, partial coverage values
            let mut mask = vec![0f32; sw * sh];
            for _ in 0..(sw * sh / 60).max(3) {
                let (x, y) = ((rnd() * sw as f32) as usize, (rnd() * sh as f32) as usize);
                mask[y.min(sh - 1) * sw + x.min(sw - 1)] = rnd();
            }
            mask[0] = 0.7;
            mask[sw * sh - 1] = 1.0;
            mask[(sh / 2) * sw] = 0.5;
            for (lx, ly, len) in [(sw as f32 / 2.0, sh as f32 / 2.0, 0.75), (-40.0, 10.0, 0.95), (sw as f32 + 30.0, sh as f32 + 5.0, 0.4), (3.3, -20.0, 0.8)] {
                let cx = FxCtx { ox: case as i32 * 7, oy: 3, ax: 0, ay: 0, s: 1.0, frame: 0 };
                let decay = 0.975f32;
                let norm: f32 = (0..RAY_SAMPLES).map(|i| decay.powi(i as i32)).sum();
                let (mut a, mut b) = (vec![0f32; sw * sh], vec![0f32; sw * sh]);
                march_rays_ref(&mask, sw, sh, lx, ly, len, decay, norm, &cx, &mut a);
                march_rays(&mask, sw, sh, lx, ly, len, decay, norm, &cx, &mut b);
                let (ab, bb): (Vec<u32>, Vec<u32>) = (a.iter().map(|v| v.to_bits()).collect(), b.iter().map(|v| v.to_bits()).collect());
                assert_eq!(ab, bb, "case {case} light {lx},{ly}");
                assert!(a.iter().any(|&v| v > 0.0));
            }
        }
    }

    #[test]
    fn sweep_moves_with_pos() {
        let (w, h) = (100, 20);
        let run = |pos: f64| {
            let mut b = vec![0u8; w * h * 4];
            b.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[40, 40, 40, 255]));
            sweep(&mut b, w, h, &[pos, 0.0, 8.0, 1.0, 0.0, 0.0, 0.0, 0.0], [1.0, 0.9, 0.7, 0.5]);
            (0..w).max_by_key(|&x| b[(10 * w + x) * 4]).unwrap()
        };
        assert!(run(0.3) < run(0.7));
    }
}
