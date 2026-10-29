//! E4 GPU path: wgpu compute kernels for the per-pixel node effects that dominate
//! full-redraw frames (riso, halftone, god rays, soft shadows with their distance field;
//! blur, drop shadow, glow; `fx.rs`). The vector raster stays on vello_cpu (see
//! research/e4-gpu/SPIKE.md for why). Phase 2: a node's consecutive GPU effects run as one
//! `fx::Chain` on a frame that stays on the GPU (one upload, one readback).
//!
//! Mode: `AGENTFX_GPU=0|1|auto` or `agentfx-render render ... --gpu[=on|off|auto]` (default
//! off). `1`/`on` uses the GPU when a hardware adapter exists and says so on stderr when
//! it falls back; `auto` does the same quietly. Software adapters (llvmpipe) are refused
//! unless `AGENTFX_GPU_ALLOW_CPU=1` (tests). Routing: an effect buffer goes to the GPU only
//! when it has at least `AGENTFX_GPU_MIN_PX` pixels (default 65536 = 256x256); small
//! buffers stay on the CPU, where dispatch + transfer would cost more than the effect.
//! `AGENTFX_GPU_FX` picks the effects (fx.rs), `AGENTFX_GPU_CHECK=1` compares every chain with
//! the CPU.
//!
//! Exactness: integer parts (hashes, box blurs, blur reductions, tint/over, squared
//! distances) are bit-identical; float parts differ by GPU rounding (log/exp
//! approximations, FMA, sqrt) and are held to a tolerance by the tests (`fx::tests`,
//! `max channel diff <= 3, PSNR >= 45 dB`).
pub mod fx;
pub mod time;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    On,
    Auto,
}

static MODE: OnceLock<Mode> = OnceLock::new();

fn parse_mode(s: &str) -> Option<Mode> {
    match s {
        "0" | "off" | "cpu" | "false" => Some(Mode::Off),
        "1" | "on" | "gpu" | "true" | "" => Some(Mode::On),
        "auto" => Some(Mode::Auto),
        _ => None,
    }
}

/// `--gpu` / `--gpu=MODE` from the command line (before the first render); else AGENTFX_GPU.
pub fn init_from_args(args: &[String]) {
    for a in args {
        if let Some(v) = a.strip_prefix("--gpu") {
            let v = v.strip_prefix('=').unwrap_or(v);
            if let Some(m) = parse_mode(v) {
                let _ = MODE.set(m);
            } else {
                eprintln!("agentfx-render: unknown --gpu mode {v:?} (on|off|auto)");
            }
        }
    }
}

pub fn mode() -> Mode {
    *MODE.get_or_init(|| agentfx_ir::env::var("AGENTFX_GPU").ok().and_then(|v| parse_mode(&v)).unwrap_or(Mode::Off))
}

/// Smallest effect buffer (px) routed to the GPU.
pub fn min_px() -> usize {
    static V: OnceLock<usize> = OnceLock::new();
    *V.get_or_init(|| agentfx_ir::env::var("AGENTFX_GPU_MIN_PX").ok().and_then(|v| v.parse().ok()).unwrap_or(65536))
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub name: String,
    pub backend: String,
    pub limits: wgpu::Limits,
    modules: Mutex<HashMap<&'static str, Arc<Module>>>,
    pool: Mutex<Vec<wgpu::Buffer>>,
    readback: Mutex<Vec<wgpu::Buffer>>,
    /// mapped upload staging buffers (MAP_WRITE | COPY_SRC), reused across jobs
    staging: Mutex<Vec<wgpu::Buffer>>,
}

struct Module {
    layout: wgpu::BindGroupLayout,
    pipes: Mutex<HashMap<&'static str, Arc<wgpu::ComputePipeline>>>,
    shader: wgpu::ShaderModule,
    pl: wgpu::PipelineLayout,
}

static GPU: OnceLock<Option<Gpu>> = OnceLock::new();

/// The shared device, when the mode allows it and an adapter exists (created once).
pub fn get() -> Option<&'static Gpu> {
    if mode() == Mode::Off {
        return None;
    }
    GPU.get_or_init(|| match Gpu::new() {
        Ok(g) => {
            eprintln!("agentfx-render: GPU effects on {} ({})", g.name, g.backend);
            Some(g)
        }
        Err(e) => {
            if mode() == Mode::On {
                eprintln!("agentfx-render: --gpu: no usable GPU ({e:#}); rendering on the CPU");
            }
            None
        }
    })
    .as_ref()
}

