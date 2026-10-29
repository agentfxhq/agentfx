//! Stylised 3D (`Content::Scene3d`): a small scene graph (camera, lights, fog, objects with
//! materials) that the renderer rasterises on the CPU into one RGBA layer, which then enters the
//! 2D pipeline like any other content (effects, blend modes, masks, motion blur apply to it).
//!
//! World space: right-handed, y up, z toward the viewer (three.js convention), units free
//! (think metres). An object's matrix is `parent * T(pos) * Ry * Rx * Rz * S * T(-pivot)`
//! (rotations in degrees; yaw `ry` is applied last, so a spin about y stays upright when the
//! object is tilted). The camera looks from `pos` at `target`, `roll` turns it about the view
//! axis, `fov` is the vertical field of view in degrees; `ortho > 0` switches to an orthographic
//! camera that sees `ortho` world units vertically.
//!
//! Track properties (all numbers unless noted), on the node that holds the content:
//! `s3d.cam.{x,y,z,tx,ty,tz,fov,roll,ortho}`, `s3d.obj.<i>.{x,y,z,rx,ry,rz,s,sx,sy,sz,opacity,
//! emissive}`, `s3d.obj.<i>.color` (color), `s3d.light.<i>.{x,y,z,intensity}`, `s3d.ambient`,
//! `s3d.fog.{near,far}`.
use serde::{Deserialize, Serialize};

pub type V3 = [f64; 3];

