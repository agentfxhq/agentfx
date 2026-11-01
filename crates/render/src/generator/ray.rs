//! Raymarched 3D kinds (`mandelbulb`, `kifs`): the shared orbit camera, the sphere-traced march
//! through a distance estimator and the shading (orbit-trap palette, step-count ambient
//! occlusion, rim light, fog, neon halo). A kind supplies only its distance estimator (`Sdf`).
//!
//! CPU / GPU agreement. Raymarching amplifies the last bit: a hit or a miss at a silhouette, one
//! step more or less, flips a pixel completely. So nothing in the march uses a transcendental
//! the GPU would compute differently: the camera basis, the light and the per-kind constants
//! (rotation matrices) are computed on the host in f64 (`uniforms`) and passed in, and `M`
//! supplies 1/x, 1/sqrt, log2, exp2, atan2 and sin/cos built from add, mul, fma and bit casts
//! only, which `ray.wgsl` reproduces operation for operation (every `a * b + c` an explicit fma;
//! drivers fuse on their own otherwise). Step counts are fixed by parameters and every early
//! out depends only on those values. Result: bit for bit on NVIDIA Vulkan (`gpu_matches_cpu`,
//! max diff 0). What is left is the driver: one that flushes denormals or does not fuse `fma`
//! breaks that, as it breaks the 2D kinds.
use super::{Generator, Input, Real, palette, shade};

/// The 3D kinds' extra math. f32: own approximations, bit for bit what `ray.wgsl` computes (about
/// 1e-6 relative, smooth); f64: std (the reference renders).
pub trait M: Real {
    /// 1 / x, x > 0
    fn rcp(self) -> Self;
    /// 1 / sqrt(x), x >= 0
    fn rsqrt(self) -> Self;
    /// sqrt(x), x >= 0
    fn root(self) -> Self;
    /// log2(x), x > 0
    fn lg2(self) -> Self;
    /// 2^x (clamped to the normal f32 range)
    fn ex2(self) -> Self;
    /// atan2(y, x) in turns (-0.5..0.5]
    fn atan2t(y: Self, x: Self) -> Self;
    /// (sin, cos) of `self * n` turns
    fn sincost(self, n: Self) -> (Self, Self);
    fn mn(self, b: Self) -> Self {
        if self < b { self } else { b }
    }
    fn mx(self, b: Self) -> Self {
        if self > b { self } else { b }
    }
}

