//! Motion IR: the backend-neutral output of the compiler. A tree of layers with
//! static style + layout constraints, and property tracks (keyframes) on typed channels.
//! Backends: `html.rs` (CSS @keyframes, preview + Chrome reference) and, later, `reel-ir`
//! (native renderer). Nothing in here is a CSS concept: channels are renderer properties,
//! eases stay symbolic (springs stay springs), paints stay paints.
use crate::values::{Ease, Paint, Rgba};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Composite,
    Paint,
    Layout,
    PerFrame,
}

/// Animatable channels. Units: px for lengths, degrees for angles, 0..1 for fractions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ch {
    X, Y, Z,             // translation px (Z = depth toward camera)
    Px, Py,              // translation as fraction of the PARENT box (cameras, cursors in shots)
    Scale, Sx, Sy,
    Rot, RotX, RotY,     // degrees
    Opacity,
    Blur, Glow,          // px radius
    Brightness, Saturate, Contrast, Hue,
    Fill,                // color (box/shape fill)
    Color,               // text color
    Stroke,              // color
    TrimStart, TrimEnd,  // 0..1 of path length
    Reveal,              // 0..1 progress of the layer's clip shape (wipe/iris)
    MaskX,               // mask position offset 0..1
    GradAngle,           // gradient angle deg
    Grad0, Grad1, Grad2, Grad3, // gradient stop colors
    Wght,                // variable font weight axis
    Wdth, Opsz, Slnt,    // variable font width / optical size / slant axes
    Axis0, Axis1, Axis2, Axis3, // custom variable axes (uppercase tags, `axis_tag`), per compile
    Tracking,            // letter spacing em
    W, H,                // box size px (layout)
    LineH,               // collapsing line height factor 0..1 (code diff)
    Count,               // counter value
    Magic,               // magic-move progress 0..1 (geometry delta measured at layout time)
    TimeSpan,            // time-effect span, seconds (slitscan/timedisplace span, echo dt)
    Halftone,            // halftone cell px (look)
    Sweep,               // light sweep position 0..1 (look)
    Rays,                // god-ray strength (look)
    Misreg,              // riso misregistration px (look)
    Rgbsplit, RgbsplitDx, RgbsplitDy, Glitch, GlitchRate, GlitchBlock, Vhs, Dither, Ascii,
    FluidTime,           // simulation time a fluid shows, s (identity unless remapped)
    PathOffset,          // text on a path: start offset along the path, px
}

