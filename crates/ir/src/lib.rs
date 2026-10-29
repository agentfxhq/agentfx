//! reel IR: the renderer's input. A scene is a tree of nodes (paint order),
//! property tracks on those nodes, and raster/vector assets. Frontends:
//! - HTML mode: HTML/CSS + @keyframes -> IR (M0: Chrome lays out and rasterizes)
//! - direct mode: hand-written / generated JSON (or the Rust API) -> IR
//!
//! JSON form and the M0/M1 support matrix: see ../IR.md.
//!
//! Coordinate convention: one shared scene space (CSS px, y down). A node's
//! transform maps its subtree from scene space to its parent's space and is applied
//! about `origin` (scene coordinates): M = T(origin) * transform * T(-origin).
//! Content rects and clip rects are given in scene coordinates.

use serde::{Deserialize, Serialize};

pub mod cube;
pub mod env;
pub mod eval;
pub mod s3d;
pub mod timing;
pub mod validate;

pub type NodeId = usize;
pub type AssetId = usize;

pub const IR_VERSION: u32 = 1;
/// Versions `load` accepts (v0 files are a subset of v1).
pub const IR_VERSIONS: [u32; 2] = [0, 1];

/// 2D affine matrix in CSS `matrix(a, b, c, d, e, f)` order:
/// x' = a*x + c*y + e, y' = b*x + d*y + f.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };
    pub fn translate(x: f64, y: f64) -> Affine {
        Affine { e: x, f: y, ..Affine::IDENTITY }
    }
    pub fn scale(sx: f64, sy: f64) -> Affine {
        Affine { a: sx, d: sy, ..Affine::IDENTITY }
    }
    pub fn rotate_deg(deg: f64) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }
    /// self * rhs (apply rhs first) — same as CSS `transform: self rhs`.
    pub fn mul(&self, r: &Affine) -> Affine {
        Affine {
            a: self.a * r.a + self.c * r.b,
            b: self.b * r.a + self.d * r.b,
            c: self.a * r.c + self.c * r.d,
            d: self.b * r.c + self.d * r.d,
            e: self.a * r.e + self.c * r.f + self.e,
            f: self.b * r.e + self.d * r.f + self.f,
        }
    }
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }
    /// Largest axis scale (column norms).
    pub fn max_scale(&self) -> f64 {
        self.a.hypot(self.b).max(self.c.hypot(self.d))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepPos {
    JumpStart,
    JumpEnd,
    JumpNone,
    JumpBoth,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Easing {
    /// JSON: "linear"
    Linear,
    /// cubic-bezier(x1, y1, x2, y2), solved like Chrome (gfx::CubicBezier).
    /// JSON: {"cubic-bezier": [0.25, 0.1, 0.25, 1]}
    CubicBezier(f64, f64, f64, f64),
    /// JSON: {"steps": [4, "jump-end"]}
    Steps(u32, StepPos),
    /// Damped spring from 0 to 1 over the segment (physical time = segment
    /// duration in seconds); may overshoot. Not a CSS feature.
    /// JSON: {"spring": {"mass": 1, "stiffness": 170, "damping": 26, "velocity": 0}}
    Spring { mass: f64, stiffness: f64, damping: f64, velocity: f64 },
}

impl Easing {
    pub const EASE: Easing = Easing::CubicBezier(0.25, 0.1, 0.25, 1.0);
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FillMode {
    None,
    Forwards,
    Backwards,
    Both,
}

/// One transform function. Lengths in px, angles in degrees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransformFn {
    Translate(f64, f64),
    Scale(f64, f64),
    Rotate(f64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Value {
    /// transform function list (empty = identity)
    Transform(Vec<TransformFn>),
    /// opacity and other scalars
    Number(f64),
    /// Any value reel does not interpolate (HTML paint/layout values); only
    /// used for change detection: equal strings = identical rendering.
    Discrete(String),
    /// Straight-alpha sRGB RGBA 0..1 (`fill.color` / `stroke.color` tracks), interpolated per
    /// channel (straight, like Reel Script's color channels), clamped to 0..1.
    Color([f64; 4]),
}

/// Result of evaluating a track at one time.
#[derive(Clone, Debug, PartialEq)]
pub enum Sampled {
    Matrix(Affine),
    Number(f64),
    Sig(String),
    Color([f64; 4]),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// 0.0 ..= 1.0
    pub offset: f64,
    pub value: Value,
    /// Easing of the segment that starts at this keyframe (None = track easing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easing: Option<Easing>,
}

/// One animation of one property on one node. Same timing model as a CSS
/// animation / Web Animation with iteration count 1: active in
/// [delay, delay + duration), fill decides before/after, and when several
/// tracks of a node set the same property the one with the highest `order`
/// that has an effect wins (otherwise the node's base value applies).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub node: NodeId,
    /// "transform" | "opacity" | "trim-start" | "trim-end" | "glyph.opacity" |
    /// "glyph.transform" | "effects.<i>.<field>" | "fill.color" | "stroke.color" (path
    /// content: replaces the fill / stroke brush by a solid color) (see IR.md for stages).
    pub property: String,
    #[serde(default)]
    pub order: usize,
    /// Free-form origin label (e.g. the @keyframes name), for reports.
    #[serde(default)]
    pub label: String,
    /// Seconds.
    pub delay: f64,
    /// Seconds, > 0.
    pub duration: f64,
    pub fill: FillMode,
    pub easing: Easing,
    /// Sorted by offset; first 0.0, last 1.0.
    pub keyframes: Vec<Keyframe>,
    /// Per-unit offset for `glyph.*` tracks: unit i (glyph/word/line of a Text node)
    /// runs the track delayed by `i * each` seconds (order per `from`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger: Option<Stagger>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stagger {
    /// Seconds between consecutive units.
    pub each: f64,
    /// "start" (default) | "end" | "center"
    #[serde(default = "start")]
    pub from: String,
}
fn start() -> String {
    "start".into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    /// `src + dst` (Porter-Duff plus with normal mix; unchanged since v1).
    Plus,
    // Separable and non-separable W3C compositing modes (source-over), drawn by vello_cpu.
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
    /// Porter-Duff source-atop (normal mix): the group shows only where the backdrop inside
    /// the enclosing group has alpha (e.g. an image fill clipped to glyphs drawn before it).
    SrcAtop,
}

/// Rounded rectangle in scene coordinates. radius = [tl, tr, br, bl].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RRect {
    pub rect: [f64; 4],
    #[serde(default)]
    pub radius: [f64; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum Content {
    /// A raster asset covering `rect` (scene coordinates). `variants`: the same
    /// content rasterized at other scales (e.g. the scales a layer rests at); the
    /// renderer uses a variant whose scale equals the on-screen scale exactly
    /// (pixel-exact copy), otherwise the largest-scale raster.
    Image {
        asset: AssetId,
        rect: [f64; 4],
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        variants: Vec<AssetId>,
    },
    /// Raster content that changes: `frames[i] = (first_frame, asset)`, sorted,
    /// first entry at frame 0. All assets cover `rect`.
    Sequence { rect: [f64; 4], frames: Vec<(u32, AssetId)> },
    /// Solid rounded rectangle (vector). color = straight RGBA 0..1.
    Rect { rect: RRect, color: [f64; 4] },
    /// Resolved vector display list (M1): what the HTML frontend emits for one paint
    /// unit (an element subset), modelled on Takumi's PaintDocument steps / anyrender
    /// commands. Coordinates are scene coordinates (each item may carry its own
    /// transform). `rect` = ink bounds in scene coordinates (damage and culling).
    /// `elements`: identity of the painted DOM elements (HTML mode: document-order
    /// element indices, = `querySelectorAll('*')`), for tracing and recorder round trips.
    Paint {
        rect: [f64; 4],
        items: Vec<Draw>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        elements: Vec<usize>,
        /// Text animation of the list's glyph runs (text v2): range/key selectors per
        /// char/word/line, text on a path, animated fill/stroke. `rect` stays the rest ink;
        /// the renderer evaluates the animated ink per frame.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<Box<TextFx>>,
    },
    /// Content that switches between states over time (repaint layers, typed text in
    /// HTML mode): `frames[i] = (first_frame, state index)`, sorted, first at frame 0.
    /// `rect` = union of the states' ink. A switch is damage.
    Switch { rect: [f64; 4], frames: Vec<(u32, usize)>, states: Vec<Content> },
    /// Native text (M1): shaped by reel at load time (Parley/HarfRust), drawn as glyph
    /// runs at the exact fractional on-screen position. Per-glyph/word/line tracks
    /// (`glyph.opacity`, `glyph.transform` with `stagger`) animate individual units.
    Text(TextContent),
    /// Vector path with fill and/or stroke; `trim` (animatable: `trim-start`,
    /// `trim-end`) draws only that fraction of the total arc length (draw-on).
    Path(PathContent),
    /// Instanced point cloud (one paint unit for any number of points): `count` points whose
    /// positions, colors, sizes and opacities are evaluated per frame (and per motion-blur
    /// sample) from a compact description: a list of formations (`forms`) the points morph
    /// between, with per-point stagger, easing, seeded drift, and an optional projection of
    /// 3D positions. No per-point tracks. Drawn by a dedicated splat routine.
    Cloud(CloudContent),
    /// 2D fluid (X5): a dye field advected by a stable-fluids simulation on a reduced grid,
    /// driven by emitters (pour, stir, other nodes' coverage as dye). Simulated once,
    /// sequentially, and cached per frame, so frame n is a pure function of n. See IR.md
    /// "Fluid".
    Fluid(FluidContent),
    /// Stylised 3D (camera, lights, fog, objects with flat/lambert/toon/phong materials,
    /// outlines, screens showing other nodes): rasterised by the renderer into one layer. See
    /// IR.md "Scene 3D" and `s3d.rs`.
    Scene3d(Box<s3d::Scene3d>),
}

/// 2D affine as `[a, b, c, d, e, f]` (CSS matrix order), used in display lists.
pub type Mat = [f64; 6];
pub const MAT_IDENTITY: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
fn is_identity(m: &Mat) -> bool {
    *m == MAT_IDENTITY
}
fn mat_identity() -> Mat {
    MAT_IDENTITY
}

/// Geometry of a display-list item (before its transform).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    /// `[x0, y0, x1, y1]`
    Rect([f64; 4]),
    /// `[x0, y0, x1, y1]` with radii `[tl, tr, br, bl]`
    RoundedRect { rect: [f64; 4], radii: [f64; 4] },
    /// SVG path data.
    Path(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Extend {
    #[default]
    Pad,
    Repeat,
    Reflect,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub offset: f32,
    /// straight sRGB RGBA 0..1
    pub color: [f32; 4],
}

/// Paint source of a fill, stroke or glyph run. Colors are straight-alpha sRGB 0..1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Brush {
    Solid([f32; 4]),
    Linear { start: [f64; 2], end: [f64; 2], stops: Vec<Stop>, #[serde(default)] extend: Extend },
    Radial {
        start_center: [f64; 2],
        start_radius: f32,
        end_center: [f64; 2],
        end_radius: f32,
        stops: Vec<Stop>,
        #[serde(default)]
        extend: Extend,
    },
    Sweep { center: [f64; 2], start_angle: f32, end_angle: f32, stops: Vec<Stop>, #[serde(default)] extend: Extend },
    /// Raster image (asset, straight-alpha PNG at natural size, `scale` ignored) in its own
    /// pixel space; the item's `brush_transform` places it. quality: 0 low, 1 medium (bilinear), 2 high.
    Image { asset: AssetId, #[serde(default = "one_u8")] quality: u8, #[serde(default)] extend: [Extend; 2], #[serde(default = "one_f32")] alpha: f32 },
}
fn one_u8() -> u8 {
    1
}
fn one_f32() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    pub width: f64,
    /// "miter" | "round" | "bevel"
    #[serde(default = "miter")]
    pub join: String,
    #[serde(default = "four")]
    pub miter_limit: f64,
    /// "butt" | "round" | "square"
    #[serde(default = "butt")]
    pub cap: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dash: Vec<f64>,
    #[serde(default)]
    pub dash_offset: f64,
}
fn miter() -> String {
    "miter".into()
}
fn butt() -> String {
    "butt".into()
}
fn four() -> f64 {
    4.0
}

/// Filter effect on a display-list layer (CSS `filter`), in the layer's local units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum DrawFilter {
    Blur { std_dev: f64 },
    DropShadow { dx: f64, dy: f64, std_dev: f64, color: [f32; 4] },
}

/// One display-list command (paint order). Mirrors anyrender's command set, so any
/// Blitz/anyrender paint can be recorded into the IR and replayed by reel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "op")]
pub enum Draw {
    Fill {
        shape: Shape,
        brush: Brush,
        #[serde(default = "mat_identity", skip_serializing_if = "is_identity")]
        transform: Mat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brush_transform: Option<Mat>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        even_odd: bool,
    },
    Stroke {
        shape: Shape,
        brush: Brush,
        style: StrokeStyle,
        #[serde(default = "mat_identity", skip_serializing_if = "is_identity")]
        transform: Mat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brush_transform: Option<Mat>,
    },
    /// Positioned glyphs of one font at one size. `glyphs = [[glyph id, x, y], ...]`
    /// in the run's space (`transform` maps it to the scene).
    Glyphs {
        font: FontId,
        size: f32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        coords: Vec<i16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        embolden: Option<[f64; 2]>,
        /// Vertical hinting requested by the producer (reel renders unhinted by default,
        /// like Chrome with `--font-render-hinting=none`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        hint: bool,
        brush: Brush,
        #[serde(default = "mat_identity", skip_serializing_if = "is_identity")]
        transform: Mat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glyph_transform: Option<Mat>,
        glyphs: Vec<(u32, f32, f32)>,
        /// Outline of the glyphs (text stroke), drawn by the rasterizer from the outlines.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<GlyphStroke>,
    },
    /// Blurred rounded rectangle (CSS box-shadow), `rect = [x0, y0, x1, y1]`.
    BoxShadow { rect: [f64; 4], radius: f64, std_dev: f64, color: [f32; 4], #[serde(default = "mat_identity", skip_serializing_if = "is_identity")] transform: Mat },
    /// Start a group: clipped by `clip` (if any), composited with `alpha`/`blend`,
    /// optionally filtered. Closed by `pop`.
    Push {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clip: Option<Shape>,
        #[serde(default = "mat_identity", skip_serializing_if = "is_identity")]
        transform: Mat,
        #[serde(default = "one_f32")]
        alpha: f32,
        #[serde(default)]
        blend: BlendMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<DrawFilter>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        even_odd: bool,
    },
    Pop,
}

pub type FontId = usize;

/// Text stroke of a glyph run: the glyph outlines stroked with `width` (run units, centered
/// on the outline). `over` false (default): drawn below the fill, so only the outer half
/// shows (outlined captions); true: drawn over the fill (CSS `-webkit-text-stroke`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlyphStroke {
    pub width: f64,
    pub brush: Brush,
    /// "round" (default) | "miter" | "bevel"
    #[serde(default = "round")]
    pub join: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub over: bool,
}
fn round() -> String {
    "round".into()
}

/// Text animation on a `paint` list (text v2). The list's glyphs are split into units
/// (char = every glyph that is not a space, word, line; found from the glyph data: space
/// glyphs and baselines), placed on `path` if given, then every animator adds its
/// properties scaled by its selector's amount per unit. Tracks: `text.fill` /
/// `text.stroke` (color), `text.stroke-width`, `text.path-offset`,
/// `animators.<i>.start|end|offset|amount` (numbers).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct TextFx {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animators: Vec<TextAnimator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<TextPath>,
}

/// One text animator: properties at full amount, applied per unit scaled by the amount
/// (`a`) the selector gives that unit: opacity `lerp(1, opacity, a)` (multiplied),
/// translate `a * translate` px, scale `lerp(1, scale, a)` and rotate `a * rotate` deg about
/// the unit's box center, blur `a * blur` (gaussian sigma px), fill color
/// `lerp(fill, color, a)`. Animators compose in list order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextAnimator {
    /// "char" | "word" | "line"
    pub by: String,
    pub selector: TextSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f64; 4]>,
}

/// Which units an animator affects, and how much (0..1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum TextSelector {
    /// After Effects range selector. Unit i of n covers `[i/n, (i+1)/n]` (in `order`); the
    /// range is `[min(start, end) + offset, max(start, end) + offset]` (fractions of the
    /// text). `shape`: `square` (covered fraction of the unit), `ramp-up` (0 before the
    /// range, rising to 1 at its end, 1 after), `ramp-down` (mirror), `triangle`, `round`
    /// (half circle), `smooth` (cosine bell), all evaluated at the unit's center except
    /// `square`. Times `amount`.
    Range {
        #[serde(default)]
        start: f64,
        #[serde(default = "one")]
        end: f64,
        #[serde(default)]
        offset: f64,
        #[serde(default = "square")]
        shape: String,
        #[serde(default = "one")]
        amount: f64,
        /// "forward" (default) | "reverse" | "random" (seeded permutation)
        #[serde(default = "forward")]
        order: String,
        #[serde(default)]
        seed: u64,
    },
    /// Per-unit key times: unit i's amount follows `units[i]` (`[[t, amount], ...]`,
    /// seconds, sorted), eased per segment by `easing`, held before the first and after the
    /// last key. Units past the list use the last entry. Times `amount`.
    Keys {
        units: Vec<Vec<[f64; 2]>>,
        #[serde(default = "linear")]
        easing: Easing,
        #[serde(default = "one")]
        amount: f64,
    },
}
fn square() -> String {
    "square".into()
}
fn forward() -> String {
    "forward".into()
}
fn linear() -> Easing {
    Easing::Linear
}

/// Text on a path: every glyph's origin moves to arc length `x - x0 + offset` along `d`
/// (scene coordinates; x0 = the leftmost glyph origin of the text), rotated to the tangent
/// at the glyph's center; the distance from the first line's baseline becomes a distance
/// along the normal. Glyphs off either end are hidden. `offset` animates with
/// `text.path-offset` (px).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextPath {
    pub d: String,
    #[serde(default)]
    pub offset: f64,
}

