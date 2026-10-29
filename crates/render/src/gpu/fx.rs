//! GPU versions of node effects (`vraster::effect_pixmap`), as a `Chain`: consecutive GPU
//! effects of one node run on a frame that stays on the GPU (one upload before the first,
//! one readback after the last; phase 2 step 1). A step returns false when the effect
//! should run on the CPU (GPU off, buffer below the routing threshold, effect not ported
//! or not enabled, limits exceeded): the chain is flushed first, so the CPU sees current
//! pixels. A GPU error at the flush replays the chain's effects on the CPU (the host
//! buffer still holds the chain's input), counted in stats.
use super::{Gpu, Job, Params};
use crate::look::{FxCtx, NPARAM, TIME};
use agentfx_ir::Effect;
use std::sync::atomic::Ordering;
use std::time::Instant;

const BLUR: &str = include_str!("blur.wgsl");
const RISO: &str = include_str!("riso.wgsl");
const HALFTONE: &str = include_str!("halftone.wgsl");
const LIGHT: &str = include_str!("light.wgsl");
const COMP: &str = include_str!("comp.wgsl");
const SHADOW: &str = include_str!("shadow.wgsl");

/// Is this look/light effect ported (whatever the routing)?
pub fn ported(e: &Effect) -> bool {
    matches!(e, Effect::Riso { .. } | Effect::Halftone { .. } | Effect::Shadow { .. } | Effect::Rays { .. })
}

fn fx_name(e: &Effect) -> &'static str {
    match e {
        Effect::Riso { .. } => "riso",
        Effect::Halftone { .. } => "halftone",
        Effect::Shadow { .. } => "shadow",
        Effect::Rays { .. } => "rays",
        Effect::Blur { .. } => "blur",
        Effect::DropShadow { .. } => "drop-shadow",
        Effect::Glow { .. } => "glow",
        _ => "",
    }
}

/// `AGENTFX_GPU_FX=riso,halftone,shadow,rays,blur,drop-shadow,glow` picks the effects that
/// always use the GPU (a chain starts for them). Default: riso, halftone, rays, shadow
/// (a shadow outside a chain sends only the alpha up and the occlusion back). The others
/// are cheap on the CPU next to a frame upload + readback, so by default they run on the
/// GPU only when the frame is already there (they join an active chain): see `light`.
fn enabled(name: &str) -> bool {
    static ON: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let list = ON.get_or_init(|| agentfx_ir::env::var("AGENTFX_GPU_FX").unwrap_or_else(|_| "riso,halftone,rays,shadow".into()).split(',').map(|s| s.trim().to_string()).collect());
    list.iter().any(|x| x == name)
}

/// Effects that join an active chain even when not `enabled` (`AGENTFX_GPU_JOIN=0`: never).
fn joins(name: &str) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| agentfx_ir::env::var("AGENTFX_GPU_JOIN").map_or(true, |v| v != "0")) && matches!(name, "blur" | "drop-shadow" | "glow")
}

/// One effect of a chain, kept for the CPU replay (fallback, `AGENTFX_GPU_CHECK`).
#[derive(Clone)]
enum Op {
    Look(Effect, [f64; NPARAM], FxCtx),
    Blur(f64),
    DropShadow([f64; 4], f64, i64, i64),
    Glow([f64; 4], f64, f64),
}

impl Op {
    fn name(&self) -> &'static str {
        match self {
            Op::Look(e, ..) => fx_name(e),
            Op::Blur(..) => "blur",
            Op::DropShadow(..) => "drop-shadow",
            Op::Glow(..) => "glow",
        }
    }

    fn cpu(&self, buf: &mut [u8], w: usize, h: usize) {
        match self {
            Op::Look(e, p, cx) => crate::look::apply(e, p, buf, w, h, cx),
            Op::Blur(sigma) => crate::effects::blur(buf, w, h, *sigma),
            Op::DropShadow(c, sigma, dx, dy) => crate::effects::drop_shadow(buf, w, h, *c, *sigma, *dx, *dy),
            Op::Glow(c, k, sigma) => crate::effects::glow(buf, w, h, *c, *k, *sigma),
        }
    }

    /// Needs the current pixels on the host (a CPU part depends on them).
    fn needs_host(&self) -> bool {
        false
    }

    /// Can the GPU run it (exactly)?
    fn gpu_ok(&self) -> bool {
        match self {
            Op::Blur(s) | Op::DropShadow(_, s, ..) | Op::Glow(_, _, s) => blur_gpu_ok(*s),
            Op::Look(Effect::Halftone { .. }, p, cx) => blur_gpu_ok(halftone_sigma(p[0], cx.s)),
            _ => true,
        }
    }

    /// Largest storage binding (bytes) for a w x h buffer.
    fn max_binding(&self, w: usize, h: usize) -> u64 {
        match self {
            Op::Look(Effect::Riso { .. } | Effect::Halftone { .. }, ..) => (w * h * 16) as u64,
            _ => (w * h * 4) as u64,
        }
    }
}

/// A frame resident on the GPU with the passes recorded so far.
struct Active {
    g: &'static Gpu,
    job: Job<'static>,
    px: wgpu::Buffer,
    w: usize,
    h: usize,
    ops: Vec<Op>,
    temps: Vec<wgpu::Buffer>,
    t: Instant,
    input: Option<Vec<u8>>,
}

impl Active {
    fn temp(&mut self, size: u64) -> wgpu::Buffer {
        let b = self.g.buffer(size);
        self.temps.push(b.clone());
        b
    }

    fn upload(&mut self, data: &[u8]) -> wgpu::Buffer {
        let b = self.job.upload(data);
        self.temps.push(b.clone());
        b
    }
}

/// The GPU side of one node's effect list (see the module doc). Use: one `Chain` per
/// effect list; `look` (etc.) per effect; `flush` before any CPU code that reads the
/// buffer outside the chain's steps and after the last effect.
#[derive(Default)]
pub struct Chain {
    a: Option<Active>,
    /// tests / benchmarks: this device, every ported effect, no size routing
    forced: Option<&'static Gpu>,
}

impl Chain {
    pub fn on(g: &'static Gpu) -> Chain {
        Chain { a: None, forced: Some(g) }
    }

    /// Look / light effect `e` on the GPU; false: run it on the CPU (buffer current).
    pub fn look(&mut self, e: &Effect, p: &[f64; NPARAM], buf: &mut [u8], w: usize, h: usize, cx: &FxCtx) -> bool {
        if !ported(e) {
            self.flush(buf);
            return false;
        }
        if let (Effect::Shadow { color, .. }, None) = (e, &self.a) {
            // not on the GPU yet: only the alpha goes up and the occlusion comes back
            if let Some(g) = self.device_for("shadow", w, h).filter(|g| g.fits((w * h * 4) as u64)) {
                let _p = crate::prof::scope("gpu.fx");
                let t = Instant::now();
                let check = agentfx_ir::env::var("AGENTFX_GPU_CHECK").is_ok().then(|| buf.to_vec());
                match shadow_host(g, buf, w, h, cx, p, *color) {
                    Ok(()) => {
                        super::account(1, w * h, t);
                        if let Some(cpu) = check {
                            check_report(&[Op::Look(e.clone(), *p, *cx)], cpu, buf, w, h);
                        }
                        return true;
                    }
                    Err(err) => {
                        fail(&err);
                        return false;
                    }
                }
            }
        }
        self.step(Op::Look(e.clone(), *p, *cx), buf, w, h)
    }

