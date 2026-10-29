//! `scene3d`: stylised 3D in Reel Script (reel-ir `scene3d` content). The scene is one layer of
//! the 2D pipeline (every look, effect, blend and mask applies to it); its children are 3D
//! objects (`box plane floor sphere cylinder cone torus extrude mesh screen group light`) that
//! are layers of their own for timing and tracks (`anim #crate ry ...`, `in=drop`), but are
//! never laid out or drawn in 2D: `reelir.rs` (`s3d_emit.rs`) folds them into the content.
//!
//! ```text
//! scene3d #world fov=40 camera=(0, 2, 8) target=(0, 0, 0) light=(-3, 5, 4) ambient=0.35 fog=#05010D fog-near=10 fog-far=30
//!   floor #ground size=40 y=-1 color=#0B1A10
//!   box #crate w=2 h=1.4 d=2 color=#C9A36B material=toon outline=2 outline-color=#2A1A0A
//!   extrude #L "L" depth=0.6 size=1.6 color=#7CFF6B emissive=0.6 x=-3
//!   screen #crt w=4 h=2.25 z=-2 src=#ui
//! @0s orbit #world by=30deg dur=4s ease=inout-cubic
//! anim #crate ry 0s:0deg 2s:180deg~out-back
//! ```
//!
//! World units are free (think metres), y is up, the camera looks at `target`. Rotations are
//! degrees, applied yaw (ry) last.
use crate::ast::{Node, Span};
use crate::ir::*;
use crate::lower::*;
use crate::values::*;
use agentfx_ir::s3d::{self as m, Bloom3d, Cam3d, Fog3d, Light3d, Mat3d, Obj3d, Scene3d, Shadow3d, Shape3d, V3};
use serde::Serialize;

/// Object kinds allowed inside a `scene3d`.
pub const OBJ_KINDS: &[&str] = &["box", "cube", "plane", "floor", "sphere", "cylinder", "cone", "torus", "extrude", "mesh", "screen", "group", "light"];

const SCENE_PROPS: &[&str] = &[
    "fov", "camera", "target", "roll", "near", "far", "ortho", "iso", "light", "light-color", "light-intensity", "ambient", "ambient-color", "fog", "fog-near", "fog-far",
    "fog-density", "bg", "aa", "bloom", "bloom-strength", "shadow", "shadow-blur", "shadow-color", "outline", "outline-color", "material", "color",
];
/// 2D layer props a scene3d passes on to the common element props (placement, looks, effects).
const LAYER_PROPS: &[&str] = &[
    "w", "h", "x", "y", "place", "anchor", "opacity", "blur", "glow", "glow-color", "blend", "mask", "mask-mode", "clip", "radius", "brightness", "saturate",
    "contrast", "hue", "tint", "tint-amount", "levels", "lut", "mblur", "shutter", "in", "out", "at", "until", "dur", "delay", "ease", "stagger", "hud", "z", "rot",
    "scale", "rotx", "roty", "origin", "rgbsplit", "glitch", "vhs", "dither", "ascii", "grain", "halftone", "riso", "misreg", "paper", "sweep", "rays",
];
const OBJ_PROPS: &[&str] = &[
    "x", "y", "z", "pos", "rx", "ry", "rz", "rot", "scale", "sx", "sy", "sz", "pivot", "color", "material", "opacity", "emissive", "bands", "specular", "shininess",
    "outline", "outline-color", "crease", "wire", "wire-color", "double-sided", "shadow", "ground", "texture", "in", "out", "at", "until", "dur", "delay", "ease",
    "stagger", "from",
];
/// A prop of the scene3d line that belongs to its 2D layer (placement, effects, looks).
pub fn layer_prop(k: &str) -> bool {
    LAYER_PROPS.contains(&k) || ["riso", "halftone", "grain", "paper", "rays", "rgbsplit", "glitch", "dither", "sweep", "vhs", "ascii", "mblur", "shutter"].iter().any(|p| k.starts_with(p))
}

fn shape_props(kind: &str) -> &'static [&'static str] {
    match kind {
        "box" | "cube" => &["w", "h", "d", "size", "bevel"],
        "plane" => &["w", "h", "size"],
        "floor" => &["w", "d", "size"],
        "sphere" => &["r", "seg"],
        "cylinder" => &["r", "r2", "h", "seg"],
        "cone" => &["r", "h", "seg"],
        "torus" => &["r", "tube", "seg"],
        "extrude" => &["size", "depth", "font", "weight", "tracking", "d"],
        "mesh" => &["src", "size"],
        "screen" => &["w", "h", "src", "res"],
        "light" => &["point", "dir", "intensity", "range"],
        _ => &[],
    }
}

const STD: Ease = Ease::Cubic(0.65, 0.0, 0.35, 1.0);
const OUT: Ease = Ease::Cubic(0.33, 1.0, 0.68, 1.0);
const BACK: Ease = Ease::Cubic(0.34, 1.56, 0.64, 1.0);

#[derive(Debug, Clone, Serialize)]
pub struct S3dCfg {
    /// camera, lights (the key light from `light=`), fog, bg, aa, bloom, shadow; objects and
    /// rect are filled in at emit time
    pub scene: Scene3d,
    /// `light=off`: no key light
    pub key_light: bool,
    /// bloom / shadow switched off explicitly (`bloom=off`, `shadow=off`)
    pub bloom_off: bool,
    pub shadow_off: bool,
    pub si: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjCfg {
    pub kind: String,
    /// shape, base transform, material, cast/receive (parent index set at emit)
    pub obj: Obj3d,
    /// screen: source layer (internal layer id)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    /// screen: source as written (lint)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_name: Option<String>,
    /// extrude text: font file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// texture image (absolute path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    /// a light instead of geometry
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light: Option<Light3d>,
    pub span: (u32, u32),
}

/// `(x, y, z)`, `[x, y, z]` or `x,y,z`.
pub fn parse_vec3(raw: &str) -> Option<V3> {
    let s = raw.trim().trim_start_matches(['(', '[']).trim_end_matches([')', ']']);
    let v: Vec<f64> = s.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()).map(|p| p.trim_end_matches("deg").parse::<f64>()).collect::<Result<_, _>>().ok()?;
    (v.len() == 3 && v.iter().all(|x| x.is_finite())).then(|| [v[0], v[1], v[2]])
}

fn praw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}

fn rgba(c: &Rgba) -> [f64; 4] {
    [c.r, c.g, c.b, c.a]
}

/// Approximate local half extents of an object (lint; text and meshes estimated).
pub fn half_extent(o: &ObjCfg) -> Option<V3> {
    match &o.obj.shape {
        Shape3d::Extrude { text: Some(t), size, depth, .. } => Some([0.32 * size * t.chars().count().max(1) as f64, 0.38 * size, depth / 2.0]),
        Shape3d::Extrude { size, depth, .. } => Some([size / 2.0, size / 2.0, depth / 2.0]),
        Shape3d::Mesh { size, .. } => Some([size / 2.0; 3]),
        s => m::shape_bounds(s).map(|(_, hi)| hi),
    }
}

impl<'a> Ctx<'a> {
    fn s3d_vec(&mut self, n: &Node, k: &str) -> Option<V3> {
        let raw = praw(n, k)?;
        match parse_vec3(&raw) {
            Some(v) => Some(v),
            None => {
                self.err("E_S3D", format!("{} {k}={raw}: expected three numbers", n.kind), format!("write `{k}=(0, 2, 8)`"), n.span);
                None
            }
        }
    }

    fn s3d_num(&mut self, n: &Node, k: &str, lo: f64, hi: f64) -> Option<f64> {
        let raw = praw(n, k)?;
        match parse_angle_deg(raw.trim_end_matches("px")).ok().or_else(|| parse_number(&raw).ok()) {
            Some(v) if (lo..=hi).contains(&v) => Some(v),
            _ => {
                self.err("E_S3D", format!("{} {k}={raw}: must be a number in {lo}..{hi}", n.kind), format!("e.g. `{k}={}`", if lo > 0.0 { lo.max(1.0) } else { 1.0 }), n.span);
                None
            }
        }
    }

