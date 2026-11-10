//! SVG import as native parts (P5): `svg #logo src=logo.svg parts`. The file is parsed and
//! normalised by usvg (CSS, `use`, transforms, units, shapes -> paths; already in the tree via
//! Blitz) and flattened into vector parts: every path becomes a reel path layer (crisp at any
//! scale, trim/draw-on, fill/stroke colour tracks, morph), every `<g id=...>` a group layer, so
//! verbs address them as `#logo.<svg id>` (unnamed paths: `#logo.p1`, `#logo.p2`, ... in
//! document order). Geometry stays in SVG user units here; `elements.rs` maps it into the box.
use crate::values::{ColorRef, Paint, Rgba, Stop};
use std::path::Path;
use usvg::tiny_skia_path::{PathSegment, Transform};

#[derive(Debug, Clone)]
pub struct Part {
    /// svg id (sanitised) or the generated `p<k>` for unnamed paths; None: anonymous group
    pub name: Option<String>,
    /// `[x, y, w, h]` in SVG user units (the root size space)
    pub rect: [f64; 4],
    pub opacity: f64,
    pub kind: PartKind,
    pub children: Vec<Part>,
}

#[derive(Debug, Clone)]
pub enum PartKind {
    Group,
    Path(PathPart),
}

#[derive(Debug, Clone)]
pub struct PathPart {
    /// geometry in absolute user units (transforms applied); `PathPart::d` writes path data
    pub data: usvg::tiny_skia_path::Path,
    pub fill: Option<Paint>,
    pub even_odd: bool,
    pub stroke: Option<StrokePart>,
}

#[derive(Debug, Clone)]
pub struct StrokePart {
    /// solid color (gradients: the first stop)
    pub color: Rgba,
    /// gradient stroke paint
    pub paint: Option<Paint>,
    /// width in user units (already scaled by the path's transform)
    pub width: f64,
    pub cap: &'static str,
    pub join: &'static str,
    pub dash: Option<(f64, f64)>,
}

impl PathPart {
    /// SVG path data relative to `origin` (user units), scaled by `k` (user unit -> px).
    pub fn d(&self, origin: (f64, f64), k: f64) -> String {
        path_d(&self.data, origin.0, origin.1, k)
    }
}

#[derive(Debug, Default)]
pub struct Imported {
    /// root size (width/height or viewBox), user units
    pub size: (f64, f64),
    pub parts: Vec<Part>,
    /// (what, hint): unsupported features that were dropped or approximated
    pub warnings: Vec<(String, String)>,
}

impl Imported {
    /// All addressable part names, document order.
    pub fn names(&self) -> Vec<String> {
        fn walk(p: &[Part], out: &mut Vec<String>) {
            for x in p {
                if let Some(n) = &x.name {
                    out.push(n.clone());
                }
                walk(&x.children, out);
            }
        }
        let mut v = vec![];
        walk(&self.parts, &mut v);
        v
    }
}

/// Ids as reel ids: `[A-Za-z0-9_-]`, everything else becomes `-` (dots would split `#a.b`).
pub fn sanitize(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect()
}

pub fn import(svg: &str, resources: &Path) -> Result<Imported, String> {
    let opt = usvg::Options { resources_dir: Some(resources.to_path_buf()), ..Default::default() };
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| format!("{e}"))?;
    let size = (tree.size().width() as f64, tree.size().height() as f64);
    let mut st = State { warnings: vec![], used: Default::default(), next: 0 };
    collect_ids(tree.root(), &mut st.used);
    let mut parts = vec![];
    walk(tree.root(), &mut parts, &mut st);
    Ok(Imported { size, parts, warnings: st.warnings })
}

struct State {
    warnings: Vec<(String, String)>,
    used: std::collections::HashSet<String>,
    next: usize,
}
impl State {
    fn warn(&mut self, what: String, hint: &str) {
        if !self.warnings.iter().any(|w| w.0 == what) {
            self.warnings.push((what, hint.to_string()));
        }
    }
    fn auto_name(&mut self) -> String {
        loop {
            self.next += 1;
            let n = format!("p{}", self.next);
            if !self.used.contains(&n) {
                return n;
            }
        }
    }
}

fn collect_ids(g: &usvg::Group, out: &mut std::collections::HashSet<String>) {
    for c in g.children() {
        if !c.id().is_empty() {
            out.insert(sanitize(c.id()));
        }
        if let usvg::Node::Group(g) = c {
            collect_ids(g, out);
        }
    }
}