impl Gpu {
    fn new() -> anyhow::Result<Gpu> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        if std::env::var("WGPU_BACKEND").is_err() {
            desc.backends = wgpu::Backends::VULKAN;
        }
        let instance = wgpu::Instance::new(desc);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu && agentfx_ir::env::var("AGENTFX_GPU_ALLOW_CPU").is_err() {
            anyhow::bail!("only a software adapter ({})", info.name);
        }
        let al = adapter.limits();
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: al.max_storage_buffer_binding_size,
            max_buffer_size: al.max_buffer_size,
            max_storage_buffers_per_shader_stage: al.max_storage_buffers_per_shader_stage.min(8),
            ..wgpu::Limits::downlevel_defaults()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("reel"), required_limits: limits.clone(), ..Default::default() }))?;
        device.on_uncaptured_error(Arc::new(|e| eprintln!("agentfx-render: GPU error: {e}")));
        Ok(Gpu {
            device,
            queue,
            name: info.name.clone(),
            backend: format!("{:?} {} {}", info.backend, info.driver, info.driver_info),
            limits,
            modules: Default::default(),
            pool: Default::default(),
            readback: Default::default(),
            staging: Default::default(),
        })
    }

    fn module(&self, name: &'static str, src: &'static str, nbuf: u32) -> Arc<Module> {
        let mut m = self.modules.lock().unwrap();
        m.entry(name)
            .or_insert_with(|| {
                let entries: Vec<wgpu::BindGroupLayoutEntry> = (0..=nbuf)
                    .map(|b| wgpu::BindGroupLayoutEntry {
                        binding: b,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
                        count: None,
                    })
                    .collect();
                let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(name), entries: &entries });
                let pl = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(name), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
                let code = format!("{}\n{}", include_str!("common.wgsl"), src);
                let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: wgpu::ShaderSource::Wgsl(code.into()) });
                Arc::new(Module { layout, pipes: Default::default(), shader, pl })
            })
            .clone()
    }

    fn pipeline(&self, m: &Module, entry: &'static str) -> Arc<wgpu::ComputePipeline> {
        let mut p = m.pipes.lock().unwrap();
        p.entry(entry)
            .or_insert_with(|| Arc::new(self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(entry), layout: Some(&m.pl), module: &m.shader, entry_point: Some(entry), compilation_options: Default::default(), cache: None })))
            .clone()
    }

    /// A storage buffer of at least `size` bytes from the pool (contents undefined).
    pub fn buffer(&self, size: u64) -> wgpu::Buffer {
        let size = size.max(16).next_multiple_of(4);
        take(&self.pool, size).unwrap_or_else(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: size.next_multiple_of(1 << 16), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false })
        })
    }

    pub fn give(&self, b: wgpu::Buffer) {
        give(&self.pool, b, 768 << 20);
    }

    /// Does a storage binding of `size` bytes fit the device limits?
    pub fn fits(&self, size: u64) -> bool {
        size <= self.limits.max_storage_buffer_binding_size as u64 && size <= self.limits.max_buffer_size
    }
}

fn take(pool: &Mutex<Vec<wgpu::Buffer>>, size: u64) -> Option<wgpu::Buffer> {
    let mut p = pool.lock().unwrap();
    let i = p.iter().enumerate().filter(|(_, b)| b.size() >= size && b.size() <= size.saturating_mul(2).max(size + (1 << 20))).min_by_key(|(_, b)| b.size()).map(|(i, _)| i)?;
    Some(p.swap_remove(i))
}

fn give(pool: &Mutex<Vec<wgpu::Buffer>>, b: wgpu::Buffer, cap: u64) {
    let mut p = pool.lock().unwrap();
    p.push(b);
    let mut total: u64 = p.iter().map(|b| b.size()).sum();
    while total > cap && p.len() > 1 {
        let old = p.remove(0);
        total -= old.size();
        old.destroy();
    }
}

/// `AGENTFX_GPU_STAGING=0|1`: uploads through our persistently mapped staging buffers (1) or
/// `queue.write_buffer` (0). Default 1.
pub fn staging_on() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| agentfx_ir::env::var("AGENTFX_GPU_STAGING").map_or(true, |v| v != "0"))
}

/// Parameter block (binding 0): 32-bit words, floats bit-cast.
#[derive(Default, Clone)]
pub struct Params(pub Vec<u32>);

