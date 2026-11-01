//! Generator layers (`Content::Gen`): procedural images evaluated per pixel (fractals, noise,
//! tilings). The declared parameters of every kind live in `agentfx_ir::generator::KINDS`
//! (shared with validation, the compiler and the catalog); this module holds the
//! implementations and the registry (`lookup`).
//!
//! A kind implements `Generator`: `sample` (CPU reference, straight RGBA at a point of its
//! space), optionally `view` (unit space -> its space, an affine composed on the host) and
//! optionally `wgsl` (the same function as a WGSL `fn sample(u: f32, v: f32) -> vec4<f32>`;
//! `common.wgsl` provides the entry point, supersampling, palette and dither). Kinds without
//! a kernel always run on the CPU; vector kinds (polylines, grids) override `cpu` and
//! rasterize their geometry into the buffer themselves.
//!
//! Unit space: origin at the rect centre, v from -1 (top) to 1 (bottom), u scaled like v
//! (so u spans +-aspect). The painter maps device pixels into it through the node's
//! transform, so a generator is resolution independent and crisp under any scale.
//!
//! Exactness: CPU and GPU agree within 1/255 per channel when the CPU runs in f32 mode
//! (`Input::f32`, what the conformance tests use); normal renders run the CPU in f64.
pub mod flow;
pub mod julia;
pub mod life;
pub mod lsystem;
pub mod mandelbrot;
pub mod noise;
pub mod penrose;
pub mod raster;
pub mod truchet;
pub mod voronoi;

use agentfx_ir::generator::Kind;
use agentfx_ir::{GenContent, Scene};
use std::ops::{Add, Div, Mul, Neg, Sub};

/// A generator kind's implementation.
pub trait Generator: Sync {
    /// Straight RGBA (0..1) at unit point (u, v). In f32 mode (`g.f32`), compute in f32 like
    /// the GPU kernel.
    fn sample(&self, g: &Input, u: f64, v: f64) -> [f32; 4];
    /// Unit space -> the kind's own space (`[a, b, c, d, e, f]`, like `Input::m`), e.g. the
    /// complex plane of a fractal view. Composed into the pixel mapping on the host in f64, so
    /// `sample` receives the kind's coordinates and CPU and GPU see identical inputs.
    fn view(&self, _p: &[f64]) -> [f64; 6] {
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    }
    /// WGSL source defining `fn sample(u: f32, v: f32) -> vec4<f32>` (parameters: `gp(i)` in
    /// declaration order). None: the kind runs on the CPU only.
    fn wgsl(&self) -> Option<&'static str> {
        None
    }
    /// The whole image: premultiplied RGBA8, `g.w x g.h`. Default: `sample` per pixel.
    fn cpu(&self, g: &Input, out: &mut [u8]) {
        shade(self, g, out)
    }
}

/// The registry: one line per kind (its parameters: `agentfx_ir::generator::KINDS`).
pub fn lookup(kind: &str) -> Option<&'static dyn Generator> {
    Some(match kind {
        "julia" => &julia::Julia,
        "mandelbrot" => &mandelbrot::Mandelbrot,
        "noise" => &noise::Noise,
        "flow" => &flow::Flow,
        "truchet" => &truchet::Truchet,
        "voronoi" => &voronoi::Voronoi,
        "lsystem" => &lsystem::LSystem,
        "penrose" => &penrose::Penrose,
        "life" => &life::Life,
        _ => return None,
    })
}

/// What one evaluation sees.
pub struct Input<'a> {
    pub w: usize,
    pub h: usize,
    /// device position of pixel (0, 0) (the dither pattern is fixed to the device grid)
    pub org: (i32, i32),
    /// pixel coordinates (x + 0.5 at a pixel centre) -> the kind's space (unit space composed
    /// with `Generator::view`): `[a, b, c, d, e, f]`, u = a x + c y + e, v = b x + d y + f
    pub m: [f64; 6],
    /// parameter values at t, in the kind's declaration order
    pub p: &'a [f64],
    /// palette stops, straight sRGB RGBA, cyclic (at least one)
    pub pal: &'a [[f32; 4]],
    /// supersampling: samples per pixel axis
    pub aa: u32,
    pub seed: u64,
    pub t: f64,
    /// f32 math (GPU conformance); else f64
    pub f32: bool,
    /// rect width / height: unit space spans u in +-aspect, v in +-1
    pub aspect: f64,
}

