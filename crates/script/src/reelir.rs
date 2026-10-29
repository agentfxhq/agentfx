//! Direct mode (S1): Motion IR + native layout/paint (`native.rs`, Blitz) -> reel-ir v1
//! (renderer `crates/render`, docs/IR.md v1). No Chrome anywhere.
//!
//! - Geometry, text and box paint come from the native layout pass: every layer becomes a
//!   reel-ir node whose content is its *own* display list (box, borders, shadows, rich text
//!   glyph runs, inline SVG, raw `html`), vector, in scene coordinates.
//! - Composite channels become `transform` / `opacity` tracks with symbolic eases (springs stay
//!   springs); several overlapping channels, layout offsets and 3D are sampled per frame.
//! - Paint/layout channels reel-ir has no track for (color, fill, gradients, wght, tracking,
//!   w, h, line-h, counters) are resolved by the compiler: at every frame where such a value
//!   changes, the snapshot is restyled and laid out again; layers whose own paint changed get
//!   `switch` states (vector repaint states), layers that only moved get layout-offset
//!   translate keys. Marked `W_IR_SWITCH` (a reel-ir track would be cheaper, see
//!   IR-PROPOSALS.md).
//! - Wipe/iris reveals and paint masks are alpha/luma `matte` effects with a hidden source node
//!   whose transform is animated (exact, composite stage).
//! - Everything reel-ir cannot express is listed (`W_IR_UNSUPPORTED` / `W_IR_APPROX`).
use crate::diag::Diag;
use crate::html::Snap;
use crate::ir::*;
use crate::motion::{self, Chans, A2};
use crate::native::Native;
use crate::values::{ColorRef, Ease, Paint, Rgba, Stop};
use agentfx_ir::{Brush, Draw};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub struct Out {
    pub scene: Value,
    pub diags: Vec<Diag>,
    pub stats: Value,
}

fn rgba(c: &Rgba) -> Value {
    json!([r4(c.r.clamp(0.0, 1.0)), r4(c.g.clamp(0.0, 1.0)), r4(c.b.clamp(0.0, 1.0)), r4(c.a.clamp(0.0, 1.0))])
}
fn r4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}
fn lit(c: &ColorRef) -> Rgba {
    match c {
        ColorRef::Lit(c) => c.clone(),
        ColorRef::Token(_) => Rgba { r: 1.0, g: 0.0, b: 1.0, a: 1.0 },
    }
}

/// Motion IR paint over a box -> reel-ir brush (scene coords, CSS gradient geometry).
pub fn brush(p: &Paint, r: [f64; 4]) -> Option<Brush> {
    let stops = |s: &Vec<Stop>| -> (Vec<(f64, Rgba)>, f64, f64) {
        let n = s.len().max(2) as f64 - 1.0;
        let v: Vec<(f64, Rgba)> = s.iter().enumerate().map(|(k, st)| (st.pos.unwrap_or(k as f64 / n), lit(&st.color))).collect();
        let lo = v.iter().map(|x| x.0).fold(f64::MAX, f64::min).min(0.0);
        let hi = v.iter().map(|x| x.0).fold(f64::MIN, f64::max).max(1.0);
        (v, lo, hi)
    };
    let js = |v: &Vec<(f64, Rgba)>, lo: f64, hi: f64| -> Vec<agentfx_ir::Stop> {
        v.iter().map(|(o, c)| agentfx_ir::Stop { offset: ((o - lo) / (hi - lo)) as f32, color: [c.r as f32, c.g as f32, c.b as f32, c.a as f32] }).collect()
    };
    Some(match p {
        Paint::Solid(c) => {
            let c = lit(c);
            Brush::Solid([c.r as f32, c.g as f32, c.b as f32, c.a as f32])
        }
        Paint::Linear { angle_deg, stops: s } => {
            let (v, lo, hi) = stops(s);
            let a = angle_deg.to_radians();
            let (dx, dy) = (a.sin(), -a.cos());
            let l = (r[2] * dx).abs() + (r[3] * dy).abs();
            let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
            let s0 = (cx - dx * l / 2.0, cy - dy * l / 2.0);
            Brush::Linear { start: [s0.0 + dx * l * lo, s0.1 + dy * l * lo], end: [s0.0 + dx * l * hi, s0.1 + dy * l * hi], stops: js(&v, lo, hi), extend: agentfx_ir::Extend::Pad }
        }
        Paint::Radial { cx, cy, stops: s } => {
            let (v, lo, hi) = stops(s);
            let c = [r[0] + cx * r[2], r[1] + cy * r[3]];
            let far = [(r[0], r[1]), (r[0] + r[2], r[1]), (r[0], r[1] + r[3]), (r[0] + r[2], r[1] + r[3])].iter().map(|(x, y)| ((x - c[0]).powi(2) + (y - c[1]).powi(2)).sqrt()).fold(0.0, f64::max);
            Brush::Radial { start_center: c, start_radius: (far * lo) as f32, end_center: c, end_radius: (far * hi) as f32, stops: js(&v, lo, hi), extend: agentfx_ir::Extend::Pad }
        }
        Paint::Conic { angle_deg, stops: s } => {
            let (v, lo, hi) = stops(s);
            Brush::Sweep { center: [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0], start_angle: (angle_deg - 90.0) as f32, end_angle: (angle_deg + 270.0) as f32, stops: js(&v, lo, hi), extend: agentfx_ir::Extend::Pad }
        }
        // raster paints need an asset: `Em::raster_brush`
        Paint::Mesh { .. } | Paint::Image { .. } => return None,
    })
}

fn easing(e: &Ease) -> Value {
    match e {
        Ease::Linear => json!("linear"),
        Ease::Cubic(a, b, c, d) => json!({"cubic-bezier": [a, b, c, d]}),
        Ease::Steps(n) => json!({"steps": [n, "jump-end"]}),
        Ease::Hold => json!({"steps": [1, "jump-end"]}),
        Ease::Spring { stiffness, damping, mass, velocity } => json!({"spring": {"mass": mass, "stiffness": stiffness, "damping": damping, "velocity": velocity}}),
        Ease::Bounce { .. } => json!({"cubic-bezier": [0.33, 1.0, 0.68, 1.0]}),
    }
}

/// SVG path data cursor: numbers per the SVG grammar (`1.5.5` = 1.5 0.5, `1-2`, exponents),
/// arc flags as single `0`/`1` characters (`a1 1 0 01 10 10`).
struct PathCursor<'s> {
    b: &'s [u8],
    i: usize,
}
impl PathCursor<'_> {
    fn skip(&mut self) {
        while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b',') {
            self.i += 1;
        }
    }
    fn cmd(&mut self) -> Option<char> {
        self.skip();
        let c = *self.b.get(self.i)?;
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            self.i += 1;
            return Some(c as char);
        }
        None
    }
    fn at_end(&mut self) -> bool {
        self.skip();
        self.i >= self.b.len()
    }
    fn num(&mut self) -> Option<f64> {
        self.skip();
        let st = self.i;
        let b = self.b;
        let mut j = self.i;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let d0 = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j < b.len() && b[j] == b'.' {
            j += 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
        }
        if j == d0 || (j == d0 + 1 && b[d0] == b'.') {
            return None;
        }
        if j < b.len() && (b[j] == b'e' || b[j] == b'E') {
            let mut k = j + 1;
            if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
                k += 1;
            }
            if k < b.len() && b[k].is_ascii_digit() {
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                j = k;
            }
        }
        self.i = j;
        std::str::from_utf8(&b[st..j]).ok()?.parse().ok()
    }
    fn flag(&mut self) -> Option<f64> {
        self.skip();
        match self.b.get(self.i)? {
            b'0' => {
                self.i += 1;
                Some(0.0)
            }
            b'1' => {
                self.i += 1;
                Some(1.0)
            }
            _ => None,
        }
    }
}

