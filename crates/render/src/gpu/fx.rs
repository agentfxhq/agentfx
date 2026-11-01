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
const DIGITAL: &str = include_str!("digital.wgsl");
const WARP: &str = include_str!("warp.wgsl");
const LENS: &str = include_str!("lens.wgsl");

/// Is this look/light effect ported (whatever the routing)?
pub fn ported(e: &Effect) -> bool {
    matches!(e, Effect::Riso { .. } | Effect::Halftone { .. } | Effect::Shadow { .. } | Effect::Rays { .. } | Effect::Rgbsplit { .. } | Effect::Glitch { .. } | Effect::Vhs { .. } | Effect::Grain { .. } | Effect::Glass { .. } | Effect::Optics { .. })
}

fn fx_name(e: &Effect) -> &'static str {
    match e {
        Effect::Riso { .. } => "riso",
        Effect::Halftone { .. } => "halftone",
        Effect::Shadow { .. } => "shadow",
        Effect::Rays { .. } => "rays",
        Effect::Rgbsplit { .. } => "rgbsplit",
        Effect::Glitch { .. } => "glitch",
        Effect::Vhs { .. } => "vhs",
        Effect::Grain { .. } => "grain",
        Effect::Blur { .. } => "blur",
        Effect::DropShadow { .. } => "drop-shadow",
        Effect::Glow { .. } => "glow",
        Effect::Glass { .. } => "glass",
        Effect::Optics { .. } => "optics",
        _ => "",
    }
}

/// Every ported effect (the default of `AGENTFX_GPU_FX`).
pub const ALL_FX: &str = "riso,halftone,rays,shadow,rgbsplit,glitch,vhs,grain,glass,optics,blur,drop-shadow,glow,kaleido,tunnel";

/// `AGENTFX_GPU_FX=riso,halftone,...` picks the effects that may start a chain (move the
/// frame to the GPU); default: all ported effects (`ALL_FX`). A chain starts only for a
/// buffer of at least `start_px` pixels (a shadow outside a chain sends only the alpha up
/// and the occlusion back). Effects that are not picked still join a chain that is already
/// on the GPU (`joins`).
fn enabled(name: &str) -> bool {
    static ON: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let list = ON.get_or_init(|| agentfx_ir::env::var("AGENTFX_GPU_FX").unwrap_or_else(|_| ALL_FX.into()).split(',').map(|s| s.trim().to_string()).collect());
    list.iter().any(|x| x == name)
}

/// Smallest buffer (px) for which `name` moves a frame to the GPU. Blur, drop shadow and
/// glow are cheap per pixel on the CPU next to the frame's upload + readback:
/// `AGENTFX_GPU_BLUR_MIN_PX` (default 512 K px); the others: `AGENTFX_GPU_MIN_PX`.
fn start_px(name: &str) -> usize {
    static BLUR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    match name {
        "blur" | "drop-shadow" | "glow" => (*BLUR.get_or_init(|| agentfx_ir::env::var("AGENTFX_GPU_BLUR_MIN_PX").ok().and_then(|v| v.parse().ok()).unwrap_or(1 << 19))).max(super::min_px()),
        _ => super::min_px(),
    }
}

/// Effects that join an active chain even when not `enabled` (`AGENTFX_GPU_JOIN=0`: never).
fn joins(name: &str) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| agentfx_ir::env::var("AGENTFX_GPU_JOIN").map_or(true, |v| v != "0")) && matches!(name, "blur" | "drop-shadow" | "glow" | "rgbsplit" | "glitch" | "vhs" | "grain" | "glass" | "optics" | "kaleido" | "tunnel")
}

/// `AGENTFX_GPU_RESIDENT=0`: read the frame back after every GPU effect (no residency
/// across a node's effects; for measuring what the chain saves).
fn resident() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| agentfx_ir::env::var("AGENTFX_GPU_RESIDENT").map_or(true, |v| v != "0"))
}

/// One effect of a chain, kept for the CPU replay (fallback, `AGENTFX_GPU_CHECK`).
#[derive(Clone)]
enum Op {
    Look(Effect, [f64; NPARAM], FxCtx),
    Glass(Effect, [f64; NPARAM], FxCtx, Vec<u8>, u8),
    Blur(f64),
    DropShadow([f64; 4], f64, i64, i64),
    Glow([f64; 4], f64, f64),
    Warp(crate::warp::WarpCfg),
}

