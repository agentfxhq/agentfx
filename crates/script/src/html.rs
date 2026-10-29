//! HTML backend: Motion IR -> one self-contained HTML page (CSS @keyframes on registered
//! custom properties, no animation library). Seekable: `window.__reel.seek(t)` pauses every
//! animation at t (Web Animations API), so Chrome is a deterministic reference renderer.
//! Load-time hooks (magic move, connectors, auto-fit) run once before frame 0.
use crate::ir::*;
use crate::values::{ease_to_css, ColorRef, Ease, Paint, Rgba, Stop};
use base64::Engine;
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;
use std::path::Path;

pub struct HtmlOpts {
    pub embed: bool,
    pub fonts_dir: std::path::PathBuf,
    /// S1 native layout: a static snapshot (no animation, no runtime, no @property) with every
    /// channel written as its literal value (`Snap::vals`, default = channel default). Composite
    /// channels and effects (transforms except the anchor, opacity, filters, masks, clip-path,
    /// blend, 3D) are left out: the reel-ir nodes carry them. Layout/paint channels stay in.
    pub snap: Option<Snap>,
}

#[derive(Clone, Default)]
pub struct Snap {
    pub vals: HashMap<(String, Ch), TV>,
    /// font-size overrides (the `fit` hook, resolved natively)
    pub font_size: HashMap<String, f64>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
/// Number as CSS writes it (4 decimals, trailing zeros trimmed).
pub fn fnum(v: f64) -> String {
    f(v)
}
fn f(v: f64) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" { "0".into() } else { s }
}
fn css_color(c: &Rgba) -> String {
    c.to_css()
}
fn stop_css(s: &Stop) -> String {
    let c = match &s.color {
        ColorRef::Lit(c) => css_color(c),
        ColorRef::Token(t) => format!("var(--t-{t})"),
    };
    match s.pos {
        Some(p) => format!("{c} {}%", f(p * 100.0)),
        None => c,
    }
}

fn var(ch: Ch) -> String {
    format!("--r-{}", ch.name())
}

fn chan_val(ch: Ch, v: &TV) -> String {
    match v {
        TV::C(c) => css_color(c),
        TV::N(n) => match ch {
            Ch::X | Ch::Y | Ch::Z | Ch::W | Ch::H | Ch::Blur | Ch::Glow => format!("{}px", f(*n)),
            Ch::Rot | Ch::RotX | Ch::RotY | Ch::Hue | Ch::GradAngle => format!("{}deg", f(*n)),
            Ch::Count => format!("{}", n.round() as i64),
            _ => f(*n),
        },
    }
}

fn syntax(ch: Ch) -> &'static str {
    match ch {
        Ch::X | Ch::Y | Ch::Z | Ch::W | Ch::H | Ch::Blur | Ch::Glow => "<length>",
        Ch::Rot | Ch::RotX | Ch::RotY | Ch::Hue | Ch::GradAngle => "<angle>",
        Ch::Count => "<integer>",
        c if c.is_color() => "<color>",
        _ => "<number>",
    }
}

pub struct Page {
    css: String,
    body: String,
    anims: HashMap<String, Vec<String>>,
    inline_vars: HashMap<String, Vec<(Ch, String)>>,
    chans: HashMap<String, BTreeSet<Ch>>,
    kf: usize,
    embed: bool,
    img_cache: HashMap<String, String>,
    duration: f64,
    snap: Option<Snap>,
    cur_layer: String,
}

