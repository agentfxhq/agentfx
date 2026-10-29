//! Text v2: custom font files, variable axes and OpenType features, text stroke and color
//! tracks, text animators (`animate`), text on a path (`on=#path`). Story/motion side; the
//! reel-ir side lives in `reelir.rs` (`TextFx` on the text layer's paint, `GlyphStroke`).
use crate::ast::{Document, Node};
use crate::diag::Diag;
use crate::ir::{Ch, Font, Kind, Layer, Track};
use crate::lower::{Ctx, Ev};
use crate::values::{Ease, Rgba};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;

fn praw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}
fn flag(n: &Node, f: &str) -> bool {
    crate::lower::flag(n, f)
}

/// A selector parameter: static base plus an optional animation (absolute times).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Param {
    pub base: f64,
    /// (t0, t1, from, to, ease): `a..b` over the verb's `dur=`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anim: Option<(f64, f64, f64, f64, Ease)>,
}
impl Param {
    fn fixed(v: f64) -> Param {
        Param { base: v, anim: None }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Sel {
    Range { start: Param, end: Param, offset: Param, amount: Param, shape: String, order: String, seed: u64 },
    /// per-unit keys `[(t, amount)]`, absolute seconds
    Keys { units: Vec<Vec<(f64, f64)>>, ease: Ease },
}

/// One `animate` verb: the properties at full amount and the selector.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TextAnim {
    pub by: String,
    pub sel: Sel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translate: Option<(f64, f64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<(f64, f64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgba>,
}

pub const ANIMATE_PROPS: &[&str] = &[
    "by", "range", "start", "end", "offset", "amount", "shape", "order", "seed", "times", "dir", "dur", "ease",
    "opacity", "x", "y", "scale", "sx", "sy", "rot", "blur", "color",
];
pub const SHAPES: &[&str] = &["square", "ramp-up", "ramp-down", "ramp", "triangle", "round", "smooth"];

/// `50%` -> 0.5, `0.5` -> 0.5 (fractions of the text for range parameters).
fn frac(s: &str) -> Option<f64> {
    let s = s.trim();
    match s.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok().map(|v| v / 100.0),
        None => s.parse::<f64>().ok(),
    }
    .filter(|v| v.is_finite())
}

/// `a..b` -> (a, b); `a` -> (a, a).
fn span(s: &str) -> Option<(f64, f64)> {
    match s.split_once("..") {
        Some((a, b)) => Some((frac(a)?, frac(b)?)),
        None => frac(s).map(|v| (v, v)),
    }
}

impl Ctx<'_> {
    /// `animate #title by=char|word|line [range=0%..100%] [offset=-100%..100%] [shape=]
    /// [order=forward|reverse|random seed=] [times=[...] dir=in|out] dur= ease= <props>`
    pub fn animate_verb(&mut self, e: &Ev) {
        let n = e.node.clone();
        let t = e.t;
        let Some(crate::events::Target::El(i)) = self.target(e, true) else { return };
        if !matches!(self.layers[i].kind, Kind::Text { .. }) {
            let what = if self.kids[i].iter().any(|&c| matches!(self.layers[c].kind, Kind::Text { .. })) { "split text: animate the whole text (no `split=`); the animator splits it into units itself" } else { "target a text element" };
            self.err("E_TARGET", "`animate` needs a text element", what, n.span);
            return;
        }
        for k in n.props.keys() {
            if !ANIMATE_PROPS.contains(&k.as_str()) {
                let h = crate::values::suggest(k, ANIMATE_PROPS).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("animate props: {}", ANIMATE_PROPS.join(" ")));
                self.warn("W_ANIMATE_PROP", format!("animate: unknown prop `{k}`"), h, n.span);
            }
        }
        let by = praw(&n, "by").unwrap_or_else(|| "char".into());
        if !matches!(by.as_str(), "char" | "word" | "line") {
            let h = crate::values::suggest(&by, &["char", "word", "line"]).map(|s| format!("did you mean `{s}`?")).unwrap_or("by=char|word|line".into());
            self.err("E_VALUE", format!("animate by={by}"), h, n.span);
            return;
        }
        let dur = self.dur(&n, "dur").unwrap_or(1.0).max(1e-3);
        let ease = self.ease(&n, "ease", Ease::Cubic(0.33, 1.0, 0.68, 1.0));
        let sel = if let Some(times) = praw(&n, "times") {
            // per-unit key times (relative to the event): unit k animates over [t + t_k, t + t_k + dur]
            let mut ts = vec![];
            for p in times.trim_matches(['[', ']']).split(',').map(str::trim).filter(|p| !p.is_empty()) {
                match crate::values::parse_dur(p) {
                    Ok(v) => ts.push(v.secs(self.fps)),
                    Err(_) => {
                        self.err("E_VALUE", format!("animate times: `{p}` is not a duration"), "write `times=[0s, 0.4s, 0.9s]` (one start per unit, from the event)", n.span);
                        return;
                    }
                }
            }
            if ts.is_empty() {
                self.err("E_VALUE", "animate times=[]: empty", "one start per unit, e.g. `times=[0s, 0.4s, 0.9s]`", n.span);
                return;
            }
            let (a, b) = match praw(&n, "dir").as_deref() {
                None | Some("in") => (1.0, 0.0),
                Some("out") => (0.0, 1.0),
                Some(x) => {
                    self.err("E_VALUE", format!("animate dir={x}"), "dir=in (from the props to rest, default) or dir=out", n.span);
                    return;
                }
            };
            Sel::Keys { units: ts.iter().map(|&tk| vec![(t + tk, a), (t + tk + dur, b)]).collect(), ease }
        } else {
            let shape = match praw(&n, "shape").as_deref() {
                None => "square".to_string(),
                Some("ramp") => "ramp-up".to_string(),
                Some(x) if SHAPES.contains(&x) => x.to_string(),
                Some(x) => {
                    let h = crate::values::suggest(x, SHAPES).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("shape={}", SHAPES.join("|")));
                    self.err("E_VALUE", format!("animate shape={x}"), h, n.span);
                    return;
                }
            };
            let order = praw(&n, "order").unwrap_or_else(|| "forward".into());
            if !matches!(order.as_str(), "forward" | "reverse" | "random") {
                self.err("E_VALUE", format!("animate order={order}"), "order=forward|reverse|random", n.span);
                return;
            }
            let seed = self.num(&n, "seed").unwrap_or(1.0).max(0.0) as u64;
            let param = |me: &mut Self, k: &str, dflt: f64| -> Option<Param> {
                let Some(raw) = praw(&n, k) else { return Some(Param::fixed(dflt)) };
                match span(&raw) {
                    Some((a, b)) if a == b => Some(Param::fixed(a)),
                    Some((a, b)) => Some(Param { base: a, anim: Some((t, t + dur, a, b, ease.clone())) }),
                    None => {
                        me.err("E_VALUE", format!("animate {k}={raw}"), format!("`{k}=` takes a fraction or percent, or `from..to` animated over dur= (e.g. `{k}=-100%..100%`)"), n.span);
                        None
                    }
                }
            };
            let (mut start, mut end) = (Param::fixed(0.0), Param::fixed(1.0));
            if let Some(r) = praw(&n, "range") {
                match r.split_once("..").and_then(|(a, b)| Some((frac(a)?, frac(b)?))) {
                    Some((a, b)) => {
                        start = Param::fixed(a);
                        end = Param::fixed(b);
                    }
                    None => {
                        self.err("E_VALUE", format!("animate range={r}"), "write `range=0%..100%` (start..end of the selection; animate it with start= / end= / offset=)", n.span);
                        return;
                    }
                }
            }
            if n.props.contains_key("start") {
                let Some(p) = param(self, "start", 0.0) else { return };
                start = p;
            }
            if n.props.contains_key("end") {
                let Some(p) = param(self, "end", 1.0) else { return };
                end = p;
            }
            let Some(offset) = param(self, "offset", 0.0) else { return };
            let Some(amount) = param(self, "amount", 1.0) else { return };
            Sel::Range { start, end, offset, amount, shape, order, seed }
        };
        let num = |me: &mut Self, k: &str| me.num(&n, k);
        let opacity = praw(&n, "opacity").and_then(|v| frac(&v)).map(|v| v.clamp(0.0, 1.0));
        let (x, y) = (num(self, "x"), num(self, "y"));
        let translate = (x.is_some() || y.is_some()).then(|| (x.unwrap_or(0.0), y.unwrap_or(0.0)));
        let s = num(self, "scale");
        let (sx, sy) = (num(self, "sx").or(s), num(self, "sy").or(s));
        let scale = (sx.is_some() || sy.is_some()).then(|| (sx.unwrap_or(1.0), sy.unwrap_or(1.0)));
        let rotate = num(self, "rot");
        let blur = num(self, "blur").map(|b| b.max(0.0));
        let color = self.color(&n, "color");
        if opacity.is_none() && translate.is_none() && scale.is_none() && rotate.is_none() && blur.is_none() && color.is_none() {
            self.warn("W_ANIMATE_PROP", "animate without properties: nothing changes", "add what the selected units do, e.g. `opacity=0 y=40 blur=6 color=$accent`", n.span);
        }
        self.layers[i].style.text_anims.push(TextAnim { by, sel, opacity, translate, scale, rotate, blur, color });
    }
}

