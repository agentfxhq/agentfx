//! `agentfx storyboard`: a director's storyboard of a compiled video. One hand-drawn sketch
//! panel per shot (the main layers as outlines, tones and lettering at a representative time),
//! motion arrows derived from the animation tracks, camera moves as frame-in-frame or edge
//! arrows, and a note per shot (the author's `note`, else an automatic summary). CPU only
//! (tiny-skia), seeded, deterministic.
//!
//! What a shot is (docs/SPEC.md, "Storyboard"):
//! - every scene starts a shot; its transition (`in=`) is the cut into it;
//! - inside a scene, `@t note "..."` starts a shot at t (author-defined breaks win);
//! - otherwise shots are found from the IR: the visible "mass" of the scene's drawables
//!   (area x opacity x trim, inside the frame) is sampled every 0.1 s, and a large turnover
//!   within about a second (things appearing/disappearing) is a break: abrupt = CUT,
//!   gradual = CONT. (or DISSOLVE when one image replaces another); a full-frame flash at the
//!   break makes it a SMASH CUT.
pub mod hand;
pub mod sketch;

use crate::ir::{Ch, Ir, Kind, Layer};
use crate::motion::{self, Chans, A2};
use crate::values::{ColorRef, Paint, Rgba};
use serde::Serialize;
use sketch::{Ink, Pen, Sketch, P};
use std::collections::HashMap;

/// Laid-out geometry the board needs: rest boxes of every layer and text line boxes (scene px).
#[derive(Default, Clone)]
pub struct Layout {
    pub rects: HashMap<String, [f64; 4]>,
    pub lines: HashMap<String, Vec<(String, [f64; 4])>>,
}

impl Layout {
    pub fn from_native(ir: &Ir, nat: &crate::native::Native) -> Layout {
        let mut l = Layout::default();
        ir.root.walk(&mut |x| {
            if let Some(r) = nat.rect(&x.id) {
                l.rects.insert(x.id.clone(), r);
            }
            if matches!(x.kind, Kind::Text { .. } | Kind::Counter { .. }) {
                let ls: Vec<(String, [f64; 4])> = nat.lines(&x.id).into_iter().filter(|li| !li.text.trim().is_empty() && li.rect[2] > 0.0).map(|li| (li.text.trim().to_string(), li.rect)).collect();
                if !ls.is_empty() {
                    l.lines.insert(x.id.clone(), ls);
                }
            }
        });
        l
    }
}

