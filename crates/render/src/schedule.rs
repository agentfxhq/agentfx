//! Core of reel: IR scene -> per-frame committed node states, content selection,
//! parameter signatures, ink rects and damage (dirty rects). No pixels are looked at
//! (principle 4).
//!
//! Change quantization (from the e1 finding that 18 % of "changed" Remotion frames
//! change no pixels): a node's animated transform/opacity is only *committed* when it
//! differs from the last committed value by at least POS_EPS_PX of on-screen movement
//! (max corner displacement of the node's subtree bounds) or OPACITY_EPS. Frames
//! draw committed values, so damage and pixels always agree.
//!
//! Everything else that changes pixels (content switches, path trim, per-glyph tracks,
//! effect parameters of the node or an ancestor, matte sources, motion-blur sub-frame
//! samples) enters a per-node signature; damage is the evaluated ink rect (content
//! bounds through the chain, grown by effect margins, swept over motion-blur samples,
//! clipped by ancestor clips) before and after the change.
use anyhow::{Result, bail};
use agentfx_ir::eval::{eval_track, transform_matrix};
use agentfx_ir::{Affine, AssetId, Content, Effect, NodeId, Sampled, Scene};
use std::hash::{Hash, Hasher};

/// Movement below this (screen px) is not damage.
pub const POS_EPS_PX: f64 = 1.0 / 16.0;
/// Opacity change below this is not damage (8-bit output).
pub const OPACITY_EPS: f64 = 1.0 / 255.0;
/// Blur kernels are cut at this many standard deviations (ink margin).
pub const BLUR_EXTENT: f64 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeState {
    /// Committed local matrix (origin applied).
    pub local: Affine,
    /// Scene -> frame transform of this node's content.
    pub chain: Affine,
    pub opacity: f64,
    pub eff_opacity: f64,
}

/// What the schedule needs to know about content beyond the IR (shaped text, parsed
/// paths). `bounds` = content ink in scene coordinates at time t (None = nothing drawn);
/// `sig` = hash of the content's own animated parameters at t.
pub trait ContentInfo {
    fn bounds(&self, scene: &Scene, n: NodeId, t: f64) -> Option<[f64; 4]>;
    fn sig(&self, scene: &Scene, n: NodeId, t: f64) -> u64;
    /// Whether `bounds`/`sig` of node `n` can change over time through anything other than
    /// the node's own tracks (planner v2 re-evaluates such nodes every frame). Default:
    /// yes (always safe).
    fn time_dependent(&self, _scene: &Scene, _n: NodeId) -> bool {
        true
    }
}

/// Bounds from the IR alone (rect fields; paths/text unsupported) — for planning passes.
pub struct RectInfo;
impl ContentInfo for RectInfo {
    fn bounds(&self, scene: &Scene, n: NodeId, _t: f64) -> Option<[f64; 4]> {
        scene.nodes[n].content.as_ref().and_then(content_rect)
    }
    fn sig(&self, _: &Scene, _: NodeId, _: f64) -> u64 {
        0
    }
    fn time_dependent(&self, _: &Scene, _: NodeId) -> bool {
        false
    }
}

pub struct Schedule {
    pub frames: usize,
    pub preorder: Vec<NodeId>,
    pub parents: Vec<Option<NodeId>>,
    pub states: Vec<Vec<NodeState>>,
    /// Asset shown by each Image/Sequence node, or state shown by a Switch, per frame.
    pub content: Vec<Vec<Option<AssetId>>>,
    /// Per frame and node: signature of everything else that changes the node's pixels.
    pub sig: Vec<Vec<u64>>,
    /// Per frame and content node: screen-space ink (empty = nothing visible).
    pub ink: Vec<Vec<[f64; 4]>>,
    /// The same before ancestor clips (input extent of offscreen effects).
    pub raw_ink: Vec<Vec<[f64; 4]>>,
    pub damage: Vec<Vec<[i32; 4]>>,
    /// Max on-screen scale per node over the frames where it is visible (raster scale input).
    pub max_scale: Vec<f64>,
    /// How many node-frames had a sub-threshold change that was not committed.
    pub quantized_changes: usize,
    /// tracks per node, sorted by order
    pub tracks: Vec<Vec<usize>>,
    /// nodes drawn only as matte sources (hidden or below a hidden node)
    #[allow(dead_code)]
    pub matte_only: Vec<bool>,
}

