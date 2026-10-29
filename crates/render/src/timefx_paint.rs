//! Painter side of the time effects (X3, see `timefx.rs`): render a node's subtree at other
//! times than the frame's and combine the renders. Child module of `vraster` (uses the
//! painter's private render helpers).
use super::*;
use crate::timefx::{self, SPAN_EPS};
use agentfx_ir::{TimeFx, TimeMap};

/// Byte budget of the per-painter cache of time-sample rasters (`AGENTFX_TIME_CACHE_MB`).
fn time_cache_limit() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| agentfx_ir::env::var("AGENTFX_TIME_CACHE_MB").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(192) << 20)
}

/// `AGENTFX_TIME_RING=1`: slit-scan / displace through the time ring (`ring_pixmap`).
fn ring_on() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| agentfx_ir::env::var("AGENTFX_TIME_RING").is_ok_and(|v| v != "0"))
}

/// GPU memory budget (MB) of one painter's time ring (`AGENTFX_TIME_RING_MB`, default 1024).
fn ring_mb() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| agentfx_ir::env::var("AGENTFX_TIME_RING_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(1024))
}

/// A subtree evaluated at a sample time.
struct Sample {
    t: f64,
    states: Vec<NodeState>,
    frame: usize,
    key: u64,
}

impl<'a> Painter<'a> {
    /// The node's subtree with its time effect applied, in screen rect `o` (premultiplied).
    pub(super) fn time_pixmap(&mut self, view: &View, f: usize, n: NodeId, o: [i32; 4], matte: bool) -> Result<Pixmap> {
        let scene = self.scene;
        let tf = scene.nodes[n].time.as_ref().expect("time effect");
        let span = timefx::span_at(scene, &self.sched.tracks[n], tf, view.t);
        if span.abs() < SPAN_EPS {
            return self.render_view(view, f, o, Some(n), 0, matte);
        }
        let _p = crate::prof::scope_px(match tf {
            TimeFx::Echo { .. } => "time.echo",
            TimeFx::Slitscan { .. } => "time.slitscan",
            _ => "time.levels",
        }, o[2] as u64 * o[3] as u64);
        self.time_depth += 1;
        let r = match tf {
            TimeFx::Echo { .. } => self.echo_pixmap(view, f, n, o, matte, tf, span),
            _ => match self.ring_pixmap(view, f, n, o, matte, tf, span) {
                Ok(Some(px)) => Ok(px),
                Ok(None) => self.levels_pixmap(view, f, n, o, matte, tf, span),
                Err(e) => Err(e),
            },
        };
        self.time_depth -= 1;
        r
    }

    fn sample(&self, n: NodeId, ts: f64) -> Result<Sample> {
        let _p = crate::prof::scope("time.eval");
        let scene = self.scene;
        let base_m: Vec<agentfx_ir::Affine> = scene.nodes.iter().map(|x| agentfx_ir::eval::transform_matrix(&x.transform)).collect();
        let locals = schedule::eval_locals(scene, &self.sched.tracks, &base_m, ts)?;
        let states = schedule::chains(&self.sched.preorder, &self.sched.parents, &locals);
        let frame = timefx::frame_of(ts, scene.fps as f64, self.sched.frames);
        let mut s = Sample { t: ts, states, frame, key: 0 };
        s.key = {
            let v = View { t: ts, states: &s.states, content: &self.sched.content[frame] };
            let mut h = std::collections::hash_map::DefaultHasher::new();
            0x7173u32.hash(&mut h);
            n.hash(&mut h);
            for c in k(&v.states[n].chain).as_coeffs() {
                ((c * 1e9).round() as i64).hash(&mut h);
            }
            v.states[n].opacity.to_bits().hash(&mut h);
            self.hash_subtree(&v, n, Affine::IDENTITY, &mut h);
            h.finish()
        };
        Ok(s)
    }