#[derive(Clone, Debug)]
pub struct Opts {
    /// panels per row (default by aspect: 3 landscape, 4 square, 5 portrait)
    pub cols: Option<usize>,
    pub seed: u64,
    pub title: String,
    /// cap on automatic shots for the whole video
    pub max_shots: usize,
}
impl Default for Opts {
    fn default() -> Self {
        Opts { cols: None, seed: 1, title: String::new(), max_shots: 16 }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Move {
    pub layer: String,
    pub name: String,
    /// path | grow | shrink | stretch-x | stretch-y | rotate | shake | pulse
    pub kind: String,
    /// screen trajectory of the subject's centre (scene px), first..last visible sample
    pub path: Vec<[f64; 2]>,
    /// bounding box at the start and at the end of the move (scene px)
    pub from: [f64; 4],
    pub to: [f64; 4],
    /// px for path/shake, ratio for scale/stretch, degrees for rotate, pulses for pulse
    pub amount: f64,
    /// members of a group of like movers (16 bars rising = one move with count 16)
    pub count: usize,
    #[serde(skip)]
    pub weight: f64,
    #[serde(skip)]
    pub t0: f64,
    #[serde(skip)]
    pub members: Vec<usize>,
    /// the name is the subject's own id (not a shared ancestor's): plural for groups
    #[serde(skip)]
    pub own: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct Cam {
    /// push-in | pull-out | pan-left | pan-right | tilt-up | tilt-down | roll | dolly-in | dolly-out | orbit
    pub kind: String,
    /// frame-in-frame rectangle (scene px): the end framing (push) or the start framing (pull)
    pub inner: Option<[f64; 4]>,
    /// zoom factor, pan px or degrees
    pub amount: f64,
    pub layer: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Shot {
    pub n: usize,
    pub scene: String,
    pub scene_ix: usize,
    pub start: f64,
    pub end: f64,
    /// time the panel shows
    pub rep: f64,
    /// transition into this shot (None for the first)
    pub trans: Option<String>,
    pub note: String,
    /// true when the note is the automatic summary
    pub auto: bool,
    pub moves: Vec<Move>,
    pub camera: Vec<Cam>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Board {
    pub title: String,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    pub duration: f64,
    pub shots: Vec<Shot>,
}

/// Scene transitions as written (`in=` of every scene, `cut` when absent), in scene order.
pub fn scene_transitions(doc: &crate::ast::Document) -> Vec<String> {
    doc.nodes.iter().filter(|n| n.kind == "scene").map(|n| n.props.get("in").map(|v| v.raw()).unwrap_or_else(|| "cut".into())).collect()
}

/// The storyboard label of a scene transition.
pub fn transition_label(tr: &str) -> String {
    let t = tr.split('(').next().unwrap_or(tr);
    match t {
        "cut" => "CUT",
        "fade" | "crossfade" | "dissolve" => "DISSOLVE",
        "zoom" => "ZOOM THROUGH",
        "magic" => "MAGIC MOVE",
        "iris" => "IRIS",
        x if x.starts_with("push") => "PUSH",
        x if x.starts_with("slide") => "SLIDE",
        x if x.starts_with("wipe") => "WIPE",
        _ => "CUT",
    }
    .to_string()
}

// ---------------------------------------------------------------- analysis

#[derive(Clone, Debug)]
enum Shape {
    /// rect / ellipse / filled group; radius
    Box { ellipse: bool, radius: f64 },
    /// path subpaths in rest scene coords (closed flag)
    Lines(Vec<(Vec<P>, bool)>),
    Text,
    Media { video: bool },
    Raw,
    Cloud,
    Fluid,
    S3d,
}

#[derive(Clone, Debug)]
struct Item {
    li: usize,
    scene: usize,
    /// visual mass at rest, px^2
    mass0: f64,
    flash: bool,
    shape: Shape,
    /// fill luminance (0..1) and alpha, None = no fill
    tone: Option<(f64, f64)>,
    stroke_w: f64,
}

struct SceneR {
    id: String,
    root: String,
    world: String,
    start: f64,
    end: f64,
    bg: f64,
}

const DT: f64 = 0.1;
const MOVE_CH: [Ch; 8] = [Ch::X, Ch::Y, Ch::Px, Ch::Py, Ch::Scale, Ch::Sx, Ch::Sy, Ch::Rot];

pub struct Storyboard<'a> {
    ir: &'a Ir,
    ch: Chans<'a>,
    lay: &'a Layout,
    order: Vec<&'a Layer>,
    idx: HashMap<String, usize>,
    parent: Vec<Option<usize>>,
    kids: Vec<Vec<usize>>,
    scene_of: Vec<Option<usize>>,
    scenes: Vec<SceneR>,
    items: Vec<Item>,
    item_of: HashMap<usize, usize>,
    times: Vec<f64>,
    worlds: Vec<HashMap<String, (A2, f64)>>,
    /// [sample][item] visible mass as a fraction of the frame
    mass: Vec<Vec<f64>>,
    fa: f64,
    diag: f64,
    pub board: Board,
    opts: Opts,
}

fn lum(c: &Rgba) -> f64 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

fn paint_tone(p: &Paint) -> Option<(f64, f64)> {
    let lit = |c: &ColorRef| match c {
        ColorRef::Lit(c) => Some(c.clone()),
        ColorRef::Token(_) => None,
    };
    let cols: Vec<Rgba> = match p {
        Paint::Solid(c) => lit(c).into_iter().collect(),
        Paint::Linear { stops, .. } | Paint::Radial { stops, .. } | Paint::Conic { stops, .. } => stops.iter().filter_map(|s| lit(&s.color)).collect(),
        Paint::Mesh { colors, .. } => colors.iter().filter_map(lit).collect(),
        Paint::Image { .. } => return Some((0.5, 1.0)),
    };
    if cols.is_empty() {
        return Some((0.5, 1.0));
    }
    let n = cols.len() as f64;
    let a = cols.iter().map(|c| c.a).sum::<f64>() / n;
    if a < 0.02 {
        return None;
    }
    Some((cols.iter().map(lum).sum::<f64>() / n, a))
}

fn area(r: &[f64; 4]) -> f64 {
    r[2].max(0.0) * r[3].max(0.0)
}
fn inter(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    (x1 - x0).max(0.0) * (y1 - y0).max(0.0)
}
fn union(a: Option<[f64; 4]>, b: [f64; 4]) -> [f64; 4] {
    match a {
        None => b,
        Some(a) => {
            let x0 = a[0].min(b[0]);
            let y0 = a[1].min(b[1]);
            let x1 = (a[0] + a[2]).max(b[0] + b[2]);
            let y1 = (a[1] + a[3]).max(b[1] + b[3]);
            [x0, y0, x1 - x0, y1 - y0]
        }
    }
}
fn center(r: &[f64; 4]) -> P {
    (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}
fn det(m: &A2) -> f64 {
    (m[0] * m[3] - m[1] * m[2]).abs()
}

/// Flattened subpaths of SVG path data mapped from its viewBox into the layer box.
fn flatten_path(d: &str, vb: (f64, f64), r: [f64; 4]) -> Vec<(Vec<P>, bool)> {
    let Ok(bp) = kurbo::BezPath::from_svg(d) else { return vec![] };
    let (sx, sy) = (if vb.0 > 0.0 { r[2] / vb.0 } else { 1.0 }, if vb.1 > 0.0 { r[3] / vb.1 } else { 1.0 });
    let mut out: Vec<(Vec<P>, bool)> = vec![];
    let mut cur: Vec<P> = vec![];
    let mut closed = false;
    let map = |p: kurbo::Point| (r[0] + p.x * sx, r[1] + p.y * sy);
    kurbo::flatten(bp.iter(), 0.4, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            if cur.len() > 1 {
                out.push((std::mem::take(&mut cur), closed));
            }
            cur = vec![map(p)];
            closed = false;
        }
        kurbo::PathEl::LineTo(p) => cur.push(map(p)),
        kurbo::PathEl::ClosePath => {
            if let Some(f) = cur.first().copied() {
                cur.push(f);
            }
            closed = true;
        }
        _ => {}
    });
    if cur.len() > 1 {
        out.push((cur, closed));
    }
    out
}

/// Short human name of a layer for notes: the author's id without trailing digits
/// (`stripe3` -> `stripe`), quoted text for text layers.
fn stem(name: &str) -> String {
    // `pt5_c` -> `pt`, `drop_c` -> `drop`, `stripe12` -> `stripe`
    let mut toks: Vec<&str> = name.split(['_', '-']).filter(|t| !t.is_empty()).collect();
    while toks.len() > 1 && toks.last().is_some_and(|t| t.chars().count() <= 1 || t.chars().all(|c| c.is_ascii_digit())) {
        toks.pop();
    }
    let toks: Vec<&str> = toks.iter().map(|t| t.trim_end_matches(|c: char| c.is_ascii_digit())).filter(|t| !t.is_empty()).collect();
    toks.join(" ")
}

fn plural(s: &str) -> String {
    if s.starts_with('"') || s.ends_with('s') {
        s.to_string()
    } else if s.ends_with('x') || s.ends_with("ch") || s.ends_with("sh") {
        format!("{s}es")
    } else if s.ends_with('y') && !s.ends_with("ay") && !s.ends_with("ey") && !s.ends_with("oy") {
        format!("{}ies", &s[..s.len() - 1])
    } else {
        format!("{s}s")
    }
}

/// "rises" -> "rise" for plural subjects (first word only: "fades in" -> "fade in").
fn verb(v: &str, many: bool) -> String {
    if !many {
        return v.to_string();
    }
    let (w, rest) = v.split_once(' ').map(|(a, b)| (a, format!(" {b}"))).unwrap_or((v, String::new()));
    let base = if w.ends_with("ches") || w.ends_with("shes") || w.ends_with("sses") || w.ends_with("xes") {
        &w[..w.len() - 2]
    } else if let Some(b) = w.strip_suffix('s') {
        b
    } else {
        w
    };
    format!("{base}{rest}")
}

fn ratio(z: f64) -> String {
    if z >= 10.0 { format!("{:.0}", z) } else { format!("{:.1}", z) }
}

/// `0:04.8` (tenths only when needed).
pub fn fmt_time(t: f64) -> String {
    let t = (t * 10.0).round() / 10.0;
    let m = (t / 60.0).floor() as u64;
    let s = t - m as f64 * 60.0;
    if (s - s.round()).abs() < 0.05 {
        format!("{m}:{:02}", s.round() as u64)
    } else {
        format!("{m}:{:04.1}", s)
    }
}

impl<'a> Storyboard<'a> {
    /// Analyse the compiled video: shots, moves, camera, notes. `transitions` = `in=` per scene
    /// (`scene_transitions`).
    pub fn new(ir: &'a Ir, lay: &'a Layout, transitions: &[String], opts: Opts) -> Storyboard<'a> {
        let ch = Chans::new(ir);
        let mut order: Vec<&Layer> = vec![];
        let mut parent: Vec<Option<usize>> = vec![];
        fn walk<'b>(l: &'b Layer, p: Option<usize>, order: &mut Vec<&'b Layer>, parent: &mut Vec<Option<usize>>) {
            let me = order.len();
            order.push(l);
            parent.push(p);
            for c in &l.children {
                walk(c, Some(me), order, parent);
            }
        }
        walk(&ir.root, None, &mut order, &mut parent);
        let idx: HashMap<String, usize> = order.iter().enumerate().map(|(i, l)| (l.id.clone(), i)).collect();
        // scenes: roots tagged `scene` with `start:<s>`
        let mut scenes: Vec<SceneR> = vec![];
        for l in order.iter() {
            if l.tags.iter().any(|t| t == "scene") {
                let st = l.tags.iter().find_map(|t| t.strip_prefix("start:")).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let id = l.id.trim_start_matches("scene-").to_string();
                let bg = l.style.fill.as_ref().and_then(paint_tone).map(|t| t.0).unwrap_or_else(|| lum(&ir.bg));
                scenes.push(SceneR { world: format!("world-{id}"), id, root: l.id.clone(), start: st, end: ir.duration, bg });
            }
        }
        scenes.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
        for i in 0..scenes.len() {
            scenes[i].end = scenes.get(i + 1).map(|s| s.start).unwrap_or(ir.duration);
        }
        if scenes.is_empty() {
            scenes.push(SceneR { id: "video".into(), root: ir.root.id.clone(), world: String::new(), start: 0.0, end: ir.duration, bg: lum(&ir.bg) });
        }
        let mut scene_of: Vec<Option<usize>> = vec![None; order.len()];
        for i in 0..order.len() {
            let mut cur = Some(i);
            while let Some(c) = cur {
                if let Some(si) = scenes.iter().position(|s| s.root == order[c].id) {
                    scene_of[i] = Some(si);
                    break;
                }
                cur = parent[c];
            }
        }
        if scenes.len() == 1 && scenes[0].root == ir.root.id {
            scene_of.iter_mut().for_each(|s| *s = Some(0));
        }
        let mut kids: Vec<Vec<usize>> = vec![vec![]; order.len()];
        for (i, p) in parent.iter().enumerate() {
            if let Some(p) = p {
                kids[*p].push(i);
            }
        }
        let fa = ir.w as f64 * ir.h as f64;
        let diag = ((ir.w as f64).powi(2) + (ir.h as f64).powi(2)).sqrt();
        let mut sb = Storyboard {
            ir, ch, lay, order, idx, parent, kids, scene_of, scenes, items: vec![], item_of: HashMap::new(), times: vec![], worlds: vec![], mass: vec![], fa, diag,
            board: Board { title: opts.title.clone(), w: ir.w, h: ir.h, fps: ir.fps, duration: ir.duration, shots: vec![] },
            opts,
        };
        sb.collect_items();
        sb.sample();
        sb.shots(transitions);
        sb
    }

    fn rect(&self, li: usize) -> [f64; 4] {
        self.lay.rects.get(&self.order[li].id).copied().unwrap_or([0.0; 4])
    }

    fn hidden(&self, l: &Layer) -> bool {
        l.style.time_map || l.style.screen_src || l.style.fluid_hidden || l.tags.iter().any(|t| t == "caption" || t == "hotspot")
    }

    fn in_hidden(&self, li: usize) -> bool {
        let mut cur = Some(li);
        while let Some(c) = cur {
            if self.hidden(self.order[c]) {
                return true;
            }
            cur = self.parent[c];
        }
        false
    }

    fn collect_items(&mut self) {
        for li in 0..self.order.len() {
            let l = self.order[li];
            let Some(scene) = self.scene_of[li] else { continue };
            if l.tags.iter().any(|t| t == "scene") || self.in_hidden(li) {
                continue;
            }
            let r = self.rect(li);
            let ra = area(&r);
            let tone = l.style.fill.as_ref().and_then(paint_tone);
            let sw = l.style.stroke_w.max(if l.style.stroke.is_some() || l.style.border.is_some() { 1.0 } else { 0.0 });
            let perim = 2.0 * (r[2] + r[3]);
            let (shape, mass0) = match &l.kind {
                Kind::Rect | Kind::Group => {
                    if matches!(l.kind, Kind::Group) && tone.is_none() && l.style.border.is_none() {
                        continue;
                    }
                    let m = if tone.is_some() { ra } else { perim * sw.max(2.0) };
                    (Shape::Box { ellipse: false, radius: l.style.radius }, m)
                }
                Kind::Ellipse => (Shape::Box { ellipse: true, radius: 0.0 }, if tone.is_some() { ra * 0.785 } else { perim * 0.8 * sw.max(2.0) }),
                Kind::Path { d, vb, .. } => {
                    let sub = flatten_path(d, *vb, r);
                    let len: f64 = sub.iter().map(|(p, _)| sketch::poly_len(p)).sum();
                    let closed = sub.iter().any(|s| s.1);
                    let m = len * sw.max(2.0) + if closed && tone.is_some() { ra * 0.6 } else { 0.0 };
                    (Shape::Lines(sub), m)
                }
                Kind::Text { .. } | Kind::Counter { .. } => (Shape::Text, ra * 0.45),
                Kind::Image { .. } => (Shape::Media { video: false }, ra),
                // a procedural image (`gen`): drawn like a still image panel
                Kind::Gen(_) => (Shape::Media { video: false }, ra),
                Kind::Video { .. } | Kind::Lottie { .. } => (Shape::Media { video: true }, ra),
                Kind::Html { .. } | Kind::Svg { .. } => (Shape::Raw, ra * 0.5),
                Kind::Cloud(_) => (Shape::Cloud, ra.min(self.fa) * 0.3),
                Kind::Fluid(_) => (Shape::Fluid, ra.min(self.fa) * 0.25),
                Kind::Scene3d(_) => (Shape::S3d, ra),
                Kind::Obj3d(_) => continue,
            };
            // a flash: big, light, normally invisible full-frame plate that pulses
            let big = ra >= self.fa * 0.85;
            let light = tone.map(|t| t.0 > 0.55).unwrap_or(false) || matches!(l.style.blend.as_deref(), Some("screen" | "add" | "plus-lighter" | "lighten" | "color-dodge"));
            let starts_hidden = self.ch.track(&l.id, Ch::Opacity).map(|t| t.keys.first().map(|k| k.v.n()).unwrap_or(t.base.n()) < 0.05).unwrap_or(false);
            let flash = big && light && starts_hidden && matches!(shape, Shape::Box { .. });
            let it = Item { li, scene, mass0, flash, shape, tone, stroke_w: sw };
            self.item_of.insert(li, self.items.len());
            self.items.push(it);
        }
    }

    fn world_at(&self, t: f64) -> HashMap<String, (A2, f64)> {
        let rf = |id: &str| self.lay.rects.get(id).copied();
        motion::world(self.ir, &self.ch, &rf, &HashMap::new(), t)
    }

    fn trim_frac(&self, id: &str, t: f64) -> f64 {
        let a = if self.ch.has(id, Ch::TrimStart) { self.ch.val(id, Ch::TrimStart, t) } else { 0.0 };
        let b = if self.ch.has(id, Ch::TrimEnd) { self.ch.val(id, Ch::TrimEnd, t) } else { 1.0 };
        (b - a).clamp(0.0, 1.0)
    }

    fn item_mass(&self, it: &Item, w: &HashMap<String, (A2, f64)>, t: f64) -> f64 {
        let l = self.order[it.li];
        let Some((m, op)) = w.get(&l.id) else { return 0.0 };
        if *op < 0.02 {
            return 0.0;
        }
        let r = self.rect(it.li);
        let bb = motion::a2_bbox(m, r);
        let frame = [0.0, 0.0, self.ir.w as f64, self.ir.h as f64];
        let vis = if area(&bb) < 1.0 {
            let c = center(&bb);
            if c.0 >= 0.0 && c.1 >= 0.0 && c.0 <= frame[2] && c.1 <= frame[3] { 1.0 } else { 0.0 }
        } else {
            inter(&bb, &frame) / area(&bb)
        };
        let trim = if matches!(it.shape, Shape::Lines(_)) { self.trim_frac(&l.id, t) } else { 1.0 };
        let reveal = if self.ch.has(&l.id, Ch::Reveal) { self.ch.val(&l.id, Ch::Reveal, t).clamp(0.0, 1.0) } else { 1.0 };
        (it.mass0 * det(m).min(25.0) * op.min(1.0) * vis * trim * reveal / self.fa).min(1.2)
    }

    fn sample(&mut self) {
        let n = ((self.ir.duration / DT).floor() as usize).max(1);
        for k in 0..=n {
            let t = (k as f64 * DT).min(self.ir.duration - 1e-3).max(0.0);
            let w = self.world_at(t);
            let row: Vec<f64> = self.items.iter().map(|it| if it.flash { 0.0 } else { self.item_mass(it, &w, t) }).collect();
            self.times.push(t);
            self.worlds.push(w);
            self.mass.push(row);
        }
    }

    fn k_of(&self, t: f64) -> usize {
        ((t / DT).round() as usize).min(self.times.len() - 1)
    }

    fn scene_mass(&self, k: usize, si: usize) -> f64 {
        self.items.iter().enumerate().filter(|(_, it)| it.scene == si).map(|(i, _)| self.mass[k][i]).sum()
    }

    /// Turnover between samples k-1 and k inside scene si: (total, gained, lost).
    fn change(&self, k: usize, si: usize) -> (f64, f64, f64) {
        if k == 0 {
            return (0.0, 0.0, 0.0);
        }
        let (mut g, mut l) = (0.0, 0.0);
        for (i, it) in self.items.iter().enumerate() {
            if it.scene != si {
                continue;
            }
            let d = self.mass[k][i].min(1.0) - self.mass[k - 1][i].min(1.0);
            if d > 0.0 { g += d } else { l -= d }
        }
        (g + l, g, l)
    }

    fn flash_near(&self, t: f64, si: usize) -> bool {
        let (k0, k1) = (self.k_of((t - 0.4).max(0.0)), self.k_of(t + 0.4));
        self.items.iter().filter(|it| it.flash && it.scene == si).any(|it| (k0..=k1).any(|k| self.worlds[k].get(&self.order[it.li].id).map(|w| w.1 >= 0.35).unwrap_or(false)))
    }

    /// Automatic breaks inside a scene: (time, label, strength).
    fn auto_breaks(&self, si: usize) -> Vec<(f64, String, f64)> {
        let s = &self.scenes[si];
        let (ks, ke) = (self.k_of(s.start) + 1, self.k_of(s.end));
        if ke <= ks + 2 || s.end - s.start < 3.0 {
            return vec![];
        }
        let c: Vec<(f64, f64, f64)> = (0..self.times.len()).map(|k| if k >= ks && k <= ke { self.change(k, si) } else { (0.0, 0.0, 0.0) }).collect();
        let half = (0.5 / DT).round() as usize;
        let win = |k: usize| -> (f64, f64, f64, f64) {
            let (a, b) = (k.saturating_sub(half).max(ks), (k + half).min(ke));
            let mut tot = (0.0, 0.0, 0.0, 0.0);
            for j in a..=b {
                tot.0 += c[j].0;
                tot.1 += c[j].1;
                tot.2 += c[j].2;
                tot.3 = f64::max(tot.3, c[j].0);
            }
            tot
        };
        let mut cands: Vec<(usize, f64)> = vec![];
        let edge = 1.2;
        for k in ks..=ke {
            let t = self.times[k];
            if t < s.start + edge || t > s.end - edge {
                continue;
            }
            let w = win(k).0;
            let prev = if k > ks { win(k - 1).0 } else { 0.0 };
            let next = if k < ke { win(k + 1).0 } else { 0.0 };
            if w >= 0.16 && w >= prev && w > next {
                cands.push((k, w));
            }
        }
        cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
        let dur = s.end - s.start;
        let max_n = ((dur / 2.6).floor() as usize).saturating_sub(1).min(7);
        let min_gap = 2.0;
        let mut picked: Vec<(f64, String, f64)> = vec![];
        for (k, w) in cands {
            if picked.len() >= max_n {
                break;
            }
            // start of the change ramp
            let (a, b) = (k.saturating_sub(half).max(ks), (k + half).min(ke));
            let cmax = (a..=b).map(|j| c[j].0).fold(0.0, f64::max);
            let j0 = (a..=b).find(|&j| c[j].0 >= 0.2 * cmax).unwrap_or(k);
            let t = self.times[j0.saturating_sub(1).max(ks.saturating_sub(1))];
            if t < s.start + edge * 0.8 || t > s.end - edge * 0.8 {
                continue;
            }
            if picked.iter().any(|p| (p.0 - t).abs() < min_gap) {
                continue;
            }
            let (tot, g, l, peak) = win(k);
            let hard = peak >= 0.5 * tot && peak >= 0.1;
            let label = if self.flash_near(t, si) && hard {
                "SMASH CUT"
            } else if hard {
                "CUT"
            } else if g >= 0.08 && l >= 0.08 {
                "DISSOLVE"
            } else {
                "CONT."
            };
            picked.push((t, label.to_string(), w));
        }
        picked.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        picked
    }

    fn label_at(&self, t: f64, si: usize) -> String {
        let k = self.k_of(t);
        let peak = (k..(k + 3).min(self.times.len())).map(|j| self.change(j, si).0).fold(0.0, f64::max);
        let hard = peak >= 0.1;
        if self.flash_near(t, si) && hard {
            "SMASH CUT".into()
        } else if hard {
            "CUT".into()
        } else {
            "CONT.".into()
        }
    }

    fn shots(&mut self, transitions: &[String]) {
        let mut bounds: Vec<(usize, f64, Option<String>, Option<String>, f64)> = vec![]; // scene, t, label, note, strength
        for si in 0..self.scenes.len() {
            let (s0, s1) = (self.scenes[si].start, self.scenes[si].end);
            let timed: Vec<&crate::ir::ShotNote> = self.ir.notes.iter().filter(|n| n.scene == si && n.timed && n.at < s1 - 0.05).collect();
            let scene_note = self.ir.notes.iter().find(|n| n.scene == si && !n.timed).map(|n| n.text.clone());
            let first_note = timed.iter().find(|n| n.at <= s0 + 0.25).map(|n| n.text.clone()).or(scene_note);
            let mut tr = if si == 0 { None } else { Some(transition_label(transitions.get(si).map(|s| s.as_str()).unwrap_or("cut"))) };
            if tr.as_deref() == Some("CUT") && self.flash_near(s0, si) {
                tr = Some("SMASH CUT".into());
            }
            if si > 0 && tr.as_deref() == Some("CUT") && self.flash_near(s0, si - 1) {
                tr = Some("SMASH CUT".into());
            }
            bounds.push((si, s0, tr, first_note, f64::INFINITY));
            if timed.iter().any(|n| n.at > s0 + 0.25) {
                for n in timed.iter().filter(|n| n.at > s0 + 0.25) {
                    bounds.push((si, n.at, Some(self.label_at(n.at, si)), Some(n.text.clone()), f64::INFINITY));
                }
            } else {
                for (t, label, w) in self.auto_breaks(si) {
                    bounds.push((si, t, Some(label), None, w));
                }
            }
        }
        // global cap on automatic breaks (weakest go first)
        let max = self.opts.max_shots.max(self.scenes.len());
        while bounds.len() > max {
            let Some((i, _)) = bounds.iter().enumerate().filter(|(_, b)| b.4.is_finite()).min_by(|a, b| a.1 .4.partial_cmp(&b.1 .4).unwrap()) else { break };
            bounds.remove(i);
        }
        bounds.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let mut shots = vec![];
        for (i, (si, t, tr, note, _)) in bounds.iter().enumerate() {
            let end = bounds.get(i + 1).map(|b| b.1).unwrap_or(self.ir.duration).min(self.scenes[*si].end.max(*t + 0.1));
            let end = if bounds.get(i + 1).map(|b| b.0 != *si).unwrap_or(true) { self.scenes[*si].end } else { end };
            let mut shot = Shot { n: i + 1, scene: self.scenes[*si].id.clone(), scene_ix: *si, start: *t, end, rep: *t, trans: tr.clone(), note: String::new(), auto: true, moves: vec![], camera: vec![] };
            shot.rep = self.rep_time(&shot);
            let (moves, cams) = self.motion(&shot);
            shot.moves = moves;
            shot.camera = cams;
            match note {
                Some(n) => {
                    shot.note = n.clone();
                    shot.auto = false;
                }
                None => shot.note = self.summary(&shot),
            }
            shots.push(shot);
        }
        self.board.shots = shots;
    }

    fn rep_time(&self, s: &Shot) -> f64 {
        let d = s.end - s.start;
        let a = s.start + 0.3 * d;
        let b = (s.end - (0.08 * d).min(0.15)).max(a);
        let (ka, kb) = (self.k_of(a), self.k_of(b).max(self.k_of(a)));
        let mut best = (f64::MIN, b);
        for k in ka..=kb {
            let t = self.times[k];
            if t < s.start || t >= s.end || self.flash_near(t, s.scene_ix) && (ka..=kb).any(|j| !self.flash_near(self.times[j], s.scene_ix)) {
                continue;
            }
            let x = (t - s.start) / d.max(1e-6);
            let sc = self.scene_mass(k, s.scene_ix) * (0.85 + 0.2 * x);
            if sc > best.0 + 1e-9 {
                best = (sc, t);
            }
        }
        best.1
    }

    fn own_moves(&self, li: usize) -> bool {
        let id = &self.order[li].id;
        MOVE_CH.iter().any(|c| self.ch.track(id, *c).is_some_and(|t| !t.is_static()))
    }

    /// Union of the world boxes of the visible drawables in a subtree at sample k.
    fn sub_bbox(&self, li: usize, k: usize) -> Option<[f64; 4]> {
        let mut bb = None;
        let mut stack = vec![li];
        let w = &self.worlds[k];
        let frame = [0.0, 0.0, self.ir.w as f64, self.ir.h as f64];
        while let Some(c) = stack.pop() {
            if let Some(&ii) = self.item_of.get(&c) {
                if !self.items[ii].flash && self.mass[k][ii] > 1e-5 {
                    if let Some((m, op)) = w.get(&self.order[c].id) {
                        if *op > 0.1 {
                            let b = motion::a2_bbox(m, self.rect(c));
                            if inter(&b, &frame) > 0.0 || area(&b) < 1.0 {
                                bb = Some(union(bb, b));
                            }
                        }
                    }
                }
            }
            stack.extend(self.kids[c].iter().copied());
        }
        bb
    }

    fn display_name(&self, li: usize) -> String {
        let l = self.order[li];
        if let Kind::Text { runs } = &l.kind {
            let s: String = runs.iter().map(|r| r.text.as_str()).collect::<String>();
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if !s.is_empty() {
                let short: String = s.chars().take(18).collect();
                return format!("\"{}{}\"", short, if s.chars().count() > 18 { "..." } else { "" });
            }
        }
        // the author's id, else the nearest named ancestor, else the kind
        let mut cur = Some(li);
        while let Some(c) = cur {
            let lc = self.order[c];
            if lc.tags.iter().any(|t| t == "scene") {
                break;
            }
            if let Some(n) = &lc.name {
                let s = stem(n);
                if s.chars().filter(|c| c.is_alphabetic()).count() >= 3 && !matches!(s.as_str(), "camera" | "fx" | "content") {
                    return s;
                }
            }
            cur = self.parent[c];
        }
        let r = self.rect(li);
        if matches!(l.kind, Kind::Rect | Kind::Ellipse) && r[2].max(r[3]) <= 14.0 {
            return "dot".into();
        }
        match &l.kind {
            Kind::Ellipse => "circle",
            Kind::Path { .. } => "line",
            Kind::Image { .. } => "image",
            Kind::Video { .. } => "video",
            Kind::Lottie { .. } => "lottie",
            Kind::Cloud(_) => "particles",
            Kind::Fluid(_) => "fluid",
            Kind::Scene3d(_) => "3d scene",
            Kind::Text { .. } | Kind::Counter { .. } => "text",
            _ => "shape",
        }
        .to_string()
    }

    /// Subject moves and camera moves over a shot.
    fn motion(&self, s: &Shot) -> (Vec<Move>, Vec<Cam>) {
        let si = s.scene_ix;
        let (ka, kb) = (self.k_of(s.start + 0.05).min(self.times.len() - 1), self.k_of((s.end - 0.05).max(s.start)).max(self.k_of(s.start + 0.05)));
        let frame = [0.0, 0.0, self.ir.w as f64, self.ir.h as f64];
        let mut moves: Vec<Move> = vec![];
        let mut cams: Vec<Cam> = vec![];
        let world_id = &self.scenes[si].world;
        for li in 0..self.order.len() {
            if self.scene_of[li] != Some(si) || self.in_hidden(li) {
                continue;
            }
            let l = self.order[li];
            let is_world = &l.id == world_id;
            if !self.own_moves(li) || l.tags.iter().any(|t| t == "scene") {
                continue;
            }
            if self.item_of.get(&li).map(|&i| self.items[i].flash).unwrap_or(false) {
                continue;
            }
            // visible span in the shot
            let vis: Vec<usize> = (ka..=kb).filter(|&k| self.worlds[k].get(&l.id).map(|w| w.1 > 0.1).unwrap_or(false) && (is_world || self.sub_bbox(li, k).is_some())).collect();
            if vis.len() < 3 {
                continue;
            }
            let (k0, k1) = (vis[0], *vis.last().unwrap());
            if self.times[k1] - self.times[k0] < 0.25 {
                continue;
            }
            let (m0, m1) = (self.worlds[k0][&l.id].0, self.worlds[k1][&l.id].0);
            let b0 = if is_world { frame } else { self.sub_bbox(li, k0).unwrap_or(frame) };
            let b1 = if is_world { frame } else { self.sub_bbox(li, k1).unwrap_or(frame) };
            let big = is_world || area(&motion::a2_bbox(&m1, self.rect(li))).min(area(&b1)) >= 0.5 * self.fa || area(&b1) >= 0.7 * self.fa && area(&b0) >= 0.5 * self.fa;
            let id = l.id.as_str();
            let v = |c: Ch, k: usize| self.ch.val(id, c, self.times[k]);
            let own_s = |k: usize| v(Ch::Scale, k) * (v(Ch::Sx, k) * v(Ch::Sy, k)).abs().sqrt();
            // the layer's own change, without what its parents do (a scene transition)
            let (p0, p1) = match self.parent[li].map(|p| &self.order[p].id) {
                Some(pid) => (self.worlds[k0].get(pid).map(|w| w.0).unwrap_or(motion::A2_ID), self.worlds[k1].get(pid).map(|w| w.0).unwrap_or(motion::A2_ID)),
                None => (motion::A2_ID, motion::A2_ID),
            };
            let (l0, l1) = (motion::a2_mul(&motion::a2_inv(&p0), &m0), motion::a2_mul(&motion::a2_inv(&p1), &m1));
            let rel = motion::a2_mul(&p1, &motion::a2_mul(&motion::a2_mul(&l1, &motion::a2_inv(&l0)), &motion::a2_inv(&p1)));
            if big {
                // camera-like: the whole picture scales / pans / rolls
                // a one-axis stretch of a backdrop is not a zoom
                let (zx, zy) = (rel[0].hypot(rel[1]), rel[2].hypot(rel[3]));
                let z = if zx.max(zy) / zx.min(zy).max(1e-6) > 1.3 { 1.0 } else { motion::a2_scale(&rel) };
                let inv = motion::a2_inv(&rel);
                let corners = sketch::rect_poly(frame);
                let bb = |m: &A2| {
                    let pts: Vec<P> = corners.iter().map(|p| motion::a2_apply(m, p.0, p.1)).collect();
                    let x0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
                    let y0 = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
                    let x1 = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
                    let y1 = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
                    [x0, y0, x1 - x0, y1 - y0]
                };
                if z >= 1.18 {
                    cams.push(Cam { kind: "push-in".into(), inner: Some(bb(&inv)), amount: z, layer: l.id.clone() });
                } else if z <= 0.85 {
                    cams.push(Cam { kind: "pull-out".into(), inner: Some(bb(&rel)), amount: 1.0 / z, layer: l.id.clone() });
                } else {
                    let c = (self.ir.w as f64 / 2.0, self.ir.h as f64 / 2.0);
                    let c2 = motion::a2_apply(&rel, c.0, c.1);
                    let (dx, dy) = (c2.0 - c.0, c2.1 - c.1);
                    if dx.abs() >= 0.08 * self.ir.w as f64 && dx.abs() >= dy.abs() {
                        cams.push(Cam { kind: if dx < 0.0 { "pan-right" } else { "pan-left" }.into(), inner: None, amount: dx.abs(), layer: l.id.clone() });
                    } else if dy.abs() >= 0.08 * self.ir.h as f64 {
                        cams.push(Cam { kind: if dy < 0.0 { "tilt-down" } else { "tilt-up" }.into(), inner: None, amount: dy.abs(), layer: l.id.clone() });
                    }
                }
                let dr = v(Ch::Rot, k1) - v(Ch::Rot, k0);
                if dr.abs() >= 20.0 {
                    cams.push(Cam { kind: "roll".into(), inner: None, amount: dr, layer: l.id.clone() });
                }
                continue;
            }
            let name = self.display_name(li);
            let path: Vec<[f64; 2]> = vis.iter().filter_map(|&k| self.sub_bbox(li, k)).map(|b| { let c = center(&b); [c.0, c.1] }).collect();
            let own = l.name.as_ref().map(|n| stem(n) == name).unwrap_or(false) || matches!(l.kind, Kind::Text { .. });
            let base = Move { layer: l.id.clone(), name: name.clone(), kind: String::new(), path: path.clone(), from: b0, to: b1, amount: 0.0, count: 1, weight: 0.0, t0: self.times[k0], members: vec![li], own };
            let size = (area(&b1).max(area(&b0)) / self.fa).sqrt().clamp(0.05, 1.0);
            // translation by the layer's own tracks, mapped through the parent
            let pm = self.parent[li].and_then(|p| self.worlds[k1].get(&self.order[p].id)).map(|w| w.0).unwrap_or(motion::A2_ID);
            let r = self.rect(li);
            let local = |k: usize| (v(Ch::X, k) + v(Ch::Px, k) * r[2], v(Ch::Y, k) + v(Ch::Py, k) * r[3]);
            let (la, lb) = (local(k0), local(k1));
            let d = (pm[0] * (lb.0 - la.0) + pm[2] * (lb.1 - la.1), pm[1] * (lb.0 - la.0) + pm[3] * (lb.1 - la.1));
            let net = (d.0 * d.0 + d.1 * d.1).sqrt();
            let mut plen = 0.0;
            for w in vis.windows(2) {
                let (p, q) = (local(w[0]), local(w[1]));
                plen += ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt() * motion::a2_scale(&pm);
            }
            if net >= 0.05 * self.diag && plen < 2.4 * net.max(1.0) {
                moves.push(Move { kind: "path".into(), amount: net, weight: net / self.diag * size.sqrt() * 2.0, ..base.clone() });
            } else if plen >= 0.08 * self.diag && plen >= 2.4 * net.max(1.0) {
                // a sawtooth (steps mostly one way, jumps back) scrolls; anything else bobs
                let (mut pos, mut neg, mut hor) = (0usize, 0usize, 0.0f64);
                let mut ver = 0.0f64;
                for w in vis.windows(2) {
                    let (p, q) = (local(w[0]), local(w[1]));
                    hor += (q.0 - p.0).abs();
                    ver += (q.1 - p.1).abs();
                }
                for w in vis.windows(2) {
                    let (p, q) = (local(w[0]), local(w[1]));
                    let d = if hor >= ver { q.0 - p.0 } else { q.1 - p.1 };
                    if d > 1e-3 { pos += 1 } else if d < -1e-3 { neg += 1 }
                }
                let scroll = pos.max(neg) >= 4 * pos.min(neg).max(1) && pos.min(neg) > 0 || pos.min(neg) == 0 && pos.max(neg) > 0;
                let dir = if pos >= neg { 1.0 } else { -1.0 };
                let kind = if scroll { if hor >= ver { "scroll-x" } else { "scroll-y" } } else { "shake" };
                moves.push(Move { kind: kind.into(), amount: plen * dir, weight: 0.35 * size.sqrt(), ..base.clone() });
            }
            // scale
            let (sa, sb) = (own_s(k0), own_s(k1));
            let (sxa, sxb, sya, syb) = (v(Ch::Sx, k0) * v(Ch::Scale, k0), v(Ch::Sx, k1) * v(Ch::Scale, k1), v(Ch::Sy, k0) * v(Ch::Scale, k0), v(Ch::Sy, k1) * v(Ch::Scale, k1));
            let rx = (sxb.abs().max(1e-3) / sxa.abs().max(1e-3)).max(1e-3);
            let ry = (syb.abs().max(1e-3) / sya.abs().max(1e-3)).max(1e-3);
            let svals: Vec<f64> = vis.iter().map(|&k| v(Ch::Scale, k).abs().max(1e-3).ln()).collect();
            let svar: f64 = svals.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
            let (smin, smax) = svals.iter().fold((f64::MAX, f64::MIN), |a, &x| (a.0.min(x), a.1.max(x)));
            let _ = (sa, sb);
            let lr = (v(Ch::Scale, k1).abs().max(1e-3) / v(Ch::Scale, k0).abs().max(1e-3)).ln();
            let pulse = svar >= 3.0 * lr.abs().max(0.05) && (smax - smin).exp() >= 1.2;
            if pulse {
                let pulses = (svar / (2.0 * (smax - smin).max(1e-3))).round().max(1.0);
                moves.push(Move { kind: "pulse".into(), amount: pulses, weight: 0.4 * size.sqrt(), ..base.clone() });
            }
            if (rx.ln() - ry.ln()).abs() >= 0.4 && rx.max(ry) >= 1.4 || (rx.ln() - ry.ln()).abs() >= 0.4 && rx.min(ry) <= 0.7 {
                let (kind, amt) = if (rx.ln()).abs() >= (ry.ln()).abs() { ("stretch-x", rx) } else { ("stretch-y", ry) };
                moves.push(Move { kind: kind.into(), amount: amt, weight: 0.5 * amt.ln().abs().min(2.0) * size.sqrt(), ..base.clone() });
            } else if !pulse && (lr.exp() >= 1.3 || lr.exp() <= 0.77) {
                let kind = if lr > 0.0 { "grow" } else { "shrink" };
                moves.push(Move { kind: kind.into(), amount: lr.exp(), weight: 0.6 * lr.abs().min(2.0) * size.sqrt(), ..base.clone() });
            }
            // rotation
            let dr = v(Ch::Rot, k1) - v(Ch::Rot, k0);
            if dr.abs() >= 25.0 {
                moves.push(Move { kind: "rotate".into(), amount: dr, weight: 0.45 * (dr.abs() / 180.0).min(2.0) * size.sqrt(), ..base });
            }
        }
        // 3D camera (scene3d camera channels)
        for li in 0..self.order.len() {
            if self.scene_of[li] != Some(si) {
                continue;
            }
            let l = self.order[li];
            let Kind::Scene3d(cfg) = &l.kind else { continue };
            let id = l.id.as_str();
            let cam = &cfg.scene.camera;
            let val = |c: Ch, base: f64, k: usize| if self.ch.has(id, c) { self.ch.val(id, c, self.times[k]) } else { base };
            let pos = |k: usize| [val(Ch::CamX, cam.pos[0], k), val(Ch::CamY, cam.pos[1], k), val(Ch::CamZ, cam.pos[2], k)];
            let tgt = |k: usize| [val(Ch::LookX, cam.target[0], k), val(Ch::LookY, cam.target[1], k), val(Ch::LookZ, cam.target[2], k)];
            let (p0, p1, t0, t1) = (pos(ka), pos(kb), tgt(ka), tgt(kb));
            let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
            let len = |a: [f64; 3]| (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
            let (v0, v1) = (sub(p0, t0), sub(p1, t1));
            let (d0, d1) = (len(v0).max(1e-6), len(v1).max(1e-6));
            let cosang = ((v0[0] * v1[0] + v0[1] * v1[1] + v0[2] * v1[2]) / (d0 * d1)).clamp(-1.0, 1.0);
            let ang = cosang.acos().to_degrees();
            let fov = (val(Ch::Fov, cam.fov, ka), val(Ch::Fov, cam.fov, kb));
            let c = (self.ir.w as f64 / 2.0, self.ir.h as f64 / 2.0);
            let fr = |z: f64| [c.0 - c.0 / z, c.1 - c.1 / z, self.ir.w as f64 / z, self.ir.h as f64 / z];
            if ang >= 15.0 {
                cams.push(Cam { kind: "orbit".into(), inner: None, amount: ang, layer: l.id.clone() });
            }
            let z = (d0 / d1) * (fov.0 / fov.1.max(1e-3));
            if z >= 1.18 {
                cams.push(Cam { kind: "dolly-in".into(), inner: Some(fr(z)), amount: z, layer: l.id.clone() });
            } else if z <= 0.85 {
                cams.push(Cam { kind: "dolly-out".into(), inner: Some(fr(1.0 / z)), amount: 1.0 / z, layer: l.id.clone() });
            }
        }
        // like movers (same kind, same parent, similar amount and direction) become one move
        let mut grouped: Vec<Move> = vec![];
        for m in moves {
            let li = m.members[0];
            let dir = |mv: &Move| {
                let (a, b) = (mv.path.first().copied().unwrap_or([0.0; 2]), mv.path.last().copied().unwrap_or([0.0; 2]));
                (b[1] - a[1]).atan2(b[0] - a[0])
            };
            let found = grouped.iter_mut().find(|g| {
                g.kind == m.kind && self.parent[g.members[0]] == self.parent[li] && (g.amount - m.amount).abs() <= 0.35 * g.amount.abs().max(m.amount.abs()).max(1e-6)
                    && (g.kind != "path" || { let d = (dir(g) - dir(&m)).abs(); d.min(std::f64::consts::TAU - d) < 0.4 })
            });
            match found {
                Some(g) => {
                    g.count += 1;
                    g.members.push(li);
                    g.weight = g.weight.max(m.weight) + 0.05;
                    g.from = union(Some(g.from), m.from);
                    g.to = union(Some(g.to), m.to);
                }
                None => grouped.push(m),
            }
        }
        grouped.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap().then(a.t0.partial_cmp(&b.t0).unwrap()));
        grouped.truncate(4);
        // one camera move of each kind, strongest first
        cams.sort_by(|a, b| b.amount.abs().partial_cmp(&a.amount.abs()).unwrap());
        let mut seen = vec![];
        cams.retain(|c| {
            let k = c.kind.split('-').next().unwrap_or("").to_string();
            if seen.contains(&k) { false } else { seen.push(k); true }
        });
        cams.truncate(2);
        (grouped, cams)
    }

    /// Automatic note: what enters, leaves, moves and what the camera does.
    fn summary(&self, s: &Shot) -> String {
        let si = s.scene_ix;
        let (ka, kb) = (self.k_of(s.start).min(self.times.len() - 1), self.k_of((s.end - 0.05).max(s.start)));
        // (time, weight, subject, singular verb, count)
        let mut ev: Vec<(f64, f64, String, String, usize)> = vec![];
        let mut by_subject: HashMap<(String, String), usize> = HashMap::new();
        let mut push = |ev: &mut Vec<(f64, f64, String, String, usize)>, t: f64, w: f64, who: String, what: &str, n: usize| {
            let key = (who.clone(), what.to_string());
            match by_subject.get(&key) {
                Some(&i) => {
                    ev[i].1 += w;
                    ev[i].4 += n;
                    ev[i].0 = ev[i].0.min(t);
                }
                None => {
                    by_subject.insert(key, ev.len());
                    ev.push((t, w, who, what.to_string(), n));
                }
            }
        };
        for c in &s.camera {
            let what = match c.kind.as_str() {
                "push-in" => "camera pushes in",
                "pull-out" => "camera pulls out",
                "pan-left" => "camera pans left",
                "pan-right" => "camera pans right",
                "tilt-up" => "camera tilts up",
                "tilt-down" => "camera tilts down",
                "roll" => "camera rolls",
                "dolly-in" => "camera dollies in",
                "dolly-out" => "camera dollies out",
                _ => "camera orbits",
            };
            push(&mut ev, s.start, 1.0, String::new(), what, 1);
        }
        for m in &s.moves {
            let what = match m.kind.as_str() {
                "path" => {
                    let (a, b) = (m.path.first().copied().unwrap_or([0.0; 2]), m.path.last().copied().unwrap_or([0.0; 2]));
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    if dy.abs() > 1.6 * dx.abs() {
                        if dy < 0.0 { "rises" } else { "drops" }
                    } else if dx.abs() > 1.6 * dy.abs() {
                        if dx < 0.0 { "slides left" } else { "slides right" }
                    } else {
                        "drifts"
                    }
                }
                "grow" => "grows",
                "shrink" => "shrinks",
                "stretch-x" | "stretch-y" => "stretches",
                "rotate" => if m.amount.abs() >= 300.0 { "spins" } else { "turns" },
                "shake" => "bobs",
                "scroll-x" | "scroll-y" => "scrolls",
                "pulse" => "pulses",
                _ => "moves",
            };
            push(&mut ev, m.t0, m.weight.max(0.05) + 0.1, m.name.clone(), what, if m.own { m.count } else { 1 });
        }
        // entrances and exits
        for (i, it) in self.items.iter().enumerate() {
            if it.scene != si || it.flash {
                continue;
            }
            let ms: Vec<f64> = (ka..=kb).map(|k| self.mass[k][i]).collect();
            let mx = ms.iter().copied().fold(0.0, f64::max);
            if mx < 0.006 {
                continue;
            }
            let first = ms[0];
            let last = *ms.last().unwrap();
            let id = &self.order[it.li].id;
            let name = self.display_name(it.li);
            // a group noun from a shared ancestor stays singular ("the kal spins")
            let one = if self.order[it.li].name.as_ref().map(|n| stem(n) == name).unwrap_or(false) { 1 } else { 0 };
            if first < 0.15 * mx {
                let j = ms.iter().position(|&m| m >= 0.5 * mx).unwrap_or(0);
                let t = self.times[ka + j];
                // what the scene transition brings in is not an entrance of its own
                let in_transition = (s.start - self.scenes[si].start).abs() < 1e-6 && s.trans.as_deref().is_some_and(|x| x != "CUT" && x != "SMASH CUT") && t - s.start < 0.8;
                if t - s.start > 0.15 && !in_transition {
                    let tr0 = self.trim_frac(id, self.times[ka]);
                    let w = self.ch.val(id, Ch::Scale, self.times[(ka + j.saturating_sub(2)).min(kb)]).abs();
                    let w_later = self.ch.val(id, Ch::Scale, self.times[kb]).abs();
                    let what = if matches!(it.shape, Shape::Lines(_)) && tr0 < 0.5 && self.ch.has(id, Ch::TrimEnd) {
                        "draws on"
                    } else if matches!(it.shape, Shape::Cloud) {
                        "appears"
                    } else if w < 0.7 * w_later {
                        "scales in"
                    } else {
                        "fades in"
                    };
                    push(&mut ev, t, mx, name.clone(), what, one);
                }
            } else if last < 0.15 * mx {
                let j = ms.iter().rposition(|&m| m >= 0.5 * mx).unwrap_or(0);
                let t = self.times[ka + j];
                if s.end - t > 0.25 {
                    push(&mut ev, t, mx * 0.8, name.clone(), "fades out", one);
                }
            }
        }
        // cloud forms and fluid emitters inside the shot
        for (i, it) in self.items.iter().enumerate() {
            if it.scene != si {
                continue;
            }
            let l = self.order[it.li];
            let name = self.display_name(it.li);
            let mx = (ka..=kb).map(|k| self.mass[k][i]).fold(0.0, f64::max);
            match &l.kind {
                Kind::Cloud(cfg) => {
                    for f in cfg.forms.iter().filter(|f| f.at >= s.start + 0.1 && f.at < s.end) {
                        let what = match f.shape.as_str() {
                            "text" => format!("spells \"{}\"", f.text.clone().unwrap_or_default()),
                            "cluster" => "gathers".into(),
                            "points" | "path" => "takes a new shape".into(),
                            "scatter" | "random" => "scatters".into(),
                            sh => format!("becomes a {sh}"),
                        };
                        push(&mut ev, f.at, 0.3 + mx, name.clone(), &what, 1);
                    }
                }
                Kind::Fluid(cfg) => {
                    for e in &cfg.emitters {
                        let (t0, t1, what, who) = match e {
                            crate::fluid::FluidEv::Pour { t0, t1, .. } => (*t0, *t1, "pours", name.clone()),
                            crate::fluid::FluidEv::Stir { t0, t1, .. } => (*t0, *t1, "swirls", name.clone()),
                            crate::fluid::FluidEv::Source { t0, t1, id, .. } => {
                                let src = self.idx.get(id).map(|&j| self.display_name(j)).unwrap_or_else(|| "ink".into());
                                (*t0, *t1, "melts into the fluid", src)
                            }
                        };
                        if t0 < s.end && t1 > s.start {
                            push(&mut ev, t0.max(s.start), 0.15, who, what, 0);
                        }
                    }
                }
                _ => {}
            }
        }
        if ev.is_empty() {
            // a hold: name the biggest things on screen
            let k = self.k_of(s.rep);
            let mut vis: Vec<(f64, String)> = self.items.iter().enumerate().filter(|(i, it)| it.scene == si && self.mass[k][*i] > 0.01).map(|(i, it)| (self.mass[k][i], self.display_name(it.li))).collect();
            vis.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            let mut names: Vec<String> = vec![];
            for (_, n) in vis {
                if !names.contains(&n) && names.len() < 2 {
                    names.push(n);
                }
            }
            return if names.is_empty() { "hold".into() } else { format!("hold on {}", names.join(" and ")) };
        }
        ev.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.partial_cmp(&b.0).unwrap()));
        ev.truncate(3);
        ev.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut parts: Vec<String> = vec![];
        for (_, _, who, what, n) in ev {
            // a plural id (`stars`) takes a plural verb too
            let many = n > 1 || (who.ends_with('s') && !who.ends_with("ss") && !who.starts_with('"'));
            let p = if who.is_empty() { what } else { format!("{} {}", if n > 1 { plural(&who) } else { who }, verb(&what, many)) };
            if !parts.contains(&p) {
                parts.push(p);
            }
        }
        let mut out = parts.join(", ");
        while out.chars().count() > 90 && parts.len() > 1 {
            parts.pop();
            out = parts.join(", ");
        }
        out
    }
}