fn yes() -> bool {
    true
}
fn one() -> f64 {
    1.0
}
fn ones3() -> V3 {
    [1.0, 1.0, 1.0]
}
fn white() -> [f64; 4] {
    [1.0, 1.0, 1.0, 1.0]
}
fn black() -> [f64; 4] {
    [0.0, 0.0, 0.0, 1.0]
}
fn grey() -> [f64; 4] {
    [0.8, 0.8, 0.8, 1.0]
}
fn fov_d() -> f64 {
    40.0
}
fn near_d() -> f64 {
    0.1
}
fn far_d() -> f64 {
    200.0
}
fn ambient_d() -> f64 {
    0.35
}
fn aa_d() -> u32 {
    2
}
fn seg_d() -> u32 {
    32
}
fn bands_d() -> u32 {
    3
}
fn shin_d() -> f64 {
    32.0
}
fn toon() -> String {
    "toon".into()
}
fn directional() -> String {
    "directional".into()
}
fn linear() -> String {
    "linear".into()
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn sh_opacity() -> f64 {
    0.45
}
fn sh_blur() -> f64 {
    6.0
}
fn crease_d() -> f64 {
    35.0
}

/// 3D scene content. `rect` is the viewport in the node's content space (scene px).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene3d {
    pub rect: [f64; 4],
    pub camera: Cam3d,
    /// ambient light intensity (hemispheric: a little brighter from above)
    #[serde(default = "ambient_d")]
    pub ambient: f64,
    #[serde(default = "white")]
    pub ambient_color: [f64; 4],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Light3d>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog: Option<Fog3d>,
    /// background colour; None = transparent (the layer shows what is below it)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<[f64; 4]>,
    /// supersampling per axis (1..=4): aa x aa samples per output pixel
    #[serde(default = "aa_d")]
    pub aa: u32,
    /// glow of emissive surfaces (blurred emissive light added on top)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bloom: Option<Bloom3d>,
    /// planar ground shadows: casters projected onto `receive` planes along the first light
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow3d>,
    pub objects: Vec<Obj3d>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cam3d {
    pub pos: V3,
    #[serde(default)]
    pub target: V3,
    /// vertical field of view, degrees
    #[serde(default = "fov_d")]
    pub fov: f64,
    /// degrees about the view axis
    #[serde(default, skip_serializing_if = "is_zero")]
    pub roll: f64,
    #[serde(default = "near_d")]
    pub near: f64,
    #[serde(default = "far_d")]
    pub far: f64,
    /// > 0: orthographic camera seeing this many world units vertically
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ortho: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Light3d {
    /// `directional` (from `pos` toward the origin, i.e. direction = -pos) or `point` (at `pos`)
    #[serde(default = "directional")]
    pub kind: String,
    pub pos: V3,
    #[serde(default = "white")]
    pub color: [f64; 4],
    #[serde(default = "one")]
    pub intensity: f64,
    /// point lights: distance at which the light falls to half (0 = no falloff)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub range: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fog3d {
    pub color: [f64; 4],
    /// `linear` (near..far) or `exp` (1 - exp(-density * distance))
    #[serde(default = "linear")]
    pub mode: String,
    #[serde(default)]
    pub near: f64,
    #[serde(default = "far_d")]
    pub far: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub density: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bloom3d {
    /// gaussian sigma in output px
    pub radius: f64,
    #[serde(default = "one")]
    pub strength: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow3d {
    #[serde(default = "sh_opacity")]
    pub opacity: f64,
    /// softness: gaussian sigma in output px
    #[serde(default = "sh_blur")]
    pub blur: f64,
    #[serde(default = "black")]
    pub color: [f64; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Obj3d {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// index of the parent object (must be lower than this object's index)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    pub shape: Shape3d,
    #[serde(default)]
    pub pos: V3,
    /// degrees
    #[serde(default)]
    pub rot: V3,
    #[serde(default = "ones3")]
    pub scale: V3,
    #[serde(default)]
    pub pivot: V3,
    #[serde(default)]
    pub material: Mat3d,
    /// casts a planar ground shadow
    #[serde(default = "yes")]
    pub cast: bool,
    /// receives ground shadows (planes only)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub receive: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum Shape3d {
    /// no geometry: a transform for its children
    Group,
    /// centered box; `radius` rounds its edges
    Box {
        size: V3,
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: f64,
    },
    /// centered rectangle in the xy plane facing +z (rotate rx=-90 for a floor)
    Plane { size: [f64; 2] },
    Sphere {
        radius: f64,
        #[serde(default = "seg_d")]
        segments: u32,
    },
    /// along y, centered; `radius_top` 0 = cone (default: same as `radius`)
    Cylinder {
        radius: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius_top: Option<f64>,
        height: f64,
        #[serde(default = "seg_d")]
        segments: u32,
    },
    /// in the xz plane: ring radius and tube radius
    Torus {
        radius: f64,
        tube: f64,
        #[serde(default = "seg_d")]
        segments: u32,
    },
    /// text (shaped with the scene's font) or an SVG path, filled (nonzero) and extruded along
    /// z by `depth`, centered on its ink box; `size` = the text's font size or the path's
    /// height in world units
    Extrude {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default)]
        font: usize,
        #[serde(default = "one")]
        size: f64,
        #[serde(default = "weight_d")]
        weight: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        letter_spacing: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        d: Option<String>,
        #[serde(default = "depth_d")]
        depth: f64,
    },
    /// Wavefront OBJ (v, vn, f; polygons fanned), relative to the IR file; normalized so its
    /// bounding box is centered and its largest extent is `size`
    Mesh {
        path: String,
        #[serde(default = "one")]
        size: f64,
    },
    /// a plane in xy facing +z whose texture is node `node`'s subtree (usually hidden), rendered
    /// each frame over `rect` (scene px) at `res` texels per scene px (0 = from its size on screen)
    Screen {
        size: [f64; 2],
        node: crate::NodeId,
        rect: [f64; 4],
        #[serde(default, skip_serializing_if = "is_zero")]
        res: f64,
    },
}
fn weight_d() -> f64 {
    800.0
}
fn depth_d() -> f64 {
    0.4
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mat3d {
    /// `flat` (unlit), `lambert`, `toon` (banded), `phong` (lambert + specular), `wire` (edges only)
    #[serde(default = "toon")]
    pub kind: String,
    #[serde(default = "grey")]
    pub color: [f64; 4],
    #[serde(default = "one")]
    pub opacity: f64,
    /// self-illumination 0..: adds `emissive x color`, drives the scene's bloom
    #[serde(default, skip_serializing_if = "is_zero")]
    pub emissive: f64,
    #[serde(default = "bands_d")]
    pub bands: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub specular: f64,
    #[serde(default = "shin_d")]
    pub shininess: f64,
    /// screen-space outline width in output px (silhouette and creases)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub outline: f64,
    #[serde(default = "black")]
    pub outline_color: [f64; 4],
    /// crease angle (degrees) above which edges inside the object are outlined (0 = silhouette only)
    #[serde(default = "crease_d")]
    pub crease: f64,
    /// feature edges drawn as lines of this width (px); `kind: wire` draws only them
    #[serde(default, skip_serializing_if = "is_zero")]
    pub wire: f64,
    #[serde(default = "white")]
    pub wire_color: [f64; 4],
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub double_sided: bool,
    /// image asset (PNG) mapped onto the faces (box: every face; plane/screen: the face)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<crate::AssetId>,
}
impl Default for Mat3d {
    fn default() -> Self {
        Mat3d { kind: toon(), color: grey(), opacity: 1.0, emissive: 0.0, bands: 3, specular: 0.0, shininess: 32.0, outline: 0.0, outline_color: black(), crease: crease_d(), wire: 0.0, wire_color: white(), double_sided: false, asset: None }
    }
}

/// Number fields of an object (`s3d.obj.<i>.<field>`).
pub const OBJ_FIELDS: &[&str] = &["x", "y", "z", "rx", "ry", "rz", "s", "sx", "sy", "sz", "opacity", "emissive"];
pub const CAM_FIELDS: &[&str] = &["x", "y", "z", "tx", "ty", "tz", "fov", "roll", "ortho"];
pub const LIGHT_FIELDS: &[&str] = &["x", "y", "z", "intensity"];

/// Is `p` a valid track property of this scene? `Some(true)` = color property.
pub fn property(s: &Scene3d, p: &str) -> Option<bool> {
    let rest = p.strip_prefix("s3d.")?;
    let parts: Vec<&str> = rest.split('.').collect();
    match parts.as_slice() {
        ["ambient"] => Some(false),
        ["fog", "near" | "far"] if s.fog.is_some() => Some(false),
        ["cam", f] if CAM_FIELDS.contains(f) => Some(false),
        ["obj", i, f] => {
            let i: usize = i.parse().ok()?;
            if i >= s.objects.len() {
                return None;
            }
            if *f == "color" {
                Some(true)
            } else if OBJ_FIELDS.contains(f) {
                Some(false)
            } else {
                None
            }
        }
        ["light", i, f] => {
            let i: usize = i.parse().ok()?;
            (i < s.lights.len() && LIGHT_FIELDS.contains(f)).then_some(false)
        }
        _ => None,
    }
}

/// Animated values of one object at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjState {
    pub pos: V3,
    pub rot: V3,
    pub scale: V3,
    pub color: [f64; 4],
    pub opacity: f64,
    pub emissive: f64,
}

/// The scene's animated values at a time (everything a frame depends on besides screens).
#[derive(Clone, Debug, PartialEq)]
pub struct State3d {
    pub cam: Cam3d,
    pub ambient: f64,
    pub lights: Vec<Light3d>,
    pub fog: Option<Fog3d>,
    pub objs: Vec<ObjState>,
}

impl State3d {
    /// Feed every value into a hasher (planner signature).
    pub fn hash_into(&self, h: &mut impl std::hash::Hasher) {
        let mut f = |v: f64| h.write_u64(v.to_bits());
        let c = &self.cam;
        for v in c.pos.iter().chain(&c.target) {
            f(*v);
        }
        for v in [c.fov, c.roll, c.near, c.far, c.ortho, self.ambient] {
            f(v);
        }
        for l in &self.lights {
            for v in l.pos.iter().chain(&l.color) {
                f(*v);
            }
            f(l.intensity);
        }
        if let Some(fg) = &self.fog {
            f(fg.near);
            f(fg.far);
        }
        for o in &self.objs {
            for v in o.pos.iter().chain(&o.rot).chain(&o.scale).chain(&o.color) {
                f(*v);
            }
            f(o.opacity);
            f(o.emissive);
        }
    }
}

impl Scene3d {
    /// Evaluate the animated values: `num(property, base)` returns the track value or `base`,
    /// `col(property, base)` likewise for colors.
    pub fn state(&self, num: &mut dyn FnMut(&str, f64) -> f64, col: &mut dyn FnMut(&str, [f64; 4]) -> [f64; 4]) -> State3d {
        let c = &self.camera;
        let mut cam = c.clone();
        cam.pos = [num("s3d.cam.x", c.pos[0]), num("s3d.cam.y", c.pos[1]), num("s3d.cam.z", c.pos[2])];
        cam.target = [num("s3d.cam.tx", c.target[0]), num("s3d.cam.ty", c.target[1]), num("s3d.cam.tz", c.target[2])];
        cam.fov = num("s3d.cam.fov", c.fov);
        cam.roll = num("s3d.cam.roll", c.roll);
        cam.ortho = num("s3d.cam.ortho", c.ortho);
        let lights = self
            .lights
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let mut l = l.clone();
                l.pos = [num(&format!("s3d.light.{i}.x"), l.pos[0]), num(&format!("s3d.light.{i}.y"), l.pos[1]), num(&format!("s3d.light.{i}.z"), l.pos[2])];
                l.intensity = num(&format!("s3d.light.{i}.intensity"), l.intensity);
                l
            })
            .collect();
        let fog = self.fog.as_ref().map(|f| {
            let mut f = f.clone();
            f.near = num("s3d.fog.near", f.near);
            f.far = num("s3d.fog.far", f.far);
            f
        });
        let objs = self
            .objects
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let p = |f: &str| format!("s3d.obj.{i}.{f}");
                let s = num(&p("s"), 1.0);
                ObjState {
                    pos: [num(&p("x"), o.pos[0]), num(&p("y"), o.pos[1]), num(&p("z"), o.pos[2])],
                    rot: [num(&p("rx"), o.rot[0]), num(&p("ry"), o.rot[1]), num(&p("rz"), o.rot[2])],
                    scale: [s * num(&p("sx"), o.scale[0]), s * num(&p("sy"), o.scale[1]), s * num(&p("sz"), o.scale[2])],
                    color: col(&p("color"), o.material.color),
                    opacity: num(&p("opacity"), o.material.opacity),
                    emissive: num(&p("emissive"), o.material.emissive),
                }
            })
            .collect();
        State3d { cam, ambient: num("s3d.ambient", self.ambient), lights, fog, objs }
    }

    /// The base (untracked) state.
    pub fn base_state(&self) -> State3d {
        self.state(&mut |_, b| b, &mut |_, b| b)
    }

    /// Screen objects: (object index, source node).
    pub fn screens(&self) -> impl Iterator<Item = (usize, crate::NodeId)> + '_ {
        self.objects.iter().enumerate().filter_map(|(i, o)| match &o.shape {
            Shape3d::Screen { node, .. } => Some((i, *node)),
            _ => None,
        })
    }

    /// Viewport aspect (w / h).
    pub fn aspect(&self) -> f64 {
        if self.rect[3] > 0.0 { self.rect[2] / self.rect[3] } else { 1.0 }
    }
}