pub fn content_rect(c: &Content) -> Option<[f64; 4]> {
    match c {
        Content::Image { rect, .. } | Content::Sequence { rect, .. } | Content::Paint { rect, .. } | Content::Switch { rect, .. } => Some(*rect),
        Content::Rect { rect, .. } => Some(rect.rect),
        _ => None,
    }
}

pub fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    if a[2] <= 0.0 || a[3] <= 0.0 {
        return b;
    }
    if b[2] <= 0.0 || b[3] <= 0.0 {
        return a;
    }
    let x0 = a[0].min(b[0]);
    let y0 = a[1].min(b[1]);
    let x1 = (a[0] + a[2]).max(b[0] + b[2]);
    let y1 = (a[1] + a[3]).max(b[1] + b[3]);
    [x0, y0, x1 - x0, y1 - y0]
}

pub fn intersect(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    if x1 <= x0 || y1 <= y0 { [0.0; 4] } else { [x0, y0, x1 - x0, y1 - y0] }
}

pub fn inflate(r: [f64; 4], m: f64) -> [f64; 4] {
    if r[2] <= 0.0 || r[3] <= 0.0 { r } else { [r[0] - m, r[1] - m, r[2] + 2.0 * m, r[3] + 2.0 * m] }
}

pub fn transform_bbox(m: &Affine, r: [f64; 4]) -> [f64; 4] {
    let pts = [m.apply(r[0], r[1]), m.apply(r[0] + r[2], r[1]), m.apply(r[0], r[1] + r[3]), m.apply(r[0] + r[2], r[1] + r[3])];
    let x0 = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let x1 = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let y0 = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let y1 = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    [x0, y0, x1 - x0, y1 - y0]
}

pub fn to_irect(r: [f64; 4], w: f64, h: f64) -> Option<[i32; 4]> {
    if r[2] <= 0.0 || r[3] <= 0.0 {
        return None;
    }
    // +1 px for bilinear/AA fringe
    let x0 = (r[0] - 1.0).floor().max(0.0) as i32;
    let y0 = (r[1] - 1.0).floor().max(0.0) as i32;
    let x1 = (r[0] + r[2] + 1.0).ceil().min(w) as i32;
    let y1 = (r[1] + r[3] + 1.0).ceil().min(h) as i32;
    if x1 <= x0 || y1 <= y0 { None } else { Some([x0, y0, x1 - x0, y1 - y0]) }
}