// ---------------------------------------------------------------- drawing

struct PanelGeo {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    k: f64,
}
impl PanelGeo {
    fn p(&self, q: P) -> P {
        (self.x + q.0 * self.k, self.y + q.1 * self.k)
    }
    fn r(&self, r: [f64; 4]) -> [f64; 4] {
        [self.x + r[0] * self.k, self.y + r[1] * self.k, r[2] * self.k, r[3] * self.k]
    }
}

const HEADER_H: f64 = 150.0;
const TOP_LABEL: f64 = 40.0;
const NOTE_H: f64 = 96.0;
const ROW_GAP: f64 = 26.0;
const MARGIN_L: f64 = 72.0;
const MARGIN_R: f64 = 48.0;
const GUTTER: f64 = 72.0;

impl<'a> Storyboard<'a> {
    fn page_layout(&self) -> (usize, f64, f64, u32, u32) {
        let (w, h) = (self.ir.w as f64, self.ir.h as f64);
        let (def_cols, pw) = if h > w * 1.2 { (5usize, 250.0) } else if w > h * 1.2 { (3, 520.0) } else { (4, 380.0) };
        let n = self.board.shots.len().max(1);
        let cols = self.opts.cols.unwrap_or(def_cols).max(1).min(n);
        let ph = pw * h / w;
        let rows = n.div_ceil(cols);
        let pw_total = MARGIN_L + cols as f64 * pw + (cols as f64 - 1.0) * GUTTER + MARGIN_R;
        let page_w = pw_total.max(760.0);
        let page_h = HEADER_H + rows as f64 * (TOP_LABEL + ph + NOTE_H + ROW_GAP) + 30.0;
        (cols, pw, ph, page_w.ceil() as u32, page_h.ceil() as u32)
    }

