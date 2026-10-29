//! World canvas (X9): a node with `world` draws its static descendants from a tiled mip
//! pyramid while its chain (the camera) moves, and as exact vectors when it rests.
//!
//! - World space = the world node's content space (its chain maps it to the screen).
//! - Level L holds the static world rastered at scale 2^L device px per world px, cut into
//!   `tile` x `tile` tiles. Tiles are rastered on demand (vectors, exact), only where static
//!   content is, and prefetched in parallel along the planned camera path (the schedule
//!   knows the chain of every future frame). An LRU keeps the cache under `cache_mb`.
//! - A moving frame picks the finer level L = ceil(log2 s) (s = camera scale, so 1..2 texels
//!   per screen px) and resamples it with a 2x2 box of bilinear taps (a supersampled
//!   downscale: crisp text, no aliasing across level switches).
//! - Animated descendants (tracks, switches, effects, clouds, ...) are drawn live on top.
use super::*;
use rayon::prelude::*;
use std::collections::HashMap;

/// Static/live split of one world subtree (computed once per painter).
pub(super) struct WorldInfo {
    /// node is part of the world's static raster (descendants only)
    stat: Vec<bool>,
    /// world space <- node content space (static nodes)
    rel: Vec<Affine>,
    /// world-space bounds of the node's static subtree ([0; 4] = nothing)
    sub: Vec<[f64; 4]>,
    /// roots of animated subtrees, paint order
    live: Vec<NodeId>,
    /// the world node's own content is static (drawn into the tiles)
    own: bool,
    /// world-space bounds of everything static
    all: [f64; 4],
    tile: u32,
    cap: usize,
}

#[derive(Default)]
pub(super) struct TileCache {
    map: HashMap<(NodeId, i32, i64, i64), (Option<Arc<Pixmap>>, u64)>,
    bytes: usize,
    stamp: u64,
}

#[derive(Default, Debug, Clone, Copy, serde::Serialize)]
pub struct WorldStats {
    /// frames composited from tiles / drawn as vectors (rest)
    pub tile_frames: usize,
    pub vector_frames: usize,
    pub tiles_rastered: usize,
    pub tiles_prefetched: usize,
    pub tiles_empty: usize,
    pub tile_hits: usize,
    pub evicted: usize,
    pub peak_mb: f64,
    pub raster_s: f64,
    pub sample_s: f64,
    pub min_level: i32,
    pub max_level: i32,
    /// largest / smallest camera scale seen in tile frames
    pub max_scale: f64,
    pub min_scale: f64,
}

/// Frames ahead whose tiles are prefetched.
const PREFETCH: usize = 12;
/// Upper bound of tiles one frame may touch (else: vectors).
const MAX_TILES: usize = 6000;

fn kaff(a: &agentfx_ir::Affine) -> Affine {
    Affine::new([a.a, a.b, a.c, a.d, a.e, a.f])
}

/// Camera scale of a world->screen matrix (geometric mean of the axes).
fn scale_of(m: Affine) -> f64 {
    m.determinant().abs().sqrt()
}

/// Mip level for a camera scale: the finer one (1..2 texels per screen px).
pub fn level_for(s: f64) -> i32 {
    (s.log2() - 1e-9).ceil().clamp(-30.0, 60.0) as i32
}

/// Tiles (level px grid) covering screen rect `r` under world->screen `m` at `level`.
fn tile_range(m: Affine, level: i32, r: [f64; 4], tile: u32, all: [f64; 4]) -> Option<(i64, i64, i64, i64)> {
    let s = 2f64.powi(level);
    let inv = Affine::scale(s) * m.inverse();
    let pts = [(r[0], r[1]), (r[0] + r[2], r[1]), (r[0], r[1] + r[3]), (r[0] + r[2], r[1] + r[3])];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in pts {
        let p = inv * kurbo::Point::new(x, y);
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    // texels the bilinear box taps reach + the static content's extent at this level
    let (x0, y0, x1, y1) = ((x0 - 2.0).max(all[0] * s - 1.0), (y0 - 2.0).max(all[1] * s - 1.0), (x1 + 2.0).min((all[0] + all[2]) * s + 1.0), (y1 + 2.0).min((all[1] + all[3]) * s + 1.0));
    if !(x1 > x0 && y1 > y0) || !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return None;
    }
    let t = tile as f64;
    Some(((x0 / t).floor() as i64, (y0 / t).floor() as i64, (x1 / t).floor() as i64, (y1 / t).floor() as i64))
}