/// Text props of text v2 (`axes=`, `features=`, `stroke-over`, `on=#path`, `offset=`).
pub fn text_props(c: &mut Ctx, l: &mut Layer, n: &Node) {
    if let Some(a) = praw(n, "axes") {
        match crate::fonts::parse_axes(&a) {
            Ok(v) => l.style.axes = v,
            Err(bad) => c.err("E_VALUE", format!("axes: `{bad}`"), "write `axes=\"wdth 80, opsz 36, GRAD 50\"` (4-letter tag and value per axis)", n.span),
        }
    }
    if let Some(f) = praw(n, "features") {
        match crate::fonts::features_css(&f) {
            Ok(css) => l.style.features = Some(css),
            Err(bad) => c.err("E_VALUE", format!("features: `{bad}`"), "write `features=\"ss01 off, tnum, cv05 2\"` (4-letter tag, on/off or a number)", n.span),
        }
    }
    if flag(n, "stroke-over") {
        l.tags.push("stroke-over".into());
    }
    if let Some(p) = praw(n, "on") {
        let name = p.trim_start_matches('#').to_string();
        let off = praw(n, "offset").map(|v| v.trim_end_matches("px").parse::<f64>().unwrap_or(0.0)).unwrap_or(0.0);
        l.style.text_path = Some((name, off));
        l.style.nowrap = true;
    }
}