/// Map SVG path data (absolute and relative M L H V C S Q T A Z, implicit repeats) into
/// absolute commands scaled by (sx, sy) and translated by (dx, dy). Arcs keep their arc form
/// (radii scaled; a non-uniform scale of a rotated arc becomes cubics). Parsing stops at the
/// first malformed token (like SVG: the path renders up to the error).
pub fn map_path(d: &str, dx: f64, dy: f64, sx: f64, sy: f64) -> String {
    let mut c = PathCursor { b: d.as_bytes(), i: 0 };
    let mut out: Vec<String> = vec![];
    let fx = |x: f64| x * sx + dx;
    let fy = |y: f64| y * sy + dy;
    let n2 = |v: f64| format!("{:.2}", v);
    let (mut cx, mut cy, mut x0, mut y0) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut cmd: Option<char> = None;
    'outer: loop {
        if c.at_end() {
            break;
        }
        if let Some(k) = c.cmd() {
            cmd = Some(k);
            if k == 'Z' || k == 'z' {
                out.push("Z".into());
                (cx, cy) = (x0, y0);
                continue;
            }
        }
        let Some(k) = cmd else { break };
        if k == 'Z' || k == 'z' {
            break; // numbers after Z
        }
        let rel = k.is_ascii_lowercase();
        let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
        let pts = |c: &mut PathCursor, n: usize| -> Option<Vec<(f64, f64)>> {
            let mut v = vec![];
            for _ in 0..n {
                let x = c.num()?;
                let y = c.num()?;
                v.push((x + ox, y + oy));
            }
            Some(v)
        };
        match k.to_ascii_uppercase() {
            'M' | 'L' | 'T' => {
                let Some(p) = pts(&mut c, 1) else { break };
                let up = k.to_ascii_uppercase();
                out.push(format!("{up}{} {}", n2(fx(p[0].0)), n2(fy(p[0].1))));
                (cx, cy) = p[0];
                if up == 'M' {
                    (x0, y0) = p[0];
                    // further coordinate pairs are implicit line-tos
                    cmd = Some(if rel { 'l' } else { 'L' });
                }
            }
            'H' => {
                let Some(x) = c.num() else { break };
                cx = x + ox;
                out.push(format!("H{}", n2(fx(cx))));
            }
            'V' => {
                let Some(y) = c.num() else { break };
                cy = y + oy;
                out.push(format!("V{}", n2(fy(cy))));
            }
            'C' | 'S' | 'Q' => {
                let n = if k.to_ascii_uppercase() == 'C' { 3 } else { 2 };
                let Some(p) = pts(&mut c, n) else { break };
                let v: Vec<String> = p.iter().flat_map(|q| [n2(fx(q.0)), n2(fy(q.1))]).collect();
                out.push(format!("{}{}", k.to_ascii_uppercase(), v.join(" ")));
                (cx, cy) = p[n - 1];
            }
            'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (c.num(), c.num(), c.num()) else { break };
                let (Some(large), Some(sweep)) = (c.flag(), c.flag()) else { break };
                let (Some(x), Some(y)) = (c.num(), c.num()) else { break };
                let (x, y) = (x + ox, y + oy);
                let r90 = rot.rem_euclid(180.0);
                let axis = r90 == 0.0 || r90 == 90.0;
                if (sx.abs() - sy.abs()).abs() > 1e-9 && !axis {
                    // a rotated ellipse under a non-uniform scale: exact as cubics
                    let arc = kurbo::SvgArc { from: kurbo::Point::new(cx, cy), to: kurbo::Point::new(x, y), radii: kurbo::Vec2::new(rx.abs(), ry.abs()), x_rotation: rot.to_radians(), large_arc: large != 0.0, sweep: sweep != 0.0 };
                    match kurbo::Arc::from_svg_arc(&arc) {
                        Some(a) => a.to_cubic_beziers(0.01, |p1, p2, p| {
                            out.push(format!("C{} {} {} {} {} {}", n2(fx(p1.x)), n2(fy(p1.y)), n2(fx(p2.x)), n2(fy(p2.y)), n2(fx(p.x)), n2(fy(p.y))));
                        }),
                        None => out.push(format!("L{} {}", n2(fx(x)), n2(fy(y)))),
                    }
                } else {
                    let (ka, kb) = if r90 == 90.0 { (sy.abs(), sx.abs()) } else { (sx.abs(), sy.abs()) };
                    let sw = if sx * sy < 0.0 { 1.0 - sweep } else { sweep };
                    out.push(format!("A{} {} {} {} {} {} {}", n2(rx.abs() * ka), n2(ry.abs() * kb), n2(rot), large as i64, sw as i64, n2(fx(x)), n2(fy(y))));
                }
                (cx, cy) = (x, y);
            }
            _ => break 'outer,
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod path_tests {
    use super::map_path;
    #[test]
    fn absolute_paths_keep_their_form() {
        assert_eq!(map_path("M10 20 L30 40 Z", 0.0, 0.0, 1.0, 1.0), "M10.00 20.00 L30.00 40.00 Z");
        assert_eq!(map_path("M0,0 C1,2 3,4 5,6", 100.0, 10.0, 2.0, 1.0), "M100.00 10.00 C102.00 12.00 106.00 14.00 110.00 16.00");
        assert_eq!(map_path("M0 0 A10 10 0 0 1 20 0", 0.0, 0.0, 2.0, 2.0), "M0.00 0.00 A20.00 20.00 0.00 0 1 40.00 0.00");
    }
    #[test]
    fn relative_and_implicit_commands() {
        // m with implicit l, h, v, z returns to the subpath start
        assert_eq!(map_path("m10 10 20 0 v10 h-20 z l5 5", 0.0, 0.0, 1.0, 1.0), "M10.00 10.00 L30.00 10.00 V20.00 H10.00 Z L15.00 15.00");
        assert_eq!(map_path("M1 1 c1 1 2 2 3 3 s1 1 2 2", 0.0, 0.0, 1.0, 1.0), "M1.00 1.00 C2.00 2.00 3.00 3.00 4.00 4.00 S5.00 5.00 6.00 6.00");
        assert_eq!(map_path("M0 0 q5 5 10 0 t10 0", 0.0, 0.0, 1.0, 1.0), "M0.00 0.00 Q5.00 5.00 10.00 0.00 T20.00 0.00");
        // compact numbers and arc flags
        assert_eq!(map_path("M.5.5l-1.5-.5a1 1 0 01 2 2", 0.0, 0.0, 1.0, 1.0), "M0.50 0.50 L-1.00 0.00 A1.00 1.00 0.00 0 1 1.00 2.00");
        assert_eq!(map_path("M0 0L1e1 2E0", 0.0, 0.0, 1.0, 1.0), "M0.00 0.00 L10.00 2.00");
    }
    #[test]
    fn arcs_under_scales() {
        // 90 deg rotated ellipse: radii swap their scale factors; a mirror flips the sweep
        assert_eq!(map_path("M0 0 A10 5 90 0 1 0 20", 0.0, 0.0, 2.0, 1.0), "M0.00 0.00 A10.00 10.00 90.00 0 1 0.00 20.00");
        assert_eq!(map_path("M0 0 A10 10 0 0 1 20 0", 0.0, 0.0, -1.0, 1.0), "M0.00 0.00 A10.00 10.00 0.00 0 0 -20.00 0.00");
        // rotated + non-uniform: cubics ending at the mapped end point
        let d = map_path("M0 0 A10 5 30 0 1 10 10", 0.0, 0.0, 2.0, 1.0);
        assert!(d.starts_with("M0.00 0.00 C") && d.ends_with("20.00 10.00"), "{d}");
        // malformed: stops at the error
        assert_eq!(map_path("M0 0 L5 x 9", 0.0, 0.0, 1.0, 1.0), "M0.00 0.00");
    }
}

/// Channels the compiler resolves into switch states / layout offsets (no reel-ir track yet).
fn resolved_ch(c: Ch) -> bool {
    matches!(c, Ch::Color | Ch::Fill | Ch::Stroke | Ch::Grad0 | Ch::Grad1 | Ch::Grad2 | Ch::Grad3 | Ch::GradAngle | Ch::Wght | Ch::Tracking | Ch::W | Ch::H | Ch::LineH | Ch::Count) || c.is_axis()
}
fn layout_ch(c: Ch) -> bool {
    c.stage() == Stage::Layout
}
fn inherited_ch(c: Ch) -> bool {
    matches!(c, Ch::Color | Ch::Wght | Ch::Tracking) || c.is_axis()
}
fn quant(c: Ch, v: &TV) -> Vec<i64> {
    match v {
        TV::C(c) => vec![(c.r * 255.0).round() as i64, (c.g * 255.0).round() as i64, (c.b * 255.0).round() as i64, (c.a * 255.0).round() as i64],
        TV::N(n) => vec![match c {
            Ch::Count => n.round() as i64,
            Ch::Wght | Ch::GradAngle => (n * 4.0).round() as i64,
            c if c.is_axis() => (n * 4.0).round() as i64,
            Ch::Tracking | Ch::LineH => (n * 4096.0).round() as i64,
            _ => (n * 64.0).round() as i64,
        }],
    }
}

/// Offset a display list by (dx, dy) (pre-multiplied into every item transform).
fn offset_items(items: &mut [Draw], dx: f64, dy: f64) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let pre = |m: &mut [f64; 6]| {
        m[4] += dx;
        m[5] += dy;
    };
    for it in items.iter_mut() {
        match it {
            Draw::Fill { transform, .. } | Draw::Stroke { transform, .. } | Draw::Glyphs { transform, .. } | Draw::BoxShadow { transform, .. } | Draw::Push { transform, .. } => pre(transform),
            Draw::Pop => {}
        }
    }
}

/// Replace glyph brushes by a paint over `r` (text matte: `fill=` on text), in run space.
/// Raster paints (mesh, image; `raster` from `Em::raster_brush`) are drawn as an image fill of
/// the box composited source-atop over the glyphs, in one group.
fn text_fill(items: &mut Vec<Draw>, p: &Paint, r: [f64; 4], raster: Option<(Brush, [f64; 6])>) {
    if let Some((b, m)) = raster {
        if items.is_empty() {
            return;
        }
        let push = |blend| Draw::Push { clip: None, transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], alpha: 1.0, blend, filter: None, even_odd: false };
        items.insert(0, push(agentfx_ir::BlendMode::Normal));
        items.push(push(agentfx_ir::BlendMode::SrcAtop));
        items.push(Draw::Fill { shape: agentfx_ir::Shape::Rect([r[0], r[1], r[0] + r[2], r[1] + r[3]]), brush: b, transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], brush_transform: Some(m), even_odd: false });
        items.push(Draw::Pop);
        items.push(Draw::Pop);
        return;
    }
    for it in items.iter_mut() {
        if let Draw::Glyphs { brush, transform, .. } = it {
            let inv = motion::a2_inv(transform);
            // brush coordinates are in the item's local space: map the scene-space box into it
            let (x0, y0) = motion::a2_apply(&inv, r[0], r[1]);
            let (x1, y1) = motion::a2_apply(&inv, r[0] + r[2], r[1] + r[3]);
            if let Some(b) = self::brush(p, [x0, y0, x1 - x0, y1 - y0]) {
                *brush = b;
            }
        }
    }
}

/// Wrap a display list in a (rounded) rect clip.
fn clip_items(items: &mut Vec<Draw>, r: [f64; 4], radius: f64) {
    if items.is_empty() {
        return;
    }
    let shape = if radius > 0.0 {
        agentfx_ir::Shape::RoundedRect { rect: [r[0], r[1], r[0] + r[2], r[1] + r[3]], radii: [radius; 4] }
    } else {
        agentfx_ir::Shape::Rect([r[0], r[1], r[0] + r[2], r[1] + r[3]])
    };
    items.insert(0, Draw::Push { clip: Some(shape), transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], alpha: 1.0, blend: agentfx_ir::BlendMode::Normal, filter: None, even_odd: false });
    items.push(Draw::Pop);
}

fn ink(items: &[Draw], nat: &Native) -> [f64; 4] {
    let fonts = &nat.res.font_data;
    let fl = |f: usize| fonts.get(f).cloned();
    crate::record::bounds(items, &fl, false)
}

/// Switch states for one layer: (first frame, state index) and the distinct states.
#[derive(Default)]
struct States {
    frames: Vec<(u32, usize)>,
    states: Vec<Vec<Draw>>,
    keys: Vec<Vec<i64>>,
}

struct Em<'a> {
    ir: &'a Ir,
    nat: &'a mut Native,
    ch: Chans<'a>,
    nodes: Vec<Value>,
    tracks: Vec<Value>,
    assets: Vec<Value>,
    unsupported: BTreeMap<String, usize>,
    approx: BTreeMap<String, usize>,
    switched: BTreeMap<String, usize>,
    rect: HashMap<String, [f64; 4]>,
    magic: HashMap<String, (f64, f64, f64, f64)>,
    link_d: HashMap<String, String>,
    base_paint: HashMap<String, Vec<Draw>>,
    states: HashMap<String, States>,
    /// world layout offset of a layer per layout-state frame (frame, dx, dy), from the base layout
    layout_off: HashMap<String, Vec<(u32, f64, f64)>>,
    /// layers drawn with sampled world transforms (3D context)
    three_d: HashSet<String>,
    /// layers whose box size changes over time (layout channels): their overflow clip is baked
    /// into each repaint state (leaf layers) since reel-ir clips are static
    resized: HashSet<String>,
    /// layers whose own content comes from the native painter (not path/video)
    paintable: HashSet<String>,
    parent: HashMap<String, String>,
    nf: u32,
    stats: serde_json::Map<String, Value>,
    world_cache: HashMap<i64, HashMap<String, (A2, f64)>>,
    /// font files used by cloud text forms (appended after the recorder's fonts)
    cloud_fonts: Vec<String>,
    /// nodes with cloud content (their text forms' font indices are shifted at the end)
    cloud_nodes: Vec<usize>,
    /// node index of every layer, and time-effect nodes whose map is a layer (fixed up last)
    layer_node: HashMap<String, usize>,
    time_maps: Vec<(usize, String)>,
    /// fluid nodes (their source emitters name layers until patched)
    fluid_nodes: Vec<usize>,
    /// `reveal ... with #fluid`: (node, effect index, fluid layer id)
    fluid_mattes: Vec<(usize, usize, String)>,
}

const TF: [Ch; 12] = [Ch::X, Ch::Y, Ch::Z, Ch::Px, Ch::Py, Ch::Scale, Ch::Sx, Ch::Sy, Ch::Rot, Ch::RotX, Ch::RotY, Ch::Magic];

impl<'a> Em<'a> {
    fn note(&mut self, what: &str) {
        *self.unsupported.entry(what.to_string()).or_default() += 1;
    }
    fn asset(&mut self, path: &str, scale: f64) -> usize {
        if let Some(i) = self.assets.iter().position(|a| a["path"] == json!(path) && a["scale"] == json!(scale)) {
            return i;
        }
        self.assets.push(json!({"path": path, "scale": scale}));
        self.assets.len() - 1
    }
    fn glyph_fill(&self, id: &str) -> Option<(Paint, [f64; 4])> {
        let l = self.ir_layer(id)?;
        let source = if l.tags.iter().any(|t| t == "piece") {
            self.ir_layer(self.parent.get(id)?)?
        } else { l };
        if source.style.text_fill { Some((source.style.fill.clone()?, self.rect.get(&source.id).copied()?)) } else { None }
    }
    fn fps(&self) -> f64 {
        self.ir.fps
    }
    /// Raster paints (mesh gradient, image) over the scene box `r`: an image brush and the
    /// brush transform that places it (image px -> scene). None for vector paints.
    fn raster_brush(&mut self, p: &Paint, r: [f64; 4]) -> Option<(Brush, [f64; 6])> {
        let (path, pw, ph, fit) = match p {
            Paint::Mesh { cols, rows, colors } => {
                let lit: Vec<Rgba> = colors.iter().map(lit).collect();
                let path = crate::mesh::png(*cols, *rows, &lit).ok()?;
                (path.to_string_lossy().to_string(), crate::mesh::RES as f64, crate::mesh::RES as f64, "fill")
            }
            Paint::Image { src, fit } => {
                let Some((pw, ph)) = crate::lower::image_size(std::path::Path::new(src)) else {
                    self.note("unsupported image fill");
                    return None;
                };
                (src.clone(), pw, ph, fit.as_str())
            }
            _ => return None,
        };
        let (sx, sy) = match fit {
            "fill" => (r[2] / pw, r[3] / ph),
            "contain" | "scale-down" => {
                let s = (r[2] / pw).min(r[3] / ph);
                if fit == "scale-down" && s > 1.0 { (1.0, 1.0) } else { (s, s) }
            }
            "none" => (1.0, 1.0),
            _ => {
                let s = (r[2] / pw).max(r[3] / ph);
                (s, s)
            }
        };
        if matches!(fit, "contain" | "scale-down" | "none") {
            *self.approx.entry("image fill `contain`/`none` on a path or text: edges extend (pad) beyond the image".into()).or_default() += 1;
        }
        let ox = r[0] + (r[2] - pw * sx) / 2.0;
        let oy = r[1] + (r[3] - ph * sy) / 2.0;
        // brush assets live in the recorder's index space (shifted behind ours at the end)
        let a = self.nat.res.file_image(&path);
        Some((Brush::Image { asset: a, quality: 2, extend: [agentfx_ir::Extend::Pad; 2], alpha: 1.0 }, [sx, 0.0, 0.0, sy, ox, oy]))
    }
    fn track_json(&self, node: usize, prop: &str, keys: &[(f64, Value, Ease)], label: &str) -> Option<Value> {
        if keys.len() < 2 {
            return None;
        }
        // offsets relative to the emitted (rounded) delay/duration, so key times land where they
        // were computed (time-remapped tracks key exact frame times)
        let t0 = r4(keys[0].0);
        let t1 = keys[keys.len() - 1].0;
        let dur = (t1 - t0).max(1e-3);
        let mut prev = -1.0f64;
        let mut kf: Vec<Value> = vec![];
        for (t, v, e) in keys {
            let mut o = ((t - t0) / dur).clamp(0.0, 1.0);
            if o <= prev {
                if prev >= 1.0 {
                    continue;
                }
                o = (prev + 1e-9).min(1.0);
            }
            prev = o;
            kf.push(json!({"offset": o, "value": v, "easing": easing(e)}));
        }
        if kf.len() < 2 {
            return None;
        }
        let n = kf.len();
        kf[0]["offset"] = json!(0.0);
        kf[n - 1]["offset"] = json!(1.0);
        Some(json!({"node": node, "property": prop, "label": label, "delay": t0, "duration": dur, "fill": "both", "easing": "linear", "keyframes": kf}))
    }

