//! Per-layer motion blur, render side (child module of `vraster`).
//!
//! The reference path (`AGENTFX_MBLUR=v1`) evaluates the whole scene per motion-blurred node
//! and per sub-frame sample and renders every sample, so a frame with k blurred layers costs
//! O(k * nodes * samples) evaluation plus k * samples offscreen renders. Inside a time
//! effect (slit-scan, echo) that repeats for every time sample of the enclosing subtree.
//!
//! This path computes the same pixels with less work:
//! 1. Region evaluation (as planner v2 does at compile time, `schedule_v2.rs`): a sample
//!    evaluates only the nodes its render can read: the blurred node's ancestor path and
//!    subtree, plus the ancestor paths and subtrees of matte sources used inside it. Their
//!    states are bitwise the full evaluation's (`schedule::eval_local` / `chains_into`).
//! 2. Identical samples render once: samples whose evaluated states, content signatures and
//!    effect parameters are bitwise equal give the same raster, so it is rendered once and
//!    added with its multiplicity (integer accumulation: exact). A layer at rest during the
//!    shutter costs one render instead of `samples`. Subtrees whose pixels depend on the
//!    sample time in other ways (time effects, clouds, fluids, worlds, fade rasters) render
//!    every sample.
//! 3. The averaged raster is cached for the frame by (node, frame, rect, matte pass,
//!    content states): the samples sit at the frame's sub-frame times whatever view the
//!    node is drawn in, so time effects that draw the subtree at many sample times reuse it.
//! 4. Accumulation is serial for small rasters (rayon dispatch dominated before).
use super::*;

/// What a motion-blurred node's samples read.
pub(super) struct MbPlan {
    /// nodes evaluated per sample, in preorder (parents first)
    set: Vec<NodeId>,
    /// nodes whose content is drawn (the subtree and matte-source subtrees)
    drawn: Vec<NodeId>,
    /// samples can be compared by state (no other time dependence inside)
    dedup: bool,
}

/// Byte budget of the per-frame cache of averaged motion-blur rasters.
const MB_CACHE_LIMIT: usize = 256 << 20;

#[derive(Default)]
pub(super) struct MbCache {
    frame: usize,
    bytes: usize,
    map: std::collections::HashMap<(NodeId, [i32; 4], bool, bool, u64), Arc<Pixmap>>,
}

pub(super) fn mblur_v1() -> bool {
    agentfx_ir::env::var("AGENTFX_MBLUR").is_ok_and(|v| v == "v1")
}

impl<'a> Painter<'a> {
    fn mb_plan(&mut self, n: NodeId) -> Arc<MbPlan> {
        if let Some(p) = self.mb_plans.get(&n) {
            return p.clone();
        }
        let scene = self.scene;
        let nn = scene.nodes.len();
        let mut in_set = vec![false; nn];
        let mut drawn = vec![false; nn];
        // subtree roots to add: n, then matte sources found inside drawn subtrees
        let mut roots = vec![n];
        while let Some(r) = roots.pop() {
            if drawn[r] {
                continue;
            }
            let mut a = Some(r);
            while let Some(x) = a {
                in_set[x] = true;
                a = self.sched.parents[x];
            }
            let mut st = vec![r];
            while let Some(x) = st.pop() {
                if drawn[x] {
                    continue;
                }
                drawn[x] = true;
                in_set[x] = true;
                for e in &scene.nodes[x].effects {
                    if let Effect::Matte { source, .. } = e {
                        roots.push(*source);
                    }
                }
                st.extend(scene.nodes[x].children.iter().copied());
            }
        }
        let set: Vec<NodeId> = self.sched.preorder.iter().copied().filter(|&x| in_set[x]).collect();
        let drawn_v: Vec<NodeId> = set.iter().copied().filter(|&x| drawn[x]).collect();
        let dedup = !(self.lcd && self.fade_raster)
            && drawn_v.iter().all(|&x| {
                let nd = &scene.nodes[x];
                (x == n || nd.time.is_none()) && nd.world.is_none() && !matches!(self.prep.contents[x], Some(PContent::Cloud(_)) | Some(PContent::Fluid(_)))
            });
        let p = Arc::new(MbPlan { set, drawn: drawn_v, dedup });
        self.mb_plans.insert(n, p.clone());
        p
    }