impl Params {
    pub fn at(&mut self, i: usize) -> &mut u32 {
        if self.0.len() <= i {
            self.0.resize(i + 1, 0);
        }
        &mut self.0[i]
    }
    pub fn u(&mut self, i: usize, v: u32) -> &mut Self {
        *self.at(i) = v;
        self
    }
    pub fn i(&mut self, i: usize, v: i32) -> &mut Self {
        *self.at(i) = v as u32;
        self
    }
    pub fn f(&mut self, i: usize, v: f32) -> &mut Self {
        *self.at(i) = v.to_bits();
        self
    }
}

/// One command buffer of compute passes over pooled buffers, ending in readbacks.
pub struct Job<'g> {
    pub g: &'g Gpu,
    pub(crate) enc: wgpu::CommandEncoder,
    reads: Vec<(wgpu::Buffer, u64, usize)>,
    staged: Vec<wgpu::Buffer>,
    scope: wgpu::ErrorScopeGuard,
}

impl<'g> Job<'g> {
    pub fn new(g: &'g Gpu) -> Self {
        let scope = g.device.push_error_scope(wgpu::ErrorFilter::Validation);
        Job { g, enc: g.device.create_command_encoder(&Default::default()), reads: Vec::new(), staged: Vec::new(), scope }
    }

    /// Upload bytes into a pooled buffer (see `write`).
    pub fn upload(&mut self, data: &[u8]) -> wgpu::Buffer {
        let b = self.g.buffer(data.len() as u64);
        self.write(&b, data);
        b
    }

    /// Copy `data` into `dst` at this point of the command stream, through a persistently
    /// mapped staging buffer (no per-call staging allocation and page faults, as
    /// `queue.write_buffer` has; the copy is ordered with the job's passes).
    pub fn write(&mut self, dst: &wgpu::Buffer, data: &[u8]) {
        self.write_at(dst, 0, data);
    }

    /// `write` at byte offset `at` (a multiple of 4).
    pub fn write_at(&mut self, dst: &wgpu::Buffer, at: u64, data: &[u8]) {
        let _p = crate::prof::scope_px("gpu.upload", data.len() as u64 / 4);
        UP_BYTES.fetch_add(data.len() as u64, Ordering::Relaxed);
        if !staging_on() {
            // (before the job's passes: fine for the fresh buffers jobs upload into)
            self.g.queue.write_buffer(dst, at, data);
            return;
        }
        let size = (data.len() as u64).max(4).next_multiple_of(4);
        // (a pooled buffer whose re-map failed is dropped)
        let st = take(&self.g.staging, size).filter(|b| b.slice(..size).get_mapped_range_mut().is_ok()).unwrap_or_else(|| {
            self.g.device.create_buffer(&wgpu::BufferDescriptor { label: Some("staging"), size: size.next_multiple_of(1 << 16), usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: true })
        });
        {
            let mut v = st.slice(..size).get_mapped_range_mut().expect("staging buffer is mapped");
            // (bytes past the data up to the 4-byte copy size are left as they are)
            v.slice(..data.len()).copy_from_slice(data);
        }
        st.unmap();
        self.enc.copy_buffer_to_buffer(&st, 0, dst, at, size);
        self.staged.push(st);
    }

    #[allow(dead_code)]
    pub fn clear(&mut self, b: &wgpu::Buffer) {
        self.enc.clear_buffer(b, 0, None);
    }