/// reel-ir `TextFx` value of a layer (None: no animators, no path).
pub fn fx_json(l: &Layer, path_d: Option<String>, easing: &dyn Fn(&Ease) -> Value) -> Option<Value> {
    if l.style.text_anims.is_empty() && path_d.is_none() {
        return None;
    }
    let mut anims = vec![];
    for a in &l.style.text_anims {
        let sel = match &a.sel {
            Sel::Range { start, end, offset, amount, shape, order, seed } => {
                json!({"type": "range", "start": start.base, "end": end.base, "offset": offset.base, "amount": amount.base, "shape": shape, "order": order, "seed": seed})
            }
            Sel::Keys { units, ease } => json!({"type": "keys", "units": units.iter().map(|u| u.iter().map(|(t, v)| json!([r4(*t), v])).collect::<Vec<_>>()).collect::<Vec<_>>(), "easing": easing(ease)}),
        };
        let mut v = json!({"by": a.by, "selector": sel});
        if let Some(o) = a.opacity {
            v["opacity"] = json!(o);
        }
        if let Some((x, y)) = a.translate {
            v["translate"] = json!([x, y]);
        }
        if let Some((x, y)) = a.scale {
            v["scale"] = json!([x, y]);
        }
        if let Some(r) = a.rotate {
            v["rotate"] = json!(r);
        }
        if let Some(b) = a.blur {
            v["blur"] = json!(b);
        }
        if let Some(c) = &a.color {
            v["color"] = json!([c.r, c.g, c.b, c.a]);
        }
        anims.push(v);
    }
    let mut fx = json!({});
    if !anims.is_empty() {
        fx["animators"] = json!(anims);
    }
    if let Some(d) = path_d {
        fx["path"] = json!({"d": d, "offset": l.style.text_path.as_ref().map(|p| p.1).unwrap_or(0.0)});
    }
    Some(fx)
}

/// Selector parameter tracks of a layer's animators: (property, delay, duration, from, to, ease).
pub fn fx_tracks(l: &Layer) -> Vec<(String, f64, f64, f64, f64, Ease)> {
    let mut out = vec![];
    for (i, a) in l.style.text_anims.iter().enumerate() {
        if let Sel::Range { start, end, offset, amount, .. } = &a.sel {
            for (k, p) in [("start", start), ("end", end), ("offset", offset), ("amount", amount)] {
                if let Some((t0, t1, a, b, e)) = &p.anim {
                    out.push((format!("animators.{i}.{k}"), *t0, t1 - t0, *a, *b, e.clone()));
                }
            }
        }
    }
    out
}

fn r4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

/// A text layer whose `color` channel can be a reel-ir `text.fill` track instead of repaint
/// states: plain runs (no accent/code/explicit colors), no text matte, no child layers.
pub fn color_track_ok(l: &Layer, has_children: bool, t: &Track) -> bool {
    let Kind::Text { runs } = &l.kind else { return false };
    t.ch == Ch::Color && !has_children && !l.style.text_fill && runs.iter().all(|r| !r.accent && !r.code && r.color.is_none()) && t.keys.iter().all(|k| matches!(k.v, crate::ir::TV::C(_)))
}

/// Text stroke of a text layer as reel-ir (`sw=` is the visible outline width: below the fill
/// the stroke is drawn twice as wide, over the fill as wide).
pub fn stroke_of(l: &Layer) -> Option<agentfx_ir::GlyphStroke> {
    let c = l.style.stroke.as_ref()?;
    if !matches!(l.kind, Kind::Text { .. }) || l.style.stroke_w <= 0.0 {
        return None;
    }
    let over = l.tags.iter().any(|t| t == "stroke-over");
    Some(agentfx_ir::GlyphStroke { width: l.style.stroke_w * if over { 1.0 } else { 2.0 }, brush: agentfx_ir::Brush::Solid([c.r as f32, c.g as f32, c.b as f32, c.a as f32]), join: "round".into(), over })
}

