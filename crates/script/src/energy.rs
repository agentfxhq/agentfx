//! Motion-energy map: for every video frame, the fraction of the frame area that changes versus
//! the previous frame. Computed from the compiled motion IR (layer layout rects at rest + the
//! evaluated channel tracks), the same information the renderer's damage planner uses, without
//! rendering a single pixel. Area-only mode: every changed paint layer contributes the union of
//! its previous and current screen bounding box; overlapping rects are counted once.
//!
//! What counts as motion: a visible paint-bearing layer whose world transform, world opacity or
//! any own channel value changed, plus layers that animate by themselves (video, particle cloud,
//! fluid) while visible. Film grain and static looks do not count (texture, not motion).
//! Layout-stage channels (w, h, count, wght, ...) move the layer's rest box only as a whole:
//! the energy of a growing box is its full rest rect, a slight overestimate.
//!
//! Consumers: the taste lints (`taste.rs`), `agentfx energy`, the lint config. Layout rects come from
//! the caller (`compute` takes a closure), so tests can feed a synthetic IR.
use crate::ir::*;
use crate::motion::{self, Chans, A2};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct EnergyMap {
    pub fps: f64,
    pub w: u32,
    pub h: u32,
    /// Per frame: fraction (0..1) of the frame area that changed versus the previous frame.
    /// Frame 0 is 0 by definition (nothing before it); cold-open is `W_COLD_OPEN`'s job.
    pub frames: Vec<f32>,
}

/// A half-open time range in seconds.
pub type Run = (f64, f64);

impl EnergyMap {
    pub fn new(fps: f64, w: u32, h: u32, frames: Vec<f32>) -> Self {
        EnergyMap { fps, w, h, frames }
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn duration(&self) -> f64 {
        self.frames.len() as f64 / self.fps
    }
    fn frame_range(&self, t0: f64, t1: f64) -> std::ops::Range<usize> {
        let a = ((t0 * self.fps).floor().max(0.0) as usize).min(self.frames.len());
        let b = ((t1 * self.fps).ceil().max(0.0) as usize).min(self.frames.len());
        a..b.max(a)
    }
    /// Energy of the frame showing time `t`.
    pub fn at(&self, t: f64) -> f32 {
        let i = ((t * self.fps).floor().max(0.0) as usize).min(self.frames.len().saturating_sub(1));
        self.frames.get(i).copied().unwrap_or(0.0)
    }
    /// Largest per-frame energy in [t0, t1).
    pub fn max_in(&self, t0: f64, t1: f64) -> f32 {
        self.frames[self.frame_range(t0, t1)].iter().copied().fold(0.0, f32::max)
    }
    /// Mean per-frame energy in [t0, t1).
    pub fn mean_in(&self, t0: f64, t1: f64) -> f32 {
        let s = &self.frames[self.frame_range(t0, t1)];
        if s.is_empty() { 0.0 } else { s.iter().sum::<f32>() / s.len() as f32 }
    }
    /// Time of the first frame with energy above `eps`, if any.
    pub fn first_motion(&self, eps: f32) -> Option<f64> {
        self.frames.iter().position(|e| *e > eps).map(|i| i as f64 / self.fps)
    }
    /// Maximal runs of frames with energy <= `eps` that last at least `min_len` seconds.
    pub fn still_runs(&self, eps: f32, min_len: f64) -> Vec<Run> {
        let mut out = vec![];
        let mut start: Option<usize> = None;
        for i in 0..=self.frames.len() {
            let still = i < self.frames.len() && self.frames[i] <= eps;
            match (still, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    let (a, b) = (s as f64 / self.fps, i as f64 / self.fps);
                    if b - a >= min_len - 1e-9 {
                        out.push((a, b));
                    }
                    start = None;
                }
                _ => {}
            }
        }
        out
    }
    /// Maximal runs of motion (energy > `eps`); gaps shorter than `max_gap` seconds do not split a run.
    pub fn moving_runs(&self, eps: f32, max_gap: f64) -> Vec<Run> {
        let mut out: Vec<Run> = vec![];
        let mut i = 0;
        while i < self.frames.len() {
            if self.frames[i] > eps {
                let mut j = i;
                while j < self.frames.len() && self.frames[j] > eps {
                    j += 1;
                }
                let (a, b) = (i as f64 / self.fps, j as f64 / self.fps);
                match out.last_mut() {
                    Some(last) if a - last.1 < max_gap - 1e-9 => last.1 = b,
                    _ => out.push((a, b)),
                }
                i = j;
            } else {
                i += 1;
            }
        }
        out
    }
    /// One block character per bin (` ▁▂▃▄▅▆▇█`), each the max energy of its frames on a sqrt scale
    /// (so a 3 % twitch is visible next to a full-frame move).
    pub fn strip(&self, bins: usize) -> String {
        const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let n = self.frames.len();
        (0..bins.max(1))
            .map(|b| {
                let (a, z) = (b * n / bins.max(1), ((b + 1) * n / bins.max(1)).max(b * n / bins.max(1) + 1).min(n));
                let m = self.frames[a.min(n)..z].iter().copied().fold(0.0f32, f32::max);
                if m <= 1e-4 { BLOCKS[0] } else { BLOCKS[1 + ((m.sqrt() * 8.0) as usize).min(7)] }
            })
            .collect()
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"fps": self.fps, "w": self.w, "h": self.h, "frames": self.frames.iter().map(|e| (*e as f64 * 1e5).round() / 1e5).collect::<Vec<_>>()})
    }
}