fn union(a: Option<[f64; 4]>, b: [f64; 4]) -> [f64; 4] {
    match a {
        None => b,
        Some(a) => {
            let (x0, y0) = (a[0].min(b[0]), a[1].min(b[1]));
            let (x1, y1) = ((a[0] + a[2]).max(b[0] + b[2]), (a[1] + a[3]).max(b[1] + b[3]));
            [x0, y0, x1 - x0, y1 - y0]
        }
    }
}

fn walk(g: &usvg::Group, out: &mut Vec<Part>, st: &mut State) {
    for c in g.children() {
        match c {
            usvg::Node::Group(g) => {
                let label = if g.id().is_empty() { "a group".to_string() } else { format!("`{}`", g.id()) };
                if g.clip_path().is_some() {
                    st.warn(format!("clip-path on {label} is ignored"), "parts draw without clips; use the raw `svg` (no `parts`) for exact clipping, or expand the clip in your editor");
                }
                if g.mask().is_some() {
                    st.warn(format!("mask on {label} is ignored"), "use the raw `svg` (no `parts`) for masks");
                }
                if !g.filters().is_empty() {
                    st.warn(format!("filter on {label} is ignored"), "use reel effects instead (`glow=`, `blur=`, `shadow=`)");
                }
                let opacity = g.opacity().get() as f64;
                let named = !g.id().is_empty();
                if named || opacity < 0.999 {
                    let mut kids = vec![];
                    walk(g, &mut kids, st);
                    let Some(rect) = kids.iter().fold(None, |a, k| Some(union(a, k.rect))) else { continue };
                    out.push(Part { name: named.then(|| sanitize(g.id())), rect, opacity, kind: PartKind::Group, children: kids });
                } else {
                    walk(g, out, st);
                }
            }
            usvg::Node::Path(p) => {
                if !p.is_visible() {
                    continue;
                }
                if let Some(part) = path_part(p, st) {
                    out.push(part);
                }
            }
            usvg::Node::Image(i) => {
                let label = if i.id().is_empty() { String::new() } else { format!(" `{}`", i.id()) };
                st.warn(format!("embedded image{label} is not imported"), "parts are vectors only; place bitmaps with `image src=`");
            }
            usvg::Node::Text(t) => {
                // usvg converts text to outlines when it finds the font; use them when present
                let fl = t.flattened();
                if fl.children().is_empty() {
                    st.warn("text is not imported (font not available)".into(), "convert text to outlines in your editor (Illustrator: Create Outlines, Figma: Outline stroke / Flatten)");
                } else {
                    walk(fl, out, st);
                }
            }
        }
    }
}

fn path_part(p: &usvg::Path, st: &mut State) -> Option<Part> {
    let ts = p.abs_transform();
    let data = p.data().clone().transform(ts)?;
    let b = data.compute_tight_bounds().unwrap_or(data.bounds());
    // zero-extent boxes (straight lines) keep a tiny extent so the box maps 1:1
    let eps = 1e-3;
    let rect = [b.x() as f64, b.y() as f64, (b.width() as f64).max(eps), (b.height() as f64).max(eps)];
    if data.segments().next().is_none() {
        return None;
    }
    let fill = p.fill().and_then(|f| {
        let paint = paint_of(f.paint(), f.opacity().get() as f64, ts, rect, st)?;
        Some((paint, f.rule() == usvg::FillRule::EvenOdd))
    });
    let scale = ((ts.sx * ts.sy - ts.kx * ts.ky).abs() as f64).sqrt();
    let stroke = p.stroke().and_then(|s| {
        let paint = paint_of(s.paint(), s.opacity().get() as f64, ts, rect, st)?;
        let color = match &paint {
            Paint::Solid(ColorRef::Lit(c)) => c.clone(),
            Paint::Linear { stops, .. } | Paint::Radial { stops, .. } => match stops.first().map(|s| &s.color) {
                Some(ColorRef::Lit(c)) => c.clone(),
                _ => Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
            },
            _ => Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
        };
        let dash = s.dasharray().filter(|d| d.len() >= 2).map(|d| (d[0] as f64 * scale, d[1] as f64 * scale));
        if s.dasharray().is_some_and(|d| d.len() > 2) {
            st.warn("dash patterns with more than two values use the first two".into(), "reel dashes are `dash=\"on off\"`");
        }
        Some(StrokePart {
            paint: (!matches!(paint, Paint::Solid(_))).then_some(paint),
            color,
            width: s.width().get() as f64 * scale,
            cap: match s.linecap() {
                usvg::LineCap::Butt => "butt",
                usvg::LineCap::Round => "round",
                usvg::LineCap::Square => "square",
            },
            join: match s.linejoin() {
                usvg::LineJoin::Round => "round",
                usvg::LineJoin::Bevel => "bevel",
                _ => "miter",
            },
            dash,
        })
    });
    if fill.is_none() && stroke.is_none() {
        return None;
    }
    let name = if p.id().is_empty() { st.auto_name() } else { sanitize(p.id()) };
    let (fill, even_odd) = match fill {
        Some((f, eo)) => (Some(f), eo),
        None => (None, false),
    };
    Some(Part { name: Some(name), rect, opacity: 1.0, kind: PartKind::Path(PathPart { data, fill, even_odd, stroke }), children: vec![] })
}