    fn base_matrices(&mut self) -> Arc<Vec<agentfx_ir::Affine>> {
        self.base_m.get_or_insert_with(|| Arc::new(self.scene.nodes.iter().map(|x| agentfx_ir::eval::transform_matrix(&x.transform)).collect())).clone()
    }

    /// Node states of a sample at `ts`: the plan's nodes evaluated, the rest the view's.
    fn mb_states(&mut self, plan: &MbPlan, view: &View, ts: f64) -> Result<Vec<NodeState>> {
        let _p = crate::prof::scope("mblur.eval");
        let base_m = self.base_matrices();
        let locals: Vec<(agentfx_ir::Affine, f64)> = plan.set.iter().map(|&x| schedule::eval_local(self.scene, &self.sched.tracks, &base_m, x, ts)).collect::<Result<_>>()?;
        let mut st = view.states.to_vec();
        schedule::chains_into(&plan.set, &self.sched.parents, &locals, &mut st);
        Ok(st)
    }

    /// Everything a sample's pixels depend on that varies between samples, bit-exact.
    fn mb_key(&self, plan: &MbPlan, n: NodeId, st: &[NodeState], ts: f64) -> Vec<u64> {
        let mut k = Vec::with_capacity(plan.set.len() * 14 + plan.drawn.len());
        for &x in &plan.set {
            let s = &st[x];
            for a in [&s.local, &s.chain] {
                k.extend([a.a, a.b, a.c, a.d, a.e, a.f].map(f64::to_bits));
            }
            k.push(s.opacity.to_bits());
            k.push(s.eff_opacity.to_bits());
        }
        for &x in &plan.drawn {
            k.push(self.info.sig(self.scene, x, ts));
            if x != n {
                let tr = &self.sched.tracks[x];
                for (i, e) in self.scene.nodes[x].effects.iter().enumerate() {
                    k.extend(schedule::effect_params(self.scene, tr, i, e, ts).map(f64::to_bits));
                }
            }
        }
        k
    }

    /// The averaged motion-blur samples of `n`'s inner subtree in screen rect `o`.
    pub(super) fn mblur_pixmap(&mut self, view: &View, f: usize, n: NodeId, o: [i32; 4], mb: &agentfx_ir::MotionBlur, matte: bool) -> Result<Pixmap> {
        // inside a time effect the node is drawn once per time sample and tile/strip, every
        // time with the frame's samples: render its whole ink once and crop
        if self.mb_crop && self.time_depth > 0 {
            let frame = [-64.0, -64.0, self.scene.width as f64 + 128.0, self.scene.height as f64 + 128.0];
            if let Some(c) = self.sub_ink(true, f, n).and_then(|ink| irect(intersect(ink, frame))) {
                let inside = o[0] >= c[0] && o[1] >= c[1] && o[0] + o[2] <= c[0] + c[2] && o[1] + o[3] <= c[1] + c[3];
                if inside && c != o && (c[2] as usize) * (c[3] as usize) <= 4096 * 4096 {
                    let full = self.mblur_render(view, f, n, c, mb, matte)?;
                    let _p = crate::prof::scope("mblur.crop");
                    let mut out = Pixmap::new(o[2] as u16, o[3] as u16);
                    let (src, dst) = (full.data_as_u8_slice(), out.data_as_u8_slice_mut());
                    let rw = o[2] as usize * 4;
                    for y in 0..o[3] as usize {
                        let s0 = (((o[1] - c[1]) as usize + y) * c[2] as usize + (o[0] - c[0]) as usize) * 4;
                        dst[y * rw..y * rw + rw].copy_from_slice(&src[s0..s0 + rw]);
                    }
                    return Ok(out);
                }
            }
        }
        Ok(Arc::unwrap_or_clone(self.mblur_render(view, f, n, o, mb, matte)?))
    }