    /// effects::blur on the GPU; false: run it on the CPU (buffer current).
    pub fn blur(&mut self, buf: &mut [u8], w: usize, h: usize, sigma: f64) -> bool {
        self.step(Op::Blur(sigma), buf, w, h)
    }

    /// effects::drop_shadow on the GPU; false: run it on the CPU (buffer current).
    #[allow(clippy::too_many_arguments)]
    pub fn drop_shadow(&mut self, buf: &mut [u8], w: usize, h: usize, color: [f64; 4], sigma: f64, dx: i64, dy: i64) -> bool {
        self.step(Op::DropShadow(color, sigma, dx, dy), buf, w, h)
    }

    /// effects::glow on the GPU; false: run it on the CPU (buffer current).
    pub fn glow(&mut self, buf: &mut [u8], w: usize, h: usize, color: [f64; 4], strength: f64, sigma: f64) -> bool {
        self.step(Op::Glow(color, strength, sigma), buf, w, h)
    }

    fn device_for(&self, name: &str, w: usize, h: usize) -> Option<&'static Gpu> {
        if let Some(g) = self.forced {
            return Some(g);
        }
        if w == 0 || h == 0 {
            return None;
        }
        if !enabled(name) {
            // cheap effects ride along on a frame that is already on the GPU
            return self.a.as_ref().filter(|a| joins(name) && a.w == w && a.h == h).map(|a| a.g);
        }
        let g = super::get()?;
        if w * h < super::min_px() {
            super::CPU_ROUTED.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(g)
    }

    fn step(&mut self, op: Op, buf: &mut [u8], w: usize, h: usize) -> bool {
        let Some(g) = self.device_for(op.name(), w, h).filter(|_| op.gpu_ok()) else {
            self.flush(buf);
            return false;
        };
        let _p = crate::prof::scope("gpu.fx");
        if !g.fits(op.max_binding(w, h)) || self.a.as_ref().is_some_and(|a| a.w != w || a.h != h || !std::ptr::eq(a.g, g)) || (op.needs_host() && self.a.is_some()) {
            self.flush(buf);
            if !g.fits(op.max_binding(w, h)) {
                return false;
            }
        }
        let fresh = self.a.is_none();
        if fresh {
            let mut job = Job::new(g);
            let px = job.upload(buf);
            let input = agentfx_ir::env::var("AGENTFX_GPU_CHECK").is_ok().then(|| buf.to_vec());
            self.a = Some(Active { g, job, px, w, h, ops: Vec::new(), temps: Vec::new(), t: Instant::now(), input });
        }
        let a = self.a.as_mut().unwrap();
        let host: Option<&[u8]> = if fresh { Some(buf) } else { None };
        match encode(a, &op, host) {
            Ok(()) => {
                a.ops.push(op);
                true
            }
            Err(err) => {
                // nothing of this op is usable: drop the chain, replay its ops on the CPU
                let a = self.a.take().unwrap();
                fail(&err);
                for o in &a.ops {
                    o.cpu(buf, w, h);
                }
                give_all(a.g, a.temps, a.px);
                false
            }
        }
    }

    /// Bring the buffer up to date (read the frame back; on a GPU error replay on the CPU).
    pub fn flush(&mut self, buf: &mut [u8]) {
        if let Err(err) = self.finish(buf) {
            fail(&err);
        }
    }

    /// `flush` without the CPU replay on error (tests).
    pub fn finish(&mut self, buf: &mut [u8]) -> anyhow::Result<()> {
        let Some(a) = self.a.take() else { return Ok(()) };
        let _p = crate::prof::scope("gpu.fx");
        let Active { g, mut job, px, w, h, ops, temps, t, input } = a;
        job.read(&px, buf.len());
        let r = job.finish(&mut [buf]);
        give_all(g, temps, px);
        match r {
            Ok(()) => {
                super::account(ops.len(), w * h, t);
                if let Some(cpu) = input {
                    check_report(&ops, cpu, buf, w, h);
                }
                Ok(())
            }
            Err(err) => {
                if self.forced.is_none() {
                    for o in &ops {
                        o.cpu(buf, w, h);
                    }
                }
                Err(err)
            }
        }
    }
}

impl Drop for Chain {
    fn drop(&mut self) {
        debug_assert!(self.a.is_none(), "gpu::fx::Chain dropped without flush");
    }
}

fn give_all(g: &Gpu, temps: Vec<wgpu::Buffer>, px: wgpu::Buffer) {
    for b in temps {
        g.give(b);
    }
    g.give(px);
}

fn fail(err: &anyhow::Error) {
    if super::FAILED.fetch_add(1, Ordering::Relaxed) < 3 {
        eprintln!("agentfx-render: GPU effect failed, CPU fallback: {err:#}");
    }
}

/// AGENTFX_GPU_CHECK=1: run the chain's effects on the CPU too and report the difference.
fn check_report(ops: &[Op], mut cpu: Vec<u8>, gpu: &[u8], w: usize, h: usize) {
    let src = cpu.clone();
    for o in ops {
        o.cpu(&mut cpu, w, h);
    }
    let (mut mx, mut at, mut over) = (0u8, 0usize, 0usize);
    for (i, (a, b)) in cpu.iter().zip(gpu).enumerate() {
        let d = a.abs_diff(*b);
        over += (d > 2) as usize;
        if d > mx {
            mx = d;
            at = i / 4;
        }
    }
    let names: Vec<&str> = ops.iter().map(|o| o.name()).collect();
    eprintln!(
        "agentfx-render gpu-check {} {w}x{h}: max {mx}, >2: {over}{}",
        names.join("+"),
        if mx > 2 { format!(" worst ({}, {}) cpu {:?} gpu {:?} src {:?}", at % w, at / w, &cpu[at * 4..at * 4 + 4], &gpu[at * 4..at * 4 + 4], &src[at * 4..at * 4 + 4]) } else { String::new() }
    );
}

/// Record `op`'s passes on the resident frame. `host`: the current pixels when the chain
/// starts with this op (required when `op.needs_host()`).
fn encode(a: &mut Active, op: &Op, host: Option<&[u8]>) -> anyhow::Result<()> {
    match op {
        Op::Look(e, p, cx) => match e {
            Effect::Riso { inks, seed, drift, drift_speed, grain, density, paper, .. } => riso(a, cx, &crate::look::RisoCfg { inks, seed: *seed, misreg: p[0], drift: *drift, drift_speed: *drift_speed, t: p[TIME], grain: *grain, density: *density, paper: *paper }),
            Effect::Halftone { mode, ink, paper, .. } => halftone(a, cx, p[0], p[1], mode == "cmyk", *ink, *paper),
            Effect::Shadow { color, .. } => shadow(a, cx, p, *color),
            Effect::Rays { color, .. } => rays(a, cx, p, *color),
            _ => anyhow::bail!("not ported"),
        },
        Op::Blur(sigma) => {
            let px = a.px.clone();
            blur_any(a, &px, *sigma);
            Ok(())
        }
        Op::DropShadow(c, sigma, dx, dy) => under_tinted(a, *c, 1.0, *sigma, *dx, *dy),
        Op::Glow(c, k, sigma) => under_tinted(a, *c, *k, *sigma, 0, 0),
    }
}