impl M for f32 {
    fn rcp(self) -> f32 {
        let mut y = f32::from_bits(0x7ef3_11c3u32.wrapping_sub(self.to_bits()));
        for _ in 0..3 {
            let e = (-self).mul_add(y, 1.0);
            y = y.mul_add(e, y);
        }
        y
    }
    fn rsqrt(self) -> f32 {
        let mut y = f32::from_bits(0x5f37_59dfu32.wrapping_sub(self.to_bits() >> 1));
        let h = 0.5 * self;
        for _ in 0..3 {
            y = y * (-(h * y)).mul_add(y, 1.5);
        }
        y
    }
    fn root(self) -> f32 {
        self * self.rsqrt()
    }
    fn lg2(self) -> f32 {
        let b = self.to_bits();
        let mut e = ((b >> 23) & 0xff) as i32 - 127;
        let mut m = f32::from_bits((b & 0x7f_ffff) | 0x3f80_0000);
        if m > 1.5 {
            m = m * 0.5;
            e += 1;
        }
        let s = (m - 1.0) * (m + 1.0).rcp();
        let s2 = s * s;
        let pl = s2.mul_add(s2.mul_add(s2.mul_add(s2.mul_add(0.111_111_11, 0.142_857_15), 0.2), 0.333_333_34), 1.0);
        // ln m = 2 s pl; log2 = ln m / ln 2
        (s * pl).mul_add(2.885_39, e as f32)
    }
    fn ex2(self) -> f32 {
        let x = self.clamp(-126.0, 126.0);
        let n = (x + 0.5).floor();
        let f = x - n;
        let p = f.mul_add(f.mul_add(f.mul_add(f.mul_add(f.mul_add(f.mul_add(f.mul_add(1.525_273_4e-5, 1.540_353e-4), 1.333_355_8e-3), 9.618_129e-3), 5.550_411e-2), 0.240_226_5), 0.693_147_2), 1.0);
        p * f32::from_bits(((n as i32 + 127) as u32) << 23)
    }
    fn atan2t(y: f32, x: f32) -> f32 {
        let (ax, ay) = (x.abs(), y.abs());
        let hi = ax.mx(ay).mx(1e-30);
        let lo = ax.mn(ay);
        let a = lo * hi.rcp();
        let a2 = a * a;
        let p = a2.mul_add(a2.mul_add(a2.mul_add(a2.mul_add(a2.mul_add(-0.011_721_2, 0.052_653_32), -0.116_432_87), 0.193_543_46), -0.332_623_47), 0.999_977_26);
        // octant fix-ups as single fmas (a product fed into an add is what drivers fuse)
        let sw = ay > ax;
        let r = (a * p).mul_add(if sw { -0.159_154_94 } else { 0.159_154_94 }, if sw { 0.25 } else { 0.0 });
        let r = if x < 0.0 { 0.5 - r } else { r };
        if y < 0.0 { -r } else { r }
    }
    fn sincost(self, n: f32) -> (f32, f32) {
        let f = self.mul_add(n, -self.mul_add(n, 0.5).floor());
        let q = f.mul_add(4.0, 0.5).floor();
        let r = q.mul_add(-0.25, f) * 6.283_185_5;
        let r2 = r * r;
        let s = r * r2.mul_add(r2.mul_add(r2.mul_add(r2.mul_add(2.755_732e-6, -1.984_127e-4), 8.333_334e-3), -0.166_666_67), 1.0);
        let c = r2.mul_add(r2.mul_add(r2.mul_add(r2.mul_add(2.480_158_8e-5, -1.388_888_9e-3), 4.166_666_8e-2), -0.5), 1.0);
        match ((q as i32 % 4) + 4) % 4 {
            0 => (s, c),
            1 => (c, -s),
            2 => (-s, -c),
            _ => (-c, s),
        }
    }
}

impl M for f64 {
    fn rcp(self) -> f64 {
        1.0 / self
    }
    fn rsqrt(self) -> f64 {
        1.0 / self.sqrt()
    }
    fn root(self) -> f64 {
        self.sqrt()
    }
    fn lg2(self) -> f64 {
        self.log2()
    }
    fn ex2(self) -> f64 {
        self.clamp(-1000.0, 1000.0).exp2()
    }
    fn atan2t(y: f64, x: f64) -> f64 {
        y.atan2(x) / std::f64::consts::TAU
    }
    fn sincost(self, n: f64) -> (f64, f64) {
        (self * n * std::f64::consts::TAU).sin_cos()
    }
}

pub type V3<T> = [T; 3];

/// a . b, `fma(a.x, b.x, fma(a.y, b.y, a.z * b.z))` (`dot3` in ray.wgsl)
pub fn dot3<T: Real>(a: V3<T>, b: V3<T>) -> T {
    a[0].fma(b[0], a[1].fma(b[1], a[2] * b[2]))
}

/// The 3x3 matrix `m` (row major) times v (`mul3` in ray.wgsl).
pub fn mul3<T: Real>(m: &[T], v: V3<T>) -> V3<T> {
    [0, 1, 2].map(|r| m[3 * r].fma(v[0], m[3 * r + 1].fma(v[1], m[3 * r + 2] * v[2])))
}

/// A kind's distance estimator at p: (a, b, orbit trap), the distance being `a * b` (two factors,
/// so the march can step with one explicit `fma(a, b, t)`). `u`: the uniforms (`UK` on are the
/// kind's own).
pub trait Sdf: Sync {
    fn de<T: M>(&self, u: &[T], p: V3<T>) -> (T, T, T);
}

/// The parameter indices of the shared camera and shading parameters of a kind.
pub struct Shared {
    pub yaw: usize,
    pub pitch: usize,
    pub dist: usize,
    pub zoom: usize,
    pub steps: usize,
    pub halo: usize,
    pub fog: usize,
    pub occlusion: usize,
    pub rim: usize,
    pub speed: usize,
    pub offset: usize,
}

/// Screen half-height at zoom 1 (tan of half the vertical field of view, 53.1 degrees).
pub const HALF: f64 = 0.5;
/// The first uniform of the kind's own (after the shared block).
pub const UK: usize = 24;