impl<'a> Painter<'a> {
    pub(super) fn world_mode() -> u8 {
        match agentfx_ir::env::var("AGENTFX_WORLD").as_deref() {
            Ok("0") | Ok("off") => 0,
            Ok("tiles") => 2,
            _ => 1,
        }
    }

    /// Does frame `f` composite world `w` from tiles? (its chain moves into the next frame)
    pub(super) fn world_tiles_at(&self, w: NodeId, f: usize) -> bool {
        match self.world_mode {
            0 => false,
            2 => true,
            _ => {
                let s = &self.sched.states;
                f + 1 < s.len() && s[f][w].chain != s[f + 1][w].chain
            }
        }
    }

    pub(super) fn world_info(&mut self, w: NodeId) -> Arc<WorldInfo> {
        if let Some(i) = self.world_infos.get(&w) {
            return i.clone();
        }
        let scene = self.scene;
        let sched = self.sched;
        let nn = scene.nodes.len();
        let nf = sched.frames;
        let cfg = scene.nodes[w].world.unwrap_or_default();
        let content_still = |x: NodeId| (0..nf).all(|f| sched.content[f][x] == sched.content[0][x] && sched.sig[f][x] == sched.sig[0][x]);
        let still = |x: NodeId| content_still(x) && (0..nf).all(|f| sched.states[f][x].local == sched.states[0][x].local && sched.states[f][x].opacity == sched.states[0][x].opacity);
        let simple = |x: NodeId| {
            let nd = &scene.nodes[x];
            !nd.hidden && nd.effects.is_empty() && nd.motion_blur.is_none() && nd.time.is_none() && nd.blend == agentfx_ir::BlendMode::Normal && !matches!(nd.content, Some(Content::Cloud(_))) && !self.dynamic_subtree[x]
        };
        let mut wi = WorldInfo { stat: vec![false; nn], rel: vec![Affine::IDENTITY; nn], sub: vec![[0.0; 4]; nn], live: vec![], own: false, all: [0.0; 4], tile: cfg.tile, cap: cfg.cache_mb as usize * (1 << 20) };
        wi.own = scene.nodes[w].content.is_some() && content_still(w) && !matches!(scene.nodes[w].content, Some(Content::Cloud(_)));
        fn visit(p: &Painter, wi: &mut WorldInfo, x: NodeId, simple: &dyn Fn(NodeId) -> bool, still: &dyn Fn(NodeId) -> bool) {
            for &c in &p.scene.nodes[x].children {
                if simple(c) && still(c) {
                    wi.stat[c] = true;
                    wi.rel[c] = wi.rel[x] * kaff(&p.sched.states[0][c].local);
                    visit(p, wi, c, simple, still);
                    let mut b = [0.0; 4];
                    if p.scene.nodes[c].content.is_some() {
                        if let Some(cb) = p.info.bounds(p.scene, c, 0.0) {
                            b = transform_bbox(&ra(wi.rel[c]), cb);
                        }
                    }
                    for &g in &p.scene.nodes[c].children {
                        if wi.stat[g] {
                            b = union(b, wi.sub[g]);
                        }
                    }
                    wi.sub[c] = b;
                } else {
                    wi.live.push(c);
                }
            }
        }
        visit(self, &mut wi, w, &simple, &still);
        let mut all = [0.0; 4];
        if wi.own {
            if let Some(cb) = self.info.bounds(scene, w, 0.0) {
                all = cb;
            }
        }
        for &c in &scene.nodes[w].children {
            if wi.stat[c] {
                all = union(all, wi.sub[c]);
            }
        }
        // shadows / AA margin
        wi.all = if all[2] > 0.0 { schedule::inflate(all, 2.0) } else { all };
        let wi = Arc::new(wi);
        self.world_infos.insert(w, wi.clone());
        wi
    }

