//! Planner v2 (E1): the damage schedule of `schedule::build_v1`, bit for bit, without
//! evaluating every node at every frame and without the pairwise damage merge.
//!
//! **Activity intervals (interval bounds in time).** A track's value is constant before
//! `delay` and after `delay + duration` (fill decides which constant), so every track gets
//! a conservative closed activity interval `[delay - EPS, delay + duration + EPS]`. A node's
//! activity is the union of its own and its ancestors' track intervals. Everything the
//! schedule derives for a node at a frame (committed state, chain, content choice, effect
//! and group signatures, ink) is a deterministic function of values sampled at the frame
//! time (and its motion-blur sub-frame times) of the node and its ancestors, plus the
//! previous committed state. So when no activity interval of a node touches the time range
//! from the previous frame's earliest sample to this frame's latest sample, every derived
//! value equals the previous frame's, bit for bit, and the node is skipped. The rest are
//! "busy" frames, turned into per-node frame ranges and swept with an event list, so total
//! work is proportional to busy node-frames instead of nodes x frames.
//!
//! Nodes that cannot be bounded this way are busy at every frame: time effects (and their
//! subtrees), time-dependent looks (and subtrees), matte targets, tracks with a per-unit
//! stagger, and content whose ink or signature depends on t itself (clouds, fluids:
//! `ContentInfo::time_dependent`). Sequence/switch contents are busy at their switch frames.
//!
//! **Motion blur / time effects** are evaluated per region (the nodes whose nearest
//! motion-blurred ancestor is that node): the ancestor path at the sample time, then the
//! region in preorder. v1 evaluated the whole scene per motion-blurred node and sample,
//! O(n^2) per frame with per-layer motion blur.
//!
//! **Damage merge** (`merge_rects_fast`) reproduces the greedy merge of
//! `schedule::merge_rects` (lexicographically first mergeable pair, repeated) exactly, with
//! a linked list, a resume pointer instead of a restart, and an R-tree over conservative
//! "reach" boxes for the candidate queries. v1 restarted the O(k^2) scan and shifted the
//! vector after every merge.
use crate::schedule::{
    ContentInfo, NodeState, OPACITY_EPS, POS_EPS_PX, Schedule, effect_margin, effect_params, hash_f64s, inflate, intersect, mb_times,
    merge_rects, seq_state, to_irect, transform_bbox, union,
};
use anyhow::{Result, bail};
use agentfx_ir::eval::{eval_track, transform_matrix};
use agentfx_ir::{Affine, AssetId, Content, Effect, NodeId, Sampled, Scene};
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, RTreeObject};
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Margin (seconds) around a track's active range. `eval_segment` snaps times within 1e-6 s
/// of the range ends onto them; outside `[delay - EPS, end + EPS]` the value is constant.
const EPS: f64 = 1e-5;
const NIL: usize = usize::MAX;