    /// Draw the board to a PNG.
    pub fn render_png(&self, out: &std::path::Path) -> anyhow::Result<()> {
        let sk = self.render();
        if let Some(d) = out.parent() {
            if !d.as_os_str().is_empty() {
                std::fs::create_dir_all(d)?;
            }
        }
        sk.save_png(out)
    }

    pub fn render(&self) -> Sketch {
        let (cols, pw, ph, page_w, page_h) = self.page_layout();
        let seed = self.opts.seed;
        let mut sk = Sketch::new(page_w, page_h);
        sk.paper_grain(sketch::mix(seed, 0xA11));
        self.header(&mut sk, page_w as f64);
        for (i, shot) in self.board.shots.iter().enumerate() {
            let (r, c) = (i / cols, i % cols);
            let x = MARGIN_L + c as f64 * (pw + GUTTER);
            let y = HEADER_H + r as f64 * (TOP_LABEL + ph + NOTE_H + ROW_GAP) + TOP_LABEL;
            let g = PanelGeo { x, y, w: pw, h: ph, k: pw / self.ir.w as f64 };
            self.panel(&mut sk, shot, &g, sketch::mix(seed, 1000 + i as u64));
        }
        sk
    }

    fn header(&self, sk: &mut Sketch, page_w: f64) {
        let seed = sketch::mix(self.opts.seed, 7);
        let title = if self.board.title.is_empty() { "untitled".to_string() } else { self.board.title.clone() };
        let ink = Pen::ink(2.6);
        let tw = sk.text(&title, MARGIN_L, 62.0, 30.0, &ink, seed, 0.0);
        sk.line((MARGIN_L - 6.0, 76.0), (MARGIN_L + tw + 10.0, 74.0), &Pen::ink(1.8).overshoot(8.0), sketch::mix(seed, 2));
        let shots = self.board.shots.len();
        let sub = format!("storyboard \u{00B7} {} \u{00B7} {}\u{00D7}{} \u{00B7} {} fps \u{00B7} {} shot{}", fmt_time(self.board.duration), self.board.w, self.board.h, (self.board.fps * 100.0).round() / 100.0, shots, if shots == 1 { "" } else { "s" });
        sk.text(&sub, MARGIN_L, 104.0, 12.5, &Pen::pencil(1.4), sketch::mix(seed, 3), 0.0);
        // legend (top right): subject move, camera, ghost
        let lx = page_w - MARGIN_R - 330.0;
        if lx > MARGIN_L + tw + 40.0 {
            let ly = 50.0;
            sk.fat_arrow(&[(lx, ly + 4.0), (lx + 26.0, ly - 2.0), (lx + 52.0, ly + 2.0)], 9.0, sketch::RED.alpha(0.85), &Pen::ink(1.2), sketch::mix(seed, 10));
            sk.text("subject move", lx + 62.0, ly + 6.0, 10.5, &Pen::pencil(1.2), sketch::mix(seed, 11), 0.0);
            let lx2 = lx + 190.0;
            sk.rect([lx2, ly - 8.0, 30.0, 18.0], &Pen::ink(1.4).color(sketch::BLUE), sketch::mix(seed, 12));
            sk.thin_arrow(&[(lx2 - 8.0, ly - 14.0), (lx2 + 2.0, ly - 6.0)], &Pen::ink(1.1).color(sketch::BLUE), 4.0, sketch::mix(seed, 13));
            sk.text("camera", lx2 + 40.0, ly + 6.0, 10.5, &Pen::pencil(1.2), sketch::mix(seed, 14), 0.0);
            let ly2 = ly + 30.0;
            sk.rect([lx, ly2 - 8.0, 30.0, 16.0], &Pen::pencil(1.1).dashed(4.0, 3.0), sketch::mix(seed, 15));
            sk.text("start position", lx + 40.0, ly2 + 5.0, 10.5, &Pen::pencil(1.2), sketch::mix(seed, 16), 0.0);
            sk.text("time  shot  note", lx2, ly2 + 5.0, 10.5, &Pen::pencil(1.2), sketch::mix(seed, 17), 0.0);
        }
    }