    fn mblur_render(&mut self, view: &View, f: usize, n: NodeId, o: [i32; 4], mb: &agentfx_ir::MotionBlur, matte: bool) -> Result<Arc<Pixmap>> {
        let scene = self.scene;
        let (w, h) = (o[2] as usize, o[3] as usize);
        let times = schedule::mb_times(scene.fps as f64, f, mb.samples, mb.shutter);
        let _p = crate::prof::scope("mblur");
        let plan = self.mb_plan(n);
        // the frame's cache (samples do not depend on the view's states, only on its
        // content selection of the drawn nodes)
        let ckey = {
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            for &x in &plan.drawn {
                view.content[x].hash(&mut hs);
            }
            (n, o, matte, self.fluid_matte > 0, hs.finish())
        };
        if self.mb_cache.frame != f {
            self.mb_cache.map.clear();
            self.mb_cache.bytes = 0;
            self.mb_cache.frame = f;
        }
        if let Some(px) = self.mb_cache.map.get(&ckey) {
            self.stats.mb_hits += 1;
            return Ok(px.clone());
        }
        crate::prof::add_px("mblur", (w * h) as u64 * times.len() as u64);
        // group bit-identical samples
        let mut groups: Vec<(Vec<NodeState>, f64, u16, Option<Vec<u64>>)> = Vec::new();
        for &ts in &times {
            let st = self.mb_states(&plan, view, ts)?;
            let key = plan.dedup.then(|| self.mb_key(&plan, n, &st, ts));
            if let Some(k) = &key {
                if let Some(g) = groups.iter_mut().find(|g| g.3.as_ref() == Some(k)) {
                    g.2 += 1;
                    continue;
                }
            }
            groups.push((st, ts, 1, key));
        }
        self.stats.mb_samples += times.len();
        self.stats.mb_renders += groups.len();
        let px = if groups.len() == 1 {
            // all samples equal: (k * v + k / 2) / k = v, the raster itself
            let (st, ts, _, _) = &groups[0];
            let v = View { t: *ts, states: st, content: view.content };
            self.render_view(&v, f, o, Some(n), 0, matte)?
        } else {
            let mut acc = vec![0u16; w * h * 4];
            for (st, ts, cnt, _) in &groups {
                let v = View { t: *ts, states: st, content: view.content };
                let p = self.render_view(&v, f, o, Some(n), 0, matte)?;
                let _p = crate::prof::scope("mblur.accum");
                effects::accumulate_weighted(&mut acc, p.data_as_u8_slice(), *cnt);
            }
            let _p = crate::prof::scope("mblur.accum");
            let avg = effects::average_small(&acc, times.len() as u32);
            let mut p = Pixmap::new(w as u16, h as u16);
            p.data_as_u8_slice_mut().copy_from_slice(&avg);
            p
        };
        let px = Arc::new(px);
        let bytes = w * h * 4;
        if bytes <= MB_CACHE_LIMIT / 4 {
            if self.mb_cache.bytes + bytes > MB_CACHE_LIMIT {
                self.mb_cache.map.clear();
                self.mb_cache.bytes = 0;
            }
            self.mb_cache.bytes += bytes;
            self.mb_cache.map.insert(ckey, px.clone());
        }
        Ok(px)
    }