/// The default CPU image: `aa x aa` samples per pixel, averaged premultiplied, dithered to u8.
pub fn shade<G: Generator + ?Sized>(imp: &G, g: &Input, out: &mut [u8]) {
    use rayon::prelude::*;
    let aa = g.aa.max(1);
    let inv = 1.0 / (aa * aa) as f32;
    let m = g.m;
    let mf = m.map(|v| v as f32);
    out.par_chunks_mut(g.w * 4).enumerate().take(g.h).for_each(|(y, row)| {
        for x in 0..g.w {
            let mut acc = [0f32; 4];
            for sy in 0..aa {
                for sx in 0..aa {
                    let px = x as f32 + (sx as f32 + 0.5) / aa as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / aa as f32;
                    let (u, v) = if g.f32 {
                        (mf[0].mul_add(px, mf[2].mul_add(py, mf[4])) as f64, mf[1].mul_add(px, mf[3].mul_add(py, mf[5])) as f64)
                    } else {
                        let (px, py) = (px as f64, py as f64);
                        (m[0] * px + m[2] * py + m[4], m[1] * px + m[3] * py + m[5])
                    };
                    let c = imp.sample(g, u, v);
                    acc[0] += c[0] * c[3];
                    acc[1] += c[1] * c[3];
                    acc[2] += c[2] * c[3];
                    acc[3] += c[3];
                }
            }
            let d = crate::look::unit(crate::look::hash(g.org.0 + x as i32, g.org.1 + y as i32, g.seed as u32));
            let o = &mut row[x * 4..x * 4 + 4];
            for k in 0..4 {
                o[k] = (acc[k] * inv * 255.0 + d).clamp(0.0, 255.0) as u8;
            }
        }
    });
}

/// Cyclic palette lookup (bitwise what `palette` in common.wgsl computes).
pub fn palette(pal: &[[f32; 4]], s: f32) -> [f32; 4] {
    let n = pal.len();
    let x = (s - s.floor()) * n as f32;
    let i = (x.floor() as usize).min(n - 1);
    let f = x - x.floor();
    let (a, b) = (pal[i], pal[(i + 1) % n]);
    [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * f)
}

/// f32 / f64 for kinds that run in both precisions.
pub trait Real: Copy + PartialOrd + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> + Neg<Output = Self> {
    fn of(v: f64) -> Self;
    fn f64(self) -> f64;
    fn log2(self) -> Self;
    /// `self * b + c`: fused in f32 (the kernels use WGSL `fma`, so chaotic orbits match the
    /// GPU bit for bit), plain in f64 (no FMA hardware on every CPU; the f64 path is the
    /// reference, not a GPU mirror).
    fn fma(self, b: Self, c: Self) -> Self;
    fn floor(self) -> Self;
    /// `floor` as a lattice index (WGSL `i32(floor(x))`).
    fn cell(self) -> i32;
    fn sqrt(self) -> Self;
    fn abs(self) -> Self;
    fn clamp01(self) -> Self {
        if self < Self::of(0.0) { Self::of(0.0) } else if self > Self::of(1.0) { Self::of(1.0) } else { self }
    }
    fn fract(self) -> Self {
        self - self.floor()
    }
}
impl Real for f32 {
    fn of(v: f64) -> Self { v as f32 }
    fn f64(self) -> f64 { self as f64 }
    fn log2(self) -> Self { f32::log2(self) }
    fn fma(self, b: Self, c: Self) -> Self { self.mul_add(b, c) }
    fn floor(self) -> Self { f32::floor(self) }
    fn cell(self) -> i32 { f32::floor(self) as i32 }
    fn sqrt(self) -> Self { f32::sqrt(self) }
    fn abs(self) -> Self { f32::abs(self) }
}
impl Real for f64 {
    fn of(v: f64) -> Self { v }
    fn f64(self) -> f64 { self }
    fn log2(self) -> Self { f64::log2(self) }
    fn fma(self, b: Self, c: Self) -> Self { self * b + c }
    fn floor(self) -> Self { f64::floor(self) }
    fn cell(self) -> i32 { f64::floor(self) as i32 }
    fn sqrt(self) -> Self { f64::sqrt(self) }
    fn abs(self) -> Self { f64::abs(self) }
}