/// A font file (TTF/OTF; collections via `index`), relative to the IR file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontAsset {
    pub path: String,
    #[serde(default)]
    pub index: u32,
}

/// Native text: authoring form (shaped at load) — position, font, size, weight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
    /// Index into `Scene::fonts`.
    pub font: FontId,
    pub size: f64,
    /// Variable-font weight (wght axis) / weight selection. Default 400.
    #[serde(default = "w400")]
    pub weight: f64,
    /// Extra advance per character, px.
    #[serde(default)]
    pub letter_spacing: f64,
    /// Line height as a multiple of `size`.
    #[serde(default = "lh")]
    pub line_height: f64,
    /// Top-left of the layout box (scene coordinates); with `align` center/right
    /// the box spans `max_width` (required for those).
    pub pos: [f64; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f64>,
    /// "left" | "center" | "right"
    #[serde(default = "left")]
    pub align: String,
    pub brush: Brush,
    /// Unit that per-glyph tracks address: "glyph" (default), "word", "line".
    #[serde(default = "glyph_unit")]
    pub unit: String,
}
fn w400() -> f64 {
    400.0
}
fn lh() -> f64 {
    1.2
}
fn left() -> String {
    "left".into()
}
fn glyph_unit() -> String {
    "glyph".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathContent {
    /// SVG path data, scene coordinates.
    pub d: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Brush>,
    /// Brush transform of the fill (brush space -> scene), e.g. to place an image brush.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_transform: Option<Mat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<PathStroke>,
    /// Base trim `[start, end]` as fractions of the total length (tracks `trim-start` / `trim-end`).
    #[serde(default = "full_trim")]
    pub trim: [f64; 2],
}
fn full_trim() -> [f64; 2] {
    [0.0, 1.0]
}

/// Point cloud content (`Content::Cloud`). Positions are scene coordinates (z: toward the
/// viewer, only used through `projection`). Deterministic: `f(t)` is a pure function of the
/// description and `seed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloudContent {
    /// Number of points.
    pub count: u32,
    #[serde(default)]
    pub seed: u64,
    /// Point radius, scene px (times the form's `size`, the per-point jitter and the
    /// projection scale).
    #[serde(default = "cloud_r")]
    pub radius: f64,
    /// Per-point radius variation: factor uniformly in `[1 - j, 1 + j]` (0..1).
    #[serde(default)]
    pub radius_jitter: f64,
    /// Colors (straight sRGB RGBA 0..1) that `color` rules index into.
    pub palette: Vec<[f32; 4]>,
    /// Initial palette split (fractions per palette entry, random by seed) when the first
    /// form has no `color` rule. Empty = all points palette[0].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weights: Vec<f64>,
    /// Formations in time order; `forms[0]` is where the points start (its timing is ignored).
    pub forms: Vec<CloudForm>,
    /// Projection of `(x, y, z, 1)` into the node's space: piecewise linear 3x4 matrices
    /// (rows x, y, w; screen = (x/w, y/w)), clamped at the ends. Empty: 2D (z ignored).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projection: Vec<CloudView>,
    /// Seconds added to the scene time before evaluating (a cut-out frame range of a longer
    /// scene keeps its cloud's motion).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub time_offset: f64,
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn cloud_r() -> f64 {
    2.0
}

/// Fluid content (`Content::Fluid`): a dye density field simulated with stable fluids
/// (semi-Lagrangian advection, pressure projection, vorticity confinement) on a grid of
/// `rect` size x `res`, drawn as `color` with coverage `1 - exp(-density * dye)`.
/// Emitter positions are in the node's content coordinates (like `rect`); times are seconds
/// of simulation time (= scene time + `time_offset`, or the `fluid.time` track).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FluidContent {
    pub rect: [f64; 4],
    /// grid cells per scene px (0.05..=1, default 0.25)
    #[serde(default = "fluid_res")]
    pub res: f64,
    /// kinematic viscosity in (rect width)^2 / s (Stam's units); 0 = inviscid
    #[serde(default)]
    pub viscosity: f64,
    /// fraction of the dye kept per 1/30 s (0..=1, default 0.99)
    #[serde(default = "fluid_diss")]
    pub dissipation: f64,
    /// fraction of the velocity kept per 1/30 s (0..=1, default 0.995)
    #[serde(default = "fluid_drag")]
    pub drag: f64,
    /// vorticity confinement strength (0 = off; 5..30 curls)
    #[serde(default)]
    pub vorticity: f64,
    /// upward acceleration per unit dye, scene px/s^2 (smoke); negative sinks
    #[serde(default)]
    pub buoyancy: f64,
    /// seeded, slowly evolving curl-noise acceleration everywhere, scene px/s^2
    #[serde(default)]
    pub turbulence: f64,
    #[serde(default)]
    pub seed: u64,
    /// straight RGBA 0..1
    pub color: [f64; 4],
    /// display gain: coverage = 1 - exp(-density * dye)
    #[serde(default = "fluid_density")]
    pub density: f64,
    /// dense dye darker by up to this fraction (0..1): depth inside the stain
    #[serde(default)]
    pub shade: f64,
    /// 0 = off; else (0..1) drawn normally only as a rim: coverage above `edge` fades out to
    /// transparent at full coverage, while a matte effect sourcing the fluid still sees the
    /// full coverage (an ink reveal with dark edges)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub edge: f64,
    /// multigrid V-cycles of the pressure solve per step (1..=20, default 2)
    #[serde(default = "fluid_iters")]
    pub iterations: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emitters: Vec<FluidEmitter>,
    /// seconds added to the scene time (a cut-out frame range keeps its fluid's history)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub time_offset: f64,
}
fn fluid_res() -> f64 {
    0.25
}
fn fluid_diss() -> f64 {
    0.99
}
fn fluid_drag() -> f64 {
    0.995
}
fn fluid_density() -> f64 {
    2.5
}
fn fluid_iters() -> u32 {
    2
}