impl Ch {
    pub fn stage(self) -> Stage {
        use Ch::*;
        match self {
            X | Y | Z | Px | Py | Scale | Sx | Sy | Rot | RotX | RotY | Opacity | Magic => Stage::Composite,
            Blur | Glow | Brightness | Saturate | Contrast | Hue | Fill | Color | Stroke | TrimStart | TrimEnd | Reveal | MaskX
            | GradAngle | Grad0 | Grad1 | Grad2 | Grad3 | TimeSpan | Halftone | Sweep | Rays | Misreg | FluidTime | PathOffset | Rgbsplit | RgbsplitDx | RgbsplitDy | Glitch | GlitchRate | GlitchBlock | Vhs | Dither | Ascii => Stage::Paint,
            Wght | Wdth | Opsz | Slnt | Axis0 | Axis1 | Axis2 | Axis3 | Tracking | W | H | LineH | Count => Stage::Layout,
        }
    }
    pub fn default_value(self) -> TV {
        use Ch::*;
        match self {
            Scale | Sx | Sy | Opacity | Brightness | Saturate | Contrast | TrimEnd | Reveal | LineH | Magic => TV::N(1.0),
            Fill | Color | Stroke | Grad0 | Grad1 | Grad2 | Grad3 => TV::C(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }),
            Wght => TV::N(400.0),
            Wdth => TV::N(100.0),
            _ => TV::N(0.0),
        }
    }
    pub fn is_color(self) -> bool {
        matches!(self, Ch::Fill | Ch::Color | Ch::Stroke | Ch::Grad0 | Ch::Grad1 | Ch::Grad2 | Ch::Grad3)
    }
    pub fn name(self) -> String {
        serde_json::to_value(self).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }
    pub fn parse(s: &str) -> Option<Ch> {
        // custom variable-font axes: registered (uppercase) tags like GRAD, XTRA, YOPQ
        if s.len() == 4 && s.as_bytes()[0].is_ascii_uppercase() && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) {
            return axis_ch(s);
        }
        let s = s.to_ascii_lowercase();
        Some(match s.as_str() {
            "x" => Ch::X, "y" => Ch::Y, "z" => Ch::Z, "px" => Ch::Px, "py" => Ch::Py,
            "scale" | "s" => Ch::Scale, "sx" | "scale-x" => Ch::Sx, "sy" | "scale-y" => Ch::Sy,
            "rot" | "rotate" | "r" => Ch::Rot, "rotx" | "rot-x" => Ch::RotX, "roty" | "rot-y" => Ch::RotY,
            "opacity" | "o" | "alpha" => Ch::Opacity, "blur" => Ch::Blur, "glow" => Ch::Glow,
            "brightness" => Ch::Brightness, "saturate" | "saturation" => Ch::Saturate, "contrast" => Ch::Contrast,
            "hue" => Ch::Hue, "fill" => Ch::Fill, "color" => Ch::Color, "stroke" => Ch::Stroke,
            "trim-start" => Ch::TrimStart, "trim" | "trim-end" => Ch::TrimEnd, "reveal" => Ch::Reveal,
            "mask-x" => Ch::MaskX, "grad-angle" | "angle" => Ch::GradAngle, "grad0" => Ch::Grad0, "grad1" => Ch::Grad1,
            "grad2" => Ch::Grad2, "grad3" => Ch::Grad3, "wght" | "weight" | "font-weight" => Ch::Wght, "tracking" => Ch::Tracking,
            "wdth" => Ch::Wdth, "opsz" => Ch::Opsz, "slnt" => Ch::Slnt, "path-offset" | "offset" => Ch::PathOffset,
            "w" | "width" => Ch::W, "h" | "height" => Ch::H, "count" => Ch::Count,
            "halftone" => Ch::Halftone, "sweep" => Ch::Sweep, "rays" => Ch::Rays, "misreg" => Ch::Misreg,
            "rgbsplit" => Ch::Rgbsplit, "rgbsplit-dx" => Ch::RgbsplitDx, "rgbsplit-dy" => Ch::RgbsplitDy,
            "glitch" => Ch::Glitch, "glitch-rate" => Ch::GlitchRate, "glitch-block" => Ch::GlitchBlock,
            "vhs" => Ch::Vhs, "dither" => Ch::Dither, "ascii" => Ch::Ascii,
            _ => return None,
        })
    }
    /// The axis tag of an axis channel (`wght`, `wdth`, `opsz`, `slnt`, custom tags).
    pub fn axis_tag(self) -> Option<String> {
        match self {
            Ch::Wght => Some("wght".into()),
            Ch::Wdth => Some("wdth".into()),
            Ch::Opsz => Some("opsz".into()),
            Ch::Slnt => Some("slnt".into()),
            Ch::Axis0 | Ch::Axis1 | Ch::Axis2 | Ch::Axis3 => {
                let k = [Ch::Axis0, Ch::Axis1, Ch::Axis2, Ch::Axis3].iter().position(|c| *c == self).unwrap();
                AXIS_TAGS.with(|t| t.borrow().get(k).cloned())
            }
            _ => None,
        }
    }
    pub fn is_axis(self) -> bool {
        matches!(self, Ch::Wght | Ch::Wdth | Ch::Opsz | Ch::Slnt | Ch::Axis0 | Ch::Axis1 | Ch::Axis2 | Ch::Axis3)
    }
    pub const NAMES: [&'static str; 52] = [
        "x", "y", "z", "px", "py", "scale", "sx", "sy", "rot", "rotx", "roty", "opacity", "blur", "glow", "brightness",
        "saturate", "contrast", "hue", "fill", "color", "stroke", "trim-start", "trim", "reveal", "mask-x", "grad-angle", "grad0",
        "grad1", "grad2", "grad3", "wght", "tracking", "w", "h", "count", "halftone", "sweep", "rays", "misreg",
        "rgbsplit", "rgbsplit-dx", "rgbsplit-dy", "glitch", "glitch-rate", "glitch-block", "vhs", "dither", "ascii",
        "wdth", "opsz", "slnt", "path-offset",
    ];
}