/// Smooth 2D gradient noise in about [-1, 1] at lattice spacing 1 (quintic fade, gradients
/// from the integer `look::hash`, so f32 CPU and GPU see the same lattice). `gnoise` in
/// common.wgsl.
pub fn gnoise<T: Real>(x: T, y: T, seed: u32) -> T {
    let (xf, yf) = (x.floor(), y.floor());
    let (ix, iy) = (x.cell(), y.cell());
    let (tx, ty) = (x - xf, y - yf);
    let one = T::of(1.0);
    let corner = |cx: i32, cy: i32, dx: T, dy: T| {
        let h = crate::look::hash(ix + cx, iy + cy, seed);
        let gx = T::of((h & 0xffff) as f64).fma(T::of(1.0 / 32768.0), -one);
        let gy = T::of((h >> 16) as f64).fma(T::of(1.0 / 32768.0), -one);
        gx.fma(dx, gy * dy)
    };
    let fade = |t: T| t * t * t * t.fma(t.fma(T::of(6.0), T::of(-15.0)), T::of(10.0));
    let (sx, sy) = (fade(tx), fade(ty));
    let a = corner(0, 0, tx, ty);
    let b = corner(1, 0, tx - one, ty);
    let c = corner(0, 1, tx, ty - one);
    let d = corner(1, 1, tx - one, ty - one);
    let ab = (b - a).fma(sx, a);
    let cd = (d - c).fma(sx, c);
    (cd - ab).fma(sy, ab)
}

/// Fractal sum of `gnoise`: `oct` octaves, each rotated and scaled by 2 (gain 0.5), about
/// [-1, 1]. `fbm` in common.wgsl.
pub fn fbm<T: Real>(mut x: T, mut y: T, oct: u32, seed: u32) -> T {
    let (mut sum, mut amp) = (T::of(0.0), T::of(0.5));
    for o in 0..oct {
        sum = amp.fma(gnoise(x, y, seed.wrapping_add(o.wrapping_mul(7919))), sum);
        let nx = T::of(1.6).fma(x, T::of(1.2) * y);
        let ny = T::of(-1.2).fma(x, T::of(1.6) * y);
        x = nx;
        y = ny;
        amp = amp * T::of(0.5);
    }
    sum * T::of(1.6)
}

/// Escape-time iteration of z -> z^2 + c: the smooth (continuous) escape count
/// `n + 1 - log2(log2 |z|)`, None inside (no escape within `iter`). `escape` in common.wgsl.
pub fn escape<T: Real>(mut zr: T, mut zi: T, cr: T, ci: T, iter: u32, bail2: T) -> Option<f32> {
    for n in 0..iter {
        let (r2, i2) = (zr * zr, zi * zi);
        if r2 + i2 > bail2 {
            let lz = (r2 + i2).log2() * T::of(0.5);
            return Some((T::of(n as f64 + 1.0) - lz.log2()).f64() as f32);
        }
        zi = (zr + zr).fma(zi, ci);
        zr = r2 - i2 + cr;
    }
    None
}

/// Colour of a smooth escape count: palette at `offset + speed * log2(1 + mu) / 2`, opaque
/// black inside. `fractal_color` in common.wgsl.
pub fn fractal_color(pal: &[[f32; 4]], mu: Option<f32>, speed: f32, offset: f32) -> [f32; 4] {
    match mu {
        Some(mu) => palette(pal, offset + speed * (1.0 + mu.max(0.0)).log2() * 0.5),
        None => [0.0, 0.0, 0.0, 1.0],
    }
}

/// The fractal view: unit point (u, v) -> complex plane, centred at (re, im), rotated by
/// `turn` degrees, `half` units from the centre to the top edge at zoom 1 (imaginary axis up).
pub fn fractal_view(re: f64, im: f64, zoom: f64, depth: f64, turn: f64, half: f64) -> [f64; 6] {
    let s = half / (zoom * depth.exp2());
    let (sn, cs) = turn.to_radians().sin_cos();
    [s * cs, s * sn, s * sn, -s * cs, re, im]
}