    fn s3d_props(&mut self, n: &Node, ok: &[&[&str]]) {
        let all: Vec<&str> = ok.iter().flat_map(|s| s.iter().copied()).collect();
        for k in n.props.keys() {
            if all.contains(&k.as_str()) || k.starts_with("riso") || k.starts_with("halftone") || k.starts_with("grain") || k.starts_with("paper") || k.starts_with("rays") || k.starts_with("rgbsplit") || k.starts_with("glitch") || k.starts_with("dither") {
                continue;
            }
            let hint = suggest(k, &all).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("`{}` takes: {}", n.kind, all.join(" ")));
            self.warn("W_S3D_PROP", format!("{}: unknown prop `{k}`", n.kind), hint, n.span);
        }
    }

    /// `scene3d #id ...` with object children. Default box: the whole frame (scene world).
    pub fn scene3d(&mut self, parent: usize, si: usize, n: &Node, lid: String) -> usize {
        self.s3d_props(n, &[SCENE_PROPS, LAYER_PROPS]);
        let iso = flag(n, "iso");
        let mut cam = Cam3d { pos: [0.0, 2.0, 8.0], target: [0.0, 0.0, 0.0], fov: 40.0, roll: 0.0, near: 0.1, far: 200.0, ortho: 0.0 };
        if iso {
            cam.pos = [10.0, 8.165, 10.0];
            cam.ortho = 8.0;
        }
        if let Some(v) = self.s3d_vec(n, "camera") {
            cam.pos = v;
        }
        if let Some(v) = self.s3d_vec(n, "target") {
            cam.target = v;
        }
        if let Some(v) = self.s3d_num(n, "fov", 1.0, 170.0) {
            cam.fov = v;
        }
        if let Some(v) = self.s3d_num(n, "roll", -3600.0, 3600.0) {
            cam.roll = v;
        }
        if let Some(v) = self.s3d_num(n, "near", 1e-4, 1e6) {
            cam.near = v;
        }
        if let Some(v) = self.s3d_num(n, "far", 1e-3, 1e7) {
            cam.far = v;
        }
        if let Some(v) = self.s3d_num(n, "ortho", 0.0, 1e6) {
            cam.ortho = v;
        }
        if cam.far <= cam.near {
            self.err("E_S3D", format!("scene3d near={} far={}", cam.near, cam.far), "far must be larger than near, e.g. `near=0.1 far=200`", n.span);
            cam.far = cam.near * 1000.0;
        }
        if m::len(m::sub(cam.target, cam.pos)) < 1e-9 {
            self.err("E_S3D", "scene3d camera sits on its target", "move it back: `camera=(0, 2, 8) target=(0, 0, 0)`", n.span);
            cam.pos[2] += 8.0;
        }
        let light_off = praw(n, "light").as_deref() == Some("off");
        let mut lights = vec![];
        if !light_off {
            let pos = if n.props.contains_key("light") { self.s3d_vec(n, "light").unwrap_or([-3.0, 5.0, 4.0]) } else { [-3.0, 5.0, 4.0] };
            let color = self.color(n, "light-color").map(|c| rgba(&c)).unwrap_or([1.0; 4]);
            let intensity = self.s3d_num(n, "light-intensity", 0.0, 100.0).unwrap_or(0.75);
            lights.push(Light3d { kind: "directional".into(), pos, color, intensity, range: 0.0 });
        }
        let ambient = self.s3d_num(n, "ambient", 0.0, 100.0).unwrap_or(0.4);
        let ambient_color = self.color(n, "ambient-color").map(|c| rgba(&c)).unwrap_or([1.0; 4]);
        let fog = if n.props.contains_key("fog") && praw(n, "fog").as_deref() != Some("off") {
            let color = self.color(n, "fog").map(|c| rgba(&c)).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let dist = m::len(m::sub(cam.target, cam.pos));
            let density = self.s3d_num(n, "fog-density", 0.0, 100.0);
            let near = self.s3d_num(n, "fog-near", 0.0, 1e7).unwrap_or(dist);
            let far = self.s3d_num(n, "fog-far", 0.0, 1e7).unwrap_or(dist * 3.5);
            if density.is_none() && far <= near {
                self.err("E_S3D", format!("scene3d fog-near={near} fog-far={far}"), "fog-far must be larger than fog-near", n.span);
            }
            Some(Fog3d { color, mode: if density.is_some() { "exp".into() } else { "linear".into() }, near, far: far.max(near + 1e-3), density: density.unwrap_or(0.0) })
        } else {
            None
        };
        let bg = if n.props.contains_key("bg") { self.color(n, "bg").map(|c| rgba(&c)) } else { None };
        let aa = self.s3d_num(n, "aa", 1.0, 4.0).map(|v| v.round() as u32).unwrap_or(2);
        let bloom_off = praw(n, "bloom").as_deref() == Some("off");
        let bloom = if n.props.contains_key("bloom") && !bloom_off {
            Some(Bloom3d { radius: self.s3d_num(n, "bloom", 0.0, 400.0).unwrap_or(14.0), strength: self.s3d_num(n, "bloom-strength", 0.0, 10.0).unwrap_or(1.0) })
        } else {
            None
        };
        let shadow_off = praw(n, "shadow").as_deref() == Some("off");
        let shadow = if shadow_off {
            None
        } else {
            let opacity = if n.props.contains_key("shadow") { self.s3d_num(n, "shadow", 0.0, 1.0).unwrap_or(0.45) } else { 0.45 };
            let blur = self.s3d_num(n, "shadow-blur", 0.0, 200.0).unwrap_or(6.0);
            let color = self.color(n, "shadow-color").map(|c| rgba(&c)).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            Some(Shadow3d { opacity, blur, color })
        };
        let scene = Scene3d { rect: [0.0; 4], camera: cam.clone(), ambient, ambient_color, lights, fog, bg, aa, bloom, shadow, objects: vec![] };
        let cfg = S3dCfg { scene, key_light: !light_off, bloom_off, shadow_off, si };
        let mut l = Layer::new(lid, Kind::Scene3d(Box::new(cfg)));
        l.tags.push("scene3d".into());
        l.tags.push("cam-precise".into());
        let sized = ["w", "h", "x", "y", "place"].iter().any(|k| n.props.contains_key(*k));
        let idx = if sized {
            self.add(Some(parent), l)
        } else {
            l.layout.pos = Pos::Fill;
            let content = self.scenes[si].content;
            if parent == content {
                // a full-frame 3D scene is the backdrop: in the scene's world, below the 2D content
                let world = self.scenes[si].world;
                let i = self.add(Some(world), l);
                self.kids[world].pop();
                let at = self.kids[world].iter().position(|&k| k == content).unwrap_or(0);
                self.kids[world].insert(at, i);
                i
            } else {
                self.add(Some(parent), l)
            }
        };
        for (ch, v) in [(Ch::CamX, cam.pos[0]), (Ch::CamY, cam.pos[1]), (Ch::CamZ, cam.pos[2]), (Ch::LookX, cam.target[0]), (Ch::LookY, cam.target[1]), (Ch::LookZ, cam.target[2]), (Ch::Fov, cam.fov), (Ch::Roll, cam.roll), (Ch::Ambient, ambient)] {
            self.set_base(idx, ch, TV::N(v));
        }
        // object defaults from the scene line
        let mut dm = Mat3d::default();
        if let Some(k) = praw(n, "material") {
            dm.kind = self.s3d_material_kind(&k, n.span);
        }
        if let Some(v) = self.s3d_num(n, "outline", 0.0, 64.0) {
            dm.outline = v;
        }
        if let Some(c) = self.color(n, "outline-color") {
            dm.outline_color = rgba(&c);
        }
        if let Some(c) = self.color(n, "color") {
            dm.color = rgba(&c);
        } else {
            dm.color = [0.85, 0.87, 0.9, 1.0];
        }
        for c in &n.children {
            let k = c.kind.as_str();
            if OBJ_KINDS.contains(&k) {
                self.obj3d(idx, si, c, &dm);
            } else if !(k == "at" || k == "anim" || k == "set" || k == "stack" || is_verb(k)) {
                let hint = suggest(k, OBJ_KINDS).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("3D objects: {}", OBJ_KINDS.join(" ")));
                self.err("E_S3D_KIND", format!("`{k}` is not a 3D object (inside scene3d)"), hint, c.span);
            }
        }
        idx
    }

    fn s3d_material_kind(&mut self, k: &str, span: Span) -> String {
        const KINDS: &[&str] = &["flat", "lambert", "toon", "phong", "wire"];
        if KINDS.contains(&k) {
            k.to_string()
        } else {
            let hint = suggest(k, KINDS).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("materials: {}", KINDS.join(" ")));
            self.err("E_S3D", format!("unknown material `{k}`"), hint, span);
            "toon".into()
        }
    }

    /// One object (and its object children).
    fn obj3d(&mut self, parent: usize, si: usize, n: &Node, dm: &Mat3d) -> Option<usize> {
        let k = n.kind.as_str();
        self.s3d_props(n, &[OBJ_PROPS, shape_props(k)]);
        let user = id_arg(n);
        let lid = match &user {
            Some(u) => {
                if self.names.get(&format!("{u}@{si}")).is_some() {
                    self.err("E_DUP_ID", format!("id `#{u}` is used twice in scene `{}`", self.scenes[si].id), "give each element a unique id within its scene", n.span);
                }
                format!("u-{u}-{si}")
            }
            None => String::new(),
        };
        let pos_num = |me: &mut Self, key: &str, lo: f64, hi: f64| me.s3d_num(n, key, lo, hi);
        let mut mat = dm.clone();
        if k == "screen" {
            mat.kind = "flat".into();
            mat.color = [0.04, 0.045, 0.05, 1.0];
        }
        if let Some(km) = praw(n, "material") {
            mat.kind = self.s3d_material_kind(&km, n.span);
        }
        if let Some(c) = self.color(n, "color") {
            mat.color = rgba(&c);
        }
        if let Some(v) = pos_num(self, "opacity", 0.0, 1.0) {
            mat.opacity = v;
        }
        if let Some(v) = pos_num(self, "emissive", 0.0, 100.0) {
            mat.emissive = v;
        }
        if let Some(v) = pos_num(self, "bands", 2.0, 16.0) {
            mat.bands = v.round() as u32;
        }
        if let Some(v) = pos_num(self, "specular", 0.0, 10.0) {
            mat.specular = v;
        }
        if let Some(v) = pos_num(self, "shininess", 1.0, 1000.0) {
            mat.shininess = v;
        }
        if let Some(v) = pos_num(self, "outline", 0.0, 64.0) {
            mat.outline = v;
        }
        if let Some(c) = self.color(n, "outline-color") {
            mat.outline_color = rgba(&c);
        }
        if let Some(v) = pos_num(self, "crease", 0.0, 180.0) {
            mat.crease = v;
        }
        if let Some(v) = pos_num(self, "wire", 0.0, 64.0) {
            mat.wire = v;
        }
        if let Some(c) = self.color(n, "wire-color") {
            mat.wire_color = rgba(&c);
        }
        mat.double_sided = flag(n, "double-sided");
        let texture = praw(n, "texture").map(|t| {
            let p = self.asset(&t, n);
            if !p.to_ascii_lowercase().ends_with(".png") {
                self.err("E_S3D", format!("texture `{t}`: PNG only"), "convert the image to PNG", n.span);
            }
            p
        });
        // shape
        let size1 = |me: &mut Self, key: &str, d: f64| me.s3d_num(n, key, 1e-6, 1e6).unwrap_or(d);
        let mut src = None;
        let mut src_name = None;
        let mut font = None;
        let mut light = None;
        let mut receive = flag(n, "ground");
        let mut rot_default = [0.0; 3];
        let shape = match k {
            "box" | "cube" => {
                let s = self.s3d_num(n, "size", 1e-6, 1e6).unwrap_or(1.0);
                let size = [size1(self, "w", s), size1(self, "h", s), size1(self, "d", s)];
                let bevel = self.s3d_num(n, "bevel", 0.0, 1e6).unwrap_or(0.0);
                let mx = size.iter().cloned().fold(f64::MAX, f64::min) / 2.0;
                if bevel > mx {
                    self.warn("W_S3D_SIZE", format!("box bevel={bevel} is more than half its smallest side"), format!("use bevel <= {mx}"), n.span);
                }
                Shape3d::Box { size, radius: bevel.min(mx) }
            }
            "plane" => {
                let s = self.s3d_num(n, "size", 1e-6, 1e6).unwrap_or(1.0);
                Shape3d::Plane { size: [size1(self, "w", s), size1(self, "h", s)] }
            }
            "floor" => {
                let s = self.s3d_num(n, "size", 1e-6, 1e6).unwrap_or(200.0);
                receive = true;
                rot_default = [-90.0, 0.0, 0.0];
                // a floor takes no default outline (its far edge would draw a horizon line)
                if !n.props.contains_key("outline") {
                    mat.outline = 0.0;
                }
                Shape3d::Plane { size: [size1(self, "w", s), size1(self, "d", s)] }
            }
            "sphere" => Shape3d::Sphere { radius: size1(self, "r", 0.5), segments: self.s3d_num(n, "seg", 3.0, 256.0).unwrap_or(48.0) as u32 },
            "cylinder" | "cone" => {
                let r = size1(self, "r", 0.5);
                let r2 = if k == "cone" { Some(0.0) } else { self.s3d_num(n, "r2", 0.0, 1e6) };
                Shape3d::Cylinder { radius: r, radius_top: r2, height: size1(self, "h", 1.0), segments: self.s3d_num(n, "seg", 3.0, 256.0).unwrap_or(48.0) as u32 }
            }
            "torus" => Shape3d::Torus { radius: size1(self, "r", 0.8), tube: size1(self, "tube", 0.25), segments: self.s3d_num(n, "seg", 3.0, 256.0).unwrap_or(48.0) as u32 },
            "extrude" => {
                let d = praw(n, "d");
                let text = if d.is_some() { None } else { text_arg(n) };
                if d.is_none() && text.as_deref().is_none_or(|t| t.trim().is_empty()) {
                    self.err("E_S3D", "extrude needs text or a path", "write `extrude #L \"L\" depth=0.6` or `extrude #logo d=\"M0 0 L10 0 L5 8 Z\"`", n.span);
                    return None;
                }
                if let Some(d) = &d {
                    if kurbo_like_path_invalid(d) {
                        self.err("E_S3D", "extrude d=: not an SVG path", "write SVG path data, e.g. `d=\"M0 0 L10 0 L5 8 Z\"`", n.span);
                        return None;
                    }
                }
                if text.is_some() {
                    let fam = praw(n, "font").unwrap_or_else(|| "Inter".into());
                    font = Some(crate::cloud::font_file(&fam));
                }
                Shape3d::Extrude {
                    text,
                    font: 0,
                    size: size1(self, "size", 1.0),
                    weight: self.s3d_num(n, "weight", 1.0, 1000.0).unwrap_or(800.0),
                    letter_spacing: self.s3d_num(n, "tracking", -1.0, 2.0).unwrap_or(0.0),
                    d,
                    depth: self.s3d_num(n, "depth", 0.0, 1e6).unwrap_or(0.4),
                }
            }
            "mesh" => {
                let Some(s) = praw(n, "src") else {
                    self.err("E_S3D", "mesh needs src=model.obj", "a Wavefront OBJ file relative to the .reel file", n.span);
                    return None;
                };
                let p = self.asset(&s, n);
                if !p.to_ascii_lowercase().ends_with(".obj") {
                    self.err("E_S3D", format!("mesh src={s}: only Wavefront .obj is supported"), "export the model as OBJ (glTF is planned)", n.span);
                }
                Shape3d::Mesh { path: p, size: size1(self, "size", 1.0) }
            }
            "screen" => {
                match praw(n, "src") {
                    Some(s) => {
                        let id = s.trim_start_matches('#').to_string();
                        src_name = Some(id);
                    }
                    None => self.err("E_S3D_SCREEN", "screen needs src=#layer", "point it at a 2D layer of this scene: `screen #crt w=4 h=2.25 src=#ui`", n.span),
                }
                let w = size1(self, "w", 4.0);
                let h = size1(self, "h", w * 9.0 / 16.0);
                src = Some(String::new());
                Shape3d::Screen { size: [w, h], node: 0, rect: [0.0, 0.0, 1.0, 1.0], res: self.s3d_num(n, "res", 0.0, 4.0).unwrap_or(0.0) }
            }
            "light" => {
                let (kind, pos) = if let Some(p) = self.s3d_vec(n, "point") {
                    ("point", p)
                } else if let Some(d) = self.s3d_vec(n, "dir") {
                    ("directional", d)
                } else {
                    let p = [self.s3d_num(n, "x", -1e6, 1e6).unwrap_or(0.0), self.s3d_num(n, "y", -1e6, 1e6).unwrap_or(3.0), self.s3d_num(n, "z", -1e6, 1e6).unwrap_or(0.0)];
                    ("point", p)
                };
                let color = self.color(n, "color").map(|c| rgba(&c)).unwrap_or([1.0; 4]);
                let intensity = self.s3d_num(n, "intensity", 0.0, 1000.0).unwrap_or(0.8);
                let range = self.s3d_num(n, "range", 0.0, 1e6).unwrap_or(0.0);
                light = Some(Light3d { kind: kind.into(), pos, color, intensity, range });
                Shape3d::Group
            }
            _ => Shape3d::Group,
        };
        let mut pos = self.s3d_vec(n, "pos").unwrap_or([0.0; 3]);
        if light.is_none() {
            for (i, key) in ["x", "y", "z"].iter().enumerate() {
                if let Some(v) = self.s3d_num(n, key, -1e7, 1e7) {
                    pos[i] = v;
                }
            }
        } else if let Some(l) = &light {
            pos = l.pos;
        }
        let mut rot = match praw(n, "rot") {
            Some(r) => match parse_vec3(&r) {
                Some(v) => v,
                None => {
                    // a single angle turns about y (yaw)
                    let v = self.s3d_num(n, "rot", -1e6, 1e6).unwrap_or(0.0);
                    [rot_default[0], v, rot_default[2]]
                }
            },
            None => rot_default,
        };
        for (i, key) in ["rx", "ry", "rz"].iter().enumerate() {
            if let Some(v) = self.s3d_num(n, key, -1e6, 1e6) {
                rot[i] = v;
            }
        }
        let mut scale = [1.0; 3];
        if let Some(r) = praw(n, "scale") {
            match parse_vec3(&r) {
                Some(v) => scale = v,
                None => {
                    if let Some(v) = self.s3d_num(n, "scale", 0.0, 1e6) {
                        scale = [v; 3];
                    }
                }
            }
        }
        for (i, key) in ["sx", "sy", "sz"].iter().enumerate() {
            if let Some(v) = self.s3d_num(n, key, 0.0, 1e6) {
                scale[i] = v;
            }
        }
        let pivot = self.s3d_vec(n, "pivot").unwrap_or([0.0; 3]);
        let cast = praw(n, "shadow").as_deref() != Some("off") && k != "floor";
        let obj = Obj3d { id: user.clone().unwrap_or_default(), parent: None, shape, pos, rot, scale, pivot, material: mat.clone(), cast, receive };
        let light_intensity = light.as_ref().map(|l| l.intensity);
        let cfg = ObjCfg { kind: k.to_string(), obj, src, src_name, font, texture, light, span: (n.span.line, n.span.col) };
        let mut l = Layer::new(lid, Kind::Obj3d(Box::new(cfg)));
        l.layout.pos = Pos::Abs;
        l.layout.x = Some(Len::px(0.0));
        l.layout.y = Some(Len::px(0.0));
        l.layout.w = Some(Len::px(0.0));
        l.layout.h = Some(Len::px(0.0));
        l.tags.push("obj3d".into());
        // world units: no 1/64 px quantization of x/y/z (lower.rs `normalize_track`)
        l.tags.push("cam-precise".into());
        let i = self.add(Some(parent), l);
        if let Some(u) = &user {
            self.layers[i].name = Some(u.clone());
            self.names.insert(u.clone(), Named { idx: i, scene: si, kind: k.to_string() });
            self.names.insert(format!("{u}@{si}"), Named { idx: i, scene: si, kind: k.to_string() });
        }
        // base values of the animatable channels (verbs start from them)
        let base: [(Ch, f64, f64); 11] = [
            (Ch::X, pos[0], 0.0),
            (Ch::Y, pos[1], 0.0),
            (Ch::Z, pos[2], 0.0),
            (Ch::RotX, rot[0], 0.0),
            (Ch::RotY, rot[1], 0.0),
            (Ch::Rot, rot[2], 0.0),
            (Ch::Sx, scale[0], 1.0),
            (Ch::Sy, scale[1], 1.0),
            (Ch::Sz, scale[2], 1.0),
            (Ch::Opacity, mat.opacity, 1.0),
            (Ch::Emissive, mat.emissive, 0.0),
        ];
        for (ch, v, d) in base {
            if v != d {
                self.set_base(i, ch, TV::N(v));
            }
        }
        if let Some(li) = light_intensity {
            self.set_base(i, Ch::Intensity, TV::N(li));
        }
        self.set_base(i, Ch::Color, TV::C(Rgba { r: mat.color[0], g: mat.color[1], b: mat.color[2], a: mat.color[3] }));
        for c in &n.children {
            let ck = c.kind.as_str();
            if OBJ_KINDS.contains(&ck) {
                self.obj3d(i, si, c, dm);
            } else if !(ck == "at" || ck == "anim" || ck == "set" || ck == "stack" || is_verb(ck)) {
                let hint = suggest(ck, OBJ_KINDS).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("3D objects: {}", OBJ_KINDS.join(" ")));
                self.err("E_S3D_KIND", format!("`{ck}` is not a 3D object (inside scene3d)"), hint, c.span);
            }
        }
        Some(i)
    }

    /// Resolve `screen src=#id` against the scene's names (after the whole scene is built):
    /// the source layer is hidden in the 2D pass.
    pub fn s3d_resolve_screens(&mut self, si: usize) {
        let objs: Vec<usize> = (0..self.layers.len()).filter(|&i| matches!(&self.layers[i].kind, Kind::Obj3d(c) if c.src.is_some())).collect();
        for i in objs {
            let Some(c) = self.layers[i].kind.as_obj3d().cloned() else { continue };
            let Some(sname) = c.src_name.clone() else { continue };
            // the screen must belong to this scene
            let mine = self.names.values().any(|nm| nm.idx == i && nm.scene == si) || self.scene_of_layer(i) == Some(si);
            if !mine {
                continue;
            }
            let span = Span { line: c.span.0, col: c.span.1 };
            match self.names.get(&format!("{sname}@{si}")).map(|nm| nm.idx) {
                Some(j) if matches!(self.layers[j].kind, Kind::Obj3d(_) | Kind::Scene3d(_)) => {
                    self.err("E_S3D_SCREEN", format!("screen src=#{sname}: a 3D object or scene cannot be a screen source"), "point it at a 2D layer (a `layer`, `window`, `code`, `chart`, ...)", span);
                }
                Some(j) => {
                    if self.contains_layer(j, i) {
                        self.err("E_S3D_SCREEN", format!("screen src=#{sname} contains the 3D scene itself"), "use a layer outside the scene3d", span);
                        continue;
                    }
                    self.layers[j].style.screen_src = true;
                    if self.layers[j].layout.pos == Pos::Flow {
                        self.warn("W_S3D_SCREEN", format!("screen source `#{sname}` sits in the scene's layout flow (it is hidden, but takes space)"), format!("place it absolutely: `layer #{sname} x=0 y=0 w=1280 h=720`"), span);
                    }
                    let lid = self.lid(j);
                    if let Kind::Obj3d(c) = &mut self.layers[i].kind {
                        c.src = Some(lid);
                    }
                }
                None => {
                    let cands: Vec<String> = self.names.iter().filter(|(k, v)| v.scene == si && k.ends_with(&format!("@{si}")) && !matches!(self.layers[v.idx].kind, Kind::Obj3d(_))).map(|(k, _)| k.split('@').next().unwrap().to_string()).collect();
                    let refs: Vec<&str> = cands.iter().map(|s| s.as_str()).collect();
                    let hint = suggest(&sname, &refs).map(|s| format!("did you mean `#{s}`?")).unwrap_or_else(|| format!("declare it in this scene: `layer #{sname} x=0 y=0 w=1280 h=720` with the UI inside"));
                    self.err("E_S3D_SCREEN", format!("screen src=#{sname}: no layer `#{sname}` in this scene"), hint, span);
                }
            }
        }
    }

    fn contains_layer(&self, root: usize, x: usize) -> bool {
        let mut st = vec![root];
        while let Some(a) = st.pop() {
            if a == x {
                return true;
            }
            st.extend(self.kids[a].iter().copied());
        }
        false
    }

    fn scene_of_layer(&self, i: usize) -> Option<usize> {
        (0..self.scenes.len()).find(|&si| self.contains_layer(self.scenes[si].root, i))
    }

    // ------------------------------------------------------------ verbs

    /// 3D verbs and presets. Returns true when the event was handled here.
    pub fn s3d_apply(&mut self, e: &Ev) -> bool {
        let n = &e.node;
        let k = n.kind.as_str();
        const MINE: &[&str] = &["orbit", "dolly", "truck", "pedestal", "lookat", "camera3d", "spin", "tumble", "drop", "assemble", "explode", "stack", "enter", "exit", "show", "hide"];
        if !MINE.contains(&k) {
            return false;
        }
        let name = id_arg(n).or_else(|| e.target.clone());
        let target = name.as_ref().and_then(|nm| self.names.get(&format!("{nm}@{}", e.scene)).map(|x| x.idx));
        let kind_3d = target.map(|i| match &self.layers[i].kind {
            Kind::Scene3d(_) => 1,
            Kind::Obj3d(_) => 2,
            _ => 0,
        });
        match (k, kind_3d) {
            ("orbit" | "dolly", Some(1)) | ("truck" | "pedestal" | "lookat" | "camera3d", _) => {
                let Some(i) = target.filter(|_| kind_3d == Some(1)) else {
                    self.err("E_S3D_TARGET", format!("`{k}` moves the camera of a scene3d"), format!("write `{k} #world ...` with the scene3d's id"), n.span);
                    return true;
                };
                self.s3d_camera(e, i);
                true
            }
            ("spin" | "tumble" | "assemble" | "explode" | "stack", _) => {
                let Some(i) = target.filter(|_| kind_3d == Some(2)) else {
                    self.err("E_S3D_TARGET", format!("`{k}` needs a 3D object or group"), format!("write `{k} #crate ...` with the id of an object inside a scene3d"), n.span);
                    return true;
                };
                self.s3d_object_verb(e, i);
                true
            }
            ("drop", Some(2)) => {
                let i = target.unwrap();
                let from = self.s3d_num(n, "from", 0.0, 1e6).unwrap_or(3.0);
                let bounce = self.s3d_num(n, "bounce", 0.0, 0.95).unwrap_or(0.35);
                let g = self.s3d_num(n, "g", 0.01, 1e4).unwrap_or(25.0);
                self.s3d_drop(i, e.t, from, bounce, g);
                true
            }
            ("enter" | "exit" | "show" | "hide", Some(2)) => {
                let i = target.unwrap();
                let preset = if k == "show" || k == "hide" { k.to_string() } else { n.args.iter().filter_map(|a| a.as_str()).find(|s| !s.starts_with('#')).map(str::to_string).unwrap_or_else(|| "fade".into()) };
                self.s3d_preset(e, i, k == "enter" || k == "show", &preset);
                true
            }
            _ => false,
        }
    }

    /// from -> to; an untouched channel takes the start value as its base (an entrance hides
    /// the object before it starts, like the 2D presets)
    #[allow(clippy::too_many_arguments)]
    fn s3d_ft(&mut self, i: usize, ch: Ch, t0: f64, t1: f64, a: TV, b: TV, ease: Ease) {
        let tr = self.track(i, ch);
        if tr.keys.is_empty() {
            tr.base = a.clone();
        }
        tr.from_to(t0, t1, a, b, ease);
    }

    fn cur(&mut self, i: usize, ch: Ch, t: f64) -> f64 {
        self.track(i, ch).eval(t).n()
    }

    /// Per-frame keys from t0 to t0 + d for channel `ch` (values from `f(eased progress)`).
    fn bake_ch(&mut self, i: usize, ch: Ch, t0: f64, d: f64, ease: &Ease, f: &dyn Fn(f64) -> f64) {
        let fps = self.fps.max(1.0);
        let n = ((d * fps).ceil() as usize).max(1);
        let keys: Vec<(f64, TV)> = (0..=n).map(|j| {
            let u = (j as f64 / n as f64).min(1.0);
            (t0 + d * u, TV::N(f(ease.sample(u))))
        }).collect();
        self.track(i, ch).bake(t0, &keys);
    }

    /// Camera moves: `orbit #w by=30deg [pitch=10deg]`, `dolly #w by=2|to=5`, `truck #w by=1`,
    /// `pedestal #w by=1`, `lookat #w #obj|at=(x, y, z)`, `camera3d #w orbit= pitch= dolly= truck=
    /// pedestal= lookat= fov= roll=` (all at once); `dur= ease=`.
    fn s3d_camera(&mut self, e: &Ev, i: usize) {
        let n = e.node.clone();
        let t = e.t;
        let k = n.kind.as_str();
        const PROPS: &[&str] = &["by", "to", "pitch", "dur", "ease", "at", "orbit", "dolly", "truck", "pedestal", "lookat", "fov", "roll"];
        for key in n.props.keys() {
            if !PROPS.contains(&key.as_str()) {
                let hint = suggest(key, PROPS).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("camera moves take: {}", PROPS.join(" ")));
                self.warn("W_S3D_PROP", format!("{k}: unknown prop `{key}`"), hint, n.span);
            }
        }
        let d = self.dur(&n, "dur").unwrap_or(2.0).max(1e-3);
        let ease = self.ease(&n, "ease", STD);
        // a positional angle/amount: `orbit #w 30deg`
        let pos_arg = n.args.iter().filter_map(|a| match a {
            crate::ast::Value::Num(x) => Some(*x),
            crate::ast::Value::Str(s) if !s.starts_with('#') => parse_angle_deg(s).ok(),
            _ => None,
        }).next();
        let by = |me: &mut Self, key: &str| -> Option<f64> { me.s3d_num(&n, key, -1e6, 1e6) };
        let (mut yaw, mut pitch, mut dolly, mut dolly_to, mut truck, mut ped, mut look, mut fov, mut roll) = (0.0, 0.0, 0.0, None, 0.0, 0.0, None, None, None);
        match k {
            "orbit" => {
                yaw = by(self, "by").or(pos_arg).unwrap_or(30.0);
                pitch = by(self, "pitch").unwrap_or(0.0);
            }
            "dolly" => {
                dolly = by(self, "by").or(pos_arg).unwrap_or(0.0);
                dolly_to = self.s3d_num(&n, "to", 1e-6, 1e7);
                if !n.props.contains_key("by") && !n.props.contains_key("to") && pos_arg.is_none() {
                    self.err("E_S3D", "dolly needs by= (forward, world units) or to= (distance to the target)", "write `dolly #world by=2 dur=2s` (negative = back)", n.span);
                    return;
                }
            }
            "truck" => truck = by(self, "by").or(pos_arg).unwrap_or(1.0),
            "pedestal" => ped = by(self, "by").or(pos_arg).unwrap_or(1.0),
            "lookat" => {}
            _ => {
                yaw = by(self, "orbit").unwrap_or(0.0);
                pitch = by(self, "pitch").unwrap_or(0.0);
                dolly = by(self, "dolly").unwrap_or(0.0);
                truck = by(self, "truck").unwrap_or(0.0);
                ped = by(self, "pedestal").unwrap_or(0.0);
                fov = self.s3d_num(&n, "fov", 1.0, 170.0);
                roll = by(self, "roll");
            }
        }
        if k == "lookat" || n.props.contains_key("lookat") {
            let key = if k == "lookat" { "at" } else { "lookat" };
            let ids: Vec<String> = n.args.iter().filter_map(|a| a.as_str()).filter(|s| s.starts_with('#')).map(|s| s[1..].to_string()).collect();
            let raw = praw(&n, key).unwrap_or_default();
            let tid = if k == "lookat" { ids.get(1).cloned() } else { None }.or_else(|| raw.strip_prefix('#').map(str::to_string));
            look = match tid {
                Some(id) => match self.names.get(&format!("{id}@{}", e.scene)).map(|x| x.idx) {
                    Some(j) if matches!(self.layers[j].kind, Kind::Obj3d(_)) => Some(self.s3d_world_pos(j, t + d)),
                    _ => {
                        self.err("E_S3D_TARGET", format!("{k}: `#{id}` is not a 3D object"), "aim at an object of the scene3d or a point: `at=(0, 1, 0)`", n.span);
                        return;
                    }
                },
                None => match parse_vec3(&raw) {
                    Some(v) => Some(v),
                    None => {
                        self.err("E_S3D", format!("{k} needs a target"), "write `lookat #world #crate` or `lookat #world at=(0, 1, 0)`", n.span);
                        return;
                    }
                },
            };
        }
        let p0 = [self.cur(i, Ch::CamX, t), self.cur(i, Ch::CamY, t), self.cur(i, Ch::CamZ, t)];
        let t0v = [self.cur(i, Ch::LookX, t), self.cur(i, Ch::LookY, t), self.cur(i, Ch::LookZ, t)];
        let off = m::sub(p0, t0v);
        let r0 = m::len(off).max(1e-9);
        let yaw0 = off[0].atan2(off[2]);
        let pitch0 = (off[1] / r0).clamp(-1.0, 1.0).asin();
        let fwd = m::norm(m::mul(off, -1.0));
        let right = m::norm(m::cross(fwd, [0.0, 1.0, 0.0]));
        let t1v = look.unwrap_or(t0v);
        let r1 = dolly_to.unwrap_or(r0 - dolly);
        if r1 <= 1e-3 {
            self.warn("W_S3D_CLIP", format!("{k}: the camera would reach or pass its target (distance {r1:.2})"), "dolly less, or move the target with lookat", n.span);
        }
        let r1 = r1.max(1e-3);
        let state = move |p: f64| -> (V3, V3) {
            let tgt = m::add(m::add(m::add(t0v, m::mul(m::sub(t1v, t0v), p)), m::mul(right, truck * p)), [0.0, ped * p, 0.0]);
            let yw = yaw0 + (yaw * p).to_radians();
            let pt = (pitch0 + (pitch * p).to_radians()).clamp(-1.55, 1.55);
            let r = r0 + (r1 - r0) * p;
            let o = [r * pt.cos() * yw.sin(), r * pt.sin(), r * pt.cos() * yw.cos()];
            (m::add(tgt, o), tgt)
        };
        let moves_pos = yaw != 0.0 || pitch != 0.0 || (r1 - r0).abs() > 1e-12 || truck != 0.0 || ped != 0.0 || look.is_some();
        if moves_pos {
            for (ax, ch) in [(0usize, Ch::CamX), (1, Ch::CamY), (2, Ch::CamZ)] {
                self.bake_ch(i, ch, t, d, &ease, &|p| state(p).0[ax]);
            }
            for (ax, ch) in [(0usize, Ch::LookX), (1, Ch::LookY), (2, Ch::LookZ)] {
                self.bake_ch(i, ch, t, d, &ease, &|p| state(p).1[ax]);
            }
        }
        if let Some(f) = fov {
            self.track(i, Ch::Fov).to(t, t + d, TV::N(f), ease.clone());
        }
        if let Some(r) = roll {
            let cur = self.cur(i, Ch::Roll, t);
            self.track(i, Ch::Roll).to(t, t + d, TV::N(cur + r), ease.clone());
        }
    }

    /// World position of an object's origin at time t (through its groups).
    pub fn s3d_world_pos(&mut self, i: usize, t: f64) -> V3 {
        let mut chain = vec![];
        let mut x = i;
        while matches!(self.layers[x].kind, Kind::Obj3d(_)) {
            chain.push(x);
            match self.kids.iter().position(|k| k.contains(&x)) {
                Some(p) => x = p,
                None => break,
            }
        }
        let mut mtx = m::M4::IDENTITY;
        for &j in chain.iter().rev() {
            let piv = self.layers[j].kind.as_obj3d().map(|c| c.obj.pivot).unwrap_or([0.0; 3]);
            let pos = [self.cur(j, Ch::X, t), self.cur(j, Ch::Y, t), self.cur(j, Ch::Z, t)];
            let rot = [self.cur(j, Ch::RotX, t), self.cur(j, Ch::RotY, t), self.cur(j, Ch::Rot, t)];
            let s = self.cur(j, Ch::Scale, t);
            let sc = [s * self.cur(j, Ch::Sx, t), s * self.cur(j, Ch::Sy, t), s * self.cur(j, Ch::Sz, t)];
            let local = m::M4::translate(pos).mul(&m::M4::rot_y(rot[1])).mul(&m::M4::rot_x(rot[0])).mul(&m::M4::rot_z(rot[2])).mul(&m::M4::scale(sc)).mul(&m::M4::translate([-piv[0], -piv[1], -piv[2]]));
            mtx = mtx.mul(&local);
        }
        mtx.point([0.0; 3])
    }

    /// Bouncing fall onto the object's current height: from `from` above, restitution `bounce`,
    /// gravity `g` (units/s^2), baked per frame.
    fn s3d_drop(&mut self, i: usize, t: f64, from: f64, bounce: f64, g: f64) {
        let rest = self.cur(i, Ch::Y, t);
        let fps = self.fps.max(1.0);
        // segment list: (start time, start velocity up), first a fall from rest + from
        let t_fall = (2.0 * from / g).sqrt();
        let mut v = g * t_fall * bounce;
        let mut segs = vec![];
        let mut tt = t_fall;
        while v > 0.4 && segs.len() < 6 {
            let dur = 2.0 * v / g;
            segs.push((tt, v, dur));
            tt += dur;
            v *= bounce;
        }
        let total = tt.max(1.0 / fps);
        let n = (total * fps).ceil() as usize;
        let y_at = |s: f64| -> f64 {
            if s < t_fall {
                return rest + from - 0.5 * g * s * s;
            }
            for &(a, v0, dur) in &segs {
                if s >= a && s < a + dur {
                    let u = s - a;
                    return rest + v0 * u - 0.5 * g * u * u;
                }
            }
            rest
        };
        let keys: Vec<(f64, TV)> = (0..=n).map(|j| {
            let s = (j as f64 / fps).min(total);
            (t + s, TV::N(y_at(s)))
        }).collect();
        let tr = self.track(i, Ch::Y);
        tr.jump(t - 1e-3, TV::N(rest + from));
        tr.bake(t, &keys);
    }

    /// Children of a group (or the object itself when it has none).
    fn pieces(&self, i: usize) -> Vec<usize> {
        let kids: Vec<usize> = self.kids[i].iter().copied().filter(|&c| matches!(self.layers[c].kind, Kind::Obj3d(_))).collect();
        if kids.is_empty() { vec![i] } else { kids }
    }

    /// `spin`, `tumble`, `assemble`, `explode`, `stack`.
    fn s3d_object_verb(&mut self, e: &Ev, i: usize) {
        let n = e.node.clone();
        let t = e.t;
        let k = n.kind.as_str();
        let props: &[&str] = match k {
            "spin" => &["axis", "by", "turns", "dur", "ease"],
            "tumble" => &["speed", "dur", "ease"],
            "assemble" | "explode" => &["dist", "stagger", "dur", "ease", "seed", "spin"],
            _ => &["gap", "axis", "dur", "stagger", "from", "ease"],
        };
        for key in n.props.keys() {
            if !props.contains(&key.as_str()) {
                let hint = suggest(key, props).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("`{k}` takes: {}", props.join(" ")));
                self.warn("W_S3D_PROP", format!("{k}: unknown prop `{key}`"), hint, n.span);
            }
        }
        match k {
            "spin" => {
                let axis = praw(&n, "axis").unwrap_or_else(|| "y".into());
                let ch = match axis.as_str() {
                    "x" => Ch::RotX,
                    "z" => Ch::Rot,
                    "y" => Ch::RotY,
                    other => {
                        self.err("E_S3D", format!("spin axis={other}"), "axis=x|y|z", n.span);
                        return;
                    }
                };
                let by = self.s3d_num(&n, "by", -1e6, 1e6).or_else(|| self.s3d_num(&n, "turns", -1e4, 1e4).map(|v| v * 360.0)).unwrap_or(360.0);
                let d = self.dur(&n, "dur").unwrap_or(1.2).max(1e-3);
                let ease = self.ease(&n, "ease", STD);
                let cur = self.cur(i, ch, t);
                self.track(i, ch).to(t, t + d, TV::N(cur + by), ease);
            }
            "tumble" => {
                let d = self.dur(&n, "dur").unwrap_or(2.0).max(1e-3);
                let sp = self.s3d_num(&n, "speed", -1e5, 1e5).unwrap_or(120.0);
                let ease = self.ease(&n, "ease", Ease::Linear);
                for (ch, f) in [(Ch::RotX, 0.7), (Ch::RotY, 1.0), (Ch::Rot, 0.35)] {
                    let cur = self.cur(i, ch, t);
                    self.track(i, ch).to(t, t + d, TV::N(cur + sp * d * f), ease.clone());
                }
            }
            "assemble" | "explode" => {
                let out = k == "explode";
                let d = self.dur(&n, "dur").unwrap_or(0.9).max(1e-3);
                let st = self.dur(&n, "stagger").unwrap_or(0.07);
                let dist = self.s3d_num(&n, "dist", 0.0, 1e6).unwrap_or(4.0);
                let spin = self.s3d_num(&n, "spin", 0.0, 1e5).unwrap_or(160.0);
                let seed = self.s3d_num(&n, "seed", 0.0, 1e12).unwrap_or(1.0) as u64;
                let ease = self.ease(&n, "ease", if out { Ease::Cubic(0.55, 0.0, 1.0, 0.45) } else { BACK });
                for (j, p) in self.pieces(i).into_iter().enumerate() {
                    let tj = t + st * j as f64;
                    let r = |s: u64| -> f64 { (splitmix(seed.wrapping_mul(0x9E37).wrapping_add((j as u64) << 8).wrapping_add(s)) as f64 / u64::MAX as f64) * 2.0 - 1.0 };
                    // a direction biased away from the camera-facing plane, never zero
                    let dir = m::norm([r(1), r(2) * 0.8 + 0.35, r(3) * 0.6 + 0.2]);
                    for (ax, ch) in [(0usize, Ch::X), (1, Ch::Y), (2, Ch::Z)] {
                        let rest = self.cur(p, ch, tj);
                        let far = rest + dir[ax] * dist;
                        if out {
                            self.track(p, ch).to(tj, tj + d, TV::N(far), ease.clone());
                        } else {
                            self.s3d_ft(p, ch, tj, tj + d, TV::N(far), TV::N(rest), ease.clone());
                        }
                    }
                    for (q, ch) in [(4u64, Ch::RotX), (5, Ch::RotY), (6, Ch::Rot)] {
                        let rest = self.cur(p, ch, tj);
                        let far = rest + r(q) * spin;
                        if out {
                            self.track(p, ch).to(tj, tj + d, TV::N(far), ease.clone());
                        } else {
                            self.s3d_ft(p, ch, tj, tj + d, TV::N(far), TV::N(rest), ease.clone());
                        }
                    }
                    let o = self.cur(p, Ch::Opacity, tj);
                    if out {
                        self.track(p, Ch::Opacity).to(tj + d * 0.6, tj + d, TV::N(0.0), Ease::Linear);
                    } else {
                        self.s3d_ft(p, Ch::Opacity, tj, tj + d * 0.3, TV::N(0.0), TV::N(o.max(1e-3)), Ease::Linear);
                    }
                }
            }
            _ => {
                // stack: pile the children along an axis, each falling in from `from` above
                let axis = praw(&n, "axis").unwrap_or_else(|| "y".into());
                let ax = match axis.as_str() {
                    "x" => 0usize,
                    "y" => 1,
                    "z" => 2,
                    other => {
                        self.err("E_S3D", format!("stack axis={other}"), "axis=x|y|z", n.span);
                        return;
                    }
                };
                let ch = [Ch::X, Ch::Y, Ch::Z][ax];
                let gap = self.s3d_num(&n, "gap", -1e6, 1e6).unwrap_or(0.04);
                let d = self.dur(&n, "dur").unwrap_or(0.45).max(0.0);
                let st = self.dur(&n, "stagger").unwrap_or(0.18);
                let from = self.s3d_num(&n, "from", 0.0, 1e6).unwrap_or(3.0);
                let ease = self.ease(&n, "ease", Ease::Cubic(0.5, 0.0, 0.75, 0.0));
                let pieces = self.pieces(i);
                if pieces.len() == 1 && pieces[0] == i {
                    self.err("E_S3D_TARGET", "stack needs a group with objects inside", "write `group #ship` with `box` children, then `@1s stack #ship`", n.span);
                    return;
                }
                // the pile starts at the group's origin (the first piece's bottom face there)
                let mut level = 0.0f64;
                for (j, p) in pieces.into_iter().enumerate() {
                    let tj = t + st * j as f64;
                    let he = self.layers[p].kind.as_obj3d().and_then(half_extent).map(|h| h[ax]).unwrap_or(0.5);
                    let s = self.cur(p, Ch::Scale, tj) * self.cur(p, [Ch::Sx, Ch::Sy, Ch::Sz][ax], tj);
                    let h = he * s;
                    let rest = level + h;
                    level += 2.0 * h + gap;
                    if d <= 0.0 {
                        self.track(p, ch).jump(tj, TV::N(rest));
                    } else {
                        self.s3d_ft(p, ch, tj, tj + d, TV::N(rest + from), TV::N(rest), ease.clone());
                        let o = self.cur(p, Ch::Opacity, tj).max(1e-3);
                        self.s3d_ft(p, Ch::Opacity, tj, tj + d.min(0.15), TV::N(0.0), TV::N(o), Ease::Linear);
                    }
                }
            }
        }
    }

    /// `in=` / `out=` presets (and show/hide) for objects.
    fn s3d_preset(&mut self, e: &Ev, i: usize, enter: bool, preset: &str) {
        let n = e.node.clone();
        let t = e.t;
        let d = self.dur(&n, "dur").unwrap_or(0.7).max(1e-3);
        let ease = self.ease(&n, "ease", if enter { OUT } else { Ease::Cubic(0.32, 0.0, 0.67, 0.0) });
        let o = self.cur(i, Ch::Opacity, t).max(1e-3);
        let o_rest = if enter { self.layers[i].kind.as_obj3d().map(|c| c.obj.material.opacity).unwrap_or(1.0).max(o) } else { o };
        let fade = |me: &mut Self, a: f64, b: f64, dd: f64| {
            me.s3d_ft(i, Ch::Opacity, t, t + dd, TV::N(a), TV::N(b), Ease::Linear);
        };
        match (preset, enter) {
            ("show", _) => self.track(i, Ch::Opacity).jump(t, TV::N(o_rest)),
            ("hide", _) => self.track(i, Ch::Opacity).jump(t, TV::N(0.0)),
            ("fade", true) => fade(self, 0.0, o_rest, d),
            ("fade", false) => fade(self, o, 0.0, d),
            ("pop", _) | ("zoom", _) => {
                let ease = if enter { self.ease(&n, "ease", Ease::Spring { stiffness: 220.0, damping: 16.0, mass: 1.0, velocity: 0.0 }) } else { ease };
                let s = self.cur(i, Ch::Scale, t);
                if enter {
                    self.s3d_ft(i, Ch::Scale, t, t + d, TV::N(0.001), TV::N(s.max(1e-3)), ease);
                    fade(self, 0.0, o_rest, (d * 0.25).max(1e-3));
                } else {
                    self.track(i, Ch::Scale).to(t, t + d, TV::N(0.001), ease);
                    self.track(i, Ch::Opacity).to(t + d * 0.7, t + d, TV::N(0.0), Ease::Linear);
                }
            }
            ("rise", _) | ("sink", _) => {
                let y = self.cur(i, Ch::Y, t);
                let dy = if preset == "rise" { -1.5 } else { -1.5 };
                if enter {
                    self.s3d_ft(i, Ch::Y, t, t + d, TV::N(y + dy), TV::N(y), ease);
                    fade(self, 0.0, o_rest, d * 0.6);
                } else {
                    self.track(i, Ch::Y).to(t, t + d, TV::N(y + dy), ease);
                    self.track(i, Ch::Opacity).to(t, t + d, TV::N(0.0), Ease::Linear);
                }
            }
            ("drop", true) => {
                let from = self.s3d_num(&n, "from", 0.0, 1e6).unwrap_or(3.0);
                self.s3d_drop(i, t, from, 0.35, 25.0);
                fade(self, 0.0, o_rest, 0.08);
            }
            ("spin", _) | ("flip", _) => {
                let ch = if preset == "flip" { Ch::RotX } else { Ch::RotY };
                let r = self.cur(i, ch, t);
                if enter {
                    self.s3d_ft(i, ch, t, t + d, TV::N(r - 180.0), TV::N(r), ease);
                    fade(self, 0.0, o_rest, d * 0.4);
                } else {
                    self.track(i, ch).to(t, t + d, TV::N(r + 180.0), ease);
                    self.track(i, Ch::Opacity).to(t + d * 0.5, t + d, TV::N(0.0), Ease::Linear);
                }
            }
            ("assemble", true) | ("explode", false) => {
                let mut e2 = e.clone();
                e2.node.kind = if enter { "assemble".into() } else { "explode".into() };
                e2.node.props.retain(|k, _| ["dur", "ease"].contains(&k.as_str()));
                self.s3d_object_verb(&e2, i);
            }
            (p, _) => {
                const P: &[&str] = &["fade", "pop", "zoom", "rise", "sink", "drop", "spin", "flip", "assemble", "explode"];
                let hint = suggest(p, P).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("3D presets: {}", P.join(" ")));
                self.err("E_S3D", format!("`{p}` is not a 3D {} preset", if enter { "enter" } else { "exit" }), hint, n.span);
            }
        }
    }

    // ------------------------------------------------------------ lint

    /// Objects behind the camera or outside the frame for their whole visible time, clipped by
    /// the near plane, absurd sizes. Runs after all events (tracks are final).
    pub fn s3d_lint(&mut self) {
        let scenes: Vec<usize> = (0..self.layers.len()).filter(|&i| matches!(self.layers[i].kind, Kind::Scene3d(_))).collect();
        for si3 in scenes {
            let Some(cfg) = self.layers[si3].kind.as_scene3d().cloned() else { continue };
            let sc = &self.scenes[cfg.si];
            let (t0, t1) = (sc.start, sc.start + sc.dur);
            let n = (((t1 - t0) / 0.2).ceil() as usize).clamp(1, 600);
            let times: Vec<f64> = (0..=n).map(|j| t0 + (t1 - t0) * j as f64 / n as f64).collect();
            // objects in preorder with their layer ids
            let mut objs = vec![];
            let mut st = self.kids[si3].iter().rev().copied().collect::<Vec<_>>();
            while let Some(x) = st.pop() {
                if matches!(self.layers[x].kind, Kind::Obj3d(_)) {
                    objs.push(x);
                    st.extend(self.kids[x].iter().rev().copied());
                }
            }
            let aspect = if self.h > 0.0 { self.w / self.h } else { 16.0 / 9.0 };
            for &o in &objs {
                let Some(oc) = self.layers[o].kind.as_obj3d().cloned() else { continue };
                if oc.light.is_some() {
                    continue;
                }
                let span = Span { line: oc.span.0, col: oc.span.1 };
                let name = oc.obj.id.clone();
                let label = if name.is_empty() { format!("(line {})", oc.span.0) } else { format!("#{name}") };
                let Some(he) = half_extent(&oc) else { continue };
                let ext = 2.0 * he[0].max(he[1]).max(he[2]);
                if ext > 1e4 || (ext > 0.0 && ext < 1e-3) {
                    self.warn("W_S3D_SIZE", format!("{} {label}: size {ext:.4} world units is outside a sensible range", oc.kind), "keep objects between about 0.01 and 1000 units (camera near/far and precision)", span);
                }
                let (mut seen, mut in_front, mut clipped, mut max_px) = (false, false, false, 0.0f64);
                for &t in &times {
                    if self.cur(o, Ch::Opacity, t) <= 0.0 {
                        continue;
                    }
                    let cam = Cam3d {
                        pos: [self.cur(si3, Ch::CamX, t), self.cur(si3, Ch::CamY, t), self.cur(si3, Ch::CamZ, t)],
                        target: [self.cur(si3, Ch::LookX, t), self.cur(si3, Ch::LookY, t), self.cur(si3, Ch::LookZ, t)],
                        fov: self.cur(si3, Ch::Fov, t),
                        roll: self.cur(si3, Ch::Roll, t),
                        ..cfg.scene.camera.clone()
                    };
                    let v = m::View3::new(&cam, aspect);
                    // world-space corners of the local box (through groups)
                    let c0 = self.s3d_world_pos(o, t);
                    let s = self.cur(o, Ch::Scale, t).abs() * self.cur(o, Ch::Sx, t).abs().max(self.cur(o, Ch::Sy, t).abs()).max(self.cur(o, Ch::Sz, t).abs());
                    let r = m::len(he) * s.max(1e-9);
                    let pc = v.view.point(c0);
                    let depth = -pc[2];
                    if depth + r < v.near {
                        continue; // behind
                    }
                    in_front = true;
                    if depth - r < v.near {
                        clipped = true;
                    }
                    // sphere bounds against the frustum (a little margin)
                    let (hx, hy) = if v.ortho { (v.half_h * v.aspect, v.half_h) } else { (depth.max(v.near) * v.half_h * v.aspect, depth.max(v.near) * v.half_h) };
                    if pc[0].abs() - r <= hx * 1.02 && pc[1].abs() - r <= hy * 1.02 {
                        seen = true;
                        let px = if v.ortho { r / v.half_h * self.h / 2.0 } else { r / (depth.max(v.near) * v.half_h) * self.h / 2.0 };
                        max_px = max_px.max(px);
                    }
                }
                if !in_front {
                    self.warn("W_S3D_VIEW", format!("{} {label} is behind the camera the whole time", oc.kind), "move it in front of the camera (toward the target), or turn the camera with lookat/orbit", span);
                } else if !seen {
                    self.warn("W_S3D_VIEW", format!("{} {label} is outside the frame the whole time", oc.kind), "check its x/y/z against camera= and target=, or widen fov=", span);
                } else if max_px < 1.5 {
                    self.warn("W_S3D_SIZE", format!("{} {label} is smaller than 2 px on screen", oc.kind), "make it bigger or bring the camera closer", span);
                }
                if clipped && in_front && oc.kind != "floor" {
                    self.warn("W_S3D_CLIP", format!("{} {label} crosses the camera's near plane (it is cut open)", oc.kind), "keep the camera a bit further away, or lower near=", span);
                }
            }
        }
    }
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Quick check that a string looks like SVG path data (starts with a moveto).
fn kurbo_like_path_invalid(d: &str) -> bool {
    !d.trim_start().starts_with(['M', 'm']) || kurbo::BezPath::from_svg(d).is_err()
}

impl Kind {
    pub fn as_obj3d(&self) -> Option<&ObjCfg> {
        match self {
            Kind::Obj3d(c) => Some(c),
            _ => None,
        }
    }
    pub fn as_scene3d(&self) -> Option<&S3dCfg> {
        match self {
            Kind::Scene3d(c) => Some(c),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vectors_parse_in_every_spelling() {
        assert_eq!(parse_vec3("(0, 2, 8)"), Some([0.0, 2.0, 8.0]));
        assert_eq!(parse_vec3("[1,-2.5, 3]"), Some([1.0, -2.5, 3.0]));
        assert_eq!(parse_vec3("1 2 3"), Some([1.0, 2.0, 3.0]));
        assert_eq!(parse_vec3("(1, 2)"), None);
        assert_eq!(parse_vec3("(a, b, c)"), None);
    }
}
