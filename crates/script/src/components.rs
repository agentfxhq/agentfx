//! S1 component library: designed-by-default components on brand tokens.
//!
//! Every component is an ordinary Layer tree (both backends render it), built from theme tokens
//! only (colors, radius, type scale), with one design language: layered surfaces (lighter top,
//! 1 px border, two soft shadows), icon tiles on the accent gradient, a fixed type hierarchy
//! (kicker / title / body / number), generous padding.
use crate::ast::{Node, Value};
use crate::ir::*;
use crate::lower::*;
use crate::theme::hex;
use crate::values::*;

pub const COMPONENTS: &[&str] = &["card", "stat", "note", "node", "lower3", "chart", "steps", "pill", "quote", "list", "window", "device", "progress", "panel"];

pub const ICONS: &[(&str, &str)] = &[
    ("panels", "<rect x='3' y='4' width='18' height='16' rx='2'/><path d='M9 4v16M15 4v16'/>"),
    ("upload", "<path d='M12 16V4M7 9l5-5 5 5'/><path d='M4 16v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3'/>"),
    ("sync", "<path d='M7 4v14M3 14l4 4 4-4'/><path d='M17 20V6M13 10l4-4 4 4'/>"),
    ("check", "<path d='M5 12l5 5 9-10'/>"),
    ("x", "<path d='M6 6l12 12M18 6L6 18'/>"),
    ("arrow", "<path d='M5 12h14M13 6l6 6-6 6'/>"),
    ("play", "<path d='M7 4l13 8-13 8z'/>"),
    ("chart", "<path d='M4 20V10M10 20V4M16 20v-7M22 20H2'/>"),
    ("code", "<path d='M8 7l-5 5 5 5M16 7l5 5-5 5'/>"),
    ("cloud", "<path d='M7 18a5 5 0 1 1 1-9.9A6 6 0 0 1 19 10a4 4 0 0 1-1 8z'/>"),
    ("db", "<ellipse cx='12' cy='5' rx='8' ry='3'/><path d='M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3'/>"),
    ("user", "<circle cx='12' cy='8' r='4'/><path d='M4 21c1-4 4.5-6 8-6s7 2 8 6'/>"),
    ("lock", "<rect x='5' y='11' width='14' height='10' rx='2'/><path d='M8 11V7a4 4 0 0 1 8 0v4'/>"),
    ("bolt", "<path d='M13 2L4 14h7l-1 8 9-12h-7z'/>"),
    ("star", "<path d='M12 3l2.8 5.8 6.2.9-4.5 4.4 1 6.3L12 17.5 6.5 20.4l1-6.3L3 9.7l6.2-.9z'/>"),
    ("gear", "<circle cx='12' cy='12' r='3'/><path d='M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1'/>"),
    ("search", "<circle cx='11' cy='11' r='7'/><path d='M21 21l-5-5'/>"),
    ("mail", "<rect x='3' y='5' width='18' height='14' rx='2'/><path d='M3 7l9 6 9-6'/>"),
    ("file", "<path d='M14 3H6v18h12V7z'/><path d='M14 3v4h4'/>"),
    ("globe", "<circle cx='12' cy='12' r='9'/><path d='M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18'/>"),
    ("server", "<rect x='3' y='4' width='18' height='7' rx='2'/><rect x='3' y='13' width='18' height='7' rx='2'/><path d='M7 7.5h.01M7 16.5h.01'/>"),
    ("phone", "<rect x='7' y='2' width='10' height='20' rx='2'/><path d='M11 18h2'/>"),
    ("sparkle", "<path d='M12 3l2 7 7 2-7 2-2 7-2-7-7-2 7-2z'/>"),
    ("chat", "<path d='M4 5h16v11H9l-5 4z'/>"),
    ("git", "<circle cx='6' cy='6' r='2.5'/><circle cx='6' cy='18' r='2.5'/><circle cx='18' cy='9' r='2.5'/><path d='M6 8.5v7M18 11.5c0 4-6 3-11 5'/>"),
    ("eye", "<path d='M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z'/><circle cx='12' cy='12' r='3'/>"),
    ("rocket", "<path d='M12 15l-3-3c1-5 4-9 10-10-1 6-5 9-10 10M9 12l-4 1-2 4 4-1M12 15l-1 4-4 2 1-4'/>"),
    ("clock", "<circle cx='12' cy='12' r='9'/><path d='M12 7v5l3 2'/>"),
    ("cart", "<circle cx='9' cy='20' r='1.5'/><circle cx='18' cy='20' r='1.5'/><path d='M2 3h3l3 12h11l2-8H6'/>"),
    ("trend", "<path d='M3 17l6-6 4 4 8-8M15 7h6v6'/>"),
    ("link", "<path d='M10 14a4 4 0 0 0 6 0l3-3a4 4 0 0 0-6-6l-1 1M14 10a4 4 0 0 0-6 0l-3 3a4 4 0 0 0 6 6l1-1'/>"),
    ("cursor", "<path d='M5 3l14 8-6 2-2 6z'/>"),
    ("shield", "<path d='M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z'/>"),
    ("heart", "<path d='M12 20s-7-4.4-9-9a5 5 0 0 1 9-3 5 5 0 0 1 9 3c-2 4.6-9 9-9 9z'/>"),
    ("calendar", "<rect x='3' y='5' width='18' height='16' rx='2'/><path d='M3 10h18M8 3v4M16 3v4'/>"),
    ("money", "<rect x='2' y='6' width='20' height='12' rx='2'/><circle cx='12' cy='12' r='3'/>"),
    ("target", "<circle cx='12' cy='12' r='9'/><circle cx='12' cy='12' r='5'/><circle cx='12' cy='12' r='1'/>"),
    ("layers", "<path d='M12 3l9 5-9 5-9-5z'/><path d='M3 13l9 5 9-5'/>"),
];

pub fn icon_body(name: &str) -> Option<&'static str> {
    ICONS.iter().find(|(k, _)| *k == name).map(|(_, b)| *b)
}

fn praw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}
fn a(c: &Rgba, alpha: f64) -> Rgba {
    Rgba { a: c.a * alpha, ..c.clone() }
}
fn solid(c: &Rgba) -> Paint {
    Paint::Solid(ColorRef::Lit(c.clone()))
}
fn lin(angle: f64, c0: &Rgba, c1: &Rgba) -> Paint {
    Paint::Linear { angle_deg: angle, stops: vec![Stop { color: ColorRef::Lit(c0.clone()), pos: Some(0.0) }, Stop { color: ColorRef::Lit(c1.clone()), pos: Some(1.0) }] }
}
fn str_args(n: &Node) -> Vec<String> {
    n.args.iter().filter_map(|v| match v {
        Value::Str(s) if !s.starts_with('#') => Some(s.clone()),
        Value::Num(f) => Some(crate::ast::fmt_num(*f)),
        _ => None,
    }).collect()
}
fn nums(n: &Node, k: &str) -> Vec<f64> {
    match n.props.get(k) {
        Some(Value::List(v)) => v.iter().filter_map(|x| match x {
            Value::Num(f) => Some(*f),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }).collect(),
        Some(v) => v.raw().split([',', ' ']).filter_map(|x| x.trim().parse().ok()).collect(),
        None => vec![],
    }
}
/// Smooth cubic path through points (same curve as the chart line).
fn smooth(pts: &[(f64, f64)]) -> String {
    let mut d = String::new();
    for (k, (x, y)) in pts.iter().enumerate() {
        if k == 0 {
            d.push_str(&format!("M{x:.1} {y:.1}"));
        } else {
            let (px, py) = pts[k - 1];
            let cx = (px + x) / 2.0;
            d.push_str(&format!(" C{cx:.1} {py:.1} {cx:.1} {y:.1} {x:.1} {y:.1}"));
        }
    }
    d
}
/// "Nice" axis maximum and step (1 / 2 / 2.5 / 5 x 10^n), 4 or 5 intervals.
fn nice_axis(max: f64) -> (f64, f64) {
    let max = max.max(1e-9);
    let raw = max / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw - 1e-12).unwrap_or(10.0 * mag);
    ((max / step).ceil() * step, step)
}
fn fmt_tick(v: f64, unit: &str, de: bool) -> String {
    let s = if v.fract().abs() < 1e-9 { format!("{}", v as i64) } else { format!("{:.1}", v) };
    format!("{}{unit}", if de { s.replace('.', ",") } else { s })
}
/// Value label: decimal comma for German (default `locale=de`), as written in the narration.
fn fmt_val(v: f64, de: bool) -> String {
    let s = crate::ast::fmt_num((v * 100.0).round() / 100.0);
    if de { s.replace('.', ",") } else { s }
}