    fn panel(&self, sk: &mut Sketch, shot: &Shot, g: &PanelGeo, seed: u64) {
        let pr = [g.x, g.y, g.w, g.h];
        // panel paper and the scene's tone
        sk.fill(&[sketch::rect_poly(pr)], sketch::PANEL);
        let bg = self.scenes[shot.scene_ix].bg;
        sk.clip_rect(Some(pr));
        let dark = (1.0 - bg).clamp(0.0, 1.0);
        if dark > 0.35 {
            // marker wash: overlapping diagonal strokes
            sk.fill(&[sketch::rect_poly(pr)], Ink { a: (0.07 * dark) as f32, ..sketch::GRAPHITE });
            let marker = Pen { ink: Ink { a: (0.045 * dark) as f32, ..sketch::GRAPHITE }, w: g.h * 0.16, wobble: 3.0, overshoot: 0.0, passes: 1, taper: 0.2, dash: None };
            for i in 0..7 {
                let yy = g.y + g.h * (i as f64 / 6.0) + sketch::rnds(seed, 50 + i) * 6.0;
                sk.stroke(&sketch::resample(&[(g.x - 10.0, yy + 18.0), (g.x + g.w + 10.0, yy - 18.0)], 8.0), &marker, sketch::mix(seed, 60 + i));
            }
        }
        let k = self.k_of(shot.rep);
        self.draw_content(sk, shot, g, k, seed);
        // ghosts at the start position of movers
        for (mi, m) in shot.moves.iter().enumerate() {
            if m.kind == "path" {
                for (j, &li) in m.members.iter().take(3).enumerate() {
                    let ka = self.k_of(m.t0);
                    if let Some(b) = self.sub_bbox(li, ka) {
                        let r = g.r(b);
                        if r[2] > 3.0 && r[3] > 3.0 {
                            sk.rect(r, &Pen::pencil(1.1).dashed(5.0, 4.0).overshoot(1.5), sketch::mix(seed, 300 + mi as u64 * 10 + j as u64));
                        }
                    }
                }
            }
        }
        // allow arrows a little past the frame edge
        sk.clip_rect(Some([g.x - 14.0, g.y - 14.0, g.w + 28.0, g.h + 28.0]));
        for (mi, m) in shot.moves.iter().take(3).enumerate() {
            self.draw_move(sk, m, g, sketch::mix(seed, 500 + mi as u64));
        }
        sk.clip_rect(Some(pr));
        for (ci, c) in shot.camera.iter().enumerate() {
            self.draw_cam(sk, c, g, ci, sketch::mix(seed, 700 + ci as u64));
        }
        sk.clip_rect(None);
        // the frame: heavy ink, two passes
        sk.rect(pr, &Pen::ink(2.3).passes(2).overshoot(7.0), sketch::mix(seed, 1));
        // shot number badge, scene, time
        let (bx, by) = (g.x + 4.0, g.y - 20.0);
        sk.ellipse(bx + 12.0, by, 14.0, 12.5, &Pen::ink(1.6), sketch::mix(seed, 2));
        sk.text(&shot.n.to_string(), bx + 12.0, by + 6.0, 12.5, &Pen::ink(1.6), sketch::mix(seed, 3), 0.5);
        if self.scenes.len() > 1 {
            sk.text(&format!("sc. {}", shot.scene), bx + 34.0, by + 5.5, 10.5, &Pen::pencil(1.2), sketch::mix(seed, 4), 0.0);
        }
        let tr = format!("{} \u{2013} {}", fmt_time(shot.start), fmt_time(shot.end));
        sk.text(&tr, g.x + g.w, by + 5.5, 11.5, &Pen::pencil(1.3), sketch::mix(seed, 5), 1.0);
        // note
        let cap = 12.5;
        let lines = hand::wrap(&shot.note, cap, g.w - 6.0, 3);
        for (i, ln) in lines.iter().enumerate() {
            sk.text(ln, g.x + 2.0, g.y + g.h + 28.0 + i as f64 * 23.0, cap, &Pen::ink(1.5), sketch::mix(seed, 20 + i as u64), 0.0);
        }
        // transition into the shot, written in the gutter to its left
        if let Some(t) = &shot.trans {
            let cx = g.x - GUTTER / 2.0;
            let cy = g.y + g.h * 0.45;
            let pen = Pen::ink(1.6);
            sk.text_rot(t, cx + 5.0, cy, 12.0, &pen, sketch::mix(seed, 30), 0.5, -90.0);
            let ay = g.y + g.h * 0.86;
            let x0 = cx - 18.0;
            let x1 = (g.x - 8.0).min(cx + 24.0);
            let hard = t.contains("CUT");
            let p = if hard { Pen::ink(1.6) } else { Pen::ink(1.4).dashed(5.0, 3.5) };
            sk.thin_arrow(&sketch::resample(&[(x0, ay), (x1, ay - 1.0)], 4.0), &p, 6.0, sketch::mix(seed, 31));
        }
    }