/// `svg w= h=`: scale the root `<svg>` to the layer box (viewBox fit, `preserveAspectRatio`
/// as authored, default xMidYMid meet) instead of cropping it at its own width/height. A root
/// without a viewBox gets one from its numeric width/height. Roots already at 100% are kept.
pub fn fit_svg(markup: &str, w_set: bool, h_set: bool) -> String {
    let lower = markup.to_ascii_lowercase();
    let Some(st) = lower.find("<svg") else { return markup.to_string() };
    // end of the start tag (quotes respected)
    let b = markup.as_bytes();
    let mut q: Option<u8> = None;
    let mut end = None;
    for (i, &c) in b.iter().enumerate().skip(st + 4) {
        match q {
            Some(x) if c == x => q = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => q = Some(c),
            None if c == b'>' => {
                end = Some(i);
                break;
            }
            None => {}
        }
    }
    let Some(end) = end else { return markup.to_string() };
    let selfclose = b[end - 1] == b'/';
    let inner = &markup[st + 4..if selfclose { end - 1 } else { end }];
    // attributes: name=value pairs (quoted or bare) and bare names
    let mut attrs: Vec<(String, Option<String>)> = vec![];
    let ib = inner.as_bytes();
    let mut i = 0;
    while i < ib.len() {
        while i < ib.len() && ib[i].is_ascii_whitespace() {
            i += 1;
        }
        let n0 = i;
        while i < ib.len() && !ib[i].is_ascii_whitespace() && ib[i] != b'=' {
            i += 1;
        }
        if i == n0 {
            i += 1;
            continue;
        }
        let name = inner[n0..i].to_string();
        while i < ib.len() && ib[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < ib.len() && ib[i] == b'=' {
            i += 1;
            while i < ib.len() && ib[i].is_ascii_whitespace() {
                i += 1;
            }
            let v = if i < ib.len() && (ib[i] == b'"' || ib[i] == b'\'') {
                let qc = ib[i];
                let v0 = i + 1;
                i = v0;
                while i < ib.len() && ib[i] != qc {
                    i += 1;
                }
                let v = inner[v0..i.min(ib.len())].to_string();
                i += 1;
                v
            } else {
                let v0 = i;
                while i < ib.len() && !ib[i].is_ascii_whitespace() {
                    i += 1;
                }
                inner[v0..i].to_string()
            };
            attrs.push((name, Some(v)));
        } else {
            attrs.push((name, None));
        }
    }
    let get = |k: &str| attrs.iter().find(|a| a.0.eq_ignore_ascii_case(k)).and_then(|a| a.1.clone());
    let (w, h) = (get("width"), get("height"));
    if w.as_deref().map(str::trim) == Some("100%") && h.as_deref().map(str::trim) == Some("100%") {
        return markup.to_string();
    }
    let num = |v: &Option<String>| v.as_deref().map(|s| s.trim().trim_end_matches("px")).and_then(|s| s.parse::<f64>().ok()).filter(|v| *v > 0.0);
    let mut out_attrs: Vec<String> = vec![];
    if get("viewBox").is_none() {
        match (num(&w), num(&h)) {
            (Some(a), Some(b)) => out_attrs.push(format!("viewBox=\"0 0 {} {}\"", f(a), f(b))),
            _ => return markup.to_string(), // nothing to scale by: keep the authored size
        }
    }
    for (k, v) in &attrs {
        if k.eq_ignore_ascii_case("width") || k.eq_ignore_ascii_case("height") {
            continue;
        }
        if k.eq_ignore_ascii_case("style") {
            out_attrs.push(format!("style=\"display:block;{}\"", v.clone().unwrap_or_default().replace('"', "'")));
            continue;
        }
        out_attrs.push(match v {
            Some(v) => format!("{k}=\"{}\"", v.replace('"', "&quot;")),
            None => k.clone(),
        });
    }
    if !attrs.iter().any(|a| a.0.eq_ignore_ascii_case("style")) {
        out_attrs.push("style=\"display:block\"".into());
    }
    if w_set {
        out_attrs.push("width=\"100%\"".into());
    }
    if h_set {
        out_attrs.push("height=\"100%\"".into());
    }
    format!("{}<svg {}{}>{}", &markup[..st], out_attrs.join(" "), if selfclose { "/" } else { "" }, &markup[end + 1..])
}

fn paint_css(p: &Paint, page: &mut Page, chans: &BTreeSet<Ch>) -> String {
    match p {
        Paint::Solid(c) => match c {
            ColorRef::Lit(c) => css_color(c),
            ColorRef::Token(t) => format!("var(--t-{t})"),
        },
        Paint::Linear { angle_deg, stops } => {
            let animated = chans.iter().any(|c| matches!(c, Ch::GradAngle | Ch::Grad0 | Ch::Grad1 | Ch::Grad2 | Ch::Grad3));
            if animated {
                let gs = [Ch::Grad0, Ch::Grad1, Ch::Grad2, Ch::Grad3];
                let snapv = |c: Ch| -> String {
                    match &page.snap {
                        Some(sn) => chan_val(c, &sn.vals.get(&(page.cur_layer.clone(), c)).cloned().unwrap_or(c.default_value())),
                        None => format!("var({})", var(c)),
                    }
                };
                let parts: Vec<String> = stops.iter().enumerate().map(|(k, s)| {
                    let c = if k < 4 { snapv(gs[k]) } else { stop_css(s) };
                    match s.pos {
                        Some(p) if k < 4 => format!("{c} {}%", f(p * 100.0)),
                        _ => c,
                    }
                }).collect();
                format!("linear-gradient({}, {})", snapv(Ch::GradAngle), parts.join(", "))
            } else {
                format!("linear-gradient({}deg, {})", f(*angle_deg), stops.iter().map(stop_css).collect::<Vec<_>>().join(", "))
            }
        }
        Paint::Radial { cx, cy, stops } => format!("radial-gradient(circle at {}% {}%, {})", f(cx * 100.0), f(cy * 100.0), stops.iter().map(stop_css).collect::<Vec<_>>().join(", ")),
        Paint::Conic { angle_deg, stops } => format!("conic-gradient(from {}deg, {})", f(*angle_deg), stops.iter().map(stop_css).collect::<Vec<_>>().join(", ")),
        Paint::Mesh { cols, rows, colors } if colors.iter().all(|c| matches!(c, ColorRef::Lit(_))) => {
            // true mesh gradient: one shared raster (src/mesh.rs) stretched over the box
            let lit: Vec<crate::values::Rgba> = colors.iter().filter_map(|c| if let ColorRef::Lit(c) = c { Some(c.clone()) } else { None }).collect();
            match crate::mesh::png(*cols, *rows, &lit) {
                Ok(p) => format!("url(\"{}\") 0 0 / 100% 100% no-repeat", page.url(&p.to_string_lossy())),
                Err(_) => lit.first().map(css_color).unwrap_or("#000".into()),
            }
        }
        Paint::Mesh { cols, rows, colors } => {
            // approximation: one soft radial blob per mesh point
            let mut parts = vec![];
            for (k, c) in colors.iter().enumerate() {
                let x = if *cols > 1 { (k as u32 % cols) as f64 / (*cols - 1) as f64 * 100.0 } else { 50.0 };
                let y = if *rows > 1 { (k as u32 / cols) as f64 / (*rows - 1) as f64 * 100.0 } else { 50.0 };
                let col = match c {
                    ColorRef::Lit(c) => css_color(c),
                    ColorRef::Token(t) => format!("var(--t-{t})"),
                };
                parts.push(format!("radial-gradient(circle at {}% {}%, {col} 0%, transparent 70%)", f(x), f(y)));
            }
            let first = colors.first().map(|c| match c {
                ColorRef::Lit(c) => css_color(c),
                _ => "#000".into(),
            }).unwrap_or("#000".into());
            format!("{}, {first}", parts.join(", "))
        }
        Paint::Image { src, fit } => {
            let url = page.url(src);
            format!("url(\"{url}\") center / {} no-repeat", if fit == "fill" { "100% 100%" } else { fit.as_str() })
        }
    }
}

impl Page {
    fn url(&mut self, src: &str) -> String {
        if src.starts_with("http") || src.starts_with("data:") {
            return src.to_string();
        }
        if !self.embed {
            return format!("file://{}", src);
        }
        if let Some(u) = self.img_cache.get(src) {
            return u.clone();
        }
        let mime = match Path::new(src).extension().and_then(|e| e.to_str()).unwrap_or("") {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "svg" => "image/svg+xml",
            "webp" => "image/webp",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            _ => "application/octet-stream",
        };
        let u = match std::fs::read(src) {
            Ok(b) => format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(b)),
            Err(_) => format!("file://{src}"),
        };
        self.img_cache.insert(src.to_string(), u.clone());
        u
    }

    fn keyframes(&mut self, t: &Track) -> String {
        self.kf += 1;
        let name = format!("k{}", self.kf);
        let total = self.duration.max(1e-6);
        let v = var(t.ch);
        let mut s = format!("@keyframes {name}{{");
        let keys = &t.keys;
        let pct = |x: f64| f((x / total * 100.0).clamp(0.0, 100.0));
        if keys[0].t > 1e-6 {
            let _ = write!(s, "0%{{{v}:{};animation-timing-function:linear}}", chan_val(t.ch, &keys[0].v));
        }
        for k in keys {
            let tf = match &k.ease {
                Ease::Linear => "linear".to_string(),
                e => ease_to_css(e),
            };
            let _ = write!(s, "{}%{{{v}:{};animation-timing-function:{tf}}}", pct(k.t), chan_val(t.ch, &k.v));
        }
        if keys[keys.len() - 1].t < total - 1e-6 {
            let _ = write!(s, "100%{{{v}:{}}}", chan_val(t.ch, &keys[keys.len() - 1].v));
        }
        s.push('}');
        self.css.push_str(&s);
        self.css.push('\n');
        name
    }

    /// `var(--r-ch)` (animated page) or the literal value at the snapshot time.
    fn cv(&self, l: &Layer, ch: Ch) -> String {
        match &self.snap {
            None => format!("var({})", var(ch)),
            Some(s) => chan_val(ch, &s.vals.get(&(l.id.clone(), ch)).cloned().unwrap_or(ch.default_value())),
        }
    }

    fn style_of(&mut self, l: &Layer, parent_flow: &str) -> String {
        let mut st = String::new();
        let chans = self.chans.get(&l.id).cloned().unwrap_or_default();
        let has = |c: Ch| chans.contains(&c);
        let snap = self.snap.is_some();
        self.cur_layer = l.id.clone();
        let lay = &l.layout;
        let len = |x: &Len| match x.unit {
            "%" => format!("{}%", f(x.v)),
            "em" => format!("{}em", f(x.v)),
            _ => format!("{}px", f(x.v)),
        };
        match lay.pos {
            Pos::Fill => st.push_str("position:absolute;inset:0;"),
            Pos::Abs => {
                st.push_str("position:absolute;");
                if let Some(x) = &lay.x {
                    let _ = write!(st, "left:{};", len(x));
                }
                if let Some(y) = &lay.y {
                    let _ = write!(st, "top:{};", len(y));
                }
                if lay.x.is_none() && lay.y.is_none() {
                    st.push_str("left:0;top:0;");
                }
            }
            Pos::Flow => st.push_str("position:relative;"),
        }
        if let Some(w) = &lay.w {
            if !has(Ch::W) {
                let _ = write!(st, "width:{};", len(w));
            }
        }
        if let Some(h) = &lay.h {
            if has(Ch::LineH) {
                let _ = write!(st, "height:calc({} * {});overflow:hidden;", self.cv(l, Ch::LineH), len(h));
            } else if !has(Ch::H) {
                let _ = write!(st, "height:{};", len(h));
            }
        }
        if has(Ch::W) {
            let _ = write!(st, "width:{};", self.cv(l, Ch::W));
        }
        if has(Ch::H) {
            let _ = write!(st, "height:{};", self.cv(l, Ch::H));
        }
        if let Some(m) = &lay.maxw {
            let _ = write!(st, "max-width:{};", len(m));
        }
        if let Some(a) = lay.aspect {
            let _ = write!(st, "aspect-ratio:{};", f(a));
        }
        if lay.grow > 0.0 {
            let _ = write!(st, "flex-grow:{};min-width:0;", f(lay.grow));
        }
        if parent_flow != "none" && parent_flow != "inline" && lay.pos == Pos::Flow {
            st.push_str("flex-shrink:0;");
        }
        if lay.pad.iter().any(|p| *p != 0.0) {
            let _ = write!(st, "padding:{}px {}px {}px {}px;", f(lay.pad[0]), f(lay.pad[1]), f(lay.pad[2]), f(lay.pad[3]));
        }
        let al = |s: &str| match s {
            "start" | "left" | "top" => "flex-start",
            "end" | "right" | "bottom" => "flex-end",
            "between" => "space-between",
            "stretch" => "stretch",
            "baseline" => "baseline",
            _ => "center",
        };
        match lay.flow {
            "row" | "col" => {
                let _ = write!(st, "display:flex;flex-direction:{};gap:{}px;align-items:{};justify-content:{};", if lay.flow == "row" { "row" } else { "column" }, f(lay.gap), al(&lay.align), al(&lay.justify));
                if lay.wrap {
                    st.push_str("flex-wrap:wrap;");
                }
            }
            "grid" => {
                let _ = write!(st, "display:grid;grid-template-columns:repeat({},minmax(0,1fr));gap:{}px;align-items:{};", lay.cols.max(1), f(lay.gap), al(&lay.align));
            }
            "inline" => {
                let _ = write!(st, "display:flex;flex-wrap:{};align-items:baseline;justify-content:{};", if l.style.nowrap { "nowrap" } else { "wrap" }, al(&lay.justify));
            }
            _ => {}
        }
        // ---- style
        let s = &l.style;
        let is_box = matches!(l.kind, Kind::Group | Kind::Rect | Kind::Ellipse | Kind::Image { .. } | Kind::Video { .. } | Kind::Text { .. } | Kind::Counter { .. } | Kind::Html { .. });
        if let Some(p) = &s.fill {
            if !matches!(l.kind, Kind::Path { .. }) {
                if s.text_fill && snap {
                    // native: glyph brushes are replaced by the paint (reelir.rs)
                    st.push_str("color:#fff;");
                } else if s.text_fill {
                    let _ = write!(st, "background:{};-webkit-background-clip:text;background-clip:text;color:transparent;", paint_css(p, self, &chans));
                } else {
                    let _ = write!(st, "background:{};", paint_css(p, self, &chans));
                }
            }
        }
        if has(Ch::Fill) && !matches!(l.kind, Kind::Path { .. }) {
            let _ = write!(st, "background-color:{};", self.cv(l, Ch::Fill));
        }
        if s.radius > 0.0 {
            let _ = write!(st, "border-radius:{}px;", f(s.radius));
        }
        if matches!(l.kind, Kind::Ellipse) {
            st.push_str("border-radius:50%;");
        }
        if let Some((w, c)) = &s.border {
            let _ = write!(st, "border:{}px solid {};", f(*w), css_color(c));
        }
        if let (Some(c), true) = (&s.stroke, matches!(l.kind, Kind::Rect | Kind::Ellipse)) {
            let _ = write!(st, "border:{}px solid {};", f(s.stroke_w), css_color(c));
        }
        let mut drop = vec![];
        let mut boxsh = vec![];
        for sh in &s.shadows {
            let c = sh.color.as_ref().map(css_color).unwrap_or("#0006".into());
            if is_box && !matches!(l.kind, Kind::Text { .. }) {
                boxsh.push(format!("{}{}px {}px {}px {}px {c}", if sh.inner { "inset " } else { "" }, f(sh.x), f(sh.y), f(sh.blur), f(sh.spread)));
            } else {
                drop.push(format!("drop-shadow({}px {}px {}px {c})", f(sh.x), f(sh.y), f(sh.blur / 2.0)));
            }
        }
        if !boxsh.is_empty() {
            let _ = write!(st, "box-shadow:{};", boxsh.join(","));
        }
        // text
        if let Some(fam) = &s.font {
            let _ = write!(st, "font-family:'{}',sans-serif;", fam);
        }
        let fit_size = self.snap.as_ref().and_then(|sn| sn.font_size.get(&l.id).copied());
        if let Some(sz) = fit_size.or(s.size) {
            let _ = write!(st, "font-size:{}px;", f(sz));
        }
        if has(Ch::Wght) {
            let _ = write!(st, "font-weight:{};", self.cv(l, Ch::Wght));
        } else if let Some(w) = s.weight {
            let _ = write!(st, "font-weight:{};", f(w));
        }
        // variable axes (static `axes=`, animated wdth/opsz/slnt/custom channels) and features
        let mut vars: Vec<(String, String)> = s.axes.iter().map(|(t, v)| (t.clone(), f(*v))).collect();
        for ch in [Ch::Wdth, Ch::Opsz, Ch::Slnt, Ch::Axis0, Ch::Axis1, Ch::Axis2, Ch::Axis3] {
            if let (true, Some(tag)) = (has(ch), ch.axis_tag()) {
                let v = self.cv(l, ch);
                match vars.iter_mut().find(|x| x.0 == tag) {
                    Some(x) => x.1 = v,
                    None => vars.push((tag, v)),
                }
            }
        }
        if !vars.is_empty() {
            let _ = write!(st, "font-variation-settings:{};", vars.iter().map(|(t, v)| format!("\"{t}\" {v}")).collect::<Vec<_>>().join(","));
        }
        if let Some(ft) = &s.features {
            let _ = write!(st, "font-feature-settings:{ft};");
        }
        if let (Some(c), Kind::Text { .. }, false) = (&s.stroke, &l.kind, snap) {
            // Chrome reference of the text stroke (reel draws the outline from the glyph outlines;
            // `stroke-over` = centered over the fill, default = below it)
            let over = l.tags.iter().any(|t| t == "stroke-over");
            let (w, order) = if over { (s.stroke_w, "") } else { (2.0 * s.stroke_w, "paint-order:stroke fill;") };
            let _ = write!(st, "-webkit-text-stroke:{}px {};{order}", f(w), css_color(c));
        }
        if has(Ch::Color) {
            let _ = write!(st, "color:{};", self.cv(l, Ch::Color));
        } else if let (Some(c), false) = (&s.color, s.text_fill) {
            let _ = write!(st, "color:{};", css_color(c));
        }
        if let Some(a) = &s.accent {
            let _ = write!(st, "--accent:{};", css_color(a));
        }
        if let Some(a) = &s.text_align {
            let _ = write!(st, "text-align:{a};");
        }
        if let Some(lh) = s.line_height {
            let _ = write!(st, "line-height:{};", f(lh));
        }
        if has(Ch::Tracking) {
            let _ = write!(st, "letter-spacing:calc({} * 1em);", self.cv(l, Ch::Tracking));
        } else if let Some(t) = s.tracking {
            let _ = write!(st, "letter-spacing:{}em;", f(t));
        }
        if s.uppercase {
            st.push_str("text-transform:uppercase;");
        }
        if l.tags.iter().any(|t| t == "piece") {
            st.push_str("white-space:pre;");
        } else if s.nowrap {
            st.push_str("white-space:nowrap;");
        }
        if let (Some(b), false) = (&s.blend, snap) {
            let _ = write!(st, "mix-blend-mode:{b};");
        }
        if let (Some(m), false) = (&s.mask, snap) {
            let mc = paint_css(m, self, &BTreeSet::new());
            let _ = write!(st, "-webkit-mask-image:{mc};mask-image:{mc};");
            if s.mask_luma {
                st.push_str("mask-mode:luminance;");
            }
            if has(Ch::MaskX) {
                let _ = write!(st, "mask-size:300% 100%;-webkit-mask-size:300% 100%;mask-position:calc(var({}) * 100%) 0;-webkit-mask-position:calc(var({}) * 100%) 0;", var(Ch::MaskX), var(Ch::MaskX));
            }
        }
        if s.overflow_clip {
            st.push_str("overflow:hidden;");
        }
        if let (Some(p), false) = (s.perspective, snap) {
            let _ = write!(st, "perspective:{}px;", f(p));
        }
        if s.preserve3d && !snap {
            st.push_str("transform-style:preserve-3d;");
        }
        // clip (wipe / iris) driven by the reveal channel
        if let (Some(c), false) = (&s.clip, snap) {
            let r = format!("(1 - var({}))", var(Ch::Reveal));
            let cp = match c {
                ClipShape::Wipe(d) => match d.as_str() {
                    "right" => format!("inset(0 0 0 calc({r} * 100%))"),
                    "top" => format!("inset(0 0 calc({r} * 100%) 0)"),
                    "bottom" => format!("inset(calc({r} * 100%) 0 0 0)"),
                    _ => format!("inset(-2px calc({r} * 100%) -2px -2px)"),
                },
                ClipShape::Polygon(p) => format!("polygon({})", p.iter().map(|p| format!("{}px {}px", p[0], p[1])).collect::<Vec<_>>().join(",")),
                ClipShape::Iris(x, y) => format!("circle(calc(var({}) * 72%) at {}% {}%)", var(Ch::Reveal), f(x * 100.0), f(y * 100.0)),
            };
            let _ = write!(st, "clip-path:{cp};");
        }
        // filters
        let mut fl = vec![];
        if has(Ch::Blur) {
            fl.push(format!("blur(var({}))", var(Ch::Blur)));
        }
        for (c, n) in [(Ch::Brightness, "brightness"), (Ch::Saturate, "saturate"), (Ch::Contrast, "contrast")] {
            if has(c) {
                fl.push(format!("{n}(var({}))", var(c)));
            }
        }
        if has(Ch::Hue) {
            fl.push(format!("hue-rotate(var({}))", var(Ch::Hue)));
        }
        if has(Ch::Glow) {
            let gc = s.glow_color.as_ref().map(css_color).unwrap_or("#3cbf87".into());
            fl.push(format!("drop-shadow(0 0 var({}) {gc})", var(Ch::Glow)));
        }
        fl.extend(drop);
        if !fl.is_empty() && !snap {
            let _ = write!(st, "filter:{};", fl.join(" "));
        }
        if has(Ch::Opacity) && !snap {
            let _ = write!(st, "opacity:var({});", var(Ch::Opacity));
        }
        // transform
        let mut tf = vec![];
        if lay.pos == Pos::Abs && (lay.anchor.0 != 0.0 || lay.anchor.1 != 0.0) {
            tf.push(format!("translate({}%,{}%)", f(-lay.anchor.0 * 100.0), f(-lay.anchor.1 * 100.0)));
        }
        // snapshot: only the anchor translate; everything else is a reel-ir node transform
        let hc = |c: Ch| !snap && has(c);
        if hc(Ch::Px) || hc(Ch::Py) {
            tf.push(format!("translate(calc(var({}) * 100%),calc(var({}) * 100%))", var(Ch::Px), var(Ch::Py)));
        }
        if hc(Ch::Magic) {
            let k = format!("(1 - var({}))", var(Ch::Magic));
            tf.push(format!("translate(calc(var(--mdx, 0px) * {k}),calc(var(--mdy, 0px) * {k})) scale(calc(1 + (var(--msx, 1) - 1) * {k}),calc(1 + (var(--msy, 1) - 1) * {k}))"));
        }
        if hc(Ch::X) || hc(Ch::Y) || hc(Ch::Z) {
            tf.push(format!("translate3d(var({}),var({}),var({}))", var(Ch::X), var(Ch::Y), var(Ch::Z)));
        }
        if hc(Ch::RotX) {
            tf.push(format!("rotateX(var({}))", var(Ch::RotX)));
        }
        if hc(Ch::RotY) {
            tf.push(format!("rotateY(var({}))", var(Ch::RotY)));
        }
        if hc(Ch::Rot) {
            tf.push(format!("rotate(var({}))", var(Ch::Rot)));
        }
        if hc(Ch::Sx) || hc(Ch::Sy) {
            tf.push(format!("scale(var({}),var({}))", var(Ch::Sx), var(Ch::Sy)));
        }
        if hc(Ch::Scale) {
            tf.push(format!("scale(var({}))", var(Ch::Scale)));
        }
        if !tf.is_empty() {
            let _ = write!(st, "transform:{};", tf.join(" "));
        }
        if s.origin != (0.5, 0.5) {
            let _ = write!(st, "transform-origin:{}% {}%;", f(s.origin.0 * 100.0), f(s.origin.1 * 100.0));
        }
        if let (Some(vs), false) = (self.inline_vars.get(&l.id), snap) {
            for (c, v) in vs {
                let _ = write!(st, "{}:{v};", var(*c));
            }
        }
        if let Some(a) = self.anims.get(&l.id) {
            let total = f(self.duration);
            let list: Vec<String> = a.iter().map(|n| format!("{n} {total}s linear both paused")).collect();
            let _ = write!(st, "animation:{};", list.join(","));
        }
        if let Some(extra) = &s.css {
            st.push_str(extra);
        }
        st
    }

    fn emit(&mut self, l: &Layer, parent_flow: &str) {
        let style = self.style_of(l, parent_flow);
        let mut cls: Vec<String> = l.tags.iter().filter(|t| !t.starts_with("start:")).map(|t| format!("t-{t}")).collect();
        let start_attr = l.tags.iter().find_map(|t| t.strip_prefix("start:")).map(|v| format!(" data-start=\"{v}\"")).unwrap_or_default();
        if matches!(l.kind, Kind::Counter { .. }) {
            // tabular figures: a counting number never changes its advance widths
            cls.push("tnum".into());
        }
        cls.push("L".into());
        let name_attr = l.name.as_ref().map(|n| format!(" data-id=\"{}\"", esc(n))).unwrap_or_default() + &start_attr;
        let id = esc(&l.id);
        let open = |b: &mut String, extra: &str| {
            let _ = write!(b, "<div id=\"{id}\" class=\"{}\"{name_attr}{extra} style=\"{}\">", cls.join(" "), esc(&style));
        };
        match &l.kind {
            Kind::Text { runs } => {
                open(&mut self.body, "");
                for r in runs {
                    let mut st = String::new();
                    if r.bold {
                        st.push_str("font-weight:800;");
                    }
                    if r.accent {
                        st.push_str("color:var(--accent);");
                    }
                    if r.code {
                        st.push_str("font-family:'Noto Sans Mono',monospace;font-size:.9em;background:#ffffff1f;padding:0 .2em;border-radius:.2em;");
                    }
                    if let Some(c) = &r.color {
                        let _ = write!(st, "color:{};", css_color(c));
                    }
                    let t = esc(&r.text).replace('\n', "<br>");
                    if st.is_empty() {
                        self.body.push_str(&t);
                    } else {
                        let _ = write!(self.body, "<span style=\"{st}\">{t}</span>");
                    }
                }
            }
            Kind::Counter { prefix, suffix } => {
                open(&mut self.body, " data-counter=\"1\"");
                if let Some(sn) = &self.snap {
                    let n = sn.vals.get(&(l.id.clone(), Ch::Count)).map(|v| v.n().round() as i64).unwrap_or(0);
                    let _ = write!(self.body, "{}{}{}", esc(prefix), crate::ir::counter_text(l, n), esc(suffix));
                } else {
                    let _ = write!(self.body, "{}<span class=\"ctr\"></span>{}", esc(prefix), esc(suffix));
                }
            }
            Kind::Image { src, .. } if self.snap.is_some() => {
                open(&mut self.body, "");
                // layout-only placeholder with the PNG's intrinsic size (drawn as reel `image` content)
                let (w, h) = crate::lower::png_size(Path::new(src)).unwrap_or((1.0, 1.0));
                let _ = write!(self.body, "<img class=\"ph\" src=\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='{}' height='{}'/%3E\" style=\"width:100%;height:100%;display:block\">", f(w), f(h));
            }
            Kind::Image { src, fit } => {
                open(&mut self.body, "");
                let u = self.url(src);
                let _ = write!(self.body, "<img src=\"{}\" style=\"width:100%;height:100%;object-fit:{};display:block\">", esc(&u), if fit == "fill" { "fill" } else { fit });
            }
            Kind::Video { src } => {
                open(&mut self.body, "");
                let u = self.url(src);
                let _ = write!(self.body, "<video src=\"{}\" muted playsinline preload=\"auto\" style=\"width:100%;height:100%;object-fit:cover;display:block\"></video>", esc(&u));
            }
            Kind::Html { markup } if l.children.iter().any(|c| c.tags.iter().any(|t| t == "part")) => {
                open(&mut self.body, "");
                // parts: the element carrying data-part="x" becomes the layer (id, class L, channel styles)
                let mut m = markup.clone();
                for c in l.children.iter().filter(|c| c.tags.iter().any(|t| t == "part")) {
                    let part = c.name.as_deref().and_then(|n| n.split_once('.')).map(|x| x.1.to_string()).unwrap_or_default();
                    let style = self.style_of(c, "none");
                    m = inject_part(&m, &part, &c.id, &style);
                }
                self.body.push_str(&m);
            }
            Kind::Svg { markup } if l.layout.w.is_some() || l.layout.h.is_some() => {
                open(&mut self.body, "");
                self.body.push_str(&fit_svg(markup, l.layout.w.is_some(), l.layout.h.is_some()));
            }
            Kind::Svg { markup } | Kind::Html { markup } => {
                open(&mut self.body, "");
                self.body.push_str(markup);
            }
            Kind::Path { d, vb, .. } => {
                open(&mut self.body, "");
                let s = &l.style;
                let chans = self.chans.get(&l.id).cloned().unwrap_or_default();
                let fill = match &s.fill {
                    Some(p @ Paint::Solid(_)) => paint_css(p, self, &chans),
                    Some(Paint::Linear { angle_deg, stops }) => {
                        // svg gradient
                        let gid = format!("{}-grad", l.id);
                        let a = (angle_deg - 90.0).to_radians();
                        let (x2, y2) = (0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin());
                        let _ = write!(self.body, "<svg width=\"0\" height=\"0\" style=\"position:absolute\"><defs><linearGradient id=\"{gid}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">{}</linearGradient></defs></svg>", f(1.0 - x2), f(1.0 - y2), f(x2), f(y2),
                            stops.iter().enumerate().map(|(k, st)| {
                                let c = match &st.color { ColorRef::Lit(c) => css_color(c), _ => "#fff".into() };
                                format!("<stop offset=\"{}\" stop-color=\"{c}\"/>", f(st.pos.unwrap_or(k as f64 / (stops.len().max(2) - 1) as f64)))
                            }).collect::<String>());
                        format!("url(#{gid})")
                    }
                    _ => "none".into(),
                };
                let stroke = if chans.contains(&Ch::Stroke) { self.cv(l, Ch::Stroke) } else { s.stroke.as_ref().map(css_color).unwrap_or("none".into()) };
                let mut pst = String::new();
                if self.snap.is_some() {
                } else if chans.contains(&Ch::TrimStart) {
                    let _ = write!(pst, "stroke-dasharray:calc(var({e}) - var({s0})) 2;stroke-dashoffset:calc(-1 * var({s0}));", e = var(Ch::TrimEnd), s0 = var(Ch::TrimStart));
                } else if chans.contains(&Ch::TrimEnd) {
                    let _ = write!(pst, "stroke-dasharray:1 1;stroke-dashoffset:calc(1 - var({}));", var(Ch::TrimEnd));
                } else if let Some((a, b)) = s.dash {
                    let _ = write!(pst, "stroke-dasharray:{} {};", f(a), f(b));
                }
                let pl = if chans.contains(&Ch::TrimEnd) || chans.contains(&Ch::TrimStart) { " pathLength=\"1\"" } else { "" };
                let _ = write!(self.body, "<svg viewBox=\"0 0 {} {}\" width=\"100%\" height=\"100%\" preserveAspectRatio=\"none\" style=\"overflow:visible;display:block\"><path class=\"p\"{pl} d=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"{}\" stroke-linejoin=\"round\" style=\"{}\"/></svg>",
                    f(vb.0), f(vb.1), esc(d), esc(&fill), esc(&stroke), f(s.stroke_w), if l.tags.iter().any(|t| t == "slice") { "butt" } else { "round" }, pst);
            }
            _ => open(&mut self.body, ""),
        }
        for c in l.children.iter().filter(|c| !c.tags.iter().any(|t| t == "part")) {
            self.emit(c, l.layout.flow);
        }
        if let (Some(g), false) = (l.style.grain, self.snap.is_some()) {
            let _ = write!(self.body, "<div class=\"grain\" style=\"opacity:{}\"></div>", f(g));
        }
        if let Some(v) = l.style.vignette {
            // own layer id (`<id>~vig`) so the native path paints it above the children
            let _ = write!(self.body, "<div id=\"{}~vig\" class=\"L\" style=\"position:absolute;inset:0;pointer-events:none;background:radial-gradient(ellipse at center, transparent 45%, rgb(0 0 0 / {}) 100%)\"></div>", esc(&l.id), f(v));
        }
        self.body.push_str("</div>");
    }
}