/// Kind colors for notes/status.
fn kind_color(th: &crate::theme::Theme, k: &str) -> Rgba {
    let key = match k {
        "warn" | "warning" => "warn",
        "danger" | "error" | "err" => "danger",
        "tip" => "mint",
        _ => "accent2",
    };
    th.colors[key].clone()
}

pub struct TextSpec<'s> {
    pub size: f64,
    pub weight: f64,
    pub color: Rgba,
    pub align: &'s str,
    pub tracking: Option<f64>,
    pub upper: bool,
    pub nowrap: bool,
    pub lh: Option<f64>,
}

impl<'a> Ctx<'a> {
    fn light(&self) -> bool {
        self.th.name.contains("light")
    }
    fn c(&self, k: &str) -> Rgba {
        self.th.colors.get(k).cloned().unwrap_or(hex("#f0f"))
    }
    /// The surface treatment shared by cards, tiles, nodes, notes (design `surface style=`).
    fn surface(&self, l: &mut Layer, radius: f64) {
        let d = &self.th.d;
        l.style.radius = radius;
        let deep = if self.light() { hex("#0f172a1f") } else { hex("#00000052") };
        let tight = if self.light() { hex("#0f172a14") } else { hex("#00000038") };
        let soft = vec![Shadow { y: 24.0, blur: 60.0, color: Some(deep), ..Default::default() }, Shadow { y: 2.0, blur: 6.0, color: Some(tight), ..Default::default() }];
        let shadows = d.shadows.clone().unwrap_or(soft);
        match d.surface.as_str() {
            "flat" => {
                l.style.fill = Some(solid(&self.c("surface")));
                l.style.border = None;
                l.style.shadows = d.shadows.clone().unwrap_or_default();
            }
            "outline" => {
                l.style.fill = None;
                l.style.border = Some((d.border_w.max(1.0), self.c("border")));
                l.style.shadows = d.shadows.clone().unwrap_or_default();
            }
            "glass" => {
                l.style.fill = Some(lin(160.0, &a(&self.c("paper"), 0.16), &a(&self.c("paper"), 0.05)));
                l.style.border = Some((d.border_w, a(&self.c("paper"), 0.22)));
                l.style.shadows = shadows;
            }
            "tinted" => {
                l.style.fill = Some(solid(&a(&self.c("accent"), 0.12)));
                l.style.border = Some((d.border_w, a(&self.c("accent"), 0.35)));
                l.style.shadows = d.shadows.clone().unwrap_or_default();
            }
            _ => {
                l.style.fill = Some(lin(180.0, &self.c("surface2"), &self.c("surface")));
                l.style.border = Some((d.border_w, self.c("border")));
                l.style.shadows = shadows;
            }
        }
    }
    pub fn surface_pub(&self, l: &mut Layer, radius: f64) {
        self.surface(l, radius)
    }
    /// Scaled length (component scale from the design spec).
    pub fn s(&self, v: f64) -> f64 {
        (v * self.k).round()
    }
    fn col(&self, id: String, gap: f64) -> Layer {
        let mut l = Layer::new(id, Kind::Group);
        l.layout.flow = "col";
        l.layout.gap = gap;
        l.layout.align = "start".into();
        l.layout.justify = "start".into();
        l
    }
    fn row(&self, id: String, gap: f64) -> Layer {
        let mut l = Layer::new(id, Kind::Group);
        l.layout.flow = "row";
        l.layout.gap = gap;
        l.layout.align = "center".into();
        l.layout.justify = "start".into();
        l
    }
    pub fn text_l(&mut self, parent: usize, id: String, text: &str, s: TextSpec) -> usize {
        let mut l = Layer::new(id, Kind::Text { runs: self.runs(text) });
        l.tags.push("text".into());
        // design roles: labels (upper, tracked), headings (>= 650 weight), body
        let d = &self.th.d;
        let label = s.upper || s.tracking.is_some_and(|t| t > 0.03);
        let heading = !label && s.weight >= 650.0;
        let (weight, tracking, upper, font) = if label {
            (if s.weight >= 600.0 { d.w_label } else { s.weight }, Some(d.tr_label), d.label_upper && s.upper, self.th.role_font(false))
        } else if heading {
            (if s.weight >= 790.0 { d.w_display } else { d.w_heading }, s.tracking.map(|_| if s.size >= 80.0 { d.tr_display } else { d.tr_heading }), false, self.th.role_font(true))
        } else {
            (if (s.weight - 500.0).abs() < 60.0 { d.w_body + (s.weight - 500.0) } else { s.weight }, s.tracking, false, self.th.role_font(false))
        };
        l.style.size = Some((s.size * self.k).round());
        l.style.weight = Some(weight);
        l.style.font = Some(font);
        l.style.color = Some(s.color);
        l.style.accent = Some(self.c("mint"));
        l.style.text_align = Some(s.align.into());
        l.style.tracking = tracking;
        l.style.uppercase = upper;
        if !label && !heading && s.lh.is_some() {
            l.style.line_height = Some(d.lh_body);
        }
        l.style.nowrap = s.nowrap;
        if l.style.line_height.is_none() {
            l.style.line_height = s.lh;
        }
        self.add(Some(parent), l)
    }
    fn kicker(&mut self, parent: usize, id: String, text: &str, color: Rgba) -> usize {
        self.text_l(parent, id, text, TextSpec { size: 22.0, weight: 700.0, color, align: "left", tracking: Some(0.08), upper: true, nowrap: true, lh: None })
    }
    /// Inline SVG icon layer (stroke icons from ICONS).
    pub fn icon_l(&mut self, parent: usize, id: String, name: &str, size: f64, color: &Rgba, sw: f64, n: &Node) -> Option<usize> {
        let Some(body) = icon_body(name) else {
            let names: Vec<&str> = ICONS.iter().map(|(k, _)| *k).collect();
            let hint = crate::values::suggest(name, &names).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("icons: {}", names.join(", ")));
            self.err("E_ICON", format!("unknown icon `{name}`"), hint, n.span);
            return None;
        };
        let markup = format!("<svg viewBox='0 0 24 24' width='100%' height='100%' fill='none' stroke='{}' stroke-width='{sw}' stroke-linecap='round' stroke-linejoin='round'>{body}</svg>", color.to_css());
        let mut l = Layer::new(id, Kind::Svg { markup });
        l.layout.w = Some(Len::px(size));
        l.layout.h = Some(Len::px(size));
        Some(self.add(Some(parent), l))
    }
    /// Icon tile (design `icons style=`): tile (gradient, default), solid, outline, plain, ring.
    fn icon_tile(&mut self, parent: usize, id: String, name: &str, size: f64, fill: Paint, stroke: &Rgba, n: &Node) -> usize {
        let size = self.s(size);
        let style = self.th.d.icons.clone();
        let sw = self.th.d.icon_stroke;
        let mut t = Layer::new(id.clone(), Kind::Group);
        t.layout.w = Some(Len::px(size));
        t.layout.h = Some(Len::px(size));
        t.layout.flow = "row";
        // tinted fills (notes, lists) keep their own look in every style
        let tinted = matches!(&fill, Paint::Solid(ColorRef::Lit(c)) if c.a < 0.5);
        let mut ic = stroke.clone();
        match style.as_str() {
            _ if tinted => {
                t.style.fill = Some(fill);
                t.style.radius = (size / 4.0).round();
            }
            "solid" => {
                t.style.fill = Some(solid(&self.c("accent")));
                t.style.radius = self.th.d.radius_sm.unwrap_or((size / 4.0).round());
            }
            "outline" => {
                t.style.border = Some((self.th.d.border_w.max(1.5), self.c("accent2")));
                t.style.radius = self.th.d.radius_sm.unwrap_or((size / 4.0).round());
                ic = self.c("accent2");
            }
            "ring" => {
                t.style.border = Some((self.th.d.border_w.max(2.0), self.c("accent2")));
                t.style.radius = size;
                ic = self.c("accent2");
            }
            "plain" => {
                ic = self.c("accent2");
            }
            _ => {
                t.style.fill = Some(fill);
                t.style.radius = self.th.d.radius_sm.unwrap_or((size / 4.0).round());
            }
        }
        let ti = self.add(Some(parent), t);
        let isz = if style == "plain" { size * 0.8 } else { size * 0.55 };
        self.icon_l(ti, format!("{id}-i"), name, isz.round(), &ic, sw, n);
        ti
    }
    /// Tiles (cards, stats) in a row share the row's width equally and stretch to equal height:
    /// the row fills the safe width unless it has its own `w`.
    fn tile_in_row(&mut self, parent: usize, l: &mut Layer, n: &Node) {
        if self.layers[parent].layout.flow == "row" && !n.props.contains_key("w") && !n.props.contains_key("grow") {
            if self.layers[parent].layout.w.is_none() {
                self.layers[parent].layout.w = Some(Len::pct(100.0));
            }
            self.layers[parent].layout.align = "stretch".into();
            l.layout.grow = 1.0;
            l.layout.w = Some(Len::px(0.0));
            l.layout.maxw = None;
        } else if self.h > self.w && self.layers[parent].layout.flow == "col" && !n.props.contains_key("w") {
            // portrait stack: tiles take the full column width
            l.layout.w = Some(Len::pct(100.0));
            l.layout.maxw = None;
        }
    }
    /// Hidden accent state (`activate #id` fades it in): accent gradient + mint border, behind the
    /// content (first child), same radius as the tile.
    fn active_layer(&mut self, gi: usize) {
        let id = self.lid(gi);
        let mut l = Layer::new(format!("{id}-active"), Kind::Rect);
        l.layout.pos = Pos::Fill;
        l.style.fill = Some(lin(150.0, &self.c("accent2"), &self.c("accent")));
        l.style.border = Some((2.0, self.c("mint")));
        l.style.radius = self.layers[gi].style.radius;
        let ai = self.add(Some(gi), l);
        self.set_base(ai, Ch::Opacity, TV::N(0.0));
    }
    fn accent_tile(&self) -> Paint {
        lin(150.0, &self.c("accent2"), &self.c("accent"))
    }
    fn bar(&mut self, parent: usize, id: String, color: &Rgba, w: f64) -> usize {
        let mut b = Layer::new(id, Kind::Rect);
        b.layout.pos = Pos::Abs;
        b.layout.x = Some(Len::px(0.0));
        b.layout.y = Some(Len::px(0.0));
        b.layout.w = Some(Len::px(w));
        b.layout.h = Some(Len::pct(100.0));
        b.style.fill = Some(solid(color));
        self.add(Some(parent), b)
    }

    /// Build component `n.kind`; None when it is not a component.
    pub fn component(&mut self, parent: usize, si: usize, n: &Node, lid: String) -> Option<usize> {
        if !crate::components::COMPONENTS.contains(&n.kind.as_str()) {
            return None;
        }
        let d = &self.th.d;
        let prev_k = self.k;
        self.k = match n.kind.as_str() {
            // an explicit w= is the author's size decision: the diagram scale would overflow it
            "node" if n.props.contains_key("w") => d.scale,
            // skins are drawn at video size already; portrait stacks nodes, so no diagram boost
            "node" if self.skin_for(n).is_some() || self.h > self.w => d.scale,
            "node" => d.scale * d.scale_diagram,
            "chart" => d.scale * d.scale_chart,
            _ => d.scale,
        };
        // L3: an HTML/CSS skin (node skin=tile, or the design's default `skin node=tile`)
        if let Some(sk) = self.skin_for(n) {
            let kind = n.kind.clone();
            if let Some(i) = self.skin(parent, si, n, lid.clone(), &kind, &sk) {
                self.k = prev_k;
                return Some(i);
            }
        }
        let r = if n.kind == "panel" {
            let u = id_arg(n);
            Some(self.panel_c(parent, si, n, lid, u))
        } else if n.kind == "chart" && (n.props.contains_key("title") || n.props.contains_key("source") || n.props.contains_key("kicker")) {
            let fi = self.chart_frame_open(parent, n, &lid);
            let ci = self.chart2(fi, n, lid.clone());
            self.chart_frame_close(fi, n, &lid);
            Some(ci)
        } else {
            self.component_inner(parent, si, n, lid)
        };
        self.k = prev_k;
        r
    }
    fn component_inner(&mut self, parent: usize, si: usize, n: &Node, lid: String) -> Option<usize> {
        Some(match n.kind.as_str() {
            "card" => self.card(parent, n, lid),
            "stat" => self.stat(parent, n, lid),
            "note" => self.note(parent, n, lid),
            "node" => self.node_c(parent, n, lid),
            "lower3" => self.lower3_c(si, n, lid),
            "chart" => self.chart2(parent, n, lid),
            "steps" => self.steps(parent, n, lid),
            "pill" => self.pill(parent, n, lid),
            "quote" => self.quote(parent, n, lid),
            "list" => self.list(parent, n, lid),
            "window" => self.window(parent, n, lid),
            "device" => self.device(parent, n, lid),
            "progress" => self.progress_c(parent, n, lid),
            _ => return None,
        })
    }

    fn card(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let variant = praw(n, "variant").unwrap_or_else(|| "surface".into());
        let mut g = self.col(lid, self.s(16.0));
        g.layout.pad = [self.s(40.0), self.s(44.0), self.s(42.0), self.s(44.0)];
        self.surface(&mut g, self.th.radius);
        let (title_c, sub_c, tile_fill, tile_stroke) = match variant.as_str() {
            "glass" => {
                g.style.fill = Some(lin(160.0, &hex("#ffffff1f"), &hex("#ffffff0a")));
                g.style.border = Some((1.0, hex("#ffffff33")));
                (self.c("text"), self.c("muted"), self.accent_tile(), self.c("paper"))
            }
            "accent" => {
                g.style.fill = Some(lin(150.0, &self.c("accent2"), &self.c("accent")));
                g.style.border = Some((1.0, hex("#ffffff2e")));
                (self.c("paper"), a(&self.c("paper"), 0.85), solid(&hex("#ffffff26")), self.c("paper"))
            }
            "outline" => {
                g.style.fill = None;
                g.style.shadows.clear();
                g.style.border = Some((2.0, self.c("accent2")));
                (self.c("text"), self.c("muted"), self.accent_tile(), self.c("paper"))
            }
            _ => (self.c("text"), self.c("muted"), self.accent_tile(), self.c("paper")),
        };
        if variant != "outline" && variant != "surface" && variant != "glass" && variant != "accent" {
            self.warn("E_VALUE", format!("card variant `{variant}` unknown"), "variant=surface|glass|accent|outline", n.span);
        }
        if !n.props.contains_key("w") && !n.props.contains_key("grow") {
            g.layout.maxw = Some(Len::px(620.0));
        }
        self.tile_in_row(parent, &mut g, n);
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        self.active_layer(gi);
        if let Some(ic) = praw(n, "icon") {
            self.icon_tile(gi, format!("{id}-icon"), &ic, 64.0, tile_fill, &tile_stroke, n);
            // a little air between tile and text
            let mut sp = Layer::new(format!("{id}-sp"), Kind::Group);
            sp.layout.h = Some(Len::px(4.0));
            self.add(Some(gi), sp);
        }
        if let Some(k) = praw(n, "kicker") {
            let kc = if variant == "accent" { a(&self.c("paper"), 0.8) } else { self.c("mint") };
            self.kicker(gi, format!("{id}-kicker"), &k, kc);
        }
        if let Some(t) = text_arg(n) {
            let size = self.num(n, "size").unwrap_or(44.0);
            self.text_l(gi, format!("{id}-label"), &t, TextSpec { size, weight: 720.0, color: title_c, align: "left", tracking: Some(-0.01), upper: false, nowrap: false, lh: Some(1.15) });
        }
        if let Some(s) = praw(n, "sub") {
            self.text_l(gi, format!("{id}-sub"), &s, TextSpec { size: 30.0, weight: 500.0, color: sub_c, align: "left", tracking: None, upper: false, nowrap: false, lh: Some(1.35) });
        }
        gi
    }

    fn stat(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let mut g = self.col(lid, self.s(12.0));
        g.layout.pad = [self.s(34.0), self.s(40.0), self.s(38.0), self.s(40.0)];
        self.surface(&mut g, self.th.radius);
        if !n.props.contains_key("w") && !n.props.contains_key("grow") {
            g.layout.w = Some(Len::px(400.0));
        }
        self.tile_in_row(parent, &mut g, n);
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        self.active_layer(gi);
        let label = praw(n, "label").or_else(|| text_arg(n));
        if praw(n, "icon").is_some() || label.is_some() {
            let r = self.row(format!("{id}-top"), 14.0);
            let ri = self.add(Some(gi), r);
            if let Some(ic) = praw(n, "icon") {
                let f = self.accent_tile();
                let p = self.c("paper");
                self.icon_tile(ri, format!("{id}-icon"), &ic, 48.0, f, &p, n);
            }
            if let Some(l) = label {
                let c = self.c("muted");
                self.kicker(ri, format!("{id}-label"), &l, c);
            }
        }
        let value = self.num(n, "value").or_else(|| self.num(n, "to")).unwrap_or(0.0);
        let from = self.num(n, "from").unwrap_or(value);
        let mut v = Layer::new(format!("{id}-value"), Kind::Counter { prefix: praw(n, "prefix").unwrap_or_default(), suffix: praw(n, "suffix").unwrap_or_default() });
        if let Some(sp) = praw(n, "sep") {
            v.tags.push(format!("sep:{sp}"));
        }
        v.tags.push("text".into());
        v.style.size = Some(self.num(n, "size").unwrap_or(96.0));
        v.style.weight = Some(800.0);
        v.style.tracking = Some(-0.03);
        v.style.color = Some(self.c("text"));
        v.style.text_align = Some("left".into());
        v.style.nowrap = true;
        v.style.line_height = Some(1.05);
        let vi = self.add(Some(gi), v);
        // `in=count` / `count #stat` animate this counter (from= .. value=)
        self.set_base(vi, Ch::Count, TV::N(from));
        let _ = value;
        let delta = praw(n, "delta");
        let sub = praw(n, "sub");
        if delta.is_some() || sub.is_some() {
            let r = self.row(format!("{id}-meta"), 12.0);
            let ri = self.add(Some(gi), r);
            if let Some(d) = delta {
                let neg = d.trim_start().starts_with('-') || d.trim_start().starts_with('\u{2212}');
                let lighten = |c: Rgba| Rgba { r: (c.r + 1.0) / 2.0, g: (c.g + 1.0) / 2.0, b: (c.b + 1.0) / 2.0, a: 1.0 };
                let (fc, tc) = if neg { (self.c("danger"), if self.light() { self.c("danger") } else { lighten(self.c("danger")) }) } else { (self.c("accent2"), self.c("mint")) };
                let mut p = Layer::new(format!("{id}-delta"), Kind::Text { runs: self.runs(&d) });
                p.tags.push("text".into());
                p.style.size = Some(24.0);
                p.style.weight = Some(750.0);
                p.style.color = Some(tc);
                p.style.fill = Some(solid(&a(&fc, 0.2)));
                p.style.radius = 999.0;
                p.style.nowrap = true;
                p.layout.pad = [5.0, 14.0, 5.0, 14.0];
                self.add(Some(ri), p);
            }
            if let Some(s) = sub {
                let c = self.c("muted");
                self.text_l(ri, format!("{id}-sub"), &s, TextSpec { size: 24.0, weight: 500.0, color: c, align: "left", tracking: None, upper: false, nowrap: true, lh: None });
            }
        }
        let spark = nums(n, "spark");
        if spark.len() >= 2 {
            let (w, h) = (240.0, 56.0);
            let max = spark.iter().cloned().fold(f64::MIN, f64::max);
            let min = spark.iter().cloned().fold(f64::MAX, f64::min);
            let span = (max - min).max(1e-9);
            let pts: Vec<(f64, f64)> = spark.iter().enumerate().map(|(k, v)| (k as f64 / (spark.len() - 1) as f64 * (w - 6.0) + 3.0, 4.0 + (h - 8.0) * (1.0 - (v - min) / span))).collect();
            let d = smooth(&pts);
            let mut box_ = Layer::new(format!("{id}-spark"), Kind::Group);
            box_.layout.w = Some(Len::pct(100.0));
            box_.layout.h = Some(Len::px(h));
            let bi = self.add(Some(gi), box_);
            let mut ar = Layer::new(format!("{id}-spark-area"), Kind::Path { d: format!("{d} L{:.1} {h} L{:.1} {h} Z", pts.last().unwrap().0, pts[0].0), closed: true, vb: (w, h) });
            ar.layout.pos = Pos::Fill;
            ar.style.fill = Some(lin(180.0, &a(&self.c("accent2"), 0.35), &a(&self.c("accent2"), 0.0)));
            ar.tags.push("area".into());
            self.add(Some(bi), ar);
            let mut ln = Layer::new(format!("{id}-spark-line"), Kind::Path { d, closed: false, vb: (w, h) });
            ln.layout.pos = Pos::Fill;
            ln.style.stroke = Some(self.c("mint"));
            ln.style.stroke_w = 3.0;
            ln.tags.push("line".into());
            self.add(Some(bi), ln);
        }
        gi
    }

    fn note(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let kind = praw(n, "kind").unwrap_or_else(|| "info".into());
        let kc = kind_color(&self.th, &kind);
        let mut g = self.row(lid, 22.0);
        g.layout.align = "start".into();
        g.layout.pad = [30.0, 38.0, 30.0, 40.0];
        self.surface(&mut g, self.th.radius);
        g.style.overflow_clip = true;
        if !n.props.contains_key("w") && !n.props.contains_key("maxw") {
            g.layout.maxw = Some(Len::px(980.0));
        }
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        self.bar(gi, format!("{id}-bar"), &kc, 6.0);
        let icon = praw(n, "icon").unwrap_or_else(|| match kind.as_str() {
            "warn" | "warning" => "bolt",
            "danger" | "error" => "x",
            "tip" => "sparkle",
            "success" => "check",
            _ => "eye",
        }.into());
        self.icon_tile(gi, format!("{id}-icon"), &icon, 56.0, solid(&a(&kc, 0.18)), &kc, n);
        let mut c = self.col(format!("{id}-body"), 6.0);
        c.layout.grow = 1.0;
        let ci = self.add(Some(gi), c);
        if let Some(t) = praw(n, "title") {
            let tc = self.c("text");
            self.text_l(ci, format!("{id}-title"), &t, TextSpec { size: 34.0, weight: 720.0, color: tc, align: "left", tracking: Some(-0.01), upper: false, nowrap: false, lh: Some(1.2) });
        }
        if let Some(t) = text_arg(n) {
            let mc = self.c("muted");
            self.text_l(ci, format!("{id}-text"), &t, TextSpec { size: 30.0, weight: 500.0, color: mc, align: "left", tracking: None, upper: false, nowrap: false, lh: Some(1.35) });
        }
        gi
    }

    fn node_c(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let variant = praw(n, "variant").unwrap_or_else(|| "default".into());
        let mut g = self.row(lid, self.s(18.0));
        g.layout.pad = [self.s(22.0), self.s(32.0), self.s(22.0), self.s(24.0)];
        self.surface(&mut g, (self.th.radius * 0.8).round());
        let (lc, sc, tf, ts) = match variant.as_str() {
            "primary" => {
                g.style.fill = Some(lin(150.0, &self.c("accent2"), &self.c("accent")));
                g.style.border = Some((1.0, hex("#ffffff2e")));
                (self.c("paper"), a(&self.c("paper"), 0.8), solid(&hex("#ffffff26")), self.c("paper"))
            }
            "muted" => {
                g.style.fill = Some(solid(&self.c("surface")));
                g.style.shadows.clear();
                (self.c("muted"), self.c("faint"), solid(&self.c("surface2")), self.c("muted"))
            }
            _ => (self.c("text"), self.c("muted"), self.accent_tile(), self.c("paper")),
        };
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        self.active_layer(gi);
        let bs = self.s(40.0);
        self.done_badge(gi, bs);
        if let Some(ic) = praw(n, "icon") {
            self.icon_tile(gi, format!("{id}-icon"), &ic, 54.0, tf, &ts, n);
        }
        let label = text_arg(n);
        let sub = praw(n, "sub");
        if label.is_some() || sub.is_some() {
            let mut c = self.col(format!("{id}-txt"), 2.0);
            c.layout.align = "start".into();
            let ci = self.add(Some(gi), c);
            if let Some(t) = label {
                let size = self.num(n, "size").unwrap_or(34.0);
                self.text_l(ci, format!("{id}-label"), &t, TextSpec { size, weight: 720.0, color: lc, align: "left", tracking: Some(-0.01), upper: false, nowrap: true, lh: Some(1.2) });
            }
            if let Some(s) = sub {
                self.text_l(ci, format!("{id}-sub"), &s, TextSpec { size: 24.0, weight: 500.0, color: sc, align: "left", tracking: None, upper: false, nowrap: true, lh: Some(1.25) });
            }
        }
        if let Some(st) = praw(n, "status") {
            let c = match st.as_str() {
                "warn" => self.c("warn"),
                "err" | "error" => self.c("danger"),
                _ => self.c("accent2"),
            };
            let mut d = Layer::new(format!("{id}-status"), Kind::Ellipse);
            d.layout.pos = Pos::Abs;
            d.layout.x = Some(Len::pct(100.0));
            d.layout.y = Some(Len::px(0.0));
            d.layout.anchor = (0.6, 0.4);
            d.layout.w = Some(Len::px(16.0));
            d.layout.h = Some(Len::px(16.0));
            d.style.fill = Some(solid(&c));
            d.style.border = Some((3.0, self.c("bg")));
            self.add(Some(gi), d);
        }
        gi
    }

    fn lower3_c(&mut self, si: usize, n: &Node, lid: String) -> usize {
        let ov = self.scenes[si].overlay;
        let mut g = self.row(lid, 18.0);
        g.layout.pos = Pos::Abs;
        g.layout.x = Some(Len::pct(self.th.safe.0 * 100.0));
        g.layout.y = Some(Len::pct(100.0 - self.th.safe.1 * 100.0 - 16.0));
        g.layout.anchor = (0.0, 1.0);
        g.layout.pad = [18.0, 30.0, 18.0, 34.0];
        g.style.fill = Some(lin(90.0, &hex("#000000b8"), &hex("#00000080")));
        g.style.border = Some((1.0, hex("#ffffff1f")));
        g.style.radius = 16.0;
        g.style.overflow_clip = true;
        g.style.shadows = vec![Shadow { y: 18.0, blur: 48.0, color: Some(hex("#00000059")), ..Default::default() }];
        let gi = self.add(Some(ov), g);
        let id = self.lid(gi);
        let ac = self.c("accent2");
        self.bar(gi, format!("{id}-bar"), &ac, 6.0);
        if let Some(ic) = praw(n, "icon") {
            let f = self.accent_tile();
            let p = self.c("paper");
            self.icon_tile(gi, format!("{id}-icon"), &ic, 52.0, f, &p, n);
        }
        let c = self.col(format!("{id}-txt"), 4.0);
        let ci = self.add(Some(gi), c);
        let name = text_arg(n).unwrap_or_default();
        self.text_l(ci, format!("{id}-name"), &name, TextSpec { size: 42.0, weight: 750.0, color: hex("#ffffff"), align: "left", tracking: Some(-0.01), upper: false, nowrap: true, lh: Some(1.15) });
        if let Some(s) = praw(n, "sub") {
            let m = self.c("mint");
            self.text_l(ci, format!("{id}-role"), &s, TextSpec { size: 24.0, weight: 650.0, color: m, align: "left", tracking: Some(0.06), upper: true, nowrap: true, lh: None });
        }
        gi
    }

    fn pill(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let c = self.color(n, "color").unwrap_or_else(|| self.c("accent2"));
        let tc = if self.light() { c.clone() } else { Rgba { r: (c.r + 1.0) / 2.0, g: (c.g + 1.0) / 2.0, b: (c.b + 1.0) / 2.0, a: 1.0 } };
        let mut p = Layer::new(lid, Kind::Text { runs: self.runs(&text_arg(n).unwrap_or_default()) });
        p.tags.push("text".into());
        p.style.size = Some(self.num(n, "size").unwrap_or(22.0));
        p.style.weight = Some(750.0);
        p.style.uppercase = true;
        p.style.tracking = Some(0.06);
        p.style.color = Some(tc);
        p.style.fill = Some(solid(&a(&c, 0.2)));
        p.style.border = Some((1.0, a(&c, 0.45)));
        p.style.radius = 999.0;
        p.style.nowrap = true;
        p.layout.pad = [6.0, 16.0, 6.0, 16.0];
        self.add(Some(parent), p)
    }

    fn quote(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let g = self.col(lid, 14.0);
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        let ac = self.c("accent2");
        self.text_l(gi, format!("{id}-mark"), "\u{201C}", TextSpec { size: 140.0, weight: 800.0, color: ac, align: "left", tracking: None, upper: false, nowrap: true, lh: Some(0.7) });
        let t = text_arg(n).unwrap_or_default();
        let size = self.num(n, "size").unwrap_or(44.0);
        let tc = self.c("text");
        let ti = self.text_l(gi, format!("{id}-text"), &t, TextSpec { size, weight: 620.0, color: tc, align: "left", tracking: Some(-0.01), upper: false, nowrap: false, lh: Some(1.25) });
        self.layers[ti].layout.maxw = Some(Len::px(if self.h > self.w { 900.0 } else { 820.0 }));
        if let Some(b) = praw(n, "by") {
            let m = self.c("mint");
            self.kicker(gi, format!("{id}-by"), &b, m);
        }
        gi
    }

    fn list(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let g = self.col(lid, 18.0);
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        let icon = praw(n, "icon").unwrap_or_else(|| "check".into());
        let items = str_args(n);
        if items.is_empty() {
            self.warn("W_EMPTY_TEXT", "list without items", "write `list \"First\" \"Second\"`", n.span);
        }
        let size = self.num(n, "size").unwrap_or(32.0);
        for (k, it) in items.iter().enumerate() {
            let r = self.row(format!("{id}-i{k}"), 18.0);
            let ri = self.add(Some(gi), r);
            let tint = solid(&a(&self.c("accent2"), 0.18));
            let m = self.c("mint");
            self.icon_tile(ri, format!("{id}-i{k}-icon"), &icon, (size * 1.2).round(), tint, &m, n);
            let tc = self.c("text");
            self.text_l(ri, format!("{id}-i{k}-text"), it, TextSpec { size, weight: 550.0, color: tc, align: "left", tracking: None, upper: false, nowrap: false, lh: Some(1.3) });
        }
        gi
    }

    fn steps(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let portrait = self.h > self.w;
        let items = str_args(n);
        let active = self.num(n, "active").unwrap_or(items.len() as f64) as usize;
        let mut g = if portrait { self.col(lid, 0.0) } else { self.row(lid, 0.0) };
        g.layout.align = if portrait { "start".into() } else { "start".into() };
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        for (k, it) in items.iter().enumerate() {
            let done = k + 1 <= active;
            if k > 0 {
                // connector
                let mut c = Layer::new(format!("{id}-c{k}"), Kind::Rect);
                if portrait {
                    c.layout.w = Some(Len::px(3.0));
                    c.layout.h = Some(Len::px(34.0));
                } else {
                    c.layout.w = Some(Len::px(120.0));
                    c.layout.h = Some(Len::px(3.0));
                }
                c.style.fill = Some(solid(&if done { self.c("accent2") } else { self.c("border") }));
                c.style.radius = 2.0;
                let ci = self.add(Some(gi), c);
                if !portrait {
                    // centered on the circles (52 px)
                    self.layers[ci].layout.pad = [0.0; 4];
                    self.set_base(ci, Ch::Y, TV::N(28.5));
                } else {
                    self.set_base(ci, Ch::X, TV::N(28.5));
                }
            }
            let mut s = if portrait { self.row(format!("{id}-s{k}"), 18.0) } else { self.col(format!("{id}-s{k}"), 14.0) };
            s.layout.align = "center".into();
            let si_ = self.add(Some(gi), s);
            let mut circ = Layer::new(format!("{id}-s{k}-dot"), Kind::Group);
            circ.layout.w = Some(Len::px(60.0));
            circ.layout.h = Some(Len::px(60.0));
            circ.layout.flow = "row";
            circ.style.radius = 30.0;
            if done {
                circ.style.fill = Some(lin(150.0, &self.c("accent2"), &self.c("accent")));
                circ.style.shadows = vec![Shadow { y: 6.0, blur: 18.0, color: Some(a(&self.c("accent2"), 0.45)), ..Default::default() }];
            } else {
                circ.style.fill = Some(solid(&self.c("surface2")));
                circ.style.border = Some((1.0, self.c("border")));
            }
            let ci = self.add(Some(si_), circ);
            let nc = if done { self.c("paper") } else { self.c("muted") };
            self.text_l(ci, format!("{id}-s{k}-n"), &format!("{}", k + 1), TextSpec { size: 26.0, weight: 800.0, color: nc, align: "center", tracking: None, upper: false, nowrap: true, lh: Some(1.0) });
            let lc = if done { self.c("text") } else { self.c("muted") };
            self.text_l(si_, format!("{id}-s{k}-label"), it, TextSpec { size: 30.0, weight: 650.0, color: lc, align: "center", tracking: None, upper: false, nowrap: true, lh: None });
        }
        gi
    }

    fn window(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let mut g = self.col(lid, 0.0);
        g.layout.align = "stretch".into();
        self.surface(&mut g, 16.0);
        g.style.overflow_clip = true;
        if !n.props.contains_key("w") {
            g.layout.w = Some(Len::px(1200.0_f64.min(self.w * (1.0 - 2.0 * self.th.safe.0))));
        }
        if !n.props.contains_key("h") {
            g.layout.h = Some(Len::px(680.0_f64.min(self.h * 0.7)));
        }
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        let mut bar = self.row(format!("{id}-bar"), 8.0);
        bar.layout.h = Some(Len::px(44.0));
        bar.layout.pad = [0.0, 18.0, 0.0, 18.0];
        bar.style.fill = Some(solid(&if self.light() { hex("#0f172a0d") } else { hex("#ffffff0f") }));
        let bi = self.add(Some(gi), bar);
        for (k, c) in ["#ff5f57", "#febc2e", "#28c840"].iter().enumerate() {
            let mut d = Layer::new(format!("{id}-tl{k}"), Kind::Ellipse);
            d.layout.w = Some(Len::px(12.0));
            d.layout.h = Some(Len::px(12.0));
            d.style.fill = Some(solid(&hex(c)));
            self.add(Some(bi), d);
        }
        if let Some(t) = praw(n, "title") {
            let mut tl = Layer::new(format!("{id}-title"), Kind::Text { runs: self.runs(&t) });
            tl.tags.push("text".into());
            tl.style.size = Some(20.0);
            tl.style.weight = Some(600.0);
            tl.style.color = Some(self.c("muted"));
            tl.style.nowrap = true;
            tl.layout.pos = Pos::Abs;
            tl.layout.x = Some(Len::pct(50.0));
            tl.layout.y = Some(Len::pct(50.0));
            tl.layout.anchor = (0.5, 0.5);
            self.add(Some(bi), tl);
        }
        let mut body = self.col(format!("{id}-body"), 18.0);
        body.layout.pad = [28.0, 28.0, 28.0, 28.0];
        body.layout.grow = 1.0;
        self.add(Some(gi), body);
        gi
    }

    fn device(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let ty = praw(n, "type").unwrap_or_else(|| "laptop".into());
        let src = praw(n, "shot").or_else(|| praw(n, "src")).unwrap_or_default();
        let path = if src.is_empty() {
            self.err("E_ASSET", "device needs shot=<png>", "write `device type=phone shot=app.png`", n.span);
            String::new()
        } else {
            self.asset(&src, n)
        };
        let (pw, ph) = crate::lower::png_size(std::path::Path::new(&path)).unwrap_or((1600.0, 1000.0));
        let (dw, bezel, rad) = match ty.as_str() {
            "phone" => (380.0, 14.0, 54.0),
            "tablet" => (760.0, 22.0, 36.0),
            _ => (1100.0, 16.0, 20.0),
        };
        let w = self.num(n, "w").unwrap_or(dw).min(self.w * (1.0 - 2.0 * self.th.safe.0));
        let mut g = self.col(lid, 0.0);
        g.layout.align = "center".into();
        g.layout.w = Some(Len::px(w));
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        let mut frame = Layer::new(format!("{id}-frame"), Kind::Group);
        frame.layout.w = Some(Len::pct(100.0));
        frame.layout.pad = [bezel; 4];
        frame.style.fill = Some(lin(180.0, &hex("#2a2f36"), &hex("#15181d")));
        frame.style.radius = rad;
        frame.style.border = Some((1.5, hex("#ffffff2e")));
        frame.style.shadows = vec![Shadow { y: 36.0, blur: 90.0, color: Some(hex("#00000080")), ..Default::default() }, Shadow { y: 2.0, blur: 4.0, color: Some(hex("#00000059")), ..Default::default() }];
        let fi = self.add(Some(gi), frame);
        let mut screen = Layer::new(format!("{id}-screen"), Kind::Group);
        screen.layout.w = Some(Len::pct(100.0));
        screen.layout.aspect = Some(pw / ph.max(1.0));
        screen.style.overflow_clip = true;
        screen.style.radius = (rad - bezel).max(4.0);
        screen.style.fill = Some(solid(&hex("#000")));
        let si_ = self.add(Some(fi), screen);
        let mut img = Layer::new(format!("{id}-shot"), Kind::Image { src: path, fit: "cover".into() });
        img.layout.pos = Pos::Fill;
        self.add(Some(si_), img);
        if ty == "phone" {
            let mut notch = Layer::new(format!("{id}-notch"), Kind::Rect);
            notch.layout.pos = Pos::Abs;
            notch.layout.x = Some(Len::pct(50.0));
            notch.layout.y = Some(Len::px(10.0));
            notch.layout.anchor = (0.5, 0.0);
            notch.layout.w = Some(Len::px(w * 0.3));
            notch.layout.h = Some(Len::px(26.0));
            notch.style.fill = Some(solid(&hex("#000")));
            notch.style.radius = 13.0;
            self.add(Some(si_), notch);
        }
        if ty == "laptop" {
            let mut base = Layer::new(format!("{id}-base"), Kind::Rect);
            base.layout.w = Some(Len::pct(116.0));
            base.layout.h = Some(Len::px(20.0));
            base.style.fill = Some(lin(180.0, &hex("#d4d8de"), &hex("#8d939b")));
            base.style.radius = 10.0;
            base.style.shadows = vec![Shadow { y: 18.0, blur: 40.0, color: Some(hex("#00000066")), ..Default::default() }];
            self.add(Some(gi), base);
        }
        gi
    }

    /// Charts with axes, gridlines, tick labels, value labels and legends (parts keep the S0 ids:
    /// `-c{k}` `-v{k}` `-b{k}` `-line` `-area` `-d{k}` `-s{k}` `-lab{k}`).
    pub fn chart2(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let ty = praw(n, "type").unwrap_or_else(|| "bar".into());
        let data = nums(n, "data");
        if data.is_empty() {
            self.err("E_CHART", "chart needs data=[...]", "write `chart type=bar data=[3, 5, 8] labels=[\"A\", \"B\", \"C\"]`", n.span);
        }
        let labels: Vec<String> = match n.props.get("labels") {
            Some(Value::List(v)) => v.iter().map(|x| x.raw()).collect(),
            _ => vec![],
        };
        if !labels.is_empty() && labels.len() != data.len() {
            self.err("E_CHART", format!("{} labels for {} values", labels.len(), data.len()), "give one label per data value", n.span);
        }
        let w = self.num(n, "w").unwrap_or(1100.0).min(self.w * (1.0 - 2.0 * self.th.safe.0));
        let h = self.num(n, "h").unwrap_or(520.0);
        let dmax = data.iter().cloned().fold(0.0, f64::max);
        let (nice_max, step) = nice_axis(self.num(n, "max").unwrap_or(dmax));
        let max = self.num(n, "max").unwrap_or(nice_max);
        let col = self.color(n, "color").unwrap_or_else(|| self.c("accent2"));
        let unit = praw(n, "unit").unwrap_or_default();
        let grid = praw(n, "grid").map(|v| v != "false").unwrap_or(true);
        let axis = praw(n, "axis").map(|v| v != "false").unwrap_or(true) && matches!(ty.as_str(), "bar" | "line" | "area");
        let highlight = self.num(n, "highlight").map(|v| v as usize);
        let de = praw(n, "locale").map(|l| l.starts_with("de")).unwrap_or(true);
        let mut g = Layer::new(lid, Kind::Group);
        g.layout.w = Some(Len::px(w));
        g.layout.h = Some(Len::px(h));
        g.tags.push(format!("chart-{ty}"));
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        let nn = data.len().max(1) as f64;
        let (label_h, val_h) = (40.0, 46.0);
        let x0 = if axis { 70.0 } else { 0.0 };
        let pw = w - x0;
        let yb = h - label_h; // baseline
        let plot_h = yb - val_h;
        let y_of = |v: f64| yb - (v / max.max(1e-9)).clamp(0.0, 1.0) * plot_h;
        let line_c = if self.light() { hex("#0f172a1a") } else { hex("#ffffff1a") };
        if matches!(ty.as_str(), "bar" | "line" | "area") {
            let mut ticks = vec![];
            let mut v = 0.0;
            while v <= max + 1e-9 && ticks.len() < 12 {
                ticks.push(v);
                v += step;
            }
            for (k, tv) in ticks.iter().enumerate() {
                let y = y_of(*tv);
                if grid && k > 0 {
                    let mut gl = Layer::new(format!("{id}-grid{k}"), Kind::Rect);
                    gl.layout.pos = Pos::Abs;
                    gl.layout.x = Some(Len::px(x0));
                    gl.layout.y = Some(Len::px(y - 0.5));
                    gl.layout.w = Some(Len::px(pw));
                    gl.layout.h = Some(Len::px(1.0));
                    gl.style.fill = Some(solid(&line_c));
                    gl.tags.push("grid".into());
                    self.add(Some(gi), gl);
                }
                if axis {
                    let mut tl = Layer::new(format!("{id}-tick{k}"), Kind::Text { runs: vec![Run { text: fmt_tick(*tv, &unit, de), ..Default::default() }] });
                    tl.layout.pos = Pos::Abs;
                    tl.layout.x = Some(Len::px(x0 - 14.0));
                    tl.layout.y = Some(Len::px(y));
                    tl.layout.anchor = (1.0, 0.5);
                    tl.style.size = Some(22.0);
                    tl.style.weight = Some(550.0);
                    tl.style.color = Some(self.c("faint"));
                    tl.style.text_align = Some("right".into());
                    tl.style.nowrap = true;
                    tl.tags.push("text".into());
                    tl.tags.push("tick".into());
                    self.add(Some(gi), tl);
                }
            }
            let mut base = Layer::new(format!("{id}-baseline"), Kind::Rect);
            base.layout.pos = Pos::Abs;
            base.layout.x = Some(Len::px(x0));
            base.layout.y = Some(Len::px(yb - 1.0));
            base.layout.w = Some(Len::px(pw));
            base.layout.h = Some(Len::px(2.0));
            base.style.fill = Some(solid(&self.c("border")));
            base.tags.push("grid".into());
            self.add(Some(gi), base);
        }
        match ty.as_str() {
            "bar" => {
                let gap = (pw / nn * 0.28).clamp(12.0, 44.0);
                // Fixed geometry (no flex): column boxes depend only on the data count and the chart
                // width, never on label content, so a counting value label cannot move a bar.
                let colw = ((pw - gap) - gap * (nn - 1.0)) / nn;
                let mut area = Layer::new(format!("{id}-plot"), Kind::Group);
                area.layout.pos = Pos::Abs;
                area.layout.x = Some(Len::px(x0));
                area.layout.y = Some(Len::px(0.0));
                area.layout.w = Some(Len::px(pw));
                area.layout.h = Some(Len::px(yb));
                area.style.overflow_clip = true;
                let ai = self.add(Some(gi), area);
                let mut row = Layer::new(format!("{id}-bars"), Kind::Group);
                row.layout.pos = Pos::Abs;
                row.layout.x = Some(Len::px(0.0));
                row.layout.y = Some(Len::px(0.0));
                row.layout.w = Some(Len::px(pw));
                row.layout.h = Some(Len::px(yb + 12.0));
                let ri = self.add(Some(ai), row);
                for (k, v) in data.iter().enumerate() {
                    let bh = (v / max.max(1e-9)).clamp(0.0, 1.0) * plot_h + 12.0;
                    let mut c = Layer::new(format!("{id}-c{k}"), Kind::Group);
                    c.layout.pos = Pos::Abs;
                    c.layout.x = Some(Len::px(gap / 2.0 + k as f64 * (colw + gap)));
                    c.layout.y = Some(Len::px(0.0));
                    c.layout.w = Some(Len::px(colw));
                    c.layout.h = Some(Len::px(yb + 12.0));
                    let ci = self.add(Some(ri), c);
                    let hi = highlight == Some(k + 1);
                    let bc = if hi { self.c("mint") } else { col.clone() };
                    let mut b = Layer::new(format!("{id}-b{k}"), Kind::Rect);
                    b.layout.pos = Pos::Abs;
                    b.layout.x = Some(Len::px(0.0));
                    b.layout.y = Some(Len::px(yb + 12.0 - bh));
                    b.layout.w = Some(Len::px(colw));
                    b.layout.h = Some(Len::px(bh));
                    let plain = matches!(self.th.d.surface.as_str(), "flat" | "outline") || self.th.d.shadows.as_ref().is_some_and(|s| s.is_empty());
                    b.style.fill = Some(if plain { solid(&bc) } else { lin(180.0, &bc, &a(&bc, 0.55)) });
                    b.style.radius = self.th.d.radius_sm.unwrap_or(10.0).min(10.0);
                    b.style.origin = (0.5, 1.0);
                    if hi && !plain {
                        b.style.glow_color = Some(self.c("mint"));
                        b.style.shadows = vec![Shadow { y: 0.0, blur: 28.0, color: Some(a(&self.c("mint"), 0.45)), ..Default::default() }];
                    }
                    b.tags.push("bar".into());
                    self.add(Some(ci), b);
                    // value label: absolute, centered on the column, bottom edge 10 px above the bar top;
                    // the enter animation rides it on the growing bar (tag ride:<bar height>)
                    let mut val = Layer::new(format!("{id}-v{k}"), Kind::Counter { prefix: String::new(), suffix: unit.clone() });
                    val.layout.pos = Pos::Abs;
                    val.layout.x = Some(Len::px(colw / 2.0));
                    val.layout.y = Some(Len::px(yb + 12.0 - bh - 10.0));
                    val.layout.anchor = (0.5, 1.0);
                    val.style.size = Some(30.0);
                    val.style.weight = Some(750.0);
                    val.style.color = Some(self.c("text"));
                    val.style.text_align = Some("center".into());
                    val.style.nowrap = true;
                    val.tags.push("text".into());
                    val.tags.push(format!("ride:{bh:.2}"));
                    val.tags.push(format!("col:{k}"));
                    let vi = self.add(Some(ci), val);
                    self.set_base(vi, Ch::Count, TV::N(*v));
                    if let (true, Some(note)) = (hi, praw(n, "note")) {
                        // annotation on the highlighted value: a pill above its value label
                        let mut nl = Layer::new(format!("{id}-note"), Kind::Text { runs: self.runs(&note) });
                        nl.tags.push("text".into());
                        nl.tags.push("note".into());
                        nl.layout.pos = Pos::Abs;
                        nl.layout.x = Some(Len::px(colw / 2.0));
                        nl.layout.y = Some(Len::px(yb + 12.0 - bh - 56.0));
                        nl.layout.anchor = (0.5, 1.0);
                        nl.layout.pad = [6.0, 14.0, 6.0, 14.0];
                        nl.style.size = Some(24.0);
                        nl.style.weight = Some(self.th.d.w_label);
                        nl.style.color = Some(if self.light() { self.c("paper") } else { self.c("ink") });
                        nl.style.fill = Some(solid(&self.c("mint")));
                        nl.style.radius = self.th.d.radius_sm.unwrap_or(999.0);
                        nl.style.nowrap = true;
                        self.add(Some(ci), nl);
                    }
                }
                // x labels: same flex distribution as the bars
                let mut lr = Layer::new(format!("{id}-labels"), Kind::Group);
                lr.layout.pos = Pos::Abs;
                lr.layout.x = Some(Len::px(x0 + gap / 2.0));
                lr.layout.y = Some(Len::px(yb + 10.0));
                lr.layout.w = Some(Len::px(pw - gap));
                lr.layout.flow = "row";
                lr.layout.gap = gap;
                let li = self.add(Some(gi), lr);
                for k in 0..data.len() {
                    let mut lab = Layer::new(format!("{id}-lab{k}"), Kind::Text { runs: vec![Run { text: labels.get(k).cloned().unwrap_or_default(), ..Default::default() }] });
                    lab.style.size = Some(26.0);
                    lab.style.weight = Some(550.0);
                    lab.style.color = Some(self.c("muted"));
                    lab.style.text_align = Some("center".into());
                    lab.style.nowrap = true;
                    lab.layout.w = Some(Len::px(colw));
                    lab.tags.push("text".into());
                    self.add(Some(li), lab);
                }
            }
            "line" | "area" => {
                let pts: Vec<(f64, f64)> = data.iter().enumerate().map(|(k, v)| {
                    let x = if data.len() > 1 { x0 + 24.0 + k as f64 / (data.len() - 1) as f64 * (pw - 48.0) } else { x0 + pw / 2.0 };
                    (x, y_of(*v))
                }).collect();
                let d = smooth(&pts);
                if ty == "area" && !pts.is_empty() {
                    let mut ar = Layer::new(format!("{id}-area"), Kind::Path { d: format!("{d} L{:.1} {:.1} L{:.1} {:.1} Z", pts.last().unwrap().0, yb, pts[0].0, yb), closed: true, vb: (w, h) });
                    ar.layout.pos = Pos::Fill;
                    ar.style.fill = Some(lin(180.0, &a(&col, 0.42), &a(&col, 0.0)));
                    ar.style.clip = Some(ClipShape::Wipe("left".into()));
                    ar.tags.push("area".into());
                    self.add(Some(gi), ar);
                }
                let mut l = Layer::new(format!("{id}-line"), Kind::Path { d, closed: false, vb: (w, h) });
                l.layout.pos = Pos::Fill;
                l.style.stroke = Some(col.clone());
                l.style.stroke_w = 5.0;
                l.tags.push("line".into());
                self.add(Some(gi), l);
                for (k, (x, y)) in pts.iter().enumerate() {
                    let hi = highlight == Some(k + 1);
                    let mut dot = Layer::new(format!("{id}-d{k}"), Kind::Ellipse);
                    dot.layout.pos = Pos::Abs;
                    dot.layout.x = Some(Len::px(*x));
                    dot.layout.y = Some(Len::px(*y));
                    dot.layout.anchor = (0.5, 0.5);
                    let ds = if hi { 24.0 } else { 16.0 };
                    dot.layout.w = Some(Len::px(ds));
                    dot.layout.h = Some(Len::px(ds));
                    dot.style.fill = Some(solid(&self.c("bg")));
                    dot.style.border = Some((4.0, if hi { self.c("mint") } else { col.clone() }));
                    dot.tags.push("dot".into());
                    self.add(Some(gi), dot);
                    if flag(n, "values") || hi {
                        let mut t = Layer::new(format!("{id}-val{k}"), Kind::Text { runs: vec![Run { text: format!("{}{}", fmt_val(data[k], de), unit), ..Default::default() }] });
                        t.layout.pos = Pos::Abs;
                        t.layout.x = Some(Len::px(*x));
                        t.layout.y = Some(Len::px(y - 16.0));
                        t.layout.anchor = (0.5, 1.0);
                        t.style.size = Some(27.0);
                        t.style.weight = Some(750.0);
                        t.style.color = Some(if hi { self.c("mint") } else { self.c("text") });
                        t.style.nowrap = true;
                        t.tags.push("text".into());
                        t.tags.push("dot".into());
                        self.add(Some(gi), t);
                    }
                    if let Some(lab) = labels.get(k) {
                        let mut t = Layer::new(format!("{id}-lab{k}"), Kind::Text { runs: vec![Run { text: lab.clone(), ..Default::default() }] });
                        t.layout.pos = Pos::Abs;
                        t.layout.x = Some(Len::px(*x));
                        t.layout.y = Some(Len::px(yb + 10.0));
                        t.layout.anchor = (0.5, 0.0);
                        t.style.size = Some(26.0);
                        t.style.weight = Some(550.0);
                        t.style.color = Some(self.c("muted"));
                        t.style.nowrap = true;
                        t.tags.push("text".into());
                        self.add(Some(gi), t);
                    }
                }
            }
            "pie" | "donut" => {
                let total: f64 = data.iter().sum::<f64>().max(1e-9);
                let r = (h.min(w * 0.5) / 2.0 - 20.0).max(10.0);
                let (cx, cy) = (r + 20.0, h / 2.0);
                let palette = ["accent2", "mint", "warn", "danger", "muted", "accent"];
                let mut acc = 0.0;
                let sw = if ty == "pie" { r } else { r * 0.34 };
                let rr = if ty == "pie" { r / 2.0 } else { r - sw / 2.0 };
                for (k, v) in data.iter().enumerate() {
                    let a0 = acc / total * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
                    acc += v;
                    let a1 = acc / total * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
                    let large = if a1 - a0 > std::f64::consts::PI { 1 } else { 0 };
                    let d = format!("M{:.1} {:.1} A{rr:.1} {rr:.1} 0 {large} 1 {:.1} {:.1}", cx + rr * a0.cos(), cy + rr * a0.sin(), cx + rr * a1.cos(), cy + rr * a1.sin());
                    let mut s = Layer::new(format!("{id}-s{k}"), Kind::Path { d, closed: false, vb: (w, h) });
                    s.layout.pos = Pos::Fill;
                    let c = self.c(palette[k % palette.len()]);
                    s.style.stroke = Some(c.clone());
                    s.style.stroke_w = sw;
                    s.tags.push("slice".into());
                    self.add(Some(gi), s);
                }
                if ty == "donut" {
                    // center: the largest slice (value + label)
                    let (bk, bv) = data.iter().enumerate().fold((0usize, f64::MIN), |acc, (k, v)| if *v > acc.1 { (k, *v) } else { acc });
                    let mut cc = self.col(format!("{id}-center"), 2.0);
                    cc.layout.pos = Pos::Abs;
                    cc.layout.x = Some(Len::px(cx));
                    cc.layout.y = Some(Len::px(cy));
                    cc.layout.anchor = (0.5, 0.5);
                    cc.layout.align = "center".into();
                    let cci = self.add(Some(gi), cc);
                    let mut tot = Layer::new(format!("{id}-total"), Kind::Counter { prefix: String::new(), suffix: unit.clone() });
                    tot.style.size = Some((r * 0.4).clamp(30.0, 64.0));
                    tot.style.weight = Some(800.0);
                    tot.style.tracking = Some(-0.03);
                    tot.style.color = Some(self.c("text"));
                    tot.style.nowrap = true;
                    tot.style.text_align = Some("center".into());
                    tot.tags.push("text".into());
                    let ti = self.add(Some(cci), tot);
                    self.set_base(ti, Ch::Count, TV::N(bv.round()));
                    if let Some(lb) = labels.get(bk) {
                        let m = self.c("muted");
                        self.text_l(cci, format!("{id}-center-label"), lb, TextSpec { size: 22.0, weight: 650.0, color: m, align: "center", tracking: Some(0.04), upper: true, nowrap: true, lh: None });
                    }
                }
                // legend
                let mut lg = self.col(format!("{id}-legend"), 16.0);
                lg.layout.pos = Pos::Abs;
                lg.layout.x = Some(Len::px(2.0 * r + 90.0));
                lg.layout.y = Some(Len::pct(50.0));
                lg.layout.anchor = (0.0, 0.5);
                let lgi = self.add(Some(gi), lg);
                for (k, v) in data.iter().enumerate() {
                    let c = self.c(palette[k % palette.len()]);
                    let r_ = self.row(format!("{id}-lab{k}"), 14.0);
                    let ri = self.add(Some(lgi), r_);
                    let mut sw_ = Layer::new(format!("{id}-sw{k}"), Kind::Ellipse);
                    sw_.layout.w = Some(Len::px(16.0));
                    sw_.layout.h = Some(Len::px(16.0));
                    sw_.style.fill = Some(solid(&c));
                    self.add(Some(ri), sw_);
                    let tc = self.c("text");
                    self.text_l(ri, format!("{id}-lab{k}-t"), labels.get(k).map(|s| s.as_str()).unwrap_or(""), TextSpec { size: 32.0, weight: 650.0, color: tc, align: "left", tracking: None, upper: false, nowrap: true, lh: None });
                    let mc = self.c("muted");
                    let pct = (v / total * 100.0).round();
                    let vt = if unit.trim() == "%" || (total - 100.0).abs() < 0.5 { format!("{} %", fmt_val(*v, de)) } else { format!("{}{} ({} %)", fmt_val(*v, de), unit, pct) };
                    self.text_l(ri, format!("{id}-lab{k}-v"), &vt, TextSpec { size: 30.0, weight: 500.0, color: mc, align: "left", tracking: None, upper: false, nowrap: true, lh: None });
                }
            }
            _ => self.err("E_CHART", format!("unknown chart type `{ty}`"), "type=bar|line|area|pie|donut", n.span),
        }
        gi
    }
}