    // ---------------------------------------------------------------- layout-time hooks
    fn hooks(&mut self) {
        // fit: shrink the font until the text fits its box (was a load-time JS hook)
        let fits: Vec<String> = self.ir.hooks.iter().filter_map(|h| if let Hook::Fit { layer } = h { Some(layer.clone()) } else { None }).collect();
        for id in fits {
            let Some(mut fs) = self.nat.text_style(&id).map(|s| s.0) else { continue };
            for _ in 0..40 {
                let (ox, oy) = self.nat.overflow(&id);
                if ox <= 1.0 && oy <= 1.0 {
                    break;
                }
                fs *= 0.95;
                let mut snap = self.nat.snap().clone();
                snap.font_size.insert(id.clone(), fs);
                self.nat.set_snap(&snap, std::slice::from_ref(&id));
            }
        }
        // base rects (after fit)
        let ids: Vec<String> = { let mut v = vec![]; self.ir.root.walk(&mut |l| v.push(l.id.clone())); v };
        for id in &ids {
            if let Some(r) = self.nat.rect(id) {
                self.rect.insert(id.clone(), r);
            }
        }
        for h in &self.ir.hooks {
            match h {
                Hook::Magic { from, to } => {
                    if let (Some(a), Some(b)) = (self.rect.get(from), self.rect.get(to)) {
                        let ca = (a[0] + a[2] / 2.0, a[1] + a[3] / 2.0);
                        let cb = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
                        self.magic.insert(to.clone(), (ca.0 - cb.0, ca.1 - cb.1, a[2] / b[2].max(1.0), a[3] / b[3].max(1.0)));
                        self.magic.insert(from.clone(), (cb.0 - ca.0, cb.1 - ca.1, b[2] / a[2].max(1.0), b[3] / a[3].max(1.0)));
                    }
                }
                Hook::Link { path, a, b, curve } => {
                    // endpoints by user name within the path's scene (same rule as the HTML runtime)
                    let (ra, rb) = (self.find_named(path, a), self.find_named(path, b));
                    if let (Some(ra), Some(rb)) = (ra, rb) {
                        self.link_d.insert(path.clone(), link_path(ra, rb, *curve));
                    }
                }
                _ => {}
            }
        }
    }

    fn scene_of(&self, id: &str) -> Option<String> {
        let mut cur = Some(id.to_string());
        while let Some(c) = cur {
            if c.starts_with("scene-") || self.parent.get(&c).map(|p| p == "stage").unwrap_or(false) {
                return Some(c);
            }
            cur = self.parent.get(&c).cloned();
        }
        None
    }
    /// Id of the layer named `name` (same scene first), like `find_named`.
    fn find_named_id(&self, from: &str, name: &str) -> Option<String> {
        let sc = self.scene_of(from);
        let mut hit: Option<String> = None;
        let mut any: Option<String> = None;
        self.ir.root.walk(&mut |l| {
            if l.name.as_deref() == Some(name) {
                if any.is_none() {
                    any = Some(l.id.clone());
                }
                if hit.is_none() && self.scene_of(&l.id) == sc {
                    hit = Some(l.id.clone());
                }
            }
        });
        hit.or(any)
    }