    fn draw_content(&self, sk: &mut Sketch, shot: &Shot, g: &PanelGeo, k: usize, seed: u64) {
        let w = &self.worlds[k];
        let t = self.times[k];
        let bg = self.scenes[shot.scene_ix].bg;
        let dark_scene = bg < 0.35;
        for (ii, it) in self.items.iter().enumerate() {
            if it.scene != shot.scene_ix || it.flash || self.mass[k][ii] <= 0.0 && !matches!(it.shape, Shape::Fluid) {
                continue;
            }
            let l = self.order[it.li];
            let Some((m, op)) = w.get(&l.id) else { continue };
            if *op < 0.1 {
                continue;
            }
            let s = sketch::mix(seed, sketch::hash_str(&l.id));
            let r = self.rect(it.li);
            let fade = (0.35 + 0.65 * op.min(1.0)).min(1.0);
            let quad: Vec<P> = sketch::rect_poly(r).iter().map(|p| g.p(motion::a2_apply(m, p.0, p.1))).collect();
            let bb = g.r(motion::a2_bbox(m, r));
            if bb[2] < 0.6 && bb[3] < 0.6 {
                continue;
            }
            let big = area(&bb) >= 0.45 * g.w * g.h;
            let axis = m[1].abs() < 1e-3 && m[2].abs() < 1e-3;
            let outline = Pen::pencil((it.stroke_w * g.k * 0.9).clamp(1.0, 2.6)).color(sketch::GRAPHITE.alpha(fade));
            match &it.shape {
                Shape::Box { ellipse, radius } => {
                    let region: Vec<P> = if *ellipse {
                        let c = (quad.iter().map(|p| p.0).sum::<f64>() / 4.0, quad.iter().map(|p| p.1).sum::<f64>() / 4.0);
                        let ax = (sketch::dist(quad[0], quad[1]) / 2.0, sketch::dist(quad[1], quad[2]) / 2.0);
                        sketch::ellipse_poly(c.0, c.1, ax.0, ax.1, 48)
                    } else if axis && *radius > 0.5 {
                        sketch::rounded_rect_poly(bb, radius * g.k * det(m).sqrt())
                    } else {
                        quad.clone()
                    };
                    self.tone(sk, &region, it.tone, big, dark_scene, fade, s);
                    let full = big && bb[2] >= g.w * 0.95 && bb[3] >= g.h * 0.95;
                    if full {
                        continue;
                    }
                    let pen = if it.tone.map(|t| t.0 > 0.55).unwrap_or(false) && dark_scene { Pen::ink(1.7).color(sketch::INK.alpha(fade)) } else { outline };
                    let pen = if big { pen.width(1.1).color(sketch::GRAPHITE.alpha(0.6 * fade)) } else { pen };
                    if *ellipse {
                        let c = center(&bb);
                        if axis {
                            sk.ellipse(c.0, c.1, bb[2] / 2.0, bb[3] / 2.0, &pen, s);
                        } else {
                            sk.stroke_closed(&region, &pen, s);
                        }
                    } else if axis && *radius <= 0.5 {
                        sk.rect(bb, &pen, s);
                    } else {
                        sk.stroke_closed(&region, &pen, s);
                    }
                }
                Shape::Lines(sub) => {
                    let (a, b) = (if self.ch.has(&l.id, Ch::TrimStart) { self.ch.val(&l.id, Ch::TrimStart, t) } else { 0.0 }, if self.ch.has(&l.id, Ch::TrimEnd) { self.ch.val(&l.id, Ch::TrimEnd, t) } else { 1.0 });
                    for (j, (pts, closed)) in sub.iter().enumerate() {
                        let pp: Vec<P> = pts.iter().map(|p| g.p(motion::a2_apply(m, p.0, p.1))).collect();
                        if *closed && it.tone.is_some() && a <= 0.0 && b >= 1.0 {
                            self.tone(sk, &pp, it.tone, big, dark_scene, fade, sketch::mix(s, j as u64));
                        }
                        let pp = if a > 0.0 || b < 1.0 { sketch::trim(&pp, a, b) } else { pp };
                        if pp.len() >= 2 {
                            sk.stroke(&sketch::resample(&pp, 3.0), &outline, sketch::mix(s, 100 + j as u64));
                        }
                    }
                }
                Shape::Text => self.draw_text(sk, it, m, g, fade, s),
                Shape::Media { video } => {
                    self.tone(sk, &quad, Some((0.6, 1.0)), big, dark_scene, fade, s);
                    sk.stroke_closed(&quad, &outline, s);
                    sk.line(quad[0], quad[2], &outline.width(1.0), sketch::mix(s, 1));
                    sk.line(quad[1], quad[3], &outline.width(1.0), sketch::mix(s, 2));
                    let c = center(&bb);
                    let lab = if *video { "video" } else { "img" };
                    sk.fill(&[sketch::rect_poly([c.0 - 18.0, c.1 - 9.0, 36.0, 15.0])], sketch::PANEL.alpha(0.9));
                    sk.text(lab, c.0, c.1 + 4.0, 9.0, &Pen::pencil(1.1), sketch::mix(s, 3), 0.5);
                }
                Shape::Raw => {
                    sk.stroke_closed(&quad, &outline, s);
                    let n = ((bb[3] / 12.0).floor() as usize).clamp(1, 4);
                    for i in 0..n {
                        let yy = bb[1] + bb[3] * (i as f64 + 1.0) / (n as f64 + 1.0);
                        self.squiggle(sk, bb[0] + bb[2] * 0.12, bb[0] + bb[2] * (0.6 + 0.3 * sketch::rnd(s, i as u64)), yy, &Pen::pencil(1.0), sketch::mix(s, 10 + i as u64));
                    }
                }
                Shape::Cloud => self.draw_cloud(sk, it, m, g, t, fade, s),
                Shape::Fluid => self.draw_fluid(sk, it, m, g, t, s),
                Shape::S3d => {
                    sk.stroke_closed(&quad, &outline, s);
                    let c = center(&bb);
                    let e = bb[2].min(bb[3]) * 0.22;
                    let (dx, dy) = (e * 0.5, -e * 0.4);
                    let f = [(c.0 - e, c.1 - e + 6.0), (c.0 + e, c.1 - e + 6.0), (c.0 + e, c.1 + e + 6.0), (c.0 - e, c.1 + e + 6.0)];
                    let b2: Vec<P> = f.iter().map(|p| (p.0 + dx, p.1 + dy)).collect();
                    let pen = Pen::pencil(1.3);
                    sk.stroke_closed(&f, &pen, sketch::mix(s, 1));
                    sk.stroke_closed(&b2, &pen.width(1.0), sketch::mix(s, 2));
                    for i in 0..4 {
                        sk.line(f[i], b2[i], &pen.width(1.0), sketch::mix(s, 3 + i as u64));
                    }
                    sk.text("3d", bb[0] + 8.0, bb[1] + 16.0, 10.0, &Pen::pencil(1.1), sketch::mix(s, 9), 0.0);
                }
            }
        }
    }

    /// Tone a region: big areas get a light marker wash, small bright things in a dark scene
    /// are knocked out to paper, everything else is hatched by darkness.
    #[allow(clippy::too_many_arguments)]
    fn tone(&self, sk: &mut Sketch, region: &[P], tone: Option<(f64, f64)>, big: bool, dark_scene: bool, fade: f64, seed: u64) {
        let Some((l, a)) = tone else { return };
        let a = (a * fade).clamp(0.0, 1.0);
        if region.len() < 3 || a < 0.05 {
            return;
        }
        let darkness = (1.0 - l).clamp(0.0, 1.0);
        let reg = [region.to_vec()];
        if big {
            sk.fill(&reg, Ink { a: ((0.03 + 0.12 * darkness) * a) as f32, ..sketch::GRAPHITE });
            return;
        }
        if dark_scene && l > 0.45 {
            // a light subject on a dark ground: paper, plus a little shading on one side
            sk.fill(&reg, sketch::PANEL.alpha(0.92 * a));
            sk.hatch(&reg, 60.0, 7.0, &Pen::pencil(0.7).color(sketch::GRAPHITE.alpha(0.35 * a)).wobble(0.4), seed);
            return;
        }
        let spacing = 8.5 - 5.0 * darkness;
        sk.fill(&reg, Ink { a: ((0.05 + 0.1 * darkness) * a) as f32, ..sketch::GRAPHITE });
        sk.hatch(&reg, 45.0 + 8.0 * sketch::rnds(seed, 1), spacing, &Pen::pencil(0.75).color(sketch::GRAPHITE.alpha((0.5 + 0.3 * darkness) * a)).wobble(0.45), seed);
        if darkness > 0.72 {
            sk.hatch(&reg, -40.0 + 8.0 * sketch::rnds(seed, 2), spacing * 1.3, &Pen::pencil(0.7).color(sketch::GRAPHITE.alpha(0.45 * a)).wobble(0.45), sketch::mix(seed, 9));
        }
    }

    fn squiggle(&self, sk: &mut Sketch, x0: f64, x1: f64, y: f64, pen: &Pen, seed: u64) {
        let n = ((x1 - x0) / 3.0).max(2.0) as usize;
        let pts: Vec<P> = (0..=n).map(|i| {
            let x = x0 + (x1 - x0) * i as f64 / n as f64;
            (x, y + (i as f64 * 1.3).sin() * 2.2 + sketch::noise(seed, i as f64 * 0.3) * 1.2)
        }).collect();
        sk.stroke(&pts, pen, seed);
    }