impl State3d {
    /// World matrix of every object (parents are evaluated first).
    pub fn world(&self, s: &Scene3d) -> Vec<M4> {
        let mut out: Vec<M4> = Vec::with_capacity(self.objs.len());
        for (i, (o, st)) in s.objects.iter().zip(&self.objs).enumerate() {
            let local = M4::translate(st.pos)
                .mul(&M4::rot_y(st.rot[1]))
                .mul(&M4::rot_x(st.rot[0]))
                .mul(&M4::rot_z(st.rot[2]))
                .mul(&M4::scale(st.scale))
                .mul(&M4::translate([-o.pivot[0], -o.pivot[1], -o.pivot[2]]));
            let m = match o.parent {
                Some(p) if p < i => out[p].mul(&local),
                _ => local,
            };
            out.push(m);
        }
        out
    }
    /// Effective opacity of every object (times its ancestors').
    pub fn opacities(&self, s: &Scene3d) -> Vec<f64> {
        let mut out: Vec<f64> = Vec::with_capacity(self.objs.len());
        for (i, (o, st)) in s.objects.iter().zip(&self.objs).enumerate() {
            let a = st.opacity.clamp(0.0, 1.0);
            out.push(match o.parent {
                Some(p) if p < i => out[p] * a,
                _ => a,
            });
        }
        out
    }
}

