//! Space warps: `kaleido` (N-fold mirror symmetry), node effects that resample the
//! subtree's offscreen buffer
//! (`vraster::effect_pixmap`). The CPU reference here and `gpu/warp.wgsl` evaluate the same
//! f32 expression trees: every multiply-add is an explicit fma on both sides (llvmpipe
//! fuses a * b + c), atan2 / sin / cos are the polynomials below (no libm vs. driver
//! transcendentals), and everything per frame (wedge angles, 1 / zoom, the center)
//! is computed once on the host into `WarpCfg`. Both are pure functions of the parameters
//! at the frame time: no state, so any frame renders alone (scrubbing, segments).
//!
//! Sampling is bilinear on premultiplied RGBA8 inside the sampling domain (`dom`, the
//! layer box): kaleido mirrors at its edges.
use crate::look::NPARAM;
use agentfx_ir::Effect;
use rayon::prelude::*;

/// Extra ink (screen px) of a warp: its output covers the frame from anywhere.
pub fn margin(frame: [f64; 2]) -> f64 {
    frame[0].hypot(frame[1]) + 64.0
}

/// Edge modes of `sample`.
pub const MIRROR: u32 = 0;
pub const WRAP: u32 = 1;

/// Kinds.
pub const KALEIDO: u32 = 0;

/// One warp at one frame, resolved to buffer px (host side, shared by CPU and GPU).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpCfg {
    pub kind: u32,
    /// center, buffer px
    pub c: [f32; 2],
    /// sampling domain x0 y0 x1 y1 (buffer px, inside the buffer, nonempty)
    pub dom: [i32; 4],
    pub amount: f32,
    // kaleido: start angle (rad), wedge, half wedge, 1 / wedge, mirror, 1 / zoom
    pub ang: f32,
    pub seg: f32,
    pub hseg: f32,
    pub invseg: f32,
    pub mirror: bool,
    pub invzoom: f32,
}

impl WarpCfg {
    /// Does it leave the buffer exactly as it is?
    pub fn identity(&self) -> bool {
        match self.kind {
            KALEIDO => self.amount <= 0.0,
            _ => false,
        }
    }
}

/// Resolve a warp effect for a buffer at `o` (screen x, y, w, h). `m` = the node's chain
/// (kurbo coefficients a b c d e f: node px -> screen px), `ink` = the subtree's screen
/// ink box (the domain when the effect has no `rect`). None: nothing to sample.
pub fn cfg(e: &Effect, p: &[f64; NPARAM], m: [f64; 6], ink: [f64; 4], o: [i32; 4]) -> Option<WarpCfg> {
    let (w, h) = (o[2], o[3]);
    let to_buf = |x: f64, y: f64| [m[0] * x + m[2] * y + m[4] - o[0] as f64, m[1] * x + m[3] * y + m[5] - o[1] as f64];
    let rot = m[1].atan2(m[0]);
    let rect = match e {
        Effect::Kaleido { rect, .. } => *rect,
        _ => return None,
    };
    let d = match rect {
        Some(r) => {
            let cs = [to_buf(r[0], r[1]), to_buf(r[0] + r[2], r[1]), to_buf(r[0], r[1] + r[3]), to_buf(r[0] + r[2], r[1] + r[3])];
            let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for c in cs {
                x0 = x0.min(c[0]);
                y0 = y0.min(c[1]);
                x1 = x1.max(c[0]);
                y1 = y1.max(c[1]);
            }
            [x0, y0, x1, y1]
        }
        None => [ink[0] - o[0] as f64, ink[1] - o[1] as f64, ink[0] + ink[2] - o[0] as f64, ink[1] + ink[3] - o[1] as f64],
    };
    let clampi = |v: f64, hi: i32| (v.round().clamp(0.0, hi as f64)) as i32;
    let dom = [clampi(d[0], w), clampi(d[1], h), clampi(d[2], w), clampi(d[3], h)];
    if dom[2] <= dom[0] || dom[3] <= dom[1] {
        return None;
    }
    let mut c = WarpCfg {
        kind: KALEIDO, c: [0.0; 2], dom, amount: p[0].clamp(0.0, 1.0) as f32, ang: 0.0, seg: 1.0, hseg: 0.5, invseg: 1.0, mirror: true, invzoom: 1.0,
    };
    match e {
        Effect::Kaleido { segments, mirror, .. } => {
            let n = (*segments).clamp(2, 64) as f64;
            let ctr = to_buf(p[3], p[4]);
            let seg = std::f64::consts::TAU / n;
            c.c = [ctr[0] as f32, ctr[1] as f32];
            c.ang = (p[1].to_radians() + rot).rem_euclid(std::f64::consts::TAU) as f32;
            c.seg = seg as f32;
            c.hseg = (seg * 0.5) as f32;
            c.invseg = (n / std::f64::consts::TAU) as f32;
            c.mirror = *mirror;
            c.invzoom = (1.0 / p[2].clamp(0.05, 20.0)) as f32;
        }
        _ => return None,
    }
    Some(c)
}