/// Put the stroke on every glyph run of a display list.
pub fn apply_stroke(items: &mut [agentfx_ir::Draw], st: &agentfx_ir::GlyphStroke) {
    for it in items.iter_mut() {
        if let agentfx_ir::Draw::Glyphs { stroke, .. } = it {
            *stroke = Some(st.clone());
        }
    }
}

/// `font "Brand" src="fonts/Brand.woff2"` at top level: checked (file exists, decodes, parses)
/// and returned for registration under the alias.
pub fn custom_fonts(doc: &Document, base: &Path, diags: &mut Vec<Diag>) -> Vec<Font> {
    let mut out: Vec<Font> = vec![];
    for n in doc.nodes.iter().filter(|n| n.kind == "font") {
        let family = n.args.first().and_then(|a| a.as_str()).map(|s| s.trim_matches('"').to_string()).unwrap_or_default();
        if family.is_empty() || family.contains(['\'', '"', ';', '{', '}']) {
            diags.push(Diag::error("E_FONT", "`font` needs a family name", "write `font \"Brand\" src=\"fonts/Brand.woff2\"`", n.span));
            continue;
        }
        let Some(src) = n.props.get("src").map(|v| v.raw()) else {
            diags.push(Diag::error("E_FONT", format!("font \"{family}\": no `src=`"), "write `font \"Brand\" src=\"fonts/Brand.ttf\"` (TTF, OTF, WOFF or WOFF2)", n.span));
            continue;
        };
        for k in n.props.keys() {
            if k != "src" {
                diags.push(Diag::warn("W_FONT_PROP", format!("font \"{family}\": unknown prop `{k}`"), "only `src=` is read; axes and features go on the text (`axes=`, `features=`)", n.span));
            }
        }
        let path = base.join(&src);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "woff" | "woff2") {
            diags.push(Diag::error("E_FONT", format!("font \"{family}\": `{src}` is not a font file"), "use a .ttf, .otf, .woff or .woff2 file", n.span));
            continue;
        }
        if let Err(e) = crate::fonts::load(&path) {
            diags.push(Diag::error("E_FONT", format!("font \"{family}\" src=\"{src}\": {e:#}"), "check the path (relative to the .reel file) and that the file is a valid TTF/OTF/WOFF/WOFF2", n.span));
            continue;
        }
        if out.iter().any(|f| f.family == family) {
            diags.push(Diag::error("E_FONT", format!("font \"{family}\" declared twice"), "one `font` line per family (a variable font covers all weights)", n.span));
            continue;
        }
        let abs = path.canonicalize().unwrap_or(path);
        out.push(Font { family, src: src.clone(), file: Some(abs.to_string_lossy().into_owned()) });
    }
    out
}

/// Families text can name with `font=`: the registered ones (`assets/fonts/fonts.json`) and
/// the video's `font` declarations.
pub fn known_families(ir_fonts: &[Font]) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_to_string(crate::run::home().join("assets/fonts/fonts.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|j| j["fonts"].as_array().map(|a| a.iter().filter_map(|f| f["family"].as_str().map(str::to_string)).collect()))
        .unwrap_or_else(|| vec!["Inter".into(), "Noto Sans Mono".into()]);
    v.extend(ir_fonts.iter().filter(|f| f.file.is_some()).map(|f| f.family.clone()));
    v.dedup();
    v
}

/// `W_FONT_FAMILY`: a text `font=` that names no registered or declared family (the layout
/// would silently fall back to Inter).
pub fn lint_families(nodes: &[&Node], ir_fonts: &[Font], d: &mut Vec<Diag>) {
    let fams = known_families(ir_fonts);
    for n in nodes {
        if matches!(n.kind.as_str(), "captions" | "form" | "cloud" | "font" | "video" | "theme") {
            continue;
        }
        let Some(f) = praw(n, "font") else { continue };
        let f = f.trim_matches(['"', '\'']);
        if f == "mono" || f.starts_with('$') || matches!(f, "sans-serif" | "serif" | "monospace") || fams.iter().any(|x| x.eq_ignore_ascii_case(f)) {
            continue;
        }
        let names: Vec<&str> = fams.iter().map(|s| s.as_str()).collect();
        let hint = crate::values::suggest(f, &names).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("registered: {}; or declare it: `font \"{f}\" src=\"fonts/{f}.woff2\"`", names.join(", ")));
        d.push(Diag::warn("W_FONT_FAMILY", format!("font `{f}` is not registered (Inter is used)"), hint, n.span));
    }
}
