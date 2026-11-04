//! Space warps: `kaleido` (N-fold mirror symmetry), `tunnel` (fly down a tube made of the
//! subtree) and the distort pack (`dir-blur`, `radial-blur` zoom / spin, `chromatic`,
//! `displace` by fractal noise or a map layer), node effects that resample the subtree's
//! offscreen buffer (`vraster::effect_pixmap`). The CPU reference here and `gpu/warp.wgsl` evaluate the same
//! f32 expression trees: every multiply-add is an explicit fma on both sides (llvmpipe
//! fuses a * b + c), atan2 / sin / cos are the polynomials below (no libm vs. driver
//! transcendentals), and everything per frame (sin / cos of the roll, 1 / amount, scales)
//! is computed once on the host into `WarpCfg`. Both are pure functions of the parameters
//! at the frame time: no state, so any frame renders alone (scrubbing, segments).
//!
//! Sampling is bilinear on premultiplied RGBA8 inside the sampling domain (`dom`, the
//! layer box): kaleido mirrors at its edges, the texture tunnel wraps around it (the angle
//! around the tube and the depth), the perspective tunnel reads transparent outside it.
//! The distort pack samples the whole buffer (the frame) and reads transparent outside it;
//! its blurs average up to `MAXTAP` samples per pixel with host-made tap tables (offsets,
//! rotations, spectral weights), so the per-pixel work is fma chains identical on both sides.
use crate::look::{NPARAM, TIME};
use agentfx_ir::Effect;
use rayon::prelude::*;

/// Extra ink (screen px) of a warp: its output covers the frame from anywhere.
pub fn margin(frame: [f64; 2]) -> f64 {
    frame[0].hypot(frame[1]) + 64.0
}

/// Edge modes of `sample`.
pub const MIRROR: u32 = 0;
pub const WRAP: u32 = 1;
pub const CLEAR: u32 = 2;

/// Kinds.
pub const KALEIDO: u32 = 0;
pub const TUNNEL_TEXTURE: u32 = 1;
pub const TUNNEL_PERSPECTIVE: u32 = 2;
pub const DIR_BLUR: u32 = 3;
pub const ZOOM_BLUR: u32 = 4;
pub const SPIN_BLUR: u32 = 5;
pub const CHROMATIC: u32 = 6;
pub const DISPLACE_NOISE: u32 = 7;
pub const DISPLACE_MAP: u32 = 8;

/// Most samples per pixel of a distort blur (directional, zoom, spin) and of the chromatic
/// fringe (odd counts there). Longer streaks space their samples further apart (bilinear).
pub const MAXTAP: usize = 64;

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
    // tunnel: roll cos / sin, focal px and its inverse, flat floor (amount ~ 0), cylinder
    // radius rho = 1 / amount, its center height 1 - rho, 1 - 2 rho, the gap 2 pi (rho - 1)
    pub rc: f32,
    pub rs: f32,
    pub f: f32,
    pub invf: f32,
    pub plane: bool,
    pub rho: f32,
    pub yc: f32,
    pub cc: f32,
    pub gap: f32,
    /// spiral rad per depth unit, camera travel (depth + speed t), fog
    pub twist: f32,
    pub dz: f32,
    pub fog: f32,
    /// fog color, premultiplied, 0..255
    pub fogc: [f32; 4],
    /// texture: px per floor unit (domain width / 2 pi), domain center x, domain bottom y
    pub k: f32,
    pub tcx: f32,
    pub ty1: f32,
    // distort pack: tap count, directional step (buffer px per tap index), noise origin
    // (buffer px), 1 / feature size, drift (noise units), octaves, seeds of the x / y fields,
    // displacement px (noise: per unit of fbm; map: per unit of (channel - alpha / 2))
    pub n: u32,
    pub step: [f32; 2],
    pub org: [f32; 2],
    pub inv: f32,
    pub drift: [f32; 2],
    pub oct: u32,
    pub seed: [u32; 2],
    pub disp: f32,
    /// taps: dir-blur [offset index, -, weight, -]; zoom [scale, -, weight, -]; spin [cos,
    /// sin, weight, -]; chromatic [scale, red weight, green weight, blue weight]
    pub tab: [[f32; 4]; MAXTAP],
}