impl Op {
    fn name(&self) -> &'static str {
        match self {
            Op::Look(e, ..) => fx_name(e),
            Op::Glass(..) => "glass",
            Op::Blur(..) => "blur",
            Op::DropShadow(..) => "drop-shadow",
            Op::Glow(..) => "glow",
            Op::Warp(c) => if c.kind == crate::warp::KALEIDO { "kaleido" } else { "tunnel" },
        }
    }

    fn cpu(&self, buf: &mut [u8], w: usize, h: usize) {
        match self {
            Op::Look(e, p, cx) => crate::look::apply(e, p, buf, w, h, cx),
            Op::Glass(e, p, cx, backdrop, _) => crate::look::apply_with_backdrop(e, p, buf, Some(backdrop), w, h, cx),
            Op::Blur(sigma) => crate::effects::blur(buf, w, h, *sigma),
            Op::DropShadow(c, sigma, dx, dy) => crate::effects::drop_shadow(buf, w, h, *c, *sigma, *dx, *dy),
            Op::Glow(c, k, sigma) => crate::effects::glow(buf, w, h, *c, *k, *sigma),
            Op::Warp(c) => crate::warp::apply(c, buf, w, h),
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
            // sampling table: per glitch block (at most one per pixel) 3 words
            Op::Look(Effect::Glitch { .. }, ..) => (w * h * 12 + (w + 2 * h) * 4) as u64,
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
    #[allow(dead_code)]
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

    /// A glass layer needs both its alpha-derived lens surface and the pixels already
    /// composited below it. The backdrop is captured by vraster in the same screen rect.
    pub fn glass(&mut self, e: &Effect, p: &[f64; NPARAM], buf: &mut [u8], backdrop: &[u8], w: usize, h: usize, cx: &FxCtx) -> bool {
        let max_alpha = buf.chunks_exact(4).map(|v| v[3]).max().unwrap_or(0).max(1);
        self.step(Op::Glass(e.clone(), *p, *cx, backdrop.to_vec(), max_alpha), buf, w, h)
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

    /// warp::apply on the GPU; false: run it on the CPU (buffer current).
    pub fn warp(&mut self, c: &crate::warp::WarpCfg, buf: &mut [u8], w: usize, h: usize) -> bool {
        if c.identity() {
            return true;
        }
        self.step(Op::Warp(*c), buf, w, h)
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
        // a frame that is already on the GPU: every enabled or joining effect rides along
        if let Some(a) = self.a.as_ref().filter(|a| a.w == w && a.h == h && (enabled(name) || joins(name))) {
            return Some(a.g);
        }
        if !enabled(name) {
            return None;
        }
        let g = super::get()?;
        if w * h < start_px(name) {
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
                if !resident() {
                    self.flush(buf);
                }
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

pub(crate) fn fail(err: &anyhow::Error) {
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
fn encode(a: &mut Active, op: &Op, _host: Option<&[u8]>) -> anyhow::Result<()> {
    match op {
        Op::Look(e, p, cx) => match e {
            Effect::Riso { inks, seed, drift, drift_speed, grain, density, paper, .. } => riso(a, cx, &crate::look::RisoCfg { inks, seed: *seed, misreg: p[0], drift: *drift, drift_speed: *drift_speed, t: p[TIME], grain: *grain, density: *density, paper: *paper }),
            Effect::Halftone { mode, ink, paper, .. } => halftone(a, cx, p[0], p[1], mode == "cmyk", *ink, *paper),
            Effect::Shadow { color, .. } => shadow(a, cx, p, *color),
            Effect::Rays { color, .. } => rays(a, cx, p, *color),
            Effect::Rgbsplit { .. } | Effect::Glitch { .. } | Effect::Vhs { .. } => digital(a, e, p, cx),
            Effect::Grain { seed, live, size, .. } => grain(a, cx, p[0], *seed, *live, *size),
            Effect::Glass { .. } => anyhow::bail!("glass requires a backdrop"),
            Effect::Optics { seed, .. } => lens_optics(a, p, *seed, cx),
            _ => anyhow::bail!("not ported"),
        },
        Op::Glass(Effect::Glass { tint, .. }, p, _, backdrop, max_alpha) => lens_glass(a, p, *tint, backdrop, *max_alpha),
        Op::Glass(..) => anyhow::bail!("invalid glass operation"),
        Op::Blur(sigma) => {
            let px = a.px.clone();
            blur_any(a, &px, *sigma);
            Ok(())
        }
        Op::DropShadow(c, sigma, dx, dy) => under_tinted(a, *c, 1.0, *sigma, *dx, *dy),
        Op::Glow(c, k, sigma) => under_tinted(a, *c, *k, *sigma, 0, 0),
        Op::Warp(c) => {
            warp(a, c);
            Ok(())
        }
    }
}

fn lens_glass(a: &mut Active, p: &[f64; NPARAM], tint: Option<[f64; 4]>, backdrop: &[u8], max_alpha: u8) -> anyhow::Result<()> {
    let mut q = Params::default();
    q.u(0, a.w as u32).u(1, a.h as u32);
    for (i, v) in p[..6].iter().enumerate() { q.f(i + 2, *v as f32); }
    for (i, v) in tint.unwrap_or([0.0; 4]).iter().enumerate() { q.f(i + 8, *v as f32); }
    q.f(12, max_alpha as f32);
    let out = a.g.buffer((a.w * a.h * 4) as u64);
    let px = a.px.clone();
    let bg = a.upload(backdrop);
    a.job.pass("lens", LENS, 3, "glass", &q, &[&px, &out, &bg], [a.w as u32, a.h as u32]);
    a.temps.push(std::mem::replace(&mut a.px, out));
    Ok(())
}

fn lens_optics(a: &mut Active, p: &[f64; NPARAM], seed: u64, cx: &FxCtx) -> anyhow::Result<()> {
    let mut q = Params::default();
    q.u(0, a.w as u32).u(1, a.h as u32);
    for (i, v) in p[..7].iter().enumerate() { q.f(i + 2, *v as f32); }
    q.u(9, seed as u32).i(10, cx.ox).i(11, cx.oy).u(12, cx.frame);
    let out = a.g.buffer((a.w * a.h * 4) as u64);
    let px = a.px.clone();
    // The shared lens module declares a backdrop binding for glass. Optics does not use
    // it, but give the validation layer a distinct read buffer rather than aliasing `px`.
    let spare = a.temp((a.w * a.h * 4) as u64);
    a.job.enc.copy_buffer_to_buffer(&px, 0, &spare, 0, (a.w * a.h * 4) as u64);
    a.job.pass("lens", LENS, 3, "optics", &q, &[&px, &out, &spare], [a.w as u32, a.h as u32]);
    a.temps.push(std::mem::replace(&mut a.px, out));
    Ok(())
}

/// Run one effect on a given device regardless of mode/routing (tests, benchmarks).
#[allow(dead_code)]
pub fn apply_on(g: &'static Gpu, e: &Effect, p: &[f64; NPARAM], buf: &mut [u8], w: usize, h: usize, cx: &FxCtx) -> anyhow::Result<bool> {
    let mut c = Chain::on(g);
    let ran = if matches!(e, Effect::Glass { .. }) {
        let backdrop = buf.to_vec();
        c.glass(e, p, buf, &backdrop, w, h, cx)
    } else {
        c.look(e, p, buf, w, h, cx)
    };
    if !ran {
        return Ok(false);
    }
    c.finish(buf)?;
    Ok(true)
}

/// Run glass with an explicit already-composited backdrop (the renderer path and the
/// conformance test both use this rather than treating the lens pixels as the backdrop).
#[allow(dead_code)]
pub fn apply_glass_on(g: &'static Gpu, e: &Effect, p: &[f64; NPARAM], lens: &mut [u8], backdrop: &[u8], w: usize, h: usize, cx: &FxCtx) -> anyhow::Result<bool> {
    let mut c = Chain::on(g);
    if !c.glass(e, p, lens, backdrop, w, h, cx) {
        return Ok(false);
    }
    c.finish(lens)?;
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

/// The GPU blur matches effects::blur exactly up to this reduction (the integer upsample's
/// u32 sums, comp.wgsl `up`).
fn blur_gpu_ok(sigma: f64) -> bool {
    sigma.is_finite() && blur_scale(sigma) <= 1024
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
    p.u(0, w as u32).u(1, h as u32).u(8, scale as u32).u(9, sw as u32).u(10, sh as u32).u(11, 0).u(12, 0);
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

/// effects::TiledBlur::finish on the GPU, bitwise: the reduced buffer `small` (sw x sh, from
/// `TiledBlur::reduced`) blurred with `rsigma` and reconstructed at `scale` for the crop
/// (cx, cy, cw, ch) into `out`; with `sharp` (cw x ch), the sharp layer over it (a glow).
/// None: not for the GPU (off, below the routing threshold, limits); Some(Err): GPU error
/// (the caller falls back to the CPU).
#[allow(clippy::too_many_arguments)]
pub fn tiled_finish(name: &str, small: &[u8], sw: usize, sh: usize, scale: usize, rsigma: f64, out: &mut [u8], cx: usize, cy: usize, cw: usize, ch: usize, sharp: Option<&[u8]>) -> Option<anyhow::Result<()>> {
    if !enabled(name) || cw * ch < start_px(name) || scale > 1024 || !blur_full_res(rsigma) {
        return None;
    }
    let g = super::get()?;
    if !g.fits((cw * ch * 4) as u64) || !g.fits((sw * sh * 4) as u64) {
        return None;
    }
    let _p = crate::prof::scope("gpu.fx");
    let t = Instant::now();
    let r = tiled_finish_on(g, small, sw, sh, scale, rsigma, out, cx, cy, cw, ch, sharp);
    if r.is_ok() {
        super::account(1, cw * ch, t);
    }
    Some(r)
}

/// `tiled_finish` on device `g` (no routing).
#[allow(clippy::too_many_arguments)]
pub(crate) fn tiled_finish_on(g: &Gpu, small: &[u8], sw: usize, sh: usize, scale: usize, rsigma: f64, out: &mut [u8], cx: usize, cy: usize, cw: usize, ch: usize, sharp: Option<&[u8]>) -> anyhow::Result<()> {
    let mut job = Job::new(g);
    let sm = job.upload(small);
    let tmp = g.buffer((sw * sh * 4) as u64);
    blur_passes(&mut job, &sm, &tmp, sw, sh, rsigma).expect("full-res reduced sigma");
    let blurred = g.buffer((cw * ch * 4) as u64);
    let mut p = Params::default();
    p.u(0, cw as u32).u(1, ch as u32).u(8, scale as u32).u(9, sw as u32).u(10, sh as u32).u(11, cx as u32).u(12, cy as u32);
    job.pass("comp", COMP, 2, "up", &p, &[&blurred, &sm], [cw as u32, ch as u32]);
    let mut bufs = vec![sm, tmp];
    let res = if let Some(sharp) = sharp {
        let top = job.upload(sharp);
        let mut q = Params::default();
        q.u(0, cw as u32).u(1, ch as u32).i(6, 0).i(7, 0);
        job.pass("comp", COMP, 2, "over_shift", &q, &[&top, &blurred], [cw as u32, ch as u32]);
        bufs.push(blurred);
        top
    } else {
        blurred
    };
    job.read(&res, cw * ch * 4);
    let r = job.finish(&mut [out]);
    bufs.push(res);
    for b in bufs {
        g.give(b);
    }
    r
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

// ---------------------------------------------------------------- digital looks

/// digital::rgb's division as a table (a * 256 + c -> c / a, 0 for a = 0): the GPU's f32
/// division is not correctly rounded, the glitch posterize step needs the exact quotient.
fn unpremul_table(g: &Gpu) -> wgpu::Buffer {
    g.constant("unpremul", || {
        let mut t = vec![0f32; 65536];
        for a in 1..256 {
            for c in 0..256 {
                t[a * 256 + c] = c as f32 / a as f32;
            }
        }
        bytemuck::cast_slice(&t).to_vec()
    })
}

/// rgbsplit / glitch / vhs: a gather through the host's sampling tables (digital::Tab).
fn digital(a: &mut Active, e: &Effect, p: &[f64; NPARAM], cx: &FxCtx) -> anyhow::Result<()> {
    let (w, h) = (a.w, a.h);
    let Some(tab) = crate::digital::Tab::new(e, p, w, h, cx) else { return Ok(()) };
    let mut q = Params::default();
    q.u(0, w as u32).u(1, h as u32).u(2, tab.mode).f(3, p[0].clamp(0.0, 1.0) as f32).u(4, tab.gw as u32).i(5, tab.x0).u(6, tab.seed);
    let len = (w * h * 4) as u64;
    let lut = unpremul_table(a.g);
    let t = a.upload(bytemuck::cast_slice(&tab.t));
    let out = a.temp(len);
    let px = a.px.clone();
    a.job.pass("digital", DIGITAL, 4, "digital", &q, &[&px, &out, &lut, &t], [w as u32, h as u32]);
    a.job.enc.copy_buffer_to_buffer(&out, 0, &px, 0, len);
    Ok(())
}

/// look::grain on the resident frame.
fn grain(a: &mut Active, cx: &FxCtx, amount: f64, seed: u64, live: bool, size: f64) -> anyhow::Result<()> {
    if amount <= 0.0 {
        return Ok(());
    }
    let (w, h) = (a.w, a.h);
    // (identical to look::grain)
    let mut sd = seed as u32 ^ (seed >> 32) as u32;
    if live {
        sd = sd.wrapping_add(cx.frame.wrapping_mul(0x9E37_79B1));
    }
    let amt = (amount * 255.0) as f32;
    let size = (size * cx.s).max(0.25) as f32;
    let mut q = Params::default();
    q.u(0, w as u32).u(1, h as u32).i(7, cx.ox - cx.ax).i(8, cx.oy - cx.ay).u(9, sd).f(10, amt).f(11, size).u(12, (size <= 1.0) as u32);
    let dummy = a.temp(16);
    let px = a.px.clone();
    a.job.pass("digital", DIGITAL, 4, "grain", &q, &[&px, &dummy, &dummy, &dummy], [w as u32, h as u32]);
    Ok(())
}

// ---------------------------------------------------------------- self-test

/// `--gpu=auto`'s start-up check (a few ms): blur (both paths) and glow bit-exact, vhs,
/// glitch, grain and the warps (kaleido, tunnel) within 1 of the CPU on a small fixture. An
/// error means this adapter / driver does not compute what the CPU does: the render stays on
/// the CPU.
pub fn self_test(g: &'static Gpu) -> anyhow::Result<()> {
    let (w, h) = (96, 64);
    let img: Vec<u8> = (0..w * h).flat_map(|i| {
        let (x, y) = (i % w, i / w);
        let v = crate::look::hash(i as i32, 0, 7);
        let a = if (x / 16 + y / 16) % 3 == 0 { 0 } else if x > 60 { 255 } else { (v & 255) as u8 };
        [(v >> 8) as u8, (x * 2) as u8, (y * 3) as u8, a].map(|c| c.min(a))
    }).collect();
    let cx = FxCtx { ox: 3, oy: -2, ax: 0, ay: 0, s: 1.0, frame: 7 };
    let check = |name: &str, cpu: &[u8], gpu: &[u8], tol: u8| -> anyhow::Result<()> {
        let mx = cpu.iter().zip(gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        anyhow::ensure!(mx <= tol, "{name}: max channel difference {mx} (allowed {tol})");
        Ok(())
    };
    for sigma in [2.5, 40.0] {
        let mut cpu = img.clone();
        crate::effects::glow(&mut cpu, w, h, [1.0, 0.5, 0.2, 1.0], 1.0, sigma);
        let mut gpu = img.clone();
        let mut ch = Chain::on(g);
        anyhow::ensure!(ch.glow(&mut gpu, w, h, [1.0, 0.5, 0.2, 1.0], 1.0, sigma), "glow not on the GPU");
        ch.finish(&mut gpu)?;
        check("glow", &cpu, &gpu, 0)?;
    }
    let looks = [r#"{"type":"vhs","amount":1,"seed":5}"#, r#"{"type":"glitch","amount":1,"seed":5,"rate":10,"block":6}"#, r#"{"type":"grain","amount":0.3,"seed":5,"live":true,"size":2}"#];
    for j in looks {
        let e: Effect = serde_json::from_str(j)?;
        let mut p = [0.0; NPARAM];
        for (i, f) in e.look_fields().iter().enumerate() {
            p[i] = e.look_field(f);
        }
        p[TIME] = 0.6;
        let mut cpu = img.clone();
        crate::look::apply(&e, &p, &mut cpu, w, h, &cx);
        let mut gpu = img.clone();
        anyhow::ensure!(apply_on(g, &e, &p, &mut gpu, w, h, &cx)?, "{j}: not on the GPU");
        check(j, &cpu, &gpu, 1)?;
    }
    // the warps (kaleido, tunnel texture + perspective) within 1
    let warps = [
        r#"{"type":"kaleido","segments":6,"mirror":true,"center":[40,30],"angle":17,"zoom":1.25,"amount":1}"#,
        r#"{"type":"tunnel","amount":0.8,"depth":0.4,"speed":1.5,"twist":20,"angle":8,"radius":40,"fog":0.2,"fog_color":[0.1,0,0.2,0.8],"center":[50,30],"map":"texture"}"#,
        r#"{"type":"tunnel","amount":0.6,"depth":0.4,"speed":1.5,"twist":20,"angle":8,"radius":40,"fog":0.2,"center":[50,30],"map":"perspective"}"#,
    ];
    for j in warps {
        let e: Effect = serde_json::from_str(j)?;
        let mut p = [0.0; NPARAM];
        for (i, f) in e.look_fields().iter().enumerate() {
            p[i] = e.look_field(f);
        }
        p[TIME] = 0.6;
        let id = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let Some(c) = crate::warp::cfg(&e, &p, id, [0.0, 0.0, w as f64, h as f64], [0, 0, w as i32, h as i32]) else { anyhow::bail!("{j}: no warp") };
        let mut cpu = img.clone();
        crate::warp::apply(&c, &mut cpu, w, h);
        let mut gpu = img.clone();
        let mut ch = Chain::on(g);
        anyhow::ensure!(ch.warp(&c, &mut gpu, w, h), "{j}: not on the GPU");
        ch.finish(&mut gpu)?;
        check(j, &cpu, &gpu, 1)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- warps

/// warp::apply: one pass over a copy of the frame (warp.wgsl, same f32 expressions).
fn warp(a: &mut Active, c: &crate::warp::WarpCfg) {
    let (w, h) = (a.w, a.h);
    let mut q = Params::default();
    q.u(0, w as u32).u(1, h as u32).u(2, c.kind).i(3, c.dom[0]).i(4, c.dom[1]).i(5, c.dom[2]).i(6, c.dom[3]);
    q.f(7, c.c[0]).f(8, c.c[1]).f(9, c.amount).f(10, c.ang).f(11, c.seg).f(12, c.hseg).f(13, c.invseg).u(14, c.mirror as u32).f(15, c.invzoom);
    q.f(16, c.rc).f(17, c.rs).f(18, c.f).f(19, c.invf).u(20, c.plane as u32).f(21, c.rho).f(22, c.yc).f(23, c.cc).f(24, c.gap);
    q.f(25, c.twist).f(26, c.dz).f(27, c.fog).f(28, c.fogc[0]).f(29, c.fogc[1]).f(30, c.fogc[2]).f(31, c.fogc[3]).f(32, c.k).f(33, c.tcx).f(34, c.ty1);
    let len = (w * h * 4) as u64;
    let px = a.px.clone();
    let src = a.temp(len);
    a.job.enc.copy_buffer_to_buffer(&px, 0, &src, 0, len);
    a.job.pass("warp", WARP, 2, "warp", &q, &[&px, &src], [w as u32, h as u32]);
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
        let mut p = [0.0; NPARAM];
        p[..6].copy_from_slice(&[15.0, 1.0, 2.0, 1.3, 0.14, 0.2]);
        out.push(("glass", Effect::Glass { strength: 15.0, blur: 1.0, dispersion: 2.0, thickness: 1.3, highlight: 0.14, rim: 0.2, tint: Some([0.72, 0.9, 1.0, 0.12]) }, p));
        let mut p = [0.0; NPARAM];
        p[..7].copy_from_slice(&[0.7, 1.5, 0.1, 0.6, 0.4, 0.25, 0.1]);
        out.push(("optics", Effect::Optics { streak: 0.7, bokeh: 1.5, distortion: 0.1, aberration: 0.6, vignette: 0.4, halation: 0.25, grain: 0.1, seed: 7 }, p));
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

    /// Premultiplied noise with random alpha (every alpha, every channel <= alpha).
    pub fn noisy(w: usize, h: usize) -> Vec<u8> {
        (0..w * h).flat_map(|i| {
            let v = crate::look::hash(i as i32, 0, 7);
            let a = (v & 255) as u8;
            [(v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8, a].map(|c| c.min(a))
        }).collect()
    }

    pub fn digital_looks() -> Vec<(&'static str, Effect, [f64; NPARAM])> {
        let looks = [
            ("rgbsplit", r#"{"type":"rgbsplit","dx":6,"dy":0,"amount":0.35}"#, 0.0),
            ("rgbsplit-radial", r#"{"type":"rgbsplit","dx":18,"dy":-5,"amount":1,"mode":"radial"}"#, 0.0),
            ("glitch", r#"{"type":"glitch","amount":0.6,"seed":13,"rate":14,"block":9}"#, 1.3),
            ("glitch-fine", r#"{"type":"glitch","amount":1,"seed":42,"rate":8,"block":1.5}"#, 0.4),
            ("vhs", r#"{"type":"vhs","amount":1,"seed":13}"#, 2.7),
            ("vhs-light", r#"{"type":"vhs","amount":0.05,"seed":3}"#, 40.1),
            ("grain", r#"{"type":"grain","amount":0.06,"seed":1,"live":false,"size":1}"#, 0.0),
            ("grain-live", r#"{"type":"grain","amount":0.5,"seed":13,"live":true,"size":0.5}"#, 0.0),
            ("grain-coarse", r#"{"type":"grain","amount":0.07,"seed":3,"live":false,"size":3}"#, 0.0),
            ("grain-big", r#"{"type":"grain","amount":0.5,"seed":13,"live":true,"size":4.5}"#, 0.0),
        ];
        looks.iter().map(|(n, j, t)| {
            let e: Effect = serde_json::from_str(j).unwrap();
            let mut p = [0.0; NPARAM];
            for (i, f) in e.look_fields().iter().enumerate() {
                p[i] = e.look_field(f);
            }
            if e.time_dependent() {
                p[TIME] = *t;
            }
            (*n, e, p)
        }).collect()
    }

    /// GPU digital looks (rgbsplit, glitch, vhs) and grain vs the CPU: at most 1/255 per
    /// channel, on smooth and on noisy sources, at node scales 1 and 1.37.
    #[test]
    fn gpu_digital_looks_match_cpu() {
        let Some(g) = device() else { return };
        let (w, h) = (403, 297);
        let srcs = [("poster", poster(w, h, false)), ("poster-opaque", poster(w, h, true)), ("noisy", noisy(w, h))];
        for (name, e, p) in digital_looks() {
            for sc in [1.0, 1.37] {
                for (sn, img) in &srcs {
                    let c = FxCtx { ox: 37, oy: -11, ax: 13, ay: -7, s: sc, frame: 5 };
                    let mut cpu = img.clone();
                    crate::look::apply(&e, &p, &mut cpu, w, h, &c);
                    let mut gpu = img.clone();
                    assert!(apply_on(g, &e, &p, &mut gpu, w, h, &c).unwrap(), "{name}: not run on GPU");
                    let (mx, psnr, _) = diff(&cpu, &gpu);
                    let off = cpu.iter().zip(&gpu).filter(|(a, b)| a != b).count();
                    if mx > 1 {
                        let i = cpu.iter().zip(&gpu).position(|(a, b)| a.abs_diff(*b) == mx).unwrap() / 4;
                        eprintln!("  worst px ({}, {}): cpu {:?} gpu {:?} src {:?}", i % w, i / w, &cpu[i * 4..i * 4 + 4], &gpu[i * 4..i * 4 + 4], &img[i * 4..i * 4 + 4]);
                    }
                    eprintln!("{name:16} s {sc} {sn:14} max {mx} psnr {psnr:.1} dB, {off} channels differ");
                    assert!(mx <= 1, "{name} s {sc} {sn}: max {mx}");
                    assert!(cpu != *img, "{name}: effect did nothing (bad fixture)");
                }
            }
        }
    }

    /// Warps (kaleido, tunnel): GPU within 1/255 of the CPU reference on every channel.
    #[test]
    fn gpu_warps_match_cpu_within_one() {
        let Some(g) = device() else { return };
        let (w, h) = (403, 297);
        for opaque in [false, true] {
            let img = poster(w, h, opaque);
            for (name, e, p) in crate::warp::tests::cases() {
                let c = crate::warp::cfg(&e, &p, crate::warp::tests::M, [0.0, 0.0, w as f64, h as f64], [0, 0, w as i32, h as i32]).unwrap();
                let mut cpu = img.clone();
                crate::warp::apply(&c, &mut cpu, w, h);
                let mut gpu = img.clone();
                let mut chain = Chain::on(g);
                assert!(chain.warp(&c, &mut gpu, w, h), "{name}: not run on GPU");
                chain.finish(&mut gpu).unwrap();
                let (mx, psnr, over) = diff(&cpu, &gpu);
                if mx > 1 {
                    let i = cpu.iter().zip(&gpu).position(|(a, b)| a.abs_diff(*b) == mx).unwrap() / 4;
                    eprintln!("  worst px ({}, {}): cpu {:?} gpu {:?}", i % w, i / w, &cpu[i * 4..i * 4 + 4], &gpu[i * 4..i * 4 + 4]);
                }
                eprintln!("{name:28} opaque={opaque:5} max {mx} psnr {psnr:.1} dB, >1: {:.4} %", over * 100.0);
                assert!(mx <= 1, "{name} (opaque {opaque}): max {mx}");
                assert_ne!(cpu, img, "{name}: warp did nothing (bad fixture)");
            }
        }
    }

    /// The `#fx` / `#tape` chains of HORIZON (grain, rgbsplit, glitch, vhs on one resident
    /// frame, then a glow) vs the CPU sequence: PSNR >= 45 dB, at most 1e-4 of the channels
    /// off by more than 1.
    #[test]
    fn gpu_digital_chain_matches_cpu_sequence() {
        let Some(g) = device() else { return };
        let (w, h) = (403, 297);
        let fx = digital_looks();
        let pick = |n: &str| fx.iter().find(|x| x.0 == n).unwrap().clone();
        let c = FxCtx { ox: 0, oy: 0, ax: 0, ay: 0, s: 1.0, frame: 9 };
        for img in [poster(w, h, true), noisy(w, h)] {
            let seq = [pick("grain-coarse"), pick("rgbsplit"), pick("glitch"), pick("vhs")];
            let mut cpu = img.clone();
            for (_, e, p) in &seq {
                crate::look::apply(e, p, &mut cpu, w, h, &c);
            }
            crate::effects::glow(&mut cpu, w, h, [1.0, 0.3, 0.8, 1.0], 0.9, 12.0);
            let mut gpu = img.clone();
            let mut ch = Chain::on(g);
            for (n, e, p) in &seq {
                assert!(ch.look(e, p, &mut gpu, w, h, &c), "{n}");
            }
            assert!(ch.glow(&mut gpu, w, h, [1.0, 0.3, 0.8, 1.0], 0.9, 12.0));
            assert!(gpu == img, "the host buffer is untouched until the flush");
            ch.finish(&mut gpu).unwrap();
            let (mx, psnr, over) = diff(&cpu, &gpu);
            eprintln!("digital chain: max {mx} psnr {psnr:.1} dB, >1: {:.4} %", over * 100.0);
            // (a +-1 from one kernel can move a posterized glitch block across a level: the
            // chain's bound is on the share of channels, not the maximum)
            assert!(psnr >= 45.0 && over <= 1e-4, "digital chain: max {mx}, psnr {psnr:.1}, >1: {over}");
        }
    }

    #[test]
    fn gpu_lenses_match_cpu_within_one_lsb() {
        let Some(g) = device() else { return };
        let c = cx(37, -11);
        for (name, e, p) in effects().into_iter().filter(|(_, e, _)| matches!(e, Effect::Glass { .. } | Effect::Optics { .. })) {
            let img = poster(403, 297, false);
            let mut cpu = img.clone();
            let backdrop = poster(403, 297, true);
            if matches!(e, Effect::Glass { .. }) {
                crate::look::apply_with_backdrop(&e, &p, &mut cpu, Some(&backdrop), 403, 297, &c);
            } else {
                crate::look::apply(&e, &p, &mut cpu, 403, 297, &c);
            }
            let mut gpu = img;
            let ran = if matches!(e, Effect::Glass { .. }) { apply_glass_on(g, &e, &p, &mut gpu, &backdrop, 403, 297, &c) } else { apply_on(g, &e, &p, &mut gpu, 403, 297, &c) };
            assert!(ran.unwrap(), "{name}: not run on GPU");
            let mx = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
            assert!(mx <= 1, "{name}: CPU/GPU max channel difference {mx}");
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
        for (name, e, p) in digital_looks() {
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
        for sigma in [0.3, 1.2, 2.5, 4.0, 6.9, 7.2, 12.0, 30.0, 95.0, 400.0, 2000.0] {
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

    /// The GPU reconstruction of a tiled blur / glow (`tiled_finish_on`) is bitwise
    /// TiledBlur::finish (+ the sharp layer over it), also for crops at odd offsets.
    #[test]
    fn gpu_tiled_finish_is_bit_exact() {
        let Some(g) = device() else { return };
        let (w, h) = (1500, 1500);
        // scales 4, 32 and 256 (above the old f32 upsample's limit of 128)
        for sigma in [30.0, 300.0, 1500.0] {
            // (at sigma 1500 the transparent poster blurs to nothing)
            let img = poster(w, h, sigma > 1000.0);
            let mut tb = crate::effects::TiledBlur::new(w, h, sigma).unwrap();
            let sc = tb.scale();
            let t = (256 / sc).max(1) * sc;
            for ty in (0..h).step_by(t) {
                for tx in (0..w).step_by(t) {
                    let (tw, th) = (t.min(w - tx), t.min(h - ty));
                    let tile: Vec<u8> = (ty..ty + th).flat_map(|y| img[(y * w + tx) * 4..(y * w + tx + tw) * 4].to_vec()).collect();
                    tb.add(&tile, tx, ty, tw, th);
                }
            }
            let (cx, cy, cw, ch) = (sc * 3 % w.min(97), sc % 61, 611, 333);
            let sharp: Vec<u8> = (cy..cy + ch).flat_map(|y| img[(y * w + cx) * 4..(y * w + cx + cw) * 4].to_vec()).collect();
            let (small, sw, sh, scale, rs) = tb.reduced();
            for with_sharp in [false, true] {
                let mut gpu = vec![0u8; cw * ch * 4];
                tiled_finish_on(g, &small, sw, sh, scale, rs, &mut gpu, cx, cy, cw, ch, with_sharp.then_some(&sharp[..])).unwrap();
                let mut cpu = vec![0u8; cw * ch * 4];
                let tb2 = tb.clone();
                tb2.finish(&mut cpu, cx, cy, cw, ch);
                if with_sharp {
                    crate::effects::over(&mut cpu, &sharp);
                }
                assert!(cpu == gpu, "sigma {sigma} sharp {with_sharp}: max diff {}", diff(&cpu, &gpu).0);
                assert!(cpu.iter().any(|&v| v != 0), "bad fixture (sigma {sigma}, sharp {with_sharp}, scale {scale}, {sw}x{sh})");
            }
        }
    }

    /// `--gpu=auto`'s start-up self-test passes on the test device.
    #[test]
    fn gpu_self_test_passes() {
        let Some(g) = device() else { return };
        self_test(g).unwrap();
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