/// Merge overlapping rects, then merge pairs whose union wastes little area (fewer,
/// larger draws are cheaper than many slivers).
pub fn merge_rects(mut v: Vec<[i32; 4]>) -> Vec<[i32; 4]> {
    let area = |r: &[i32; 4]| r[2] as i64 * r[3] as i64;
    let uni = |a: &[i32; 4], b: &[i32; 4]| {
        let x0 = a[0].min(b[0]);
        let y0 = a[1].min(b[1]);
        let x1 = (a[0] + a[2]).max(b[0] + b[2]);
        let y1 = (a[1] + a[3]).max(b[1] + b[3]);
        [x0, y0, x1 - x0, y1 - y0]
    };
    loop {
        let mut merged = false;
        'outer: for i in 0..v.len() {
            for j in i + 1..v.len() {
                let (a, b) = (v[i], v[j]);
                let touch = a[0] <= b[0] + b[2] && b[0] <= a[0] + a[2] && a[1] <= b[1] + b[3] && b[1] <= a[1] + a[3];
                let u = uni(&a, &b);
                if touch || area(&u) <= (area(&a) + area(&b)) * 5 / 4 + 4096 {
                    v[i] = u;
                    v.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            return v;
        }
    }
}

pub(crate) fn hash_f64s(h: &mut impl Hasher, v: &[f64]) {
    for x in v {
        x.to_bits().hash(h);
    }
}

/// Number-valued property at t: highest-order track with an effect, else `base`.
pub fn number_at(scene: &Scene, tracks: &[usize], prop: &str, t: f64, base: f64) -> f64 {
    let mut v = base;
    for &i in tracks {
        let tr = &scene.tracks[i];
        if tr.property == prop {
            if let Some(Sampled::Number(x)) = eval_track(tr, t) {
                v = x;
            }
        }
    }
    v
}

/// Value of a color track (`fill.color` / `stroke.color`) at t; None = no track has an effect.
pub fn color_at(scene: &Scene, tracks: &[usize], prop: &str, t: f64) -> Option<[f64; 4]> {
    let mut v = None;
    for &i in tracks {
        let tr = &scene.tracks[i];
        if tr.property == prop {
            if let Some(Sampled::Color(c)) = eval_track(tr, t) {
                v = Some(c);
            }
        }
    }
    v
}

/// Effect parameters at t: [radius, dx, dy, strength] for blur/drop-shadow/glow; look and
/// light effects: their `look_fields` in order, and the time in slot `look::TIME` when the
/// effect is time-dependent (so the signature changes every frame). Unused fields 0.
pub fn effect_params(scene: &Scene, tracks: &[usize], k: usize, e: &Effect, t: f64) -> [f64; crate::look::NPARAM] {
    let p = |f: &str, base: f64| number_at(scene, tracks, &format!("effects.{k}.{f}"), t, base);
    let mut out = [0.0; crate::look::NPARAM];
    match e {
        Effect::Blur { radius } => out[0] = p("radius", *radius).max(0.0),
        Effect::DropShadow { dx, dy, radius, .. } => out[..3].copy_from_slice(&[p("radius", *radius).max(0.0), p("dx", *dx), p("dy", *dy)]),
        Effect::Glow { radius, strength, .. } => {
            out[0] = p("radius", *radius).max(0.0);
            out[3] = p("strength", *strength);
        }
        Effect::Matte { .. } => {}
        e => {
            for (i, f) in e.look_fields().iter().enumerate() {
                out[i] = p(f, e.look_field(f));
            }
            if e.time_dependent() {
                out[crate::look::TIME] = t;
            }
        }
    }
    out
}

/// Extra ink (screen px) an effect adds around its subtree; `s` = node scale.
pub fn effect_margin(scene: &Scene, e: &Effect, p: &[f64; crate::look::NPARAM], s: f64) -> f64 {
    match e {
        Effect::Blur { .. } | Effect::Glow { .. } => s * BLUR_EXTENT * p[0],
        Effect::DropShadow { .. } => s * (BLUR_EXTENT * p[0] + p[1].abs().max(p[2].abs())),
        Effect::Matte { .. } => 0.0,
        e => crate::look::margin(e, p, s, [scene.width as f64, scene.height as f64]),
    }
}

/// Uncommitted local matrix (origin applied) and opacity of every node at time t.
pub fn eval_locals(scene: &Scene, tracks: &[Vec<usize>], base_m: &[Affine], t: f64) -> Result<Vec<(Affine, f64)>> {
    (0..scene.nodes.len()).map(|n| eval_local(scene, tracks, base_m, n, t)).collect()
}

/// Uncommitted local matrix (origin applied) and opacity of node `n` at time t.
pub fn eval_local(scene: &Scene, tracks: &[Vec<usize>], base_m: &[Affine], n: NodeId, t: f64) -> Result<(Affine, f64)> {
    let node = &scene.nodes[n];
    let mut m = base_m[n];
    let mut o = node.opacity;
    for &i in &tracks[n] {
        let tr = &scene.tracks[i];
        match (tr.property.as_str(), eval_track(tr, t)) {
            ("transform", Some(Sampled::Matrix(x))) => m = x,
            ("opacity", Some(Sampled::Number(x))) => o = x,
            ("transform" | "opacity", Some(s)) => bail!("track {i}: unexpected sample {s:?}"),
            _ => {}
        }
    }
    let (ox, oy) = (node.origin[0], node.origin[1]);
    Ok((Affine::translate(ox, oy).mul(&m).mul(&Affine::translate(-ox, -oy)), o))
}

/// `chains` for the nodes of `set` only (in preorder; every node's parent is in the set or
/// it is a root), written into `st`: the same arithmetic as `chains`, so the states of those
/// nodes are bitwise the ones a full evaluation gives.
pub fn chains_into(set: &[NodeId], parents: &[Option<NodeId>], locals: &[(Affine, f64)], st: &mut [NodeState]) {
    for (&n, &(l, o)) in set.iter().zip(locals) {
        let (pchain, peff) = match parents[n] {
            Some(p) => (st[p].chain, st[p].eff_opacity),
            None => (Affine::IDENTITY, 1.0),
        };
        st[n] = NodeState { local: l, chain: pchain.mul(&l), opacity: o, eff_opacity: peff * o };
    }
}

/// Chains and effective opacities from local values (preorder).
pub fn chains(preorder: &[NodeId], parents: &[Option<NodeId>], locals: &[(Affine, f64)]) -> Vec<NodeState> {
    let mut st = vec![NodeState { local: Affine::IDENTITY, chain: Affine::IDENTITY, opacity: 1.0, eff_opacity: 1.0 }; locals.len()];
    for &n in preorder {
        let (pchain, peff) = match parents[n] {
            Some(p) => (st[p].chain, st[p].eff_opacity),
            None => (Affine::IDENTITY, 1.0),
        };
        let (l, o) = locals[n];
        st[n] = NodeState { local: l, chain: pchain.mul(&l), opacity: o, eff_opacity: peff * o };
    }
    st
}

/// Switch / sequence state of a node at frame `f` (None for other content).
pub fn seq_state(scene: &Scene, n: NodeId, f: usize) -> Option<AssetId> {
    match &scene.nodes[n].content {
        Some(Content::Sequence { frames: seq, .. }) | Some(Content::Switch { frames: seq, .. }) => seq.iter().rev().find(|(ff, _)| *ff as usize <= f).map(|x| x.1),
        _ => None,
    }
}

/// Sub-frame sample times of a motion-blurred frame (centered on frame time).
pub fn mb_times(fps: f64, f: usize, samples: u32, shutter: f64) -> Vec<f64> {
    (0..samples).map(|k| (f as f64 + shutter * ((k as f64 + 0.5) / samples as f64 - 0.5)) / fps).collect()
}

/// Which planner `build` runs: `AGENTFX_PLANNER=v1` selects the reference planner (every node
/// evaluated at every frame, pairwise damage merge); anything else planner v2
/// (`schedule_v2`: activity intervals + spatial index, same output).
pub fn planner_v1() -> bool {
    agentfx_ir::env::var("AGENTFX_PLANNER").is_ok_and(|v| v == "v1")
}

pub fn build(scene: &Scene, info: &dyn ContentInfo) -> Result<Schedule> {
    if planner_v1() {
        return build_v1(scene, info);
    }
    let s = crate::schedule_v2::build(scene, info)?;
    // every schedule built by a test is checked against the reference planner
    #[cfg(test)]
    crate::schedule_v2::assert_same(&build_v1(scene, info)?, &s, "schedule::build (v1 vs v2)");
    Ok(s)
}

/// Reference planner (v1): evaluates every node at every frame. Kept for A/B
/// (`AGENTFX_PLANNER=v1`) and as the oracle of the v2 equivalence tests.
pub fn build_v1(scene: &Scene, info: &dyn ContentInfo) -> Result<Schedule> {
    let n_nodes = scene.nodes.len();
    let parents = scene.parents();
    let (w, h) = (scene.width as f64, scene.height as f64);
    let mut preorder = Vec::with_capacity(n_nodes);
    let mut stack = vec![0usize];
    while let Some(n) = stack.pop() {
        preorder.push(n);
        for &c in scene.nodes[n].children.iter().rev() {
            stack.push(c);
        }
    }
    if preorder.len() != n_nodes {
        bail!("scene graph is not a tree reachable from node 0");
    }
    let mut tracks: Vec<Vec<usize>> = vec![Vec::new(); n_nodes];
    for (i, t) in scene.tracks.iter().enumerate() {
        if agentfx_ir::validate::property_kind(&t.property).is_none() {
            bail!("track {i}: property {} not implemented", t.property);
        }
        tracks[t.node].push(i);
    }
    for v in &mut tracks {
        v.sort_by_key(|&i| scene.tracks[i].order);
    }
    let mut matte_only = vec![false; n_nodes];
    for &n in &preorder {
        matte_only[n] = scene.nodes[n].hidden || parents[n].is_some_and(|p| matte_only[p]);
    }
    // static subtree bounds (scene coords, t = 0) for displacement measurement
    let mut bounds = vec![[0.0f64; 4]; n_nodes];
    for &n in preorder.iter().rev() {
        let mut b = info.bounds(scene, n, 0.0).unwrap_or([0.0; 4]);
        for &c in &scene.nodes[n].children {
            b = union(b, bounds[c]);
        }
        bounds[n] = b;
    }
    let base_m: Vec<Affine> = scene.nodes.iter().map(|n| transform_matrix(&n.transform)).collect();
    // matte targets per source subtree: node -> list of matte sources affecting it
    let mut matte_srcs: Vec<Vec<NodeId>> = vec![vec![]; n_nodes];
    for &n in &preorder {
        let mut v = parents[n].map(|p| matte_srcs[p].clone()).unwrap_or_default();
        for e in &scene.nodes[n].effects {
            if let Effect::Matte { source, .. } = e {
                v.push(*source);
            }
        }
        if let Some(agentfx_ir::TimeFx::Displace { map: agentfx_ir::TimeMap::Layer { node }, .. }) = &scene.nodes[n].time {
            v.push(*node);
        }
        matte_srcs[n] = v;
    }
    let mb_node: Vec<Option<NodeId>> = {
        let mut v = vec![None; n_nodes];
        for &n in &preorder {
            v[n] = if scene.nodes[n].motion_blur.is_some() { Some(n) } else { parents[n].and_then(|p| v[p]) };
        }
        v
    };
    // nearest ancestor-or-self with a time effect (X3)
    let tfx_node: Vec<Option<NodeId>> = {
        let mut v = vec![None; n_nodes];
        for &n in &preorder {
            v[n] = if scene.nodes[n].time.is_some() { Some(n) } else { parents[n].and_then(|p| v[p]) };
        }
        v
    };
    let subtree = |root: NodeId| -> Vec<NodeId> {
        let mut out = vec![];
        let mut st = vec![root];
        while let Some(x) = st.pop() {
            out.push(x);
            st.extend(scene.nodes[x].children.iter().copied());
        }
        out
    };

    let frames = scene.frames as usize;
    let fps = scene.fps as f64;
    let mut states: Vec<Vec<NodeState>> = Vec::with_capacity(frames);
    let mut content: Vec<Vec<Option<AssetId>>> = Vec::with_capacity(frames);
    let mut sigs: Vec<Vec<u64>> = Vec::with_capacity(frames);
    let mut inks: Vec<Vec<[f64; 4]>> = Vec::with_capacity(frames);
    let mut raw_inks: Vec<Vec<[f64; 4]>> = Vec::with_capacity(frames);
    let mut max_scale = vec![0.0f64; n_nodes];
    let mut committed: Vec<Option<(Affine, f64)>> = vec![None; n_nodes];
    let mut quantized = 0usize;
    for f in 0..frames {
        let t = f as f64 / fps;
        let locals = eval_locals(scene, &tracks, &base_m, t)?;
        let mut st = vec![NodeState { local: Affine::IDENTITY, chain: Affine::IDENTITY, opacity: 1.0, eff_opacity: 1.0 }; n_nodes];
        for &n in &preorder {
            let (local, o) = locals[n];
            let (pchain, peff) = match parents[n] {
                Some(p) => (st[p].chain, st[p].eff_opacity),
                None => (Affine::IDENTITY, 1.0),
            };
            let (cl, co) = match committed[n] {
                None => (local, o),
                Some((cl, co)) => {
                    let b = bounds[n];
                    let corners = [(b[0], b[1]), (b[0] + b[2], b[1]), (b[0], b[1] + b[3]), (b[0] + b[2], b[1] + b[3])];
                    let s = pchain.max_scale();
                    let moved = corners
                        .iter()
                        .map(|&(x, y)| {
                            let (a, bb) = (local.apply(x, y), cl.apply(x, y));
                            (a.0 - bb.0).hypot(a.1 - bb.1) * s
                        })
                        .fold(0.0, f64::max);
                    let faded = (o - co).abs();
                    if local == cl && o == co {
                        (cl, co)
                    } else if moved >= POS_EPS_PX || faded >= OPACITY_EPS || (o == 0.0) != (co == 0.0) {
                        (local, o)
                    } else {
                        quantized += 1;
                        (cl, co)
                    }
                }
            };
            committed[n] = Some((cl, co));
            let chain = pchain.mul(&cl);
            let eff = peff * co;
            if eff > 0.0 {
                max_scale[n] = max_scale[n].max(chain.max_scale());
            }
            st[n] = NodeState { local: cl, chain, opacity: co, eff_opacity: eff };
        }
        let cont: Vec<Option<AssetId>> = scene
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| match &node.content {
                Some(Content::Image { asset, variants, .. }) => {
                    let s = st[i].chain.max_scale();
                    Some(variants.iter().copied().find(|&v| (scene.assets.get(v).map_or(0.0, |a| a.scale) - s).abs() < 1e-6).unwrap_or(*asset))
                }
                Some(Content::Sequence { frames: seq, .. }) => seq.iter().rev().find(|(ff, _)| *ff as usize <= f).map(|x| x.1),
                Some(Content::Switch { frames: seq, .. }) => seq.iter().rev().find(|(ff, _)| *ff as usize <= f).map(|x| x.1),
                _ => None,
            })
            .collect();
        // effect parameters and margins (screen px) per node, accumulated down the tree
        let mut eff_sig = vec![0u64; n_nodes];
        let mut margin = vec![0.0f64; n_nodes];
        for &n in &preorder {
            let node = &scene.nodes[n];
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            if let Some(p) = parents[n] {
                eff_sig[p].hash(&mut hs);
            }
            let mut m = parents[n].map_or(0.0, |p| margin[p]);
            let s = st[n].chain.max_scale();
            for (k, e) in node.effects.iter().enumerate() {
                let p = effect_params(scene, &tracks[n], k, e, t);
                hash_f64s(&mut hs, &p);
                m += effect_margin(scene, e, &p, s);
            }
            if let Some(tf) = &node.time {
                crate::timefx::span_at(scene, &tracks[n], tf, t).to_bits().hash(&mut hs);
            }
            eff_sig[n] = hs.finish();
            margin[n] = m;
        }
        // where opacity is applied and where clips sit (both change pixels without
        // changing a content node's chain / effective opacity)
        let mut group_sig = vec![0u64; n_nodes];
        for &n in &preorder {
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            if let Some(p) = parents[n] {
                group_sig[p].hash(&mut hs);
            }
            st[n].opacity.to_bits().hash(&mut hs);
            if !scene.nodes[n].clips.is_empty() {
                let pc = parents[n].map(|p| st[p].chain).unwrap_or(Affine::IDENTITY);
                hash_f64s(&mut hs, &[pc.a, pc.b, pc.c, pc.d, pc.e, pc.f]);
            }
            group_sig[n] = hs.finish();
        }
        // motion-blur sample states (uncommitted) of each motion-blurred node
        let mut mb_states: Vec<Option<Vec<Vec<NodeState>>>> = vec![None; n_nodes];
        for &n in &preorder {
            if let Some(mb) = &scene.nodes[n].motion_blur {
                let mut v = vec![];
                for ts in mb_times(fps, f, mb.samples, mb.shutter) {
                    let l = eval_locals(scene, &tracks, &base_m, ts)?;
                    v.push(chains(&preorder, &parents, &l));
                }
                mb_states[n] = Some(v);
            }
        }
        // time-effect sample states (uncommitted) of each node with a time effect
        let mut tfx_states: Vec<Option<Vec<(f64, usize, Vec<NodeState>)>>> = vec![None; n_nodes];
        for &n in &preorder {
            if let Some(tf) = &scene.nodes[n].time {
                let mut v = vec![];
                for ts in crate::timefx::sig_times(scene, &tracks[n], tf, t) {
                    let l = eval_locals(scene, &tracks, &base_m, ts)?;
                    v.push((ts, crate::timefx::frame_of(ts, fps, frames), chains(&preorder, &parents, &l)));
                }
                tfx_states[n] = Some(v);
            }
        }
        let mut sig = vec![0u64; n_nodes];
        let mut ink = vec![[0.0; 4]; n_nodes];
        let mut raw_ink = vec![[0.0; 4]; n_nodes];
        for n in 0..n_nodes {
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            eff_sig[n].hash(&mut hs);
            group_sig[n].hash(&mut hs);
            info.sig(scene, n, t).hash(&mut hs);
            for &src in &matte_srcs[n] {
                for x in subtree(src) {
                    hash_f64s(&mut hs, &[st[x].chain.a, st[x].chain.b, st[x].chain.c, st[x].chain.d, st[x].chain.e, st[x].chain.f, st[x].eff_opacity]);
                    cont[x].hash(&mut hs);
                    eff_sig[x].hash(&mut hs);
                    group_sig[x].hash(&mut hs);
                    info.sig(scene, x, t).hash(&mut hs);
                }
            }
            let Some(rect) = info.bounds(scene, n, t) else {
                sig[n] = hs.finish();
                continue;
            };
            let mut r = if st[n].eff_opacity > 0.0 || matte_only[n] { transform_bbox(&st[n].chain, rect) } else { [0.0; 4] };
            if let Some(m) = mb_node[n] {
                let mb = scene.nodes[m].motion_blur.unwrap();
                let times = mb_times(fps, f, mb.samples, mb.shutter);
                for (s, ts) in mb_states[m].as_ref().unwrap().iter().zip(&times) {
                    hash_f64s(&mut hs, &[s[n].chain.a, s[n].chain.b, s[n].chain.c, s[n].chain.d, s[n].chain.e, s[n].chain.f, s[n].eff_opacity]);
                    // content animated within the shutter (glyphs, trim) at the sample time
                    info.sig(scene, n, *ts).hash(&mut hs);
                    if s[n].eff_opacity > 0.0 {
                        if let Some(rs) = info.bounds(scene, n, *ts) {
                            r = union(r, transform_bbox(&s[n].chain, rs));
                        }
                    }
                }
            }
            if let Some(m) = tfx_node[n] {
                for (ts, fi, s) in tfx_states[m].as_ref().unwrap() {
                    hash_f64s(&mut hs, &[s[n].chain.a, s[n].chain.b, s[n].chain.c, s[n].chain.d, s[n].chain.e, s[n].chain.f, s[n].eff_opacity]);
                    info.sig(scene, n, *ts).hash(&mut hs);
                    seq_state(scene, n, *fi).hash(&mut hs);
                    if s[n].eff_opacity > 0.0 {
                        if let Some(rs) = info.bounds(scene, n, *ts) {
                            r = union(r, transform_bbox(&s[n].chain, rs));
                        }
                    }
                }
            }
            sig[n] = hs.finish();
            r = inflate(r, margin[n]);
            raw_ink[n] = r;
            // ancestor clips (in each clipping node's parent space)
            let mut c = [-1e6, -1e6, 2e6, 2e6];
            let mut x = Some(n);
            while let Some(k) = x {
                let pm = parents[k].map(|p| st[p].chain).unwrap_or(Affine::IDENTITY);
                for rr in &scene.nodes[k].clips {
                    c = intersect(c, transform_bbox(&pm, rr.rect));
                }
                x = parents[k];
            }
            ink[n] = intersect(r, c);
        }
        states.push(st);
        content.push(cont);
        sigs.push(sig);
        inks.push(ink);
        raw_inks.push(raw_ink);
    }

    let mut damage = Vec::with_capacity(frames);
    for f in 0..frames {
        if f == 0 {
            damage.push(vec![[0, 0, w as i32, h as i32]]);
            continue;
        }
        let mut rects = Vec::new();
        for n in 0..n_nodes {
            if scene.nodes[n].content.is_none() {
                continue;
            }
            let (a, b) = (&states[f - 1][n], &states[f][n]);
            if a.chain == b.chain && a.eff_opacity == b.eff_opacity && content[f - 1][n] == content[f][n] && sigs[f - 1][n] == sigs[f][n] {
                continue;
            }
            // matte-only nodes are drawn through their targets (whose sig covers them)
            if matte_only[n] {
                continue;
            }
            for fr in [f - 1, f] {
                if let Some(ir) = to_irect(inks[fr][n], w, h) {
                    rects.push(ir);
                }
            }
        }
        damage.push(merge_rects(rects));
    }
    Ok(Schedule { frames, preorder, parents, states, content, sig: sigs, ink: inks, raw_ink: raw_inks, damage, max_scale, quantized_changes: quantized, tracks, matte_only })
}