/// What drives a fluid, active for simulation times `start <= t < end`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum FluidEmitter {
    /// A jet: dye at `rate` per second (peak, Gaussian of `radius` px) and velocity pulled
    /// towards `speed` px/s along `dir` (degrees, 0 = +x, 90 = +y/down), the direction
    /// swinging by a seeded +-`wobble` degrees.
    Pour { start: f64, end: f64, at: [f64; 2], #[serde(default)] dir: f64, #[serde(default = "pour_speed")] speed: f64, radius: f64, #[serde(default = "one")] rate: f64, #[serde(default)] wobble: f64 },
    /// A vortex: velocity pulled towards a rigid rotation of `strength` rad/s (negative =
    /// counter-clockwise on screen) inside a Gaussian of `radius` px.
    Stir { start: f64, end: f64, at: [f64; 2], radius: f64, strength: f64 },
    /// Another node's coverage (its subtree, own opacity ignored, evaluated at that time)
    /// injected as dye: `amount` in total, spread evenly over the window (all at once when
    /// it is shorter than a step), plus a velocity kick: `speed` px/s along `dir` (degrees)
    /// and seeded curl noise of `noise` px/s, inside the coverage.
    Source { start: f64, end: f64, node: NodeId, #[serde(default = "one")] amount: f64, #[serde(default)] speed: f64, #[serde(default)] dir: f64, #[serde(default)] noise: f64 },
}
fn pour_speed() -> f64 {
    400.0
}

impl FluidEmitter {
    pub fn window(&self) -> (f64, f64) {
        match self {
            FluidEmitter::Pour { start, end, .. } | FluidEmitter::Stir { start, end, .. } | FluidEmitter::Source { start, end, .. } => (*start, *end),
        }
    }
}

/// One projection key of a point cloud.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloudView {
    /// Seconds.
    pub t: f64,
    /// Row-major 3x4: `[m00, m01, m02, m03, m10, .., m13, m20, .., m23]`.
    pub m: [f64; 12],
}

