//! Time ring (E4 phase 2 step 3): renders of a time-effect subtree at grid times, kept on
//! the GPU across frames (`timefx_paint::ring_pixmap`), and the per-pixel gather that
//! builds the slit-scan / time-displacement frame from them.
use super::{Gpu, Job, Params};
use std::collections::HashMap;

const TIME: &str = include_str!("time.wgsl");

/// How the gather finds a pixel's grid coordinate.
pub enum Coord<'a> {
    /// one value per column (`x`) or row
    Line { xaxis: bool, c: &'a [f32] },
    /// a u16 map per pixel, c = q * kq
    Plane { q: &'a [u16], kq: f32 },
}

impl Coord<'_> {
    /// Grid coordinate of pixel (x, y) (bitwise what the kernel computes).
    #[inline]
    pub fn at(&self, x: usize, y: usize, w: usize) -> f32 {
        match self {
            Coord::Line { xaxis: true, c } => c[x],
            Coord::Line { xaxis: false, c } => c[y],
            Coord::Plane { q, kq } => q[y * w + x] as f32 * kq,
        }
    }
}

/// `a + (b - a) * fr` rounded as the CPU levels path does.
#[inline]
pub fn lerp8(a: u8, b: u8, fr: f32) -> u8 {
    let fa = a as f32;
    let d = (b as f32 - fa) * fr;
    (fa + d + 0.5) as u8
}

/// Slots of `w x h` renders in one storage buffer, keyed by sample state; least recently
/// used slots are reused.
pub struct Ring {
    g: &'static Gpu,
    pub w: usize,
    pub h: usize,
    buf: wgpu::Buffer,
    keys: Vec<Option<u64>>,
    used: Vec<u64>,
    index: HashMap<u64, usize>,
    stamp: u64,
}