/// The shared uniform block: 0-2 eye, 3-5 forward, 6-8 right, 9-11 up, 12-14 light direction,
/// 15 surface epsilon per unit of ray length, 16 bounding sphere radius, 17 steps, 18 halo,
/// 19 fog, 20 occlusion, 21 rim, 22 color-speed, 23 color-offset (`UK` = 24 on: the kind's).
pub fn uniforms(p: &[f64], s: &Shared, bound: f64) -> Vec<f64> {
    let (yaw, pitch) = (p[s.yaw].to_radians(), p[s.pitch].to_radians());
    let d = p[s.dist];
    let eye = [d * pitch.cos() * yaw.sin(), d * pitch.sin(), d * pitch.cos() * yaw.cos()];
    let fw = [-eye[0] / d, -eye[1] / d, -eye[2] / d];
    // right = forward x world up (0, 1, 0), up = right x forward
    let rl = (fw[2] * fw[2] + fw[0] * fw[0]).sqrt().max(1e-12);
    let rt = [-fw[2] / rl, 0.0, fw[0] / rl];
    let up = [rt[1] * fw[2] - rt[2] * fw[1], rt[2] * fw[0] - rt[0] * fw[2], rt[0] * fw[1] - rt[1] * fw[0]];
    // key light from the upper left behind the camera
    let l = [0, 1, 2].map(|k| 0.75 * up[k] - 0.55 * rt[k] - fw[k]);
    let ll = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
    let mut u = Vec::with_capacity(UK + 24);
    u.extend_from_slice(&eye);
    u.extend_from_slice(&fw);
    u.extend_from_slice(&rt);
    u.extend_from_slice(&up);
    u.extend(l.map(|v| v / ll));
    u.push(0.0012 * HALF / p[s.zoom]);
    u.push(bound);
    u.push(p[s.steps].round());
    for i in [s.halo, s.fog, s.occlusion, s.rim, s.speed, s.offset] {
        u.push(p[i]);
    }
    u
}