thread_local! {
    /// Custom axis tags of the current compile, in first-use order (slot k = `Ch::Axis<k>`).
    static AXIS_TAGS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Channel of a custom (uppercase) axis tag; None when four custom axes are already in use.
pub fn axis_ch(tag: &str) -> Option<Ch> {
    AXIS_TAGS.with(|t| {
        let mut t = t.borrow_mut();
        let k = match t.iter().position(|x| x == tag) {
            Some(k) => k,
            None if t.len() < 4 => {
                t.push(tag.to_string());
                t.len() - 1
            }
            None => return None,
        };
        Some([Ch::Axis0, Ch::Axis1, Ch::Axis2, Ch::Axis3][k])
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TV {
    N(f64),
    C(Rgba),
}
impl TV {
    pub fn n(&self) -> f64 {
        match self {
            TV::N(v) => *v,
            TV::C(c) => c.a,
        }
    }
    pub fn lerp(&self, b: &TV, t: f64) -> TV {
        match (self, b) {
            (TV::N(a), TV::N(b)) => TV::N(a + (b - a) * t),
            (TV::C(a), TV::C(b)) => TV::C(Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }),
            _ => b.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Key {
    pub t: f64,
    pub v: TV,
    /// Ease of the segment that STARTS at this key.
    pub ease: Ease,
}

#[derive(Debug, Clone, Serialize)]
pub struct Track {
    pub layer: String,
    pub ch: Ch,
    pub base: TV,
    pub keys: Vec<Key>,
}

impl Track {
    pub fn new(layer: &str, ch: Ch, base: TV) -> Self {
        Track { layer: layer.into(), ch, base, keys: vec![] }
    }
    pub fn eval(&self, t: f64) -> TV {
        let k = &self.keys;
        if k.is_empty() {
            return self.base.clone();
        }
        if t <= k[0].t {
            return k[0].v.clone();
        }
        for i in 0..k.len() - 1 {
            if t < k[i + 1].t {
                let span = (k[i + 1].t - k[i].t).max(1e-9);
                let p = k[i].ease.sample(((t - k[i].t) / span).clamp(0.0, 1.0));
                return k[i].v.lerp(&k[i + 1].v, p);
            }
        }
        k[k.len() - 1].v.clone()
    }
    /// Drop keys after `t` and pin the current value at `t` (the "later event wins, starts
    /// from where things are" rule). Returns the pinned value.
    fn cut(&mut self, t: f64) -> TV {
        let v = self.eval(t);
        // If t interrupts a running non-linear segment, the kept part must keep its original curve:
        // re-sample it into linear keys (12 steps) so frames before t do not change.
        let interrupted: Option<(Key, TV, f64)> = self.keys.windows(2).find(|w| w[0].t < t - 1e-6 && w[1].t > t + 1e-6)
            .map(|w| (w[0].clone(), w[1].v.clone(), w[1].t));
        self.keys.retain(|k| k.t < t - 1e-6);
        if self.keys.is_empty() && t > 1e-6 {
            self.keys.push(Key { t: 0.0, v: self.base.clone(), ease: Ease::Hold });
        }
        if let Some((k0, v1, t1)) = interrupted {
            if !matches!(k0.ease, Ease::Linear | Ease::Hold) {
                let n = 12;
                if let Some(last) = self.keys.last_mut() {
                    last.ease = Ease::Linear;
                }
                for i in 1..n {
                    let ti = k0.t + (t - k0.t) * i as f64 / n as f64;
                    let p = k0.ease.sample(((ti - k0.t) / (t1 - k0.t).max(1e-9)).clamp(0.0, 1.0));
                    self.keys.push(Key { t: ti, v: k0.v.lerp(&v1, p), ease: Ease::Linear });
                }
            }
        }
        self.keys.push(Key { t, v: v.clone(), ease: Ease::Linear });
        v
    }
    /// Animate from the current value at t0 to `v` at t1.
    pub fn to(&mut self, t0: f64, t1: f64, v: TV, ease: Ease) {
        self.cut(t0);
        if let Some(k) = self.keys.last_mut() {
            k.ease = ease;
        }
        self.keys.push(Key { t: t1.max(t0 + 1e-3), v, ease: Ease::Linear });
    }
    /// Replace everything from t0 on with linear keys (baked per-frame curves, camera director).
    pub fn bake(&mut self, t0: f64, keys: &[(f64, TV)]) {
        self.cut(t0);
        self.keys.retain(|k| k.t < t0 - 1e-6);
        if self.keys.is_empty() && t0 > 1e-6 {
            self.keys.push(Key { t: 0.0, v: self.base.clone(), ease: Ease::Hold });
        }
        if let Some(k) = self.keys.last_mut() {
            k.ease = Ease::Linear;
        }
        for (t, v) in keys {
            self.keys.push(Key { t: *t, v: v.clone(), ease: Ease::Linear });
        }
    }
    /// Jump to `v` at t (holds the old value until t).
    pub fn jump(&mut self, t: f64, v: TV) {
        self.cut(t);
        if let Some(k) = self.keys.last_mut() {
            k.ease = Ease::Hold;
        }
        self.keys.push(Key { t: t + 1e-3, v, ease: Ease::Linear });
    }
    /// Explicit from->to (sets the start value first).
    pub fn from_to(&mut self, t0: f64, t1: f64, a: TV, b: TV, ease: Ease) {
        self.jump(t0 - 1e-3, a);
        self.to(t0, t1, b, ease);
    }
    pub fn is_static(&self) -> bool {
        self.keys.windows(2).all(|w| w[0].v == w[1].v) && self.keys.first().map(|k| k.v == self.base).unwrap_or(true)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Kind {
    Group,
    Rect,
    Ellipse,
    /// SVG path data in the layer's own box coordinates (viewBox = w x h).
    Path { d: String, closed: bool, vb: (f64, f64) },
    /// Text; `runs` carry inline styling. Kinetic type splits text into child layers instead.
    Text { runs: Vec<Run> },
    Image { src: String, fit: String },
    Video { src: String },
    /// Raw escape hatches (rendered by the HTML path / Chrome fallback in the native renderer).
    Html { markup: String },
    Svg { markup: String },
    /// Integer counter shown as text: prefix + value + suffix.
    Counter { prefix: String, suffix: String },
    /// Instanced point cloud (reel-ir `cloud` content): drawn by the native renderer only.
    Cloud(Box<CloudCfg>),
    /// 2D fluid (reel-ir `fluid` content): simulated by the native renderer only.
    Fluid(Box<crate::fluid::FluidCfg>),
}

/// A point cloud as written: shape props stay raw (lengths resolve against the laid-out box
/// in the reel-ir backend); times are absolute (video seconds).
#[derive(Debug, Clone, Serialize)]
pub struct CloudCfg {
    pub n: u32,
    pub seed: u64,
    pub r: f64,
    pub rj: f64,
    pub palette: Vec<Rgba>,
    pub weights: Vec<f64>,
    /// own perspective (px) about the box center, when no scene camera projects it
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persp: Option<f64>,
    pub forms: Vec<CloudFormCfg>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloudFormCfg {
    pub at: f64,
    pub dur: f64,
    pub stagger: f64,
    pub by: String,
    pub ease: Ease,
    pub assign: String,
    pub shape: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// shape props as written (`x`, `y`, `w`, `h`, `r`, `arms`, `flow`, `font`, ...)
    pub props: std::collections::BTreeMap<String, String>,
    /// (amp px, scale px, speed /s)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift: Option<(f64, f64, f64)>,
    pub size: f64,
    pub opacity: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<CloudColorCfg>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloudColorCfg {
    pub by: String,
    pub split: Vec<f64>,
    pub angle: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wave: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Run {
    pub text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub accent: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub code: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgba>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Pos {
    /// participates in the parent's flow (stack/grid)
    Flow,
    /// absolute: x/y of the anchor point, as px or fraction ("%" → stored as fraction with `rel`)
    Abs,
    /// fills the parent box
    Fill,
}

#[derive(Debug, Clone, Serialize)]
pub struct Len {
    pub v: f64,
    /// "px" | "%" (of parent) | "em"
    pub unit: &'static str,
}
impl Len {
    pub fn px(v: f64) -> Self {
        Len { v, unit: "px" }
    }
    pub fn pct(v: f64) -> Self {
        Len { v, unit: "%" }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Layout {
    pub pos: Pos,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<Len>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<Len>,
    /// anchor point of the box for Abs placement (0..1, 0..1)
    pub anchor: (f64, f64),
    #[serde(skip_serializing_if = "Option::is_none")]
    pub w: Option<Len>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<Len>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxw: Option<Len>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect: Option<f64>,
    /// child flow: "row" | "col" | "grid" | "inline" | "none"
    pub flow: &'static str,
    pub gap: f64,
    pub cols: u32,
    /// cross-axis alignment: start|center|end|stretch ; main-axis: start|center|end|between
    pub align: String,
    pub justify: String,
    pub pad: [f64; 4],
    pub grow: f64,
    pub wrap: bool,
}
impl Default for Layout {
    fn default() -> Self {
        Layout { pos: Pos::Flow, x: None, y: None, anchor: (0.0, 0.0), w: None, h: None, maxw: None, aspect: None, flow: "none", gap: 0.0, cols: 0,
            align: "center".into(), justify: "center".into(), pad: [0.0; 4], grow: 0.0, wrap: false }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Shadow {
    pub x: f64,
    pub y: f64,
    pub blur: f64,
    pub color: Option<Rgba>,
    #[serde(skip_serializing_if = "is_zero")]
    pub spread: f64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inner: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum ClipShape {
    Polygon(Vec<[f64; 2]>),
    /// reveal from an edge: "left" means the visible part grows from the left edge
    Wipe(String),
    /// circle grows from (cx, cy) fractions
    Iris(f64, f64),
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Style {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<Paint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Rgba>,
    /// gradient stroke (`stroke=linear(...)` on paths); `stroke` then holds its first color
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke_paint: Option<Paint>,
    #[serde(skip_serializing_if = "is_zero")]
    pub stroke_w: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dash: Option<(f64, f64)>,
    #[serde(skip_serializing_if = "is_zero")]
    pub radius: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border: Option<(f64, Rgba)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shadows: Vec<Shadow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glow_color: Option<Rgba>,
    // text
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgba>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<Rgba>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_align: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracking: Option<f64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub uppercase: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub nowrap: bool,
    /// static variable-font axes (`axes="wdth 80, GRAD 50"`)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<(String, f64)>,
    /// OpenType features as a CSS `font-feature-settings` value (`features="ss01 off, tnum"`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<String>,
    /// text animators (`animate #t ...`), reel-ir `TextFx` (direct mode)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub text_anims: Vec<crate::textv2::TextAnim>,
    /// text on a path (`on=#path offset=`): target path layer id, start offset px
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_path: Option<(String, f64)>,
    /// fill text with the paint (text matte)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub text_fill: bool,
    // compositing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<Paint>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub mask_luma: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipShape>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub overflow_clip: bool,
    /// transform origin (fractions)
    pub origin: (f64, f64),
    /// perspective distance for children (2.5D), px
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perspective: Option<f64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub preserve3d: bool,
    /// static filters (applied before animated channels)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grain: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vignette: Option<f64>,
    /// motion blur of the layer's subtree: (samples 2..64, shutter in frames 0..4]; reel-ir
    /// `motion_blur` (direct mode); the HTML backend ignores it
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_blur: Option<(u32, f64)>,
    /// time effect (`slitscan`, `echo`, `timedisplace`): reel-ir `time`, direct mode only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_fx: Option<crate::timefx::TimeFxSpec>,
    /// the layer is a `timedisplace map=#id` source: hidden (reel-ir `hidden`)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub time_map: bool,
    /// print looks and light (`look.rs`): reel-ir effects, direct mode only
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub looks: Vec<crate::look::Look>,
    /// color grading beyond the CSS filter channels (direct mode only): `tint=` color and
    /// amount, `levels=` [in_black, in_white, gamma, out_black, out_white], `lut=` .cube file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<(Rgba, f64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub levels: Option<[f64; 5]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lut: Option<String>,
    /// this layer is a light source (`light #id`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light: Option<crate::look::LightCfg>,
    /// `reveal #x with #fluid`: (fluid layer id, matte mode) -> reel-ir matte effect
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fluid_matte: Option<(String, String)>,
    /// a fluid drawn only as a matte (`reveal ... only`)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fluid_hidden: bool,
    /// cut out of the parent instead of drawn (`knockout`): reel-ir alpha-inverted matte
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub knockout: bool,
    /// raw extra CSS for the HTML backend only (escape hatch, never used by story/motion verbs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Debug, Clone, Serialize)]
pub struct Layer {
    pub id: String,
    /// user-facing id (`#name`) if the author gave one
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: Kind,
    pub layout: Layout,
    pub style: Style,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Layer>,
    /// class tags for the HTML backend + lint measure ("text", "caption", "hotspot", ...)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl Layer {
    pub fn new(id: impl Into<String>, kind: Kind) -> Self {
        Layer { id: id.into(), name: None, kind, layout: Layout::default(), style: Style { origin: (0.5, 0.5), ..Default::default() }, children: vec![], tags: vec![] }
    }
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Layer)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }
}

/// Work that needs final layout (measured once before frame 0, never per frame).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Hook {
    /// magic move: geometry delta from layer `from` to layer `to`
    Magic { from: String, to: String },
    /// connector path between two layers, drawn into path layer `path`
    Link { path: String, a: String, b: String, curve: bool },
    /// shrink font size of `layer` until its text fits its box
    Fit { layer: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioClip {
    pub src: String,
    pub at: f64,
    pub dur: f64,
    pub gain: f64,
    /// music: duck to `duck` gain while any voice clip plays
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duck: Option<f64>,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Font {
    pub family: String,
    pub src: String,
    /// custom font file (`font "Brand" src=...`), absolute path; registered under `family`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ir {
    pub version: &'static str,
    pub format: String,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    pub duration: f64,
    pub bg: Rgba,
    pub fonts: Vec<Font>,
    pub root: Layer,
    pub tracks: Vec<Track>,
    pub hooks: Vec<Hook>,
    pub audio: Vec<AudioClip>,
    /// Determinism: quantization steps for "no visible change" (see SPEC §Determinism).
    pub quant: Quant,
}

#[derive(Debug, Clone, Serialize)]
pub struct Quant {
    pub px: f64,
    pub opacity: f64,
    pub angle: f64,
    pub scale: f64,
    pub color: f64,
}
impl Default for Quant {
    fn default() -> Self {
        Quant { px: 1.0 / 64.0, opacity: 1.0 / 255.0, angle: 0.01, scale: 1.0 / 4096.0, color: 1.0 / 255.0 }
    }
}

/// Counter text: thousands grouping (`sep:<c>` tag on the layer; default `.` from 10 000 on, as German
/// narration writes "12.480"; `sep=none` turns it off).
pub fn counter_text(l: &Layer, v: i64) -> String {
    let sep = l.tags.iter().find_map(|t| t.strip_prefix("sep:")).unwrap_or(if v.abs() >= 10_000 { "." } else { "" });
    if sep.is_empty() || sep == "none" {
        return v.to_string();
    }
    let digits = v.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push_str(sep);
        }
        out.push(c);
    }
    if v < 0 { format!("-{out}") } else { out }
}