impl Ring {
    /// A ring of `n` slots, None when it does not fit the device.
    pub fn new(g: &'static Gpu, w: usize, h: usize, n: usize) -> Option<Ring> {
        let bytes = (n * w * h * 4) as u64;
        if !g.fits(bytes) || n == 0 {
            return None;
        }
        let buf = g.device.create_buffer(&wgpu::BufferDescriptor { label: Some("time ring"), size: bytes.next_multiple_of(4), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Some(Ring { g, w, h, buf, keys: vec![None; n], used: vec![0; n], index: HashMap::new(), stamp: 0 })
    }

    pub fn slots(&self) -> usize {
        self.keys.len()
    }

    /// Start a frame (slots looked up or filled from here on are pinned for it).
    pub fn begin(&mut self) {
        self.stamp += 1;
    }

    /// The slot holding `key`, pinned for this frame.
    pub fn lookup(&mut self, key: u64) -> Option<u32> {
        let s = *self.index.get(&key)?;
        self.used[s] = self.stamp;
        Some(s as u32)
    }

    /// Upload a render for `key` into a free or least recently used slot not pinned this
    /// frame (None: all slots pinned).
    pub fn put(&mut self, job: &mut Job, key: u64, px: &[u8]) -> Option<u32> {
        let s = (0..self.keys.len()).filter(|&i| self.used[i] != self.stamp).min_by_key(|&i| (self.keys[i].is_some(), self.used[i]))?;
        if let Some(old) = self.keys[s].take() {
            self.index.remove(&old);
        }
        job.write_at(&self.buf, (s * self.w * self.h * 4) as u64, &px[..self.w * self.h * 4]);
        self.keys[s] = Some(key);
        self.index.insert(key, s);
        self.used[s] = self.stamp;
        Some(s as u32)
    }

    /// Gather a `w x h` frame at (dx, dy) of the slots: grid coordinate per pixel,
    /// `slots[j - jlo]` per grid step.
    #[allow(clippy::too_many_arguments)]
    pub fn gather(&self, mut job: Job, coord: &Coord, jlo: i32, slots: &[u32], w: usize, h: usize, dx: usize, dy: usize, out: &mut [u8]) -> anyhow::Result<()> {
        anyhow::ensure!(dx + w <= self.w && dy + h <= self.h, "gather rect outside the ring slots");
        let mut p = Params::default();
        p.u(0, w as u32).u(1, h as u32).i(3, jlo).u(4, slots.len() as u32).u(6, self.w as u32).u(7, self.h as u32).u(8, dx as u32).u(9, dy as u32);
        let cb = match coord {
            Coord::Line { xaxis, c } => {
                p.u(2, if *xaxis { 0 } else { 1 });
                job.upload(bytemuck::cast_slice(c))
            }
            Coord::Plane { q, kq } => {
                p.u(2, 2).f(5, *kq);
                let mut v = q.to_vec();
                if v.len() % 2 == 1 {
                    v.push(0);
                }
                job.upload(bytemuck::cast_slice(&v))
            }
        };
        let sb = job.upload(bytemuck::cast_slice(slots));
        let ob = self.g.buffer((w * h * 4) as u64);
        job.pass("time", TIME, 4, "ring_gather", &p, &[&self.buf, &cb, &sb, &ob], [w as u32, h as u32]);
        job.read(&ob, w * h * 4);
        let r = job.finish(&mut [out]);
        for b in [cb, sb, ob] {
            self.g.give(b);
        }
        r
    }
}

/// The CPU gather (reference and fallback): same coordinates, same rounding. `rasters`
/// are `fw` wide; the output `w x h` sits at (dx, dy) in them.
#[allow(clippy::too_many_arguments)]
pub fn gather_cpu(coord: &Coord, jlo: i32, rasters: &[&[u8]], fw: usize, dx: usize, dy: usize, w: usize, h: usize, out: &mut [u8]) {
    use rayon::prelude::*;
    out.par_chunks_mut(w * 4).enumerate().take(h).for_each(|(y, row)| {
        for x in 0..w {
            let c = coord.at(x, y, w);
            let jf = c.floor();
            let fr = c - jf;
            let j = (jf as i32 - jlo) as usize;
            let i = ((y + dy) * fw + x + dx) * 4;
            let a = &rasters[j][i..i + 4];
            let o = &mut row[x * 4..x * 4 + 4];
            if fr <= 0.0 {
                o.copy_from_slice(a);
            } else {
                let b = &rasters[j + 1][i..i + 4];
                for k in 0..4 {
                    o[k] = lerp8(a[k], b[k], fr);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GPU gather equals the CPU gather (both coordinate kinds, offsets, j ranges with
    /// negative spans).
    #[test]
    fn gpu_ring_gather_matches_cpu() {
        let Some(g) = crate::gpu::fx::tests::device() else { return };
        let (fw, fh, w, h, dx, dy) = (97, 61, 80, 50, 9, 7);
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as u8
        };
        let rasters: Vec<Vec<u8>> = (0..6).map(|_| (0..fw * fh * 4).map(|_| rnd()).collect()).collect();
        let mut ring = Ring::new(g, fw, fh, 8).unwrap();
        ring.begin();
        let mut job = Some(Job::new(g));
        let slots: Vec<u32> = rasters.iter().enumerate().map(|(i, r)| ring.put(job.as_mut().unwrap(), 100 + i as u64, r).unwrap()).collect();
        let lines: Vec<f32> = (0..h).map(|y| -2.0 + 5.0 * y as f32 / (h - 1) as f32).collect();
        let plane: Vec<u16> = (0..w * h).map(|i| ((i * 2654435761usize) >> 7) as u16).collect();
        let kq = -2.9 / 65535.0f32;
        let cases = [(Coord::Line { xaxis: false, c: &lines }, -2), (Coord::Plane { q: &plane, kq }, -3)];
        for (k, (coord, jlo)) in cases.iter().enumerate() {
            let refs: Vec<&[u8]> = rasters.iter().map(|r| &r[..]).collect();
            let mut cpu = vec![0u8; w * h * 4];
            gather_cpu(coord, *jlo, &refs, fw, dx, dy, w, h, &mut cpu);
            let mut gpu = vec![0u8; w * h * 4];
            ring.gather(job.take().unwrap_or_else(|| Job::new(g)), coord, *jlo, &slots, w, h, dx, dy, &mut gpu).unwrap();
            let mx = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            assert!(mx <= 1, "case {k}: max diff {mx}");
            eprintln!("ring gather case {k}: max diff {mx}");
        }
    }
}