#[inline(always)]
fn fma(a: f32, b: f32, c: f32) -> f32 {
    a.mul_add(b, c)
}

/// atan2(y, x) in (-pi, pi]: Abramowitz & Stegun 4.4.49 (|error| < 2e-8 on [0, 1]) after
/// reduction to min / max.
#[inline]
pub fn atan2p(y: f32, x: f32) -> f32 {
    let ax = x.abs();
    let ay = y.abs();
    let mx = ax.max(ay);
    if mx == 0.0 {
        return 0.0;
    }
    let a = ax.min(ay) / mx;
    let s = a * a;
    let mut q = fma(s, 0.002_866_225_782_781_839_4, -0.016_165_737_062_692_642);
    q = fma(s, q, 0.042_909_614_741_802_216);
    q = fma(s, q, -0.075_289_636_850_357_06);
    q = fma(s, q, 0.106_562_636_792_659_76);
    q = fma(s, q, -0.142_088_994_383_811_95);
    q = fma(s, q, 0.199_935_510_754_585_27);
    q = fma(s, q, -0.333_331_465_721_130_37);
    let mut r = fma(a * s, q, a);
    if ay > ax {
        r = 1.570_796_370_506_286_6 - r;
    }
    if x < 0.0 {
        r = 3.141_592_741_012_573_2 - r;
    }
    if y < 0.0 {
        r = -r;
    }
    r
}

const PIO2_HI: f32 = 1.570_796_370_506_286_6;
const PIO2_LO: f32 = -4.371_138_828_673_793e-8;

/// (sin b, cos b): Cody-Waite reduction to [-pi/4, pi/4], Taylor polynomials.
#[inline]
pub fn sincosp(b: f32) -> (f32, f32) {
    let q = fma(b, 0.636_619_746_685_028_1, 0.5).floor();
    let r = fma(-q, PIO2_LO, fma(-q, PIO2_HI, b));
    let s2 = r * r;
    let sn = fma(r * s2, fma(s2, fma(s2, -1.984_127_011_382_952_3e-4, 8.333_333_767_950_535e-3), -0.166_666_671_633_720_4), r);
    let cs = fma(s2, fma(s2, fma(s2, fma(s2, 2.480_158_764_228_690_4e-5, -1.388_888_922_519_981_9e-3), 4.166_666_790_843_01e-2), -0.5), 1.0);
    match (q as i32) & 3 {
        0 => (sn, cs),
        1 => (cs, -sn),
        2 => (-sn, -cs),
        _ => (-cs, sn),
    }
}