impl WarpCfg {
    fn base(dom: [i32; 4]) -> WarpCfg {
        WarpCfg {
            kind: KALEIDO, c: [0.0; 2], dom, amount: 0.0, ang: 0.0, seg: 1.0, hseg: 0.5, invseg: 1.0, mirror: true, invzoom: 1.0,
            rc: 1.0, rs: 0.0, f: 1.0, invf: 1.0, plane: true, rho: 1.0, yc: 0.0, cc: -1.0, gap: 0.0, twist: 0.0, dz: 0.0, fog: 0.0, fogc: [0.0; 4], k: 1.0, tcx: 0.0, ty1: 0.0,
            n: 0, step: [0.0; 2], org: [0.0; 2], inv: 1.0, drift: [0.0; 2], oct: 1, seed: [0; 2], disp: 0.0, tab: [[0.0; 4]; MAXTAP],
        }
    }
    /// Needs the map buffer (`apply_map`).
    pub fn uses_map(&self) -> bool {
        self.kind == DISPLACE_MAP
    }
}

impl WarpCfg {
    /// Does it leave the buffer exactly as it is?
    pub fn identity(&self) -> bool {
        match self.kind {
            KALEIDO => self.amount <= 0.0,
            TUNNEL_PERSPECTIVE => self.amount <= 0.0 && self.dz == 0.0 && self.twist == 0.0 && self.rs == 0.0 && self.rc == 1.0 && self.fog == 0.0,
            DIR_BLUR..=DISPLACE_MAP => self.amount == 0.0,
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
    let s = (m[0] * m[3] - m[1] * m[2]).abs().sqrt().max(1e-9);
    let rot = m[1].atan2(m[0]);
    let rect = match e {
        Effect::Kaleido { rect, .. } | Effect::Tunnel { rect, .. } => *rect,
        Effect::DirBlur { .. } | Effect::RadialBlur { .. } | Effect::Chromatic { .. } | Effect::Displace { .. } => {
            if w <= 0 || h <= 0 {
                return None;
            }
            let ink_b = [ink[0] - o[0] as f64, ink[1] - o[1] as f64, ink[0] + ink[2] - o[0] as f64, ink[1] + ink[3] - o[1] as f64];
            return Some(distort(e, p, s, rot, &to_buf, ink_b, [0, 0, w, h]));
        }
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
    let mut c = WarpCfg { amount: p[0].clamp(0.0, 1.0) as f32, ..WarpCfg::base(dom) };
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
        Effect::Tunnel { speed, fog_color, center, map, time_offset, .. } => {
            let ctr = to_buf(center[0], center[1]);
            c.kind = if map == "perspective" { TUNNEL_PERSPECTIVE } else { TUNNEL_TEXTURE };
            c.c = [ctr[0] as f32, ctr[1] as f32];
            let roll = p[3].to_radians() + rot;
            let (rs, rc) = if roll == 0.0 { (0.0, 1.0) } else { roll.sin_cos() };
            c.rc = rc as f32;
            c.rs = rs as f32;
            let f = (p[4].max(1.0) * s).max(1e-3);
            c.f = f as f32;
            c.invf = (1.0 / f) as f32;
            let a = p[0].clamp(0.0, 1.0);
            c.plane = a < 1e-4;
            if !c.plane {
                let rho = 1.0 / a;
                c.rho = rho as f32;
                c.yc = (1.0 - rho) as f32;
                c.cc = (1.0 - 2.0 * rho) as f32;
                c.gap = (std::f64::consts::TAU * (rho - 1.0)) as f32;
            } else {
                c.gap = 1e30;
            }
            c.twist = p[2].to_radians() as f32;
            c.dz = (p[1] + speed * (p[TIME] + time_offset)) as f32;
            c.fog = p[5].max(0.0) as f32;
            if let Some(fc) = fog_color {
                let al = fc[3].clamp(0.0, 1.0);
                c.fogc = [(fc[0] * al * 255.0) as f32, (fc[1] * al * 255.0) as f32, (fc[2] * al * 255.0) as f32, (al * 255.0) as f32];
            }
            c.k = ((dom[2] - dom[0]) as f64 / std::f64::consts::TAU) as f32;
            c.tcx = ((dom[0] + dom[2]) as f64 * 0.5) as f32;
            c.ty1 = dom[3] as f32;
        }
        _ => return None,
    }
    Some(c)
}

/// `n` (>= 2) values spread evenly over [lo, hi].
fn spread(n: usize, lo: f64, hi: f64) -> impl Iterator<Item = f64> {
    (0..n).map(move |i| lo + (hi - lo) * i as f64 / (n - 1) as f64)
}

/// Samples for a streak of `len` buffer px: about one per px, 2..=MAXTAP.
fn taps(len: f64) -> usize {
    (len.abs().ceil() as usize + 1).clamp(2, MAXTAP)
}

/// The distort pack resolved for a buffer (`dom` = the whole buffer): `ink` = the subtree's
/// ink box in buffer px (x0 y0 x1 y1, the reach of the radial effects), `s` / `rot` = the
/// node's scale and rotation, `to_buf` = node px -> buffer px.
fn distort(e: &Effect, p: &[f64; NPARAM], s: f64, rot: f64, to_buf: &dyn Fn(f64, f64) -> [f64; 2], ink: [f64; 4], dom: [i32; 4]) -> WarpCfg {
    let mut c = WarpCfg::base(dom);
    // farthest ink corner from a center (buffer px): the reach of zoom / spin / chromatic
    let reach = |ctr: [f64; 2]| [(ink[0], ink[1]), (ink[2], ink[1]), (ink[0], ink[3]), (ink[2], ink[3])].iter().map(|(x, y)| (x - ctr[0]).hypot(y - ctr[1])).fold(1.0, f64::max);
    match e {
        Effect::DirBlur { .. } => {
            let len = p[0].clamp(0.0, 2000.0) * s;
            let n = taps(len);
            let a = p[1].to_radians() + rot;
            let d = len / (n - 1) as f64;
            c.kind = DIR_BLUR;
            c.amount = len as f32;
            c.n = n as u32;
            c.step = [(d * a.cos()) as f32, (d * a.sin()) as f32];
            let wgt = (1.0 / n as f64) as f32;
            for (i, k) in spread(n, -0.5 * (n - 1) as f64, 0.5 * (n - 1) as f64).enumerate() {
                c.tab[i] = [k as f32, 0.0, wgt, 0.0];
            }
        }
        Effect::RadialBlur { mode, .. } => {
            let ctr = to_buf(p[1], p[2]);
            let r = reach(ctr);
            c.c = [ctr[0] as f32, ctr[1] as f32];
            if mode == "spin" {
                let a = p[0].clamp(0.0, 360.0).to_radians();
                let n = taps(a * r);
                c.kind = SPIN_BLUR;
                c.amount = a as f32;
                c.n = n as u32;
                let wgt = (1.0 / n as f64) as f32;
                for (i, t) in spread(n, -0.5 * a, 0.5 * a).enumerate() {
                    c.tab[i] = [t.cos() as f32, t.sin() as f32, wgt, 0.0];
                }
            } else {
                let a = p[0].clamp(0.0, 1.0);
                let n = taps(a * r);
                c.kind = ZOOM_BLUR;
                c.amount = a as f32;
                c.n = n as u32;
                let wgt = (1.0 / n as f64) as f32;
                for (i, k) in spread(n, 1.0, 1.0 - a).enumerate() {
                    c.tab[i] = [k as f32, 0.0, wgt, 0.0];
                }
            }
        }
        Effect::Chromatic { .. } => {
            let ctr = to_buf(p[1], p[2]);
            let amt = p[0].clamp(-500.0, 500.0) * s;
            let k = amt / reach(ctr);
            // odd count: the middle tap is pure green
            let mut n = (2 * amt.abs().ceil() as usize + 1).clamp(3, MAXTAP - 1);
            if n % 2 == 0 {
                n -= 1;
            }
            c.kind = CHROMATIC;
            c.c = [ctr[0] as f32, ctr[1] as f32];
            c.amount = amt as f32;
            c.n = n as u32;
            // u = 0 red .. 0.5 green .. 1 blue (tent weights), red sampled farther out
            let w: Vec<[f64; 3]> = spread(n, 0.0, 1.0).map(|u| [(1.0 - 2.0 * u).max(0.0), 1.0 - (2.0 * u - 1.0).abs(), (2.0 * u - 1.0).max(0.0)]).collect();
            let sum: [f64; 3] = std::array::from_fn(|ch| w.iter().map(|x| x[ch]).sum::<f64>().max(1e-12));
            for (i, u) in spread(n, 0.0, 1.0).enumerate() {
                let t = 2.0 * u - 1.0;
                c.tab[i] = [(1.0 - k * t) as f32, (w[i][0] / sum[0]) as f32, (w[i][1] / sum[1]) as f32, (w[i][2] / sum[2]) as f32];
            }
        }
        Effect::Displace { octaves, seed, source, .. } => {
            let amt = p[0].clamp(-2000.0, 2000.0) * s;
            c.amount = amt as f32;
            if source.is_some() {
                c.kind = DISPLACE_MAP;
                c.disp = (amt / 127.5) as f32;
            } else {
                c.kind = DISPLACE_NOISE;
                c.disp = amt as f32;
                let o = to_buf(0.0, 0.0);
                c.org = [o[0] as f32, o[1] as f32];
                c.inv = (1.0 / (p[1].clamp(1.0, 1e5) * s).max(1e-6)) as f32;
                let d = p[2] * p[TIME];
                c.drift = [d as f32, (0.618 * d) as f32];
                c.oct = (*octaves).clamp(1, 8);
                let sd = *seed as u32;
                c.seed = [sd, sd.wrapping_add(0x9e37_79b9)];
            }
        }
        _ => {}
    }
    c
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

/// The ray of pixel (px, py) (center-relative, rolled, over the focal length) hits the
/// curled floor at (u = arc position, z = depth), with coverage (strip edges, antialiased);
/// None: it misses (the open side of the floor).
#[inline]
fn hit(c: &WarpCfg, ex: f32, ey: f32) -> Option<(f32, f32, f32)> {
    let (u, z) = if c.plane {
        if ey <= 1e-7 {
            return None;
        }
        let t = 1.0 / ey;
        (t * ex, t)
    } else {
        let b = ey * c.yc;
        let d2 = fma(ex, ex, ey * ey);
        let disc = fma(b, b, -(d2 * c.cc));
        let sq = disc.sqrt();
        let t = if b >= 0.0 { (b + sq) / d2.max(1e-12) } else { -c.cc / (sq - b) };
        let vx = t * ex;
        let vy = fma(t, ey, -c.yc);
        (c.rho * atan2p(vx, vy), t)
    };
    let pxu = c.f / z;
    let e = 3.141_592_741_012_573_2 - u.abs();
    let cov = fma(e, pxu, 0.5).clamp(0.0, 1.0).max(fma(-c.gap, pxu, 1.0));
    if cov <= 0.0 {
        return None;
    }
    Some((u, z, cov))
}

/// Perspective map: a point past the drawing's bottom edge slides back toward the vanishing
/// point along its ray (lines through the vanishing point continue, the nearest row of the
/// drawing repeats), so the near walls of the tube are never empty. Past the sides the
/// drawing mirrors (`sample` MIRROR).
#[inline]
fn ray_clamp(c: &WarpCfg, oy: f32) -> f32 {
    let by = (c.dom[3] as f32 - 0.5) - c.c[1];
    if by > 0.0 && oy > by { by / oy } else { 1.0 }
}

#[inline]
/// Fog by `z`, the hit's distance along the tube from the camera (independent of the travel
/// `dz`, so a long flight does not fog the whole tube).
fn fade(c: &WarpCfg, z: f32, s: [f32; 4]) -> [f32; 4] {
    let fz = c.fog * z;
    let f = 1.0 / fma(fz, fz, 1.0);
    std::array::from_fn(|k| fma(s[k] - c.fogc[k], f, c.fogc[k]))
}

/// The warped pixel (x, y) of `src`, premultiplied 0..255.
#[inline]
pub fn pixel(c: &WarpCfg, src: &[u8], w: usize, x: usize, y: usize) -> [f32; 4] {
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    let qx = px - c.c[0];
    let qy = py - c.c[1];
    match c.kind {
        KALEIDO => {
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
        DIR_BLUR..=SPIN_BLUR => {
            let mut acc = [0.0f32; 4];
            for t in &c.tab[..c.n as usize] {
                let (sx, sy) = match c.kind {
                    DIR_BLUR => (fma(c.step[0], t[0], px), fma(c.step[1], t[0], py)),
                    ZOOM_BLUR => (fma(qx, t[0], c.c[0]), fma(qy, t[0], c.c[1])),
                    _ => (fma(t[0], qx, fma(-t[1], qy, c.c[0])), fma(t[1], qx, fma(t[0], qy, c.c[1]))),
                };
                let v = sample(src, w, c.dom, CLEAR, sx, sy);
                for k in 0..4 {
                    acc[k] = fma(v[k], t[2], acc[k]);
                }
            }
            acc
        }
        CHROMATIC => {
            // per channel: its color and its own coverage; alpha = the widest coverage
            let mut col = [0.0f32; 3];
            let mut cov = [0.0f32; 3];
            for t in &c.tab[..c.n as usize] {
                let v = sample(src, w, c.dom, CLEAR, fma(qx, t[0], c.c[0]), fma(qy, t[0], c.c[1]));
                for k in 0..3 {
                    col[k] = fma(v[k], t[k + 1], col[k]);
                    cov[k] = fma(v[3], t[k + 1], cov[k]);
                }
            }
            [col[0], col[1], col[2], cov[0].max(cov[1]).max(cov[2])]
        }
        DISPLACE_NOISE => {
            let nx = fma(px - c.org[0], c.inv, c.drift[0]);
            let ny = fma(py - c.org[1], c.inv, c.drift[1]);
            let dx = crate::generator::fbm(nx, ny, c.oct, c.seed[0]);
            let dy = crate::generator::fbm(nx, ny, c.oct, c.seed[1]);
            sample(src, w, c.dom, CLEAR, fma(dx, c.disp, px), fma(dy, c.disp, py))
        }
        // needs the map: `map_pixel`
        DISPLACE_MAP => sample(src, w, c.dom, CLEAR, px, py),
        kind => {
            let dx = fma(c.rc, qx, c.rs * qy);
            let dy = fma(c.rc, qy, -(c.rs * qx));
            let h = hit(c, dx * c.invf, dy * c.invf);
            if kind == TUNNEL_TEXTURE {
                let Some((u, z, cov)) = h else { return [0.0; 4] };
                let zz = z + c.dz;
                let ur = fma(c.twist, zz, u);
                let s = sample(src, w, c.dom, WRAP, fma(ur, c.k, c.tcx), fma(-zz, c.k, c.ty1));
                // fog by the distance from the camera, not by the travelled texture depth
                let col = fade(c, z, s);
                col.map(|v| v * cov)
            } else {
                let miss = sample(src, w, c.dom, CLEAR, c.c[0] + dx, c.c[1] + dy);
                let Some((u, z, cov)) = h else { return miss };
                let zz = (z + c.dz).max(1e-3);
                let ur = fma(c.twist, zz, u);
                let iz = c.f / zz;
                let (ox, oy) = (ur * iz, iz);
                let k = ray_clamp(c, oy);
                let s = sample(src, w, c.dom, MIRROR, fma(ox, k, c.c[0]), fma(oy, k, c.c[1]));
                let col = fade(c, z, s);
                std::array::from_fn(|k| fma(col[k] - miss[k], cov, miss[k]))
            }
        }
    }
}

/// The pixel (x, y) displaced by `map` (premultiplied, the size of `src`): red moves x,
/// green moves y, by (channel - alpha / 2) x `disp` (transparent or mid-grey: no move).
#[inline]
pub fn map_pixel(c: &WarpCfg, src: &[u8], map: &[u8], w: usize, x: usize, y: usize) -> [f32; 4] {
    let i = (y * w + x) * 4;
    let (mr, mg, ma) = (map[i] as f32, map[i + 1] as f32, map[i + 3] as f32);
    let dx = fma(-0.5, ma, mr);
    let dy = fma(-0.5, ma, mg);
    sample(src, w, c.dom, CLEAR, fma(dx, c.disp, x as f32 + 0.5), fma(dy, c.disp, y as f32 + 0.5))
}

/// Apply a resolved warp to a w x h premultiplied RGBA8 buffer in place.
pub fn apply(c: &WarpCfg, buf: &mut [u8], w: usize, h: usize) {
    apply_map(c, buf, None, w, h)
}

/// `apply` with the map of a `displace` by a layer (`uses_map`; without a map of the
/// buffer's size that kind leaves the buffer as it is).
pub fn apply_map(c: &WarpCfg, buf: &mut [u8], map: Option<&[u8]>, w: usize, h: usize) {
    if w == 0 || h == 0 || c.identity() {
        return;
    }
    let map = map.filter(|m| m.len() == buf.len());
    if c.uses_map() && map.is_none() {
        return;
    }
    let src = buf.to_vec();
    buf.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let v = match map {
                Some(m) if c.uses_map() => map_pixel(c, &src, m, w, x, y),
                _ => pixel(c, &src, w, x, y),
            };
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

    pub fn tunnel(map: &str, amount: f64, t: f64) -> (Effect, [f64; NPARAM]) {
        let e = Effect::Tunnel { amount, depth: 0.4, speed: 1.5, twist: 20.0, angle: 8.0, radius: 120.0, fog: 0.2, fog_color: Some([0.1, 0.0, 0.2, 0.8]), center: [190.0, 140.0], map: map.into(), rect: Some([0.0, 0.0, 403.0, 297.0]), time_offset: 0.0 };
        let mut p = [0.0; NPARAM];
        for (i, f) in e.look_fields().iter().enumerate() {
            p[i] = e.look_field(f);
        }
        p[TIME] = t;
        (e, p)
    }

    /// The warps of the conformance tests (`gpu::fx` tests too).
    pub fn cases() -> Vec<(String, Effect, [f64; NPARAM])> {
        let mut v = vec![];
        for (n, m, a) in [(6, true, 1.0), (8, true, 0.55), (5, false, 1.0), (12, true, 1.0)] {
            let (e, p) = kaleido(n, m, a);
            v.push((format!("kaleido-{n}-{m}-{a}"), e, p));
        }
        for map in ["texture", "perspective"] {
            for a in [0.0, 0.35, 0.8, 1.0] {
                let (e, p) = tunnel(map, a, 1.25);
                v.push((format!("tunnel-{map}-{a}"), e, p));
            }
        }
        for (name, e) in distorts() {
            v.push((name, e.clone(), params(&e, 1.7)));
        }
        v
    }

    /// The distort pack cases (no map layer).
    pub fn distorts() -> Vec<(String, Effect)> {
        vec![
            ("dir-blur-24-30".into(), Effect::DirBlur { length: 24.0, angle: 30.0 }),
            ("dir-blur-200-0".into(), Effect::DirBlur { length: 200.0, angle: 0.0 }),
            ("dir-blur-3-97".into(), Effect::DirBlur { length: 3.0, angle: 97.0 }),
            ("zoom-0.25".into(), Effect::RadialBlur { amount: 0.25, mode: "zoom".into(), center: [150.0, 120.0] }),
            ("zoom-1".into(), Effect::RadialBlur { amount: 1.0, mode: "zoom".into(), center: [10.0, 290.0] }),
            ("spin-12".into(), Effect::RadialBlur { amount: 12.0, mode: "spin".into(), center: [201.0, 148.0] }),
            ("spin-90".into(), Effect::RadialBlur { amount: 90.0, mode: "spin".into(), center: [60.0, 40.0] }),
            ("chromatic-6".into(), Effect::Chromatic { amount: 6.0, center: [201.0, 148.0] }),
            ("chromatic--2.5".into(), Effect::Chromatic { amount: -2.5, center: [120.0, 60.0] }),
            ("chromatic-40".into(), Effect::Chromatic { amount: 40.0, center: [300.0, 200.0] }),
            ("displace-noise".into(), Effect::Displace { amount: 14.0, scale: 60.0, octaves: 3, speed: 0.4, seed: 9, source: None }),
            ("displace-noise-8oct".into(), Effect::Displace { amount: -30.0, scale: 150.0, octaves: 8, speed: 0.0, seed: 3, source: None }),
        ]
    }

    pub fn params(e: &Effect, t: f64) -> [f64; NPARAM] {
        let mut p = [0.0; NPARAM];
        for (i, f) in e.look_fields().iter().enumerate() {
            p[i] = e.look_field(f);
        }
        p[TIME] = t;
        p
    }

    /// A displacement map: smooth red / green ramps and a transparent hole.
    pub fn map_fixture(w: usize, h: usize) -> Vec<u8> {
        (0..w * h).flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let a: u32 = if (x as i64 - 80).pow(2) + (y as i64 - 60).pow(2) < 900 { 0 } else { 255 };
            let r = ((x * 255 / w.max(1)) as u32 * a / 255) as u8;
            let g = (((y * 7) % 256) as u32 * a / 255) as u8;
            [r, g, 0, a as u8]
        }).collect()
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
        let e0 = Effect::Tunnel { amount: 0.0, depth: 0.0, speed: 0.0, twist: 0.0, angle: 0.0, radius: 120.0, fog: 0.0, fog_color: None, center: [190.0, 140.0], map: "perspective".into(), rect: None, time_offset: 0.0 };
        let mut p = [0.0; NPARAM];
        for (i, f) in e0.look_fields().iter().enumerate() {
            p[i] = e0.look_field(f);
        }
        assert_eq!(run(&e0, &p, &src, w, h), src, "perspective tunnel at amount 0");
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
    fn tunnel_is_a_pure_function_of_time() {
        let (w, h) = (160, 120);
        let src = poster(w, h);
        let (e, p3) = tunnel("texture", 1.0, 3.0);
        let (_, p1) = tunnel("texture", 1.0, 1.0);
        let a = run(&e, &p3, &src, w, h);
        let _ = run(&e, &p1, &src, w, h);
        assert_eq!(a, run(&e, &p3, &src, w, h));
        assert_ne!(a, run(&e, &p1, &src, w, h), "it moves");
        assert!(e.time_dependent());
        // depth and speed x time are the same travel
        let Effect::Tunnel { speed, .. } = e else { unreachable!() };
        let mut q = p3;
        q[1] += speed * 3.0;
        q[TIME] = 0.0;
        let (e0, _) = tunnel("texture", 1.0, 0.0);
        let still = match e0 { Effect::Tunnel { speed: _, amount, depth, twist, angle, radius, fog, fog_color, center, map, rect, time_offset } => Effect::Tunnel { amount, depth, speed: 0.0, twist, angle, radius, fog, fog_color, center, map, rect, time_offset }, _ => unreachable!() };
        assert!(!still.time_dependent());
        let b = run(&still, &q, &src, w, h);
        let d = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
        assert!(d <= 1, "travel = depth + speed t ({d})");
    }

    #[test]
    fn tunnel_fog_depends_on_depth_not_travel() {
        // an opaque layer with a transparent fog: the output alpha is coverage x fog, the same
        // after a long flight (speed x t) as at the start; it used to fade with the travel
        let (w, h) = (160, 120);
        let src = crate::gpu::fx::tests::poster(w, h, true);
        for map in ["texture", "perspective"] {
            let (e, p0) = tunnel(map, 1.0, 0.0);
            let e = match e { Effect::Tunnel { amount, depth, speed, twist, angle, radius, center, map, rect, time_offset, .. } => Effect::Tunnel { amount, depth, speed, twist, angle, radius, fog: 0.6, fog_color: None, center, map, rect, time_offset }, _ => unreachable!() };
            let mut p0 = p0;
            for (i, f) in e.look_fields().iter().enumerate() {
                p0[i] = e.look_field(f);
            }
            let mut p1 = p0;
            p1[TIME] = 400.0;
            let (a, b) = (run(&e, &p0, &src, w, h), run(&e, &p1, &src, w, h));
            let alpha = |v: &[u8]| v.chunks_exact(4).map(|q| q[3] as u64).sum::<u64>();
            assert!(alpha(&a) > 0, "{map}: fixture shows nothing");
            let d = a.chunks_exact(4).zip(b.chunks_exact(4)).map(|(x, y)| x[3].abs_diff(y[3])).max().unwrap();
            assert!(d <= 1, "{map}: alpha after a long flight differs by {d} (sum {} vs {})", alpha(&a), alpha(&b));
        }
    }

    fn cfg_of(e: &Effect, p: &[f64; NPARAM], w: usize, h: usize) -> WarpCfg {
        cfg(e, p, M, [0.0, 0.0, w as f64, h as f64], [0, 0, w as i32, h as i32]).unwrap()
    }

    #[test]
    fn distort_zero_is_identity() {
        let (w, h) = (120, 90);
        let src = poster(w, h);
        let zero = [
            Effect::DirBlur { length: 0.0, angle: 30.0 },
            Effect::RadialBlur { amount: 0.0, mode: "zoom".into(), center: [60.0, 45.0] },
            Effect::RadialBlur { amount: 0.0, mode: "spin".into(), center: [60.0, 45.0] },
            Effect::Chromatic { amount: 0.0, center: [60.0, 45.0] },
            Effect::Displace { amount: 0.0, scale: 40.0, octaves: 3, speed: 1.0, seed: 1, source: None },
        ];
        for e in zero {
            let p = params(&e, 0.5);
            assert!(cfg_of(&e, &p, w, h).identity(), "{e:?}");
            assert_eq!(run(&e, &p, &src, w, h), src, "{e:?}");
        }
    }

    #[test]
    fn dir_blur_streaks_along_its_angle() {
        // a 1 px vertical white line: a horizontal blur of 21 px spreads it over 21 columns
        // at 1/21 each; a vertical blur leaves it where it is
        let (w, h) = (101, 41);
        let mut src = vec![0u8; w * h * 4];
        for y in 0..h {
            src[(y * w + 50) * 4..][..4].copy_from_slice(&[255; 4]);
        }
        let blur = |angle: f64| run(&Effect::DirBlur { length: 20.0, angle }, &params(&Effect::DirBlur { length: 20.0, angle }, 0.0), &src, w, h);
        let hz = blur(0.0);
        let row = |b: &[u8], x: usize| b[(20 * w + x) * 4 + 3];
        let lit: Vec<usize> = (0..w).filter(|&x| row(&hz, x) > 0).collect();
        assert_eq!((lit[0], *lit.last().unwrap()), (40, 60), "{lit:?}");
        assert!((row(&hz, 50) as i32 - 12).abs() <= 1, "{}", row(&hz, 50));
        let vt = blur(90.0);
        assert_eq!(row(&vt, 50), 255);
        assert_eq!(row(&vt, 49), 0);
    }

    #[test]
    fn radial_blurs_keep_the_center_sharp() {
        let (w, h) = (161, 121);
        let src = poster(w, h);
        let c = [80.5, 60.5];
        for mode in ["zoom", "spin"] {
            let e = Effect::RadialBlur { amount: if mode == "zoom" { 0.6 } else { 40.0 }, mode: mode.into(), center: c };
            let out = run(&e, &params(&e, 0.0), &src, w, h);
            let at = |b: &[u8], x: usize, y: usize| b[(y * w + x) * 4..][..4].to_vec();
            let d = at(&out, 80, 60).iter().zip(at(&src, 80, 60)).map(|(a, b)| a.abs_diff(b)).max().unwrap();
            assert!(d <= 2, "{mode}: center moved by {d}");
            assert_ne!(out, src, "{mode}");
        }
    }

    #[test]
    fn chromatic_splits_red_and_blue_apart() {
        // a white disc on transparent: red shrinks toward the center, blue grows past it,
        // green stays; alpha covers the widest
        let (w, h) = (161, 161);
        let src: Vec<u8> = (0..w * h).flat_map(|i| {
            let (x, y) = ((i % w) as f64 - 80.0, (i / w) as f64 - 80.0);
            if x.hypot(y) < 50.0 { [255u8; 4] } else { [0; 4] }
        }).collect();
        let e = Effect::Chromatic { amount: 8.0, center: [80.5, 80.5] };
        let out = run(&e, &params(&e, 0.0), &src, w, h);
        let px = |x: usize| &out[(80 * w + x) * 4..][..4];
        // just inside the original edge (x = 80 + 48): red already gone, blue still there
        let edge = px(128);
        assert!(edge[0] < edge[2], "inside the edge red {} blue {}", edge[0], edge[2]);
        // just outside (x = 80 + 52): only blue (and some green) spill out
        let out_px = px(132);
        assert!(out_px[2] > 0 && out_px[0] == 0, "outside {out_px:?}");
        assert!(out.chunks_exact(4).all(|q| q[..3].iter().all(|c| *c <= q[3])), "premultiplied");
    }

    #[test]
    fn displace_map_moves_by_red_and_green() {
        let (w, h) = (160, 120);
        let src = poster(w, h);
        // alpha 254: green 127 is exactly half (no y move), red 254 moves x by amount x 127 / 127.5
        let e = Effect::Displace { amount: 10.0 * 127.5 / 127.0, scale: 80.0, octaves: 3, speed: 0.0, seed: 0, source: Some(1) };
        let p = params(&e, 0.0);
        let c = cfg_of(&e, &p, w, h);
        assert!(c.uses_map());
        // a transparent map and an exact half grey move nothing
        let mut b = src.clone();
        apply_map(&c, &mut b, Some(&vec![0u8; w * h * 4]), w, h);
        assert_eq!(b, src, "transparent map");
        let mut b = src.clone();
        apply_map(&c, &mut b, Some(&(0..w * h).flat_map(|_| [127u8, 127, 0, 254]).collect::<Vec<_>>()), w, h);
        assert_eq!(b, src, "half grey map");
        // full red: every pixel samples 10 px to the right
        let red: Vec<u8> = (0..w * h).flat_map(|_| [254u8, 127, 0, 254]).collect();
        let mut b = src.clone();
        apply_map(&c, &mut b, Some(&red), w, h);
        let mut worst = 0u8;
        for y in 0..h {
            for x in 0..w - 11 {
                let a = &b[(y * w + x) * 4..][..4];
                let s = &src[(y * w + x + 10) * 4..][..4];
                worst = worst.max(a.iter().zip(s).map(|(p, q)| p.abs_diff(*q)).max().unwrap());
            }
        }
        assert!(worst <= 2, "shift by 10 px off by {worst}");
        // no map: unchanged
        let mut b = src.clone();
        apply_map(&c, &mut b, None, w, h);
        assert_eq!(b, src);
    }

    #[test]
    fn displace_noise_moves_with_time_only_when_it_has_speed() {
        let (w, h) = (120, 90);
        let src = poster(w, h);
        let e = Effect::Displace { amount: 12.0, scale: 30.0, octaves: 4, speed: 0.5, seed: 4, source: None };
        let a = run(&e, &params(&e, 0.0), &src, w, h);
        assert_ne!(a, src);
        assert_ne!(a, run(&e, &params(&e, 1.0), &src, w, h), "it evolves");
        assert!(e.time_dependent());
        let still = Effect::Displace { amount: 12.0, scale: 30.0, octaves: 4, speed: 0.0, seed: 4, source: None };
        assert!(!still.time_dependent());
        let other_seed = Effect::Displace { amount: 12.0, scale: 30.0, octaves: 4, speed: 0.5, seed: 5, source: None };
        assert_ne!(a, run(&other_seed, &params(&other_seed, 0.0), &src, w, h), "seeded");
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