    /// Render sample `s` of node `n`'s inner subtree into screen rect `r` (cached by state).
    fn sample_raster(&mut self, f: usize, n: NodeId, s: &Sample, r: [i32; 4], matte: bool) -> Result<Arc<Pixmap>> {
        let key = (s.key, r, matte);
        if let Some(p) = self.time_cache.get(&key) {
            self.stats.time_hits += 1;
            return Ok(p.clone());
        }
        let v = View { t: s.t, states: &s.states, content: &self.sched.content[s.frame] };
        let px = Arc::new(self.render_view(&v, f, r, Some(n), 0, matte)?);
        self.stats.time_renders += 1;
        self.stats.time_px += (r[2] as u64) * (r[3] as u64);
        let bytes = r[2] as usize * r[3] as usize * 4;
        if self.time_cache_bytes + bytes > time_cache_limit() {
            self.time_cache.clear();
            self.time_cache_bytes = 0;
        }
        if bytes <= time_cache_limit() / 4 {
            self.time_cache_bytes += bytes;
            self.time_cache.insert(key, px.clone());
        }
        Ok(px)
    }

    /// Screen ink of `n`'s subtree in sample `s` (content bounds through the sample chains,
    /// grown by the effect margins inside the subtree).
    fn sample_ink(&self, n: NodeId, s: &Sample) -> [f64; 4] {
        let scene = self.scene;
        let mut r = [0.0; 4];
        let mut st = vec![(n, 0.0f64)];
        while let Some((x, m)) = st.pop() {
            let ns = &s.states[x];
            if ns.opacity <= 0.0 {
                continue;
            }
            let mut m2 = m;
            if x != n {
                let sc = ns.chain.max_scale();
                for (i, e) in scene.nodes[x].effects.iter().enumerate() {
                    let p = schedule::effect_params(scene, &self.sched.tracks[x], i, e, s.t);
                    m2 += sc * match e {
                        Effect::Blur { .. } | Effect::Glow { .. } => schedule::BLUR_EXTENT * p[0],
                        Effect::DropShadow { .. } => schedule::BLUR_EXTENT * p[0] + p[1].abs().max(p[2].abs()),
                        Effect::Matte { .. } => 0.0,
                        other => crate::look::margin(other, &p, 1.0, [scene.width as f64, scene.height as f64]),
                    };
                }
            }
            if let Some(b) = self.info.bounds(scene, x, s.t) {
                r = union(r, schedule::inflate(transform_bbox(&ns.chain, b), m2 + 2.0));
            }
            for &c in &scene.nodes[x].children {
                st.push((c, m2));
            }
        }
        r
    }

    /// Echo: copies at `t - k * span` (oldest first, opacity decay^k, tint, blend) under the
    /// frame's subtree.
    #[allow(clippy::too_many_arguments)]
    fn echo_pixmap(&mut self, view: &View, f: usize, n: NodeId, o: [i32; 4], matte: bool, tf: &TimeFx, span: f64) -> Result<Pixmap> {
        let TimeFx::Echo { count, decay, blend, tint, .. } = tf else { unreachable!() };
        let (w, h) = (o[2] as usize, o[3] as usize);
        let mut acc = vec![0u8; w * h * 4];
        let oo = [o[0] as f64, o[1] as f64, o[2] as f64, o[3] as f64];
        for kk in (1..=*count as usize).rev() {
            let s = self.sample(n, view.t - span * kk as f64)?;
            let Some(r) = irect(intersect(self.sample_ink(n, &s), oo)) else { continue };
            let px = self.sample_raster(f, n, &s, r, matte)?;
            let mut buf = px.data_as_u8_slice().to_vec();
            let c = match tint.len() {
                0 => None,
                1 => Some(tint[0]),
                _ => {
                    let u = if *count > 1 { (kk - 1) as f64 / (*count - 1) as f64 } else { 0.0 };
                    Some(std::array::from_fn(|j| tint[0][j] + (tint[1][j] - tint[0][j]) * u))
                }
            };
            timefx::tint_scale(&mut buf, c, decay.powi(kk as i32));
            timefx::blend_into(&mut acc, w, h, &buf, r[2] as usize, r[3] as usize, (r[0] - o[0]) as i64, (r[1] - o[1]) as i64, *blend);
        }
        let cur = self.render_view(view, f, o, Some(n), 0, matte)?;
        timefx::blend_into(&mut acc, w, h, cur.data_as_u8_slice(), w, h, 0, 0, agentfx_ir::BlendMode::Normal);
        let mut p = Pixmap::new(w as u16, h as u16);
        p.data_as_u8_slice_mut().copy_from_slice(&acc);
        Ok(p)
    }