    /// Draw the static world into a `tile` x `tile` context at `level`, tile (tx, ty).
    fn raster_tile(&mut self, view: &View, w: NodeId, wi: &WorldInfo, level: i32, tx: i64, ty: i64) -> Result<Option<Pixmap>> {
        let t = wi.tile as f64;
        let s = 2f64.powi(level);
        let wr = [tx as f64 * t / s, ty as f64 * t / s, t / s, t / s];
        if intersect(wr, wi.all)[2] <= 0.0 {
            return Ok(None);
        }
        let m = Affine::translate((-(tx as f64) * t, -(ty as f64) * t)) * Affine::scale(s);
        let mut ctx = self.take_ctx(wi.tile as u16, wi.tile as u16, 0);
        let region = [0.0, 0.0, t, t];
        self.iso += 1;
        let saved = std::mem::replace(&mut self.origin, (tx * wi.tile as i64, ty * wi.tile as i64));
        let mut drew = false;
        let mut res = Ok(());
        if wi.own {
            if let Some(cb) = self.info.bounds(self.scene, w, view.t) {
                if intersect(cb, wr)[2] > 0.0 {
                    drew = true;
                    res = self.draw_content(&mut ctx, view, w, m, 1.0, region);
                }
            }
        }
        if res.is_ok() {
            for &c in &self.scene.nodes[w].children {
                if wi.stat[c] && intersect(wi.sub[c], wr)[2] > 0.0 {
                    drew = true;
                    res = self.draw_static(&mut ctx, view, wi, c, m, wr, region, 1.0);
                    if res.is_err() {
                        break;
                    }
                }
            }
        }
        self.origin = saved;
        self.iso -= 1;
        res?;
        if !drew {
            self.give_ctx(ctx);
            return Ok(None);
        }
        ctx.flush();
        let mut px = Pixmap::new(wi.tile as u16, wi.tile as u16);
        ctx.render(&mut px, &mut self.res);
        self.give_ctx(ctx);
        Ok(Some(px))
    }

    /// A static node into a tile (`m`: world -> tile px; `wr`: the tile in world space):
    /// clips, opacity, content, static children; mirrors `draw_node_vector`.
    #[allow(clippy::too_many_arguments)]
    fn draw_static(&mut self, ctx: &mut RenderContext, view: &View, wi: &WorldInfo, n: NodeId, m: Affine, wr: [f64; 4], region: [f64; 4], alpha: f64) -> Result<()> {
        let node = &self.scene.nodes[n];
        let st = view.states[n];
        if st.opacity <= 0.0 {
            return Ok(());
        }
        let parent = self.sched.parents[n].unwrap_or(0);
        let prel = wi.rel[parent];
        let mut clips = 0;
        for c in &node.clips {
            let rr = c.rect;
            let p = kurbo::RoundedRect::from_rect(Rect::new(rr[0], rr[1], rr[0] + rr[2], rr[1] + rr[3]), kurbo::RoundedRectRadii::new(c.radius[0], c.radius[1], c.radius[2], c.radius[3])).to_path(0.1);
            ctx.set_transform(m * prel);
            ctx.set_fill_rule(Fill::NonZero);
            ctx.push_clip_path(&p);
            clips += 1;
        }
        let op = st.opacity * alpha;
        let op = if op >= OPAQUE { 1.0 } else { op };
        let (group_alpha, child_alpha) = if node.isolate { (op, 1.0) } else { (1.0, op) };
        let layer = group_alpha < 1.0;
        if layer {
            ctx.set_transform(Affine::IDENTITY);
            ctx.push_layer(None, None, Some(group_alpha as f32), None, None);
        }
        let mut res = Ok(());
        if node.content.is_some() {
            if let Some(cb) = self.info.bounds(self.scene, n, view.t) {
                if intersect(transform_bbox(&ra(wi.rel[n]), cb), wr)[2] > 0.0 {
                    res = self.draw_content(ctx, view, n, m * wi.rel[n], child_alpha, region);
                }
            }
        }
        if res.is_ok() {
            for &c in &node.children {
                if wi.stat[c] && intersect(wi.sub[c], wr)[2] > 0.0 {
                    res = self.draw_static(ctx, view, wi, c, m, wr, region, child_alpha);
                    if res.is_err() {
                        break;
                    }
                }
            }
        }
        if layer {
            ctx.pop_layer();
        }
        for _ in 0..clips {
            ctx.pop_clip();
        }
        res
    }

    /// Tiles a frame of world `w` needs (screen rect `r`), or None when the view is degenerate.
    fn frame_tiles(&self, wi: &WorldInfo, w: NodeId, f: usize, r: [f64; 4]) -> Option<(i32, (i64, i64, i64, i64))> {
        let m = kaff(&self.sched.states[f][w].chain);
        let s = scale_of(m);
        if !(s.is_finite() && s > 1e-12) {
            return None;
        }
        let level = level_for(s);
        let tr = tile_range(m, level, r, wi.tile, wi.all)?;
        let n = (tr.2 - tr.0 + 1) as i128 * (tr.3 - tr.1 + 1) as i128;
        (n as usize <= MAX_TILES).then_some((level, tr))
    }