/// Run one effect on a given device regardless of mode/routing (tests, benchmarks).
pub fn apply_on(g: &'static Gpu, e: &Effect, p: &[f64; NPARAM], buf: &mut [u8], w: usize, h: usize, cx: &FxCtx) -> anyhow::Result<bool> {
    let mut c = Chain::on(g);
    if !c.look(e, p, buf, w, h, cx) {
        return Ok(false);
    }
    c.finish(buf)?;
    Ok(true)
}

fn rgb(c: [f64; 4]) -> [f32; 3] {
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

// ---------------------------------------------------------------- blur

/// effects::blur's reduction factor for `sigma`.
fn blur_scale(sigma: f64) -> usize {
    let mut scale = 1usize;
    while scale < (1 << 20) && sigma / (scale * 2) as f64 >= (3.5_f64.powi(2) + 0.25).sqrt() {
        scale *= 2;
    }
    scale
}

/// The GPU blur matches effects::blur exactly up to this reduction (f32 upsample).
fn blur_gpu_ok(sigma: f64) -> bool {
    sigma.is_finite() && blur_scale(sigma) <= 128
}

/// effects::blur's full-resolution box path applies (sigma below the downsampling limit).
fn blur_full_res(sigma: f64) -> bool {
    sigma / 2.0 < (3.5_f64.powi(2) + 0.25).sqrt()
}

/// effects::blur's box passes for the full-resolution case, as passes of `job` on `a`
/// (result in `a`; `tmp` same size). None when sigma needs the downsampled path.
fn blur_passes(job: &mut Job, a: &wgpu::Buffer, tmp: &wgpu::Buffer, w: usize, h: usize, sigma: f64) -> Option<()> {
    if sigma < 0.2 {
        return Some(());
    }
    if !blur_full_res(sigma) {
        return None;
    }
    let bx = crate::effects::boxes(sigma);
    let mut src = a;
    let mut dst = tmp;
    for entry in ["box_h", "box_v"] {
        for b in bx {
            let r = ((b - 1) / 2) as u32;
            let norm = if r == 0 { 0 } else { ((1u64 << 32) / (2 * r as u64 + 1)) as u32 };
            let mut p = Params::default();
            p.u(0, w as u32).u(1, h as u32).u(2, r).u(3, norm);
            job.pass("blur", BLUR, 2, entry, &p, &[src, dst], [w as u32, h as u32]);
            std::mem::swap(&mut src, &mut dst);
        }
    }
    // six passes: the result is back in `a`
    Some(())
}

/// effects::blur of resident buffer `b` (a.w x a.h) in place, both paths (full-resolution
/// boxes; downsample, boxes, bilinear upsample), bit-exact.
fn blur_any(a: &mut Active, b: &wgpu::Buffer, sigma: f64) {
    let (w, h) = (a.w, a.h);
    if sigma < 0.2 {
        return;
    }
    if blur_full_res(sigma) {
        let tmp = a.temp((w * h * 4) as u64);
        blur_passes(&mut a.job, b, &tmp, w, h, sigma).expect("full-res blur");
        return;
    }
    let scale = blur_scale(sigma);
    let (sw, sh) = (w.div_ceil(scale), h.div_ceil(scale));
    let small = a.temp((sw * sh * 4) as u64);
    let tmp = a.temp((sw * sh * 4) as u64);
    let mut p = Params::default();
    p.u(0, w as u32).u(1, h as u32).u(8, scale as u32).u(9, sw as u32).u(10, sh as u32);
    a.job.pass("comp", COMP, 2, "down", &p, &[b, &small], [sw as u32, sh as u32]);
    let reduced = ((sigma / scale as f64).powi(2) - 0.25).sqrt();
    blur_passes(&mut a.job, &small, &tmp, sw, sh, reduced).expect("reduced sigma is full-res");
    a.job.pass("comp", COMP, 2, "up", &p, &[b, &small], [w as u32, h as u32]);
}

/// Drop shadow / glow: tinted alpha, blurred, shifted, under the frame.
fn under_tinted(a: &mut Active, color: [f64; 4], strength: f64, sigma: f64, dx: i64, dy: i64) -> anyhow::Result<()> {
    let (w, h) = (a.w, a.h);
    let al = (color[3] * strength).clamp(0.0, 1.0);
    let k = [color[0] * al, color[1] * al, color[2] * al, al].map(|v| (v.clamp(0.0, 1.0) * 65536.0) as u32);
    let mut p = Params::default();
    p.u(0, w as u32).u(1, h as u32).u(2, k[0]).u(3, k[1]).u(4, k[2]).u(5, k[3]).i(6, dx.clamp(-(1 << 30), 1 << 30) as i32).i(7, dy.clamp(-(1 << 30), 1 << 30) as i32);
    let sh = a.temp((w * h * 4) as u64);
    let px = a.px.clone();
    a.job.pass("comp", COMP, 2, "tint", &p, &[&px, &sh], [w as u32, h as u32]);
    blur_any(a, &sh, sigma);
    a.job.pass("comp", COMP, 2, "over_shift", &p, &[&px, &sh], [w as u32, h as u32]);
    Ok(())
}

// ---------------------------------------------------------------- riso

fn riso(a: &mut Active, cx: &FxCtx, cfg: &crate::look::RisoCfg) -> anyhow::Result<()> {
    use crate::look::{fbm, plate_offset};
    let (w, h) = (a.w, a.h);
    let n = cfg.inks.len().clamp(1, 4);
    let inks: Vec<[f32; 3]> = cfg.inks[..n].iter().map(|c| {
        let al = c[3].clamp(0.0, 1.0) as f32;
        let r = rgb(*c);
        [1.0 - al * (1.0 - r[0]), 1.0 - al * (1.0 - r[1]), 1.0 - al * (1.0 - r[2])]
    }).collect();
    let dd: Vec<[f32; 3]> = inks.iter().map(|c| [-(c[0].max(0.03)).ln(), -(c[1].max(0.03)).ln(), -(c[2].max(0.03)).ln()]).collect();
    let mut gm = [[0f32; 4]; 4];
    for i in 0..n {
        for j in 0..n {
            gm[i][j] = (0..3).map(|c| dd[i][c] * dd[j][c]).sum();
        }
    }
    let paper = cfg.paper.map(rgb);
    let seed = cfg.seed as u32 ^ (cfg.seed >> 32) as u32;
    let offs: Vec<(f32, f32)> = (0..n).map(|k| {
        let (dx, dy) = plate_offset(seed, k, cfg.misreg, cfg.drift, cfg.drift_speed, cfg.t);
        ((dx * cx.s) as f32, (dy * cx.s) as f32)
    }).collect();
    let (grain, dens) = (cfg.grain as f32, cfg.density as f32);
    let s = cx.s.max(1e-3) as f32;
    const G: i32 = 8;
    let (l0x, l0y) = (cx.ox - cx.ax, cx.oy - cx.ay);
    let (i0, j0) = (l0x.div_euclid(G), l0y.div_euclid(G));
    let gw = (w as i32 / G + 3) as usize;
    let gh = (h as i32 / G + 3) as usize;
    // coarse density grid on the CPU (small; identical to look::riso)
    let dgrid: Vec<f32> = if dens > 0.0 {
        use rayon::prelude::*;
        (0..n * gw * gh).into_par_iter().map(|q| {
            let k = q / (gw * gh);
            let (gx, gy) = ((q % (gw * gh)) % gw, (q % (gw * gh)) / gw);
            let ps = seed.wrapping_add(k as u32 * 1013);
            fbm(((i0 + gx as i32) * G) as f32 / s, ((j0 + gy as i32) * G) as f32 / s, 160.0, 3, ps ^ 0xA5A5) - 0.5
        }).collect()
    } else {
        vec![0.0]
    };
    let mut p = Params::default();
    p.u(0, w as u32).u(1, h as u32).u(2, n as u32).u(3, paper.is_some() as u32);
    if let Some(pp) = paper {
        p.f(4, pp[0]).f(5, pp[1]).f(6, pp[2]);
    }
    p.f(7, grain).f(8, dens).f(9, s).i(10, l0x).i(11, l0y).i(12, i0).i(13, j0).u(14, gw as u32).u(15, gh as u32).u(16, seed);
    for k in 0..n {
        for c in 0..3 {
            p.f(17 + k * 3 + c, dd[k][c]);
        }
        for j in 0..n {
            p.f(29 + k * 4 + j, gm[k][j]);
        }
        p.f(45 + k * 2, offs[k].0).f(46 + k * 2, offs[k].1);
    }
    p.at(52);
    let planes = a.temp((w * h * 16) as u64);
    let alpha = a.temp((w * h * 4) as u64);
    let grid = a.upload(bytemuck::cast_slice(&dgrid));
    let px = a.px.clone();
    let bufs = [&px, &planes, &alpha, &grid];
    a.job.pass("riso", RISO, 4, "riso_sep", &p, &bufs, [w as u32, h as u32]);
    a.job.pass("riso", RISO, 4, "riso_comp", &p, &bufs, [w as u32, h as u32]);
    Ok(())
}

// ---------------------------------------------------------------- halftone

/// CMYK screens: ink colors and angles relative to `angle - 45` (look::CMYK).
const CMYK: [([f32; 3], f32); 4] = [([0.0, 0.62, 0.89], 15.0), ([0.90, 0.10, 0.52], 75.0), ([1.0, 0.92, 0.05], 0.0), ([0.13, 0.12, 0.14], 45.0)];

fn halftone_sigma(cell: f64, s: f64) -> f64 {
    (((cell * s).max(1.0) as f32) as f64 * 0.33).max(0.3)
}

#[allow(clippy::too_many_arguments)]
fn halftone(a: &mut Active, cx: &FxCtx, cell: f64, angle: f64, cmyk: bool, ink: [f64; 4], paper: Option<[f64; 4]>) -> anyhow::Result<()> {
    let (w, h) = (a.w, a.h);
    let sigma = halftone_sigma(cell, cx.s);
    let cell = (cell * cx.s).max(1.0) as f32;
    let screens: Vec<([f32; 3], f32)> = if cmyk {
        CMYK.iter().map(|(c, an)| (*c, an + angle as f32 - 45.0)).collect()
    } else {
        let al = ink[3].clamp(0.0, 1.0) as f32;
        let r = rgb(ink);
        vec![([1.0 - al * (1.0 - r[0]), 1.0 - al * (1.0 - r[1]), 1.0 - al * (1.0 - r[2])], angle as f32)]
    };
    let paper = paper.map(rgb);
    let trig: Vec<(f32, f32)> = screens.iter().map(|(_, an)| (an.to_radians().cos(), an.to_radians().sin())).collect();
    let (l0x, l0y) = ((cx.ox - cx.ax) as f32, (cx.oy - cx.ay) as f32);
    let mut p = Params::default();
    p.u(0, w as u32).u(1, h as u32).u(2, screens.len() as u32).f(3, cell).f(4, l0x).f(5, l0y).u(6, paper.is_some() as u32);
    if let Some(pp) = paper {
        p.f(7, pp[0]).f(8, pp[1]).f(9, pp[2]);
    }
    p.u(10, cmyk as u32).f(11, 1.0 / std::f32::consts::PI);
    let mut total = 0usize;
    for (si, (ink, _)) in screens.iter().enumerate() {
        // grid extent: identical to look::halftone
        let (co, sn) = trig[si];
        let corners = [(l0x, l0y), (l0x + w as f32, l0y), (l0x, l0y + h as f32), (l0x + w as f32, l0y + h as f32)];
        let us: Vec<f32> = corners.iter().map(|(x, y)| (x * co + y * sn) / cell).collect();
        let vs: Vec<f32> = corners.iter().map(|(x, y)| (-x * sn + y * co) / cell).collect();
        let u0 = us.iter().cloned().fold(f32::INFINITY, f32::min).floor() as i32 - 2;
        let v0 = vs.iter().cloned().fold(f32::INFINITY, f32::min).floor() as i32 - 2;
        let gw = (us.iter().cloned().fold(f32::NEG_INFINITY, f32::max).floor() as i32 + 3 - u0) as usize;
        let gh = (vs.iter().cloned().fold(f32::NEG_INFINITY, f32::max).floor() as i32 + 3 - v0) as usize;
        let b = 16 + si * 12;
        p.f(b, ink[0]).f(b + 1, ink[1]).f(b + 2, ink[2]).f(b + 3, co).f(b + 4, sn).i(b + 5, u0).i(b + 6, v0).u(b + 7, gw as u32).u(b + 8, gh as u32).u(b + 9, total as u32);
        total += gw * gh;
    }
    p.u(12, total as u32);
    p.at(16 + 4 * 12);
    let len = (w * h * 4) as u64;
    let px = a.px.clone();
    // blurred source (both blur paths on the GPU, bit-exact)
    let src = a.temp(len);
    a.job.enc.copy_buffer_to_buffer(&px, 0, &src, 0, len);
    blur_any(a, &src, sigma);
    let tone = a.temp((w * h * 16) as u64);
    let rg = a.temp((total * 4) as u64);
    let bufs = [&px, &src, &tone, &rg];
    a.job.pass("halftone", HALFTONE, 4, "ht_tone", &p, &bufs, [w as u32, h as u32]);
    a.job.pass("halftone", HALFTONE, 4, "ht_grid", &p, &bufs, [total as u32, 0]);
    a.job.pass("halftone", HALFTONE, 4, "ht_final", &p, &bufs, [w as u32, h as u32]);
    Ok(())
}

// ---------------------------------------------------------------- shadow

/// Field distance (field px) up to which the GPU SDF must be exact for the shadow
/// (shadow.wgsl `sh_cols`).
fn sdf_window(rr: f32, ds: usize) -> u32 {
    (rr / ds as f32).ceil() as u32 + 3
}

/// Shadow geometry and the SDF passes (alpha plane, exact EDT, field) on `src` (RGBA, or
/// packed alpha when `alpha_src`). Returns (params, field, occ) or None when there is no
/// shadow. `occ` is also the outside transform's scratch.
#[allow(clippy::too_many_arguments)]
fn shadow_field(job: &mut Job, temps: &mut Vec<wgpu::Buffer>, src: &wgpu::Buffer, alpha_src: bool, w: usize, h: usize, cx: &FxCtx, p: &[f64; NPARAM], color: [f64; 4]) -> Option<(Params, wgpu::Buffer, wgpu::Buffer)> {
    let (lz, ht, r) = crate::light::shadow_geom(p);
    let strength = (color[3] * p[5]).clamp(0.0, 1.0) as f32;
    if strength <= 0.0 || ht <= 0.0 {
        return None;
    }
    let (lx, ly) = ((p[0] - cx.ox as f64) as f32, (p[1] - cx.oy as f64) as f32);
    let k = ((lz - ht) / lz) as f32;
    let ds = ((r / 3.0).floor() as usize).clamp(1, 4);
    let (sw, sh) = (w.div_ceil(ds), h.div_ceil(ds));
    let rr = (r as f32).max(0.6);
    let mut q = Params::default();
    q.u(0, w as u32).u(1, h as u32).u(2, sw as u32).u(3, sh as u32).u(4, ds as u32).f(5, lx).f(6, ly).f(7, k).f(8, rr).f(9, 1.0 / ds as f32).f(10, strength);
    q.f(11, color[0] as f32).f(12, color[1] as f32).f(13, color[2] as f32);
    let mut tb = |size: usize| {
        let b = job.g.buffer(size as u64);
        temps.push(b.clone());
        b
    };
    let n = sw * sh;
    let cov = tb(n * 4);
    let gi = tb(n * 4);
    let go = tb(n * 4);
    let scr = tb(16);
    let fld = tb(n * 4);
    let occ = tb((n * 4).max((w * h).div_ceil(2) * 4));
    let mut qs = q.clone();
    qs.u(14, alpha_src as u32).u(15, sdf_window(rr, ds));
    let bufs = [src, &cov, &gi, &go, &scr, &fld, &occ];
    job.pass("shadow", SHADOW, 7, "sh_cov", &qs, &bufs, [sw as u32, sh as u32]);
    job.pass("shadow", SHADOW, 7, "sh_cols", &qs, &bufs, [sw as u32, sh as u32]);
    job.pass("shadow", SHADOW, 7, "sh_rows", &qs, &bufs, [sw as u32, sh as u32]);
    job.pass("shadow", SHADOW, 7, "sh_field", &qs, &bufs, [sw as u32, sh as u32]);
    Some((qs, fld, occ))
}

/// Shadow on the resident frame (composite on the GPU).
fn shadow(a: &mut Active, cx: &FxCtx, p: &[f64; NPARAM], color: [f64; 4]) -> anyhow::Result<()> {
    let (w, h) = (a.w, a.h);
    let px = a.px.clone();
    let Some((q, fld, _)) = shadow_field(&mut a.job, &mut a.temps, &px, false, w, h, cx, p, color) else { return Ok(()) };
    let dummy = a.temp(16);
    a.job.pass("light", LIGHT, 5, "shadow", &q, &[&px, &fld, &dummy, &dummy, &dummy], [w as u32, h as u32]);
    Ok(())
}

/// Shadow of a frame on the host: its alpha goes up (1 B/px), the occlusion comes back
/// (u16, 2 B/px) and the composite runs on the CPU (light::shadow's formula).
fn shadow_host(g: &Gpu, buf: &mut [u8], w: usize, h: usize, cx: &FxCtx, p: &[f64; NPARAM], color: [f64; 4]) -> anyhow::Result<()> {
    use rayon::prelude::*;
    let alpha: Vec<u8> = {
        let _p = crate::prof::scope("gpu.alpha");
        let mut a = vec![0u8; (w * h).next_multiple_of(4)];
        a.par_chunks_mut(1 << 14).zip(buf.par_chunks(1 << 16)).for_each(|(o, b)| {
            for (o, p) in o.iter_mut().zip(b.chunks_exact(4)) {
                *o = p[3];
            }
        });
        a
    };
    let mut job = Job::new(g);
    let mut temps = Vec::new();
    let src = job.upload(&alpha);
    temps.push(src.clone());
    let Some((q, _, occ)) = shadow_field(&mut job, &mut temps, &src, true, w, h, cx, p, color) else {
        for b in temps {
            g.give(b);
        }
        return Ok(());
    };
    let n = w * h;
    let bufs = [&src, &temps[1], &temps[2], &temps[3], &temps[4], &temps[5], &occ];
    job.pass("shadow", SHADOW, 7, "sh_occ", &q, &bufs, [n.div_ceil(2) as u32, 0]);
    job.read(&occ, n.div_ceil(2) * 4);
    let mut o16 = vec![0u16; n.div_ceil(2) * 2];
    let r = job.finish(&mut [bytemuck::cast_slice_mut(&mut o16[..])]);
    for b in temps {
        g.give(b);
    }
    r?;
    let _p = crate::prof::scope("gpu.composite");
    let col = [color[0] as f32, color[1] as f32, color[2] as f32];
    buf.par_chunks_mut(w * 4).zip(o16.par_chunks(w)).for_each(|(row, oc)| {
        for (o, &v) in row.chunks_exact_mut(4).zip(oc) {
            if v == 0 {
                continue;
            }
            let occ = v as f32 / 65535.0;
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
    Ok(())
}

// ---------------------------------------------------------------- rays

const RAY_SAMPLES: usize = 96;

fn rays(a: &mut Active, cx: &FxCtx, p: &[f64; NPARAM], color: [f64; 4]) -> anyhow::Result<()> {
    let (w, h) = (a.w, a.h);
    let strength = (p[3] * color[3]).max(0.0) as f32;
    let len = p[2].clamp(0.0, 0.95) as f32;
    if strength <= 0.0 || len <= 0.0 {
        return Ok(());
    }
    let ds = 2usize;
    let (sw, sh) = (w.div_ceil(ds), h.div_ceil(ds));
    let (lx, ly) = (((p[0] - cx.ox as f64) / ds as f64) as f32, ((p[1] - cx.oy as f64) / ds as f64) as f32);
    let decay = 0.975f32;
    let norm: f32 = (0..RAY_SAMPLES).map(|i| decay.powi(i as i32)).sum();
    let mut q = Params::default();
    q.u(0, w as u32).u(1, h as u32).u(2, sw as u32).u(3, sh as u32).u(4, ds as u32).f(5, lx).f(6, ly).f(7, len).f(8, norm).f(9, decay).i(10, cx.ox).i(11, cx.oy).f(12, strength);
    q.f(13, color[0] as f32).f(14, color[1] as f32).f(15, color[2] as f32);
    let (bw, bh) = (sw.div_ceil(8), sh.div_ceil(8));
    q.u(16, bw as u32).u(17, bh as u32).u(18, agentfx_ir::env::var("AGENTFX_GPU_RAYSKIP").map_or(1, |v| (v != "0") as u32));
    let bb = a.upload(bytemuck::cast_slice(&[u32::MAX, u32::MAX, 0, 0]));
    let blk = a.upload(bytemuck::cast_slice(&vec![0u32; bw * bh + (bw + 1) * (bh + 1)]));
    let mask = a.temp((sw * sh * 4) as u64);
    let lum = a.temp((sw * sh * 4) as u64);
    let tmp = a.temp((sw * sh * 4) as u64);
    let px = a.px.clone();
    let bufs = [&px, &mask, &bb, &lum, &blk];
    let full = [w as u32, h as u32];
    let half = [sw as u32, sh as u32];
    a.job.pass("light", LIGHT, 5, "rays_bbox", &q, &bufs, full);
    a.job.pass("light", LIGHT, 5, "rays_mask", &q, &bufs, half);
    // light::march_rays' empty-space skip (bitwise the full march): block occupancy and
    // its summed-area table; AGENTFX_GPU_RAYSKIP=0 keeps only the whole-ray test
    a.job.pass("light", LIGHT, 5, "rays_occ", &q, &bufs, half);
    a.job.pass("light", LIGHT, 5, "rays_sat_rows", &q, &bufs, [bh as u32, 0]);
    a.job.pass("light", LIGHT, 5, "rays_sat_cols", &q, &bufs, [bw as u32, 0]);
    a.job.pass("light", LIGHT, 5, "rays_march", &q, &bufs, half);
    blur_passes(&mut a.job, &lum, &tmp, sw, sh, 1.2).expect("sigma 1.2 is full-res");
    a.job.pass("light", LIGHT, 5, "rays_final", &q, &bufs, full);
    Ok(())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A test device: hardware if present, else a software adapter (llvmpipe), else None.
    pub fn device() -> Option<&'static Gpu> {
        static D: std::sync::OnceLock<Option<Gpu>> = std::sync::OnceLock::new();
        D.get_or_init(|| {
            // SAFETY: tests set this before any other thread reads it (first use, in the OnceLock)
            unsafe { std::env::set_var("AGENTFX_GPU_ALLOW_CPU", "1") };
            match Gpu::new() {
                Ok(g) => {
                    eprintln!("gpu tests on {} ({})", g.name, g.backend);
                    Some(g)
                }
                Err(e) => {
                    eprintln!("gpu tests skipped: {e:#}");
                    None
                }
            }
        })
        .as_ref()
    }

    /// (max channel diff, PSNR dB, share of channels off by more than 1)
    pub fn diff(a: &[u8], b: &[u8]) -> (u8, f64, f64) {
        let (mut mx, mut se, mut over) = (0u8, 0f64, 0usize);
        for (x, y) in a.iter().zip(b) {
            let d = x.abs_diff(*y);
            mx = mx.max(d);
            se += (d as f64).powi(2);
            over += (d > 1) as usize;
        }
        let mse = se / a.len() as f64;
        (mx, if mse == 0.0 { f64::INFINITY } else { 10.0 * (65025.0 / mse).log10() }, over as f64 / a.len() as f64)
    }

    /// A poster-like test image: gradients, shapes with holes, soft edges, transparency.
    pub fn poster(w: usize, h: usize, opaque_bg: bool) -> Vec<u8> {
        let mut b = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                let mut c = [0.95 - 0.3 * fy, 0.85 - 0.2 * fx, 0.7, if opaque_bg { 1.0 } else { 0.0 }];
                // a disc with a hole and a soft edge
                let d = ((fx - 0.4).powi(2) + (fy - 0.5).powi(2) * 0.5).sqrt();
                let cov = ((0.3 - d) * w as f32 * 0.5).clamp(0.0, 1.0) * (1.0 - ((0.08 - ((fx - 0.45).powi(2) + (fy - 0.45).powi(2)).sqrt()) * w as f32 * 0.5).clamp(0.0, 1.0));
                let ink = [0.9 * fx, 0.2 + 0.5 * fy, 0.4, 1.0];
                for k in 0..4 {
                    c[k] = c[k] * (1.0 - cov) + ink[k] * cov;
                }
                // bars
                if (x / 24) % 3 == 0 && fy > 0.7 && fx > 0.6 {
                    c = [0.1, 0.1, 0.12, 1.0];
                }
                let px = &mut b[(y * w + x) * 4..(y * w + x) * 4 + 4];
                for k in 0..3 {
                    px[k] = (c[k] * c[3] * 255.0 + 0.5) as u8;
                }
                px[3] = (c[3] * 255.0 + 0.5) as u8;
            }
        }
        b
    }

    fn cx(ox: i32, oy: i32) -> FxCtx {
        FxCtx { ox, oy, ax: 13, ay: -7, s: 1.0, frame: 5 }
    }

    pub fn effects() -> Vec<(&'static str, Effect, [f64; NPARAM])> {
        let mut out = Vec::new();
        let riso = |paper: Option<[f64; 4]>, grain: f64, density: f64| Effect::Riso { inks: vec![[0.0, 0.47, 0.75, 1.0], [1.0, 0.28, 0.5, 1.0], [0.1, 0.1, 0.12, 0.9]], seed: 7, drift: 0.6, drift_speed: 0.3, grain, density, paper, misreg: 1.5 };
        let mut p = [0.0; NPARAM];
        p[0] = 1.5;
        p[TIME] = 1.25;
        out.push(("riso", riso(None, 0.4, 0.3), p));
        out.push(("riso-paper", riso(Some([0.98, 0.96, 0.9, 1.0]), 0.6, 0.0), p));
        out.push(("riso-plain", riso(None, 0.0, 0.0), p));
        let ht = |mode: &str, paper: Option<[f64; 4]>| Effect::Halftone { mode: mode.into(), ink: [0.1, 0.1, 0.2, 1.0], paper, cell: 7.0, angle: 30.0 };
        let mut p = [0.0; NPARAM];
        p[0] = 7.0;
        p[1] = 30.0;
        out.push(("halftone-mono", ht("mono", None), p));
        out.push(("halftone-cmyk-paper", ht("cmyk", Some([0.97, 0.95, 0.9, 1.0])), p));
        let mut p = [0.0; NPARAM];
        p[0] = 2.0; // SUNFALL "fall": 2 px CMYK cells on paper
        p[1] = 45.0;
        out.push(("halftone-cmyk-fine", ht("cmyk", Some([0.9569, 0.9373, 0.902, 1.0])), p));
        let mut p = [0.0; NPARAM];
        p[0] = 26.0; // cell above the full-res blur limit: CPU blur, GPU rest
        p[1] = 15.0;
        out.push(("halftone-cmyk-big", ht("cmyk", None), p));
        let mut p = [0.0; NPARAM];
        p[..6].copy_from_slice(&[100.0, -80.0, 900.0, 60.0, 40.0, 0.8]);
        out.push(("shadow", Effect::Shadow { color: [0.05, 0.05, 0.1, 0.7], light: [100.0, -80.0, 900.0], light_radius: 60.0, height: 40.0, strength: 0.8 }, p));
        let mut p = [0.0; NPARAM];
        p[..4].copy_from_slice(&[150.0, 60.0, 0.6, 0.9]);
        out.push(("rays", Effect::Rays { color: [1.0, 0.9, 0.7, 1.0], light: [150.0, 60.0], length: 0.6, strength: 0.9 }, p));
        out
    }

    /// GPU vs CPU on every ported effect: max channel diff <= 3 and PSNR >= 45 dB.
    #[test]
    fn gpu_matches_cpu_within_tolerance() {
        let Some(g) = device() else { return };
        let (w, h) = (403, 297); // odd sizes: edges, partial workgroups, partial ds blocks
        for opaque in [false, true] {
            let img = poster(w, h, opaque);
            for (name, e, p) in effects() {
                let c = cx(37, -11);
                let mut cpu = img.clone();
                crate::look::apply(&e, &p, &mut cpu, w, h, &c);
                let mut gpu = img.clone();
                assert!(apply_on(g, &e, &p, &mut gpu, w, h, &c).unwrap(), "{name}: not run on GPU");
                let (mx, psnr, over) = diff(&cpu, &gpu);
                if mx > 2 {
                    let i = cpu.iter().zip(&gpu).position(|(a, b)| a.abs_diff(*b) == mx).unwrap() / 4;
                    eprintln!("  worst px ({}, {}): cpu {:?} gpu {:?} src {:?}", i % w, i / w, &cpu[i * 4..i * 4 + 4], &gpu[i * 4..i * 4 + 4], &img[i * 4..i * 4 + 4]);
                }
                eprintln!("{name:22} opaque={opaque:5} max {mx} psnr {psnr:.1} dB, >1: {:.4} %", over * 100.0);
                assert!(mx <= 3 && psnr >= 45.0, "{name} (opaque {opaque}): max {mx}, psnr {psnr:.1}");
                // (an opaque layer has no holes: rays add nothing there, correctly)
                assert!(opaque || cpu != img, "{name}: effect did nothing (bad fixture)");
            }
        }
    }

    /// Per-effect cost at 1080p, CPU vs GPU (incl. upload + readback), a chain of two
    /// effects vs two single calls, and the bare transfer cost.
    /// `cargo test --release gpu_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn gpu_bench() {
        let Some(g) = device() else { return };
        let (w, h) = (1920, 1080);
        let img = poster(w, h, false);
        let reps = 5;
        let c = cx(0, 0);
        let ms = |t: Instant| t.elapsed().as_secs_f64() / reps as f64 * 1e3;
        // transfer only: upload (staged) + readback of the frame
        for _ in 0..2 {
            let t = Instant::now();
            for _ in 0..reps {
                let mut job = Job::new(g);
                let b = job.upload(&img);
                job.read(&b, img.len());
                let mut out = img.clone();
                job.finish(&mut [&mut out[..]]).unwrap();
                g.give(b);
            }
            eprintln!("transfer 1080p up+down: {:.1} ms", ms(t));
        }
        let b = g.buffer(img.len() as u64);
        let t = Instant::now();
        for _ in 0..reps {
            let mut job = Job::new(g);
            job.write(&b, &img);
            job.finish(&mut []).unwrap();
        }
        let up = ms(t);
        let t = Instant::now();
        for _ in 0..reps {
            g.queue.write_buffer(&b, 0, &img);
            g.queue.submit([]);
            g.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
        let up_wb = ms(t);
        let t = Instant::now();
        for _ in 0..reps {
            let mut job = Job::new(g);
            job.read(&b, img.len());
            let mut out = img.clone();
            job.finish(&mut [&mut out[..]]).unwrap();
        }
        let down = ms(t);
        g.give(b);
        eprintln!("  upload {up:.1} ms (queue.write_buffer {up_wb:.1} ms), readback {down:.1} ms");
        type Run<'a> = Box<dyn Fn(&mut Chain, &mut [u8]) -> bool + 'a>;
        type Cpu<'a> = Box<dyn Fn(&mut [u8]) + 'a>;
        let mut cases: Vec<(String, Cpu, Run)> = Vec::new();
        for (name, e, p) in effects() {
            let (e2, p2) = (e.clone(), p);
            cases.push((name.to_string(), Box::new(move |b: &mut [u8]| crate::look::apply(&e, &p, b, w, h, &c)), Box::new(move |ch: &mut Chain, b: &mut [u8]| ch.look(&e2, &p2, b, w, h, &c))));
        }
        for sigma in [3.0, 25.0] {
            cases.push((format!("blur {sigma}"), Box::new(move |b: &mut [u8]| crate::effects::blur(b, w, h, sigma)), Box::new(move |ch: &mut Chain, b: &mut [u8]| ch.blur(b, w, h, sigma))));
        }
        let col = [0.0, 0.0, 0.1, 0.6];
        cases.push(("drop-shadow 12".into(), Box::new(move |b: &mut [u8]| crate::effects::drop_shadow(b, w, h, col, 12.0, 8, 10)), Box::new(move |ch: &mut Chain, b: &mut [u8]| ch.drop_shadow(b, w, h, col, 12.0, 8, 10))));
        cases.push(("glow 20".into(), Box::new(move |b: &mut [u8]| crate::effects::glow(b, w, h, col, 1.0, 20.0)), Box::new(move |ch: &mut Chain, b: &mut [u8]| ch.glow(b, w, h, col, 1.0, 20.0))));
        let fx = effects();
        let (_, ht, hp) = fx.iter().find(|x| x.0 == "halftone-cmyk-fine").unwrap().clone();
        let (_, ri, rp) = fx.iter().find(|x| x.0 == "riso").unwrap().clone();
        {
            let (ht1, ri1, ht2, ri2) = (ht.clone(), ri.clone(), ht.clone(), ri.clone());
            cases.push(("chain halftone+riso".into(), Box::new(move |b: &mut [u8]| {
                crate::look::apply(&ht1, &hp, b, w, h, &c);
                crate::look::apply(&ri1, &rp, b, w, h, &c);
            }), Box::new(move |ch: &mut Chain, b: &mut [u8]| ch.look(&ht2, &hp, b, w, h, &c) && ch.look(&ri2, &rp, b, w, h, &c))));
            cases.push(("2 calls halftone, riso".into(), Box::new(|_: &mut [u8]| {}), Box::new(move |ch: &mut Chain, b: &mut [u8]| {
                let ok = ch.look(&ht, &hp, b, w, h, &c);
                ch.finish(b).unwrap();
                ok && ch.look(&ri, &rp, b, w, h, &c)
            })));
        }
        for (name, cpu, run) in &cases {
            let mut b = img.clone();
            let mut ch = Chain::on(g);
            assert!(run(&mut ch, &mut b), "{name}: not on the GPU"); // warm-up (pipelines)
            ch.finish(&mut b).unwrap();
            let t = Instant::now();
            for _ in 0..reps {
                let mut b = img.clone();
                cpu(&mut b);
            }
            let cpu_ms = ms(t);
            let t = Instant::now();
            for _ in 0..reps {
                let mut b = img.clone();
                let mut ch = Chain::on(g);
                run(&mut ch, &mut b);
                ch.finish(&mut b).unwrap();
            }
            let gpu_ms = ms(t);
            eprintln!("{name:24} cpu {cpu_ms:7.1} ms  gpu {gpu_ms:6.1} ms  x{:.1}", cpu_ms / gpu_ms);
        }
    }

    /// The GPU blur is bit-identical to effects::blur (both paths: full resolution and
    /// downsampled), and so are drop shadow and glow built on it.
    #[test]
    fn gpu_blur_is_bit_exact() {
        let Some(g) = device() else { return };
        let (w, h) = (211, 157);
        let img = poster(w, h, false);
        for sigma in [0.3, 1.2, 2.5, 4.0, 6.9, 7.2, 12.0, 30.0, 95.0] {
            let mut cpu = img.clone();
            crate::effects::blur(&mut cpu, w, h, sigma);
            let mut gpu = img.clone();
            let mut c = Chain::on(g);
            assert!(c.blur(&mut gpu, w, h, sigma));
            c.finish(&mut gpu).unwrap();
            assert!(cpu == gpu, "blur sigma {sigma}: max diff {}", diff(&cpu, &gpu).0);
            let col = [0.1, 0.2, 0.6, 0.7];
            for (dx, dy) in [(0, 0), (9, -4), (-300, 2)] {
                let mut cpu = img.clone();
                crate::effects::drop_shadow(&mut cpu, w, h, col, sigma, dx, dy);
                let mut gpu = img.clone();
                let mut c = Chain::on(g);
                assert!(c.drop_shadow(&mut gpu, w, h, col, sigma, dx, dy));
                c.finish(&mut gpu).unwrap();
                assert!(cpu == gpu, "drop shadow sigma {sigma} ({dx}, {dy}): max diff {}", diff(&cpu, &gpu).0);
            }
            let mut cpu = img.clone();
            crate::effects::glow(&mut cpu, w, h, col, 1.7, sigma);
            let mut gpu = img.clone();
            let mut c = Chain::on(g);
            assert!(c.glow(&mut gpu, w, h, col, 1.7, sigma));
            c.finish(&mut gpu).unwrap();
            assert!(cpu == gpu, "glow sigma {sigma}: max diff {}", diff(&cpu, &gpu).0);
        }
    }

    /// The GPU signed distance field equals light::sdf (alpha plane + exact EDT) within the
    /// distance the shadow can see; only the GPU sqrt may differ by an ulp.
    #[test]
    fn gpu_sdf_matches_exact_edt() {
        let Some(g) = device() else { return };
        let mut dss = vec![];
        for (w, h, opaque) in [(403, 297, false), (211, 157, true), (64, 300, false)] {
            let mut img = poster(w, h, opaque);
            // speckles and a hole pattern: many small features
            for y in 0..h {
                for x in 0..w {
                    if (x * 7 + y * 13) % 97 == 0 || ((x / 5 + y / 7) % 11 == 0 && x > w / 2) {
                        img[(y * w + x) * 4 + 3] = if (x + y) % 3 == 0 { 255 } else { 0 };
                    }
                    // a checkerboard patch: blocks of exactly half coverage (inside test at 0.5)
                    if x < 48 && y < 64 {
                        img[(y * w + x) * 4 + 3] = if (x + y) % 2 == 0 { 255 } else { 0 };
                    }
                }
            }
            for r in [2.0, 20.0, 40.0, 75.0] {
                // penumbra r * 40 / 100: ds 1, 2, 4, 4
                let mut p = [0.0; NPARAM];
                p[..6].copy_from_slice(&[100.0, -80.0, 100.0, r, 40.0, 0.8]);
                let (_, _, rr) = crate::light::shadow_geom(&p);
                let ds = ((rr / 3.0).floor() as usize).clamp(1, 4);
                let (cov, sw, sh) = crate::light::alpha_plane(&img, w, h, ds);
                let cpu = crate::light::sdf(&cov, sw, sh);
                let mut job = Job::new(g);
                let mut temps = Vec::new();
                let src = job.upload(&img);
                let (_, fld, _) = shadow_field(&mut job, &mut temps, &src, false, w, h, &cx(0, 0), &p, [0.0, 0.0, 0.0, 1.0]).unwrap();
                job.read(&fld, sw * sh * 4);
                let mut gpu = vec![0f32; sw * sh];
                job.finish(&mut [bytemuck::cast_slice_mut(&mut gpu[..])]).unwrap();
                for b in temps {
                    g.give(b);
                }
                g.give(src);
                // exact within the window the shadow can see (+-C field px)
                let cw = sdf_window((rr as f32).max(0.6), ds) as f32 - 1.0;
                let mut worst = (0f32, 0usize);
                for (i, (a, b)) in cpu.iter().zip(&gpu).enumerate() {
                    let (a, b) = (a.clamp(-cw, cw), b.clamp(-cw, cw));
                    let d = (a - b).abs() / a.abs().max(1.0);
                    if d > worst.0 {
                        worst = (d, i);
                    }
                }
                let i = worst.1;
                assert!(worst.0 <= 1e-6, "{w}x{h} ds {ds}: rel diff {} at {i}: cpu {} gpu {}", worst.0, cpu[i], gpu[i]);
                dss.push(ds);
            }
        }
        assert!(dss.contains(&1) && dss.contains(&2) && dss.contains(&4), "fixture covers ds 1, 2, 4: {dss:?}");
    }

    /// Shadow on the resident frame (after a blur in the same chain) vs the CPU.
    #[test]
    fn gpu_resident_shadow_matches_cpu() {
        let Some(g) = device() else { return };
        let (w, h) = (403, 297);
        let img = poster(w, h, false);
        let (_, e, p) = effects().into_iter().find(|x| x.0 == "shadow").unwrap();
        let c = cx(37, -11);
        let mut cpu = img.clone();
        crate::effects::blur(&mut cpu, w, h, 2.0);
        crate::look::apply(&e, &p, &mut cpu, w, h, &c);
        let mut gpu = img.clone();
        let mut ch = Chain::on(g);
        assert!(ch.blur(&mut gpu, w, h, 2.0));
        assert!(ch.look(&e, &p, &mut gpu, w, h, &c));
        ch.finish(&mut gpu).unwrap();
        let (mx, psnr, _) = diff(&cpu, &gpu);
        eprintln!("resident shadow: max {mx} psnr {psnr:.1} dB");
        assert!(mx <= 3 && psnr >= 45.0);
    }

    /// A chain of several effects on one resident frame (one upload, one readback) equals
    /// the CPU effects applied in order, within the per-effect tolerance.
    #[test]
    fn gpu_chain_matches_cpu_sequence() {
        let Some(g) = device() else { return };
        let (w, h) = (403, 297);
        let fx = effects();
        let pick = |n: &str| fx.iter().find(|x| x.0 == n).unwrap().clone();
        let c = cx(37, -11);
        for opaque in [false, true] {
            let img = poster(w, h, opaque);
            let (_, ht, hp) = pick("halftone-cmyk-fine");
            let (_, ri, rp) = pick("riso");
            let (_, ra, rap) = pick("rays");
            let mut cpu = img.clone();
            crate::effects::blur(&mut cpu, w, h, 1.5);
            crate::look::apply(&ht, &hp, &mut cpu, w, h, &c);
            crate::effects::glow(&mut cpu, w, h, [1.0, 0.8, 0.3, 1.0], 0.8, 9.0);
            crate::look::apply(&ri, &rp, &mut cpu, w, h, &c);
            crate::effects::drop_shadow(&mut cpu, w, h, [0.0, 0.0, 0.0, 0.6], 4.0, 6, 8);
            crate::look::apply(&ra, &rap, &mut cpu, w, h, &c);
            let mut gpu = img.clone();
            let mut ch = Chain::on(g);
            assert!(ch.blur(&mut gpu, w, h, 1.5));
            assert!(ch.look(&ht, &hp, &mut gpu, w, h, &c));
            assert!(ch.glow(&mut gpu, w, h, [1.0, 0.8, 0.3, 1.0], 0.8, 9.0));
            assert!(ch.look(&ri, &rp, &mut gpu, w, h, &c));
            assert!(ch.drop_shadow(&mut gpu, w, h, [0.0, 0.0, 0.0, 0.6], 4.0, 6, 8));
            assert!(ch.look(&ra, &rap, &mut gpu, w, h, &c));
            assert!(gpu == img, "the host buffer is untouched until the flush");
            ch.finish(&mut gpu).unwrap();
            let (mx, psnr, over) = diff(&cpu, &gpu);
            eprintln!("chain opaque={opaque}: max {mx} psnr {psnr:.1} dB, >1: {:.4} %", over * 100.0);
            assert!(mx <= 3 && psnr >= 45.0, "chain (opaque {opaque}): max {mx}, psnr {psnr:.1}");
        }
    }
}