fn n(v: f32, o: f64, k: f64) -> String {
    let x = ((v as f64 - o) * k * 1000.0).round() / 1000.0;
    let s = format!("{x}");
    if s == "-0" { "0".into() } else { s }
}

/// SVG path data of a (transformed) path, translated by `-(ox, oy)` and scaled by `k`.
fn path_d(p: &usvg::tiny_skia_path::Path, ox: f64, oy: f64, k: f64) -> String {
    let mut d = String::new();
    let xy = |a: usvg::tiny_skia_path::Point| format!("{} {}", n(a.x, ox, k), n(a.y, oy, k));
    for seg in p.segments() {
        match seg {
            PathSegment::MoveTo(a) => d.push_str(&format!("M{} ", xy(a))),
            PathSegment::LineTo(a) => d.push_str(&format!("L{} ", xy(a))),
            PathSegment::QuadTo(a, b) => d.push_str(&format!("Q{} {} ", xy(a), xy(b))),
            PathSegment::CubicTo(a, b, c) => d.push_str(&format!("C{} {} {} ", xy(a), xy(b), xy(c))),
            PathSegment::Close => d.push_str("Z "),
        }
    }
    d.trim_end().to_string()
}

fn rgba(c: usvg::Color, a: f64) -> Rgba {
    Rgba { r: c.red as f64 / 255.0, g: c.green as f64 / 255.0, b: c.blue as f64 / 255.0, a: a.clamp(0.0, 1.0) }
}

fn stops(s: &[usvg::Stop], opacity: f64, map: impl Fn(f64) -> f64) -> Vec<Stop> {
    s.iter().map(|x| Stop { color: ColorRef::Lit(rgba(x.color(), x.opacity().get() as f64 * opacity)), pos: Some(map(x.offset().get() as f64)) }).collect()
}