    /// Sample states of a motion-blurred leaf cloud: its chain per sample (region evaluation).
    pub(super) fn mb_chain(&mut self, view: &View, n: NodeId, ts: f64) -> Result<Affine> {
        let plan = self.mb_plan(n);
        let st = self.mb_states(&plan, view, ts)?;
        Ok(k(&st[n].chain))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tr(node: usize, prop: &str, kfs: serde_json::Value) -> serde_json::Value {
        json!({"node": node, "property": prop, "delay": 0.0, "duration": 1.0, "fill": "both", "easing": "linear", "keyframes": kfs})
    }

    fn mv(x: f64, y: f64, r: f64) -> serde_json::Value {
        json!({"transform": [{"translate": [x, y]}, {"rotate": r}]})
    }

    fn rect(x: f64, y: f64, w: f64, h: f64, c: [f64; 4]) -> serde_json::Value {
        json!({"type": "rect", "rect": {"rect": [x, y, w, h], "radius": [3.0, 3.0, 3.0, 3.0]}, "color": c})
    }

    /// A small "shatter" scene like the story's undo: matte-revealed shards with per-layer
    /// motion blur (moving, resting, fading), a static blurred layer, a blurred group with a
    /// blurred child, a blurred cloud, under an animated ancestor with a riso look; `time`:
    /// an optional time effect on the shards' group.
    fn scene(time: Option<serde_json::Value>) -> Scene {
        let ink = [0.1, 0.1, 0.12, 1.0];
        let mut nodes = vec![
            json!({"name": "root", "children": [1], "content": rect(0.0, 0.0, 320.0, 180.0, [0.95, 0.93, 0.9, 1.0])}),
            json!({"name": "scene", "children": [2, 9, 10, 13], "origin": [160.0, 90.0],
                   "effects": [{"type": "riso", "inks": [[1.0, 0.3, 0.1, 1.0], [0.17, 0.35, 1.0, 1.0]], "seed": 5, "misreg": 2.0, "drift": 1.0, "drift_speed": 0.5, "grain": 0.4, "density": 0.25}]}),
            json!({"name": "sun", "children": [3], "origin": [160.0, 90.0]}),
            json!({"name": "disc", "children": [4, 6, 8], "origin": [160.0, 90.0]}),
            json!({"name": "shard a", "children": [5], "origin": [120.0, 80.0], "content": rect(90.0, 50.0, 70.0, 60.0, ink),
                   "motion_blur": {"samples": 12, "shutter": 0.5}, "effects": [{"type": "matte", "source": 5, "mode": "alpha"}]}),
            json!({"name": "reveal a", "hidden": true, "content": {"type": "path", "d": "M95 55 L150 60 L130 105 L92 100 Z", "fill": {"solid": [1, 1, 1, 1]}}}),
            json!({"name": "shard b", "children": [7], "origin": [190.0, 90.0], "content": rect(160.0, 60.0, 60.0, 60.0, [0.9, 0.3, 0.1, 1.0]),
                   "motion_blur": {"samples": 12, "shutter": 0.5}, "effects": [{"type": "matte", "source": 7, "mode": "alpha"}]}),
            json!({"name": "reveal b", "hidden": true, "content": {"type": "path", "d": "M165 65 L215 70 L205 115 L170 110 Z", "fill": {"solid": [1, 1, 1, 1]}}}),
            json!({"name": "static blurred", "content": rect(20.0, 120.0, 40.0, 30.0, ink), "motion_blur": {"samples": 8, "shutter": 0.5}}),
            json!({"name": "face", "children": [11, 12], "origin": [250.0, 140.0], "motion_blur": {"samples": 6, "shutter": 1.0}}),
            json!({"name": "stars", "motion_blur": {"samples": 4, "shutter": 0.5},
                   "content": {"type": "cloud", "count": 300, "seed": 3, "radius": 1.5, "palette": [[1.0, 0.9, 0.6, 1.0]],
                               "forms": [{"shape": {"type": "field", "rect": [0, 0, 320, 60], "flow": [40, 0]}, "drift": {"amp": 4, "scale": 60}}]}}),
            json!({"name": "eye l", "origin": [240.0, 135.0], "content": rect(235.0, 130.0, 10.0, 10.0, ink)}),
            json!({"name": "eye r", "origin": [262.0, 135.0], "content": rect(257.0, 130.0, 10.0, 10.0, ink), "motion_blur": {"samples": 5, "shutter": 0.5}}),
            json!({"name": "overlay", "content": rect(0.0, 170.0, 320.0, 10.0, [0.2, 0.2, 0.2, 0.5])}),
        ];
        if let Some(t) = time {
            nodes[2]["time"] = t;
        }
        let tracks = vec![
            tr(1, "transform", json!([{"offset": 0.0, "value": mv(0.0, 0.0, 0.0)}, {"offset": 0.4, "value": mv(6.0, -4.0, 2.0)}, {"offset": 1.0, "value": mv(6.0, -4.0, 2.0)}])),
            tr(4, "transform", json!([{"offset": 0.0, "value": mv(-60.0, 40.0, -30.0)}, {"offset": 0.3, "value": mv(-60.0, 40.0, -30.0)}, {"offset": 0.7, "value": mv(0.0, 0.0, 0.0)}, {"offset": 1.0, "value": mv(0.0, 0.0, 0.0)}])),
            tr(4, "opacity", json!([{"offset": 0.0, "value": {"number": 0.0}}, {"offset": 0.1, "value": {"number": 1.0}}, {"offset": 1.0, "value": {"number": 1.0}}])),
            tr(6, "transform", json!([{"offset": 0.0, "value": mv(70.0, -50.0, 40.0)}, {"offset": 0.5, "value": mv(0.0, 0.0, 0.0)}, {"offset": 1.0, "value": mv(10.0, 5.0, 0.0)}])),
            tr(9, "transform", json!([{"offset": 0.0, "value": mv(-120.0, 0.0, 0.0)}, {"offset": 0.6, "value": mv(0.0, 0.0, 360.0)}, {"offset": 1.0, "value": mv(0.0, 0.0, 360.0)}])),
            tr(12, "transform", json!([{"offset": 0.0, "value": mv(0.0, 0.0, 0.0)}, {"offset": 0.5, "value": mv(0.0, 8.0, 0.0)}, {"offset": 1.0, "value": mv(0.0, 0.0, 0.0)}])),
            tr(2, "time.span", json!([{"offset": 0.0, "value": {"number": 0.0}}, {"offset": 0.2, "value": {"number": 0.3}}, {"offset": 1.0, "value": {"number": 0.3}}])),
        ];
        let s = json!({"version": agentfx_ir::IR_VERSION, "width": 320, "height": 180, "fps": 30, "frames": 30, "nodes": nodes, "tracks": tracks});
        serde_json::from_value(s).unwrap()
    }

    /// Frames rendered through the motion-blur path `mode` ("v1", "exact", "crop"): the full
    /// frame, then every damage rect of the frame (partial redraws).
    fn render(scene: &Scene, mode: &str, frames: &[usize]) -> (Vec<Vec<u8>>, PainterStats) {
        let prep = Prepared::new(scene, Path::new("."), None, None).unwrap();
        let info = Info::new(scene, &prep);
        let sched = schedule::build(scene, &info).unwrap();
        let mut p = Painter::new(scene, &prep, &sched, &info, 0);
        p.mblur_v1 = mode == "v1";
        p.mb_crop = mode == "crop";
        let mut out = vec![];
        for &f in frames {
            out.push(p.render_rect(f, [0, 0, scene.width as i32, scene.height as i32]).unwrap().data_as_u8_slice().to_vec());
            for &r in &sched.damage[f] {
                out.push(p.render_rect(f, r).unwrap().data_as_u8_slice().to_vec());
            }
        }
        (out, p.stats)
    }

    fn max_diff(a: &[Vec<u8>], b: &[Vec<u8>]) -> (u8, usize) {
        assert_eq!(a.len(), b.len());
        let mut m = 0u8;
        let mut px = 0;
        for (x, y) in a.iter().zip(b) {
            assert_eq!(x.len(), y.len());
            for (p, q) in x.chunks_exact(4).zip(y.chunks_exact(4)) {
                let d = p.iter().zip(q).map(|(u, v)| u.abs_diff(*v)).max().unwrap();
                m = m.max(d);
                px += (d > 0) as usize;
            }
        }
        (m, px)
    }

    const FRAMES: [usize; 8] = [0, 3, 6, 9, 12, 17, 22, 29];

    #[test]
    fn region_eval_and_sample_dedup_equal_v1() {
        let s = scene(None);
        let (a, sa) = render(&s, "v1", &FRAMES);
        let (b, sb) = render(&s, "exact", &FRAMES);
        assert!(a == b, "max diff {:?}", max_diff(&a, &b));
        // the static layer and the resting shards (once their ancestor rests) render once
        assert!(sb.mb_renders + 40 < sb.mb_samples, "{sb:?}");
        assert_eq!(sa.mb_samples, 0);
    }

    #[test]
    fn exact_equals_v1_inside_time_effects() {
        for t in [
            json!({"type": "slitscan", "axis": "y", "span": 0.0, "band": 4.0}),
            json!({"type": "slitscan", "axis": "x", "span": 0.0, "band": 2.0, "range": [320.0, 0.0]}),
            json!({"type": "echo", "count": 3, "span": 0.0, "decay": 0.6}),
        ] {
            let s = scene(Some(t.clone()));
            let (a, _) = render(&s, "v1", &FRAMES);
            let (b, sb) = render(&s, "exact", &FRAMES);
            assert!(a == b, "{t}: max diff {:?}", max_diff(&a, &b));
            assert!(sb.mb_samples > 0);
        }
    }

    #[test]
    fn crop_inside_time_effects_within_tolerance() {
        // inside a time effect the blurred layers are rendered once over their whole ink and
        // cropped per strip: the same raster at another integer origin (float rounding of the
        // path coordinates may move an edge pixel by a level or two)
        for t in [json!({"type": "slitscan", "axis": "y", "span": 0.0, "band": 4.0}), json!({"type": "echo", "count": 3, "span": 0.0, "decay": 0.6})] {
            let s = scene(Some(t.clone()));
            let (a, _) = render(&s, "v1", &FRAMES);
            let (b, sb) = render(&s, "crop", &FRAMES);
            let (m, px) = max_diff(&a, &b);
            let total: usize = a.iter().map(|x| x.len() / 4).sum();
            assert!(m <= 4 && px * 1000 <= total, "{t}: max {m}, {px} of {total} px differ");
            assert!(sb.mb_hits > 0, "{sb:?}");
        }
    }

    #[test]
    fn example_fixtures_equal_v1() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ir/direct");
        for name in ["showcase-16x9.ir.json", "showcase-9x16.ir.json"] {
            let scene = Scene::load(&dir.join(name)).unwrap();
            let prep = Prepared::new(&scene, &dir, None, None).unwrap();
            let info = Info::new(&scene, &prep);
            let sched = schedule::build(&scene, &info).unwrap();
            // the comet (12 samples) flies 5.2 .. 6.5 s
            let frames: Vec<usize> = (150..200).step_by(5).collect();
            let mut out = [vec![], vec![]];
            let mut samples = 0;
            for (i, v1) in [true, false].into_iter().enumerate() {
                let mut p = Painter::new(&scene, &prep, &sched, &info, 0);
                p.mblur_v1 = v1;
                for &f in &frames {
                    let full = [0, 0, scene.width as i32, scene.height as i32];
                    for &r in std::iter::once(&full).chain(&sched.damage[f]) {
                        out[i].push(p.render_rect(f, r).unwrap().data_as_u8_slice().to_vec());
                    }
                }
                samples += p.stats.mb_samples;
            }
            assert!(samples > 0, "{name}: no motion blur rendered");
            assert!(out[0] == out[1], "{name}: max diff {:?}", max_diff(&out[0], &out[1]));
        }
    }