    /// Dispatch `entry` of module `name` with params and buffers (bindings 1..). `grid`:
    /// invocations in x, y for `@workgroup_size(16, 16)` entries, or `[n, 0]` for linear
    /// `@workgroup_size(64)` entries (folded into rows of 64 groups).
    pub fn pass(&mut self, name: &'static str, src: &'static str, nbuf: u32, entry: &'static str, p: &Params, bufs: &[&wgpu::Buffer], grid: [u32; 2]) {
        let m = self.g.module(name, src, nbuf);
        let pipe = self.g.pipeline(&m, entry);
        let mut words = p.0.clone();
        if words.is_empty() {
            words.push(0);
        }
        let pb = self.g.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&words), usage: wgpu::BufferUsages::STORAGE });
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: pb.as_entire_binding() }];
        for (i, b) in bufs.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry { binding: i as u32 + 1, resource: b.as_entire_binding() });
        }
        assert_eq!(bufs.len() as u32, nbuf, "{name}::{entry}: buffer count");
        let bg = self.g.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &m.layout, entries: &entries });
        let groups = if grid[1] == 0 {
            let n = grid[0].div_ceil(64);
            (n.min(64), n.div_ceil(64), 1)
        } else {
            (grid[0].div_ceil(16), grid[1].div_ceil(16), 1)
        };
        let mut cp = self.enc.begin_compute_pass(&Default::default());
        cp.set_pipeline(&pipe);
        cp.set_bind_group(0, &bg, &[]);
        cp.dispatch_workgroups(groups.0, groups.1, groups.2);
    }

    /// Read `len` bytes of `b` back after `finish` (in call order).
    pub fn read(&mut self, b: &wgpu::Buffer, len: usize) {
        let size = (len as u64).next_multiple_of(4);
        let rb = take(&self.g.readback, size).unwrap_or_else(|| {
            self.g.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: size.next_multiple_of(1 << 16), usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
        });
        self.enc.copy_buffer_to_buffer(b, 0, &rb, 0, size);
        DOWN_BYTES.fetch_add(len as u64, Ordering::Relaxed);
        self.reads.push((rb, size, len));
    }

    /// Submit, wait, and copy each `read` into the matching output slice.
    pub fn finish(self, outs: &mut [&mut [u8]]) -> anyhow::Result<()> {
        let Job { g, enc, reads, staged, scope } = self;
        let _p = crate::prof::scope("gpu.wait");
        g.queue.submit([enc.finish()]);
        let err = pollster::block_on(scope.pop());
        if let Some(e) = err {
            for (rb, ..) in reads {
                give(&g.readback, rb, 256 << 20);
            }
            anyhow::bail!("GPU validation: {e}");
        }
        for (rb, size, _) in &reads {
            rb.slice(..*size).map_async(wgpu::MapMode::Read, |r| {
                if let Err(e) = r {
                    eprintln!("agentfx-render: GPU map failed: {e}");
                }
            });
        }
        // staging buffers: map again for the next job (idle now: resolves on this poll)
        for st in &staged {
            st.slice(..).map_async(wgpu::MapMode::Write, |_| {});
        }
        g.device.poll(wgpu::PollType::wait_indefinitely())?;
        drop(_p);
        for st in staged {
            give(&g.staging, st, 256 << 20);
        }
        let _p = crate::prof::scope("gpu.readback");
        for ((rb, size, len), out) in reads.into_iter().zip(outs.iter_mut()) {
            {
                let v = rb.slice(..size).get_mapped_range().map_err(|e| anyhow::anyhow!("GPU map: {e:?}"))?;
                out[..len].copy_from_slice(&v[..len]);
            }
            rb.unmap();
            give(&g.readback, rb, 256 << 20);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- stats

pub static CALLS: AtomicU64 = AtomicU64::new(0);
pub static PX: AtomicU64 = AtomicU64::new(0);
pub static NANOS: AtomicU64 = AtomicU64::new(0);
pub static CPU_ROUTED: AtomicU64 = AtomicU64::new(0);
pub static FAILED: AtomicU64 = AtomicU64::new(0);
pub static CHAINS: AtomicU64 = AtomicU64::new(0);
pub static UP_BYTES: AtomicU64 = AtomicU64::new(0);
pub static DOWN_BYTES: AtomicU64 = AtomicU64::new(0);

pub(crate) fn account(calls: usize, px: usize, t: Instant) {
    CALLS.fetch_add(calls as u64, Ordering::Relaxed);
    CHAINS.fetch_add(1, Ordering::Relaxed);
    PX.fetch_add((px * calls) as u64, Ordering::Relaxed);
    NANOS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// Render stats block (`"gpu"` in --stats JSON); null when the GPU path is off.
pub fn stats_json() -> serde_json::Value {
    let Some(g) = GPU.get().and_then(|g| g.as_ref()) else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "adapter": g.name,
        "backend": g.backend,
        "effect_calls": CALLS.load(Ordering::Relaxed),
        "chains": CHAINS.load(Ordering::Relaxed),
        "upload_mb": (UP_BYTES.load(Ordering::Relaxed) as f64 / 1e5).round() / 10.0,
        "readback_mb": (DOWN_BYTES.load(Ordering::Relaxed) as f64 / 1e5).round() / 10.0,
        "effect_mpx": (PX.load(Ordering::Relaxed) as f64 / 1e5).round() / 10.0,
        "effect_s": NANOS.load(Ordering::Relaxed) as f64 / 1e9,
        "cpu_routed_small": CPU_ROUTED.load(Ordering::Relaxed),
        "failed_fell_back": FAILED.load(Ordering::Relaxed),
        "min_px": min_px(),
    })
}