    /// Text v2 on a text layer's content: animators and text on a path (`text` of the paint),
    /// `text.fill` (color channel), `text.path-offset` and selector parameter tracks.
    fn text_v2(&mut self, l: &Layer, idx: usize, tracks: &[&Track], content: &mut Option<Value>, out: &mut Vec<Value>) {
        // text on a path: the target path's scene-space data (rest geometry)
        let mut path_d = None;
        if let Some((name, _)) = &l.style.text_path {
            match self.find_named_id(&l.id, name).and_then(|id| self.ir_layer(&id).map(|p| (id, p))) {
                Some((pid, p)) => match &p.kind {
                    Kind::Path { d, vb, .. } => {
                        let r = self.rect.get(&pid).copied().unwrap_or([0.0; 4]);
                        let sx = if vb.0 > 0.0 { r[2] / vb.0 } else { 1.0 };
                        let sy = if vb.1 > 0.0 { r[3] / vb.1 } else { 1.0 };
                        path_d = Some(self.link_d.get(&pid).cloned().unwrap_or_else(|| map_path(d, r[0], r[1], sx, sy)));
                    }
                    _ => self.note(&format!("text on=#{name}: not a path")),
                },
                None => self.note(&format!("text on=#{name}: no such path")),
            }
        }
        let anims = !l.style.text_anims.is_empty() || path_d.is_some();
        if anims {
            match content.as_mut() {
                Some(c) if c["type"] == "paint" => {
                    if let Some(fx) = crate::textv2::fx_json(l, path_d.clone(), &easing) {
                        c["text"] = fx;
                    }
                }
                Some(_) => self.note("text animators / on= on a text whose paint or layout channels change (repaint states)"),
                None => {}
            }
        }
        let paint = content.as_ref().is_some_and(|c| c["type"] == "paint");
        if !paint {
            return;
        }
        for (prop, t0, d, a, b, e) in crate::textv2::fx_tracks(l) {
            if content.as_ref().and_then(|c| c.get("text")).is_none() {
                break;
            }
            if let Some(j) = self.track_json(idx, &prop, &[(t0, json!({"number": a}), e), (t0 + d, json!({"number": b}), Ease::Linear)], "animate") {
                out.push(j);
            }
        }
        if let Some(t) = tracks.iter().find(|t| crate::textv2::color_track_ok(l, !l.children.is_empty(), t) && t.keys.len() > 1 && !t.is_static()) {
            let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| {
                let TV::C(c) = &k.v else { unreachable!() };
                (k.t, json!({"color": rgba(c)}), k.ease.clone())
            }).collect();
            if let Some(j) = self.track_json(idx, "text.fill", &keys, "color") {
                out.push(j);
            }
        }
        if path_d.is_some() {
            if let Some(t) = tracks.iter().find(|t| t.ch == Ch::PathOffset && t.keys.len() > 1) {
                let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| (k.t, json!({"number": r4(k.v.n())}), k.ease.clone())).collect();
                if let Some(j) = self.track_json(idx, "text.path-offset", &keys, "path-offset") {
                    out.push(j);
                }
            }
        }
    }

    fn find_named(&self, from: &str, name: &str) -> Option<[f64; 4]> {
        let sc = self.scene_of(from);
        let mut hit: Option<String> = None;
        let mut any: Option<String> = None;
        self.ir.root.walk(&mut |l| {
            if l.name.as_deref() == Some(name) {
                if any.is_none() {
                    any = Some(l.id.clone());
                }
                if hit.is_none() && self.scene_of(&l.id) == sc {
                    hit = Some(l.id.clone());
                }
            }
        });
        hit.or(any).and_then(|id| self.rect.get(&id).copied())
    }

    // ---------------------------------------------------------------- resolved channels
    /// Frames where any resolved (paint/layout) channel changes value; at each one the snapshot
    /// is restyled + relaid out, changed layers repainted (switch states), moved layers get
    /// layout offsets.
    fn resolve_states(&mut self) {
        let t0 = std::time::Instant::now();
        let fps = self.fps();
        let ir = self.ir;
        // path fill/stroke colors are reel-ir color tracks (no repaint states)
        let path_color = |t: &Track| (matches!(t.ch, Ch::Fill | Ch::Stroke) && self.ir_layer(&t.layer).is_some_and(|l| matches!(l.kind, Kind::Path { .. })))
            || self.ir_layer(&t.layer).is_some_and(|l| crate::textv2::color_track_ok(l, !l.children.is_empty(), t));
        let dyn_tracks: Vec<&Track> = ir.tracks.iter().filter(|t| resolved_ch(t.ch) && t.keys.len() > 1 && !t.is_static() && !path_color(t)).collect();
        let dyn_layers: BTreeSet<String> = dyn_tracks.iter().map(|t| t.layer.clone()).collect();
        let any_layout = dyn_tracks.iter().any(|t| layout_ch(t.ch));
        // the snapshot carries every track's value (static ones too)
        let snap_at = |t: f64, base: &Snap| -> Snap {
            let mut s = base.clone();
            for tr in &ir.tracks {
                if resolved_ch(tr.ch) {
                    s.vals.insert((tr.layer.clone(), tr.ch), tr.eval(t));
                }
            }
            s
        };
        let mut frames: BTreeSet<u32> = BTreeSet::new();
        for tr in &dyn_tracks {
            let f0 = (tr.keys[0].t * fps).floor().max(0.0) as u32;
            let f1 = ((tr.keys[tr.keys.len() - 1].t * fps).ceil() as u32 + 1).min(self.nf.saturating_sub(1));
            let mut prev = quant(tr.ch, &tr.eval(f0 as f64 / fps));
            for f in f0 + 1..=f1 {
                let v = quant(tr.ch, &tr.eval(f as f64 / fps));
                if v != prev {
                    frames.insert(f);
                    prev = v;
                }
            }
        }
        // layers whose paint depends on resolved values: the animated ones and the descendants of
        // layers with an animated inherited channel (color, wght, tracking)
        let mut watch: BTreeSet<String> = dyn_layers.clone();
        let inh_layers: HashSet<String> = dyn_tracks.iter().filter(|t| inherited_ch(t.ch)).map(|t| t.layer.clone()).collect();
        self.ir.root.walk(&mut |l| {
            let mut cur = Some(l.id.clone());
            while let Some(c) = cur {
                if inh_layers.contains(&c) {
                    watch.insert(l.id.clone());
                    break;
                }
                cur = self.parent.get(&c).cloned();
            }
        });
        let all_ids: Vec<String> = { let mut v = vec![]; self.ir.root.walk(&mut |l| v.push(l.id.clone())); v };
        let key_of = |em: &Em, id: &str, rect: Option<[f64; 4]>| -> Vec<i64> {
            let mut k = vec![];
            let mut cur = Some(id.to_string());
            let mut own = true;
            while let Some(c) = cur {
                for tr in em.ch.by.get(c.as_str()).into_iter().flatten() {
                    if resolved_ch(tr.ch) && (own || inherited_ch(tr.ch)) {
                        k.push(tr.ch as i64);
                        k.extend(quant(tr.ch, &em.nat.snap().vals.get(&(c.clone(), tr.ch)).cloned().unwrap_or(tr.ch.default_value())));
                    }
                }
                own = false;
                cur = em.parent.get(&c).cloned();
            }
            if let Some(r) = rect {
                k.push((r[2] * 64.0).round() as i64);
                k.push((r[3] * 64.0).round() as i64);
            }
            k
        };
        let base_snap = self.nat.snap().clone();
        let mut cur_rect: HashMap<String, [f64; 4]> = self.rect.clone();
        let mut last_key: HashMap<String, Vec<i64>> = HashMap::new();
        let mut n_states = 0usize;
        for &f in &frames {
            let t = f as f64 / fps;
            let s = snap_at(t, &base_snap);
            let changed_layers: Vec<String> = dyn_layers.iter().cloned().collect();
            self.nat.set_snap(&s, &changed_layers);
            let mut moved_or_resized: Vec<String> = vec![];
            if any_layout {
                for id in &all_ids {
                    if let Some(r) = self.nat.rect(id) {
                        let old = cur_rect.get(id).copied().unwrap_or(r);
                        if (0..4).any(|i| (old[i] - r[i]).abs() > 1.0 / 64.0) {
                            moved_or_resized.push(id.clone());
                            let b = self.rect.get(id).copied().unwrap_or(r);
                            self.layout_off.entry(id.clone()).or_default().push((f, r[0] - b[0], r[1] - b[1]));
                            cur_rect.insert(id.clone(), r);
                        }
                    }
                }
            }
            let mut repaint: BTreeSet<String> = watch.clone();
            for id in &moved_or_resized {
                let b = self.rect.get(id).copied().unwrap_or([0.0; 4]);
                let r = cur_rect[id];
                if (b[2] - r[2]).abs() > 1.0 / 64.0 || (b[3] - r[3]).abs() > 1.0 / 64.0 || self.states.contains_key(id) {
                    repaint.insert(id.clone());
                    if (b[2] - r[2]).abs() > 1.0 / 64.0 || (b[3] - r[3]).abs() > 1.0 / 64.0 {
                        self.resized.insert(id.clone());
                    }
                }
            }
            for id in repaint {
                if !self.paintable.contains(&id) {
                    continue;
                }
                let sized = self.layout_off.contains_key(&id).then(|| cur_rect[&id]);
                let key = key_of(self, &id, sized);
                let base_key = last_key.entry(id.clone()).or_insert_with(|| key_of_base(self, &id, &base_snap, sized.map(|_| self.rect[&id])));
                let _ = base_key;
                if last_key.get(&id) == Some(&key) {
                    continue;
                }
                if self.states.get(&id).is_none_or(|st| st.states.is_empty()) {
                    let mut base = self.base_paint.get(&id).cloned().unwrap_or_default();
                    if let Some(l) = self.ir_layer(&id) {
                        if l.style.overflow_clip && l.children.is_empty() {
                            let b = self.rect[&id];
                            clip_items(&mut base, b, l.style.radius);
                        }
                    }
                    let bk = last_key[&id].clone();
                    let st = self.states.entry(id.clone()).or_default();
                    st.states.push(base);
                    st.keys.push(bk);
                    st.frames.push((0, 0));
                }
                let si = match self.states[&id].keys.iter().position(|k| *k == key) {
                    Some(i) => i,
                    None => {
                        let mut items = match self.nat.paint(&id, matches!(self.ir_layer(&id).map(|l| &l.kind), Some(Kind::Image { .. }))) {
                            Ok(v) => v,
                            Err(_) => vec![],
                        };
                        if let Some(r) = cur_rect.get(&id) {
                            let b = self.rect[&id];
                            offset_items(&mut items, b[0] - r[0], b[1] - r[1]);
                            if self.ir_layer(&id).is_some_and(|l| l.style.overflow_clip && l.children.is_empty()) {
                                clip_items(&mut items, [b[0], b[1], r[2], r[3]], self.ir_layer(&id).map(|l| l.style.radius).unwrap_or(0.0));
                            }
                        }
                        if let Some((p, r)) = self.glyph_fill(&id) {
                            let rb = self.raster_brush(&p, r);
                            text_fill(&mut items, &p, r, rb);
                        }
                        if let Some(sk) = self.ir_layer(&id).and_then(crate::textv2::stroke_of) {
                            crate::textv2::apply_stroke(&mut items, &sk);
                        }
                        let st = self.states.get_mut(&id).unwrap();
                        st.states.push(items);
                        st.keys.push(key.clone());
                        n_states += 1;
                        st.states.len() - 1
                    }
                };
                self.states.get_mut(&id).unwrap().frames.push((f, si));
                last_key.insert(id.clone(), key);
            }
        }
        // back to the base snapshot (the rest of the pipeline reads base geometry)
        if !frames.is_empty() {
            self.nat.set_snap(&base_snap, &dyn_layers.iter().cloned().collect::<Vec<_>>());
        }
        self.states.retain(|_, s| s.states.len() > 1);
        self.stats.insert("resolved_frames".into(), json!(frames.len()));
        self.stats.insert("repaint_states".into(), json!(n_states));
        self.stats.insert("resolve_s".into(), json!(t0.elapsed().as_secs_f64()));
    }

    fn ir_layer(&self, id: &str) -> Option<&'a Layer> {
        fn find<'b>(l: &'b Layer, id: &str) -> Option<&'b Layer> {
            if l.id == id {
                return Some(l);
            }
            l.children.iter().find_map(|c| find(c, id))
        }
        find(&self.ir.root, id)
    }

    // ---------------------------------------------------------------- nodes
    fn layer(&mut self, l: &'a Layer, parent_3d: bool) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(Value::Null);
        let r = self.rect.get(&l.id).copied().unwrap_or([0.0, 0.0, self.ir.w as f64, self.ir.h as f64]);
        let tracks: Vec<&Track> = self.ch.by.get(l.id.as_str()).cloned().unwrap_or_default();
        let base = |ch: Ch| tracks.iter().find(|t| t.ch == ch).map(|t| t.keys.first().map(|k| k.v.n()).unwrap_or(t.base.n()));
        let in3d = parent_3d || motion::is_3d(l, &self.ch);
        let mut node = json!({"name": l.name.clone().unwrap_or(l.id.clone()), "children": [],
            "origin": [r4(r[0] + l.style.origin.0 * r[2]), r4(r[1] + l.style.origin.1 * r[3])], "opacity": base(Ch::Opacity).unwrap_or(1.0)});
        if in3d {
            node["origin"] = json!([0.0, 0.0]);
            self.three_d.insert(l.id.clone());
        }
        let baked_clip = self.resized.contains(&l.id) && l.children.is_empty();
        if self.resized.contains(&l.id) && !l.children.is_empty() && l.style.overflow_clip {
            self.note("overflow clip of a resizing container (clip track proposed); clipped at its rest size");
        }
        if l.style.overflow_clip && !baked_clip {
            let rad = l.style.radius;
            node["clips"] = json!([{"rect": [r4(r[0]), r4(r[1]), r4(r[2]), r4(r[3])], "radius": [rad, rad, rad, rad]}]);
        }
        if let Some(b) = &l.style.blend {
            let ok = ["normal", "multiply", "screen", "overlay", "darken", "lighten", "plus", "color-dodge", "color-burn", "hard-light", "soft-light", "difference", "exclusion", "hue", "saturation", "color", "luminosity"];
            let b = if b == "add" || b == "plus-lighter" { "plus".to_string() } else { b.clone() };
            if ok.contains(&b.as_str()) {
                node["blend"] = json!(b);
            } else {
                self.note(&format!("blend mode {b}"));
            }
        }
        if let Some((samples, shutter)) = l.style.motion_blur {
            node["motion_blur"] = json!({"samples": samples, "shutter": shutter});
        }
        if l.tags.iter().any(|t| t == "world") {
            // X9: static content drawn from the renderer's mip tile cache while the camera moves
            let tag = |p: &str| l.tags.iter().find_map(|t| t.strip_prefix(p)).and_then(|v| v.parse::<u32>().ok());
            node["world"] = json!({"tile": tag("world-tile:").unwrap_or(256), "cache_mb": tag("world-cache:").unwrap_or(512)});
        }
        self.layer_node.insert(l.id.clone(), idx);
        if let Some(spec) = &l.style.time_fx {
            if l.style.motion_blur.is_some() {
                self.note("motion blur and a time effect on the same layer (motion blur dropped; put mblur on a child)");
                node.as_object_mut().unwrap().remove("motion_blur");
            }
            node["time"] = crate::timefx::ir_time(spec, r, self.ir.w as f64, self.ir.h as f64, None);
            if let crate::timefx::TimeFxSpec::Displace { map: crate::timefx::MapSpec::Layer { id }, .. } = spec {
                self.time_maps.push((idx, id.clone()));
            }
        }
        if l.style.time_map || l.style.fluid_hidden {
            node["hidden"] = json!(true);
        }
        // effects
        let mut effects = vec![];
        // children cut out of this layer (`knockout`): alpha-inverted mattes first (their
        // sources are patched in once the children have node indices)
        let knockouts: Vec<usize> = l.children.iter().enumerate().filter(|(_, c)| c.style.knockout).map(|(k, _)| k).collect();
        for _ in &knockouts {
            effects.push(json!({"type": "matte", "source": 0, "mode": "alpha-inverted"}));
        }
        if l.style.knockout {
            node["hidden"] = json!(true);
        }
        let mut fx_tracks: Vec<(usize, Ch)> = vec![];
        if tracks.iter().any(|t| t.ch == Ch::Blur) {
            fx_tracks.push((effects.len(), Ch::Blur));
            effects.push(json!({"type": "blur", "radius": base(Ch::Blur).unwrap_or(0.0)}));
        }
        // color adjust (CSS filter order: blur, brightness, saturate, contrast, hue-rotate, then the
        // drop-shadow glow) plus tint / levels / LUT: one reel-ir `color` effect
        let grade = [Ch::Brightness, Ch::Saturate, Ch::Contrast, Ch::Hue].iter().any(|c| tracks.iter().any(|t| t.ch == *c)) || l.style.tint.is_some() || l.style.levels.is_some() || l.style.lut.is_some();
        if grade {
            let k = effects.len();
            let mut e = json!({"type": "color", "brightness": base(Ch::Brightness).unwrap_or(1.0).max(0.0), "saturate": base(Ch::Saturate).unwrap_or(1.0).max(0.0),
                "contrast": base(Ch::Contrast).unwrap_or(1.0).max(0.0), "hue": base(Ch::Hue).unwrap_or(0.0)});
            if let Some((c, a)) = &l.style.tint {
                e["tint"] = rgba(c);
                e["tint_amount"] = json!(a);
            }
            if let Some(lv) = &l.style.levels {
                e["levels"] = json!(lv);
            }
            if let Some(p) = &l.style.lut {
                e["lut"] = json!(p);
            }
            effects.push(e);
            for (c, f) in [(Ch::Brightness, "brightness"), (Ch::Saturate, "saturate"), (Ch::Contrast, "contrast"), (Ch::Hue, "hue")] {
                if let Some(t) = tracks.iter().find(|t| t.ch == c && t.keys.len() > 1 && !t.is_static()) {
                    let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|kk| (kk.t, json!({"number": r4(if f == "hue" { kk.v.n() } else { kk.v.n().max(0.0) })}), kk.ease.clone())).collect();
                    if let Some(j) = self.track_json(idx, &format!("effects.{k}.{f}"), &keys, f) {
                        self.tracks.push(j);
                    }
                }
            }
        }
        if tracks.iter().any(|t| t.ch == Ch::Glow) {
            let c = l.style.glow_color.clone().unwrap_or(Rgba { r: 0.24, g: 0.75, b: 0.53, a: 1.0 });
            fx_tracks.push((effects.len(), Ch::Glow));
            effects.push(json!({"type": "glow", "radius": base(Ch::Glow).unwrap_or(0.0), "color": rgba(&c)}));
        }
        // shadows on non-box kinds are CSS drop-shadow filters (html.rs): reel drop-shadow effect
        let is_box = matches!(l.kind, Kind::Group | Kind::Rect | Kind::Ellipse | Kind::Image { .. } | Kind::Video { .. } | Kind::Counter { .. } | Kind::Html { .. });
        if !is_box {
            for sh in &l.style.shadows {
                let c = sh.color.clone().unwrap_or(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.4 });
                effects.push(json!({"type": "drop-shadow", "dx": sh.x, "dy": sh.y, "radius": sh.blur / 2.0, "color": rgba(&c)}));
            }
        }
        self.look_effects(l, idx, &tracks, &mut effects);
        if let Some((fid, mode)) = &l.style.fluid_matte {
            self.fluid_mattes.push((idx, effects.len(), fid.clone()));
            effects.push(json!({"type": "matte", "source": 0, "mode": mode}));
        }
        // content
        let mut extra_children: Vec<Value> = vec![];
        let own = self.base_paint.get(&l.id).cloned().unwrap_or_default();
        let mut content: Option<Value> = None;
        let mut projected = false;
        let mut path_color_tracks: Vec<(&str, Vec<(f64, Value, Ease)>)> = vec![];
        let mut text_tracks: Vec<Value> = vec![];
        match &l.kind {
            Kind::Fluid(cfg) => {
                if !own.is_empty() {
                    extra_children.push(json!({"name": format!("{}-box", l.id), "children": [], "content": self.paint_content(&own)}));
                }
                self.fluid_nodes.push(idx);
                content = Some(crate::fluid::ir_fluid(cfg, r));
            }
            Kind::Cloud(cfg) => {
                let mut c = crate::cloud::resolve(cfg, r, &mut self.cloud_fonts, &easing);
                if in3d || cfg.persp.is_some() {
                    c["projection"] = self.cloud_projection(l, cfg, r);
                    projected = true;
                }
                if !own.is_empty() {
                    extra_children.push(json!({"name": format!("{}-box", l.id), "children": [], "content": self.paint_content(&own)}));
                }
                self.cloud_nodes.push(idx);
                content = Some(c);
            }
            Kind::Image { src, fit } => {
                if !own.is_empty() {
                    extra_children.push(json!({"name": format!("{}-box", l.id), "children": [], "content": self.paint_content(&own)}));
                }
                match crate::lower::image_size(std::path::Path::new(src)) {
                    Some((pw, ph)) => {
                        let cr = self.nat.content_rect(&l.id).unwrap_or(r);
                        let (dw, dh) = match fit.as_str() {
                            "cover" => {
                                let s = (cr[2] / pw).max(cr[3] / ph);
                                (pw * s, ph * s)
                            }
                            "contain" => {
                                let s = (cr[2] / pw).min(cr[3] / ph);
                                (pw * s, ph * s)
                            }
                            _ => (cr[2], cr[3]),
                        };
                        let rect = [cr[0] + (cr[2] - dw) / 2.0, cr[1] + (cr[3] - dh) / 2.0, dw, dh];
                        let aspect_ok = ((dw / dh) - (pw / ph)).abs() / (pw / ph) < 0.01;
                        if aspect_ok && dw > 0.0 {
                            let s = pw / dw;
                            let a = self.asset(src, s);
                            let mut img = json!({"name": format!("{}-img", l.id), "children": [], "content": {"type": "image", "asset": a, "rect": [r4(rect[0]), r4(rect[1]), r4(rect[2]), r4(ph / s)]}});
                            if fit == "cover" && (dw > cr[2] + 0.5 || dh > cr[3] + 0.5) {
                                img["clips"] = json!([{"rect": [r4(cr[0]), r4(cr[1]), r4(cr[2]), r4(cr[3])], "radius": [l.style.radius, l.style.radius, l.style.radius, l.style.radius]}]);
                            }
                            extra_children.push(img);
                        } else if dw > 0.0 && dh > 0.0 {
                            // non-uniform `fill`: an image asset at natural size, scaled by a static transform
                            let s = pw / dw;
                            let a = self.asset(src, s);
                            let sy = (dh / dw) / (ph / pw);
                            extra_children.push(json!({"name": format!("{}-img", l.id), "children": [], "origin": [rect[0], rect[1]], "transform": [{"scale": [1.0, r4(sy)]}],
                                "content": {"type": "image", "asset": a, "rect": [r4(rect[0]), r4(rect[1]), r4(dw), r4(ph / s)]}}));
                        }
                    }
                    None => self.note("unsupported image"),
                }
            }
            Kind::Video { .. } => self.note("video layer (native decode planned, reel M6)"),
            Kind::Path { d, vb, .. } => {
                let dd = if let Some(ld) = self.link_d.get(&l.id) {
                    ld.clone()
                } else {
                    let sx = if vb.0 > 0.0 { r[2] / vb.0 } else { 1.0 };
                    let sy = if vb.1 > 0.0 { r[3] / vb.1 } else { 1.0 };
                    map_path(d, r[0], r[1], sx, sy)
                };
                if dd.trim().is_empty() {
                    self.note("connector without endpoints");
                } else {
                    let mut pc = json!({"type": "path", "d": dd, "trim": [base(Ch::TrimStart).unwrap_or(0.0).clamp(0.0, 1.0), base(Ch::TrimEnd).unwrap_or(1.0).clamp(0.0, 1.0)]});
                    if let Some((b, m)) = l.style.fill.clone().and_then(|p| self.raster_brush(&p, r)) {
                        pc["fill"] = serde_json::to_value(b).unwrap();
                        pc["fill_transform"] = json!(m);
                    } else if let Some(f) = l.style.fill.as_ref().and_then(|p| brush(p, r)) {
                        pc["fill"] = serde_json::to_value(f).unwrap();
                    }
                    if let Some(s) = &l.style.stroke {
                        // P10: reel draws a zero-length trim with round caps as a dot (Chrome draws nothing), so a
                        // path whose trim ever collapses (draw-on from 0) gets butt caps
                        let collapses = tracks.iter().any(|t| matches!(t.ch, Ch::TrimEnd | Ch::TrimStart) && (t.base.n() <= 1e-3 || t.keys.iter().any(|k| k.v.n() <= 1e-3)) && t.ch == Ch::TrimEnd)
                            || base(Ch::TrimEnd).map(|v| v <= 1e-3).unwrap_or(false);
                        let cap = if l.tags.iter().any(|t| t == "slice") || collapses { "butt" } else { "round" };
                        let sb = match l.style.stroke_paint.as_ref().and_then(|p| brush(p, r)) {
                            Some(b) => serde_json::to_value(b).unwrap(),
                            None => json!({"solid": rgba(s)}),
                        };
                        let mut st = json!({"width": l.style.stroke_w, "join": "round", "cap": cap, "brush": sb});
                        if let Some((a, b)) = l.style.dash {
                            st["dash"] = json!([a, b]);
                        }
                        pc["stroke"] = st;
                    }
                    if pc.get("fill").is_none() && pc.get("stroke").is_none() {
                        pc["stroke"] = json!({"width": 1, "brush": {"solid": [0, 0, 0, 0]}});
                    }
                    content = Some(pc);
                }
                // animated fill / stroke colors: reel-ir color tracks on the path's brushes
                for (ch, prop, slot) in [(Ch::Fill, "fill.color", "fill"), (Ch::Stroke, "stroke.color", "stroke")] {
                    let Some(t) = tracks.iter().find(|t| t.ch == ch && t.keys.len() > 1 && !t.is_static()) else { continue };
                    if content.as_ref().and_then(|c| c.get(slot)).is_none() {
                        continue;
                    }
                    if !t.keys.iter().all(|k| matches!(k.v, TV::C(_))) {
                        self.note(&format!("animated path {slot} (non-color keys)"));
                        continue;
                    }
                    let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| {
                        let TV::C(c) = &k.v else { unreachable!() };
                        (k.t, json!({"color": rgba(c)}), k.ease.clone())
                    }).collect();
                    path_color_tracks.push((prop, keys));
                }
            }
            _ => {
                if let Some(st) = self.states.remove(&l.id) {
                    *self.switched.entry(format!("{} channels on `{}`", if self.layout_off.contains_key(&l.id) { "layout" } else { "paint" }, l.name.clone().unwrap_or(l.id.clone()))).or_default() += st.states.len();
                    let rects: Vec<[f64; 4]> = st.states.iter().map(|s| ink(s, self.nat)).collect();
                    let u = union(&rects);
                    let states: Vec<Value> = st.states.iter().map(|s| json!({"type": "paint", "rect": u, "items": s})).collect();
                    content = Some(json!({"type": "switch", "rect": u, "frames": st.frames, "states": states}));
                } else if !own.is_empty() {
                    content = Some(self.paint_content(&own));
                }
                if matches!(l.kind, Kind::Text { .. }) {
                    self.text_v2(l, idx, &tracks, &mut content, &mut text_tracks);
                }
            }
        }
        if let Some(c) = content {
            node["content"] = c;
        }
        // reveal (wipe/iris) and paint masks -> matte with an animated hidden source
        let mut matte_src: Option<Value> = None;
        let mut matte_tracks: Vec<(Ch, Box<dyn Fn(f64) -> Value>)> = vec![];
        if let Some(clip) = &l.style.clip {
            let (shape, origin, f): (Value, [f64; 2], Box<dyn Fn(f64) -> Value>) = match clip {
                ClipShape::Polygon(p) => {
                    let d = p.iter().enumerate().map(|(i,p)| format!("{}{} {}", if i == 0 { "M" } else { "L" }, r[0]+p[0], r[1]+p[1])).collect::<Vec<_>>().join(" ") + " Z";
                    (json!({"type":"path", "d":d, "fill":{"solid":[1,1,1,1]}, "trim":[0,1]}), [r[0],r[1]], Box::new(|_| tf_scale(1.0,1.0)))
                },
                ClipShape::Wipe(d) => match d.as_str() {
                    "right" => (rect_c(r), [r[0] + r[2], r[1]], Box::new(|v| tf_scale(v, 1.0))),
                    "top" => (rect_c(r), [r[0], r[1]], Box::new(|v| tf_scale(1.0, v))),
                    "bottom" => (rect_c(r), [r[0], r[1] + r[3]], Box::new(|v| tf_scale(1.0, v))),
                    _ => {
                        let rr = [r[0] - 2.0, r[1] - 2.0, r[2] + 2.0, r[3] + 4.0];
                        let w = r[2];
                        (rect_c(rr), [rr[0], rr[1]], Box::new(move |v| tf_scale((v * w + 2.0) / (w + 2.0), 1.0)))
                    }
                },
                ClipShape::Iris(x, y) => {
                    let c = [r[0] + x * r[2], r[1] + y * r[3]];
                    let rad = 0.72 * ((r[2] * r[2] + r[3] * r[3]) / 2.0).sqrt();
                    let k = 0.5523 * rad;
                    let d = format!("M{} {} C{} {} {} {} {} {} C{} {} {} {} {} {} C{} {} {} {} {} {} C{} {} {} {} {} {} Z",
                        c[0] + rad, c[1], c[0] + rad, c[1] + k, c[0] + k, c[1] + rad, c[0], c[1] + rad,
                        c[0] - k, c[1] + rad, c[0] - rad, c[1] + k, c[0] - rad, c[1],
                        c[0] - rad, c[1] - k, c[0] - k, c[1] - rad, c[0], c[1] - rad,
                        c[0] + k, c[1] - rad, c[0] + rad, c[1] - k, c[0] + rad, c[1]);
                    (json!({"type": "path", "d": d, "fill": {"solid": [1, 1, 1, 1]}, "trim": [0, 1]}), c, Box::new(|v| tf_scale(v.max(1e-4), v.max(1e-4))))
                }
            };
            let v0 = base(Ch::Reveal).unwrap_or(1.0);
            matte_src = Some(json!({"name": format!("{}-reveal", l.id), "children": [], "hidden": true, "origin": origin, "content": shape, "transform": f(v0)["transform"].clone()}));
            matte_tracks.push((Ch::Reveal, f));
        } else if let Some(m) = &l.style.mask {
            let has_mx = tracks.iter().any(|t| t.ch == Ch::MaskX);
            let mr = if has_mx { [r[0], r[1], r[2] * 3.0, r[3]] } else { r };
            let rb = self.raster_brush(m, mr);
            match rb.map(|(b, t)| (b, Some(t))).or_else(|| brush(m, mr).map(|b| (b, None))) {
                Some((b, bt)) => {
                    let items = vec![Draw::Fill { shape: agentfx_ir::Shape::Rect([mr[0], mr[1], mr[0] + mr[2], mr[1] + mr[3]]), brush: b, transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], brush_transform: bt, even_odd: false }];
                    let w = r[2];
                    let f: Box<dyn Fn(f64) -> Value> = Box::new(move |v| json!({"transform": [{"translate": [r4(-2.0 * w * v), 0.0]}]}));
                    let v0 = base(Ch::MaskX).unwrap_or(0.0);
                    let mut src = json!({"name": format!("{}-mask", l.id), "children": [], "hidden": true, "origin": [mr[0], mr[1]], "content": {"type": "paint", "rect": mr, "items": items}});
                    if has_mx {
                        src["transform"] = f(v0)["transform"].clone();
                        matte_tracks.push((Ch::MaskX, f));
                    }
                    matte_src = Some(src);
                }
                None => self.note("image mask"),
            }
        }
        // unsupported channels
        for t in &tracks {
            if t.keys.len() > 1 && !t.is_static() {
                let ok = matches!(t.ch, Ch::Rgbsplit | Ch::RgbsplitDx | Ch::RgbsplitDy | Ch::Glitch | Ch::GlitchRate | Ch::GlitchBlock | Ch::Vhs | Ch::Dither | Ch::Ascii | Ch::Halftone | Ch::Sweep | Ch::Rays | Ch::Misreg | Ch::Opacity | Ch::Blur | Ch::Glow | Ch::Brightness | Ch::Saturate | Ch::Contrast | Ch::Hue | Ch::TimeSpan | Ch::FluidTime | Ch::TrimEnd | Ch::TrimStart | Ch::Reveal | Ch::MaskX) || TF.contains(&t.ch) || resolved_ch(t.ch)
                    || (t.ch == Ch::PathOffset && l.style.text_path.is_some());
                if !ok {
                    self.note(&format!("channel {} ({:?} stage)", t.ch.name(), t.ch.stage()));
                }
            }
        }
        // children
        let mut kids = vec![];
        let mut matte_idx = None;
        if let Some(ms) = matte_src {
            let i = self.nodes.len();
            self.nodes.push(ms);
            kids.push(i);
            matte_idx = Some(i);
            let mode = if l.style.mask_luma { "luma" } else { "alpha" };
            effects.push(json!({"type": "matte", "source": i, "mode": mode}));
        }
        if !effects.is_empty() {
            node["effects"] = json!(effects);
        }
        self.nodes[idx] = node;
        for x in extra_children {
            let i = self.nodes.len();
            self.nodes.push(x);
            kids.push(i);
        }
        let first_child = kids.len();
        for c in &l.children {
            kids.push(self.layer(c, in3d));
        }
        for (m, &k) in knockouts.iter().enumerate() {
            self.nodes[idx]["effects"][m]["source"] = json!(kids[first_child + k]);
        }
        // vignette overlay: its own element in the snapshot (`<id>~vig`), above the children
        let vig = format!("{}~vig", l.id);
        if self.nat.has(&vig) {
            if let Ok(items) = self.nat.paint(&vig, false) {
                let i = self.nodes.len();
                let c = self.paint_content(&items);
                self.nodes.push(json!({"name": vig, "children": [], "content": c}));
                kids.push(i);
            }
        }
        self.nodes[idx]["children"] = json!(kids);
        // tracks
        if let Some(t) = tracks.iter().find(|t| t.ch == Ch::Opacity) {
            let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| (k.t, json!({"number": r4(k.v.n().clamp(0.0, 1.0))}), k.ease.clone())).collect();
            if let Some(j) = self.track_json(idx, "opacity", &keys, "opacity") {
                self.tracks.push(j);
            }
        }
        if l.style.time_fx.is_some() {
            if let Some(t) = tracks.iter().find(|t| t.ch == Ch::TimeSpan) {
                let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| (k.t, json!({"number": r4(k.v.n())}), k.ease.clone())).collect();
                if let Some(j) = self.track_json(idx, "time.span", &keys, "time-span") {
                    self.tracks.push(j);
                }
                let v0 = t.keys.first().map(|k| k.v.n()).unwrap_or(t.base.n());
                self.nodes[idx]["time"]["span"] = json!(r4(v0));
            }
        }
        if matches!(l.kind, Kind::Fluid(_)) {
            if let Some(t) = tracks.iter().find(|t| t.ch == Ch::FluidTime && !crate::fluid::identity_time(t)) {
                let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| (k.t, json!({"number": r4(k.v.n())}), k.ease.clone())).collect();
                if let Some(j) = self.track_json(idx, "fluid.time", &keys, "fluid-time") {
                    self.tracks.push(j);
                }
            }
        }
        for (i, ch) in fx_tracks {
            if let Some(t) = tracks.iter().find(|t| t.ch == ch) {
                let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| (k.t, json!({"number": r4(k.v.n().max(0.0))}), k.ease.clone())).collect();
                if let Some(j) = self.track_json(idx, &format!("effects.{i}.radius"), &keys, &ch.name()) {
                    self.tracks.push(j);
                }
            }
        }
        if matches!(l.kind, Kind::Path { .. }) && self.nodes[idx].get("content").is_some() {
            for ch in [Ch::TrimEnd, Ch::TrimStart] {
                if let Some(t) = tracks.iter().find(|t| t.ch == ch) {
                    let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| (k.t, json!({"number": r4(k.v.n().clamp(0.0, 1.0))}), k.ease.clone())).collect();
                    if let Some(j) = self.track_json(idx, if ch == Ch::TrimEnd { "trim-end" } else { "trim-start" }, &keys, "trim") {
                        self.tracks.push(j);
                    }
                }
            }
        }
        for (prop, keys) in &path_color_tracks {
            if let Some(j) = self.track_json(idx, prop, keys, prop) {
                self.tracks.push(j);
            }
        }
        self.tracks.extend(text_tracks);
        if let Some(mi) = matte_idx {
            for (ch, f) in &matte_tracks {
                if let Some(t) = tracks.iter().find(|t| t.ch == *ch) {
                    let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| (k.t, f(k.v.n()), k.ease.clone())).collect();
                    if let Some(j) = self.track_json(mi, "transform", &keys, &ch.name()) {
                        self.tracks.push(j);
                    }
                }
            }
        }
        if !projected {
            self.transform_tracks(l, idx, &tracks, in3d);
        }
        idx
    }

    /// Per-frame 3x4 projection of a cloud's (x, y, z, 1) into its parent node's space: the
    /// full 4x4 chain (scene camera perspective, 3D rotations, the cloud's own transform, its
    /// own `persp=`) followed by the inverse of the parent's (linearized) reel-ir chain. Runs of
    /// equal matrices collapse to their ends.
    fn cloud_projection(&mut self, l: &Layer, cfg: &CloudCfg, r: [f64; 4]) -> Value {
        let fps = self.fps();
        let pid = self.parent.get(&l.id).cloned();
        let mut keys: Vec<(f64, [f64; 12])> = vec![];
        for f in 0..self.nf {
            let t = f as f64 / fps;
            let rects = |id: &str| self.rect.get(id).copied();
            let Some((pm, lm)) = motion::chain_m4(self.ir, &self.ch, &rects, &self.magic, t, &l.id) else { break };
            let mut m = motion::m4_mul(&pm, &lm);
            if let Some(d) = cfg.persp {
                let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
                let w = m[3][0] * cx + m[3][1] * cy + m[3][3];
                let (sx, sy) = ((m[0][0] * cx + m[0][1] * cy + m[0][3]) / w, (m[1][0] * cx + m[1][1] * cy + m[1][3]) / w);
                m = motion::m4_mul(&motion::persp_at(d, sx, sy), &m);
            }
            let w = self.world_at(t);
            let ap = pid.as_ref().and_then(|p| w.get(p)).map(|x| x.0).unwrap_or(motion::A2_ID);
            let inv = motion::a2_inv(&ap);
            let mut k = [0.0; 12];
            for j in 0..4 {
                let (x, y, ww) = (m[0][j], m[1][j], m[3][j]);
                k[j] = inv[0] * x + inv[2] * y + inv[4] * ww;
                k[4 + j] = inv[1] * x + inv[3] * y + inv[5] * ww;
                k[8 + j] = ww;
            }
            let k = k.map(|v| (v * 1e9).round() / 1e9);
            let n = keys.len();
            if n >= 2 && keys[n - 1].1 == k && keys[n - 2].1 == k {
                keys[n - 1].0 = t;
            } else {
                keys.push((t, k));
            }
        }
        if keys.len() >= 2 && keys.iter().all(|x| x.1 == keys[0].1) {
            keys.truncate(1);
        }
        json!(keys.iter().map(|(t, m)| json!({"t": (t * 1e6).round() / 1e6, "m": m})).collect::<Vec<_>>())
    }

    fn paint_content(&self, items: &[Draw]) -> Value {
        let b = ink(items, self.nat);
        json!({"type": "paint", "rect": [r4(b[0]), r4(b[1]), r4(b[2].max(1.0)), r4(b[3].max(1.0))], "items": items})
    }

    /// Local 2D transform value (relative to the parent) of a non-3D layer at channel values.
    fn tf_value(&self, l: &Layer, vals: &HashMap<Ch, f64>, layout: (f64, f64)) -> Value {
        let g = self.rect.get(&l.id).copied().unwrap_or([0.0; 4]);
        let v = |c: Ch| vals.get(&c).copied().unwrap_or_else(|| c.default_value().n());
        let k = 1.0 - v(Ch::Magic);
        let (mdx, mdy, msx, msy) = self.magic.get(&l.id).copied().unwrap_or((0.0, 0.0, 1.0, 1.0));
        let tx = v(Ch::X) + v(Ch::Px) * g[2] + mdx * k + layout.0;
        let ty = v(Ch::Y) + v(Ch::Py) * g[3] + mdy * k + layout.1;
        let sx = v(Ch::Scale) * v(Ch::Sx) * (1.0 + (msx - 1.0) * k);
        let sy = v(Ch::Scale) * v(Ch::Sy) * (1.0 + (msy - 1.0) * k);
        if l.tags.iter().any(|t| t == "cam-precise") {
            // camera director (X9): full precision (zooms of 1e4+ magnify any rounding)
            return json!({"transform": [{"translate": [tx, ty]}, {"rotate": r4(v(Ch::Rot))}, {"scale": [sx, sy]}]});
        }
        json!({"transform": [{"translate": [r4(tx), r4(ty)]}, {"rotate": r4(v(Ch::Rot))}, {"scale": [r4(sx), r4(sy)]}]})
    }

    /// Layout offset of a layer relative to its parent at frame f (hold between state frames).
    fn layout_local(&self, id: &str, f: u32) -> (f64, f64) {
        let at = |id: &str| -> (f64, f64) {
            match self.layout_off.get(id) {
                Some(v) => v.iter().rev().find(|x| x.0 <= f).map(|x| (x.1, x.2)).unwrap_or((0.0, 0.0)),
                None => (0.0, 0.0),
            }
        };
        let own = at(id);
        let par = self.parent.get(id).map(|p| at(p)).unwrap_or((0.0, 0.0));
        (own.0 - par.0, own.1 - par.1)
    }

    fn transform_tracks(&mut self, l: &Layer, idx: usize, tracks: &[&Track], in3d: bool) {
        let fps = self.fps();
        let tf: Vec<&Track> = tracks.iter().copied().filter(|t| TF.contains(&t.ch) && t.keys.len() > 1 && !t.is_static()).collect();
        let static_vals: HashMap<Ch, f64> = tracks.iter().filter(|t| TF.contains(&t.ch)).map(|t| (t.ch, t.keys.first().map(|k| k.v.n()).unwrap_or(t.base.n()))).collect();
        let has_layout = self.layout_off.contains_key(&l.id) || self.parent.get(&l.id).is_some_and(|p| self.layout_off.contains_key(p));
        if in3d {
            *self.approx.entry("2.5D camera / 3D rotation: projective transform linearized per frame at each layer center (reel camera proposed)".into()).or_default() += 1;
            self.sampled_3d(l, idx);
            return;
        }
        if tf.is_empty() && !has_layout {
            if !static_vals.is_empty() {
                let v = self.tf_value(l, &static_vals, (0.0, 0.0));
                self.nodes[idx]["transform"] = v["transform"].clone();
            }
        } else if tf.len() == 1 && !has_layout {
            let t = tf[0];
            let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|k| {
                let mut vals = static_vals.clone();
                vals.insert(t.ch, k.v.n());
                (k.t, self.tf_value(l, &vals, (0.0, 0.0)), k.ease.clone())
            }).collect();
            if let Some(j) = self.track_json(idx, "transform", &keys, &t.ch.name()) {
                self.tracks.push(j);
            }
        } else {
            // sampled per frame where any input changes (runs of equal values collapse)
            let mut fr: BTreeSet<u32> = BTreeSet::new();
            for t in &tf {
                let f0 = (t.keys[0].t * fps).floor().max(0.0) as u32;
                let f1 = ((t.keys[t.keys.len() - 1].t * fps).ceil() as u32 + 1).min(self.nf - 1);
                fr.extend(f0..=f1);
            }
            for id in [Some(l.id.clone()), self.parent.get(&l.id).cloned()].into_iter().flatten() {
                for (f, _, _) in self.layout_off.get(&id).into_iter().flatten() {
                    fr.insert(f.saturating_sub(1));
                    fr.insert(*f);
                }
            }
            fr.insert(0);
            let mut keys: Vec<(f64, Value, Ease)> = vec![];
            let frames: Vec<u32> = fr.into_iter().collect();
            for (i, &f) in frames.iter().enumerate() {
                let t = f as f64 / fps;
                let mut vals = static_vals.clone();
                for tr in tracks {
                    if TF.contains(&tr.ch) {
                        vals.insert(tr.ch, tr.eval(t).n());
                    }
                }
                let v = self.tf_value(l, &vals, self.layout_local(&l.id, f));
                // a gap in the sampled frames holds the value (layout states switch, not slide)
                let gap = i > 0 && f > frames[i - 1] + 1;
                let n = keys.len();
                if gap {
                    if let Some(last) = keys.last_mut() {
                        last.2 = Ease::Hold;
                    }
                }
                if n >= 2 && keys[n - 1].1 == v && keys[n - 2].1 == v && !gap {
                    keys[n - 1].0 = t;
                } else {
                    keys.push((t, v, Ease::Linear));
                }
            }
            if keys.len() >= 2 {
                if let Some(j) = self.track_json(idx, "transform", &keys, "transform (sampled)") {
                    self.tracks.push(j);
                }
            } else if let Some(k) = keys.first() {
                self.nodes[idx]["transform"] = k.1["transform"].clone();
            }
        }
    }

    /// 3D context: local affine = linearized world(parent)^-1 · linearized world(self), per frame.
    fn sampled_3d(&mut self, l: &Layer, idx: usize) {
        let fps = self.fps();
        let mut keys: Vec<(f64, Value, Ease)> = vec![];
        let pid = self.parent.get(&l.id).cloned();
        let mut last_v: Option<Value> = None;
        let n = self.nf;
        for f in 0..n {
            let t = f as f64 / fps;
            let w = self.world_at(t);
            let me = w.get(&l.id).map(|x| x.0).unwrap_or(motion::A2_ID);
            let pw = pid.as_ref().and_then(|p| w.get(p)).map(|x| x.0).unwrap_or(motion::A2_ID);
            let loc = motion::a2_mul(&motion::a2_inv(&pw), &me);
            let (tx, ty, r1, sx, sy, r2, _) = motion::decompose(&loc);
            let v = json!({"transform": [{"translate": [r4(tx), r4(ty)]}, {"rotate": r4(r1)}, {"scale": [r4(sx), r4(sy)]}, {"rotate": r4(r2)}]});
            if last_v.as_ref() == Some(&v) {
                let nk = keys.len();
                if nk >= 2 && keys[nk - 1].1 == v && keys[nk - 2].1 == v {
                    keys[nk - 1].0 = t;
                    continue;
                }
            }
            keys.push((t, v.clone(), Ease::Linear));
            last_v = Some(v);
        }
        if keys.iter().all(|k| k.1 == keys[0].1) {
            if let Some(k) = keys.first() {
                self.nodes[idx]["transform"] = k.1["transform"].clone();
            }
        } else if let Some(j) = self.track_json(idx, "transform", &keys, "3D (sampled, linearized)") {
            self.tracks.push(j);
        }
    }

    fn world_at(&mut self, t: f64) -> HashMap<String, (A2, f64)> {
        let key = (t * self.fps()).round() as i64;
        if let Some(w) = self.world_cache.get(&key) {
            return w.clone();
        }
        let rects = |id: &str| self.rect.get(id).copied();
        let w = motion::world(self.ir, &self.ch, &rects, &self.magic, t);
        self.world_cache.insert(key, w.clone());
        w
    }
}