/// Area of the union of rects `[x, y, w, h]` clipped to `[0, w] x [0, h]` (sweep over x slabs).
pub fn union_area(rects: &[[f64; 4]], w: f64, h: f64) -> f64 {
    let rs: Vec<[f64; 4]> = rects
        .iter()
        .map(|r| [r[0].max(0.0), r[1].max(0.0), (r[0] + r[2]).min(w), (r[1] + r[3]).min(h)])
        .filter(|r| r[2] > r[0] && r[3] > r[1])
        .collect();
    if rs.is_empty() {
        return 0.0;
    }
    let mut xs: Vec<f64> = rs.iter().flat_map(|r| [r[0], r[2]]).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs.dedup();
    let mut total = 0.0;
    for sl in xs.windows(2) {
        let (x0, x1) = (sl[0], sl[1]);
        let mut ys: Vec<(f64, f64)> = rs.iter().filter(|r| r[0] <= x0 && r[2] >= x1).map(|r| (r[1], r[3])).collect();
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (mut covered, mut cur_end) = (0.0, f64::MIN);
        for (y0, y1) in ys {
            let s = y0.max(cur_end);
            if y1 > s {
                covered += y1 - s;
            }
            cur_end = cur_end.max(y1);
        }
        total += covered * (x1 - x0);
    }
    total
}

/// Does this layer put pixels on screen by itself (as opposed to a pure container)?
fn paints(l: &Layer) -> bool {
    let s = &l.style;
    !matches!(l.kind, Kind::Group) || s.fill.is_some() || s.stroke.is_some() || s.border.is_some() || !s.shadows.is_empty() || s.glow_color.is_some()
}

/// Layers that animate by themselves while visible (video, particles, fluid).
fn self_animating(l: &Layer) -> bool {
    matches!(l.kind, Kind::Video { .. } | Kind::Cloud(_) | Kind::Fluid(_))
}

const OP_MIN: f64 = 0.004;

#[derive(Clone)]
struct State {
    m: A2,
    op: f64,
    chans: Vec<f64>,
}

fn differs(a: &State, b: &State) -> bool {
    a.m.iter().zip(&b.m).enumerate().any(|(i, (x, y))| (x - y).abs() > if i < 4 { 1e-4 } else { 1.0 / 64.0 })
        || (a.op - b.op).abs() > 1.0 / 255.0
        || a.chans.len() != b.chans.len()
        || a.chans.iter().zip(&b.chans).any(|(x, y)| (x - y).abs() > 1e-3)
}