/// A formation of a point cloud and the morph into it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloudForm {
    /// Seconds: the morph into this form starts (stagger offset 0).
    #[serde(default)]
    pub at: f64,
    /// Seconds each point takes to arrive (0 = jump).
    #[serde(default)]
    pub dur: f64,
    /// Seconds between the first and the last point's start (negative: reversed order).
    #[serde(default)]
    pub stagger: f64,
    /// Stagger order: `index` | `random` | `distance` (from the form's center) | `x` | `y` | `angle`.
    #[serde(default = "by_index")]
    pub stagger_by: String,
    #[serde(default = "lin")]
    pub easing: Easing,
    /// How points are matched to the form's slots: `curve` (Hilbert order of source and
    /// target: neighbours stay neighbours, few crossings; default), `angle`, `index`, `random`.
    #[serde(default = "curve")]
    pub assign: String,
    pub shape: CloudShape,
    /// Seeded, divergence-free drift (curl noise) added to the slots while in this form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<CloudDrift>,
    /// Radius factor in this form.
    #[serde(default = "one")]
    pub size: f64,
    /// Point opacity in this form.
    #[serde(default = "one")]
    pub opacity: f64,
    /// Palette assignment of the slots (None: points keep their colors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<CloudColor>,
}
fn by_index() -> String {
    "index".into()
}
fn curve() -> String {
    "curve".into()
}
fn lin() -> Easing {
    Easing::Linear
}