    /// Slit-scan and time displacement: every pixel has a level coordinate x in
    /// 0..levels-1 (time `t - span * x / (levels-1)`); it shows the lerp of the renders at
    /// levels floor(x) and floor(x)+1. Consecutive levels with identical subtree states share
    /// one render; each render covers only the pixels (per tile) that need it.
    #[allow(clippy::too_many_arguments)]
    fn levels_pixmap(&mut self, view: &View, f: usize, n: NodeId, o: [i32; 4], matte: bool, tf: &TimeFx, span: f64) -> Result<Pixmap> {
        let scene = self.scene;
        let (w, h) = (o[2] as usize, o[3] as usize);
        let mut nlev = timefx::levels(scene, tf);
        if let TimeFx::Slitscan { band, .. } = tf {
            // adaptive bands: refine the `band` spacing (down to 1 px) until the subtree moves
            // at most ~1 px between neighbouring time samples; measured over the whole window
            // (not just this rect), so partial redraws match full ones
            let (l0, l1) = (0usize, nlev - 1);
            let mut step = 0.0f64;
            let mut prev: Option<Vec<[f64; 2]>> = None;
            for l in l0..=l1 {
                let s = self.sample(n, timefx::level_time(tf, view.t, span, l, nlev))?;
                let pts = self.sample_corners(n, &s);
                if let Some(p) = &prev {
                    let d = p.iter().zip(&pts).map(|(x, y)| (x[0] - y[0]).hypot(x[1] - y[1])).fold(if p.len() == pts.len() { 0.0 } else { f64::INFINITY }, f64::max);
                    step = step.max(d);
                }
                prev = Some(pts);
            }
            let r = step.ceil().clamp(1.0, band.floor().max(1.0)) as usize;
            nlev = (nlev - 1) * r + 1;
        }
        let top = (nlev - 1) as f32;
        // level coordinate per pixel
        let mut lv = vec![0f32; w * h];
        let tile = match tf {
            TimeFx::Slitscan { axis, range, .. } => {
                let (a, b) = timefx::slit_range(scene, axis, range);
                let ux = |p: f64| (((p + 0.5 - a) / (b - a)).clamp(0.0, 1.0) as f32) * top;
                if axis == "x" {
                    let cols: Vec<f32> = (0..w).map(|x| ux((o[0] as usize + x) as f64)).collect();
                    for row in lv.chunks_exact_mut(w) {
                        row.copy_from_slice(&cols);
                    }
                } else {
                    for (y, row) in lv.chunks_exact_mut(w).enumerate() {
                        row.fill(ux((o[1] as usize + y) as f64));
                    }
                }
                w.max(h)
            }
            TimeFx::Displace { map: TimeMap::Layer { node: src }, .. } => {
                let src = *src;
                let mut c2 = self.take_ctx(w as u16, h as u16, 0);
                let b2 = Affine::translate((-o[0] as f64, -o[1] as f64));
                let reg = [o[0] as f64, o[1] as f64, o[2] as f64, o[3] as f64];
                self.draw_node(&mut c2, view, f, src, b2, reg, 1.0, true)?;
                c2.flush();
                let mut mp = Pixmap::new(w as u16, h as u16);
                c2.render(&mut mp, &mut self.res);
                self.give_ctx(c2);
                for (v, p) in lv.iter_mut().zip(mp.data_as_u8_slice().chunks_exact(4)) {
                    let l = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0;
                    *v = l.clamp(0.0, 1.0) * top;
                }
                self.time_tile
            }
            TimeFx::Displace { map, .. } => {
                for (y, row) in lv.chunks_exact_mut(w).enumerate() {
                    let sy = (o[1] as usize + y) as f64 + 0.5;
                    for (x, v) in row.iter_mut().enumerate() {
                        *v = timefx::map_u(map, (o[0] as usize + x) as f64 + 0.5, sy) * top;
                    }
                }
                self.time_tile
            }
            TimeFx::Echo { .. } => unreachable!(),
        };
        // levels in use, their samples and runs of identical states
        let (mut lo, mut hi) = (nlev - 1, 0usize);
        for &x in &lv {
            let i = (x as usize).min(nlev - 2);
            lo = lo.min(i);
            hi = hi.max(i + 1);
        }
        let mut run_of = vec![usize::MAX; nlev];
        let mut samples: Vec<Sample> = vec![];
        let mut prev_key = None;
        for l in lo..=hi {
            let s = self.sample(n, timefx::level_time(tf, view.t, span, l, nlev))?;
            if prev_key != Some(s.key) {
                prev_key = Some(s.key);
                samples.push(s);
            }
            run_of[l] = samples.len() - 1;
        }
        let nruns = samples.len();
        let mut out = Pixmap::new(w as u16, h as u16);
        if nruns == 1 {
            // the subtree rests over the whole window: one render
            let s = samples.pop().unwrap();
            let px = self.sample_raster(f, n, &s, o, matte)?;
            out.data_as_u8_slice_mut().copy_from_slice(px.data_as_u8_slice());
            return Ok(out);
        }
        if let (TimeFx::Slitscan { axis, .. }, true) = (tf, self.slit_two_pass) {
            return self.slit_two_pass(f, n, o, matte, axis == "x", &lv, &run_of, &samples, nlev);
        }
        let outb = out.data_as_u8_slice_mut();
        for ty in (0..h).step_by(tile) {
            for tx in (0..w).step_by(tile) {
                let (tw, th) = (tile.min(w - tx), tile.min(h - ty));
                // bbox (tile-local) of the pixels needing each run
                let mut bb = vec![[usize::MAX, usize::MAX, 0usize, 0usize]; nruns];
                let mut used = vec![];
                for y in 0..th {
                    let row = &lv[(ty + y) * w + tx..(ty + y) * w + tx + tw];
                    for (x, &v) in row.iter().enumerate() {
                        let i = (v as usize).min(nlev - 2);
                        let fr = v - i as f32;
                        let ra = run_of[i];
                        let rb = run_of[i + 1];
                        let need: &[usize] = if rb != ra && fr > 0.0 { &[ra, rb] } else { &[ra] };
                        for &r in need {
                            let b = &mut bb[r];
                            if b[0] == usize::MAX {
                                used.push(r);
                                *b = [x, y, x + 1, y + 1];
                            } else {
                                b[0] = b[0].min(x);
                                b[1] = b[1].min(y);
                                b[2] = b[2].max(x + 1);
                                b[3] = b[3].max(y + 1);
                            }
                        }
                    }
                }
                let mut rasters: Vec<Option<(Arc<Pixmap>, [usize; 4])>> = vec![None; nruns];
                for &r in &used {
                    let b = bb[r];
                    let rect = [o[0] + (tx + b[0]) as i32, o[1] + (ty + b[1]) as i32, (b[2] - b[0]) as i32, (b[3] - b[1]) as i32];
                    let px = self.sample_raster(f, n, &samples[r], rect, matte)?;
                    rasters[r] = Some((px, [tx + b[0], ty + b[1], b[2] - b[0], b[3] - b[1]]));
                }
                let get = |r: usize, x: usize, y: usize| -> [u8; 4] {
                    let (p, b) = rasters[r].as_ref().unwrap();
                    let i = ((y - b[1]) * b[2] + (x - b[0])) * 4;
                    let d = p.data_as_u8_slice();
                    [d[i], d[i + 1], d[i + 2], d[i + 3]]
                };
                // tile rows in parallel (same arithmetic per pixel)
                use rayon::prelude::*;
                outb.par_chunks_mut(w * 4).enumerate().skip(ty).take(th).for_each(|(y, orow)| {
                    for x in tx..tx + tw {
                        let v = lv[y * w + x];
                        let i = (v as usize).min(nlev - 2);
                        let fr = v - i as f32;
                        let (ra, rb) = (run_of[i], run_of[i + 1]);
                        let a = get(ra, x, y);
                        let px = if ra == rb || fr <= 0.0 {
                            a
                        } else {
                            let b = get(rb, x, y);
                            std::array::from_fn(|j| (a[j] as f32 + (b[j] as f32 - a[j] as f32) * fr + 0.5) as u8)
                        };
                        orow[x * 4..x * 4 + 4].copy_from_slice(&px);
                    }
                });
            }
        }
        Ok(out)
    }