fn tv_flat(v: &TV, out: &mut Vec<f64>) {
    match v {
        TV::N(n) => out.push(*n),
        TV::C(c) => out.extend([c.r, c.g, c.b, c.a]),
    }
}

/// Compute the energy map. `rect(id)` gives a layer's layout box at rest in scene coordinates
/// (`Native::rect`), or None for layers without one.
pub fn compute(ir: &Ir, rect: &dyn Fn(&str) -> Option<[f64; 4]>) -> EnergyMap {
    let fps = ir.fps.max(1.0);
    let n = (ir.duration * fps).round().max(0.0) as usize;
    let (fw, fh) = (ir.w as f64, ir.h as f64);
    let ch = Chans::new(ir);
    let magic = HashMap::new();
    let mut layers: Vec<&Layer> = vec![];
    ir.root.walk(&mut |l| layers.push(l));
    let rests: HashMap<&str, [f64; 4]> = layers.iter().filter_map(|l| rect(&l.id).map(|r| (l.id.as_str(), r))).collect();
    // layers whose world state can change at all: self or an ancestor has a non-static track
    let animated_ids: std::collections::HashSet<&str> = ir.tracks.iter().filter(|t| !t.is_static()).map(|t| t.layer.as_str()).collect();
    let mut can_move: std::collections::HashSet<&str> = std::collections::HashSet::new();
    fn mark<'a>(l: &'a Layer, inherited: bool, ids: &std::collections::HashSet<&str>, out: &mut std::collections::HashSet<&'a str>) {
        let m = inherited || ids.contains(l.id.as_str()) || self_animating(l);
        if m {
            out.insert(l.id.as_str());
        }
        for c in &l.children {
            mark(c, m, ids, out);
        }
    }
    mark(&ir.root, false, &animated_ids, &mut can_move);
    let paint: Vec<&Layer> = layers.iter().copied().filter(|l| can_move.contains(l.id.as_str()) && paints(l) && rests.get(l.id.as_str()).is_some_and(|r| r[2] > 0.0 && r[3] > 0.0)).collect();
    let rf = |id: &str| rests.get(id).copied();
    let state_at = |t: f64| -> HashMap<&str, State> {
        let world = motion::world(ir, &ch, &rf, &magic, t);
        paint
            .iter()
            .filter_map(|l| {
                let (m, op) = world.get(&l.id)?;
                let mut c = vec![];
                if let Some(tr) = ch.by.get(l.id.as_str()) {
                    for t_ in tr {
                        tv_flat(&t_.eval(t), &mut c);
                    }
                }
                Some((l.id.as_str(), State { m: *m, op: *op, chans: c }))
            })
            .collect()
    };
    let mut frames = Vec::with_capacity(n);
    let mut prev: Option<HashMap<&str, State>> = None;
    for i in 0..n {
        let cur = state_at(i as f64 / fps);
        let mut dmg: Vec<[f64; 4]> = vec![];
        if let Some(p) = &prev {
            for l in &paint {
                let (Some(a), Some(b)) = (p.get(l.id.as_str()), cur.get(l.id.as_str())) else { continue };
                if a.op < OP_MIN && b.op < OP_MIN {
                    continue;
                }
                let animated = self_animating(l);
                if !animated && !differs(a, b) {
                    continue;
                }
                let r = rests[l.id.as_str()];
                let (ba, bb) = (motion::a2_bbox(&a.m, r), motion::a2_bbox(&b.m, r));
                if a.op >= OP_MIN {
                    dmg.push(ba);
                }
                if b.op >= OP_MIN {
                    dmg.push(bb);
                }
            }
        }
        frames.push(((union_area(&dmg, fw, fh) / (fw * fh).max(1.0)) as f32).clamp(0.0, 1.0));
        prev = Some(cur);
    }
    EnergyMap::new(fps, ir.w, ir.h, frames)
}

/// Energy map of a compiled IR using the native (Blitz) layout for the layer boxes.
pub fn compute_native(ir: &Ir, nat: &crate::native::Native) -> EnergyMap {
    compute(ir, &|id| nat.rect(id))
}