/// One ray through screen point (su, sv) (the view: zoom and roll applied, v up): straight RGBA.
/// `sample` in ray.wgsl.
pub fn march<T: M, S: Sdf + ?Sized>(s: &S, u: &[T], pal: &[[f32; 4]], su: T, sv: T) -> [f32; 4] {
    let one = T::of(1.0);
    let zero = T::of(0.0);
    let v3 = |i: usize| [u[i], u[i + 1], u[i + 2]];
    let (eye, fw, rt, up, li) = (v3(0), v3(3), v3(6), v3(9), v3(12));
    let (epsk, bound) = (u[15], u[16]);
    let steps = u[17].f64() as u32;
    let (halo, fog, occl, rim, speed, offset) = (u[18], u[19], u[20], u[21], u[22], u[23]);
    let pal_t = |x: T| {
        let c = palette(pal, x.f64() as f32);
        [0, 1, 2].map(|j| T::of(c[j] as f64))
    };
    let rd0 = [0, 1, 2].map(|k| su.fma(rt[k], sv.fma(up[k], fw[k])));
    let rn = dot3(rd0, rd0).rsqrt();
    let rd = rd0.map(|c| c * rn);
    // li - rd with rd = rd0 rn, fused explicitly
    let h = [0, 1, 2].map(|j| (-rd0[j]).fma(rn, li[j]));
    let b = dot3(eye, rd);
    let c = (-bound).fma(bound, dot3(eye, eye));
    let disc = b.fma(b, -c);
    let mut col = [zero; 3];
    if disc > zero {
        let rs = disc.rsqrt();
        let t0 = (-disc).fma(rs, -b).mx(zero);
        let tmax = disc.fma(rs, -b);
        let mut t = t0;
        let mut i = 0u32;
        let mut hit = false;
        let mut trap = zero;
        // closest approach (distance per unit of ray length): the halo
        let mut near = T::of(1e10);
        while i < steps {
            let p = [0, 1, 2].map(|k| rd[k].fma(t, eye[k]));
            let (da, db, tr) = s.de(u, p);
            let d = da * db;
            if d < epsk.fma(t, T::of(1e-5)) {
                hit = true;
                trap = tr;
                break;
            }
            let q = d * (t + T::of(0.05)).rcp();
            near = near.mn(q);
            t = da.fma(db, t);
            i += 1;
            if t > tmax {
                break;
            }
        }
        let fi = T::of(i as f64) * T::of(steps as f64).rcp();
        let fa = (-(fog * (t - t0))).ex2();
        if hit {
            let p = [0, 1, 2].map(|k| rd[k].fma(t, eye[k]));
            let e = epsk.fma(t, T::of(1e-5)) * T::of(1.5);
            // tetrahedral gradient of the distance
            let ks = [[one, -one, -one], [-one, -one, one], [-one, one, -one], [one, one, one]];
            let mut n = [zero; 3];
            for kv in ks {
                let (da, db, _) = s.de(u, [0, 1, 2].map(|j| kv[j].fma(e, p[j])));
                let d = da * db;
                n = [0, 1, 2].map(|j| kv[j].fma(d, n[j]));
            }
            let n = n.map(|c| c * dot3(n, n).mx(T::of(1e-30)).rsqrt());
            let diff = dot3(n, li).mx(zero);
            let f = one - (-dot3(n, rd)).mx(zero);
            let rimv = rim * f * f;
            // Blinn specular, exponent 32 by squaring
            let mut sp = (dot3(n, h) * dot3(h, h).mx(T::of(1e-30)).rsqrt()).mx(zero);
            for _ in 0..5 {
                sp = sp * sp;
            }
            let ao = (-occl).fma(fi, one).clamp01();
            let ao = ao * ao;
            let tc = trap.root() * T::of(2.0);
            let base = pal_t(speed.fma(tc, offset));
            let rc = pal_t(speed.fma(tc, offset + T::of(0.5)));
            let lit = diff.fma(T::of(0.85), T::of(0.15)) * ao;
            let rk = rimv * ao.fma(T::of(0.6), T::of(0.4));
            for j in 0..3 {
                col[j] = T::of(0.7).fma(sp * ao, base[j].fma(lit, rc[j] * rk)) * fa;
            }
        }
        // halo. Misses: an aura graded by the closest approach (distance per unit of ray
        // length). Hits: light from the crevices, by march length.
        let nk = near * T::of(40.0);
        let k = nk.fma(nk, one).rcp();
        let (g, gs) = if hit { (halo * fi * T::of(0.9), trap.root() * T::of(2.0)) } else { (halo * k * T::of(0.8), k * T::of(0.5)) };
        let gc = pal_t(speed.fma(gs, offset + T::of(0.25)));
        for j in 0..3 {
            col[j] = gc[j].fma(g, col[j]);
        }
    }
    [col[0].clamp01().f64() as f32, col[1].clamp01().f64() as f32, col[2].clamp01().f64() as f32, 1.0]
}

/// A kind bound to its uniforms for one image (`shade` calls `sample` per sample).
struct Bound<'a, S: Sdf> {
    s: &'a S,
    u64: Vec<f64>,
    u32: Vec<f32>,
}

impl<S: Sdf> Generator for Bound<'_, S> {
    fn sample(&self, g: &Input, u: f64, v: f64) -> [f32; 4] {
        if g.f32 { march(self.s, &self.u32, g.pal, u as f32, v as f32) } else { march(self.s, &self.u64, g.pal, u, v) }
    }
}

fn bind<S: Sdf>(s: &S, u: Vec<f64>) -> Bound<'_, S> {
    let u32 = u.iter().map(|&v| v as f32).collect();
    Bound { s, u64: u, u32 }
}

/// The CPU image of a raymarched kind with uniforms `u` (`Generator::uniforms`).
pub fn cpu<S: Sdf>(s: &S, u: Vec<f64>, g: &Input, out: &mut [u8]) {
    shade(&bind(s, u), g, out)
}

/// One sample of a raymarched kind (`Generator::sample`; whole images go through `cpu`).
pub fn sample<S: Sdf>(s: &S, u: Vec<f64>, g: &Input, x: f64, y: f64) -> [f32; 4] {
    bind(s, u).sample(g, x, y)
}

/// The view of the raymarched kinds: unit space -> the screen plane (v up), zoom and roll.
pub fn view(zoom: f64, turn: f64) -> [f64; 6] {
    super::fractal_view(0.0, 0.0, zoom, 0.0, turn, HALF)
}