fn key_of_base(em: &Em, id: &str, base: &Snap, rect: Option<[f64; 4]>) -> Vec<i64> {
    let mut k = vec![];
    let mut cur = Some(id.to_string());
    let mut own = true;
    while let Some(c) = cur {
        for tr in em.ch.by.get(c.as_str()).into_iter().flatten() {
            if resolved_ch(tr.ch) && (own || inherited_ch(tr.ch)) {
                k.push(tr.ch as i64);
                k.extend(quant(tr.ch, &base.vals.get(&(c.clone(), tr.ch)).cloned().unwrap_or(tr.ch.default_value())));
            }
        }
        own = false;
        cur = em.parent.get(&c).cloned();
    }
    if let Some(r) = rect {
        k.push((r[2] * 64.0).round() as i64);
        k.push((r[3] * 64.0).round() as i64);
    }
    k
}

fn rect_c(r: [f64; 4]) -> Value {
    json!({"type": "rect", "rect": {"rect": [r4(r[0]), r4(r[1]), r4(r[2].max(0.01)), r4(r[3].max(0.01))], "radius": [0, 0, 0, 0]}, "color": [1, 1, 1, 1]})
}
fn tf_scale(sx: f64, sy: f64) -> Value {
    json!({"transform": [{"scale": [r4(sx.max(1e-4)), r4(sy.max(1e-4))]}]})
}
fn union(rs: &[[f64; 4]]) -> [f64; 4] {
    let v: Vec<&[f64; 4]> = rs.iter().filter(|r| r[2] > 0.0 && r[3] > 0.0).collect();
    if v.is_empty() {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let x0 = v.iter().map(|r| r[0]).fold(f64::MAX, f64::min);
    let y0 = v.iter().map(|r| r[1]).fold(f64::MAX, f64::min);
    let x1 = v.iter().map(|r| r[0] + r[2]).fold(f64::MIN, f64::max);
    let y1 = v.iter().map(|r| r[1] + r[3]).fold(f64::MIN, f64::max);
    [r4(x0), r4(y0), r4(x1 - x0), r4(y1 - y0)]
}

/// Connector between two boxes (scene px), the HTML runtime's geometry: edge points with a
/// margin that shrinks when the boxes are close (the arrow never flips), arrowhead at the end.
pub fn link_path(ra: [f64; 4], rb: [f64; 4], curve: bool) -> String {
    let c = |r: [f64; 4], m: f64| [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0, r[2] / 2.0 + m, r[3] / 2.0 + m];
    let edge = |p: [f64; 4], q: [f64; 4]| -> [f64; 2] {
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        let k = (if dx.abs() > 1e-6 { p[2] / dx.abs() } else { 1e9 }).min(if dy.abs() > 1e-6 { p[3] / dy.abs() } else { 1e9 });
        [p[0] + dx * k, p[1] + dy * k]
    };
    let (mut s, mut e) = ([0.0; 2], [0.0; 2]);
    for m in [14.0, 6.0, 0.0] {
        let (a, b) = (c(ra, m), c(rb, m));
        s = edge(a, b);
        e = edge(b, a);
        if (e[0] - s[0]) * (b[0] - a[0]) + (e[1] - s[1]) * (b[1] - a[1]) > 0.0 {
            break;
        }
    }
    let mut d = if curve {
        let mx = (s[0] + e[0]) / 2.0;
        format!("M{:.2} {:.2} C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2}", s[0], s[1], mx, s[1], mx, e[1], e[0], e[1])
    } else {
        format!("M{:.2} {:.2} L{:.2} {:.2}", s[0], s[1], e[0], e[1])
    };
    let ang = if curve { (e[1] - e[1]).atan2(e[0] - (s[0] + e[0]) / 2.0) } else { (e[1] - s[1]).atan2(e[0] - s[0]) };
    let (al, aw) = (18.0, 0.5);
    d += &format!(" M{:.2} {:.2} L{:.2} {:.2} L{:.2} {:.2}", e[0] - al * (ang - aw).cos(), e[1] - al * (ang - aw).sin(), e[0], e[1], e[0] - al * (ang + aw).cos(), e[1] - al * (ang + aw).sin());
    d
}

/// Emit reel-ir v1 for `ir`, laid out and painted by `nat` (loaded with the t=0 snapshot).
pub fn emit(ir: &Ir, nat: &mut Native) -> Out {
    let t0 = std::time::Instant::now();
    let nf = (ir.duration * ir.fps).round().max(1.0) as u32;
    let mut parent = HashMap::new();
    fn par(l: &Layer, p: &mut HashMap<String, String>) {
        for c in &l.children {
            p.insert(c.id.clone(), l.id.clone());
            par(c, p);
        }
    }
    par(&ir.root, &mut parent);
    let mut em = Em {
        ir,
        nat,
        ch: Chans::new(ir),
        nodes: vec![],
        tracks: vec![],
        assets: vec![],
        unsupported: BTreeMap::new(),
        approx: BTreeMap::new(),
        switched: BTreeMap::new(),
        rect: HashMap::new(),
        magic: HashMap::new(),
        link_d: HashMap::new(),
        base_paint: HashMap::new(),
        states: HashMap::new(),
        layout_off: HashMap::new(),
        three_d: HashSet::new(),
        resized: HashSet::new(),
        paintable: HashSet::new(),
        parent,
        nf,
        stats: serde_json::Map::new(),
        world_cache: HashMap::new(),
        cloud_fonts: vec![],
        cloud_nodes: vec![],
        layer_node: HashMap::new(),
        time_maps: vec![],
        fluid_nodes: vec![],
        fluid_mattes: vec![],
    };
    em.hooks();
    // base paint of every layer (own content)
    let tp = std::time::Instant::now();
    let mut ids: Vec<(String, bool, bool)> = vec![];
    ir.root.walk(&mut |l| ids.push((l.id.clone(), matches!(l.kind, Kind::Image { .. }), matches!(l.kind, Kind::Path { .. } | Kind::Video { .. }))));
    let mut paint_errors = vec![];
    for (id, img, skip) in &ids {
        if *skip {
            continue;
        }
        em.paintable.insert(id.clone());
        match em.nat.paint(id, *img) {
            Ok(mut items) => {
                if let Some((p, r)) = em.glyph_fill(id) {
                    let rb = em.raster_brush(&p, r);
                    text_fill(&mut items, &p, r, rb);
                }
                if let Some(st) = em.ir_layer(id).and_then(crate::textv2::stroke_of) {
                    crate::textv2::apply_stroke(&mut items, &st);
                }
                if !items.is_empty() {
                    em.base_paint.insert(id.clone(), items);
                }
            }
            Err(e) => paint_errors.push(format!("{e:#}")),
        }
    }
    em.stats.insert("paint_s".into(), json!(tp.elapsed().as_secs_f64()));
    em.resolve_states();
    let tn = std::time::Instant::now();
    em.layer(&ir.root, false);
    em.stats.insert("nodes_s".into(), json!(tn.elapsed().as_secs_f64()));
    let mut diags = vec![];
    for e in paint_errors {
        diags.push(Diag::warn("W_IR_UNSUPPORTED", format!("native paint: {e}"), "the HTML backend renders it; simplify the raw markup", Default::default()));
    }
    for (k, n) in &em.unsupported {
        diags.push(Diag::warn("W_IR_UNSUPPORTED", format!("reel-ir v1 cannot express: {k} ({n}x); rendered without it"), "see IR-PROPOSALS.md; the HTML backend renders it", Default::default()));
    }
    for (k, n) in &em.approx {
        diags.push(Diag::warn("W_IR_APPROX", format!("{k} ({n} layers)"), "see IR-PROPOSALS.md (camera)", Default::default()));
    }
    for (k, n) in &em.switched {
        diags.push(Diag::info("W_IR_SWITCH", format!("{k}: resolved by the compiler into {n} repaint states (switch)"), "fine; a reel-ir track would be cheaper (IR-PROPOSALS.md)", Default::default()));
    }
    if ir.audio.iter().any(|a| a.kind == "voice") {
        diags.push(Diag::info("I_IR_AUDIO", "reel-ir has no audio; `agentfx render` mixes and muxes it after the video", "", Default::default()));
    }
    // fonts registered by the recorder (in the order its display lists reference them), then
    // the cloud text forms' fonts
    let mut fonts: Vec<Value> = em.nat.res.fonts.iter().map(|f| json!({"path": f.path, "index": f.index})).collect();
    let font_off = fonts.len();
    fonts.extend(em.cloud_fonts.iter().map(|p| json!({"path": p, "index": 0})));
    let mut assets = em.assets.clone();
    let off = assets.len();
    for a in &em.nat.res.assets {
        assets.push(json!({"path": a.path, "scale": a.scale}));
    }
    // recorder image assets are referenced by brush index: shift them behind ours
    for (i, id) in std::mem::take(&mut em.time_maps) {
        match em.layer_node.get(&id) {
            Some(&m) => em.nodes[i]["time"]["map"]["node"] = json!(m),
            None => {
                em.nodes[i]["time"]["map"] = json!({"type": "radial", "center": [ir.w as f64 / 2.0, ir.h as f64 / 2.0], "radius": ir.w as f64 / 2.0});
                diags.push(Diag::warn("W_IR_UNSUPPORTED", format!("timedisplace map=#{id}: layer not emitted; radial map used"), "use a visible element of the same scene as the map", Default::default()));
            }
        }
    }
    for (i, k, id) in std::mem::take(&mut em.fluid_mattes) {
        match em.layer_node.get(&id) {
            Some(&m) => em.nodes[i]["effects"][k]["source"] = json!(m),
            None => {
                if let Some(a) = em.nodes[i]["effects"].as_array_mut() {
                    a[k] = json!({"type": "blur", "radius": 0.0});
                }
                diags.push(Diag::warn("W_IR_UNSUPPORTED", format!("reveal with #{id}: fluid not emitted; the layer is shown plainly"), "reveal with a fluid of the same scene", Default::default()));
            }
        }
    }
    for i in std::mem::take(&mut em.fluid_nodes) {
        let ems = em.nodes[i]["content"]["emitters"].as_array().cloned().unwrap_or_default();
        let mut out = vec![];
        for mut e in ems {
            if e["type"] == "source" {
                let id = e["node"].as_str().unwrap_or_default().to_string();
                match em.layer_node.get(&id) {
                    Some(&m) => e["node"] = json!(m),
                    None => {
                        diags.push(Diag::warn("W_IR_UNSUPPORTED", format!("ink #{id}: layer not emitted; dropped"), "ink a visible element of the same scene", Default::default()));
                        continue;
                    }
                }
            }
            out.push(e);
        }
        em.nodes[i]["content"]["emitters"] = json!(out);
    }
    let mut nodes = em.nodes;
    for &i in &em.cloud_nodes {
        if let Some(forms) = nodes[i]["content"]["forms"].as_array_mut() {
            for f in forms {
                if f["shape"]["type"] == "text" {
                    let fi = f["shape"]["font"].as_u64().unwrap_or(0) as usize;
                    f["shape"]["font"] = json!(fi + font_off);
                }
            }
        }
    }
    if off > 0 && !em.nat.res.assets.is_empty() {
        for n in nodes.iter_mut() {
            shift_brush_assets(&mut n["content"], off);
        }
    }
    em.stats.insert("emit_s".into(), json!(t0.elapsed().as_secs_f64()));
    em.stats.insert("nodes".into(), json!(nodes.len()));
    em.stats.insert("tracks".into(), json!(em.tracks.len()));
    let scene = json!({
        "version": 1, "width": ir.w, "height": ir.h, "fps": ir.fps.round() as u32, "frames": nf,
        "nodes": nodes, "tracks": em.tracks, "assets": assets, "fonts": fonts,
    });
    Out { scene, diags, stats: Value::Object(em.stats) }
}

fn shift_brush_assets(v: &mut Value, off: usize) {
    match v {
        Value::Object(m) => {
            if let Some(Value::Object(img)) = m.get_mut("image") {
                if let Some(a) = img.get("asset").and_then(|a| a.as_u64()) {
                    img.insert("asset".into(), json!(a as usize + off));
                }
            }
            for (_, x) in m.iter_mut() {
                shift_brush_assets(x, off);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| shift_brush_assets(x, off)),
        _ => {}
    }
}

// ---------------------------------------------------------------- looks and light (look.rs)
impl<'a> Em<'a> {
    /// reel-ir effects (and their tracks) for a layer's print looks and light effects.
    fn look_effects(&mut self, l: &Layer, idx: usize, tracks: &[&Track], effects: &mut Vec<Value>) {
        use crate::look::Look;
        let mut looks = l.style.looks.clone();
        let has = |ch: Ch| tracks.iter().any(|t| t.ch == ch && t.keys.len() > 1);
        // an animated look channel without the look: the look with defaults
        if has(Ch::Halftone) && !looks.iter().any(|x| matches!(x, Look::Halftone { .. })) {
            let cell = tracks.iter().find(|t| t.ch == Ch::Halftone).map(|t| t.base.n()).unwrap_or(8.0);
            looks.push(Look::Halftone { cell, angle: 45.0, cmyk: false, ink: Rgba { r: 0.08, g: 0.08, b: 0.1, a: 1.0 }, paper: None });
        }
        looks.sort_by_key(|x| x.order());
        let r = self.rect.get(&l.id).copied().unwrap_or([0.0, 0.0, self.ir.w as f64, self.ir.h as f64]);
        let num_track = |em: &mut Em, ch: Ch, k: usize, field: &str| {
            if let Some(t) = tracks.iter().find(|t| t.ch == ch) {
                let keys: Vec<(f64, Value, Ease)> = t.keys.iter().map(|kk| (kk.t, json!({"number": r4(kk.v.n())}), kk.ease.clone())).collect();
                if let Some(j) = em.track_json(idx, &format!("effects.{k}.{field}"), &keys, &ch.name()) {
                    em.tracks.push(j);
                }
            }
        };
        for lk in looks {
            let k = effects.len();
            match lk {
                Look::Rgbsplit { dx, dy, amount, mode } => {
                    effects.push(json!({"type": "rgbsplit", "dx": dx, "dy": dy, "amount": amount, "mode": mode}));
                    for (ch, field) in [(Ch::Rgbsplit, "amount"), (Ch::RgbsplitDx, "dx"), (Ch::RgbsplitDy, "dy")] { num_track(self, ch, k, field); }
                }
                Look::Glitch { amount, seed, rate, block } => {
                    effects.push(json!({"type": "glitch", "amount": amount, "seed": seed, "rate": rate, "block": block}));
                    for (ch, field) in [(Ch::Glitch, "amount"), (Ch::GlitchRate, "rate"), (Ch::GlitchBlock, "block")] { num_track(self, ch, k, field); }
                }
                Look::Vhs { amount, seed } => {
                    effects.push(json!({"type": "vhs", "amount": amount, "seed": seed}));
                    num_track(self, Ch::Vhs, k, "amount");
                }
                Look::Dither { levels, mode, size, palette } => {
                    effects.push(json!({"type": "dither", "levels": levels, "mode": mode, "size": size,
                        "palette": palette.map(|p| p.iter().map(rgba).collect::<Vec<_>>())}));
                    num_track(self, Ch::Dither, k, "levels");
                }
                Look::Ascii { cell, charset, color, bg } => {
                    effects.push(json!({"type": "ascii", "cell": cell, "charset": charset, "color": color, "bg": rgba(&bg)}));
                    num_track(self, Ch::Ascii, k, "cell");
                }
                Look::Riso { inks, seed, misreg, drift, speed, grain, density, paper } => {
                    let mut e = json!({"type": "riso", "inks": inks.iter().map(rgba).collect::<Vec<_>>(), "seed": seed, "misreg": misreg, "drift": drift, "drift_speed": speed, "grain": grain, "density": density});
                    if let Some(p) = paper {
                        e["paper"] = rgba(&p);
                    }
                    effects.push(e);
                    num_track(self, Ch::Misreg, k, "misreg");
                }
                Look::Halftone { cell, angle, cmyk, ink, paper } => {
                    let mut e = json!({"type": "halftone", "cell": cell, "angle": angle, "mode": if cmyk { "cmyk" } else { "mono" }, "ink": rgba(&ink)});
                    if let Some(p) = paper {
                        e["paper"] = rgba(&p);
                    }
                    effects.push(e);
                    num_track(self, Ch::Halftone, k, "cell");
                }
                Look::Grain { amount, seed, live, size } => effects.push(json!({"type": "grain", "amount": amount, "seed": seed, "live": live, "size": size})),
                Look::Paper { texture, bleed, rough, seed, color } => effects.push(json!({"type": "paper", "texture": texture, "bleed": bleed, "rough": rough, "seed": seed, "color": rgba(&color)})),
                Look::Sweep { angle, width, width_pct, color, strength } => {
                    let w = if width_pct { r[2] * width / 100.0 } else { width };
                    effects.push(json!({"type": "sweep", "pos": 0.0, "angle": angle, "width": r4(w.max(1.0)), "color": rgba(&color), "strength": strength}));
                    num_track(self, Ch::Sweep, k, "pos");
                }
                Look::Shadow { light, height, opacity, color, .. } => {
                    let Some((lid, cfg)) = self.find_light(&l.id, &light) else { continue };
                    let (x0, y0) = self.light_track(&lid, idx, k);
                    effects.push(json!({"type": "shadow", "light": [x0, y0, cfg.z], "light_radius": cfg.radius, "height": height, "color": rgba(&color), "strength": opacity}));
                }
                Look::Rays { light, length, strength, color, .. } => {
                    let Some((lid, _)) = self.find_light(&l.id, &light) else { continue };
                    let (x0, y0) = self.light_track(&lid, idx, k);
                    effects.push(json!({"type": "rays", "light": [x0, y0], "length": length, "color": rgba(&color), "strength": strength}));
                    num_track(self, Ch::Rays, k, "strength");
                }
            }
        }
    }

    /// The `light` layer named `name` in the scene of layer `from`.
    fn find_light(&self, from: &str, name: &str) -> Option<(String, crate::look::LightCfg)> {
        fn find<'b>(l: &'b Layer, id: &str) -> Option<&'b Layer> {
            if l.id == id {
                return Some(l);
            }
            l.children.iter().find_map(|c| find(c, id))
        }
        fn light<'b>(l: &'b Layer, name: &str) -> Option<&'b Layer> {
            if l.style.light.is_some() && l.name.as_deref() == Some(name) {
                return Some(l);
            }
            l.children.iter().find_map(|c| light(c, name))
        }
        let mut cur = from.to_string();
        let mut scene = None;
        loop {
            if let Some(x) = find(&self.ir.root, &cur) {
                if x.tags.iter().any(|t| t == "scene") {
                    scene = Some(x);
                    break;
                }
            }
            match self.parent.get(&cur) {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
        let l = light(scene.unwrap_or(&self.ir.root), name)?;
        Some((l.id.clone(), l.style.light?))
    }

    /// Bake the light's world position (its box center, every output frame) into tracks
    /// `effects.<k>.lx/ly` of node `idx` (runs of equal values collapse); returns the
    /// position at frame 0.
    fn light_track(&mut self, light_id: &str, idx: usize, k: usize) -> (f64, f64) {
        let fps = self.fps();
        let r = self.rect.get(light_id).copied().unwrap_or([0.0; 4]);
        let mut pts: Vec<(f64, f64)> = Vec::with_capacity(self.nf as usize);
        for f in 0..self.nf.max(1) {
            let w = self.world_at(f as f64 / fps);
            let m = w.get(light_id).map(|x| x.0).unwrap_or(motion::A2_ID);
            let (x, y) = motion::a2_apply(&m, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
            pts.push((r4(x), r4(y)));
        }
        let p0 = pts[0];
        if pts.iter().all(|p| *p == p0) {
            return p0;
        }
        for (field, sel) in [("lx", 0usize), ("ly", 1usize)] {
            let v = |i: usize| if sel == 0 { pts[i].0 } else { pts[i].1 };
            let n = pts.len();
            let mut keys: Vec<(f64, Value, Ease)> = vec![];
            for i in 0..n {
                let keep = i == 0 || i == n - 1 || v(i) != v(i - 1) || v(i) != v(i + 1);
                if keep {
                    keys.push((i as f64 / fps, json!({"number": v(i)}), Ease::Linear));
                }
            }
            if let Some(j) = self.track_json(idx, &format!("effects.{k}.{field}"), &keys, "light") {
                self.tracks.push(j);
            }
        }
        p0
    }
}