const RUNTIME: &str = r#"
(() => {
const R = window.__reel = Object.assign(window.__reel || {}, META);
const $ = (id) => document.getElementById(id);
function measure() {
  const root = document.documentElement;
  root.classList.add('reel-measure');
  for (const h of HOOKS) {
    if (h.type === 'fit') {
      const el = $(h.layer); if (!el) continue;
      let fs = parseFloat(getComputedStyle(el).fontSize);
      for (let i = 0; i < 40 && (el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1); i++) { fs *= 0.95; el.style.fontSize = fs + 'px'; }
    }
  }
  for (const h of HOOKS) {
    if (h.type === 'magic') {
      const a = $(h.from), b = $(h.to); if (!a || !b) continue;
      const ra = a.getBoundingClientRect(), rb = b.getBoundingClientRect();
      const ca = [ra.left + ra.width / 2, ra.top + ra.height / 2], cb = [rb.left + rb.width / 2, rb.top + rb.height / 2];
      const s = R.scale || 1;
      b.style.setProperty('--mdx', (ca[0] - cb[0]) / s + 'px'); b.style.setProperty('--mdy', (ca[1] - cb[1]) / s + 'px');
      b.style.setProperty('--msx', ra.width / Math.max(rb.width, 1)); b.style.setProperty('--msy', ra.height / Math.max(rb.height, 1));
      a.style.setProperty('--mdx', (cb[0] - ca[0]) / s + 'px'); a.style.setProperty('--mdy', (cb[1] - ca[1]) / s + 'px');
      a.style.setProperty('--msx', rb.width / Math.max(ra.width, 1)); a.style.setProperty('--msy', rb.height / Math.max(ra.height, 1));
    }
    if (h.type === 'link') {
      const p = $(h.path); if (!p) continue;
      const sc = p.closest('.t-scene') || document;
      const q = (id) => sc.querySelector('[data-id="' + id + '"]') || document.querySelector('[data-id="' + id + '"]');
      const a = q(h.a), b = q(h.b), path = p.querySelector('path'); if (!a || !b || !path) continue;
      const box = p.getBoundingClientRect(), vb = p.querySelector('svg').viewBox.baseVal;
      const sx = vb.width / box.width, sy = vb.height / box.height;
      const ra = a.getBoundingClientRect(), rb = b.getBoundingClientRect();
      const c = (r, m) => [(r.left + r.width / 2 - box.left) * sx, (r.top + r.height / 2 - box.top) * sy, r.width / 2 * sx + m, r.height / 2 * sy + m];
      const edge = (P, Q) => { const dx = Q[0] - P[0], dy = Q[1] - P[1]; const k = Math.min(Math.abs(dx) > 1e-6 ? P[2] / Math.abs(dx) : 1e9, Math.abs(dy) > 1e-6 ? P[3] / Math.abs(dy) : 1e9); return [P[0] + dx * k, P[1] + dy * k]; };
      let s, e;
      for (const m of [14, 6, 0]) {   // shrink the margin when boxes are close, never let the arrow flip
        const A = c(ra, m), B = c(rb, m); s = edge(A, B); e = edge(B, A);
        if ((e[0] - s[0]) * (B[0] - A[0]) + (e[1] - s[1]) * (B[1] - A[1]) > 0) break;
      }
      let d;
      if (h.curve) { const mx = (s[0] + e[0]) / 2; d = `M${s[0]} ${s[1]} C${mx} ${s[1]} ${mx} ${e[1]} ${e[0]} ${e[1]}`; }
      else d = `M${s[0]} ${s[1]} L${e[0]} ${e[1]}`;
      const ang = Math.atan2(e[1] - (h.curve ? e[1] : s[1]), e[0] - (h.curve ? (s[0] + e[0]) / 2 : s[0]));
      const al = 18, aw = 0.5;
      d += ` M${e[0] - al * Math.cos(ang - aw)} ${e[1] - al * Math.sin(ang - aw)} L${e[0]} ${e[1]} L${e[0] - al * Math.cos(ang + aw)} ${e[1] - al * Math.sin(ang + aw)}`;
      path.setAttribute('d', d);
    }
  }
  root.classList.remove('reel-measure');
}
R.ready = (async () => {
  await document.fonts.ready;
  await Promise.all([...document.images].map((i) => i.decode().catch(() => {})));
  measure();
  R.anims = document.getAnimations();
  R.anims.forEach((a) => a.pause());
  R.videos = [...document.querySelectorAll('video')];
  return true;
})();
R.seek = async (t) => {
  await R.ready;
  for (const a of R.anims) a.currentTime = t * 1000;
  for (const v of R.videos) {
    const s = v.closest('.t-scene'); const t0 = s ? parseFloat(s.dataset.start || '0') : 0;
    const want = Math.max(0, t - t0);
    if (Math.abs(v.currentTime - want) > 1e-3) await new Promise((res) => { v.addEventListener('seeked', res, { once: true }); v.currentTime = want; });
  }
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
};
R.play = async () => { await R.ready; const t0 = performance.now(); const loop = () => { const t = ((performance.now() - t0) / 1000) % R.duration; for (const a of R.anims) a.currentTime = t * 1000; requestAnimationFrame(loop); }; loop(); };
R.measureAll = async (times) => {
  await R.ready; const out = [];
  for (const t of times) {
    await R.seek(t);
    const sb = document.getElementById('stage').getBoundingClientRect();
    const items = [];
    for (const el of document.querySelectorAll('.t-text,[data-id]')) {
      const r = el.getBoundingClientRect(); if (r.width === 0 && r.height === 0) continue;
      let op = 1, n = el; while (n && n.nodeType === 1) { op *= parseFloat(getComputedStyle(n).opacity); n = n.parentElement; }
      if (op < 0.5) continue;
      const cs = getComputedStyle(el);
      let bg = null, m = el; while (m && m.nodeType === 1) { const c = getComputedStyle(m).backgroundColor; if (c && c !== 'rgba(0, 0, 0, 0)' && c !== 'transparent') { bg = c; break; } const bi = getComputedStyle(m).backgroundImage; if (bi && bi !== 'none') { bg = 'img:' + bi.slice(0, 200); break; } m = m.parentElement; }
      items.push({ id: el.id, name: el.dataset.id || null, cls: el.className, ib: !!el.closest('.t-browser'), cap: !!el.closest('.t-caption'), x: r.left - sb.left, y: r.top - sb.top, w: r.width, h: r.height,
        fs: parseFloat(cs.fontSize) * (r.height > 0 && el.offsetHeight > 0 ? r.height / el.offsetHeight : 1), color: cs.color, bg,
        overflowX: el.scrollWidth > el.clientWidth + 2 && cs.whiteSpace !== 'nowrap' ? el.scrollWidth - el.clientWidth : 0,
        overflowY: el.scrollHeight > el.clientHeight + 2 ? el.scrollHeight - el.clientHeight : 0,
        text: el.classList.contains('t-text') ? (el.innerText || '').slice(0, 60) : '' });
    }
    const links = [];
    for (const el of document.querySelectorAll('.t-link')) {
      const path = el.querySelector('path'); if (!path) continue;
      let op = 1, n = el; while (n && n.nodeType === 1) { op *= parseFloat(getComputedStyle(n).opacity); n = n.parentElement; }
      if (op < 0.5) continue;
      const r = path.getBoundingClientRect(); let len = 0; try { len = path.getTotalLength(); } catch (e) {}
      links.push({ id: el.id, name: el.dataset.id || null, x: r.left - sb.left, y: r.top - sb.top, w: r.width, h: r.height, len, d: (path.getAttribute('d') || '').length });
    }
    out.push({ t, items, links });
  }
  return out;
};
if (location.hash.includes('play')) R.play();
})();
"#;