    /// Raster the missing tiles of frames f .. f + PREFETCH (in parallel: one painter per
    /// worker), so the frames along the planned camera path find them cached.
    fn prefetch(&mut self, view: &View, w: NodeId, wi: &Arc<WorldInfo>, f: usize) -> Result<()> {
        let screen = [0.0, 0.0, self.scene.width as f64, self.scene.height as f64];
        let mut want: Vec<(i32, i64, i64)> = vec![];
        let mut seen = std::collections::HashSet::new();
        let end = (f + PREFETCH).min(self.sched.frames);
        let budget = wi.cap / 2 / (wi.tile as usize * wi.tile as usize * 4).max(1);
        for g in f..end {
            if g > f && !self.world_tiles_at(w, g) {
                break;
            }
            let Some((level, tr)) = self.frame_tiles(wi, w, g, screen) else { continue };
            for ty in tr.1..=tr.3 {
                for tx in tr.0..=tr.2 {
                    let k = (level, tx, ty);
                    if !self.tiles.map.contains_key(&(w, level, tx, ty)) && seen.insert(k) {
                        want.push(k);
                    }
                }
            }
            if want.len() > budget {
                break;
            }
        }
        if want.len() < 8 {
            return Ok(());
        }
        let tb = std::time::Instant::now();
        let (scene, prep, sched, info) = (self.scene, self.prep, self.sched, self.info);
        let nthreads = rayon::current_num_threads().max(1);
        let chunk = want.len().div_ceil(nthreads).max(4);
        let wi2 = wi.clone();
        let out: Vec<Result<Vec<((i32, i64, i64), Option<Pixmap>)>>> = want
            .par_chunks(chunk)
            .map(|ks| {
                let mut p = Painter::new(scene, prep, sched, info, 0);
                p.world_mode = 1;
                let v = View { t: view.t, states: view.states, content: view.content };
                ks.iter().map(|&(l, x, y)| Ok(((l, x, y), p.raster_tile(&v, w, &wi2, l, x, y)?))).collect()
            })
            .collect();
        for r in out {
            for ((l, x, y), px) in r? {
                self.world_stats.tiles_prefetched += 1;
                self.insert_tile(w, l, x, y, px);
            }
        }
        self.world_stats.raster_s += tb.elapsed().as_secs_f64();
        Ok(())
    }

    fn insert_tile(&mut self, w: NodeId, l: i32, x: i64, y: i64, px: Option<Pixmap>) {
        match &px {
            Some(p) => {
                self.world_stats.tiles_rastered += 1;
                self.tiles.bytes += p.width() as usize * p.height() as usize * 4;
            }
            None => self.world_stats.tiles_empty += 1,
        }
        self.tiles.map.insert((w, l, x, y), (px.map(Arc::new), self.tiles.stamp));
        self.world_stats.peak_mb = self.world_stats.peak_mb.max(self.tiles.bytes as f64 / (1u64 << 20) as f64);
    }

    /// LRU: drop the least recently used tiles (never the current frame's) above the budget.
    fn evict(&mut self, cap: usize) {
        if self.tiles.bytes <= cap {
            return;
        }
        let mut v: Vec<((NodeId, i32, i64, i64), u64, usize)> = self.tiles.map.iter().filter(|(_, e)| e.1 < self.tiles.stamp).map(|(k, e)| (*k, e.1, e.0.as_ref().map_or(0, |p| p.width() as usize * p.height() as usize * 4))).collect();
        v.sort_by_key(|e| (e.1, e.0));
        let target = cap * 3 / 4;
        for (k, _, b) in v {
            if self.tiles.bytes <= target {
                break;
            }
            self.tiles.map.remove(&k);
            self.tiles.bytes -= b;
            self.world_stats.evicted += 1;
        }
    }