/// Uncommitted local matrix (origin applied) and opacity of node `n` at time t: one node of
/// `schedule::eval_locals` (same arithmetic, same errors).
fn eval_local(scene: &Scene, tracks: &[usize], base_m: Affine, n: NodeId, t: f64) -> Result<(Affine, f64)> {
    let node = &scene.nodes[n];
    let mut m = base_m;
    let mut o = node.opacity;
    for &i in tracks {
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

/// Merge sorted closed intervals (touching ones too).
fn merge_intervals(mut v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(v.len());
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Per-sample (uncommitted) chain and effective opacity of a region (the nodes whose
/// nearest motion-blurred / time-effect ancestor-or-self is `root`), in region order.
struct Region {
    nodes: Vec<NodeId>,
}

struct Ctx<'a> {
    scene: &'a Scene,
    parents: &'a [Option<NodeId>],
    tracks: &'a [Vec<usize>],
    base_m: &'a [Affine],
    /// index of a node inside its region (motion blur and time effects separately)
    ridx_mb: Vec<usize>,
    ridx_tfx: Vec<usize>,
}

impl Ctx<'_> {
    /// Chain/opacity of every node of `region` (rooted at `root`) at time ts, computed like
    /// `schedule::chains(eval_locals(ts))`: ancestors from the root down, then the region.
    fn region_states(&self, root: NodeId, region: &Region, ridx: &[usize], ts: f64) -> Result<Vec<NodeState>> {
        let (pchain, peff) = self.path_state(self.parents[root], ts)?;
        self.region_states_from(root, region, ridx, ts, pchain, peff)
    }

    /// Uncommitted chain and effective opacity of node `x` (None: above the root) at ts.
    fn path_state(&self, x: Option<NodeId>, ts: f64) -> Result<(Affine, f64)> {
        let mut path = Vec::new();
        let mut x = x;
        while let Some(p) = x {
            path.push(p);
            x = self.parents[p];
        }
        let (mut pchain, mut peff) = (Affine::IDENTITY, 1.0);
        for &a in path.iter().rev() {
            let (l, o) = eval_local(self.scene, &self.tracks[a], self.base_m[a], a, ts)?;
            pchain = pchain.mul(&l);
            peff *= o;
        }
        Ok((pchain, peff))
    }

    fn region_states_from(&self, root: NodeId, region: &Region, ridx: &[usize], ts: f64, pchain: Affine, peff: f64) -> Result<Vec<NodeState>> {
        let mut out: Vec<NodeState> = Vec::with_capacity(region.nodes.len());
        for &n in &region.nodes {
            let (pc, pe) = if n == root { (pchain, peff) } else {
                let p = self.parents[n].unwrap();
                let s = &out[ridx[p]];
                (s.chain, s.eff_opacity)
            };
            let (l, o) = eval_local(self.scene, &self.tracks[n], self.base_m[n], n, ts)?;
            out.push(NodeState { local: l, chain: pc.mul(&l), opacity: o, eff_opacity: pe * o });
        }
        Ok(out)
    }
}

pub fn build(scene: &Scene, info: &dyn ContentInfo) -> Result<Schedule> {
    // ------------------------------------------------------------ setup (as v1)
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
    let mut bounds = vec![[0.0f64; 4]; n_nodes];
    for &n in preorder.iter().rev() {
        let mut b = info.bounds(scene, n, 0.0).unwrap_or([0.0; 4]);
        for &c in &scene.nodes[n].children {
            b = union(b, bounds[c]);
        }
        bounds[n] = b;
    }
    let base_m: Vec<Affine> = scene.nodes.iter().map(|n| transform_matrix(&n.transform)).collect();
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
    let mut mb_node: Vec<Option<NodeId>> = vec![None; n_nodes];
    let mut tfx_node: Vec<Option<NodeId>> = vec![None; n_nodes];
    for &n in &preorder {
        mb_node[n] = if scene.nodes[n].motion_blur.is_some() { Some(n) } else { parents[n].and_then(|p| mb_node[p]) };
        tfx_node[n] = if scene.nodes[n].time.is_some() { Some(n) } else { parents[n].and_then(|p| tfx_node[p]) };
    }
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

    // ------------------------------------------------------------ regions
    let mut mb_regions: Vec<Region> = Vec::new();
    let mut tfx_regions: Vec<Region> = Vec::new();
    let mut mb_reg_of = vec![NIL; n_nodes]; // region index of a motion-blurred node
    let mut tfx_reg_of = vec![NIL; n_nodes];
    let mut ridx_mb = vec![NIL; n_nodes];
    let mut ridx_tfx = vec![NIL; n_nodes];
    for &n in &preorder {
        if let Some(m) = mb_node[n] {
            if m == n {
                mb_reg_of[n] = mb_regions.len();
                mb_regions.push(Region { nodes: vec![] });
            }
            let r = &mut mb_regions[mb_reg_of[m]];
            ridx_mb[n] = r.nodes.len();
            r.nodes.push(n);
        }
        if let Some(m) = tfx_node[n] {
            if m == n {
                tfx_reg_of[n] = tfx_regions.len();
                tfx_regions.push(Region { nodes: vec![] });
            }
            let r = &mut tfx_regions[tfx_reg_of[m]];
            ridx_tfx[n] = r.nodes.len();
            r.nodes.push(n);
        }
    }
    let ctx = Ctx { scene, parents: &parents, tracks: &tracks, base_m: &base_m, ridx_mb, ridx_tfx };

    // ------------------------------------------------------------ busy frames per node
    // always busy: inherited (time effects, time-dependent looks: their parameters feed the
    // subtree's signatures) and own (matte targets, staggered tracks, t-dependent content)
    let mut always_inh = vec![false; n_nodes];
    let mut always = vec![false; n_nodes];
    let mut act: Vec<Arc<Vec<(f64, f64)>>> = vec![Arc::new(Vec::new()); n_nodes];
    for &n in &preorder {
        let node = &scene.nodes[n];
        always_inh[n] = parents[n].is_some_and(|p| always_inh[p]) || node.time.is_some() || node.effects.iter().any(|e| e.time_dependent());
        always[n] = always_inh[n]
            || !matte_srcs[n].is_empty()
            || tracks[n].iter().any(|&i| scene.tracks[i].stagger.is_some())
            || (node.content.is_some() && info.time_dependent(scene, n));
        let inherited = parents[n].map(|p| act[p].clone()).unwrap_or_default();
        act[n] = if tracks[n].is_empty() {
            inherited
        } else {
            let mut v: Vec<(f64, f64)> = inherited.as_ref().clone();
            for &i in &tracks[n] {
                let t = &scene.tracks[i];
                v.push((t.delay - EPS, t.delay + t.duration + EPS));
            }
            Arc::new(merge_intervals(v))
        };
    }
    // frame ranges [a, b] (inclusive) per node; frame 0 is computed for every node
    let last = frames.saturating_sub(1) as i64;
    let mut starts: Vec<Vec<usize>> = vec![Vec::new(); frames + 1];
    let mut ends: Vec<Vec<usize>> = vec![Vec::new(); frames + 1];
    let mut always_list: Vec<usize> = Vec::new();
    let mut pre_idx = vec![0usize; n_nodes];
    for (i, &n) in preorder.iter().enumerate() {
        pre_idx[n] = i;
    }
    let mut busy_node_frames = 0usize;
    if frames > 1 {
        for &n in &preorder {
            if always[n] {
                always_list.push(n);
                busy_node_frames += frames - 1;
                continue;
            }
            // sub-frame sample offset (seconds) around the frame time
            let d = mb_node[n].and_then(|m| scene.nodes[m].motion_blur).map_or(0.0, |mb| mb.shutter.abs() / 2.0 / fps) + EPS;
            let mut ranges: Vec<(i64, i64)> = Vec::new();
            for &(a, b) in act[n].iter() {
                // busy iff [(f - 1)/fps - d, f/fps + d] meets [a, b]
                let lo = (((a - d) * fps).floor() as i64 - 1).max(1);
                let hi = (((b + d) * fps).ceil() as i64 + 2).min(last);
                if lo <= hi {
                    ranges.push((lo, hi));
                }
            }
            if let Some(Content::Sequence { frames: seq, .. } | Content::Switch { frames: seq, .. }) = &scene.nodes[n].content {
                for &(ff, _) in seq {
                    let ff = ff as i64;
                    if ff >= 1 && ff <= last {
                        ranges.push((ff, ff));
                    }
                }
            }
            ranges.sort();
            let mut merged: Vec<(i64, i64)> = Vec::with_capacity(ranges.len());
            for (a, b) in ranges {
                match merged.last_mut() {
                    Some(l) if a <= l.1 + 1 => l.1 = l.1.max(b),
                    _ => merged.push((a, b)),
                }
            }
            for (a, b) in merged {
                busy_node_frames += (b - a + 1) as usize;
                starts[a as usize].push(n);
                ends[b as usize + 1].push(n);
            }
        }
    }

    // ------------------------------------------------------------ frames
    let mut states: Vec<Vec<NodeState>> = Vec::with_capacity(frames);
    let mut content: Vec<Vec<Option<AssetId>>> = Vec::with_capacity(frames);
    let mut sigs: Vec<Vec<u64>> = Vec::with_capacity(frames);
    let mut inks: Vec<Vec<[f64; 4]>> = Vec::with_capacity(frames);
    let mut raw_inks: Vec<Vec<[f64; 4]>> = Vec::with_capacity(frames);
    let mut damage: Vec<Vec<[i32; 4]>> = Vec::with_capacity(frames);
    let mut max_scale = vec![0.0f64; n_nodes];
    let mut committed: Vec<Option<(Affine, f64)>> = vec![None; n_nodes];
    let mut pending = vec![false; n_nodes];
    let mut n_pending = 0usize;
    let mut quantized = 0usize;
    // current values (persist across frames; busy nodes are updated in place)
    let mut st = vec![NodeState { local: Affine::IDENTITY, chain: Affine::IDENTITY, opacity: 1.0, eff_opacity: 1.0 }; n_nodes];
    let mut cont: Vec<Option<AssetId>> = vec![None; n_nodes];
    let mut eff_sig = vec![0u64; n_nodes];
    let mut margin = vec![0.0f64; n_nodes];
    let mut group_sig = vec![0u64; n_nodes];
    let mut sig = vec![0u64; n_nodes];
    let mut ink = vec![[0.0; 4]; n_nodes];
    let mut raw_ink = vec![[0.0; 4]; n_nodes];
    // busy set, ordered by preorder index
    let mut active: BTreeSet<usize> = BTreeSet::new();
    for &n in &always_list {
        active.insert(pre_idx[n]);
    }
    let mut busy: Vec<NodeId> = Vec::new();
    let mut locals: Vec<Result<(Affine, f64)>> = Vec::new();
    let mut mb_cache: Vec<Option<Vec<Vec<NodeState>>>> = (0..mb_regions.len()).map(|_| None).collect();
    let mut tfx_cache: Vec<Option<Vec<(f64, usize, Vec<NodeState>)>>> = (0..tfx_regions.len()).map(|_| None).collect();
    let mut cache_used_mb: Vec<usize> = Vec::new();
    let mut path_cache: std::collections::HashMap<(Option<NodeId>, u32, u64), Vec<(Affine, f64)>> = std::collections::HashMap::new();
    let mut cache_used_tfx: Vec<usize> = Vec::new();
    let mut tm = [0.0f64; 6]; // eval, damage merge, row copies, states, signatures, ink
    let mut n_rects = 0usize;
    for f in 0..frames {
        let t = f as f64 / fps;
        let t_eval = std::time::Instant::now();
        busy.clear();
        if f == 0 {
            busy.extend_from_slice(&preorder);
        } else {
            for &n in &starts[f] {
                active.insert(pre_idx[n]);
            }
            for &n in &ends[f] {
                active.remove(&pre_idx[n]);
            }
            busy.extend(active.iter().map(|&i| preorder[i]));
        }
        let t_ph = std::time::Instant::now();
        // uncommitted locals (pure, in parallel for large busy sets), then the committed
        // states in preorder
        {
            use rayon::prelude::*;
            let ev = |&n: &NodeId| eval_local(scene, &tracks[n], base_m[n], n, t);
            locals.clear();
            if busy.len() >= 4096 {
                locals.par_extend(busy.par_iter().map(ev));
            } else {
                locals.extend(busy.iter().map(ev));
            }
        }
        for (bi, &n) in busy.iter().enumerate() {
            let (local, o) = std::mem::replace(&mut locals[bi], Ok((Affine::IDENTITY, 1.0)))?;
            let (pchain, peff) = match parents[n] {
                Some(p) => (st[p].chain, st[p].eff_opacity),
                None => (Affine::IDENTITY, 1.0),
            };
            let mut is_pending = false;
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
                        is_pending = true;
                        (cl, co)
                    }
                }
            };
            if is_pending != pending[n] {
                pending[n] = is_pending;
                if is_pending { n_pending += 1 } else { n_pending -= 1 }
            }
            committed[n] = Some((cl, co));
            let chain = pchain.mul(&cl);
            let eff = peff * co;
            if eff > 0.0 {
                max_scale[n] = max_scale[n].max(chain.max_scale());
            }
            st[n] = NodeState { local: cl, chain, opacity: co, eff_opacity: eff };
        }
        // v1 counts every node whose sub-threshold change stays uncommitted at this frame;
        // a skipped node keeps its inputs, so it stays pending exactly when it was
        quantized += n_pending;
        tm[3] += t_ph.elapsed().as_secs_f64();
        let t_ph = std::time::Instant::now();
        // content choice, effect and group signatures (preorder: parents first)
        for &n in &busy {
            let node = &scene.nodes[n];
            cont[n] = match &node.content {
                Some(Content::Image { asset, variants, .. }) => {
                    let s = st[n].chain.max_scale();
                    Some(variants.iter().copied().find(|&v| (scene.assets.get(v).map_or(0.0, |a| a.scale) - s).abs() < 1e-6).unwrap_or(*asset))
                }
                Some(Content::Sequence { frames: seq, .. }) => seq.iter().rev().find(|(ff, _)| *ff as usize <= f).map(|x| x.1),
                Some(Content::Switch { frames: seq, .. }) => seq.iter().rev().find(|(ff, _)| *ff as usize <= f).map(|x| x.1),
                _ => None,
            };
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
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            if let Some(p) = parents[n] {
                group_sig[p].hash(&mut hs);
            }
            st[n].opacity.to_bits().hash(&mut hs);
            if !node.clips.is_empty() {
                let pc = parents[n].map(|p| st[p].chain).unwrap_or(Affine::IDENTITY);
                hash_f64s(&mut hs, &[pc.a, pc.b, pc.c, pc.d, pc.e, pc.f]);
            }
            group_sig[n] = hs.finish();
        }
        tm[4] += t_ph.elapsed().as_secs_f64();
        // motion-blur sample states of the regions with busy content (pure: in parallel)
        {
            use rayon::prelude::*;
            let mut need: Vec<usize> = busy.iter().filter(|&&n| scene.nodes[n].content.is_some()).filter_map(|&n| mb_node[n]).map(|m| mb_reg_of[m]).collect();
            need.sort_unstable();
            need.dedup();
            if need.len() >= 64 {
                for &ri in &need {
                    let m = mb_regions[ri].nodes[0];
                    let mb = scene.nodes[m].motion_blur.unwrap();
                    let key = (parents[m], mb.samples, mb.shutter.to_bits());
                    if !path_cache.contains_key(&key) {
                        let mut pv = Vec::new();
                        for ts in mb_times(fps, f, mb.samples, mb.shutter) {
                            pv.push(ctx.path_state(parents[m], ts)?);
                        }
                        path_cache.insert(key, pv);
                    }
                }
                let pc = &path_cache;
                let done: Vec<Result<Vec<Vec<NodeState>>>> = need
                    .par_iter()
                    .map(|&ri| {
                        let m = mb_regions[ri].nodes[0];
                        let mb = scene.nodes[m].motion_blur.unwrap();
                        let pv = &pc[&(parents[m], mb.samples, mb.shutter.to_bits())];
                        mb_times(fps, f, mb.samples, mb.shutter).iter().zip(pv).map(|(&ts, &(c, e))| ctx.region_states_from(m, &mb_regions[ri], &ctx.ridx_mb, ts, c, e)).collect()
                    })
                    .collect();
                for (ri, v) in need.into_iter().zip(done) {
                    mb_cache[ri] = Some(v?);
                    cache_used_mb.push(ri);
                }
            }
        }
        // signatures and ink
        for &n in &busy {
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
                let ri = mb_reg_of[m];
                if mb_cache[ri].is_none() {
                    // the ancestor path at the sample times is shared by sibling regions
                    let key = (parents[m], mb.samples, mb.shutter.to_bits());
                    if !path_cache.contains_key(&key) {
                        let mut pv = Vec::with_capacity(times.len());
                        for &ts in &times {
                            pv.push(ctx.path_state(parents[m], ts)?);
                        }
                        path_cache.insert(key, pv);
                    }
                    let pv = &path_cache[&key];
                    let mut v = Vec::with_capacity(times.len());
                    for (&ts, &(pc, pe)) in times.iter().zip(pv) {
                        v.push(ctx.region_states_from(m, &mb_regions[ri], &ctx.ridx_mb, ts, pc, pe)?);
                    }
                    mb_cache[ri] = Some(v);
                    cache_used_mb.push(ri);
                }
                let k = ctx.ridx_mb[n];
                for (s, ts) in mb_cache[ri].as_ref().unwrap().iter().zip(&times) {
                    let s = &s[k];
                    hash_f64s(&mut hs, &[s.chain.a, s.chain.b, s.chain.c, s.chain.d, s.chain.e, s.chain.f, s.eff_opacity]);
                    info.sig(scene, n, *ts).hash(&mut hs);
                    if s.eff_opacity > 0.0 {
                        if let Some(rs) = info.bounds(scene, n, *ts) {
                            r = union(r, transform_bbox(&s.chain, rs));
                        }
                    }
                }
            }
            if let Some(m) = tfx_node[n] {
                let ri = tfx_reg_of[m];
                if tfx_cache[ri].is_none() {
                    let tf = scene.nodes[m].time.as_ref().unwrap();
                    let mut v = vec![];
                    for ts in crate::timefx::sig_times(scene, &tracks[m], tf, t) {
                        v.push((ts, crate::timefx::frame_of(ts, fps, frames), ctx.region_states(m, &tfx_regions[ri], &ctx.ridx_tfx, ts)?));
                    }
                    tfx_cache[ri] = Some(v);
                    cache_used_tfx.push(ri);
                }
                let k = ctx.ridx_tfx[n];
                for (ts, fi, s) in tfx_cache[ri].as_ref().unwrap() {
                    let s = &s[k];
                    hash_f64s(&mut hs, &[s.chain.a, s.chain.b, s.chain.c, s.chain.d, s.chain.e, s.chain.f, s.eff_opacity]);
                    info.sig(scene, n, *ts).hash(&mut hs);
                    seq_state(scene, n, *fi).hash(&mut hs);
                    if s.eff_opacity > 0.0 {
                        if let Some(rs) = info.bounds(scene, n, *ts) {
                            r = union(r, transform_bbox(&s.chain, rs));
                        }
                    }
                }
            }
            sig[n] = hs.finish();
            r = inflate(r, margin[n]);
            raw_ink[n] = r;
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
        path_cache.clear();
        for ri in cache_used_mb.drain(..) {
            mb_cache[ri] = None;
        }
        for ri in cache_used_tfx.drain(..) {
            tfx_cache[ri] = None;
        }
        tm[0] += t_eval.elapsed().as_secs_f64();
        let t_dmg = std::time::Instant::now();
        // damage: only busy nodes can differ from the previous frame (node index order, as v1)
        if f == 0 {
            damage.push(vec![[0, 0, w as i32, h as i32]]);
        } else {
            let (ps, pc, pg, pi) = (&states[f - 1], &content[f - 1], &sigs[f - 1], &inks[f - 1]);
            let mut changed: Vec<NodeId> = busy
                .iter()
                .copied()
                .filter(|&n| scene.nodes[n].content.is_some() && !matte_only[n])
                .filter(|&n| !(ps[n].chain == st[n].chain && ps[n].eff_opacity == st[n].eff_opacity && pc[n] == cont[n] && pg[n] == sig[n]))
                .collect();
            changed.sort_unstable();
            let mut rects = Vec::with_capacity(2 * changed.len());
            for n in changed {
                for r in [pi[n], ink[n]] {
                    if let Some(ir) = to_irect(r, w, h) {
                        rects.push(ir);
                    }
                }
            }
            n_rects += rects.len();
            damage.push(merge_rects_fast(rects));
        }
        tm[1] += t_dmg.elapsed().as_secs_f64();
        let t_copy = std::time::Instant::now();
        states.push(st.clone());
        content.push(cont.clone());
        sigs.push(sig.clone());
        inks.push(ink.clone());
        raw_inks.push(raw_ink.clone());
        tm[2] += t_copy.elapsed().as_secs_f64();
    }
    if agentfx_ir::env::var("AGENTFX_PLAN_DEBUG").is_ok() {
        eprintln!("planner v2: {n_nodes} nodes, {frames} frames, busy node-frames {busy_node_frames} ({:.1} %), always-busy nodes {}; eval {:.2} s, damage {:.2} s ({n_rects} rects in), rows {:.2} s, states/sigs {:.2}/{:.2} s; merge steps/queries/candidates/merges/cascades {:?}", 100.0 * busy_node_frames as f64 / (n_nodes * frames.max(1)) as f64, always_list.len(), tm[0], tm[1], tm[2], tm[3], tm[4], MSTAT.iter().map(|a| a.load(std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>());
    }
    Ok(Schedule { frames, preorder, parents, states, content, sig: sigs, ink: inks, raw_ink: raw_inks, damage, max_scale, quantized_changes: quantized, tracks, matte_only })
}

// ---------------------------------------------------------------- damage merge

type Entry = GeomWithData<Rectangle<[f64; 2]>, usize>;

fn area(r: &[i32; 4]) -> i64 {
    r[2] as i64 * r[3] as i64
}
fn uni(a: &[i32; 4], b: &[i32; 4]) -> [i32; 4] {
    let x0 = a[0].min(b[0]);
    let y0 = a[1].min(b[1]);
    let x1 = (a[0] + a[2]).max(b[0] + b[2]);
    let y1 = (a[1] + a[3]).max(b[1] + b[3]);
    [x0, y0, x1 - x0, y1 - y0]
}
/// The v1 merge predicate (`schedule::merge_rects`): touching, or the union wastes little.
fn mergeable(a: &[i32; 4], b: &[i32; 4]) -> bool {
    let touch = a[0] <= b[0] + b[2] && b[0] <= a[0] + a[2] && a[1] <= b[1] + b[3] && b[1] <= a[1] + a[3];
    touch || area(&uni(a, b)) <= (area(a) + area(b)) * 5 / 4 + 4096
}

/// Conservative reach box: if `mergeable(a, b)` then reach(a) and reach(b) intersect.
///
/// Not touching and apart by a gap gx > 0 along x: the union is at least
/// (wa + wb + gx) x M with M = max(ha, hb), and A + B <= M (wa + wb), so the area test
/// gives gx <= (wa + wb) / 4 + 4096 / M <= (wa/4 + 2048/ha) + (wb/4 + 2048/hb). Same along
/// y. Each rect is grown by its share (+1 px slack); overlapping axes need nothing.
fn reach(r: &[i32; 4]) -> Rectangle<[f64; 2]> {
    let (w, h) = (r[2].max(1) as f64, r[3].max(1) as f64);
    let rx = 0.25 * w + 2048.0 / h + 1.0;
    let ry = 0.25 * h + 2048.0 / w + 1.0;
    Rectangle::from_corners([r[0] as f64 - rx, r[1] as f64 - ry], [r[0] as f64 + w + rx, r[1] as f64 + h + ry])
}

struct Merger {
    r: Vec<[i32; 4]>,
    alive: Vec<bool>,
    next: Vec<usize>,
    prev: Vec<usize>,
    head: usize,
    /// index over the live rects except the cursor (the only rect that grows repeatedly:
    /// keeping it out avoids re-indexing a box that ends up covering everything)
    tree: Option<RTree<Entry>>,
    in_tree: Vec<bool>,
    cursor: usize,
    probe: usize,
}

/// Linear probes before the index is consulted: dense damage finds its partner by a short
/// scan in list order (as v1 does), the index only pays off for long misses.
const PROBE: usize = 256;

/// Merge statistics (AGENTFX_PLAN_DEBUG): linear steps, index queries, candidates, merges, cascades.
pub static MSTAT: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];
fn mstat(i: usize, n: u64) {
    MSTAT[i].fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

impl Merger {
    fn entry(&self, x: usize) -> Entry {
        GeomWithData::new(reach(&self.r[x]), x)
    }
    /// Lazy deletion: entries of dead rects and of the cursor stay in the tree and are
    /// filtered by `in_tree` (an entry always carries the rect's box from when it was
    /// inserted; a rect only grows while it is the cursor and is re-inserted when it stops
    /// being the cursor, so its live entry has its current box).
    fn tree_remove(&mut self, x: usize) {
        self.in_tree[x] = false;
    }
    fn tree_insert(&mut self, x: usize) {
        if self.tree.is_some() && !self.in_tree[x] && self.alive[x] {
            let e = self.entry(x);
            self.tree.as_mut().unwrap().insert(e);
            self.in_tree[x] = true;
        }
    }
    fn unlink(&mut self, x: usize) {
        self.tree_remove(x);
        self.alive[x] = false;
        let (p, n) = (self.prev[x], self.next[x]);
        if p != NIL {
            self.next[p] = n;
        } else {
            self.head = n;
        }
        if n != NIL {
            self.prev[n] = p;
        }
    }
    /// Move the cursor to x (x leaves the index, the old cursor joins it).
    fn set_cursor(&mut self, x: usize) {
        let old = self.cursor;
        self.cursor = x;
        if old != NIL && old != x {
            self.tree_insert(old);
        }
        if x != NIL {
            self.tree_remove(x);
        }
    }
    /// Grow the cursor rect (never indexed).
    fn grow_cursor(&mut self, v: [i32; 4]) {
        debug_assert!(!self.in_tree[self.cursor]);
        self.r[self.cursor] = v;
    }
    fn build_tree(&mut self) {
        if self.tree.is_none() {
            let v: Vec<Entry> = (0..self.r.len()).filter(|&x| self.alive[x] && x != self.cursor).map(|x| self.entry(x)).collect();
            for e in &v {
                self.in_tree[e.data] = true;
            }
            self.tree = Some(RTree::bulk_load(v));
        }
    }
    /// Smallest live position in [lo, hi) mergeable with `q` (via the index).
    fn query(&mut self, q: [i32; 4], lo: usize, hi: usize) -> Option<usize> {
        self.build_tree();
        let env = reach(&q).envelope();
        let (r, it) = (&self.r, &self.in_tree);
        mstat(1, 1);
        let mut nc = 0;
        let out = self.tree.as_ref().unwrap().locate_in_envelope_intersecting(&env).map(|e| e.data).inspect(|_| nc += 1).filter(|&x| x >= lo && x < hi && it[x] && mergeable(&r[x], &q)).min();
        mstat(2, nc);
        out
    }
    /// v1: the first live j after p with mergeable(v[p], v[j]).
    fn find_forward(&mut self, p: usize) -> Option<usize> {
        let q = self.r[p];
        let mut j = self.next[p];
        let mut steps = 0;
        while j != NIL && steps < self.probe {
            if mergeable(&q, &self.r[j]) {
                return Some(j);
            }
            j = self.next[j];
            steps += 1;
        }
        mstat(0, steps as u64);
        if j == NIL { None } else { self.query(q, j, usize::MAX) }
    }
    /// The first live i before p with mergeable(v[i], v[p]).
    fn find_prefix(&mut self, p: usize) -> Option<usize> {
        let q = self.r[p];
        let mut i = self.head;
        let mut steps = 0;
        while i != p && steps < self.probe {
            if mergeable(&self.r[i], &q) {
                return Some(i);
            }
            i = self.next[i];
            steps += 1;
        }
        mstat(0, steps as u64);
        if i == p { None } else { self.query(q, i, p) }
    }
}

/// `schedule::merge_rects`, same result for any input, without its O(k) shift per merge
/// and its restart from the front after every merge.
///
/// v1 repeatedly merges the lexicographically first mergeable pair (i, j). Invariant here:
/// every live rect before the cursor `p`, and every live rect between `p` and the
/// `frontier` (the furthest position the cursor reached), has no mergeable rect after it.
/// Only `v[p]` grows; when it does, the first earlier rect mergeable with it (if any) is
/// the next pair (its other candidates are unchanged), so the merge cascades backwards;
/// otherwise the next pair is (p, first mergeable after p). When p has none, the cursor
/// resumes after the frontier. Candidates are found by a short scan in list order, then
/// through an R-tree of reach boxes.
pub fn merge_rects_fast(v: Vec<[i32; 4]>) -> Vec<[i32; 4]> {
    if v.len() <= 32 {
        return merge_rects(v);
    }
    merge_rects_probe(v, PROBE)
}

fn merge_rects_probe(v: Vec<[i32; 4]>, probe: usize) -> Vec<[i32; 4]> {
    let k = v.len();
    if k == 0 {
        return v;
    }
    let mut m = Merger {
        r: v,
        alive: vec![true; k],
        next: (1..=k).map(|x| if x == k { NIL } else { x }).collect(),
        prev: (0..k).map(|x| if x == 0 { NIL } else { x - 1 }).collect(),
        head: 0,
        tree: None,
        in_tree: vec![false; k],
        cursor: NIL,
        probe,
    };
    let mut p = 0usize;
    let mut frontier = 0usize;
    m.set_cursor(p);
    while p != NIL {
        if let Some(j) = m.find_forward(p) {
            let u = uni(&m.r[p], &m.r[j]);
            m.unlink(j);
            m.grow_cursor(u);
            mstat(3, 1);
            while let Some(i) = m.find_prefix(p) {
                let u = uni(&m.r[i], &m.r[p]);
                mstat(4, 1);
                m.cursor = NIL; // p disappears, i becomes the cursor
                m.unlink(p);
                m.set_cursor(i);
                m.grow_cursor(u);
                p = i;
            }
        } else {
            let mut x = frontier.max(p) + 1;
            while x < k && !m.alive[x] {
                x += 1;
            }
            p = if x < k { x } else { NIL };
            frontier = x;
            m.set_cursor(p);
        }
    }
    let mut out = Vec::new();
    let mut x = m.head;
    while x != NIL {
        out.push(m.r[x]);
        x = m.next[x];
    }
    out
}

/// Test oracle: every field of two schedules equal, bit for bit (NaN-safe).
#[cfg(test)]
pub(crate) fn assert_same(a: &Schedule, b: &Schedule, what: &str) {
    fn st_bits(v: &[NodeState]) -> Vec<u64> {
        v.iter()
            .flat_map(|s| {
                let (l, c) = (s.local, s.chain);
                [l.a, l.b, l.c, l.d, l.e, l.f, c.a, c.b, c.c, c.d, c.e, c.f, s.opacity, s.eff_opacity]
            })
            .map(f64::to_bits)
            .collect()
    }
    fn r_bits(v: &[[f64; 4]]) -> Vec<u64> {
        v.iter().flatten().map(|x| x.to_bits()).collect()
    }
    assert_eq!(a.frames, b.frames, "{what}: frames");
    assert_eq!(a.quantized_changes, b.quantized_changes, "{what}: quantized");
    assert_eq!(a.max_scale.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), b.max_scale.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), "{what}: max_scale");
    for f in 0..a.frames {
        assert_eq!(a.damage[f], b.damage[f], "{what}: damage at frame {f}");
        assert!(st_bits(&a.states[f]) == st_bits(&b.states[f]), "{what}: states at frame {f}");
        assert_eq!(a.content[f], b.content[f], "{what}: content at frame {f}");
        assert_eq!(a.sig[f], b.sig[f], "{what}: sig at frame {f}");
        assert!(r_bits(&a.ink[f]) == r_bits(&b.ink[f]), "{what}: ink at frame {f}");
        assert!(r_bits(&a.raw_ink[f]) == r_bits(&b.raw_ink[f]), "{what}: raw_ink at frame {f}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{RectInfo, build_v1};

    /// Small deterministic PRNG (no dependency).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn f(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        fn range(&mut self, a: f64, b: f64) -> f64 {
            a + (b - a) * self.f()
        }
        fn pick<T: Clone>(&mut self, v: &[T]) -> T {
            v[(self.next() % v.len() as u64) as usize].clone()
        }
    }

    #[test]
    fn merge_fast_equals_v1_random() {
        let mut rng = Rng(0x9e3779b97f4a7c15);
        for case in 0..400 {
            let k = [0, 1, 5, 33, 40, 80, 200, 600, 2500][case % 9];
            let spread = [60.0, 400.0, 1920.0][case % 3];
            let maxs = [3.0, 12.0, 40.0, 300.0][case % 4];
            let v: Vec<[i32; 4]> = (0..k)
                .map(|_| {
                    let x = rng.range(0.0, spread) as i32;
                    let y = rng.range(0.0, spread * 0.56) as i32;
                    let w = rng.range(1.0, maxs) as i32 + 1;
                    let h = if rng.f() < 0.1 { 1 } else { rng.range(1.0, maxs) as i32 + 1 };
                    [x, y, w, h]
                })
                .collect();
            let want = merge_rects(v.clone());
            assert_eq!(merge_rects_fast(v.clone()), want, "case {case}: {v:?}");
            // short probes: the index answers most queries
            for probe in [0, 1, 3] {
                assert_eq!(merge_rects_probe(v.clone(), probe), want, "case {case} probe {probe}: {v:?}");
            }
        }
    }

    #[test]
    fn merge_fast_equals_v1_sparse_grid() {
        // far-apart rects that never merge, plus late rects that bridge them
        let mut v = Vec::new();
        for y in 0..12 {
            for x in 0..20 {
                v.push([x * 96, y * 90, 6, 6]);
            }
        }
        for x in 0..20 {
            v.push([x * 96, 500, 90, 4]);
        }
        let want = merge_rects(v.clone());
        assert_eq!(merge_rects_fast(v.clone()), want);
        for probe in [0, 2] {
            assert_eq!(merge_rects_probe(v.clone(), probe), want);
        }
    }

    /// Random dot scene (like tools/gen-dots.py): every node animated on its own, some
    /// nested under animated groups, some motion blurred, sub-threshold wiggles included.
    pub(crate) fn random_scene(seed: u64, n: usize, frames: u32, mb_frac: f64) -> Scene {
        use agentfx_ir::*;
        let mut rng = Rng(seed | 1);
        let secs = frames as f64 / 30.0;
        let mut nodes = vec![Node::new("root"), Node::new("bg")];
        nodes[1].content = Some(Content::Rect { rect: RRect { rect: [0.0, 0.0, 1920.0, 1080.0], radius: [0.0; 4] }, color: [0.0, 0.0, 0.0, 1.0] });
        nodes[0].children.push(1);
        let mut tracks: Vec<Track> = vec![];
        let tf = |v: Vec<TransformFn>| Value::Transform(v);
        let mk = |node: usize, prop: &str, delay: f64, dur: f64, fill: FillMode, easing: Easing, kfs: Vec<(f64, Value)>| Track {
            node,
            property: prop.into(),
            order: 0,
            label: String::new(),
            delay,
            duration: dur,
            fill,
            easing,
            keyframes: kfs.into_iter().map(|(o, v)| Keyframe { offset: o, value: v, easing: None }).collect(),
            stagger: None,
        };
        let mut parents_pool = vec![0usize];
        for g in 0..3 {
            let id = nodes.len();
            let mut gn = Node::new(format!("group {g}"));
            gn.origin = [960.0, 540.0];
            if g == 1 {
                gn.motion_blur = Some(MotionBlur { samples: 4, shutter: 0.5 });
            }
            if g == 2 {
                gn.clips.push(RRect { rect: [100.0, 100.0, 1500.0, 800.0], radius: [0.0; 4] });
                gn.opacity = 0.8;
            }
            nodes.push(gn);
            nodes[0].children.push(id);
            parents_pool.push(id);
            let d = rng.range(0.0, secs * 0.5);
            tracks.push(mk(id, "transform", d, rng.range(0.2, secs * 0.4), FillMode::Both, Easing::EASE, vec![(0.0, tf(vec![TransformFn::Translate(0.0, 0.0)])), (1.0, tf(vec![TransformFn::Translate(rng.range(-80.0, 80.0), rng.range(-50.0, 50.0))]))]));
        }
        let easings = [
            Easing::Linear,
            Easing::EASE,
            Easing::CubicBezier(0.3, -0.5, 0.6, 1.6),
            Easing::Spring { mass: 1.0, stiffness: 170.0, damping: 12.0, velocity: 0.0 },
            Easing::Steps(4, StepPos::JumpEnd),
        ];
        let fills = [FillMode::Both, FillMode::None, FillMode::Forwards, FillMode::Backwards];
        for k in 0..n {
            let id = nodes.len();
            let s = rng.pick(&[4.0, 6.0, 10.0]);
            let (x, y) = (rng.range(0.0, 1900.0), rng.range(0.0, 1060.0));
            let mut nd = Node::new(format!("dot {k}"));
            nd.origin = [x + s / 2.0, y + s / 2.0];
            nd.content = Some(Content::Rect { rect: RRect { rect: [x, y, s, s], radius: [s / 2.0; 4] }, color: [1.0, 0.5, 0.2, 1.0] });
            if rng.f() < mb_frac {
                nd.motion_blur = Some(MotionBlur { samples: 3, shutter: rng.pick(&[0.5, 1.0]) });
            }
            if rng.f() < 0.05 {
                nd.effects.push(Effect::Blur { radius: 2.0 });
            }
            nodes.push(nd);
            let p = rng.pick(&parents_pool);
            nodes[p].children.push(id);
            let delay = rng.range(-0.5, secs);
            let dur = rng.range(0.05, 2.0);
            let amp = if rng.f() < 0.15 { 0.02 } else { 200.0 }; // sub-threshold wiggles too
            let kf = |rng: &mut Rng| tf(vec![TransformFn::Translate(rng.range(-amp, amp), rng.range(-amp, amp)), TransformFn::Scale(1.0, 1.0), TransformFn::Rotate(rng.range(-30.0, 30.0) * (amp / 200.0))]);
            let k0 = kf(&mut rng);
            let k1 = kf(&mut rng);
            let k2 = kf(&mut rng);
            tracks.push(mk(id, "transform", delay, dur, rng.pick(&fills), rng.pick(&easings), vec![(0.0, k0), (rng.range(0.1, 0.9), k1), (1.0, k2)]));
            if rng.f() < 0.4 {
                tracks.push(mk(id, "opacity", rng.range(0.0, secs), rng.range(0.03, 1.0), rng.pick(&fills), Easing::Linear, vec![(0.0, Value::Number(0.0)), (1.0, Value::Number(1.0))]));
            }
            if rng.f() < 0.05 {
                let mut tr = mk(id, "effects.0.radius", rng.range(0.0, secs), 0.5, FillMode::Both, Easing::Linear, vec![(0.0, Value::Number(0.0)), (1.0, Value::Number(4.0))]);
                if nodes[id].effects.is_empty() {
                    nodes[id].effects.push(Effect::Blur { radius: 1.0 });
                }
                tr.order = 1;
                tracks.push(tr);
            }
        }
        // a switch node (content changes at given frames)
        let id = nodes.len();
        let mut sw = Node::new("switch");
        let st = |c: f64| Content::Rect { rect: RRect { rect: [10.0, 10.0, 50.0, 30.0], radius: [0.0; 4] }, color: [c, c, c, 1.0] };
        sw.content = Some(Content::Switch { rect: [10.0, 10.0, 50.0, 30.0], frames: vec![(0, 0), (frames / 3, 1), (frames / 2, 0)], states: vec![st(0.2), st(0.8)] });
        nodes.push(sw);
        nodes[0].children.push(id);
        Scene { version: IR_VERSION, width: 1920, height: 1080, fps: 30, frames, nodes, tracks, assets: vec![], fonts: vec![], camera: None, text_aa: TextAa::Gray }
    }

    #[test]
    fn v2_equals_v1_random_scenes() {
        for seed in 1..=12u64 {
            let n = [10, 60, 250][seed as usize % 3];
            let scene = random_scene(seed * 7919, n, 90, 0.1);
            let a = build_v1(&scene, &RectInfo).unwrap();
            let b = build(&scene, &RectInfo).unwrap();
            assert_same(&a, &b, &format!("seed {seed}"));
            assert!(b.damage.iter().skip(1).any(|d| !d.is_empty()));
        }
    }

    #[test]
    fn v2_equals_v1_per_layer_motion_blur() {
        // every dot motion blurred: the parallel region path runs
        let scene = random_scene(77, 200, 20, 1.0);
        let a = build_v1(&scene, &RectInfo).unwrap();
        let b = build(&scene, &RectInfo).unwrap();
        assert_same(&a, &b, "per-layer motion blur");
    }

    #[test]
    fn v2_equals_v1_example_fixtures() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ir/direct");
        let mut n = 0;
        for name in ["lower-third.ir.json", "showcase-16x9.ir.json", "showcase-9x16.ir.json"] {
            let path = dir.join(name);
            let scene = Scene::load(&path).unwrap();
            let prep = crate::vraster::Prepared::new(&scene, &dir, None, None).unwrap();
            let info = crate::vraster::Info::new(&scene, &prep);
            let a = build_v1(&scene, &info).unwrap();
            let b = build(&scene, &info).unwrap();
            assert_same(&a, &b, name);
            n += 1;
        }
        assert_eq!(n, 3);
    }

    #[test]
    #[ignore = "benchmark: cargo test --release -p reel planner_bench -- --ignored --nocapture"]
    fn planner_bench() {
        for n in [1000usize, 4000] {
            let scene = random_scene(42, n, 300, 0.1);
            let t = std::time::Instant::now();
            let b = build(&scene, &RectInfo).unwrap();
            let t2 = t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            let a = build_v1(&scene, &RectInfo).unwrap();
            let t1 = t.elapsed().as_secs_f64();
            assert_same(&a, &b, "bench");
            eprintln!("n={n}: v1 {t1:.2} s, v2 {t2:.3} s");
        }
    }
}