impl Schedule {
    /// Full-damage frames (cut points for parallel segments).
    pub fn full_frames(&self, w: u32, h: u32) -> Vec<usize> {
        let full = (w as i64) * (h as i64);
        (0..self.frames).filter(|&f| self.damage[f].iter().map(|r| r[2] as i64 * r[3] as i64).sum::<i64>() * 10 >= full * 9).collect()
    }

    pub fn stats(&self, scene: &Scene) -> serde_json::Value {
        let full = (scene.width * scene.height) as f64;
        let areas: Vec<f64> = self.damage.iter().map(|d| d.iter().map(|r| (r[2] * r[3]) as f64).sum::<f64>() / full).collect();
        let empty = areas.iter().filter(|&&a| a == 0.0).count();
        let nonempty: Vec<f64> = areas.iter().copied().filter(|&a| a > 0.0).collect();
        let r4 = |x: f64| (x * 1e4).round() / 1e4;
        let content_changes = (1..self.frames).filter(|&f| self.content[f] != self.content[f - 1]).count();
        serde_json::json!({
            "frames": self.frames,
            "nodes": scene.nodes.len(),
            "content_nodes": scene.nodes.iter().filter(|n| n.content.is_some()).count(),
            "tracks": scene.tracks.len(),
            "assets": scene.assets.len(),
            "frames_empty_damage": empty,
            "frames_composited": self.frames - empty,
            "frames_with_content_switch": content_changes,
            "quantized_changes": self.quantized_changes,
            "damage_area_mean_all": r4(areas.iter().sum::<f64>() / self.frames as f64),
            "damage_area_mean_nonempty": r4(if nonempty.is_empty() { 0.0 } else { nonempty.iter().sum::<f64>() / nonempty.len() as f64 }),
            "damage_area_max": r4(areas.iter().copied().fold(0.0, f64::max)),
            "damage_area_per_frame": areas.iter().map(|a| r4(*a)).collect::<Vec<_>>(),
            // [x, y, w, h] in output pixels, merged, per frame (what the renderer redraws)
            "damage_rects_per_frame": self.damage,
            "full_damage_frames": self.full_frames(scene.width, scene.height),
            "thresholds": { "pos_eps_px": POS_EPS_PX, "opacity_eps": OPACITY_EPS },
        })
    }
}