/// Give the markup element with `data-part="part"` the layer id, class `L` and the layer's style
/// (prepended to its own inline style, so authored declarations still win for static props).
fn inject_part(markup: &str, part: &str, id: &str, style: &str) -> String {
    let key = format!("data-part=\"{part}\"");
    let Some(at) = markup.find(&key) else { return markup.to_string() };
    let Some(open) = markup[..at].rfind('<') else { return markup.to_string() };
    let Some(close_rel) = markup[at..].find('>') else { return markup.to_string() };
    let close = at + close_rel;
    let tag = &markup[open..close];
    let mut t = tag.to_string();
    let st = esc(style.trim_start_matches("position:relative;"));
    if let Some(sp) = t.find("style=\"") {
        t.insert_str(sp + 7, &st);
    } else {
        t.push_str(&format!(" style=\"{st}\""));
    }
    if let Some(cp) = t.find("class=\"") {
        t.insert_str(cp + 7, "L ");
    } else {
        t.push_str(" class=\"L\"");
    }
    t.push_str(&format!(" id=\"{}\"", esc(id)));
    format!("{}{}{}", &markup[..open], t, &markup[close..])
}

/// The inline style of one layer in a snapshot page (native restyle of a single element).
pub fn layer_style(l: &Layer, parent_flow: &str, opts: &HtmlOpts, chans: &BTreeSet<Ch>) -> String {
    let mut cm = HashMap::new();
    cm.insert(l.id.clone(), chans.clone());
    let mut p = Page { css: String::new(), body: String::new(), anims: HashMap::new(), inline_vars: HashMap::new(), chans: cm, kf: 0, embed: opts.embed, img_cache: HashMap::new(), duration: 0.0, snap: opts.snap.clone(), cur_layer: String::new() };
    p.style_of(l, parent_flow)
}