// ---------------------------------------------------------------- math

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn norm(a: V3) -> V3 {
    let l = len(a);
    if l > 1e-300 { mul(a, 1.0 / l) } else { [0.0, 0.0, 0.0] }
}

/// 4x4 matrix, row-major: `m[r][c]`, column vectors (`p' = M p`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M4(pub [[f64; 4]; 4]);

impl M4 {
    pub const IDENTITY: M4 = M4([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);
    pub fn translate(t: V3) -> M4 {
        let mut m = M4::IDENTITY;
        m.0[0][3] = t[0];
        m.0[1][3] = t[1];
        m.0[2][3] = t[2];
        m
    }
    pub fn scale(s: V3) -> M4 {
        let mut m = M4::IDENTITY;
        m.0[0][0] = s[0];
        m.0[1][1] = s[1];
        m.0[2][2] = s[2];
        m
    }
    pub fn rot_x(deg: f64) -> M4 {
        let (s, c) = deg.to_radians().sin_cos();
        M4([[1.0, 0.0, 0.0, 0.0], [0.0, c, -s, 0.0], [0.0, s, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
    pub fn rot_y(deg: f64) -> M4 {
        let (s, c) = deg.to_radians().sin_cos();
        M4([[c, 0.0, s, 0.0], [0.0, 1.0, 0.0, 0.0], [-s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
    pub fn rot_z(deg: f64) -> M4 {
        let (s, c) = deg.to_radians().sin_cos();
        M4([[c, -s, 0.0, 0.0], [s, c, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }
    pub fn mul(&self, b: &M4) -> M4 {
        let mut r = [[0.0; 4]; 4];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = self.0[i][0] * b.0[0][j] + self.0[i][1] * b.0[1][j] + self.0[i][2] * b.0[2][j] + self.0[i][3] * b.0[3][j];
            }
        }
        M4(r)
    }
    pub fn point(&self, p: V3) -> V3 {
        let m = &self.0;
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ]
    }
    pub fn vector(&self, v: V3) -> V3 {
        let m = &self.0;
        [m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2], m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2], m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2]]
    }
    /// Inverse transpose of the upper 3x3 (normal matrix); zero matrix when singular.
    pub fn normal_matrix(&self) -> [[f64; 3]; 3] {
        let m = &self.0;
        let a = [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]];
        let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
        if det.abs() < 1e-300 {
            return [[0.0; 3]; 3];
        }
        let id = 1.0 / det;
        // cofactor matrix / det = inverse transpose
        [
            [(a[1][1] * a[2][2] - a[1][2] * a[2][1]) * id, -(a[1][0] * a[2][2] - a[1][2] * a[2][0]) * id, (a[1][0] * a[2][1] - a[1][1] * a[2][0]) * id],
            [-(a[0][1] * a[2][2] - a[0][2] * a[2][1]) * id, (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * id, -(a[0][0] * a[2][1] - a[0][1] * a[2][0]) * id],
            [(a[0][1] * a[1][2] - a[0][2] * a[1][1]) * id, -(a[0][0] * a[1][2] - a[0][2] * a[1][0]) * id, (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * id],
        ]
    }
    /// Determinant of the upper 3x3 (negative = mirrored: winding flips).
    pub fn det3(&self) -> f64 {
        let a = &self.0;
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    }
}

/// Apply a 3x3 matrix to a vector.
pub fn m3(m: &[[f64; 3]; 3], v: V3) -> V3 {
    [m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2], m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2], m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2]]
}

/// Camera frame: view matrix (world -> view, camera looks down -z) and projection.
#[derive(Clone, Copy, Debug)]
pub struct View3 {
    pub view: M4,
    /// camera position in world space
    pub eye: V3,
    /// vertical half extent at distance 1 (perspective) or in world units (ortho)
    pub half_h: f64,
    pub aspect: f64,
    pub ortho: bool,
    pub near: f64,
    pub far: f64,
}

impl View3 {
    pub fn new(c: &Cam3d, aspect: f64) -> View3 {
        let eye = c.pos;
        let mut f = norm(sub(c.target, eye));
        if len(f) == 0.0 {
            f = [0.0, 0.0, -1.0];
        }
        // up = world y, unless looking straight up/down
        let up0 = if dot(f, [0.0, 1.0, 0.0]).abs() > 0.9999 { [0.0, 0.0, -1.0] } else { [0.0, 1.0, 0.0] };
        let mut r = norm(cross(f, up0));
        let mut u = cross(r, f);
        if c.roll != 0.0 {
            let (s, co) = c.roll.to_radians().sin_cos();
            let (r2, u2) = (add(mul(r, co), mul(u, s)), add(mul(u, co), mul(r, -s)));
            r = r2;
            u = u2;
        }
        let view = M4([
            [r[0], r[1], r[2], -dot(r, eye)],
            [u[0], u[1], u[2], -dot(u, eye)],
            [-f[0], -f[1], -f[2], dot(f, eye)],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        let ortho = c.ortho > 0.0;
        let half_h = if ortho { c.ortho / 2.0 } else { (c.fov.clamp(1.0, 170.0).to_radians() / 2.0).tan() };
        View3 { view, eye, half_h, aspect: aspect.max(1e-6), ortho, near: c.near.max(1e-4), far: c.far.max(c.near + 1e-3) }
    }
    /// View-space point -> normalized device coordinates (x right, y up, -1..1) and the view
    /// depth (distance along the view axis). None behind the near plane.
    pub fn ndc(&self, pv: V3) -> Option<(f64, f64, f64)> {
        let d = -pv[2];
        if d < self.near {
            return None;
        }
        if self.ortho {
            Some((pv[0] / (self.half_h * self.aspect), pv[1] / self.half_h, d))
        } else {
            Some((pv[0] / (d * self.half_h * self.aspect), pv[1] / (d * self.half_h), d))
        }
    }
    /// World point -> NDC + depth.
    pub fn project(&self, p: V3) -> Option<(f64, f64, f64)> {
        self.ndc(self.view.point(p))
    }
}

/// NDC -> content px of the viewport rect.
pub fn ndc_to_rect(r: [f64; 4], x: f64, y: f64) -> (f64, f64) {
    (r[0] + (x + 1.0) * 0.5 * r[2], r[1] + (1.0 - y) * 0.5 * r[3])
}

/// Local bounding box (min, max) of a shape, before the object transform (None: no geometry
/// known without loading, e.g. meshes and text, or groups).
pub fn shape_bounds(s: &Shape3d) -> Option<(V3, V3)> {
    match s {
        Shape3d::Group => None,
        Shape3d::Box { size, .. } => Some((mul(*size, -0.5), mul(*size, 0.5))),
        Shape3d::Plane { size } | Shape3d::Screen { size, .. } => Some(([-size[0] / 2.0, -size[1] / 2.0, 0.0], [size[0] / 2.0, size[1] / 2.0, 0.0])),
        Shape3d::Sphere { radius, .. } => Some(([-radius; 3], [*radius; 3])),
        Shape3d::Cylinder { radius, radius_top, height, .. } => {
            let r = radius.max(radius_top.unwrap_or(*radius));
            Some(([-r, -height / 2.0, -r], [r, height / 2.0, r]))
        }
        Shape3d::Torus { radius, tube, .. } => {
            let r = radius + tube;
            Some(([-r, -tube, -r], [r, *tube, r]))
        }
        Shape3d::Extrude { .. } | Shape3d::Mesh { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }
    #[test]
    fn camera_projects_target_to_center_and_near_plane_rejects() {
        let c = Cam3d { pos: [0.0, 2.0, 8.0], target: [0.0, 0.0, 0.0], fov: 40.0, roll: 0.0, near: 0.1, far: 100.0, ortho: 0.0 };
        let v = View3::new(&c, 16.0 / 9.0);
        let (x, y, d) = v.project([0.0, 0.0, 0.0]).unwrap();
        assert!(close(x, 0.0) && close(y, 0.0));
        assert!(close(d, (4.0f64 + 64.0).sqrt()));
        // a point above the target projects above the center (y up)
        assert!(v.project([0.0, 1.0, 0.0]).unwrap().1 > 0.0);
        // to the right
        assert!(v.project([1.0, 0.0, 0.0]).unwrap().0 > 0.0);
        // behind the camera
        assert!(v.project([0.0, 2.0, 9.0]).is_none());
        // fov: a point on the top edge of the frustum maps to y = 1
        let h = (20.0f64).to_radians().tan();
        let pv = [0.0, h * 5.0, -5.0];
        assert!(close(v.ndc(pv).unwrap().1, 1.0));
    }
    #[test]
    fn roll_turns_the_image() {
        let mut c = Cam3d { pos: [0.0, 0.0, 5.0], target: [0.0; 3], fov: 40.0, roll: 90.0, near: 0.1, far: 100.0, ortho: 0.0 };
        let v = View3::new(&c, 1.0);
        let (x, y, _) = v.project([1.0, 0.0, 0.0]).unwrap();
        assert!(x.abs() < 1e-9 && y < 0.0, "{x} {y}");
        c.roll = 0.0;
        c.ortho = 4.0;
        let v = View3::new(&c, 1.0);
        let (x, y, _) = v.project([2.0, 2.0, -3.0]).unwrap();
        assert!(close(x, 1.0) && close(y, 1.0));
    }
    #[test]
    fn object_matrices_compose_with_parents() {
        let s: Scene3d = serde_json::from_value(serde_json::json!({
            "rect": [0, 0, 100, 100], "camera": {"pos": [0, 0, 5]},
            "objects": [
                {"shape": {"type": "group"}, "pos": [1, 0, 0], "rot": [0, 90, 0]},
                {"parent": 0, "shape": {"type": "box", "size": [1, 1, 1]}, "pos": [1, 0, 0], "scale": [2, 2, 2]}
            ]})).unwrap();
        let st = s.base_state();
        let w = st.world(&s);
        // child at local x=1 inside a group turned 90 deg about y: world (1, 0, -1)
        let p = w[1].point([0.0, 0.0, 0.0]);
        assert!(close(p[0], 1.0) && close(p[1], 0.0) && close(p[2], -1.0), "{p:?}");
        // scale 2: the box corner (0.5, 0.5, 0.5) is 1 away from the center along each axis
        let q = w[1].point([0.5, 0.5, 0.5]);
        assert!(close(q[1], 1.0));
        assert!(property(&s, "s3d.obj.1.ry") == Some(false));
        assert!(property(&s, "s3d.obj.1.color") == Some(true));
        assert!(property(&s, "s3d.obj.2.x").is_none());
        assert!(property(&s, "s3d.cam.fov").is_some());
        assert!(property(&s, "s3d.fog.near").is_none());
    }
    #[test]
    fn normal_matrix_keeps_normals_perpendicular_under_nonuniform_scale() {
        let m = M4::scale([2.0, 1.0, 1.0]).mul(&M4::rot_z(30.0));
        let nm = m.normal_matrix();
        // a surface tangent t and normal n stay perpendicular after transforming
        let t = [1.0, 1.0, 0.0];
        let n = [1.0, -1.0, 0.0];
        assert!(dot(m.vector(t), m3(&nm, n)).abs() < 1e-9);
    }
}