#[inline(always)]
fn texel(src: &[u8], w: usize, dom: [i32; 4], mode: u32, ix: i32, iy: i32) -> [f32; 4] {
    let (nx, ny) = (dom[2] - dom[0], dom[3] - dom[1]);
    let (x, y) = match mode {
        MIRROR => {
            let refl = |i: i32, n: i32| {
                let m = i.rem_euclid(2 * n);
                if m >= n { 2 * n - 1 - m } else { m }
            };
            (dom[0] + refl(ix - dom[0], nx), dom[1] + refl(iy - dom[1], ny))
        }
        WRAP => (dom[0] + (ix - dom[0]).rem_euclid(nx), dom[1] + (iy - dom[1]).rem_euclid(ny)),
        _ => {
            if ix < dom[0] || iy < dom[1] || ix >= dom[2] || iy >= dom[3] {
                return [0.0; 4];
            }
            (ix, iy)
        }
    };
    let i = (y as usize * w + x as usize) * 4;
    [src[i] as f32, src[i + 1] as f32, src[i + 2] as f32, src[i + 3] as f32]
}

/// Bilinear sample at buffer position (x, y) (pixel i covers [i, i + 1)), premultiplied 0..255.
#[inline]
fn sample(src: &[u8], w: usize, dom: [i32; 4], mode: u32, x: f32, y: f32) -> [f32; 4] {
    let fx = x.clamp(-1e7, 1e7) - 0.5;
    let fy = y.clamp(-1e7, 1e7) - 0.5;
    let xf = fx.floor();
    let yf = fy.floor();
    let tx = fx - xf;
    let ty = fy - yf;
    let (ix, iy) = (xf as i32, yf as i32);
    let a = texel(src, w, dom, mode, ix, iy);
    let b = texel(src, w, dom, mode, ix + 1, iy);
    let c = texel(src, w, dom, mode, ix, iy + 1);
    let d = texel(src, w, dom, mode, ix + 1, iy + 1);
    std::array::from_fn(|k| {
        let top = fma(b[k] - a[k], tx, a[k]);
        let bot = fma(d[k] - c[k], tx, c[k]);
        fma(bot - top, ty, top)
    })
}

/// The warped pixel (x, y) of `src`, premultiplied 0..255.
#[inline]
pub fn pixel(c: &WarpCfg, src: &[u8], w: usize, x: usize, y: usize) -> [f32; 4] {
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    let qx = px - c.c[0];
    let qy = py - c.c[1];
    let r = fma(qx, qx, qy * qy).sqrt();
    let t1 = atan2p(qy, qx) - c.ang;
    let a0 = fma(-c.seg, (t1 * c.invseg).floor(), t1);
    let a1 = if c.mirror { c.hseg - (a0 - c.hseg).abs() } else { a0 };
    let (sn, cs) = sincosp(a1 + c.ang);
    let rs = r * c.invzoom;
    let sx = fma(rs, cs, c.c[0]);
    let sy = fma(rs, sn, c.c[1]);
    sample(src, w, c.dom, MIRROR, fma(c.amount, sx - px, px), fma(c.amount, sy - py, py))
}