pub fn emit(ir: &Ir, opts: &HtmlOpts) -> String {
    let mut chans: HashMap<String, BTreeSet<Ch>> = HashMap::new();
    let mut used = BTreeSet::new();
    for t in &ir.tracks {
        chans.entry(t.layer.clone()).or_default().insert(t.ch);
        used.insert(t.ch);
    }
    let _ = used;
    let mut p = Page { css: String::new(), body: String::new(), anims: HashMap::new(), inline_vars: HashMap::new(), chans, kf: 0, embed: opts.embed, img_cache: HashMap::new(), duration: ir.duration, snap: opts.snap.clone(), cur_layer: String::new() };
    for t in ir.tracks.iter().filter(|_| opts.snap.is_none()) {
        if t.keys.is_empty() || t.keys.iter().all(|k| k.v == t.keys[0].v) {
            let v = t.keys.first().map(|k| k.v.clone()).unwrap_or(t.base.clone());
            p.inline_vars.entry(t.layer.clone()).or_default().push((t.ch, chan_val(t.ch, &v)));
        } else {
            let name = p.keyframes(t);
            p.anims.entry(t.layer.clone()).or_default().push(name);
        }
    }
    // scene start for video sync
    p.emit(&ir.root, "none");
    let mut head = String::new();
    if opts.snap.is_some() {
        // native layout snapshot: fonts are registered in the layout engine, no runtime
        let _ = write!(head, "html,body{{margin:0;padding:0;background:#000;overflow:hidden}}\n#stage{{position:relative;width:{}px;height:{}px;overflow:hidden;font-family:'Inter',sans-serif;font-feature-settings:'cv11','ss01';font-optical-sizing:auto}}\n.L{{box-sizing:border-box;margin:0}}\n.tnum{{font-feature-settings:'cv11','ss01','tnum'}}\n", ir.w, ir.h);
        return format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>reel</title>\n<style>\n{head}{}</style></head>\n<body>{}</body></html>\n", p.css, p.body);
    }
    let font = |file: &str| -> String {
        let path = opts.fonts_dir.join(file);
        match std::fs::read(&path) {
            Ok(b) if opts.embed => format!("data:font/ttf;base64,{}", base64::engine::general_purpose::STANDARD.encode(b)),
            _ => format!("file://{}", path.display()),
        }
    };
    // registered families (assets/fonts/fonts.json); only the families this video uses are embedded
    let manifest: serde_json::Value = std::fs::read_to_string(opts.fonts_dir.join("fonts.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(serde_json::json!({"fonts": [
        {"family": "Inter", "file": "Inter.ttf", "weight": "100 900"},
        {"family": "Noto Sans Mono", "file": "NotoSansMono-Regular.ttf", "weight": "400"},
        {"family": "Noto Sans Mono", "file": "NotoSansMono-Bold.ttf", "weight": "700"}]}));
    let used = |fam: &str| fam == "Inter" || fam == "Noto Sans Mono" || p.css.contains(fam) || p.body.contains(fam);
    for f in manifest["fonts"].as_array().into_iter().flatten() {
        let (Some(fam), Some(file)) = (f["family"].as_str(), f["file"].as_str()) else { continue };
        if !used(fam) {
            continue;
        }
        let _ = write!(head, "@font-face{{font-family:'{fam}';src:url({}) format('truetype');font-weight:{};font-display:block}}\n", font(file), f["weight"].as_str().unwrap_or("400"));
    }
    // custom font files (`font "Brand" src=...`): Chrome decodes WOFF/WOFF2 itself
    for cf in &ir.fonts {
        let Some(file) = &cf.file else { continue };
        let fmt = match std::path::Path::new(file).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
            Some("woff2") => "woff2",
            Some("woff") => "woff",
            Some("otf") => "opentype",
            _ => "truetype",
        };
        let url = match std::fs::read(file) {
            Ok(b) if opts.embed => format!("data:font/{fmt};base64,{}", base64::engine::general_purpose::STANDARD.encode(b)),
            _ => format!("file://{file}"),
        };
        let _ = write!(head, "@font-face{{font-family:'{}';src:url({url}) format('{fmt}');font-weight:1 1000;font-stretch:1% 1000%;font-display:block}}\n", cf.family);
    }
    let all: Vec<Ch> = Ch::NAMES.iter().filter_map(|n| Ch::parse(n)).chain([Ch::LineH, Ch::Magic, Ch::Axis0, Ch::Axis1, Ch::Axis2, Ch::Axis3]).collect();
    for ch in &all {
        let init = chan_val(*ch, &ch.default_value());
        // path channels are read by the inner <path>, so they must inherit
        let inh = matches!(ch, Ch::TrimStart | Ch::TrimEnd | Ch::Stroke);
        let _ = write!(head, "@property {}{{syntax:'{}';inherits:{};initial-value:{}}}\n", var(*ch), syntax(*ch), inh, init);
    }
    let _ = write!(head, "html,body{{margin:0;padding:0;background:#000;overflow:hidden}}\n#stage{{position:relative;width:{}px;height:{}px;overflow:hidden;font-family:'Inter',sans-serif;-webkit-font-smoothing:antialiased;font-feature-settings:'cv11','ss01';font-optical-sizing:auto}}\n", ir.w, ir.h);
    head.push_str(".L{box-sizing:border-box;margin:0}\n.tnum{font-feature-settings:'cv11','ss01','tnum'}\n.reel-measure *{animation:none!important}\n.ctr::after{content:counter(n)}\n[data-counter]{counter-reset:n var(--r-count)}\n");
    head.push_str(".grain{position:absolute;inset:0;pointer-events:none;mix-blend-mode:overlay;background-image:url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='240' height='240'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='.9' numOctaves='2' seed='7'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E\");animation:grain 0.333s steps(1) infinite}\n@keyframes grain{0%{background-position:0 0}10%{background-position:-37px 21px}20%{background-position:53px -11px}30%{background-position:-8px 64px}40%{background-position:71px 33px}50%{background-position:-49px -58px}60%{background-position:12px -27px}70%{background-position:-66px 8px}80%{background-position:29px 77px}90%{background-position:-15px -42px}}\n");
    let meta = format!("{{duration:{},fps:{},w:{},h:{},format:'{}'}}", f(ir.duration), f(ir.fps), ir.w, ir.h, ir.format);
    let hooks = serde_json::to_string(&ir.hooks).unwrap_or("[]".into());
    let runtime = RUNTIME.replace("META", &meta).replace("HOOKS", &hooks);
    // scene start attributes for video sync
    let body = p.body;

    format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>reel</title>\n<style>\n{head}{}</style></head>\n<body>{body}<script>{runtime}</script></body></html>\n", p.css)
}