    fn draw_text(&self, sk: &mut Sketch, it: &Item, m: &A2, g: &PanelGeo, fade: f64, seed: u64) {
        let l = self.order[it.li];
        let lines: Vec<(String, [f64; 4])> = match self.lay.lines.get(&l.id) {
            Some(v) => v.clone(),
            None => {
                let s = match &l.kind {
                    Kind::Text { runs } => runs.iter().map(|r| r.text.as_str()).collect::<String>(),
                    _ => String::new(),
                };
                if s.trim().is_empty() {
                    return;
                }
                vec![(s, self.rect(it.li))]
            }
        };
        let sc = det(m).sqrt();
        let pen = Pen::ink(2.0).color(sketch::INK.alpha(fade));
        for (j, (txt, r)) in lines.iter().enumerate() {
            let txt = txt.replace(['*', '{', '}', '`'], "");
            let p0 = g.p(motion::a2_apply(m, r[0], r[1] + r[3] * 0.8));
            let h = r[3] * g.k * sc;
            let wpx = r[2] * g.k * sc;
            let rot = m[1].atan2(m[0]).to_degrees();
            let mut cap = h * 0.58;
            let wu = hand::width_units(&txt) * cap / 6.0;
            if wu > wpx * 1.08 && wu > 0.0 {
                cap *= wpx * 1.08 / wu;
            }
            if cap < 3.2 {
                // too small to letter: a scribble where the line is
                let a = g.p(motion::a2_apply(m, r[0], r[1] + r[3] * 0.6));
                let b = g.p(motion::a2_apply(m, r[0] + r[2], r[1] + r[3] * 0.6));
                self.squiggle(sk, a.0, b.0.max(a.0 + 3.0), a.1, &Pen::pencil(1.0).color(sketch::GRAPHITE.alpha(fade)), sketch::mix(seed, j as u64));
                continue;
            }
            let pen = pen.width((cap * 0.13).clamp(1.0, 4.2));
            sk.text_rot(&txt, p0.0, p0.1, cap, &pen, sketch::mix(seed, 40 + j as u64), 0.0, rot);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cloud(&self, sk: &mut Sketch, it: &Item, m: &A2, g: &PanelGeo, t: f64, fade: f64, seed: u64) {
        let l = self.order[it.li];
        let Kind::Cloud(cfg) = &l.kind else { return };
        let r = self.rect(it.li);
        let form = cfg.forms.iter().filter(|f| f.at <= t + 1e-6).last().or(cfg.forms.first());
        let len = |s: Option<&String>, base: f64, dflt: f64| -> f64 {
            match s {
                Some(v) if v.ends_with('%') => v.trim_end_matches('%').trim().parse::<f64>().map(|x| x / 100.0 * base).unwrap_or(dflt),
                Some(v) => v.trim_end_matches("px").trim().parse::<f64>().unwrap_or(dflt),
                None => dflt,
            }
        };
        let n = (cfg.n as usize).clamp(30, 320);
        let mut pts: Vec<P> = vec![];
        let shape = form.map(|f| f.shape.as_str()).unwrap_or("random");
        let props = form.map(|f| &f.props);
        let get = |k: &str| props.and_then(|p| p.get(k));
        let cx = len(get("x"), r[2], r[2] / 2.0);
        let cy = len(get("y"), r[3], r[3] / 2.0);
        match shape {
            "sphere" | "ring" | "circle" | "disc" | "orbit" => {
                let rad = len(get("r"), r[2].min(r[3]), r[2].min(r[3]) * 0.3);
                for i in 0..n {
                    let a = sketch::rnd(seed, i as u64) * std::f64::consts::TAU;
                    let q = if shape == "ring" || shape == "circle" { 1.0 + 0.04 * sketch::rnds(seed, 5000 + i as u64) } else { sketch::rnd(seed, 9000 + i as u64).sqrt() };
                    pts.push((cx + rad * q * a.cos(), cy + rad * q * a.sin()));
                }
                // a sphere reads as a ball: a light contour
                let c = g.p(motion::a2_apply(m, r[0] + cx, r[1] + cy));
                let rr = rad * g.k * det(m).sqrt();
                sk.ellipse(c.0, c.1, rr, rr, &Pen::pencil(0.9).color(sketch::GRAPHITE.alpha(0.45 * fade)).dashed(6.0, 5.0), seed);
            }
            "cluster" => {
                let sx = len(get("sx"), r[2], r[2] * 0.05);
                let sy = len(get("sy"), r[3], r[3] * 0.05);
                for i in 0..n {
                    let (u, v) = (sketch::rnd(seed, i as u64).max(1e-6), sketch::rnd(seed, 7000 + i as u64));
                    let rr = (-2.0 * u.ln()).sqrt();
                    pts.push((cx + sx * rr * (v * std::f64::consts::TAU).cos(), cy + sy * rr * (v * std::f64::consts::TAU).sin()));
                }
            }
            "points" => {
                if let Some(raw) = get("points") {
                    let nums: Vec<f64> = raw.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).filter_map(|s| s.parse().ok()).collect();
                    let step = (nums.len() / 3 / n).max(1);
                    for c in nums.chunks(3).step_by(step) {
                        if c.len() >= 2 {
                            pts.push((c[0], c[1]));
                        }
                    }
                }
            }
            "text" => {
                let txt = form.and_then(|f| f.text.clone()).unwrap_or_default();
                let p = g.p(motion::a2_apply(m, r[0] + cx, r[1] + cy));
                let cap = (len(get("size"), r[3], r[3] * 0.2) * g.k * 0.7).clamp(6.0, g.h * 0.4);
                sk.text(&txt, p.0, p.1 + cap / 2.0, cap, &Pen::pencil(1.4).color(sketch::GRAPHITE.alpha(0.9 * fade)).dashed(2.5, 2.0), seed, 0.5);
                return;
            }
            _ => {
                let (w, h) = (len(get("w"), r[2], r[2]), len(get("h"), r[3], r[3]));
                for i in 0..n {
                    pts.push((cx + (sketch::rnd(seed, i as u64) - 0.5) * w, cy + (sketch::rnd(seed, 7000 + i as u64) - 0.5) * h));
                }
            }
        }
        for (i, p) in pts.iter().enumerate() {
            let q = g.p(motion::a2_apply(m, r[0] + p.0, r[1] + p.1));
            sk.dot(q, 0.9 + 0.7 * sketch::rnd(seed, 20000 + i as u64), sketch::GRAPHITE.alpha(0.75 * fade));
        }
    }

    fn draw_fluid(&self, sk: &mut Sketch, it: &Item, m: &A2, g: &PanelGeo, t: f64, seed: u64) {
        let l = self.order[it.li];
        let Kind::Fluid(cfg) = &l.kind else { return };
        let r = self.rect(it.li);
        let len = |s: &str, base: f64| -> f64 {
            if let Some(p) = s.strip_suffix('%') { p.trim().parse::<f64>().map(|x| x / 100.0 * base).unwrap_or(base / 2.0) } else { s.trim_end_matches("px").trim().parse().unwrap_or(base / 2.0) }
        };
        let pen = Pen::pencil(1.2).color(sketch::GRAPHITE.alpha(0.75));
        let mut drew = false;
        for (i, e) in cfg.emitters.iter().enumerate() {
            let s = sketch::mix(seed, i as u64);
            match e {
                crate::fluid::FluidEv::Pour { t0, t1, at, dir, speed, .. } if t >= *t0 - 0.2 && t <= t1 + 1.5 => {
                    let p = (r[0] + len(&at.0, r[2]), r[1] + len(&at.1, r[3]));
                    let a = dir.to_radians();
                    let reach = (speed * 0.35).clamp(30.0, r[3].max(r[2]) * 0.5);
                    let pts: Vec<P> = (0..=24).map(|j| {
                        let f = j as f64 / 24.0;
                        let wob = (f * 9.0 + sketch::rnd(s, 1) * 6.0).sin() * 5.0 * f;
                        let q = (p.0 + a.cos() * reach * f - a.sin() * wob, p.1 + a.sin() * reach * f + a.cos() * wob);
                        g.p(motion::a2_apply(m, q.0, q.1))
                    }).collect();
                    sk.stroke(&pts, &pen.width(2.2), s);
                    sk.stroke(&pts.iter().map(|q| (q.0 + 3.0, q.1 + 1.0)).collect::<Vec<_>>(), &pen.width(1.0), sketch::mix(s, 2));
                    drew = true;
                }
                crate::fluid::FluidEv::Stir { t0, t1, at, radius, .. } if t >= *t0 - 0.2 && t <= t1 + 1.0 => {
                    let c = (r[0] + len(&at.0, r[2]), r[1] + len(&at.1, r[3]));
                    let rad = len(radius, r[2].min(r[3])).max(12.0);
                    let pts: Vec<P> = (0..=60).map(|j| {
                        let f = j as f64 / 60.0;
                        let a = f * std::f64::consts::TAU * 2.2 + sketch::rnd(s, 3) * 6.0;
                        let rr = rad * (0.25 + 0.75 * f);
                        g.p(motion::a2_apply(m, c.0 + rr * a.cos(), c.1 + rr * a.sin() * 0.8))
                    }).collect();
                    sk.stroke(&pts, &pen, s);
                    drew = true;
                }
                crate::fluid::FluidEv::Source { t0, t1, .. } if t >= *t0 - 0.2 && t <= t1 + 2.0 => {
                    drew = true;
                }
                _ => {}
            }
        }
        if !drew {
            // resting fluid: a few soft curls where it was last stirred or poured
            let c = cfg.emitters.iter().filter_map(|e| match e {
                crate::fluid::FluidEv::Pour { t0, at, .. } | crate::fluid::FluidEv::Stir { t0, at, .. } if *t0 <= t => Some((r[0] + len(&at.0, r[2]), r[1] + len(&at.1, r[3]))),
                _ => None,
            }).last();
            if let Some(c) = c {
                for j in 0..3u64 {
                    let s = sketch::mix(seed, 40 + j);
                    let off = (sketch::rnds(s, 1) * r[2] * 0.08, sketch::rnds(s, 2) * r[3] * 0.08);
                    let rad = r[2].min(r[3]) * (0.05 + 0.04 * sketch::rnd(s, 3));
                    let pts: Vec<P> = (0..=30).map(|i| {
                        let f = i as f64 / 30.0;
                        let a = f * std::f64::consts::TAU * 1.3 + sketch::rnd(s, 4) * 6.0;
                        g.p(motion::a2_apply(m, c.0 + off.0 + rad * f * a.cos(), c.1 + off.1 + rad * f * a.sin()))
                    }).collect();
                    sk.stroke(&pts, &pen.width(1.0).color(sketch::GRAPHITE.alpha(0.5)), s);
                }
            }
        }
    }

    fn draw_move(&self, sk: &mut Sketch, m: &Move, g: &PanelGeo, seed: u64) {
        let red = sketch::RED.alpha(0.88);
        let outline = Pen::ink(1.3).wobble(0.5).passes(1);
        let to = g.r(m.to);
        let c1 = center(&to);
        let span = (g.w.min(g.h) * 0.5).max(10.0);
        match m.kind.as_str() {
            "path" => {
                let reps: Vec<usize> = if m.members.len() <= 3 { m.members.clone() } else { vec![m.members[0], m.members[m.members.len() / 2], *m.members.last().unwrap()] };
                let ka = self.k_of(m.t0);
                for (j, &li) in reps.iter().enumerate() {
                    let inside = |q: &P| q.0 >= g.x - 6.0 && q.0 <= g.x + g.w + 6.0 && q.1 >= g.y - 6.0 && q.1 <= g.y + g.h + 6.0;
                    let pts: Vec<P> = if reps.len() == 1 {
                        m.path.iter().map(|p| g.p((p[0], p[1]))).filter(inside).collect()
                    } else {
                        // member trajectories from its own boxes
                        (ka..self.times.len()).filter(|&k| self.times[k] <= self.times[ka] + (m.path.len() as f64) * DT).filter_map(|k| self.sub_bbox(li, k)).map(|b| g.p(center(&b))).filter(inside).collect()
                    };
                    if pts.len() < 2 {
                        continue;
                    }
                    let mut pts = sketch::resample(&pts, 4.0);
                    let l = sketch::poly_len(&pts);
                    if l < 10.0 {
                        continue;
                    }
                    // a straight move gets the classic bow
                    let (a, b) = (pts[0], *pts.last().unwrap());
                    let straight = sketch::dist(a, b) > 0.92 * l;
                    if straight {
                        let bow = 0.12 * sketch::dist(a, b) * if sketch::rnd(seed, j as u64) > 0.5 { 1.0 } else { -1.0 };
                        let (dx, dy) = ((b.0 - a.0) / l, (b.1 - a.1) / l);
                        let mid = ((a.0 + b.0) / 2.0 - dy * bow, (a.1 + b.1) / 2.0 + dx * bow);
                        pts = hand::catmull_rom(&[a, mid, b], 12);
                    }
                    // start and end a little inside the trajectory (not on the subject's centre)
                    let l = sketch::poly_len(&pts);
                    let pts = sketch::trim(&pts, (6.0 / l).min(0.2), 1.0 - (4.0 / l).min(0.15));
                    let width = (l * 0.09).clamp(6.0, 15.0);
                    sk.fat_arrow(&pts, width, red, &outline, sketch::mix(seed, 10 + j as u64));
                }
                if m.count > 1 {
                    let tip = g.p((m.path.last().map(|p| p[0]).unwrap_or(0.0), m.path.last().map(|p| p[1]).unwrap_or(0.0)));
                    self.tag(sk, &format!("\u{00D7}{}", m.count), (tip.0 + 10.0, tip.1 - 6.0), sketch::RED, seed);
                }
            }
            "grow" | "shrink" => {
                let grow = m.kind == "grow";
                let d = ((to[2] * to[2] + to[3] * to[3]).sqrt() * 0.16).clamp(10.0, 30.0);
                for (i, (sx, sy)) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().enumerate() {
                    let corner = (c1.0 + sx * to[2] / 2.0, c1.1 + sy * to[3] / 2.0);
                    let (ux, uy) = (sx / 2f64.sqrt(), sy / 2f64.sqrt());
                    let (a, b) = if grow { ((corner.0 + ux * 3.0, corner.1 + uy * 3.0), (corner.0 + ux * (3.0 + d), corner.1 + uy * (3.0 + d))) } else { ((corner.0 + ux * (3.0 + d), corner.1 + uy * (3.0 + d)), (corner.0 + ux * 3.0, corner.1 + uy * 3.0)) };
                    sk.fat_arrow(&sketch::resample(&[a, b], 2.0), 6.5, red, &outline, sketch::mix(seed, 20 + i as u64));
                }
            }
            "stretch-x" | "stretch-y" => {
                let hor = m.kind == "stretch-x";
                let out = m.amount > 1.0;
                let d = (if hor { to[2] } else { to[3] } * 0.2).clamp(12.0, 40.0);
                for (i, sgn) in [-1.0f64, 1.0].iter().enumerate() {
                    let edge = if hor { (c1.0 + sgn * to[2] / 2.0, c1.1) } else { (c1.0, c1.1 + sgn * to[3] / 2.0) };
                    // a stretch to the frame edge: draw the arrows just inside it
                    let lim = d + 10.0;
                    let edge = if hor { (edge.0.clamp(g.x + lim, (g.x + g.w - lim).max(g.x + lim)), edge.1) } else { (edge.0, edge.1.clamp(g.y + lim, (g.y + g.h - lim).max(g.y + lim))) };
                    let off = |k: f64| if hor { (edge.0 + sgn * k, edge.1) } else { (edge.0, edge.1 + sgn * k) };
                    let (a, b) = if out { (off(4.0), off(4.0 + d)) } else { (off(4.0 + d), off(4.0)) };
                    sk.fat_arrow(&sketch::resample(&[a, b], 2.0), 7.0, red, &outline, sketch::mix(seed, 30 + i as u64));
                }
            }
            "rotate" => {
                let rad = (to[2].max(to[3]) * 0.62).clamp(14.0, span);
                let dir = m.amount.signum();
                let a0 = -150f64.to_radians();
                let sweep = 115f64.to_radians() * dir;
                let pts: Vec<P> = (0..=24).map(|i| {
                    let a = a0 + sweep * i as f64 / 24.0;
                    (c1.0 + rad * a.cos(), c1.1 + rad * a.sin())
                }).collect();
                sk.fat_arrow(&pts, 7.5, red, &outline, sketch::mix(seed, 40));
                if m.amount.abs() >= 540.0 {
                    self.tag(sk, &format!("\u{00D7}{:.0}", (m.amount.abs() / 360.0).round()), (c1.0 + rad * 0.75, c1.1 - rad * 0.9), sketch::RED, seed);
                }
            }
            "shake" => {
                // a double-headed arrow along the main axis of the bobbing plus motion ticks
                let (mut sx, mut sy) = (0.0, 0.0);
                for w in m.path.windows(2) {
                    sx += (w[1][0] - w[0][0]).abs();
                    sy += (w[1][1] - w[0][1]).abs();
                }
                let hor = sx >= sy;
                let d = (if hor { to[2] } else { to[3] } * 0.5 + 10.0).clamp(12.0, span);
                let pen = Pen::ink(1.6).color(sketch::RED);
                let (a, b) = if hor { ((c1.0 - d, to[1] - 8.0), (c1.0 + d, to[1] - 8.0)) } else { ((to[0] + to[2] + 9.0, c1.1 - d), (to[0] + to[2] + 9.0, c1.1 + d)) };
                let line = sketch::resample(&[a, b], 3.0);
                sk.thin_arrow(&line, &pen, 6.0, sketch::mix(seed, 50));
                let rev: Vec<P> = line.iter().rev().copied().collect();
                sk.thin_arrow(&rev, &pen, 6.0, sketch::mix(seed, 51));
            }
            "scroll-x" | "scroll-y" => {
                // repeated travel one way: two parallel thin arrows beside the subject
                let hor = m.kind == "scroll-x";
                let sgn = m.amount.signum();
                let pen = Pen::ink(2.0).color(sketch::RED);
                let d = (if hor { to[2] } else { to[3] } * 0.35).clamp(14.0, span);
                for j in 0..2 {
                    let o = j as f64 * 7.0;
                    let (a, b) = if hor {
                        let y = (to[1] - 8.0 - o).max(g.y + 14.0 + o);
                        ((c1.0 - sgn * d, y), (c1.0 + sgn * d, y))
                    } else {
                        let x = (to[0] + to[2] + 8.0 + o).min(g.x + g.w - 14.0 - o);
                        ((x, c1.1 - sgn * d), (x, c1.1 + sgn * d))
                    };
                    sk.thin_arrow(&sketch::resample(&[a, b], 3.0), &pen, 6.0, sketch::mix(seed, 70 + j));
                }
            }
            "pulse" if m.count <= 2 => {
                // vibration marks: two short arcs on each side
                let pen = Pen::ink(1.5).color(sketch::RED);
                // around the subject at its smaller extent (before it stretches or grows)
                let fr = g.r(m.from);
                let (to, c1) = if area(&fr) < area(&to) && area(&fr) > 0.0 { (fr, center(&fr)) } else { (to, c1) };
                let base = (to[2].max(to[3]) / 2.0).clamp(6.0, span);
                for side in [-1.0f64, 1.0] {
                    for j in 0..2 {
                        let rr = base + 5.0 + j as f64 * 6.0;
                        let pts: Vec<P> = (0..=8).map(|i| {
                            let a = (if side < 0.0 { 180.0 } else { 0.0 } + (i as f64 / 8.0 - 0.5) * 70.0f64).to_radians();
                            (c1.0 + rr * a.cos(), c1.1 + rr * a.sin())
                        }).collect();
                        sk.stroke(&pts, &pen, sketch::mix(seed, 60 + j + if side < 0.0 { 10 } else { 0 }));
                    }
                }
            }
            _ => {}
        }
    }

    fn tag(&self, sk: &mut Sketch, s: &str, at: P, ink: Ink, seed: u64) {
        let w = hand::width_units(s) * 11.0 / 6.0;
        sk.fill(&[sketch::rect_poly([at.0 - 3.0, at.1 - 13.0, w + 6.0, 17.0])], sketch::PANEL.alpha(0.85));
        sk.text(s, at.0, at.1, 11.0, &Pen::ink(1.5).color(ink), sketch::mix(seed, 77), 0.0);
    }

    fn draw_cam(&self, sk: &mut Sketch, c: &Cam, g: &PanelGeo, ci: usize, seed: u64) {
        let blue = sketch::BLUE;
        let pen = Pen::ink(1.9).color(blue);
        let thin = Pen::ink(1.3).color(blue);
        let pr = [g.x, g.y, g.w, g.h];
        let label = match c.kind.as_str() {
            "push-in" => format!("push in{}", if c.amount >= 2.5 { format!(" \u{00D7}{}", ratio(c.amount)) } else { String::new() }),
            "pull-out" => format!("pull out{}", if c.amount >= 2.5 { format!(" \u{00D7}{}", ratio(c.amount)) } else { String::new() }),
            "dolly-in" => "dolly in".into(),
            "dolly-out" => "dolly out".into(),
            "pan-left" => "pan left".into(),
            "pan-right" => "pan right".into(),
            "tilt-up" => "tilt up".into(),
            "tilt-down" => "tilt down".into(),
            "roll" => "roll".into(),
            _ => "orbit".into(),
        };
        match c.kind.as_str() {
            "push-in" | "pull-out" | "dolly-in" | "dolly-out" => {
                let Some(inner) = c.inner else { return };
                let mut r = g.r(inner);
                // keep the inner frame readable
                let min = 0.24;
                if r[2] < g.w * min {
                    let cx = r[0] + r[2] / 2.0;
                    let cy = r[1] + r[3] / 2.0;
                    r = [cx - g.w * min / 2.0, cy - g.h * min / 2.0, g.w * min, g.h * min];
                }
                r[2] = r[2].min(g.w - 8.0);
                r[3] = r[3].min(g.h - 8.0);
                r[0] = r[0].max(g.x + 4.0).min(g.x + g.w - r[2] - 4.0);
                r[1] = r[1].max(g.y + 4.0).min(g.y + g.h - r[3] - 4.0);
                sk.rect(r, &pen, sketch::mix(seed, 1));
                let outer = sketch::rect_poly([pr[0] + 5.0, pr[1] + 5.0, pr[2] - 10.0, pr[3] - 10.0]);
                let inr = sketch::rect_poly(r);
                let inward = c.kind.ends_with("in");
                for i in 0..4 {
                    let (a, b) = if inward { (outer[i], inr[i]) } else { (inr[i], outer[i]) };
                    let (a, b) = (sketch::trim(&[a, b], 0.12, 0.9)[0], *sketch::trim(&[a, b], 0.12, 0.9).last().unwrap());
                    sk.thin_arrow(&sketch::resample(&[a, b], 4.0), &thin, 6.0, sketch::mix(seed, 10 + i as u64));
                }
                let lx = r[0] + 4.0;
                let ly = (r[1] - 6.0).max(g.y + 14.0);
                self.cam_label(sk, &label, (lx, if ly <= g.y + 14.0 { r[1] + 16.0 } else { ly }), seed);
            }
            "pan-left" | "pan-right" | "tilt-up" | "tilt-down" => {
                let (a, b) = match c.kind.as_str() {
                    "pan-left" => ((g.x + g.w * 0.78, g.y + g.h - 16.0), (g.x + g.w * 0.22, g.y + g.h - 16.0)),
                    "pan-right" => ((g.x + g.w * 0.22, g.y + g.h - 16.0), (g.x + g.w * 0.78, g.y + g.h - 16.0)),
                    "tilt-up" => ((g.x + g.w - 18.0, g.y + g.h * 0.8), (g.x + g.w - 18.0, g.y + g.h * 0.2)),
                    _ => ((g.x + g.w - 18.0, g.y + g.h * 0.2), (g.x + g.w - 18.0, g.y + g.h * 0.8)),
                };
                sk.fat_arrow(&sketch::resample(&[a, b], 3.0), 13.0, blue.alpha(0.85), &Pen::ink(1.3), sketch::mix(seed, 2));
                let lp = if c.kind.starts_with("pan") { (g.x + g.w * 0.5, g.y + g.h - 32.0) } else { (g.x + g.w - 80.0, g.y + 18.0) };
                self.cam_label(sk, &label, (lp.0 - 30.0, lp.1), seed);
            }
            "roll" | "orbit" => {
                let (cx, cy) = (g.x + g.w * 0.5, g.y + g.h * if c.kind == "orbit" { 0.82 } else { 0.5 });
                let (rx, ry) = if c.kind == "orbit" { (g.w * 0.32, g.h * 0.08) } else { (g.h * 0.36, g.h * 0.36) };
                let dir = if c.amount < 0.0 { -1.0 } else { 1.0 };
                let pts: Vec<P> = (0..=30).map(|i| {
                    let a = (200.0f64 + dir * 140.0 * i as f64 / 30.0).to_radians();
                    (cx + rx * a.cos(), cy + ry * a.sin())
                }).collect();
                sk.fat_arrow(&pts, 9.0, blue.alpha(0.85), &Pen::ink(1.2), sketch::mix(seed, 3));
                self.cam_label(sk, &label, (g.x + 10.0, g.y + 20.0 + ci as f64 * 18.0), seed);
            }
            _ => {}
        }
    }

    fn cam_label(&self, sk: &mut Sketch, s: &str, at: P, seed: u64) {
        let cap = 11.0;
        let w = hand::width_units(s) * cap / 6.0;
        sk.fill(&[sketch::rect_poly([at.0 - 4.0, at.1 - cap - 4.0, w + 8.0, cap + 8.0])], sketch::PANEL.alpha(0.88));
        sk.text(s, at.0, at.1, cap, &Pen::ink(1.5).color(sketch::BLUE), sketch::mix(seed, 99), 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verbs_and_plurals() {
        assert_eq!(verb("rises", true), "rise");
        assert_eq!(verb("fades in", true), "fade in");
        assert_eq!(verb("stretches", true), "stretch");
        assert_eq!(verb("pulses", true), "pulse");
        assert_eq!(plural("stripe"), "stripes");
        assert_eq!(plural("box"), "boxes");
        assert_eq!(stem("stripe12"), "stripe");
        assert_eq!(fmt_time(4.8), "0:04.8");
        assert_eq!(fmt_time(8.0), "0:08");
        assert_eq!(fmt_time(75.0), "1:15");
    }
}