/// Apply a resolved warp to a w x h premultiplied RGBA8 buffer in place.
pub fn apply(c: &WarpCfg, buf: &mut [u8], w: usize, h: usize) {
    if w == 0 || h == 0 || c.identity() {
        return;
    }
    let src = buf.to_vec();
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let v = pixel(c, &src, w, x, y);
            for k in 0..4 {
                row[x * 4 + k] = (v[k] + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn kaleido(segments: u32, mirror: bool, amount: f64) -> (Effect, [f64; NPARAM]) {
        let e = Effect::Kaleido { segments, mirror, center: [101.0, 77.0], angle: 17.0, zoom: 1.25, amount, rect: Some([0.0, 0.0, 403.0, 297.0]) };
        let mut p = [0.0; NPARAM];
        for (i, f) in e.look_fields().iter().enumerate() {
            p[i] = e.look_field(f);
        }
        (e, p)
    }

    /// The warps of the conformance tests (`gpu::fx` tests too).
    pub fn cases() -> Vec<(String, Effect, [f64; NPARAM])> {
        let mut v = vec![];
        for (n, m, a) in [(6, true, 1.0), (8, true, 0.55), (5, false, 1.0), (12, true, 1.0)] {
            let (e, p) = kaleido(n, m, a);
            v.push((format!("kaleido-{n}-{m}-{a}"), e, p));
        }
        v
    }

    pub const M: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

    pub fn run(e: &Effect, p: &[f64; NPARAM], src: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut b = src.to_vec();
        if let Some(c) = cfg(e, p, M, [0.0, 0.0, w as f64, h as f64], [0, 0, w as i32, h as i32]) {
            apply(&c, &mut b, w, h);
        }
        b
    }

    fn poster(w: usize, h: usize) -> Vec<u8> {
        crate::gpu::fx::tests::poster(w, h, false)
    }

    #[test]
    fn polynomials_match_libm() {
        let mut worst: (f32, f32) = (0.0, 0.0);
        for i in -2000..=2000 {
            for j in [-3.0f32, -1.0, -0.2, 0.0, 0.3, 1.0, 7.0] {
                let (y, x) = (i as f32 * 0.0137, j);
                worst.0 = worst.0.max((atan2p(y, x) - y.atan2(x)).abs());
            }
            let b = i as f32 * 0.011;
            let (s, c) = sincosp(b);
            worst.1 = worst.1.max((s - b.sin()).abs().max((c - b.cos()).abs()));
        }
        assert!(worst.0 < 1e-6 && worst.1 < 1e-6, "{worst:?}");
        assert_eq!(atan2p(0.0, 0.0), 0.0);
        assert!((atan2p(0.0, -1.0) - std::f32::consts::PI).abs() < 1e-7);
    }

    #[test]
    fn zero_amount_is_identity_and_output_is_premultiplied() {
        let (w, h) = (403, 297);
        let src = poster(w, h);
        let (e, p) = kaleido(6, true, 0.0);
        assert_eq!(run(&e, &p, &src, w, h), src);
        for (name, e, p) in cases() {
            let out = run(&e, &p, &src, w, h);
            assert!(out.chunks_exact(4).all(|q| q[..3].iter().all(|c| *c <= q[3])), "{name}: not premultiplied");
            assert_eq!(out, run(&e, &p, &src, w, h), "{name}: deterministic");
        }
    }

    #[test]
    fn kaleido_is_symmetric() {
        // a mirrored 6-fold kaleido around the buffer center: pixel and its mirror image across
        // the wedge's start line show the same color
        let (w, h) = (201, 201);
        let src = poster(w, h);
        let e = Effect::Kaleido { segments: 6, mirror: true, center: [100.5, 100.5], angle: 0.0, zoom: 1.0, amount: 1.0, rect: None };
        let mut p = [0.0; NPARAM];
        for (i, f) in e.look_fields().iter().enumerate() {
            p[i] = e.look_field(f);
        }
        let out = run(&e, &p, &src, w, h);
        // the wedge start is the +x axis: (x, y) and (x, -y) about the center are mirror images
        let mut worst = 0u8;
        for y in 0..100 {
            for x in 0..w {
                let a = &out[(y * w + x) * 4..][..4];
                let b = &out[((200 - y) * w + x) * 4..][..4];
                worst = worst.max(a.iter().zip(b).map(|(p, q)| p.abs_diff(*q)).max().unwrap());
            }
        }
        assert!(worst <= 1, "mirror symmetry off by {worst}");
        assert_ne!(out, src);
    }

    #[test]
    #[ignore = "manual 1080p cost measurement"]
    fn benchmark_1080p() {
        let (w, h) = (1920, 1080);
        let src = crate::gpu::fx::tests::poster(w, h, true);
        for (name, e, p) in cases() {
            let t = std::time::Instant::now();
            for _ in 0..3 {
                std::hint::black_box(run(&e, &p, &src, w, h));
            }
            println!("{name}: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0 / 3.0);
        }
    }
}