/// Affine `a` after `b` (both `[a, b, c, d, e, f]`).
pub fn compose(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

/// A prepared generator node.
pub struct Prep {
    pub c: GenContent,
    pub kind: &'static Kind,
    pub imp: &'static dyn Generator,
    base: Vec<f64>,
    props: Vec<String>,
    pal: Vec<[f32; 4]>,
}

impl Prep {
    pub fn new(c: &GenContent) -> anyhow::Result<Prep> {
        let kind = agentfx_ir::generator::kind(&c.kind).ok_or_else(|| anyhow::anyhow!("unknown generator kind {:?}", c.kind))?;
        let imp = lookup(&c.kind).ok_or_else(|| anyhow::anyhow!("generator kind {:?} is not implemented by this renderer", c.kind))?;
        let base = kind.params.iter().map(|p| c.params.get(p.name).copied().unwrap_or(p.default)).collect();
        let props = kind.params.iter().map(|p| format!("gen.{}", p.name)).collect();
        let pal = if c.palette.is_empty() { agentfx_ir::generator::palette("neon").unwrap_or_default() } else { c.palette.clone() };
        let pal = pal.iter().map(|s| s.map(|v| v as f32)).collect();
        Ok(Prep { c: c.clone(), kind, imp, base, props, pal })
    }

    /// Parameter values at t (tracks over the base values, clamped to the declared ranges).
    pub fn params(&self, scene: &Scene, tracks: &[usize], t: f64) -> Vec<f64> {
        self.kind.params.iter().enumerate().map(|(i, p)| crate::schedule::number_at(scene, tracks, &self.props[i], t, self.base[i]).clamp(p.min, p.max)).collect()
    }

    /// Does a track animate a parameter?
    pub fn animated(&self, scene: &Scene, tracks: &[usize]) -> bool {
        self.kind.time || tracks.iter().any(|&i| scene.tracks[i].property.starts_with("gen."))
    }

    /// Frame signature: equal signatures draw equal images.
    pub fn sig(&self, scene: &Scene, tracks: &[usize], t: f64) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in self.params(scene, tracks, t) {
            v.to_bits().hash(&mut h);
        }
        if self.kind.time {
            (t + self.c.time_offset).to_bits().hash(&mut h);
        }
        h.finish()
    }

    /// Render `w x h` pixels (premultiplied RGBA8 into `out`) with the GPU kernel when the GPU
    /// path is on and the buffer is large enough, else on the CPU.
    #[allow(clippy::too_many_arguments)]
    pub fn render(&self, p: &[f64], t: f64, w: usize, h: usize, org: (i32, i32), m: [f64; 6], out: &mut [u8]) {
        let aa = self.kind.index("aa").map_or(1, |i| p[i].round() as u32).clamp(1, 4);
        let g = Input { w, h, org, m: compose(self.imp.view(p), m), p, pal: &self.pal, aa, seed: self.c.seed, t: t + self.c.time_offset, f32: false, aspect: self.c.rect[2] / self.c.rect[3].max(1e-9) };
        if let (Some(src), Some(gpu)) = (self.imp.wgsl().filter(|_| self.kind.gpu), crate::gpu::get()) {
            if w * h >= crate::gpu::min_px() {
                let t0 = std::time::Instant::now();
                match run_gpu(gpu, self.kind.name, src, &g, out) {
                    Ok(()) => {
                        crate::gpu::account(1, w * h, t0);
                        return;
                    }
                    Err(e) => {
                        if crate::gpu::FAILED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3 {
                            eprintln!("agentfx-render: GPU generator {} failed ({e:#}); CPU fallback", self.kind.name);
                        }
                    }
                }
            } else {
                crate::gpu::CPU_ROUTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        self.imp.cpu(&g, out);
    }
}

/// Run a kind's kernel: header words 0 w, 1 h, 2-3 org (i32), 4-9 m (f32), 10 aa, 11 palette
/// stops, 12 seed, 13 t; parameters from word 16 (f32). Bindings: 1 palette (vec4 f32), 2 out.
pub fn run_gpu(gpu: &'static crate::gpu::Gpu, name: &'static str, src: &'static str, g: &Input, out: &mut [u8]) -> anyhow::Result<()> {
    use crate::gpu::{Job, Params};
    let n = g.w * g.h * 4;
    anyhow::ensure!(gpu.fits(n as u64), "{}x{} does not fit the device", g.w, g.h);
    let mut p = Params::default();
    p.u(0, g.w as u32).u(1, g.h as u32).i(2, g.org.0).i(3, g.org.1);
    for (i, v) in g.m.iter().enumerate() {
        p.f(4 + i, *v as f32);
    }
    p.u(10, g.aa).u(11, g.pal.len() as u32).u(12, g.seed as u32).f(13, g.t as f32);
    for (i, v) in g.p.iter().enumerate() {
        p.f(16 + i, *v as f32);
    }
    let pal: Vec<f32> = g.pal.iter().flatten().copied().collect();
    let mut job = Job::new(gpu);
    let pb = job.upload(bytemuck::cast_slice(&pal));
    let ob = gpu.buffer(n as u64);
    job.pass(name, src, 2, "gen_main", &p, &[&pb, &ob], [g.w as u32, g.h as u32]);
    job.read(&ob, n);
    let r = job.finish(&mut [&mut out[..n]]);
    gpu.give(pb);
    gpu.give(ob);
    r
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A `w x h` input of `kind` with `params` overriding the defaults: the unit square
    /// mapping of an unrotated rect.
    pub fn input<'a>(kind: &Kind, p: &'a mut Vec<f64>, pal: &'a [[f32; 4]], over: &[(&str, f64)], w: usize, h: usize) -> Input<'a> {
        *p = kind.params.iter().map(|q| over.iter().find(|o| o.0 == q.name).map_or(q.default, |o| o.1)).collect();
        let s = 2.0 / h as f64;
        let aa = kind.index("aa").map_or(1, |i| p[i] as u32);
        let m = compose(lookup(kind.name).unwrap().view(p), [s, 0.0, 0.0, s, -(w as f64) / h as f64, -1.0]);
        Input { w, h, org: (0, 0), m, p, pal, aa, seed: 7, t: 0.0, f32: false, aspect: w as f64 / h as f64 }
    }

    fn pal() -> Vec<[f32; 4]> {
        agentfx_ir::generator::palette("neon").unwrap().iter().map(|s| s.map(|v| v as f32)).collect()
    }

    /// Every declared kind is registered here, and every registered kind is declared.
    #[test]
    fn registry_matches_declarations() {
        for k in agentfx_ir::generator::KINDS {
            assert!(lookup(k.name).is_some(), "{} declared but not implemented", k.name);
            assert!(!k.gpu || lookup(k.name).unwrap().wgsl().is_some(), "{} declares gpu without a kernel", k.name);
        }
    }

    /// Same parameters -> identical bytes (any thread count), and a parameter change changes them.
    #[test]
    fn deterministic() {
        use std::hash::{Hash, Hasher};
        let pal = pal();
        for k in agentfx_ir::generator::KINDS {
            let mut p = vec![];
            let g = input(k, &mut p, &pal, &[], 96, 64);
            let imp = lookup(k.name).unwrap();
            let mut a = vec![0u8; 96 * 64 * 4];
            imp.cpu(&g, &mut a);
            let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
            let mut b = vec![0u8; a.len()];
            pool.install(|| imp.cpu(&g, &mut b));
            let hash = |v: &[u8]| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                v.hash(&mut h);
                h.finish()
            };
            assert_eq!(hash(&a), hash(&b), "{}", k.name);
            let mut p2 = vec![];
            let off = k.index("color-offset").unwrap();
            let mut g2 = input(k, &mut p2, &pal, &[], 96, 64);
            let mut q = g2.p.to_vec();
            q[off] += 0.25;
            g2.p = &q;
            let mut c = vec![0u8; a.len()];
            imp.cpu(&g2, &mut c);
            assert_ne!(hash(&a), hash(&c), "{}", k.name);
        }
    }

    /// Through the painter: a `gen.*` track changes frames, a frame renders identically twice,
    /// and a rotated, scaled generator node renders (clipped to its rect).
    #[test]
    fn painter_frames() {
        use crate::vraster::{Info, Painter, Prepared};
        let scene: Scene = serde_json::from_value(serde_json::json!({"version": 1, "width": 96, "height": 64, "fps": 10, "frames": 10,
            "nodes": [{"name": "root", "children": [1, 2]},
                {"name": "g", "children": [], "content": {"type": "gen", "kind": "julia", "rect": [0, 0, 96, 64], "params": {"iter": 80}, "seed": 3}},
                {"name": "small", "children": [], "origin": [70, 50], "transform": [{"rotate": 30}, {"scale": [0.5, 0.5]}], "content": {"type": "gen", "kind": "mandelbrot", "rect": [60, 40, 20, 20], "palette": [[1, 1, 1, 1], [0, 0, 1, 1]]}}],
            "tracks": [{"node": 1, "property": "gen.cx", "delay": 0, "duration": 1, "fill": "both", "easing": "linear", "keyframes": [{"offset": 0, "value": {"number": -0.8}}, {"offset": 1, "value": {"number": -0.7}}]}]})).unwrap();
        agentfx_ir::validate::validate(&scene, std::path::Path::new(".")).unwrap();
        let prep = Prepared::new(&scene, std::path::Path::new("."), None, None).unwrap();
        let info = Info::new(&scene, &prep);
        let sched = crate::schedule::build(&scene, &info).unwrap();
        let frame = |f: usize| Painter::new(&scene, &prep, &sched, &info, 0).render_rect(f, [0, 0, 96, 64]).unwrap().data_as_u8_slice().to_vec();
        let (a, b, c) = (frame(2), frame(2), frame(7));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.chunks(4).all(|p| p[3] == 255), "the julia layer covers the frame");
    }

    /// (max channel diff, share of channels off by more than 1)
    fn diff(a: &[u8], b: &[u8]) -> (u8, f64) {
        let mx = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        let off = a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > 1).count();
        (mx, off as f64 / a.len() as f64)
    }

    /// GPU kernels match the f32 CPU reference within 1/255 per channel (llvmpipe in CI).
    #[test]
    fn gpu_matches_cpu() {
        let Some(gpu) = crate::gpu::fx::tests::device() else { return };
        let pal = pal();
        let cases: &[(&str, &[(&str, f64)])] = &[
            ("julia", &[]),
            ("julia", &[("cx", -0.4), ("cy", 0.6), ("zoom", 1.6), ("turn", 30.0), ("iter", 120.0), ("color-offset", 0.3), ("aa", 1.0)]),
            ("julia", &[("cx", 0.285), ("cy", 0.01), ("color-speed", 2.5), ("bailout", 8.0)]),
            ("mandelbrot", &[]),
            ("mandelbrot", &[("re", -0.7436), ("im", 0.1318), ("zoom", 40.0), ("iter", 400.0), ("aa", 1.0)]),
            ("mandelbrot", &[("re", -0.16), ("im", 1.035), ("zoom", 6.0), ("depth", 1.5), ("turn", -20.0), ("color-speed", 0.5)]),
            ("noise", &[]),
            ("noise", &[("re", -3.5), ("im", 12.25), ("zoom", 0.4), ("turn", 37.0), ("warp", 9.0), ("octaves", 7.0), ("color-speed", 2.5), ("aa", 1.0)]),
            ("noise", &[("warp", 0.0), ("octaves", 1.0), ("flow", 3.0), ("color-offset", 0.6)]),
            ("truchet", &[]),
            ("truchet", &[("phase", 0.45), ("mode", 1.0), ("zoom", 0.6), ("turn", 15.0), ("thickness", 0.03), ("aa", 1.0)]),
            ("truchet", &[("phase", -2.7), ("re", 3.3), ("im", -7.1), ("color-speed", 4.0), ("color-offset", 0.2)]),
            ("voronoi", &[]),
            ("voronoi", &[("mode", 1.0), ("zoom", 0.5), ("turn", -25.0), ("flow", 0.7), ("aa", 1.0)]),
            ("voronoi", &[("mode", 2.0), ("jitter", 0.6), ("re", -4.5), ("im", 2.2), ("color-speed", 2.0), ("thickness", 0.1)]),
        ];
        let (w, h) = (160, 90);
        for (name, over) in cases {
            let k = agentfx_ir::generator::kind(name).unwrap();
            let imp = lookup(name).unwrap();
            let mut p = vec![];
            let mut g = input(k, &mut p, &pal, over, w, h);
            g.f32 = true;
            g.t = 1.75; // time-dependent kinds move; the others ignore it
            let mut gp = vec![0u8; w * h * 4];
            run_gpu(gpu, k.name, imp.wgsl().unwrap(), &g, &mut gp).unwrap();
            let mut cpu = vec![0u8; w * h * 4];
            imp.cpu(&g, &mut cpu);
            let (mx, off) = diff(&cpu, &gp);
            eprintln!("generator {name} {over:?}: max diff {mx}, {:.4}% channels off by > 1", off * 100.0);
            assert!(mx <= 1, "{name} {over:?}: max diff {mx}");
        }
    }
}
