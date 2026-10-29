//! M2 HTML planner ("probe planner"): any HTML-mode scene, including recorded
//! Remotion/HyperFrames scenes (union DOM, `display` ranges, individual transform
//! properties, discrete holds of any property incl. custom properties and `var()`,
//! z-index, transformed ancestors), compiled natively into IR without Chrome.
//!
//! 1. **Tracks** are split into composite tracks (`transform`, `translate`, `rotate`,
//!    `scale`, `opacity`: these become IR node tracks) and state tracks (everything
//!    else: discrete holds and interpolated paint/layout values).
//! 2. **Change frames**: every frame where some state track's value differs from the
//!    previous frame (plus frame 0).
//! 3. **Probe**: at each change frame the state values are applied as inline styles,
//!    composite properties of layers are neutralized, Blitz restyles + re-lays out and
//!    paints the whole document once (no culling). Every display-list item is attributed
//!    to the element that issued it (patched painter observer).
//! 4. **Plan** (no pixels): layers = elements with composite tracks (+ the root); groups =
//!    elements whose effects (clip, opacity, filter, blend) enclose a layer (their effect
//!    becomes an IR node); regions = lowest common ancestor of the elements whose own
//!    paint changed at a change frame (damage stays local). Paint order comes from the
//!    probes themselves (z-index and stacking contexts as Blitz paints them), merged over
//!    all probes; units = runs of elements in paint order with the same region, split by
//!    nested IR nodes.
//! 5. **IR**: layers become node chains (translate -> rotate -> scale -> transform, each
//!    with its own easing; values conjugated into scene space by the static ancestor
//!    transform), units become `Switch` contents with one display list per distinct
//!    signature (extracted from the probe paints).
use crate::blitzfe::BlitzScene;
use crate::css;
use crate::m2::state_at;
use crate::model::{Compiled, ElemId, Stage};
use crate::record::{self, Resources};
use anyhow::{Result, bail};
use agentfx_ir::eval::eval_track;
use agentfx_ir::{Content, Draw, Node, RRect, Scene, Shape, TransformFn, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::Instant;

const W: f64 = 1920.0;
const H: f64 = 1080.0;
const NONE: usize = usize::MAX;

/// One probe: the whole document painted at a change frame (neutral composites).
struct Probe {
    frame: usize,
    items: Vec<Draw>,
    /// owning element per item (the canvas background is attributed to the root, 0)
    owner: Vec<ElemId>,
    /// per element: hash of its own items (0 = paints nothing)
    hash: Vec<u64>,
    /// placement of painted elements: (parent transform, box position)
    place: HashMap<ElemId, (kurbo::Affine, kurbo::Vec2)>,
    /// transform origin (px, element space) of painted layers
    origin: HashMap<ElemId, [f64; 2]>,
}

fn hash_f(h: &mut impl Hasher, v: &[f64]) {
    for x in v {
        x.to_bits().hash(h);
    }
}
fn hash_shape(h: &mut impl Hasher, s: &Shape) {
    match s {
        Shape::Rect(r) => {
            0u8.hash(h);
            hash_f(h, r)
        }
        Shape::RoundedRect { rect, radii } => {
            1u8.hash(h);
            hash_f(h, rect);
            hash_f(h, radii)
        }
        Shape::Path(p) => {
            2u8.hash(h);
            p.hash(h)
        }
    }
}
fn hash_brush(h: &mut impl Hasher, b: &agentfx_ir::Brush) {
    // brushes are small; their JSON form is a stable, complete key
    serde_json::to_string(b).unwrap_or_default().hash(h);
}
/// Content hash of one display-list item.
fn hash_draw(h: &mut impl Hasher, d: &Draw) {
    match d {
        Draw::Fill { shape, brush, transform, brush_transform, even_odd } => {
            10u8.hash(h);
            hash_shape(h, shape);
            hash_brush(h, brush);
            hash_f(h, transform);
            if let Some(b) = brush_transform {
                hash_f(h, b);
            }
            even_odd.hash(h);
        }
        Draw::Stroke { shape, brush, style, transform, brush_transform } => {
            11u8.hash(h);
            hash_shape(h, shape);
            hash_brush(h, brush);
            serde_json::to_string(style).unwrap_or_default().hash(h);
            hash_f(h, transform);
            if let Some(b) = brush_transform {
                hash_f(h, b);
            }
        }
        Draw::Glyphs { font, size, coords, embolden, hint, brush, transform, glyph_transform, glyphs, stroke } => {
            12u8.hash(h);
            font.hash(h);
            size.to_bits().hash(h);
            coords.hash(h);
            if let Some(e) = embolden {
                hash_f(h, e);
            }
            hint.hash(h);
            hash_brush(h, brush);
            hash_f(h, transform);
            if let Some(g) = glyph_transform {
                hash_f(h, g);
            }
            for (id, x, y) in glyphs {
                id.hash(h);
                x.to_bits().hash(h);
                y.to_bits().hash(h);
            }
            if let Some(s) = stroke {
                serde_json::to_string(s).unwrap_or_default().hash(h);
            }
        }
        Draw::BoxShadow { rect, radius, std_dev, color, transform } => {
            13u8.hash(h);
            hash_f(h, rect);
            hash_f(h, &[*radius, *std_dev]);
            for c in color {
                c.to_bits().hash(h);
            }
            hash_f(h, transform);
        }
        Draw::Push { clip, transform, alpha, blend, filter, even_odd } => {
            14u8.hash(h);
            if let Some(c) = clip {
                hash_shape(h, c);
            }
            hash_f(h, transform);
            alpha.to_bits().hash(h);
            (*blend as u8).hash(h);
            serde_json::to_string(filter).unwrap_or_default().hash(h);
            even_odd.hash(h);
        }
        Draw::Pop => 15u8.hash(h),
    }
}

fn is_draw(d: &Draw) -> bool {
    !matches!(d, Draw::Push { .. } | Draw::Pop)
}

/// Remove push/pop pairs that enclose nothing.
fn prune(items: Vec<Draw>) -> Vec<Draw> {
    let mut out: Vec<Draw> = Vec::with_capacity(items.len());
    // stack of (index in out of the push, number of draws since)
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for d in items {
        match d {
            Draw::Push { .. } => {
                stack.push((out.len(), 0));
                out.push(d);
            }
            Draw::Pop => {
                let (at, n) = stack.pop().expect("balanced push/pop");
                if n == 0 {
                    out.truncate(at);
                } else {
                    out.push(Draw::Pop);
                    if let Some(top) = stack.last_mut() {
                        top.1 += n;
                    }
                }
            }
            d => {
                out.push(d);
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
            }
        }
    }
    out
}

/// Output of the planner: the IR scene plus stats.
pub struct Plan {
    pub scene: Scene,
    pub stats: serde_json::Value,
}

fn lin_ok_rotate(a: &[f64; 6]) -> bool {
    // similarity (uniform scale + rotation), no reflection
    (a[0] - a[3]).abs() < 1e-9 && (a[1] + a[2]).abs() < 1e-9
}
fn lin_axis(a: &[f64; 6]) -> bool {
    a[1].abs() < 1e-9 && a[2].abs() < 1e-9
}

/// Map a CSS transform function (local space of an element whose parent transform has
/// linear part `a`) into scene space: Lin · X · Lin⁻¹.
fn conjugate(f: &TransformFn, a: &[f64; 6]) -> Result<TransformFn> {
    Ok(match *f {
        TransformFn::Translate(x, y) => TransformFn::Translate(a[0] * x + a[2] * y, a[1] * x + a[3] * y),
        TransformFn::Rotate(d) => {
            if !lin_ok_rotate(a) {
                bail!("rotate under a non-uniformly scaled/skewed ancestor transform");
            }
            TransformFn::Rotate(d)
        }
        TransformFn::Scale(sx, sy) => {
            if !(lin_axis(a) || (sx - sy).abs() < 1e-12) {
                bail!("non-uniform scale under a rotated ancestor transform");
            }
            TransformFn::Scale(sx, sy)
        }
    })
}

fn rrect_of(shape: &Shape, m: &[f64; 6]) -> Result<RRect> {
    if !lin_axis(m) || m[0] <= 0.0 || m[3] <= 0.0 {
        bail!("group clip under a rotated/flipped transform");
    }
    let (r, radii) = match shape {
        Shape::Rect(r) => (*r, [0.0; 4]),
        Shape::RoundedRect { rect, radii } => (*rect, *radii),
        Shape::Path(d) => path_rrect(d).ok_or_else(|| anyhow::anyhow!("non-rectangular clip path around a layer: {}", &d[..d.len().min(400)]))?,
    };
    let x0 = m[0] * r[0] + m[4];
    let y0 = m[3] * r[1] + m[5];
    let x1 = m[0] * r[2] + m[4];
    let y1 = m[3] * r[3] + m[5];
    let k = m[0].min(m[3]);
    Ok(RRect { rect: [x0, y0, x1 - x0, y1 - y0], radius: radii.map(|v| v * k) })
}

/// Recognize an axis-aligned (rounded) rectangle given as path data (Blitz emits box
/// clips as paths): radii from the corner gaps, verified by area.
fn path_rrect(d: &str) -> Option<([f64; 4], [f64; 4])> {
    use kurbo::Shape as _;
    let mut p = kurbo::BezPath::from_svg(d).ok()?;
    if !matches!(p.elements().last(), Some(kurbo::PathEl::ClosePath)) {
        p.close_path();
    }
    let b = p.bounding_box();
    if b.width() <= 0.0 || b.height() <= 0.0 {
        return None;
    }
    let mut pts = Vec::new();
    for el in p.elements() {
        match el {
            kurbo::PathEl::MoveTo(a) | kurbo::PathEl::LineTo(a) => pts.push(*a),
            kurbo::PathEl::QuadTo(_, a) | kurbo::PathEl::CurveTo(_, _, a) => pts.push(*a),
            kurbo::PathEl::ClosePath => {}
        }
    }
    let e = 1e-3;
    let gap = |on_edge: &dyn Fn(&kurbo::Point) -> bool, dist: &dyn Fn(&kurbo::Point) -> f64| -> f64 {
        pts.iter().filter(|q| on_edge(q)).map(|q| dist(q)).fold(f64::INFINITY, f64::min)
    };
    let tl = gap(&|q| (q.y - b.y0).abs() < e, &|q| q.x - b.x0);
    let tr = gap(&|q| (q.y - b.y0).abs() < e, &|q| b.x1 - q.x);
    let br = gap(&|q| (q.y - b.y1).abs() < e, &|q| b.x1 - q.x);
    let bl = gap(&|q| (q.y - b.y1).abs() < e, &|q| q.x - b.x0);
    let radii = [tl, tr, br, bl].map(|r| if r.is_finite() && r > e { r } else { 0.0 });
    let rr = kurbo::RoundedRect::from_rect(b, kurbo::RoundedRectRadii::new(radii[0], radii[1], radii[2], radii[3]));
    let (a1, a2) = (p.area().abs(), rr.area().abs());
    if (a1 - a2).abs() > 0.5 + 1e-3 * a2 {
        return None;
    }
    Some(([b.x0, b.y0, b.x1, b.y1], radii))
}

pub fn build(scene_dir: &Path, out_dir: &Path) -> Result<(Plan, Resources)> {
    let t0 = Instant::now();
    let mut timing: BTreeMap<&str, f64> = BTreeMap::new();
    let compiled: Compiled = css::compile_scene(scene_dir).map_err(|errs| anyhow::anyhow!("unsupported CSS in {}:\n  - {}", scene_dir.display(), errs.join("\n  - ")))?;
    if compiled.fps.fract() != 0.0 {
        bail!("fps {} not an integer", compiled.fps);
    }
    let fps = compiled.fps;
    let frames = (compiled.duration * fps).round() as usize;
    timing.insert("css_s", t0.elapsed().as_secs_f64());
    let t = Instant::now();
    let mut bz = BlitzScene::load(scene_dir, &compiled)?;
    let mut res = Resources::new(out_dir);
    bz.image_sources(&mut res);
    timing.insert("load_s", t.elapsed().as_secs_f64());
    let n_el = compiled.tags.len();
    let parent = &compiled.parent;
    let ir_tr: Vec<agentfx_ir::Track> = compiled.tracks.iter().map(|t| t.to_ir(0)).collect();

    // ---------------------------------------------------------------- tracks
    let mut layers: BTreeSet<ElemId> = BTreeSet::new();
    layers.insert(0);
    let mut comp_by_elem: BTreeMap<ElemId, Vec<usize>> = BTreeMap::new();
    let mut state_tracks: Vec<usize> = Vec::new();
    for (i, tr) in compiled.tracks.iter().enumerate() {
        if tr.stage == Stage::Composite {
            if tr.elem == 0 {
                bail!("composite track on the root element");
            }
            layers.insert(tr.elem);
            comp_by_elem.entry(tr.elem).or_default().push(i);
        } else {
            state_tracks.push(i);
        }
    }

    // (review M3 #3) a state track (held value, e.g. a % translate) that outranks an active
    // composite track of the same property on the same element would win in a browser,
    // but the probes neutralize the property: not modeled, fail loudly
    for &si in &state_tracks {
        let st = &compiled.tracks[si];
        let Some(cl) = comp_by_elem.get(&st.elem) else { continue };
        let rivals: Vec<usize> = cl.iter().copied().filter(|&c| compiled.tracks[c].property == st.property && compiled.tracks[c].order < st.order).collect();
        if rivals.is_empty() { continue; }
        let sir = st.to_ir(0);
        let rir: Vec<_> = rivals.iter().map(|&c| compiled.tracks[c].to_ir(0)).collect();
        if let Some(f) = (0..frames).find(|&f| {
            let t = f as f64 / fps;
            eval_track(&sir, t).is_some() && rir.iter().any(|r| eval_track(r, t).is_some())
        }) {
            bail!("unsupported scene features:\n  - <{}> #{}: held `{}` animation ({}) overrides an interpolated one at frame {f} (not modeled)", compiled.tags[st.elem], st.elem, st.property, st.source);
        }
    }

    // static composite values (shown where no animation of that property is active)
    bz.resolve();
    let bases: HashMap<ElemId, ([f64; 2], f64, [f64; 2], f64, bool)> = layers.iter().filter(|&&l| l != 0).filter_map(|&l| bz.composite_base(l).map(|b| (l, b))).collect();

    // ---------------------------------------------------------------- change frames
    let t = Instant::now();
    let sample = |i: usize, f: usize| -> String {
        let tr = &compiled.tracks[i];
        let tt = f as f64 / fps;
        if tr.property == "text" {
            return match eval_track(&ir_tr[i], tt) {
                Some(agentfx_ir::Sampled::Number(v)) => format!("{}", (v + 1e-9).floor() as i64),
                _ => "-".into(),
            };
        }
        match agentfx_ir::eval::eval_segment(&ir_tr[i], tt) {
            None => "\u{0}none".into(),
            Some((k, u)) => match (&tr.keyframes[k].value, &tr.keyframes[k + 1].value) {
                (Value::Discrete(a), Value::Discrete(b)) => {
                    if a == b || u <= 0.0 {
                        a.clone()
                    } else if u >= 1.0 {
                        b.clone()
                    } else {
                        format!("{k}:{u:.6}")
                    }
                }
                _ => format!("{k}:{u:.6}"),
            },
        }
    };
    let mut cf: BTreeSet<usize> = BTreeSet::new();
    cf.insert(0);
    for &i in &state_tracks {
        let mut prev = sample(i, 0);
        for f in 1..frames {
            let v = sample(i, f);
            if v != prev {
                cf.insert(f);
                prev = v;
            }
        }
    }
    let cf: Vec<usize> = cf.into_iter().collect();
    timing.insert("change_frames_s", t.elapsed().as_secs_f64());

    // ---------------------------------------------------------------- probes
    let t = Instant::now();
    let neutral: Vec<(ElemId, String, String)> = comp_by_elem
        .iter()
        .flat_map(|(&e, v)| {
            let mut props: Vec<&str> = v.iter().map(|&i| compiled.tracks[i].property.as_str()).collect();
            props.sort();
            props.dedup();
            // identity values that keep the stacking context the animation creates in a
            // browser (`none` would drop it and hoist z-indexed descendants out of the layer)
            let mut v: Vec<(ElemId, String, String)> = props
                .into_iter()
                .map(move |p| {
                    let neutral = match p {
                        "opacity" => "1",
                        "translate" => "0px 0px",
                        "rotate" => "0deg",
                        "scale" => "1 1",
                        _ => "translate(0px, 0px)",
                    };
                    (e, p.to_string(), neutral.to_string())
                })
                .collect();
            v.push((e, "isolation".to_string(), "isolate".to_string()));
            v
        })
        .collect();
    let mut probes: Vec<Probe> = Vec::with_capacity(cf.len());
    let mut n_items = 0usize;
    for &f in &cf {
        let (mut props, texts) = state_at(&compiled, &ir_tr, f as f64 / fps, false)?;
        // neutral composite values win over state values of the same property
        props.retain(|(e, p, _)| !neutral.iter().any(|(e2, p2, _)| e2 == e && p2 == p));
        props.extend(neutral.iter().cloned());
        bz.set_props(&props, true)?;
        for (e, s) in &texts {
            bz.set_text(*e, s);
        }
        bz.resolve();
        let (items, mut owner, place) = bz.paint_probe(&mut res)?;
        for o in owner.iter_mut() {
            if *o == NONE {
                *o = 0;
            }
        }
        let mut hs: Vec<std::collections::hash_map::DefaultHasher> = Vec::new();
        let mut hash = vec![0u64; n_el];
        {
            let mut acc: HashMap<ElemId, std::collections::hash_map::DefaultHasher> = HashMap::new();
            for (d, &o) in items.iter().zip(&owner) {
                hash_draw(acc.entry(o).or_default(), d);
            }
            for (e, h) in acc {
                hash[e] = h.finish() | 1;
            }
            hs.clear();
        }
        let mut origin = HashMap::new();
        for &l in &layers {
            if l != 0 && place.contains_key(&l) {
                origin.insert(l, bz.geom_elem(l).transform_origin);
            }
        }
        n_items += items.len();
        probes.push(Probe { frame: f, items, owner, hash, place, origin });
    }
    bz.set_props(&[], false)?;
    timing.insert("probe_s", t.elapsed().as_secs_f64());

    // ---------------------------------------------------------------- groups
    let t = Instant::now();
    // layers that ever paint
    let has_layer_below: Vec<bool> = {
        let mut v = vec![false; n_el];
        for &l in &layers {
            let mut x = parent[l];
            while let Some(p) = x {
                v[p] = true;
                x = parent[p];
            }
        }
        v
    };
    // groups: elements (not layers) that push an effect and contain a layer
    let mut groups: BTreeSet<ElemId> = BTreeSet::new();
    let mut layer_push_errors: BTreeSet<ElemId> = BTreeSet::new();
    for p in &probes {
        for (d, &o) in p.items.iter().zip(&p.owner) {
            if matches!(d, Draw::Push { .. }) && has_layer_below[o] && !layers.contains(&o) {
                groups.insert(o);
            }
            // (review M3 #1) a layer's own clip/opacity/filter/blend push would not
            // enclose a nested layer (it stays in the layer's units): not modeled
            // (isolation-only pushes, e.g. the probes' own `isolation: isolate`, are harmless)
            // nor are clips covering the whole viewport (full-screen `overflow: hidden` scene
            // containers): they cut nothing that would be visible
            let covers = |c: &Shape| {
                let r = match c {
                    Shape::Rect(r) | Shape::RoundedRect { rect: r, .. } => kurbo::Rect::new(r[0], r[1], r[2], r[3]),
                    Shape::Path(d) => match kurbo::BezPath::from_svg(d) { Ok(p) => kurbo::Shape::bounding_box(&p), Err(_) => return false },
                };
                let rounded = matches!(c, Shape::RoundedRect { radii, .. } if radii.iter().any(|&x| x > 0.0));
                !rounded && r.x0 <= 0.0 && r.y0 <= 0.0 && r.x1 >= W && r.y1 >= H
            };
            let effect = matches!(d, Draw::Push { clip, alpha, blend, filter, transform, .. }
                if clip.as_ref().is_some_and(|c| !(covers(c) && *transform == [1.0, 0.0, 0.0, 1.0, 0.0, 0.0])) || *alpha < 1.0 || *blend != agentfx_ir::BlendMode::Normal || filter.is_some());
            if effect && has_layer_below[o] && layers.contains(&o) && o != 0 {
                if agentfx_ir::env::var("AGENTFX_PLAN_DEBUG").is_ok() && !layer_push_errors.contains(&o) { eprintln!("layer push #{o} frame {}: {d:?}", p.frame); }
                layer_push_errors.insert(o);
            }
        }
    }
    if !layer_push_errors.is_empty() {
        let list: Vec<String> = layer_push_errors.iter().map(|&o| format!("<{}> #{o}", compiled.tags[o])).collect();
        bail!("unsupported scene features:\n  - animated element with its own clip/opacity/filter/blend containing another animated element (not modeled): {}", list.join(", "));
    }
    // A group's own draws before its first effect push (outline, outset box-shadow) are
    // outside its clip/opacity: they belong to the parent context, as pseudo-element
    // `n_el + G` (same parent as G).
    let n_el0 = n_el;
    for p in probes.iter_mut() {
        let mut pushed: HashSet<ElemId> = HashSet::new();
        for (d, o) in p.items.iter().zip(p.owner.iter_mut()) {
            if groups.contains(o) {
                if matches!(d, Draw::Push { .. }) {
                    pushed.insert(*o);
                } else if !pushed.contains(o) {
                    *o += n_el0;
                }
            }
        }
        let mut acc: HashMap<ElemId, std::collections::hash_map::DefaultHasher> = HashMap::new();
        for (d, &o) in p.items.iter().zip(&p.owner) {
            hash_draw(acc.entry(o).or_default(), d);
        }
        p.hash = vec![0u64; 2 * n_el0];
        for (e, h) in acc {
            p.hash[e] = h.finish() | 1;
        }
    }
    let n_el = 2 * n_el0;
    let parent: Vec<Option<ElemId>> = (0..n_el).map(|x| if x < n_el0 { parent[x] } else { parent[x - n_el0] }).collect();
    let parent = &parent;
    let tags: Vec<String> = (0..n_el).map(|x| compiled.tags[x % n_el0].clone()).collect();

    // ---------------------------------------------------------------- tree helpers
    let mut children: Vec<Vec<ElemId>> = vec![vec![]; n_el];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            children[*p].push(i);
        }
    }
    let depth = |mut e: ElemId| {
        let mut d = 0;
        while let Some(p) = parent[e] {
            d += 1;
            e = p;
        }
        d
    };
    let _is_anc = |a: ElemId, mut e: ElemId| -> bool {
        while let Some(p) = parent[e] {
            if p == a {
                return true;
            }
            e = p;
        }
        false
    };
    let lca = |list: &[ElemId]| -> ElemId {
        let mut it = list.iter();
        let mut a = *it.next().unwrap();
        for &b0 in it {
            let mut b = b0;
            let (mut da, mut db) = (depth(a), depth(b));
            while da > db {
                a = parent[a].unwrap();
                da -= 1;
            }
            while db > da {
                b = parent[b].unwrap();
                db -= 1;
            }
            while a != b {
                a = parent[a].unwrap();
                b = parent[b].unwrap();
            }
        }
        a
    };
    let irset: BTreeSet<ElemId> = layers.union(&groups).copied().collect();
    // owning IR element of every element
    let mut oir = vec![0usize; n_el];
    for e in 0..n_el {
        let mut x = e;
        loop {
            if irset.contains(&x) {
                break;
            }
            match parent[x] {
                Some(p) => x = p,
                None => {
                    x = 0;
                    break;
                }
            }
        }
        oir[e] = x;
    }
    let ir_parent = |x: ElemId| -> Option<ElemId> { parent[x].map(|p| oir[p]) };

    // ---------------------------------------------------------------- regions
    let mut regions: BTreeSet<ElemId> = BTreeSet::new();
    for w in probes.windows(2) {
        let mut by_p: BTreeMap<ElemId, Vec<ElemId>> = BTreeMap::new();
        for e in 0..n_el {
            if w[0].hash[e] != w[1].hash[e] {
                by_p.entry(oir[e]).or_default().push(e);
            }
        }
        for (p, list) in by_p {
            let r = lca(&list);
            if r != p && !irset.contains(&r) {
                regions.insert(r);
            }
        }
    }
    let region_of = |e: ElemId| -> Option<ElemId> {
        let p = oir[e];
        let mut x = e;
        loop {
            if x == p {
                return None;
            }
            if regions.contains(&x) {
                return Some(x);
            }
            x = parent[x].unwrap();
        }
    };

    // ---------------------------------------------------------------- paint order
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
    enum Tok {
        Elem(ElemId),
        Child(ElemId),
    }
    // per IR element: token sequences per probe
    let mut seqs: BTreeMap<ElemId, Vec<Vec<Tok>>> = BTreeMap::new();
    for p in &probes {
        let mut cur: BTreeMap<ElemId, Vec<Tok>> = BTreeMap::new();
        let mut push = |cur: &mut BTreeMap<ElemId, Vec<Tok>>, at: ElemId, tok: Tok| {
            let v = cur.entry(at).or_default();
            if v.last() != Some(&tok) {
                v.push(tok);
            }
        };
        for (d, &o) in p.items.iter().zip(&p.owner) {
            if !is_draw(d) {
                continue;
            }
            let q = oir[o];
            push(&mut cur, q, Tok::Elem(o));
            let mut c = q;
            while let Some(pp) = ir_parent(c) {
                push(&mut cur, pp, Tok::Child(c));
                c = pp;
            }
        }
        for (k, v) in cur {
            seqs.entry(k).or_default().push(v);
        }
    }
    // merge: topological order of first occurrences
    let mut order_of: BTreeMap<ElemId, Vec<Tok>> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();
    for (&p, list) in &seqs {
        let mut first: HashMap<Tok, (usize, usize)> = HashMap::new();
        let mut edges: HashMap<Tok, BTreeSet<Tok>> = HashMap::new();
        let mut indeg: HashMap<Tok, usize> = HashMap::new();
        for (pi, s) in list.iter().enumerate() {
            let mut seen: HashSet<Tok> = HashSet::new();
            let mut prev: Option<Tok> = None;
            for (k, &tok) in s.iter().enumerate() {
                first.entry(tok).or_insert((pi, k));
                indeg.entry(tok).or_insert(0);
                if !seen.insert(tok) {
                    continue; // later occurrences: checked per unit below
                }
                if let Some(a) = prev {
                    if edges.entry(a).or_default().insert(tok) {
                        *indeg.get_mut(&tok).unwrap() += 1;
                    }
                }
                prev = Some(tok);
            }
        }
        let mut ready: BTreeSet<((usize, usize), Tok)> = indeg.iter().filter(|(_, d)| **d == 0).map(|(t, _)| (first[t], *t)).collect();
        let mut out = Vec::new();
        while let Some(x) = ready.pop_first() {
            out.push(x.1);
            if let Some(es) = edges.get(&x.1) {
                for &y in es {
                    let d = indeg.get_mut(&y).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        ready.insert((first[&y], y));
                    }
                }
            }
        }
        if out.len() != indeg.len() {
            let left: Vec<String> = indeg.iter().filter(|(t, _)| !out.contains(t)).map(|(t, _)| format!("{t:?}")).take(12).collect();
            bail!("paint order of <{}> #{p} changes between states (cycle among {left:?})", tags[p]);
        }
        order_of.insert(p, out);
    }

    // ---------------------------------------------------------------- units
    struct Unit {
        ir: ElemId,
        elems: Vec<ElemId>,
        node: usize,
    }
    let mut units: Vec<Unit> = Vec::new();
    let mut unit_of: Vec<usize> = vec![NONE; n_el];
    // children of each IR element in paint order: Unit(u) | Child(c)
    let mut ir_children: BTreeMap<ElemId, Vec<Tok>> = BTreeMap::new();
    for (&p, toks) in &order_of {
        let mut list = Vec::new();
        let mut cur: Option<(Option<ElemId>, usize)> = None;
        for &tok in toks {
            match tok {
                Tok::Child(c) => {
                    cur = None;
                    list.push(Tok::Child(c));
                }
                Tok::Elem(e) => {
                    let r = region_of(e);
                    let u = match cur {
                        Some((rr, u)) if rr == r => u,
                        _ => {
                            units.push(Unit { ir: p, elems: vec![], node: 0 });
                            let u = units.len() - 1;
                            list.push(Tok::Elem(u)); // Elem(u) = unit index here
                            cur = Some((r, u));
                            u
                        }
                    };
                    units[u].elems.push(e);
                    unit_of[e] = u;
                }
            }
        }
        ir_children.insert(p, list);
    }
    // contiguity check per probe: each unit's draws must not interleave with other
    // units or nested IR nodes of the same parent
    for p in &probes {
        let mut last: BTreeMap<ElemId, Vec<Tok>> = BTreeMap::new();
        for (d, &o) in p.items.iter().zip(&p.owner) {
            if !is_draw(d) {
                continue;
            }
            let q = oir[o];
            let tok = Tok::Elem(unit_of[o]);
            let v = last.entry(q).or_default();
            if v.last() != Some(&tok) {
                if v.contains(&tok) {
                    warnings.push(format!("frame {}: unit {} of <{}> #{q} is split in paint order (element #{o})", p.frame, unit_of[o], tags[q]));
                }
                v.push(tok);
            }
            let mut c = q;
            while let Some(pp) = ir_parent(c) {
                let v = last.entry(pp).or_default();
                let tok = Tok::Child(c);
                if v.last() != Some(&tok) {
                    if v.contains(&tok) {
                        warnings.push(format!("frame {}: IR node #{c} is split in paint order of #{pp}", p.frame));
                    }
                    v.push(tok);
                }
                c = pp;
            }
        }
    }
    warnings.sort();
    warnings.dedup();

    // ---------------------------------------------------------------- IR nodes
    let mut nodes: Vec<Node> = vec![Node::new("document")];
    let mut tracks: Vec<agentfx_ir::Track> = Vec::new();
    // placement of an IR element: from the first probe where it paints
    let first_place = |e: ElemId| -> Option<(usize, kurbo::Affine, kurbo::Vec2)> {
        probes.iter().enumerate().find_map(|(i, p)| p.place.get(&e).map(|&(a, b)| (i, a, b)))
    };
    // static (unanimated) composite values of layers: needed where no track is active
    let mut errors: Vec<String> = Vec::new();
    #[allow(clippy::too_many_arguments)]
    fn add_ir(
        x: ElemId,
        parent_node: usize,
        nodes: &mut Vec<Node>,
        tracks: &mut Vec<agentfx_ir::Track>,
        units: &mut [Unit],
        ir_children: &BTreeMap<ElemId, Vec<Tok>>,
        layers: &BTreeSet<ElemId>,
        groups: &BTreeSet<ElemId>,
        comp_by_elem: &BTreeMap<ElemId, Vec<usize>>,
        compiled: &Compiled,
        probes: &[Probe],
        first_place: &dyn Fn(ElemId) -> Option<(usize, kurbo::Affine, kurbo::Vec2)>,
        errors: &mut Vec<String>,
        warnings: &mut Vec<String>,
        frames: usize,
        fps: f64,
        bases: &HashMap<ElemId, ([f64; 2], f64, [f64; 2], f64, bool)>,
    ) {
        let tag = &compiled.tags[x];
        let mut inner = parent_node;
        if x == 0 {
            inner = 0;
        } else if layers.contains(&x) {
            let Some((pi, ptf, bp)) = first_place(x) else { return };
            let to = probes[pi].origin.get(&x).copied().unwrap_or([0.0; 2]);
            let o = ptf * kurbo::Point::new(bp.x + to[0], bp.y + to[1]);
            let lin = ptf.as_coeffs();
            // origin moving by layout matters for rotate/scale only
            let comp = &comp_by_elem[&x];
            let has_rs = comp.iter().any(|&i| matches!(compiled.tracks[i].property.as_str(), "rotate" | "scale" | "transform"));
            for p in probes.iter().skip(pi + 1) {
                if let (Some(&(a, b)), Some(t2)) = (p.place.get(&x), p.origin.get(&x)) {
                    // (review M3 #4) the IR node keeps the first probe's origin and static
                    // ancestor transform: a later change would draw wrong, so fail loudly
                    let l2 = a.as_coeffs();
                    if (0..4).any(|k| (l2[k] - lin[k]).abs() > 1e-6) {
                        errors.push(format!("<{tag}> #{x}: static ancestor transform of an animated element changes at frame {} (unsupported)", p.frame));
                        break;
                    }
                    let o2 = a * kurbo::Point::new(b.x + t2[0], b.y + t2[1]);
                    if has_rs && (o2 - o).hypot() > 0.01 {
                        errors.push(format!("<{tag}> #{x}: transform origin moves by layout ({o:?} -> {o2:?} at frame {}) under rotate/scale (unsupported)", p.frame));
                        break;
                    }
                }
            }
            let mut by_prop: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
            for &i in comp {
                by_prop.entry(compiled.tracks[i].property.as_str()).or_default().push(i);
            }
            // every composite property must be covered by a track at every frame
            // (else the stylesheet value would show through: not modeled)
            let base = bases.get(&x).copied();
            for (prop, list) in &by_prop {
                let gap = (0..frames).find(|&f| list.iter().all(|&i| eval_track(&compiled.tracks[i].to_ir(0), f as f64 / fps).is_none()));
                if let Some(f) = gap {
                    if *prop == "transform" && base.is_none_or(|b| b.4) {
                        errors.push(format!("<{tag}> #{x}: transform has no active animation at frame {f} and a static transform (unsupported)"));
                    }
                    if base.is_none() {
                        errors.push(format!("<{tag}> #{x}: {prop} has no active animation at frame {f} and no static value"));
                    }
                }
            }
            let mut first = true;
            for prop in ["translate", "rotate", "scale", "transform"] {
                let Some(list) = by_prop.get(prop) else { continue };
                let id = nodes.len();
                let mut n = Node::new(format!("<{tag}> #{x} {prop}"));
                n.origin = [o.x, o.y];
                if let Some(b) = base {
                    let f = match prop {
                        "translate" => Some(TransformFn::Translate(b.0[0], b.0[1])),
                        "rotate" => Some(TransformFn::Rotate(b.1)),
                        "scale" => Some(TransformFn::Scale(b.2[0], b.2[1])),
                        _ => None,
                    };
                    if let Some(f) = f {
                        match conjugate(&f, &lin) {
                            Ok(g) => n.transform = vec![g],
                            Err(e) => errors.push(format!("<{tag}> #{x}: {e}")),
                        }
                    }
                    if first && by_prop.contains_key("opacity") {
                        n.opacity = b.3;
                    }
                }
                nodes.push(n);
                nodes[inner].children.push(id);
                if first {
                    if let Some(ol) = by_prop.get("opacity") {
                        for &i in ol {
                            tracks.push(compiled.tracks[i].to_ir(id));
                        }
                    }
                    first = false;
                }
                for &i in list {
                    let mut tr = compiled.tracks[i].to_ir(id);
                    tr.property = "transform".into();
                    for k in tr.keyframes.iter_mut() {
                        if let Value::Transform(fs) = &mut k.value {
                            for f in fs.iter_mut() {
                                match conjugate(f, &lin) {
                                    Ok(g) => *f = g,
                                    Err(e) => {
                                        errors.push(format!("<{tag}> #{x}: {e}"));
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    tracks.push(tr);
                }
                inner = id;
            }
            if first {
                // opacity only
                let id = nodes.len();
                let mut n = Node::new(format!("<{tag}> #{x} opacity"));
                n.origin = [o.x, o.y];
                if let Some(b) = base {
                    n.opacity = b.3;
                }
                nodes.push(n);
                nodes[inner].children.push(id);
                for &i in &by_prop["opacity"] {
                    tracks.push(compiled.tracks[i].to_ir(id));
                }
                inner = id;
            }
        } else if groups.contains(&x) {
            // one IR node per effect push of the group element (outermost first)
            let mut pushes: Option<Vec<Draw>> = None;
            for p in probes {
                let mut mine = Vec::new();
                for (d, &o) in p.items.iter().zip(&p.owner) {
                    if o == x && matches!(d, Draw::Push { .. }) {
                        mine.push(d.clone());
                    }
                }
                if mine.is_empty() {
                    continue;
                }
                match &pushes {
                    None => pushes = Some(mine),
                    Some(prev) if *prev != mine => {
                        errors.push(format!("<{tag}> #{x}: group effect (clip/opacity/filter) changes over time around a layer (frame {})", p.frame));
                        break;
                    }
                    _ => {}
                }
            }
            for d in pushes.unwrap_or_default() {
                let Draw::Push { clip, transform, alpha, blend, filter, even_odd: _ } = d else { continue };
                let id = nodes.len();
                let mut n = Node::new(format!("<{tag}> #{x} group"));
                n.opacity = alpha as f64;
                n.blend = blend;
                if let Some(c) = &clip {
                    match rrect_of(c, &transform) {
                        Ok(r) => n.clips.push(r),
                        Err(e) => errors.push(format!("<{tag}> #{x}: {e}")),
                    }
                }
                if let Some(f) = filter {
                    let s = transform[0].hypot(transform[1]);
                    n.effects.push(match f {
                        agentfx_ir::DrawFilter::Blur { std_dev } => agentfx_ir::Effect::Blur { radius: std_dev * s },
                        agentfx_ir::DrawFilter::DropShadow { dx, dy, std_dev, color } => {
                            agentfx_ir::Effect::DropShadow { dx: dx * s, dy: dy * s, radius: std_dev * s, color: color.map(|c| c as f64) }
                        }
                    });
                }
                nodes.push(n);
                nodes[inner].children.push(id);
                inner = id;
            }
        }
        for tok in ir_children.get(&x).cloned().unwrap_or_default() {
            match tok {
                Tok::Elem(u) => {
                    let id = nodes.len();
                    nodes.push(Node::new(format!("unit {u}")));
                    nodes[inner].children.push(id);
                    units[u].node = id;
                }
                Tok::Child(c) => add_ir(c, inner, nodes, tracks, units, ir_children, layers, groups, comp_by_elem, compiled, probes, first_place, errors, warnings, frames, fps, bases),
            }
        }
    }
    add_ir(0, 0, &mut nodes, &mut tracks, &mut units, &ir_children, &layers, &groups, &comp_by_elem, &compiled, &probes, &first_place, &mut errors, &mut warnings, frames, fps, &bases);
    if !errors.is_empty() {
        errors.sort();
        errors.dedup();
        bail!("unsupported scene features:\n  - {}", errors.join("\n  - "));
    }
    timing.insert("plan_s", t.elapsed().as_secs_f64());

    // ---------------------------------------------------------------- unit contents
    let t = Instant::now();
    let fonts = res.font_data.clone();
    let fl = |f: usize| fonts.get(f).cloned();
    let mut n_states = 0usize;
    let ci_of = |f: usize| -> usize { cf.partition_point(|&c| c <= f) - 1 };
    for (ui, u) in units.iter().enumerate() {
        let mut keys: HashMap<u64, usize> = HashMap::new();
        let mut states: Vec<Content> = Vec::new();
        let mut state_of_probe: Vec<usize> = Vec::with_capacity(probes.len());
        for p in &probes {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for &e in &u.elems {
                p.hash[e].hash(&mut h);
            }
            let key = h.finish();
            let s = match keys.get(&key) {
                Some(&s) => s,
                None => {
                    // extract the unit's items from this probe
                    let mut out = Vec::new();
                    let mut keep: Vec<bool> = Vec::new();
                    for (d, &o) in p.items.iter().zip(&p.owner) {
                        match d {
                            Draw::Push { .. } => {
                                let k = oir[o] == u.ir && !groups.contains(&o);
                                keep.push(k);
                                if k {
                                    out.push(d.clone());
                                }
                            }
                            Draw::Pop => {
                                if keep.pop().unwrap_or(false) {
                                    out.push(Draw::Pop);
                                }
                            }
                            _ => {
                                if unit_of[o] == ui {
                                    out.push(d.clone());
                                }
                            }
                        }
                    }
                    let items = prune(out);
                    let ink = if items.is_empty() { [0.0; 4] } else { record::bounds(&items, &fl, true) };
                    let rect = schedule_clamp(ink);
                    // empty state: a 1 px placeholder rect (validation wants positive sizes)
                    let rect = if rect[2] > 0.0 { rect } else { [0.0, 0.0, 1.0, 1.0] };
                    states.push(Content::Paint { rect, items, elements: u.elems.iter().map(|&e| e % n_el0).collect(), text: None });
                    keys.insert(key, states.len() - 1);
                    n_states += 1;
                    states.len() - 1
                }
            };
            state_of_probe.push(s);
        }
        let mut fr: Vec<(u32, usize)> = Vec::new();
        for (ci, &s) in state_of_probe.iter().enumerate() {
            if fr.last().is_none_or(|x| x.1 != s) {
                fr.push((cf[ci] as u32, s));
            }
        }
        let _ = ci_of;
        let content = if states.len() == 1 {
            let c = states.pop().unwrap();
            match &c {
                Content::Paint { items, .. } if items.is_empty() => None,
                _ => Some(c),
            }
        } else {
            let mut rect = [0.0; 4];
            for s in &states {
                if let Content::Paint { rect: r, .. } = s {
                    rect = crate::schedule::union(rect, *r);
                }
            }
            Some(Content::Switch { rect, frames: fr, states })
        };
        nodes[u.node].content = content;
    }
    timing.insert("extract_s", t.elapsed().as_secs_f64());
    timing.insert("frontend_s", t0.elapsed().as_secs_f64());
    let scene = Scene {
        version: agentfx_ir::IR_VERSION,
        width: W as u32,
        height: H as u32,
        fps: fps as u32,
        frames: frames as u32,
        nodes,
        tracks,
        assets: res.assets.clone(),
        fonts: res.fonts.clone(),
        camera: None,
        // recorded scenes (Remotion/HyperFrames via the recorder) are compared against
        // renders that draw text non-composited over opaque content: LCD antialiasing
        text_aa: if compiled.recorded { agentfx_ir::TextAa::Lcd } else { agentfx_ir::TextAa::Gray },
    };
    let stats = serde_json::json!({
        "planner": "probe (M2)",
        "elements": n_el,
        "change_frames": cf.len(),
        "probe_items": n_items,
        "layers": layers.len(),
        "groups": groups.iter().map(|&g| {
            let kinds: Vec<String> = probes.iter().find_map(|p| {
                let v: Vec<String> = p.items.iter().zip(&p.owner).filter(|(d, o)| **o == g && matches!(d, Draw::Push { .. })).map(|(d, _)| match d {
                    Draw::Push { alpha, filter, blend, .. } => format!("{}{}{}", if *alpha < 1.0 { "alpha " } else { "" }, if filter.is_some() { "filter " } else { "" }, if *blend != agentfx_ir::BlendMode::Normal { "blend" } else { "clip" }),
                    _ => String::new(),
                }).collect();
                (!v.is_empty()).then_some(v)
            }).unwrap_or_default();
            format!("<{}> #{g}: {}", compiled.tags[g], kinds.join(", "))
        }).collect::<Vec<_>>(),
        "regions": regions.len(),
        "units": units.len(),
        "unit_states": n_states,
        "ir_nodes": scene.nodes.len(),
        "ir_tracks": scene.tracks.len(),
        "warnings": warnings.iter().take(40).collect::<Vec<_>>(),
        "warnings_total": warnings.len(),
        "blitz": bz.timings,
        "timing": timing,
    });
    Ok((Plan { scene, stats }, res))
}

/// Ink rect clamped to a sane area around the viewport (content far off screen still
/// has a finite rect).
fn schedule_clamp(r: [f64; 4]) -> [f64; 4] {
    let x0 = r[0].max(-4.0 * W);
    let y0 = r[1].max(-4.0 * H);
    let x1 = (r[0] + r[2]).min(5.0 * W);
    let y1 = (r[1] + r[3]).min(5.0 * H);
    if x1 <= x0 || y1 <= y0 { [0.0; 4] } else { [x0, y0, x1 - x0, y1 - y0] }
}


#[cfg(test)]
mod tests {
    #[test]
    fn rrect_from_blitz_path() {
        let d = "M1,10.000000000000002 C0.9999999999999982,5.029437251522861 5.029437251522858,1.0000000000000018 9.999999999999998,1 L390,1 C394.97056274847716,0.9999999999999982 399,5.029437251522858 399,9.999999999999998 L399,558 C399,562.9705627484772 394.97056274847716,567 390,567 L10.000000000000004,567 C5.029437251522863,567 1.0000000000000018,562.9705627484772 1,558";
        use kurbo::Shape as _;
        let p = kurbo::BezPath::from_svg(d).unwrap();
        eprintln!("area {} bbox {:?}", p.area(), p.bounding_box());
        let r = super::path_rrect(d);
        eprintln!("{r:?}");
        assert!(r.is_some());
    }
}

#[cfg(test)]
mod review_m3_tests {
    //! Planner cases from the M3 review: unsupported inputs fail loudly instead of
    //! rendering wrong; a long LCD ticker line renders (no "glyph run too large").
    use super::build;
    use std::path::{Path, PathBuf};

    fn scene(name: &str) -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/review-m3").join(name) }

    fn out(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").join(format!("plan2-review-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fails_with(name: &str, needle: &str) {
        let o = out(name);
        let r = build(&scene(name), &o);
        let _ = std::fs::remove_dir_all(&o);
        let e = match r { Ok(_) => panic!("{name}: planned without error"), Err(e) => format!("{e:#}") };
        assert!(e.contains(needle), "{name}: unexpected error: {e}");
    }

    #[test]
    fn origin_moving_under_rotate_fails_loudly() { fails_with("origin-moves", "transform origin moves by layout"); }

    #[test]
    fn layer_clip_around_nested_layer_fails_loudly() { fails_with("clip-nested", "containing another animated element"); }

    #[test]
    fn held_transform_over_interpolated_fails_loudly() { fails_with("held-over", "overrides an interpolated one"); }

    #[test]
    fn long_lcd_ticker_line_renders() {
        let o = out("ticker");
        let (p, res) = build(&scene("ticker"), &o).unwrap();
        let scene = p.scene;
        assert_eq!(scene.text_aa, agentfx_ir::TextAa::Lcd);
        let prep = crate::vraster::Prepared::new(&scene, &o, Some(res.font_data.as_slice()), Some(res.image_data.as_slice())).unwrap();
        let info = crate::vraster::Info::new(&scene, &prep);
        let sched = crate::schedule::build(&scene, &info).unwrap();
        let mut painter = crate::vraster::Painter::new(&scene, &prep, &sched, &info, 0);
        for f in 0..sched.frames {
            painter.render_rect(f, [0, 0, scene.width as i32, scene.height as i32]).unwrap();
        }
        let _ = std::fs::remove_dir_all(&o);
    }
}