    #[test]
    fn region_states_are_the_full_evaluation() {
        let s = scene(Some(json!({"type": "slitscan", "axis": "y", "span": 0.0, "band": 4.0})));
        let prep = Prepared::new(&s, Path::new("."), None, None).unwrap();
        let info = Info::new(&s, &prep);
        let sched = schedule::build(&s, &info).unwrap();
        let mut p = Painter::new(&s, &prep, &sched, &info, 0);
        let base_m: Vec<agentfx_ir::Affine> = s.nodes.iter().map(|x| agentfx_ir::eval::transform_matrix(&x.transform)).collect();
        for n in [4usize, 6, 8, 9, 10, 12] {
            let plan = p.mb_plan(n);
            // the shard's matte source is evaluated, the unrelated overlay is not
            assert!(plan.set.contains(&n) && plan.set.contains(&0));
            if n == 4 {
                assert!(plan.set.contains(&5) && !plan.set.contains(&13));
            }
            for f in [0usize, 7, 15, 29] {
                let view = View { t: f as f64 / 30.0, states: &sched.states[f], content: &sched.content[f] };
                for ts in schedule::mb_times(30.0, f, 7, 0.5) {
                    let full = schedule::chains(&sched.preorder, &sched.parents, &schedule::eval_locals(&s, &sched.tracks, &base_m, ts).unwrap());
                    let st = p.mb_states(&plan, &view, ts).unwrap();
                    for &x in &plan.set {
                        assert_eq!(format!("{:?}", st[x]), format!("{:?}", full[x]), "node {x} of plan {n} at {ts}");
                    }
                }
            }
        }
    }

    #[test]
    fn weighted_accumulate_is_repeated_accumulate() {
        let buf: Vec<u8> = (0..4096u32).map(|i| (i * 2654435761u32 >> 24) as u8).collect();
        let mut a = vec![0u16; buf.len()];
        let mut b = vec![0u16; buf.len()];
        for _ in 0..7 {
            effects::accumulate(&mut a, &buf);
        }
        effects::accumulate_weighted(&mut b, &buf, 7);
        assert_eq!(a, b);
        assert_eq!(effects::average(&a, 12), effects::average_small(&b, 12));
        // all samples equal: the average is the sample itself (render-once shortcut)
        let mut c = vec![0u16; buf.len()];
        effects::accumulate_weighted(&mut c, &buf, 12);
        assert_eq!(effects::average(&c, 12), buf);
    }
}