    /// The static world of `w` at frame `f`, resampled from tiles, into `region` (screen).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_world_tiles(&mut self, ctx: &mut RenderContext, view: &View, f: usize, w: NodeId, base: Affine, region: [f64; 4], alpha: f64, matte: bool) -> Result<bool> {
        let _p = crate::prof::scope("world.tiles");
        let wi = self.world_info(w);
        let m = kaff(&view.states[w].chain);
        let s = scale_of(m);
        if !(s.is_finite() && s > 1e-12) {
            return Ok(false);
        }
        let level = level_for(s);
        let Some(tr) = tile_range(m, level, region, wi.tile, wi.all) else {
            // nothing static in view
            return self.draw_world_live(ctx, view, f, &wi, base, region, alpha, matte).map(|_| true);
        };
        if (tr.2 - tr.0 + 1) as i128 * (tr.3 - tr.1 + 1) as i128 > MAX_TILES as i128 {
            return Ok(false);
        }
        self.tiles.stamp += 1;
        self.world_stats.tile_frames += 1;
        self.world_stats.min_level = if self.world_stats.tile_frames == 1 { level } else { self.world_stats.min_level.min(level) };
        self.world_stats.max_level = self.world_stats.max_level.max(level);
        self.world_stats.max_scale = self.world_stats.max_scale.max(s);
        self.world_stats.min_scale = if self.world_stats.min_scale == 0.0 { s } else { self.world_stats.min_scale.min(s) };
        self.prefetch(view, w, &wi, f)?;
        let (nx, ny) = ((tr.2 - tr.0 + 1) as usize, (tr.3 - tr.1 + 1) as usize);
        let mut grid: Vec<Option<Arc<Pixmap>>> = Vec::with_capacity(nx * ny);
        let stamp = self.tiles.stamp;
        for ty in tr.1..=tr.3 {
            for tx in tr.0..=tr.2 {
                let key = (w, level, tx, ty);
                if let Some(e) = self.tiles.map.get_mut(&key) {
                    e.1 = stamp;
                    self.world_stats.tile_hits += 1;
                    grid.push(e.0.clone());
                    continue;
                }
                let tb = std::time::Instant::now();
                let px = self.raster_tile(view, w, &wi, level, tx, ty)?;
                self.world_stats.raster_s += tb.elapsed().as_secs_f64();
                self.insert_tile(w, level, tx, ty, px);
                grid.push(self.tiles.map[&key].0.clone());
            }
        }
        self.evict(wi.cap);
        let ts = std::time::Instant::now();
        let r = [region[0].floor() as i64, region[1].floor() as i64, region[2].ceil() as i64, region[3].ceil() as i64];
        if r[2] <= 0 || r[3] <= 0 || r[2] > u16::MAX as i64 || r[3] > u16::MAX as i64 {
            return Ok(true);
        }
        let mut out = Pixmap::new(r[2] as u16, r[3] as u16);
        let inv = (Affine::scale(2f64.powi(level)) * m.inverse()).as_coeffs();
        sample(out.data_as_u8_slice_mut(), r[2] as usize, (r[0], r[1]), inv, &grid, (tr.0, tr.1, nx, ny), wi.tile);
        self.world_stats.sample_s += ts.elapsed().as_secs_f64();
        ctx.set_transform(base);
        draw_pixmap_alpha(ctx, Arc::new(out), r[0] as f64, r[1] as f64, alpha as f32);
        self.draw_world_live(ctx, view, f, &wi, base, region, alpha, matte)?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_world_live(&mut self, ctx: &mut RenderContext, view: &View, f: usize, wi: &WorldInfo, base: Affine, region: [f64; 4], alpha: f64, matte: bool) -> Result<()> {
        for &c in &wi.live {
            match self.sub_ink(false, f, c) {
                Some(ink) if intersect(ink, region)[2] > 0.0 || self.scene.nodes[c].motion_blur.is_some() => {}
                _ => continue,
            }
            self.draw_node(ctx, view, f, c, base, region, alpha, matte)?;
        }
        Ok(())
    }
}

/// Resample tiles into `out` (premultiplied RGBA8, `w` px wide; pixel (0, 0) = screen
/// `org`): per pixel a 2x2 box of bilinear taps at +-1/4 px, mapped through `inv`
/// (screen -> level px). Rows in parallel; pure function of its inputs.
fn sample(out: &mut [u8], w: usize, org: (i64, i64), inv: [f64; 6], grid: &[Option<Arc<Pixmap>>], g: (i64, i64, usize, usize), tile: u32) {
    if inv[1] == 0.0 && inv[2] == 0.0 && w > 0 {
        return sample_axis(out, w, org, inv, grid, g, tile);
    }
    sample_any(out, w, org, inv, grid, g, tile)
}

/// Taps of one axis: per output pixel the 2 box taps (+-1/4 px) as 4 (texel, weight) pairs.
fn axis_taps(n: usize, org: i64, scale: f64, off: f64) -> Vec<[(i64, f32); 4]> {
    (0..n)
        .map(|p| {
            let s = (org + p as i64) as f64 + 0.5;
            let mut t = [(0i64, 0f32); 4];
            for (k, o) in [-0.25, 0.25].into_iter().enumerate() {
                let l = scale * (s + o) + off - 0.5;
                let i = l.floor();
                let f = (l - i) as f32;
                t[2 * k] = (i as i64, 0.5 * (1.0 - f));
                t[2 * k + 1] = (i as i64 + 1, 0.5 * f);
            }
            t
        })
        .collect()
}

