//! HTML frontend planner: CSS timeline + frame-0 geometry (Blitz, `blitzfe.rs`; M0: Chrome)
//! -> IR scene + paint jobs (units / repaint states; painted natively by `blitzfe.rs`).
//!
//! Steps: classify tracks (composite/paint/layout) -> layer roots (principle 3/5) ->
//! paint order -> paint units (static runs between nested layers) -> IR nodes ->
//! raster scale from the IR schedule -> raster jobs (once per unit; repaint units
//! once per distinct paint/layout state).
use crate::model::*;
use crate::schedule::{self, intersect, union};
use anyhow::{Context, Result, bail};
use agentfx_ir::eval::eval_track;
use agentfx_ir::{Content, RRect, Scene};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const W: f64 = 1920.0;
const H: f64 = 1080.0;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct GeomElem {
    pub tag: String,
    pub rect: [f64; 4],
    pub ink: Option<[f64; 4]>,
    #[serde(default)]
    pub ink_error: Option<String>,
    pub position: String,
    pub z_index: String,
    pub display: String,
    pub overflow_clip: bool,
    pub radius: [f64; 4],
    pub transform_origin: [f64; 2],
    pub transform: String,
    pub opacity: f64,
    pub filter: String,
    pub mix_blend_mode: String,
    pub visibility: String,
    pub pseudo: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct Geom {
    pub elements: Vec<GeomElem>,
}

pub fn load_geom(path: &Path) -> Result<Geom> {
    let s = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(serde_json::from_str(&s)?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LayerKind {
    Composite,
    Repaint,
}

#[derive(Clone, Debug)]
enum Child {
    Unit(usize),
    Layer(usize),
}

#[derive(Clone, Debug)]
struct Layer {
    root: ElemId,
    kind: LayerKind,
    parent: Option<usize>,
    children: Vec<Child>,
    clips: Vec<ElemId>,
    composite_tracks: Vec<usize>,
    sig_tracks: Vec<usize>,
    node: usize,
}

#[derive(Clone, Debug)]
struct Unit {
    layer: Option<usize>,
    elems: Vec<ElemId>,
    /// unsnapped ink rect
    ink: [f64; 4],
    node: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub id: usize,
    pub unit: usize,
    pub file: String,
    pub time: f64,
    pub frame: usize,
    pub scale: f64,
    pub rect: [f64; 4],
    pub visible: Vec<ElemId>,
    pub opaque_root: bool,
    /// Repaint root whose subtree must stay inside `rect` and whose own box must stay `base_rect`.
    pub bounds_check: Option<ElemId>,
    pub base_rect: Option<[f64; 4]>,
}

pub struct HtmlOut {
    pub scene: Scene,
    pub jobs: Vec<Job>,
    pub neutral: Vec<ElemId>,
    pub stats: serde_json::Value,
}

fn parse_matrix_none(s: &str) -> bool {
    s == "none"
}

enum Item {
    Elem(ElemId),
    Layer(ElemId),
}

/// Raster scale: smallest k/20 >= max scale (k/20 keeps pixel alignment cheap),
/// plus the CSS px alignment q so that q * scale is an integer.
fn raster_scale(max_scale: f64) -> (f64, f64) {
    let k = ((max_scale * 20.0) - 1e-6).ceil().clamp(20.0, 80.0) as u32;
    let g = gcd(k, 20);
    (k as f64 / 20.0, (20 / g) as f64)
}
fn lcm(a: u32, b: u32) -> u32 {
    a / gcd(a, b) * b
}
fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

pub fn build(compiled: &Compiled, geom: &Geom) -> Result<HtmlOut> {
    let n_el = compiled.tags.len();
    let mut errors: Vec<String> = Vec::new();
    if geom.elements.len() != n_el {
        bail!("geometry has {} elements, compiler DOM has {}", geom.elements.len(), n_el);
    }
    for (i, (t, g)) in compiled.tags.iter().zip(&geom.elements).enumerate() {
        if !t.eq_ignore_ascii_case(&g.tag) {
            bail!("element {i}: compiler tag <{t}> vs Chrome <{}>; DOMs differ", g.tag);
        }
    }
    if compiled.fps.fract() != 0.0 {
        bail!("fps {} not an integer (M0)", compiled.fps);
    }
    let g = &geom.elements;
    let parent = &compiled.parent;
    let mut children: Vec<Vec<ElemId>> = vec![Vec::new(); n_el];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            children[*p].push(i);
        }
    }
    let is_anc = |a: ElemId, mut e: ElemId| -> bool {
        while let Some(p) = parent[e] {
            if p == a {
                return true;
            }
            e = p;
        }
        false
    };
    let tracks = &compiled.tracks;
    let ir_tracks: Vec<agentfx_ir::Track> = tracks.iter().map(|t| t.to_ir(0)).collect();
    let mut by_elem: BTreeMap<ElemId, Vec<usize>> = BTreeMap::new();
    for (i, t) in tracks.iter().enumerate() {
        by_elem.entry(t.elem).or_default().push(i);
        if t.stage != Stage::Composite && matches!(t.property.as_str(), "box-shadow" | "filter" | "text-shadow") {
            errors.push(format!("element #{} animates {}: ink bounds change, unsupported in M0", t.elem, t.property));
        }
    }
    let has_stage = |e: ElemId, s: Stage| by_elem.get(&e).is_some_and(|v| v.iter().any(|&i| tracks[i].stage == s));

    // --- repaint roots (principle 5: escalate locally)
    let mut repaint_roots: Vec<ElemId> = Vec::new();
    for &e in by_elem.keys() {
        if has_stage(e, Stage::Layout) {
            let abs = matches!(g[e].position.as_str(), "absolute" | "fixed");
            repaint_roots.push(if abs { e } else { parent[e].unwrap_or(e) });
        }
        if has_stage(e, Stage::Paint) {
            repaint_roots.push(e);
        }
    }
    repaint_roots.sort();
    repaint_roots.dedup();
    let rr = repaint_roots.clone();
    repaint_roots.retain(|&r| !rr.iter().any(|&o| o != r && is_anc(o, r)));

    // --- composite roots
    let mut roots: Vec<ElemId> = repaint_roots.clone();
    for &e in by_elem.keys() {
        if has_stage(e, Stage::Composite) && !repaint_roots.iter().any(|&r| is_anc(r, e)) && !roots.contains(&e) {
            roots.push(e);
        }
    }
    roots.sort();
    let layer_of_root: HashMap<ElemId, usize> = roots.iter().enumerate().map(|(i, &r)| (r, i)).collect();
    let mut owner: Vec<Option<usize>> = vec![None; n_el];
    for e in 0..n_el {
        let mut c = Some(e);
        while let Some(x) = c {
            if let Some(&l) = layer_of_root.get(&x) {
                owner[e] = Some(l);
                break;
            }
            c = parent[x];
        }
    }

    // --- fail loudly on what M0 cannot represent
    for (e, ge) in g.iter().enumerate() {
        if ge.z_index != "auto" && ge.position != "static" {
            errors.push(format!("<{}> #{e}: z-index {} unsupported in M0", ge.tag, ge.z_index));
        }
        if ge.mix_blend_mode != "normal" {
            errors.push(format!("<{}> #{e}: mix-blend-mode {} unsupported", ge.tag, ge.mix_blend_mode));
        }
        if let Some(why) = &ge.ink_error {
            if ge.display != "none" {
                errors.push(format!("<{}> #{e}: {why} unsupported (ink bounds unknown)", ge.tag));
            }
        }
        if !ge.pseudo.is_empty() {
            errors.push(format!("<{}> #{e}: generated content {:?} unsupported in M0", ge.tag, ge.pseudo));
        }
    }
    for &r in &roots {
        if !parse_matrix_none(&g[r].transform) {
            errors.push(format!("layer root <{}> #{r}: base transform {} unsupported (animate it instead)", g[r].tag, g[r].transform));
        }
        let mut p = parent[r];
        while let Some(x) = p {
            if layer_of_root.contains_key(&x) {
                break;
            }
            if !parse_matrix_none(&g[x].transform) {
                errors.push(format!("static ancestor <{}> #{x} of layer #{r} has transform {}: unsupported in M0", g[x].tag, g[x].transform));
            }
            if g[x].opacity < 1.0 || g[x].filter != "none" {
                errors.push(format!("static ancestor <{}> #{x} of layer #{r} has opacity/filter: group effects across layers unsupported in M0", g[x].tag));
            }
            p = parent[x];
        }
    }

    // --- paint order (CSS painting-order approximation)
    let positioned = |e: ElemId| -> bool {
        let ge = &g[e];
        ge.position != "static" || ge.opacity < 1.0 || ge.transform != "none" || ge.filter != "none" || layer_of_root.contains_key(&e)
    };
    // In-flow layer roots are painted with the positioned phase: only valid if nothing
    // later in tree order (in-flow) overlaps them.
    let subtree_ink = |r: ElemId| -> [f64; 4] {
        let mut b = [0.0; 4];
        for e in 0..n_el {
            if (e == r || is_anc(r, e)) && g[e].display != "none" {
                if let Some(i) = g[e].ink {
                    b = union(b, i);
                }
            }
        }
        b
    };
    for &r in &roots {
        let ge = &g[r];
        if ge.position == "static" && ge.opacity >= 1.0 && ge.transform == "none" && ge.filter == "none" {
            let ri = subtree_ink(r);
            for e in r + 1..n_el {
                if is_anc(r, e) || is_anc(e, r) || positioned(e) || g[e].display == "none" {
                    continue;
                }
                if let Some(i) = g[e].ink {
                    let x = intersect(ri, i);
                    if x[2] > 0.5 && x[3] > 0.5 {
                        errors.push(format!("in-flow layer root #{r} overlaps later in-flow <{}> #{e}: paint order would change", g[e].tag));
                    }
                }
            }
        }
    }
    fn collect(e: ElemId, children: &[Vec<ElemId>], positioned: &dyn Fn(ElemId) -> bool, inflow: &mut Vec<ElemId>, pos: &mut Vec<ElemId>) {
        for &c in &children[e] {
            if positioned(c) {
                pos.push(c);
            } else {
                inflow.push(c);
                collect(c, children, positioned, inflow, pos);
            }
        }
    }
    fn seq(e: ElemId, ctx: ElemId, children: &[Vec<ElemId>], positioned: &dyn Fn(ElemId) -> bool, is_root: &dyn Fn(ElemId) -> bool, out: &mut Vec<Item>) {
        out.push(Item::Elem(e));
        let mut inflow = Vec::new();
        let mut pos = Vec::new();
        collect(e, children, positioned, &mut inflow, &mut pos);
        out.extend(inflow.into_iter().map(Item::Elem));
        for p in pos {
            if is_root(p) && p != ctx {
                out.push(Item::Layer(p));
            } else {
                seq(p, ctx, children, positioned, is_root, out);
            }
        }
    }
    let is_root = |e: ElemId| layer_of_root.contains_key(&e);

    // --- layers
    let mut layers: Vec<Layer> = Vec::new();
    for &r in &roots {
        let kind = if repaint_roots.contains(&r) { LayerKind::Repaint } else { LayerKind::Composite };
        let mut composite_tracks: Vec<usize> =
            by_elem.get(&r).map(|v| v.iter().copied().filter(|&i| tracks[i].stage == Stage::Composite).collect()).unwrap_or_default();
        composite_tracks.sort_by_key(|&i| tracks[i].order);
        let mut sig_tracks = Vec::new();
        if kind == LayerKind::Repaint {
            for (&e, v) in &by_elem {
                if e == r || is_anc(r, e) {
                    for &i in v {
                        if !(e == r && tracks[i].stage == Stage::Composite) {
                            sig_tracks.push(i);
                        }
                    }
                }
            }
        }
        let mut p = parent[r];
        let mut parent_layer = None;
        let mut clips = Vec::new();
        while let Some(x) = p {
            if g[x].overflow_clip {
                clips.push(x);
            }
            if let Some(&l) = layer_of_root.get(&x) {
                parent_layer = Some(l);
                break;
            }
            p = parent[x];
        }
        layers.push(Layer { root: r, kind, parent: parent_layer, children: Vec::new(), clips, composite_tracks, sig_tracks, node: 0 });
    }

    // --- units
    let mut units: Vec<Unit> = Vec::new();
    let mut build_ctx = |ctx: Option<usize>, units: &mut Vec<Unit>| -> Vec<Child> {
        let mut items = Vec::new();
        match ctx {
            None => seq(0, usize::MAX, &children, &positioned, &is_root, &mut items),
            Some(l) => {
                let r = layers[l].root;
                seq(r, r, &children, &positioned, &is_root, &mut items)
            }
        }
        let mut out = Vec::new();
        let mut run: Vec<ElemId> = Vec::new();
        let flush = |run: &mut Vec<ElemId>, out: &mut Vec<Child>, units: &mut Vec<Unit>| {
            if run.is_empty() {
                return;
            }
            let elems = std::mem::take(run);
            units.push(Unit { layer: ctx, elems, ink: [0.0; 4], node: 0 });
            out.push(Child::Unit(units.len() - 1));
        };
        for it in items {
            match it {
                // author-hidden elements stay hidden (and out of the ink)
                Item::Elem(e) if g[e].visibility != "visible" => {}
                Item::Elem(e) => run.push(e),
                Item::Layer(r) => {
                    flush(&mut run, &mut out, units);
                    out.push(Child::Layer(layer_of_root[&r]));
                }
            }
        }
        flush(&mut run, &mut out, units);
        out
    };
    let top = build_ctx(None, &mut units);
    let all: Vec<Vec<Child>> = (0..layers.len()).map(|l| build_ctx(Some(l), &mut units)).collect();
    drop(build_ctx);
    for (l, ch) in all.into_iter().enumerate() {
        layers[l].children = ch;
    }

    // unit ink: union of element ink, clipped by clipping ancestors inside the unit's context
    for u in units.iter_mut() {
        let ctx_root = u.layer.map(|l| layers[l].root);
        let mut rect = [0.0; 4];
        for &e in &u.elems {
            let Some(mut ink) = g[e].ink else { continue };
            if g[e].display == "none" || ink[2] <= 0.0 || ink[3] <= 0.0 {
                continue;
            }
            let mut p = parent[e];
            while let Some(x) = p {
                if g[x].overflow_clip {
                    ink = intersect(ink, g[x].rect);
                }
                if Some(x) == ctx_root {
                    break;
                }
                p = parent[x];
            }
            rect = union(rect, ink);
        }
        // repaint layers may grow inside their root's box (containment test): the unit
        // covers the root's border box, not only the ink at t = 0
        if let Some(l) = u.layer {
            if layers[l].kind == LayerKind::Repaint && u.elems.contains(&layers[l].root) {
                rect = union(rect, g[layers[l].root].rect);
            }
        }
        if rect[2] > 0.0 {
            if let Some(l) = u.layer {
                // content that an outer (ancestor) clip hides at neutral would be lost when moved into view
                let mut outer = [0.0, 0.0, W, H];
                let mut x = Some(l);
                while let Some(k) = x {
                    for &c in &layers[k].clips {
                        outer = intersect(outer, g[c].rect);
                    }
                    x = layers[k].parent;
                }
                // 2.5 px tolerance: the ink includes a 2 px AA pad
                let inside = intersect(rect, outer);
                if (inside[2] - rect[2]).abs() > 4.5 || (inside[3] - rect[3]).abs() > 4.5 {
                    errors.push(format!(
                        "layer #{}: content {:?} is partly outside ancestor clips / viewport {:?} at neutral layout (M0 cannot rasterize it)",
                        layers[l].root, rect, outer
                    ));
                }
            }
        }
        u.ink = intersect(rect, [0.0, 0.0, W, H]);
    }
    if !errors.is_empty() {
        bail!("unsupported scene features:\n  - {}", errors.join("\n  - "));
    }

    // --- IR nodes
    let frames = (compiled.duration * compiled.fps).round() as usize;
    let fps = compiled.fps;
    let mut nodes: Vec<agentfx_ir::Node> = vec![agentfx_ir::Node {
        name: "document".into(),
        children: vec![],
        content: None,
        origin: [0.0, 0.0],
        transform: vec![],
        opacity: 1.0,
        clips: vec![],
        blend: Default::default(),
        isolate: true,
        effects: vec![],
        motion_blur: None, time: None, world: None,
        hidden: false,
    }];
    let mut ir_tracks_out: Vec<agentfx_ir::Track> = Vec::new();
    fn add_children(
        list: &[Child],
        parent_node: usize,
        nodes: &mut Vec<agentfx_ir::Node>,
        layers: &mut [Layer],
        units: &mut [Unit],
        g: &[GeomElem],
        tags: &[String],
    ) {
        for c in list {
            let id = nodes.len();
            match *c {
                Child::Unit(u) => {
                    units[u].node = id;
                    nodes.push(agentfx_ir::Node {
                        name: format!("unit {u}"),
                        children: vec![],
                        content: None, // filled after rasterization planning
                        origin: [0.0, 0.0],
                        transform: vec![],
                        opacity: 1.0,
                        clips: vec![],
                        blend: Default::default(),
                        isolate: true,
                        effects: vec![],
                        motion_blur: None, time: None, world: None,
                        hidden: false,
                    });
                    nodes[parent_node].children.push(id);
                }
                Child::Layer(l) => {
                    let r = layers[l].root;
                    layers[l].node = id;
                    nodes.push(agentfx_ir::Node {
                        name: format!("<{}> #{r} ({:?})", tags[r], layers[l].kind),
                        children: vec![],
                        content: None,
                        origin: [g[r].rect[0] + g[r].transform_origin[0], g[r].rect[1] + g[r].transform_origin[1]],
                        transform: vec![],
                        opacity: g[r].opacity,
                        clips: layers[l].clips.iter().map(|&e| RRect { rect: g[e].rect, radius: g[e].radius }).collect(),
                        blend: Default::default(),
                        isolate: true,
                        effects: vec![],
                        motion_blur: None, time: None, world: None,
                        hidden: false,
                    });
                    nodes[parent_node].children.push(id);
                    let ch = layers[l].children.clone();
                    add_children(&ch, id, nodes, layers, units, g, tags);
                }
            }
        }
    }
    add_children(&top, 0, &mut nodes, &mut layers, &mut units, g, &compiled.tags);
    for l in &layers {
        for &i in &l.composite_tracks {
            ir_tracks_out.push(tracks[i].to_ir(l.node));
        }
    }
    // preliminary contents (rects only) to get max scales from the core schedule
    for u in &units {
        if u.ink[2] > 0.0 {
            nodes[u.node].content = Some(Content::Image { asset: usize::MAX, rect: u.ink, variants: vec![] });
        }
    }
    let mut scene = Scene {
        version: agentfx_ir::IR_VERSION,
        width: W as u32,
        height: H as u32,
        fps: fps as u32,
        frames: frames as u32,
        nodes,
        tracks: ir_tracks_out,
        assets: vec![],
        fonts: vec![],
        camera: None,
        text_aa: Default::default(),
    };
    let pre = schedule::build(&scene, &schedule::RectInfo)?;

    // --- repaint signatures per frame (paint/layout/text values of the repaint subtree)
    let sig_at = |l: &Layer, t: f64| -> String {
        let mut best: BTreeMap<(ElemId, &str), (usize, String)> = BTreeMap::new();
        for &i in &l.sig_tracks {
            let tr = &tracks[i];
            if let Some(s) = eval_track(&ir_tracks[i], t) {
                let v = match s {
                    Sampled::Matrix(a) => format!("{:.5},{:.5},{:.5},{:.5},{:.4},{:.4}", a.a, a.b, a.c, a.d, a.e, a.f),
                    Sampled::Number(x) if tr.property == "text" => format!("{}", (x + 1e-9).floor() as i64),
                    Sampled::Number(x) => format!("{x:.5}"),
                    Sampled::Sig(s) => s,
                    Sampled::Color(c) => format!("{:.5},{:.5},{:.5},{:.5}", c[0], c[1], c[2], c[3]),
                };
                let k = (tr.elem, tr.property.as_str());
                if best.get(&k).is_none_or(|b| b.0 <= tr.order) {
                    best.insert(k, (tr.order, v));
                }
            }
        }
        best.into_iter().map(|((e, p), (_, v))| format!("{e}.{p}={v};")).collect()
    };

    // --- raster jobs + final contents
    let mut jobs: Vec<Job> = Vec::new();
    let mut unit_scale = vec![1.0; units.len()];
    for (ui, u) in units.iter().enumerate() {
        if u.ink[2] <= 0.0 {
            nodes_clear(&mut scene, u.node);
            continue;
        }
        let (scale, mut q) = raster_scale(pre.max_scale[u.node].max(1.0));
        unit_scale[ui] = scale;
        let repaint_layer = u.layer.is_some_and(|l| layers[l].kind == LayerKind::Repaint);
        // rest scales: on-screen scales held for >= 2 frames; rasterized exactly so holds are
        // pixel-exact copies (only for composite units; k/20 scales keep alignment cheap)
        let mut rest: Vec<f64> = Vec::new();
        if !repaint_layer {
            for f in 1..frames {
                let (a, b) = (&pre.states[f - 1][u.node], &pre.states[f][u.node]);
                if a.chain == b.chain && b.eff_opacity > 0.0 {
                    let sc = b.chain.max_scale();
                    let k = sc * 20.0;
                    if (k - k.round()).abs() < 1e-6 && (sc - scale).abs() > 1e-6 && sc >= 0.25 && !rest.iter().any(|r| (r - sc).abs() < 1e-6) {
                        rest.push(sc);
                    }
                }
            }
        }
        for &r in &rest {
            let k = (r * 20.0).round() as u32;
            let qq = (20 / gcd(k, 20)) as f64;
            q = lcm(q as u32, qq as u32) as f64;
        }
        let x0 = ((u.ink[0] / q).floor() * q).max(0.0);
        let y0 = ((u.ink[1] / q).floor() * q).max(0.0);
        let x1 = (((u.ink[0] + u.ink[2]) / q).ceil() * q).min(W);
        let y1 = (((u.ink[1] + u.ink[3]) / q).ceil() * q).min(H);
        let rect = [x0, y0, x1 - x0, y1 - y0];
        let mut variants = Vec::new();
        for &r in &rest {
            let id = jobs.len();
            jobs.push(Job {
                id,
                unit: ui,
                file: format!("u{ui:03}-j{id:04}.png"),
                time: 0.0,
                frame: 0,
                scale: r,
                rect,
                visible: u.elems.clone(),
                opaque_root: u.elems.contains(&0),
                bounds_check: None,
                base_rect: None,
            });
            scene.assets.push(agentfx_ir::Asset { path: format!("layers/{}", jobs[id].file), scale: r });
            variants.push(id);
        }
        let repaint = u.layer.filter(|&l| layers[l].kind == LayerKind::Repaint);
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut seq_frames: Vec<(u32, usize)> = Vec::new();
        for f in 0..frames {
            let key = match repaint {
                Some(l) => sig_at(&layers[l], f as f64 / fps),
                None => String::new(),
            };
            let id = match seen.get(&key) {
                Some(&id) => id,
                None => {
                    let id = jobs.len();
                    jobs.push(Job {
                        id,
                        unit: ui,
                        file: format!("u{ui:03}-j{id:04}.png"),
                        time: if repaint.is_some() { f as f64 / fps } else { 0.0 },
                        frame: f,
                        scale,
                        rect,
                        visible: u.elems.clone(),
                        opaque_root: u.elems.contains(&0),
                        bounds_check: repaint.map(|l| layers[l].root),
                        base_rect: repaint.map(|l| g[layers[l].root].rect),
                    });
                    scene.assets.push(agentfx_ir::Asset { path: format!("layers/{}", jobs[id].file), scale });
                    seen.insert(key, id);
                    id
                }
            };
            if seq_frames.last().is_none_or(|x| x.1 != id) {
                seq_frames.push((f as u32, id));
            }
            if repaint.is_none() {
                break;
            }
        }
        scene.nodes[u.node].content = Some(if seq_frames.len() == 1 {
            Content::Image { asset: seq_frames[0].1, rect, variants }
        } else {
            Content::Sequence { rect, frames: seq_frames }
        });
    }
    let neutral = roots.clone();
    let raster_frames = {
        let mut f: Vec<usize> = jobs.iter().map(|j| j.frame).collect();
        f.sort();
        f.dedup();
        f.len()
    };
    let stats = serde_json::json!({
        "tracks": {
            "composite": tracks.iter().filter(|t| t.stage == Stage::Composite).count(),
            "paint": tracks.iter().filter(|t| t.stage == Stage::Paint).count(),
            "layout": tracks.iter().filter(|t| t.stage == Stage::Layout).count(),
        },
        "layers": layers.iter().map(|l| {
            let units_of: Vec<usize> = l.children.iter().filter_map(|c| if let Child::Unit(u) = c { Some(*u) } else { None }).collect();
            serde_json::json!({
                "root": format!("<{}> #{}", compiled.tags[l.root], l.root),
                "kind": l.kind,
                "max_scale": (pre.max_scale[l.node] * 1000.0).round() / 1000.0,
                "raster_scale": units_of.first().map(|&u| unit_scale[u]),
                "units": units_of.len(),
                "nested_layers": l.children.len() - units_of.len(),
                "raster_jobs": jobs.iter().filter(|j| units_of.contains(&j.unit)).count(),
            })
        }).collect::<Vec<_>>(),
        "units": units.len(),
        "document_units": units.iter().filter(|u| u.layer.is_none()).count(),
        "raster_jobs": jobs.len(),
        "raster_megapixels": (jobs.iter().map(|j| j.rect[2] * j.rect[3] * j.scale * j.scale).sum::<f64>() / 1e5).round() / 10.0,
        "frames_with_raster_jobs": raster_frames,
    });
    Ok(HtmlOut { scene, jobs, neutral, stats })
}

fn nodes_clear(scene: &mut Scene, node: usize) {
    scene.nodes[node].content = None;
}