    /// Time ring (E4 phase 2 step 3, `AGENTFX_TIME_RING=1`): slit-scan / displace from renders
    /// of the subtree on a fixed time grid (`AGENTFX_TIME_RING_SUB` = m steps per frame, a
    /// power of two), every pixel lerping the two
    /// renders around its time `t - span * u(p)` (`m` = 2 by default, `auto`: adaptive). Grid times are shared by consecutive
    /// frames, so a frame renders about `m` new samples and reuses the rest: on the GPU the
    /// renders stay in a ring of slots (`gpu::time`) and the gather runs there, else the
    /// time cache holds them and the gather runs on the CPU (same arithmetic). The renders
    /// cover a fixed rect (the frame + 64 px) so they are reusable while the ink moves.
    /// The sampling differs from the default path (band / `levels` quantization), so this is
    /// opt-in. None: not applicable (subtree depends on the frame beyond its time: motion
    /// blur, live grain, fluids; rect outside the fixed one; ring over budget).
    #[allow(clippy::too_many_arguments)]
    fn ring_pixmap(&mut self, view: &View, f: usize, n: NodeId, o: [i32; 4], matte: bool, tf: &TimeFx, span: f64) -> Result<Option<Pixmap>> {
        if !ring_on() {
            return Ok(None);
        }
        let scene = self.scene;
        // subtree pixels must be a function of the sample time alone
        let mut st: Vec<NodeId> = scene.nodes[n].children.clone();
        while let Some(x) = st.pop() {
            let nd = &scene.nodes[x];
            if nd.motion_blur.is_some() || nd.effects.iter().any(|e| matches!(e, Effect::Grain { live: true, .. })) || matches!(nd.content, Some(agentfx_ir::Content::Fluid(_))) {
                return Ok(None);
            }
            st.extend(nd.children.iter().copied());
        }
        let fr = [-64, -64, scene.width as i32 + 128, scene.height as i32 + 128];
        if o[0] < fr[0] || o[1] < fr[1] || o[0] + o[2] > fr[0] + fr[2] || o[1] + o[3] > fr[1] + fr[3] {
            return Ok(None);
        }
        let (w, h) = (o[2] as usize, o[3] as usize);
        let (fw, fh) = (fr[2] as usize, fr[3] as usize);
        let (dx, dy) = ((o[0] - fr[0]) as usize, (o[1] - fr[1]) as usize);
        let fps = scene.fps as f64;
        let t = view.t;
        // grid steps per frame
        let sub = agentfx_ir::env::var("AGENTFX_TIME_RING_SUB").unwrap_or_default();
        let m = match sub.parse::<usize>().ok() {
            Some(m) => m.clamp(1, 16).next_power_of_two(),
            // default 2 steps per frame; "auto": as fine as ~1 px of motion per step (up
            // to 8; costs up to 8 renders per frame)
            None if sub != "auto" => 2,
            None => {
                let nf = (span.abs() * fps).ceil() as usize;
                let mut d = 0f64;
                let mut prev: Option<Vec<[f64; 2]>> = None;
                for j in 0..=nf {
                    let s = self.sample(n, t - span.signum() * j as f64 / fps)?;
                    let pts = self.sample_corners(n, &s);
                    if let Some(p) = &prev {
                        if p.len() == pts.len() {
                            d = d.max(p.iter().zip(&pts).map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1])).fold(0.0, f64::max));
                        }
                    }
                    prev = Some(pts);
                }
                // powers of two: a coarser grid is a subset of a finer one (reuse when m changes)
                (d.ceil() as usize).clamp(1, 8).next_power_of_two()
            }
        };
        // grid times from integer indices (bitwise the same time in every frame); the frame
        // time must sit on the grid (not inside a motion-blurred ancestor's sub-frame samples)
        let gf = t * (m as f64) * fps;
        if (gf - gf.round()).abs() > 1e-6 {
            return Ok(None);
        }
        let g0 = gf.round() as i64;
        let kf = span * m as f64 * fps; // grid coordinate at u = 1
        // grid coordinate per line / pixel (u in 0..1)
        let mut line: Vec<f32> = vec![];
        let mut plane: Vec<u16> = vec![];
        let kq = (kf / 65535.0) as f32;
        let (coord_kind, cmin, cmax) = match tf {
            TimeFx::Slitscan { axis, range, .. } => {
                let (a, b) = timefx::slit_range(scene, axis, range);
                let xaxis = axis == "x";
                let (p0, len) = if xaxis { (o[0], w) } else { (o[1], h) };
                line = (0..len).map(|i| ((((p0 + i as i32) as f64 + 0.5 - a) / (b - a)).clamp(0.0, 1.0) * kf) as f32).collect();
                let (lo, hi) = line.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &c| (lo.min(c), hi.max(c)));
                (Some(xaxis), lo, hi)
            }
            TimeFx::Displace { map, .. } => {
                let us: Vec<f32> = match map {
                    TimeMap::Layer { node: src } => {
                        let src = *src;
                        let mut c2 = self.take_ctx(w as u16, h as u16, 0);
                        let b2 = Affine::translate((-o[0] as f64, -o[1] as f64));
                        let reg = [o[0] as f64, o[1] as f64, o[2] as f64, o[3] as f64];
                        self.draw_node(&mut c2, view, f, src, b2, reg, 1.0, true)?;
                        c2.flush();
                        let mut mp = Pixmap::new(w as u16, h as u16);
                        c2.render(&mut mp, &mut self.res);
                        self.give_ctx(c2);
                        mp.data_as_u8_slice().chunks_exact(4).map(|p| ((0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0).clamp(0.0, 1.0)).collect()
                    }
                    _ => {
                        use rayon::prelude::*;
                        let mut v = vec![0f32; w * h];
                        v.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                            let sy = (o[1] as usize + y) as f64 + 0.5;
                            for (x, u) in row.iter_mut().enumerate() {
                                *u = timefx::map_u(map, (o[0] as usize + x) as f64 + 0.5, sy);
                            }
                        });
                        v
                    }
                };
                plane = us.iter().map(|&u| (u.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect();
                let (qlo, qhi) = plane.iter().fold((u16::MAX, 0u16), |(lo, hi), &q| (lo.min(q), hi.max(q)));
                let (a, b) = (qlo as f32 * kq, qhi as f32 * kq);
                (None, a.min(b), a.max(b))
            }
            TimeFx::Echo { .. } => return Ok(None),
        };
        let coord = match coord_kind {
            Some(xaxis) => crate::gpu::time::Coord::Line { xaxis, c: &line },
            None => crate::gpu::time::Coord::Plane { q: &plane, kq },
        };
        let jlo = cmin.floor() as i32;
        let jhi = cmax.floor() as i32 + (cmax.fract() > 0.0) as i32;
        let nj = (jhi - jlo + 1) as usize;
        let slot_bytes = fw * fh * 4;
        let budget = ring_mb() << 20;
        if nj * slot_bytes > budget {
            return Ok(None);
        }
        // samples at the grid times, keyed by state (+ the fixed rect and pass)
        let mut samples = Vec::with_capacity(nj);
        for j in jlo..=jhi {
            let mut s = self.sample(n, (g0 - j as i64) as f64 / (m as f64 * fps))?;
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            (s.key, fr, matte, 0x7269u32).hash(&mut hs);
            s.key = hs.finish();
            samples.push(s);
        }
        let mut out = Pixmap::new(w as u16, h as u16);
        self.stats.time_ring_frames += 1;
        if let Some(gpu) = crate::gpu::get() {
            let mut ring = match self.time_ring.take() {
                Some(r) if r.w == fw && r.h == fh && r.slots() >= nj => r,
                _ => {
                    let want = (nj + 2 * m + 2).min(budget / slot_bytes).max(nj);
                    match crate::gpu::time::Ring::new(gpu, fw, fh, want) {
                        Some(r) => r,
                        None => return Ok(None),
                    }
                }
            };
            ring.begin();
            let mut job = crate::gpu::Job::new(gpu);
            let mut slots = Vec::with_capacity(nj);
            let mut res = Ok(());
            for s in &samples {
                let slot = match ring.lookup(s.key) {
                    Some(x) => {
                        self.stats.time_hits += 1;
                        x
                    }
                    None => {
                        let v = View { t: s.t, states: &s.states, content: &self.sched.content[s.frame] };
                        // rendered as of the sample's own frame (schedule inks etc.): a ring
                        // render is a function of its time alone, whichever frame needs it
                        let px = match self.render_view(&v, s.frame, fr, Some(n), 0, matte) {
                            Ok(px) => px,
                            Err(e) => {
                                res = Err(e);
                                break;
                            }
                        };
                        self.stats.time_renders += 1;
                        self.stats.time_px += slot_bytes as u64 / 4;
                        match ring.put(&mut job, s.key, px.data_as_u8_slice()) {
                            Some(x) => x,
                            None => {
                                res = Err(anyhow::anyhow!("time ring: no free slot"));
                                break;
                            }
                        }
                    }
                };
                slots.push(slot);
            }
            let r = res.and_then(|_| {
                let _p = crate::prof::scope("gpu.time");
                ring.gather(job, &coord, jlo, &slots, w, h, dx, dy, out.data_as_u8_slice_mut())
            });
            self.time_ring = Some(ring);
            match r {
                Ok(()) => return Ok(Some(out)),
                Err(e) => {
                    // drop the ring (its slots may be stale) and gather on the CPU
                    self.time_ring = None;
                    if crate::gpu::FAILED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3 {
                        eprintln!("agentfx-render: GPU time ring failed, CPU fallback: {e:#}");
                    }
                }
            }
        }
        let mut rasters = Vec::with_capacity(nj);
        for s in &samples {
            rasters.push(self.sample_raster(s.frame, n, s, fr, matte)?);
        }
        let refs: Vec<&[u8]> = rasters.iter().map(|p| p.data_as_u8_slice()).collect();
        crate::gpu::time::gather_cpu(&coord, jlo, &refs, fw, dx, dy, w, h, out.data_as_u8_slice_mut());
        Ok(Some(out))
    }

    /// Slit-scan in two renders: every row (column) blends one even and one odd level, so all
    /// even levels are drawn into one context (each clipped to its own strip of rows, strips
    /// of even levels never overlap) and all odd levels into another; the output lerps the two.
    /// Same pixels as one render per level, without the per-render fixed cost.
    #[allow(clippy::too_many_arguments)]
    fn slit_two_pass(&mut self, f: usize, n: NodeId, o: [i32; 4], matte: bool, xaxis: bool, lv: &[f32], run_of: &[usize], samples: &[Sample], nlev: usize) -> Result<Pixmap> {
        let (w, h) = (o[2] as usize, o[3] as usize);
        let len = if xaxis { w } else { h };
        // per position along the axis: level coordinate
        let pos: Vec<f32> = (0..len).map(|p| if xaxis { lv[p] } else { lv[p * w] }).collect();
        // extent [first, last] of positions needing each level
        let mut ext = vec![(usize::MAX, 0usize); nlev];
        for (p, &v) in pos.iter().enumerate() {
            let i = (v as usize).min(nlev - 2);
            let fr = v - i as f32;
            let mut mark = |l: usize| {
                let e = &mut ext[l];
                e.0 = e.0.min(p);
                e.1 = e.1.max(p);
            };
            mark(i);
            if fr > 0.0 {
                mark(i + 1);
            }
        }
        let mut passes: Vec<Pixmap> = Vec::with_capacity(2);
        for parity in 0..2 {
            let mut ctx = self.take_ctx(w as u16, h as u16, 0);
            let base = Affine::translate((-o[0] as f64, -o[1] as f64));
            self.iso += 1;
            let saved = std::mem::replace(&mut self.origin, (o[0] as i64, o[1] as i64));
            let mut res = Ok(());
            for l in (parity..nlev).step_by(2) {
                let (a, b) = ext[l];
                if a == usize::MAX {
                    continue;
                }
                let (lo, hi) = (a as f64, (b + 1) as f64);
                let strip = if xaxis { Rect::new(lo, 0.0, hi, h as f64) } else { Rect::new(0.0, lo, w as f64, hi) };
                let region = [o[0] as f64 + strip.x0, o[1] as f64 + strip.y0, strip.width(), strip.height()];
                let s = &samples[run_of[l]];
                let v = View { t: s.t, states: &s.states, content: &self.sched.content[s.frame] };
                ctx.set_transform(Affine::IDENTITY);
                ctx.set_fill_rule(Fill::NonZero);
                ctx.push_clip_path(&strip.to_path(0.1));
                res = self.draw_inner(&mut ctx, &v, f, n, base, region, 1.0, matte);
                ctx.pop_clip();
                self.stats.time_renders += 1;
                self.stats.time_px += (strip.width() * strip.height()) as u64;
                if res.is_err() {
                    break;
                }
            }
            self.origin = saved;
            self.iso -= 1;
            res?;
            ctx.flush();
            let mut px = Pixmap::new(w as u16, h as u16);
            ctx.render(&mut px, &mut self.res);
            self.give_ctx(ctx);
            passes.push(px);
        }
        let (pa, pb) = (passes[0].data_as_u8_slice(), passes[1].data_as_u8_slice());
        let mut out = Pixmap::new(w as u16, h as u16);
        let ob = out.data_as_u8_slice_mut();
        // rows in parallel (same arithmetic per pixel)
        use rayon::prelude::*;
        ob.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let v = pos[if xaxis { x } else { y }];
                let i = (v as usize).min(nlev - 2);
                let fr = v - i as f32;
                let k = (y * w + x) * 4;
                let (a, b) = if i % 2 == 0 { (pa, pb) } else { (pb, pa) };
                let o = &mut row[x * 4..x * 4 + 4];
                if fr <= 0.0 {
                    o.copy_from_slice(&a[k..k + 4]);
                } else {
                    for j in 0..4 {
                        o[j] = (a[k + j] as f32 + (b[k + j] as f32 - a[k + j] as f32) * fr + 0.5) as u8;
                    }
                }
            }
        });
        Ok(out)
    }

    /// Screen corners of every visible content node of `n`'s subtree in sample `s` (motion
    /// measure for adaptive slit-scan bands).
    fn sample_corners(&self, n: NodeId, s: &Sample) -> Vec<[f64; 2]> {
        let scene = self.scene;
        let mut out = vec![];
        let mut st = vec![n];
        while let Some(x) = st.pop() {
            let ns = &s.states[x];
            if let (true, Some(b)) = (ns.eff_opacity > 0.0, self.info.bounds(scene, x, s.t)) {
                for (cx, cy) in [(b[0], b[1]), (b[0] + b[2], b[1]), (b[0], b[1] + b[3]), (b[0] + b[2], b[1] + b[3])] {
                    let p = ns.chain.apply(cx, cy);
                    out.push([p.0, p.1]);
                }
            }
            st.extend(scene.nodes[x].children.iter().copied());
        }
        out
    }
}