/// Drift: displacement `amp` px of a curl-noise field with feature size `scale` px that
/// evolves at `speed` (field periods per second).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloudDrift {
    pub amp: f64,
    #[serde(default = "drift_scale")]
    pub scale: f64,
    #[serde(default = "drift_speed")]
    pub speed: f64,
}
fn drift_scale() -> f64 {
    260.0
}
fn drift_speed() -> f64 {
    0.25
}

/// Palette rule: slots sorted by `by` (`random` | `index` | `x` | `y` | `z` | `distance` |
/// `angle` | `dir`), split into consecutive groups by `split` (fractions per palette entry;
/// empty = equal). `angle` (deg) sets the direction of `dir`; `center` the reference point of
/// `distance`/`angle` (default: the slots' center).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloudColor {
    #[serde(default = "random")]
    pub by: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub split: Vec<f64>,
    #[serde(default)]
    pub angle: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    /// Waviness of a `dir` split: the boundary bends by `wave[0]` px with period `wave[1]` px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave: Option<[f64; 2]>,
}
fn random() -> String {
    "random".into()
}

/// Generators of a formation's `count` slot positions (scene px).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum CloudShape {
    /// Uniform random in `rect`, z uniform in `[-depth/2, depth/2]`; `flow` px/s streams the
    /// points through the rect (wrapping around).
    Field { rect: [f64; 4], #[serde(default)] depth: f64, #[serde(default)] flow: [f64; 2] },
    /// Regular grid in `rect` (`cols` 0 = from the aspect), `jitter` in cells.
    Grid { rect: [f64; 4], #[serde(default)] cols: u32, #[serde(default)] jitter: f64 },
    /// Log-spiral galaxy: `arms`, `twist` (radians per e-fold of radius), gaussian `spread`
    /// (fraction of the radius), `squash` (y scale), `spin` deg/s (differential: faster inside).
    Spiral {
        center: [f64; 2],
        radius: f64,
        #[serde(default = "three")]
        arms: u32,
        #[serde(default = "twist")]
        twist: f64,
        #[serde(default = "spread")]
        spread: f64,
        #[serde(default = "one")]
        squash: f64,
        #[serde(default)]
        spin: f64,
    },
    /// Gaussian cluster, `sigma` = [sx, sy, sz] px.
    Gaussian { center: [f64; 3], sigma: [f64; 3] },
    /// Points on a sphere (`center` [x, y, z]), `spin` deg/s about the vertical axis.
    Sphere { center: [f64; 3], radius: f64, #[serde(default)] spin: f64 },
    /// Height field: a `rows` x cols grid over `rect` = [x, y, w, h], heights `amp` px (up) of
    /// a seeded wave field with feature size `scale` px travelling at `speed` (periods per
    /// second). `deep`: the rows run into depth instead (z from 0 to -h at ground line y; use
    /// with a projection), otherwise from the bottom of the rect up.
    Terrain {
        rect: [f64; 4],
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        deep: bool,
        #[serde(default)]
        rows: u32,
        #[serde(default = "terrain_amp")]
        amp: f64,
        #[serde(default = "terrain_scale")]
        scale: f64,
        #[serde(default)]
        speed: f64,
    },
    /// Points filling (`outline` false) or tracing the outline of shaped text.
    Text(CloudText),
    /// An explicit point list ([x, y] or [x, y, z]); repeated cyclically up to `count`.
    Points { points: Vec<Vec<f64>> },
}
fn three() -> u32 {
    3
}
fn twist() -> f64 {
    1.45
}
fn spread() -> f64 {
    0.1
}
fn terrain_amp() -> f64 {
    60.0
}
fn terrain_scale() -> f64 {
    240.0
}

/// Text formation: shaped like `TextContent` (font, size, weight, pos/max_width/align).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloudText {
    pub text: String,
    pub font: FontId,
    pub size: f64,
    #[serde(default = "w400")]
    pub weight: f64,
    #[serde(default)]
    pub letter_spacing: f64,
    #[serde(default = "lh")]
    pub line_height: f64,
    pub pos: [f64; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f64>,
    #[serde(default = "left")]
    pub align: String,
    /// Points on the glyph outlines instead of inside them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outline: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathStroke {
    #[serde(flatten)]
    pub style: StrokeStyle,
    pub brush: Brush,
}

/// Node effects ("raw Rust" part: not CSS-bound). Applied to the node's composited
/// subtree in list order. Parameters are animatable with tracks named
/// `effects.<index>.<field>` (numbers): radius, strength, dx, dy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum Effect {
    /// Gaussian blur, `radius` = standard deviation in scene px.
    Blur { radius: f64 },
    /// Blurred, tinted copy of the subtree's alpha behind it, offset by (dx, dy).
    DropShadow { #[serde(default)] dx: f64, #[serde(default)] dy: f64, radius: f64, color: [f64; 4] },
    /// Blurred, tinted copy of the subtree's alpha behind it (no offset), x `strength`.
    Glow { radius: f64, color: [f64; 4], #[serde(default = "one")] strength: f64 },
    /// Multiply the subtree by another node's coverage. `source` is drawn only as a matte
    /// (mark it `hidden`). mode: "alpha" | "alpha-inverted" | "luma" | "luma-inverted".
    Matte { source: NodeId, #[serde(default = "alpha_mode")] mode: String },
    // ---- print looks (X8, compiler/src/look.rs) and light on flat design (X6,
    // compiler/src/light.rs). Seeded and deterministic; see IR.md "Looks and light".
    /// Risograph: the subtree split into spot-color plates (`inks`, 1..=4), each offset by a
    /// seeded sub-pixel misregistration (`misreg` px, plus a slow seeded `drift` px at
    /// `drift_speed` Hz), multiplied together, grain inside the ink and low-frequency density
    /// variation. `paper`: the subtree's own paper color (printed areas stay opaque); without
    /// it the plates are transparent ink (exact over white).
    Riso {
        inks: Vec<[f64; 4]>,
        #[serde(default)]
        seed: u64,
        #[serde(default = "misreg_default")]
        misreg: f64,
        #[serde(default)]
        drift: f64,
        #[serde(default = "quarter")]
        drift_speed: f64,
        #[serde(default)]
        grain: f64,
        #[serde(default)]
        density: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paper: Option<[f64; 4]>,
    },
    /// AM halftone: round dots on a grid of `cell` px rotated by `angle` deg, dot area from
    /// darkness. mode "mono" (one `ink`) or "cmyk" (four screens at 15/75/0/45 deg + angle).
    Halftone {
        cell: f64,
        #[serde(default = "deg45")]
        angle: f64,
        #[serde(default = "mono")]
        mode: String,
        #[serde(default = "black")]
        ink: [f64; 4],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paper: Option<[f64; 4]>,
    },
    /// Film/paper grain inside the subtree's alpha: `amount` (0..1), grain `size` px. `live`:
    /// a new pattern every frame (the subtree then changes on every frame: no frame skipping).
    Grain {
        amount: f64,
        #[serde(default)]
        seed: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        live: bool,
        #[serde(default = "one")]
        size: f64,
    },
    /// Paper: ink bleed (blur-threshold of the edges, `bleed` px), edge roughness (`rough`
    /// 0..1) and paper texture (`texture` 0..1: fibers and tooth showing `color` through).
    Paper {
        #[serde(default)]
        texture: f64,
        #[serde(default)]
        bleed: f64,
        #[serde(default)]
        rough: f64,
        #[serde(default)]
        seed: u64,
        #[serde(default = "paper_white")]
        color: [f64; 4],
    },
    /// Soft shadow of the subtree (a flat layer `height` px above the page) from a spherical
    /// light at `light` = [x, y, z] (scene px; z = height above the page) with radius
    /// `light_radius`; penumbra from the subtree's signed distance field. Scene px throughout.
    Shadow {
        light: [f64; 3],
        #[serde(default = "light_radius_default")]
        light_radius: f64,
        height: f64,
        #[serde(default = "shadow_color")]
        color: [f64; 4],
        #[serde(default = "one")]
        strength: f64,
    },
    /// A soft light band across the subtree (screen-blended `color` x `strength`, gaussian
    /// profile of `width` px), at `pos` 0..1 across its ink along `angle` deg.
    Sweep {
        #[serde(default)]
        pos: f64,
        #[serde(default)]
        angle: f64,
        width: f64,
        color: [f64; 4],
        #[serde(default = "one")]
        strength: f64,
    },
    /// God rays: light from `light` [x, y] (scene px) behind the subtree streams through its
    /// holes (transparent pixels inside its ink box): radial blur of that mask away from the
    /// light over `length` x the distance to it, added in `color` x `strength`.
    Rays {
        light: [f64; 2],
        #[serde(default = "half")]
        length: f64,
        color: [f64; 4],
        #[serde(default = "one")]
        strength: f64,
    },
    /// Channel displacement in scene pixels, optionally increasing radially.
    Rgbsplit { dx: f64, dy: f64, amount: f64, #[serde(default = "split_mode")] mode: String },
    /// Seeded displacement events per second.
    Glitch { amount: f64, #[serde(default)] seed: u64, rate: f64, block: f64 },
    /// Analog tape distortion.
    Vhs { amount: f64, #[serde(default)] seed: u64 },
    /// Stable spatial quantization; Bayer matrix size is 2, 4 or 8.
    Dither { levels: f64, mode: String, #[serde(default = "bayer_size")] size: u32, #[serde(default)] palette: Option<Vec<[f64; 4]>> },
    /// Built-in 5x7 glyphs, charset ordered from dark to light.
    Ascii { cell: f64, charset: String, color: String, bg: [f64; 4] },
    /// Color adjustment of the subtree, in straight sRGB, steps in this order (each clamped
    /// to 0..1 like a CSS filter chain): `brightness` (multiplier, CSS `brightness()`),
    /// `saturate` (CSS `saturate()`), `contrast` (CSS `contrast()`), `hue` (degrees, CSS
    /// `hue-rotate()`), then `tint` (the pixel's luma times `tint` color, mixed in by
    /// `tint_amount`), `levels` (`[in_black, in_white, gamma, out_black, out_white]`, 0..1,
    /// all channels) and `lut` (a `.cube` 3D LUT file relative to the IR file, trilinear).
    /// Alpha is kept. Animatable: brightness, saturate, contrast, hue, tint (= tint_amount).
    Color {
        #[serde(default = "one")]
        brightness: f64,
        #[serde(default = "one")]
        saturate: f64,
        #[serde(default = "one")]
        contrast: f64,
        #[serde(default)]
        hue: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tint: Option<[f64; 4]>,
        #[serde(default = "one")]
        tint_amount: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        levels: Option<[f64; 5]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lut: Option<String>,
    },
}
fn split_mode() -> String { "linear".into() }
fn bayer_size() -> u32 { 4 }
fn alpha_mode() -> String {
    "alpha".into()
}
fn misreg_default() -> f64 {
    2.0
}
fn quarter() -> f64 {
    0.25
}
fn deg45() -> f64 {
    45.0
}
fn mono() -> String {
    "mono".into()
}
fn black() -> [f64; 4] {
    [0.08, 0.08, 0.1, 1.0]
}
fn paper_white() -> [f64; 4] {
    [0.97, 0.95, 0.91, 1.0]
}
fn light_radius_default() -> f64 {
    40.0
}
fn shadow_color() -> [f64; 4] {
    [0.05, 0.04, 0.08, 0.5]
}

impl Effect {
    /// Animatable fields of a look/light effect (`effects.<i>.<field>` tracks), in parameter
    /// order. Blur/drop-shadow/glow keep their own layout (see `compiler/src/schedule.rs`).
    pub fn look_fields(&self) -> &'static [&'static str] {
        match self {
            Effect::Rgbsplit { .. } => &["dx", "dy", "amount"],
            Effect::Glitch { .. } => &["amount", "rate", "block"],
            Effect::Vhs { .. } => &["amount"],
            Effect::Dither { .. } => &["levels"],
            Effect::Ascii { .. } => &["cell"],
            Effect::Riso { .. } => &["misreg"],
            Effect::Halftone { .. } => &["cell", "angle"],
            Effect::Grain { .. } => &["amount"],
            Effect::Paper { .. } => &["texture", "bleed", "rough"],
            Effect::Shadow { .. } => &["lx", "ly", "lz", "radius", "height", "strength"],
            Effect::Sweep { .. } => &["pos", "angle", "width", "strength"],
            Effect::Rays { .. } => &["lx", "ly", "length", "strength"],
            Effect::Color { .. } => &["brightness", "saturate", "contrast", "hue", "tint"],
            _ => &[],
        }
    }
    /// Base (untracked) value of a look/light field.
    pub fn look_field(&self, name: &str) -> f64 {
        match (self, name) {
            (Effect::Rgbsplit { dx, .. }, "dx") => *dx,
            (Effect::Rgbsplit { dy, .. }, "dy") => *dy,
            (Effect::Rgbsplit { amount, .. } | Effect::Glitch { amount, .. } | Effect::Vhs { amount, .. }, "amount") => *amount,
            (Effect::Glitch { rate, .. }, "rate") => *rate,
            (Effect::Glitch { block, .. }, "block") => *block,
            (Effect::Dither { levels, .. }, "levels") => *levels,
            (Effect::Ascii { cell, .. }, "cell") => *cell,
            (Effect::Riso { misreg, .. }, "misreg") => *misreg,
            (Effect::Halftone { cell, .. }, "cell") => *cell,
            (Effect::Halftone { angle, .. }, "angle") => *angle,
            (Effect::Grain { amount, .. }, "amount") => *amount,
            (Effect::Paper { texture, .. }, "texture") => *texture,
            (Effect::Paper { bleed, .. }, "bleed") => *bleed,
            (Effect::Paper { rough, .. }, "rough") => *rough,
            (Effect::Shadow { light, .. }, "lx") => light[0],
            (Effect::Shadow { light, .. }, "ly") => light[1],
            (Effect::Shadow { light, .. }, "lz") => light[2],
            (Effect::Shadow { light_radius, .. }, "radius") => *light_radius,
            (Effect::Shadow { height, .. }, "height") => *height,
            (Effect::Shadow { strength, .. } | Effect::Sweep { strength, .. } | Effect::Rays { strength, .. }, "strength") => *strength,
            (Effect::Sweep { pos, .. }, "pos") => *pos,
            (Effect::Sweep { angle, .. }, "angle") => *angle,
            (Effect::Sweep { width, .. }, "width") => *width,
            (Effect::Rays { light, .. }, "lx") => light[0],
            (Effect::Rays { light, .. }, "ly") => light[1],
            (Effect::Rays { length, .. }, "length") => *length,
            (Effect::Color { brightness, .. }, "brightness") => *brightness,
            (Effect::Color { saturate, .. }, "saturate") => *saturate,
            (Effect::Color { contrast, .. }, "contrast") => *contrast,
            (Effect::Color { hue, .. }, "hue") => *hue,
            (Effect::Color { tint_amount, .. }, "tint") => *tint_amount,
            _ => 0.0,
        }
    }
    /// The effect's pixels depend on the time itself (riso drift, live grain): the node
    /// changes on every frame.
    pub fn time_dependent(&self) -> bool {
        match self {
            Effect::Riso { drift, .. } => *drift != 0.0,
            Effect::Grain { live, .. } => *live,
            Effect::Glitch { .. } | Effect::Vhs { .. } => true,
            _ => false,
        }
    }
    /// A look/light effect (X6/X8), handled by `compiler/src/look.rs` / `light.rs`.
    pub fn is_look(&self) -> bool {
        !self.look_fields().is_empty()
    }
    /// Depends on the node's absolute position (light effects): rigid moves change it.
    pub fn position_dependent(&self) -> bool {
        matches!(self, Effect::Shadow { .. } | Effect::Rays { .. })
    }
}

/// Opt-in motion blur: the node's subtree is rendered at `samples` sub-frame times
/// spread over `shutter` frames (centered on the frame time) and averaged.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionBlur {
    pub samples: u32,
    #[serde(default = "half")]
    pub shutter: f64,
}
fn half() -> f64 {
    0.5
}

/// Time effect of a node's subtree (X3): the subtree is evaluated at other times than the
/// frame's and those renders are combined. `span` (seconds) is animatable through the track
/// property `time.span`; span 0 = the plain subtree (effect off). See IR.md "Time effects".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum TimeFx {
    /// Slit-scan: screen row (axis y) or column (axis x) at position p shows the subtree at
    /// `t - span * u`, u = (p - range[0]) / (range[1] - range[0]) clamped to 0..1. Time is
    /// sampled every `band` px and cross-faded between samples.
    Slitscan {
        #[serde(default = "axis_y")]
        axis: String,
        span: f64,
        /// screen coordinates along the axis; default the whole frame; b < a reverses
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<[f64; 2]>,
        #[serde(default = "four")]
        band: f64,
    },
    /// Echo: `count` trailing copies at `t - k * span` (k = 1..count) under the subtree,
    /// copy k at opacity `decay^k`, composited oldest first with `blend`; `tint` (optional,
    /// 1 or 2 colours) recolours the copies, newest to oldest.
    Echo {
        count: u32,
        span: f64,
        #[serde(default = "decay_default")]
        decay: f64,
        #[serde(default)]
        blend: BlendMode,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tint: Vec<[f64; 4]>,
    },
    /// Time displacement: pixel p shows the subtree at `t - span * map(p)`, map in 0..1,
    /// quantized to `levels` time samples and cross-faded between them.
    Displace {
        map: TimeMap,
        span: f64,
        #[serde(default = "levels_default")]
        levels: u32,
    },
}

/// Per-pixel time offset map (0..1) of a displace effect, screen coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum TimeMap {
    /// distance from `center` / `radius`, clamped
    Radial { center: [f64; 2], radius: f64 },
    /// projection onto the segment `from` -> `to`, clamped
    Linear { from: [f64; 2], to: [f64; 2] },
    /// seeded value noise (`octaves` of fbm), feature size `scale` px
    Noise { scale: f64, #[serde(default)] seed: u64, #[serde(default = "octaves_default")] octaves: u32 },
    /// luma of a node's subtree (usually hidden) at the frame time, composited over black
    Layer { node: NodeId },
}

fn axis_y() -> String {
    "y".into()
}
fn decay_default() -> f64 {
    0.6
}
fn levels_default() -> u32 {
    24
}
fn octaves_default() -> u32 {
    3
}

impl TimeFx {
    pub fn span(&self) -> f64 {
        match self {
            TimeFx::Slitscan { span, .. } | TimeFx::Echo { span, .. } | TimeFx::Displace { span, .. } => *span,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub name: String,
    /// Children in paint order (back to front).
    #[serde(default)]
    pub children: Vec<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,
    /// Transform origin, scene coordinates.
    #[serde(default)]
    pub origin: [f64; 2],
    /// Base transform (used when no track has an effect).
    #[serde(default)]
    pub transform: Vec<TransformFn>,
    /// Base opacity.
    #[serde(default = "one")]
    pub opacity: f64,
    /// Clips applied to this node and its subtree, in the parent's space.
    #[serde(default)]
    pub clips: Vec<RRect>,
    #[serde(default)]
    pub blend: BlendMode,
    /// Composite children into an isolated group when opacity < 1 or blend != normal
    /// (CSS group opacity). Default true.
    #[serde(default = "yes")]
    pub isolate: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion_blur: Option<MotionBlur>,
    /// Time effect of the subtree (slit-scan, echo, time displacement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<TimeFx>,
    /// World canvas (X9): the static part of the subtree is drawn from a mip tile cache while
    /// the node's transform moves (camera moves), as vectors when it rests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<World>,
    /// Evaluated but not drawn in the normal pass (matte sources).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    /// PNG path, relative to the IR file.
    pub path: String,
    /// Device px per scene px (pixmap size = content rect * scale).
    pub scale: f64,
}

/// World canvas (X9) on a node: while the node's chain (camera) moves, its static
/// descendants are composited from a tiled mip pyramid rastered in the node's own space
/// (tiles at power-of-two scales, rastered on demand along the camera path, LRU within
/// `cache_mb`); frames where the chain rests draw vectors. Animated descendants draw live.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct World {
    /// Tile edge in device px (power of two, 64..=1024).
    #[serde(default = "world_tile")]
    pub tile: u32,
    /// Tile cache budget per renderer segment (MB).
    #[serde(default = "world_cache_mb")]
    pub cache_mb: u32,
}
fn world_tile() -> u32 {
    256
}
fn world_cache_mb() -> u32 {
    512
}
impl Default for World {
    fn default() -> Self {
        World { tile: world_tile(), cache_mb: world_cache_mb() }
    }
}

/// Planned: 2.5D camera (perspective, z per node). Not in M0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub perspective: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u32,
    /// nodes[0] is the root.
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub assets: Vec<Asset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fonts: Vec<FontAsset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<Camera>,
    /// Text antialiasing: `gray` (default) or `lcd` (subpixel, like Chrome on Linux for text
    /// on opaque, non-composited content, e.g. a Remotion render).
    #[serde(default, skip_serializing_if = "TextAa::is_gray")]
    pub text_aa: TextAa,
}

/// Text antialiasing mode of a scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextAa {
    #[default]
    Gray,
    Lcd,
}
impl TextAa {
    pub fn is_gray(&self) -> bool {
        *self == TextAa::Gray
    }
}

impl Node {
    /// An empty node (identity transform, opacity 1, isolated).
    pub fn new(name: impl Into<String>) -> Node {
        Node { name: name.into(), children: vec![], content: None, origin: [0.0, 0.0], transform: vec![], opacity: 1.0, clips: vec![], blend: BlendMode::Normal, isolate: true, effects: vec![], motion_blur: None, time: None, world: None, hidden: false }
    }
}

impl Scene {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Scene> {
        let s = std::fs::read_to_string(path)?;
        let scene: Scene = serde_json::from_str(&s)?;
        Ok(scene)
    }
    /// Parent of every node (None for the root).
    pub fn parents(&self) -> Vec<Option<NodeId>> {
        let mut p = vec![None; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            for &c in &n.children {
                p[c] = Some(i);
            }
        }
        p
    }
}