/// Axis-aligned cameras (no rotation): the same 2x2 box of bilinear taps, separated into a
/// horizontal pass over the needed level rows and a vertical pass (rows in parallel).
fn sample_axis(out: &mut [u8], w: usize, org: (i64, i64), inv: [f64; 6], grid: &[Option<Arc<Pixmap>>], g: (i64, i64, usize, usize), tile: u32) {
    let h = out.len() / (w * 4);
    let sh = tile.trailing_zeros();
    let ts = tile as i64;
    let (gx0, gy0, nx, ny) = g;
    let xt = axis_taps(w, org.0, inv[0], inv[4]);
    let yt = axis_taps(h, org.1, inv[3], inv[5]);
    let x0 = xt.iter().flat_map(|t| t.iter().map(|e| e.0)).min().unwrap_or(0);
    let x1 = xt.iter().flat_map(|t| t.iter().map(|e| e.0)).max().unwrap_or(0);
    let y0 = yt.iter().flat_map(|t| t.iter().map(|e| e.0)).min().unwrap_or(0);
    let y1 = yt.iter().flat_map(|t| t.iter().map(|e| e.0)).max().unwrap_or(0);
    let rw = (x1 - x0 + 1) as usize;
    // horizontal pass: one f32 RGBA row (output width) per level row
    let hrows: Vec<Vec<f32>> = (y0..=y1)
        .into_par_iter()
        .map(|y| {
            let mut row = vec![0u8; rw * 4];
            let ty = (y >> sh) - gy0;
            if ty >= 0 && (ty as usize) < ny {
                let iy = (y & (ts - 1)) as usize;
                let mut x = x0;
                while x <= x1 {
                    let tx = (x >> sh) - gx0;
                    let end = (((x >> sh) + 1) << sh).min(x1 + 1);
                    if tx >= 0 && (tx as usize) < nx {
                        if let Some(p) = &grid[ty as usize * nx + tx as usize] {
                            let ix = (x & (ts - 1)) as usize;
                            let n = (end - x) as usize;
                            let src = &p.data_as_u8_slice()[(iy * tile as usize + ix) * 4..(iy * tile as usize + ix + n) * 4];
                            let d = ((x - x0) as usize) * 4;
                            row[d..d + n * 4].copy_from_slice(src);
                        }
                    }
                    x = end;
                }
            }
            let mut o = vec![0f32; w * 4];
            for (px, t) in xt.iter().enumerate() {
                let mut acc = [0f32; 4];
                for &(i, wt) in t {
                    let j = (i - x0) as usize * 4;
                    for k in 0..4 {
                        acc[k] += wt * row[j + k] as f32;
                    }
                }
                o[px * 4..px * 4 + 4].copy_from_slice(&acc);
            }
            o
        })
        .collect();
    out.par_chunks_mut(w * 4).enumerate().for_each(|(py, orow)| {
        let t = &yt[py];
        for px in 0..w {
            let mut acc = [0f32; 4];
            for &(i, wt) in t {
                let r = &hrows[(i - y0) as usize];
                for k in 0..4 {
                    acc[k] += wt * r[px * 4 + k];
                }
            }
            for k in 0..4 {
                orow[px * 4 + k] = (acc[k] + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    });
}

fn sample_any(out: &mut [u8], w: usize, org: (i64, i64), inv: [f64; 6], grid: &[Option<Arc<Pixmap>>], g: (i64, i64, usize, usize), tile: u32) {
    let sh = tile.trailing_zeros();
    let mask = tile as i64 - 1;
    let (gx0, gy0, nx, ny) = g;
    let fetch = |x: i64, y: i64| -> [f32; 4] {
        let (tx, ty) = ((x >> sh) - gx0, (y >> sh) - gy0);
        if tx < 0 || ty < 0 || tx as usize >= nx || ty as usize >= ny {
            return [0.0; 4];
        }
        match &grid[ty as usize * nx + tx as usize] {
            Some(p) => {
                let i = (((y & mask) << sh) + (x & mask)) as usize * 4;
                let d = &p.data_as_u8_slice()[i..i + 4];
                [d[0] as f32, d[1] as f32, d[2] as f32, d[3] as f32]
            }
            None => [0.0; 4],
        }
    };
    let [a, b, c, d, e, f] = inv;
    let taps: [(f64, f64); 4] = [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)];
    out.par_chunks_mut(w * 4).enumerate().for_each(|(py, row)| {
        let sy = (org.1 + py as i64) as f64 + 0.5;
        for px in 0..w {
            let sx = (org.0 + px as i64) as f64 + 0.5;
            let mut acc = [0f32; 4];
            for (ox, oy) in taps {
                let (x, y) = (sx + ox, sy + oy);
                // texel centers at +0.5
                let lx = a * x + c * y + e - 0.5;
                let ly = b * x + d * y + f - 0.5;
                let (x0, y0) = (lx.floor(), ly.floor());
                let (fx, fy) = ((lx - x0) as f32, (ly - y0) as f32);
                let (xi, yi) = (x0 as i64, y0 as i64);
                let p00 = fetch(xi, yi);
                let p10 = fetch(xi + 1, yi);
                let p01 = fetch(xi, yi + 1);
                let p11 = fetch(xi + 1, yi + 1);
                for k in 0..4 {
                    let top = p00[k] + (p10[k] - p00[k]) * fx;
                    let bot = p01[k] + (p11[k] - p01[k]) * fx;
                    acc[k] += top + (bot - top) * fy;
                }
            }
            let o = &mut row[px * 4..px * 4 + 4];
            for k in 0..4 {
                o[k] = (acc[k] * 0.25 + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_is_the_finer_octave() {
        assert_eq!(level_for(1.0), 0);
        assert_eq!(level_for(1.5), 1);
        assert_eq!(level_for(2.0), 1);
        assert_eq!(level_for(0.3), -1);
        assert_eq!(level_for(1e4), 14);
        for s in [0.013, 0.7, 1.0, 3.3, 77.0, 12345.0] {
            let rho = 2f64.powi(level_for(s)) / s;
            assert!((1.0 - 1e-9..2.0 + 1e-9).contains(&rho), "s {s} rho {rho}");
        }
    }

    #[test]
    fn sampling_identity_level_reproduces_the_tile() {
        // one 64 px tile, identity mapping: the 2x2 box at +-1/4 px of a constant image is exact
        let mut p = Pixmap::new(64, 64);
        for (i, px) in p.data_as_u8_slice_mut().chunks_mut(4).enumerate() {
            let v = if (i % 64) < 32 { 200 } else { 40 };
            px.copy_from_slice(&[v, v, v, 255]);
        }
        let grid = vec![Some(Arc::new(p))];
        let mut out = vec![0u8; 16 * 16 * 4];
        sample(&mut out, 16, (0, 0), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &grid, (0, 0, 1, 1), 64);
        let i = (5 * 16 + 5) * 4;
        assert_eq!(&out[i..i + 4], &[200, 200, 200, 255]);
        // outside the grid: transparent
        let mut out2 = vec![9u8; 4 * 4 * 4];
        sample(&mut out2, 4, (-100, -100), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &grid, (0, 0, 1, 1), 64);
        assert!(out2.iter().all(|&v| v == 0));
    }

    #[test]
    fn separable_path_matches_the_general_one() {
        let mut tiles = vec![];
        for t in 0..4u32 {
            let mut p = Pixmap::new(64, 64);
            for (i, px) in p.data_as_u8_slice_mut().chunks_mut(4).enumerate() {
                let v = ((i as u32 * 37 + t * 91) % 251) as u8;
                px.copy_from_slice(&[v / 2, v / 3, v / 4, v]);
            }
            tiles.push(Some(Arc::new(p)));
        }
        tiles[3] = None;
        let inv = [1.37, 0.0, 0.0, 1.37, 3.3, -5.1];
        let (mut a, mut b) = (vec![0u8; 50 * 40 * 4], vec![0u8; 50 * 40 * 4]);
        sample_axis(&mut a, 50, (10, 20), inv, &tiles, (0, 0, 2, 2), 64);
        sample_any(&mut b, 50, (10, 20), inv, &tiles, (0, 0, 2, 2), 64);
        assert!(a.iter().zip(&b).all(|(x, y)| (*x as i32 - *y as i32).abs() <= 1));
    }

    fn world_scene(cache_mb: u32) -> Scene {
        let font = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/scenes/_shared/Inter.ttf");
        let mut nodes = vec![agentfx_ir::Node::new("root"), agentfx_ir::Node::new("camera"), agentfx_ir::Node::new("world")];
        nodes[0].children = vec![1];
        nodes[1].children = vec![2];
        nodes[1].origin = [101.3, 62.7];
        nodes[2].world = Some(agentfx_ir::World { tile: 64, cache_mb });
        nodes[2].content = Some(Content::Rect { rect: agentfx_ir::RRect { rect: [0.0, 0.0, 320.0, 180.0], radius: [0.0; 4] }, color: [0.95, 0.93, 0.9, 1.0] });
        let mut kids = vec![];
        for (k, (txt, x, y, sz)) in [("Hello world", 20.0, 30.0, 34.0), ("tiny print inside", 96.0, 58.0, 3.0), ("o", 200.0, 100.0, 60.0)].iter().enumerate() {
            let mut t = agentfx_ir::Node::new(&format!("t{k}"));
            t.content = Some(serde_json::from_value(serde_json::json!({"type": "text", "text": txt, "font": 0, "size": sz, "weight": 700, "pos": [x, y], "brush": {"solid": [0.1, 0.1, 0.12, 1.0]}})).unwrap());
            kids.push(nodes.len());
            nodes.push(t);
        }
        // a scaled sub-board (powers of ten) and a live (animated) child
        let mut sub = agentfx_ir::Node::new("sub");
        sub.origin = [100.0, 60.0];
        sub.transform = vec![agentfx_ir::TransformFn::Scale(0.05, 0.05)];
        let mut st = agentfx_ir::Node::new("subtext");
        st.content = Some(serde_json::from_value(serde_json::json!({"type": "text", "text": "deep", "font": 0, "size": 40, "weight": 900, "pos": [60, 40], "brush": {"solid": [0.9, 0.3, 0.1, 1.0]}})).unwrap());
        let si = nodes.len();
        nodes.push(sub);
        nodes.push(st);
        nodes[si].children = vec![si + 1];
        kids.push(si);
        let mut live = agentfx_ir::Node::new("live");
        live.content = Some(Content::Rect { rect: agentfx_ir::RRect { rect: [150.0, 20.0, 40.0, 20.0], radius: [4.0; 4] }, color: [0.2, 0.4, 1.0, 1.0] });
        kids.push(nodes.len());
        nodes.push(live);
        nodes[2].children = kids;
        let tracks: Vec<agentfx_ir::Track> = serde_json::from_value(serde_json::json!([
            {"node": 1, "property": "transform", "delay": 0, "duration": 1, "fill": "both", "easing": "linear",
             "keyframes": [{"offset": 0, "value": {"transform": [{"scale": [1, 1]}]}}, {"offset": 1, "value": {"transform": [{"scale": [60, 60]}]}}]},
            {"node": 8, "property": "opacity", "delay": 0, "duration": 1, "fill": "both", "easing": "linear",
             "keyframes": [{"offset": 0, "value": {"number": 1}}, {"offset": 1, "value": {"number": 0.2}}]}
        ])).unwrap();
        Scene { version: agentfx_ir::IR_VERSION, width: 320, height: 180, fps: 30, frames: 40, nodes, tracks, assets: vec![], fonts: vec![agentfx_ir::FontAsset { path: font.into(), index: 0 }], camera: None, text_aa: Default::default() }
    }

    fn render(scene: &Scene, mode: u8, frames: &[usize]) -> Vec<Vec<u8>> {
        let prep = Prepared::new(scene, std::path::Path::new("."), None, None).unwrap();
        let info = Info::new(scene, &prep);
        let sched = schedule::build(scene, &info).unwrap();
        let mut p = Painter::new(scene, &prep, &sched, &info, 0);
        p.world_mode = mode;
        let out = frames.iter().map(|&f| p.render_rect(f, [0, 0, 320, 180]).unwrap().data_as_u8_slice().to_vec()).collect();
        if mode == 2 {
            assert!(p.world_stats.tile_frames == frames.len() && p.world_stats.tiles_rastered > 0, "{:?}", p.world_stats);
        }
        out
    }

    #[test]
    fn world_tiles_match_vectors_and_are_deterministic() {
        let s = world_scene(512);
        assert_eq!(s.nodes[8].name, "live");
        let frames = [0usize, 5, 11, 17, 23, 29, 30];
        let tiles = render(&s, 2, &frames);
        let vecs = render(&s, 0, &frames);
        for (k, (a, b)) in tiles.iter().zip(&vecs).enumerate() {
            let mean = a.iter().zip(b).map(|(x, y)| (*x as i32 - *y as i32).abs() as f64).sum::<f64>() / a.len() as f64;
            assert!(mean < 3.0, "frame {} mean abs diff {mean}", frames[k]);
        }
        // order, eviction (tiny cache) and fresh painters never change pixels
        let mut rev: Vec<usize> = frames.to_vec();
        rev.reverse();
        let back = render(&s, 2, &rev);
        for (k, f) in rev.iter().enumerate() {
            let i = frames.iter().position(|x| x == f).unwrap();
            assert!(back[k] == tiles[i], "frame {f} differs by render order");
        }
        let small = render(&world_scene(16), 2, &frames);
        assert!(small == tiles, "eviction changed pixels");
        // the default mode composites from tiles only while the camera moves
        let auto = render(&s, 1, &[39]);
        let vec39 = render(&s, 0, &[39]);
        assert!(auto[0] == vec39[0], "rest frames are exact vectors");
    }
}