/// A usvg paint as a reel paint over the path's box `rect` (absolute user units). Linear
/// gradients are exact (the stop positions are re-projected onto the CSS gradient line of the
/// box); radial ones keep center and radius (focal point and spread modes are dropped).
fn paint_of(p: &usvg::Paint, opacity: f64, ts: Transform, rect: [f64; 4], st: &mut State) -> Option<Paint> {
    match p {
        usvg::Paint::Color(c) => Some(Paint::Solid(ColorRef::Lit(rgba(*c, opacity)))),
        usvg::Paint::LinearGradient(g) => {
            if g.stops().is_empty() {
                return None;
            }
            let m = ts.pre_concat(g.transform());
            let map = |x: f32, y: f32| {
                let mut pt = usvg::tiny_skia_path::Point::from_xy(x, y);
                m.map_point(&mut pt);
                (pt.x as f64, pt.y as f64)
            };
            let (p0, p1) = (map(g.x1(), g.y1()), map(g.x2(), g.y2()));
            let (vx, vy) = (p1.0 - p0.0, p1.1 - p0.1);
            let len = (vx * vx + vy * vy).sqrt();
            if len < 1e-9 {
                let c = g.stops().last().unwrap();
                return Some(Paint::Solid(ColorRef::Lit(rgba(c.color(), c.opacity().get() as f64 * opacity))));
            }
            let (dx, dy) = (vx / len, vy / len);
            // CSS angle: 0deg points up, 90deg right; direction = (sin a, -cos a)
            let angle = dx.atan2(-dy).to_degrees();
            let l = (rect[2] * dx).abs() + (rect[3] * dy).abs();
            let (cx, cy) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
            let s0 = (cx - dx * l / 2.0, cy - dy * l / 2.0);
            let at = |p: (f64, f64)| ((p.0 - s0.0) * dx + (p.1 - s0.1) * dy) / l.max(1e-9);
            let (t0, t1) = (at(p0), at(p1));
            if g.spread_method() != usvg::SpreadMethod::Pad {
                st.warn("gradient spreadMethod reflect/repeat is drawn as pad".into(), "extend the gradient stops in your editor");
            }
            Some(Paint::Linear { angle_deg: (angle * 100.0).round() / 100.0, stops: stops(g.stops(), opacity, |o| t0 + o * (t1 - t0)) })
        }
        usvg::Paint::RadialGradient(g) => {
            if g.stops().is_empty() {
                return None;
            }
            let m = ts.pre_concat(g.transform());
            let mut c = usvg::tiny_skia_path::Point::from_xy(g.cx(), g.cy());
            m.map_point(&mut c);
            let r = g.r().get() as f64 * ((m.sx * m.sy - m.kx * m.ky).abs() as f64).sqrt();
            let (ccx, ccy) = (c.x as f64, c.y as f64);
            let far = [(rect[0], rect[1]), (rect[0] + rect[2], rect[1]), (rect[0], rect[1] + rect[3]), (rect[0] + rect[2], rect[1] + rect[3])]
                .iter()
                .map(|(x, y)| ((x - ccx).powi(2) + (y - ccy).powi(2)).sqrt())
                .fold(0.0, f64::max)
                .max(1e-9);
            if (g.fx() - g.cx()).abs() > 1e-3 || (g.fy() - g.cy()).abs() > 1e-3 {
                st.warn("radial gradient focal points are drawn centered".into(), "move the focal point onto the center in your editor");
            }
            Some(Paint::Radial { cx: (ccx - rect[0]) / rect[2], cy: (ccy - rect[1]) / rect[3], stops: stops(g.stops(), opacity, |o| o * r / far) })
        }
        usvg::Paint::Pattern(_) => {
            st.warn("pattern fills are not imported".into(), "use `fill=image(tex.png ...)` on the part, or the raw `svg` (no `parts`)");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100" width="200" height="100">
      <g id="mark" transform="translate(10 10)">
        <circle cx="40" cy="40" r="40" fill="#e30613"/>
        <path id="cut" d="M20 40 H60" stroke="#fff" stroke-width="8" stroke-linecap="round"/>
      </g>
      <g id="word.mark"><rect x="100" y="30" width="90" height="40" fill="url(#g)" fill-rule="evenodd"/></g>
      <defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#000"/><stop offset="1" stop-color="#fff"/></linearGradient></defs>
    </svg>"##;

    #[test]
    fn imports_groups_paths_and_ids() {
        let im = import(LOGO, Path::new(".")).unwrap();
        assert_eq!(im.size, (200.0, 100.0));
        assert_eq!(im.names(), vec!["mark", "p1", "cut", "word-mark", "p2"]);
        let mark = &im.parts[0];
        assert!(matches!(mark.kind, PartKind::Group));
        // the group transform is baked into the geometry
        assert!((mark.rect[0] - 10.0).abs() < 0.01 && (mark.rect[1] - 10.0).abs() < 0.01, "{:?}", mark.rect);
        let PartKind::Path(cut) = &mark.children[1].kind else { panic!() };
        let s = cut.stroke.as_ref().unwrap();
        assert_eq!(s.cap, "round");
        assert!((s.width - 8.0).abs() < 1e-6);
        let PartKind::Path(word) = &im.parts[1].children[0].kind else { panic!() };
        assert!(word.even_odd);
        // objectBoundingBox gradient across the rect: 90deg, stops at the box edges
        match word.fill.as_ref().unwrap() {
            Paint::Linear { angle_deg, stops } => {
                assert!((angle_deg - 90.0).abs() < 0.01, "{angle_deg}");
                assert!((stops[0].pos.unwrap()).abs() < 1e-6 && (stops[1].pos.unwrap() - 1.0).abs() < 1e-6, "{stops:?}");
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn warns_on_unsupported() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><clipPath id="c"><rect width="5" height="5"/></clipPath></defs><g clip-path="url(#c)"><rect width="10" height="10"/></g></svg>"#;
        let im = import(svg, Path::new(".")).unwrap();
        assert!(im.warnings.iter().any(|w| w.0.contains("clip-path")), "{:?}", im.warnings);
        assert!(import("<svg", Path::new(".")).is_err());
    }
}

// ------------------------------------------------------------------ layers

use crate::ast::Node;
use crate::ir::{Ch, Kind, Layer, Len, Pos, TV};
use crate::lower::{Ctx, Named};

/// Colour overrides of `svg ... parts`: `fill=` / `stroke=` for every part, `recolor="#000:$fg, #e30613:$primary"`.
#[derive(Default)]
pub struct Overrides {
    fill: Option<Paint>,
    stroke: Option<Rgba>,
    map: Vec<(Rgba, Rgba)>,
}

fn same_rgb(a: &Rgba, b: &Rgba) -> bool {
    let q = |v: f64| (v * 255.0).round() as i32;
    q(a.r) == q(b.r) && q(a.g) == q(b.g) && q(a.b) == q(b.b)
}

impl Overrides {
    fn color(&self, c: &Rgba) -> Rgba {
        match self.map.iter().find(|(from, _)| same_rgb(from, c)) {
            Some((_, to)) => Rgba { a: to.a * c.a, ..to.clone() },
            None => c.clone(),
        }
    }
    fn paint(&self, p: &Paint) -> Paint {
        if let Some(f) = &self.fill {
            return f.clone();
        }
        let lit = |c: &ColorRef| match c {
            ColorRef::Lit(c) => ColorRef::Lit(self.color(c)),
            t => t.clone(),
        };
        let st = |s: &Vec<Stop>| s.iter().map(|s| Stop { color: lit(&s.color), pos: s.pos }).collect();
        match p {
            Paint::Solid(c) => Paint::Solid(lit(c)),
            Paint::Linear { angle_deg, stops } => Paint::Linear { angle_deg: *angle_deg, stops: st(stops) },
            Paint::Radial { cx, cy, stops } => Paint::Radial { cx: *cx, cy: *cy, stops: st(stops) },
            p => p.clone(),
        }
    }
}

/// Props of an `svg ... parts` line that belong to the parts, not to the group box.
pub const PART_PROPS: &[&str] = &["fill", "stroke", "sw", "recolor"];

impl Ctx<'_> {
    fn svg_overrides(&mut self, n: &Node) -> Overrides {
        let mut o = Overrides { fill: self.paint(n, "fill"), stroke: self.color(n, "stroke"), map: vec![] };
        if let Some(r) = n.props.get("recolor").map(|v| v.raw()) {
            for pair in r.trim_matches(['[', ']']).split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let Some((a, b)) = pair.split_once(':').or_else(|| pair.split_once("->")) else {
                    self.err("E_VALUE", format!("recolor: `{pair}` is not `from:to`"), "write recolor=\"#000000:$fg, #e30613:$primary\"", n.span);
                    continue;
                };
                match (crate::values::parse_color(a.trim()), self.color_s(b.trim(), n.span)) {
                    (Ok(ColorRef::Lit(from)), Some(to)) => o.map.push((from, to)),
                    (Ok(ColorRef::Lit(_)), None) => {}
                    _ => self.err("E_COLOR", format!("recolor: `{}` must be a literal color (the color in the SVG file)", a.trim()), "write the file's hex color on the left: recolor=\"#e30613:$primary\"", n.span),
                }
            }
        }
        o
    }

    /// `svg #id src=logo.svg parts`: the SVG as native part layers under one group (the box).
    pub fn svg_parts(&mut self, parent: usize, si: usize, n: &Node, lid: String, markup: &str, user: Option<&str>) -> usize {
        let im = match import(markup, &self.base) {
            Ok(i) => i,
            Err(e) => {
                self.err("E_SVG", format!("svg parts: cannot parse the SVG: {e}"), "check that the file is valid SVG (opens in a browser); without `parts` it is embedded raw", n.span);
                return self.add(Some(parent), Layer::new(lid, Kind::Group));
            }
        };
        for (w, h) in im.warnings.clone() {
            self.warn("W_SVG_PARTS", format!("svg parts: {w}"), h, n.span);
        }
        if im.parts.is_empty() {
            self.warn("W_SVG_PARTS", "svg parts: the SVG has no visible paths", "check the file; shapes need a fill or a stroke", n.span);
        }
        let (vw, vh) = (im.size.0.max(1e-6), im.size.1.max(1e-6));
        let (lw, lh) = (self.len(n, "w"), self.len(n, "h"));
        let px = |l: &Option<Len>, full: f64| l.as_ref().map(|l| match l.unit { "%" => l.v / 100.0 * full, "em" => l.v * 16.0, _ => l.v });
        let (bw, bh) = match (px(&lw, self.w), px(&lh, self.h)) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, w * vh / vw),
            (None, Some(h)) => (h * vw / vh, h),
            (None, None) => (vw, vh),
        };
        let s = (bw / vw).min(bh / vh).max(1e-9);
        let (ox, oy) = ((bw - vw * s) / 2.0, (bh - vh * s) / 2.0);
        let mut g = Layer::new(lid, Kind::Group);
        g.tags.push("svg-parts".into());
        match (&lw, &lh) {
            (None, None) => {
                g.layout.w = Some(Len::px(bw));
                g.layout.h = Some(Len::px(bh));
            }
            (Some(_), None) | (None, Some(_)) => g.layout.aspect = Some(vw / vh),
            _ => {}
        }
        let gi = self.add(Some(parent), g);
        let ov = self.svg_overrides(n);
        let sw = self.num(n, "sw");
        let root = [-ox / s, -oy / s, bw / s, bh / s];
        let names = im.names();
        self.svg_children(gi, si, &im.parts, root, s, user, &ov, sw, n);
        if let Some(u) = user {
            self.svg_part_names.insert(format!("{u}@{si}"), names);
        }
        gi
    }

    #[allow(clippy::too_many_arguments)]
    fn svg_children(&mut self, host: usize, si: usize, parts: &[Part], frame: [f64; 4], s: f64, user: Option<&str>, ov: &Overrides, sw: Option<f64>, n: &Node) {
        for p in parts {
            let key = p.name.clone().unwrap_or_else(|| "g".into());
            let id = format!("{}-{}", self.lid(host), key);
            let kind = match &p.kind {
                PartKind::Group => Kind::Group,
                PartKind::Path(pp) => {
                    let d = pp.d((p.rect[0], p.rect[1]), s);
                    Kind::Path { closed: d.ends_with('Z'), d, vb: (p.rect[2] * s, p.rect[3] * s) }
                }
            };
            let mut l = Layer::new(id, kind);
            l.layout.pos = Pos::Abs;
            l.layout.x = Some(Len::pct((p.rect[0] - frame[0]) / frame[2] * 100.0));
            l.layout.y = Some(Len::pct((p.rect[1] - frame[1]) / frame[3] * 100.0));
            l.layout.w = Some(Len::pct(p.rect[2] / frame[2] * 100.0));
            l.layout.h = Some(Len::pct(p.rect[3] / frame[3] * 100.0));
            l.tags.push("svg-part".into());
            if let PartKind::Path(pp) = &p.kind {
                l.style.fill = pp.fill.as_ref().map(|f| ov.paint(f));
                l.style.even_odd = pp.even_odd;
                if let Some(st) = &pp.stroke {
                    l.style.stroke = Some(ov.stroke.clone().unwrap_or_else(|| ov.color(&st.color)));
                    l.style.stroke_paint = if ov.stroke.is_some() { None } else { st.paint.as_ref().map(|p| ov.paint(p)) };
                    l.style.stroke_w = sw.unwrap_or(st.width * s);
                    l.style.stroke_cap = Some(st.cap);
                    l.style.stroke_join = Some(st.join);
                    l.style.dash = st.dash.map(|(a, b)| (a * s, b * s));
                } else if l.style.fill.is_some() {
                    l.tags.push("svg-fill".into());
                }
            }
            let name = match (user, &p.name) {
                (Some(u), Some(pn)) => Some(format!("{u}.{pn}")),
                _ => None,
            };
            if let Some(nm) = &name {
                if self.names.contains_key(&format!("{nm}@{si}")) {
                    self.warn("W_SVG_PARTS", format!("svg parts: id `{}` is used twice in the SVG; `#{nm}` is the first", p.name.as_deref().unwrap_or("")), "give each SVG element a unique id", n.span);
                } else {
                    l.name = Some(nm.clone());
                }
            }
            let i = self.add(Some(host), l);
            if let Some(nm) = name.filter(|_| self.layers[i].name.is_some()) {
                self.names.insert(format!("{nm}@{si}"), Named { idx: i, scene: si, kind: "part".into() });
            }
            if p.opacity < 0.999 {
                self.set_base(i, Ch::Opacity, TV::N(p.opacity));
            }
            if matches!(p.kind, PartKind::Group) {
                self.svg_children(i, si, &p.children, p.rect, s, user, ov, sw, n);
            }
        }
    }

    /// Path layers under an svg parts group (or part group), document order.
    pub fn svg_leaves(&self, i: usize, out: &mut Vec<usize>) {
        for &c in &self.kids[i] {
            if matches!(self.layers[c].kind, Kind::Path { .. }) {
                out.push(c);
            } else {
                self.svg_leaves(c, out);
            }
        }
    }
}
