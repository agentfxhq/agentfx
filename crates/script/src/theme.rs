//! Brand themes: color/type/motion tokens. Built-ins + a `theme` block in the document
//! that overrides any token. Tokens are referenced as `$name` anywhere a color, size,
//! duration or ease is expected.
use crate::ast::{Document, Node, Value};
use crate::diag::Diag;
use crate::values::{parse_color, parse_dur, parse_ease, parse_paint, ColorRef, Ease, Paint, Rgba, Stop};
use indexmap::IndexMap;

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub colors: IndexMap<String, Rgba>,
    pub bg: Paint,
    pub font: String,
    pub mono: String,
    pub sizes: IndexMap<String, f64>,
    pub durs: IndexMap<String, f64>,
    pub eases: IndexMap<String, Ease>,
    pub radius: f64,
    /// safe area margins as fractions of width / height
    pub safe: (f64, f64),
    /// smallest legible font size (px at 1080 short side)
    pub min_font: f64,
    pub min_contrast: f64,
    pub no_emdash: bool,
    /// taste lint thresholds (`lint min-area= max-still= ...`)
    pub taste: crate::taste::TasteCfg,
    /// design layer (L2): surface, elevation, type roles, scale, treatments, do/don't
    pub d: crate::design::Design,
}

pub const MOTION_TOKENS: &str = include_str!("../../../design/motion-tokens.json");

pub fn hex(s: &str) -> Rgba {
    match parse_color(s) {
        Ok(ColorRef::Lit(c)) => c,
        _ => Rgba { r: 1.0, g: 0.0, b: 1.0, a: 1.0 },
    }
}

impl Theme {
    pub fn builtin(name: &str) -> Option<Theme> {
        // `studio-*` are the names; `dark`/`light` are short forms. `levels-*` are the pre-release
        // names, kept as undocumented aliases so older documents keep working.
        let dark = matches!(name, "studio-dark" | "dark" | "levels-dark");
        let light = matches!(name, "studio-light" | "light" | "levels-light");
        if !dark && !light {
            return None;
        }
        let mut colors = IndexMap::new();
        let c = |m: &mut IndexMap<String, Rgba>, k: &str, v: &str| {
            m.insert(k.to_string(), hex(v));
        };
        if dark {
            c(&mut colors, "bg", "#0c2a1f");
            c(&mut colors, "bg2", "#10392a");
            c(&mut colors, "surface", "#ffffff14");
            c(&mut colors, "surface2", "#ffffff24");
            c(&mut colors, "border", "#ffffff29");
            c(&mut colors, "text", "#ffffff");
            c(&mut colors, "muted", "#c5ecd9");
            c(&mut colors, "faint", "#ffffff8c");
            c(&mut colors, "mint", "#8fe0bb");
            c(&mut colors, "accent", "#17845a");
            c(&mut colors, "accent2", "#3cbf87");
            c(&mut colors, "danger", "#ff6b6b");
            c(&mut colors, "warn", "#ffc857");
            c(&mut colors, "ink", "#15181d");
            c(&mut colors, "paper", "#ffffff");
            c(&mut colors, "code-bg", "#0b1f17");
        } else {
            c(&mut colors, "bg", "#f6f7f9");
            c(&mut colors, "bg2", "#eef1f4");
            c(&mut colors, "surface", "#ffffff");
            c(&mut colors, "surface2", "#f0f2f5");
            c(&mut colors, "border", "#d9dce2");
            c(&mut colors, "text", "#15181d");
            c(&mut colors, "muted", "#5d6572");
            c(&mut colors, "faint", "#8b929e");
            c(&mut colors, "mint", "#17845a");
            c(&mut colors, "accent", "#17845a");
            c(&mut colors, "accent2", "#3cbf87");
            c(&mut colors, "danger", "#d93636");
            c(&mut colors, "warn", "#b7791f");
            c(&mut colors, "ink", "#15181d");
            c(&mut colors, "paper", "#ffffff");
            c(&mut colors, "code-bg", "#15181d");
        }
        let bg = if dark {
            parse_paint("linear(155deg, #0c2a1f 0%, #10392a 50%, #17845a 140%)").unwrap()
        } else {
            Paint::Solid(ColorRef::Lit(hex("#f6f7f9")))
        };
        let mut sizes = IndexMap::new();
        for (k, v) in [("display", 104.0), ("h1", 92.0), ("h2", 64.0), ("h3", 40.0), ("body", 33.0), ("small", 25.0), ("caption", 33.0), ("label", 22.0), ("code", 28.0)] {
            sizes.insert(k.to_string(), v);
        }
        let mut durs = IndexMap::new();
        for (k, v) in [("fast", 0.25), ("base", 0.5), ("slow", 0.9), ("move", 0.7), ("hold", 1.6)] {
            durs.insert(k.to_string(), v);
        }
        let mut eases = IndexMap::new();
        for (k, v) in [("std", "inout-cubic"), ("out", "out-cubic"), ("in", "in-cubic"), ("pop", "spring(260, 20)"), ("soft", "spring(120, 20)")] {
            eases.insert(k.to_string(), parse_ease(v).unwrap());
        }
        // motion tokens v0, measured by `agentfx motion mine` (design/motion-tokens.json, M3-style names)
        let tok: serde_json::Value = serde_json::from_str(MOTION_TOKENS).unwrap_or_default();
        for (k, v) in tok["duration"].as_object().into_iter().flatten() {
            durs.insert(k.clone(), v.as_f64().unwrap_or(0.4));
        }
        for (k, v) in tok["easing"].as_object().into_iter().flatten() {
            if let Some(e) = v.as_str().and_then(|s| parse_ease(s).ok()) {
                eases.insert(k.clone(), e);
            }
        }
        for role in ["enter", "exit", "move", "emphasis"] {
            if let Some(d) = tok["roles"][role]["duration"].as_str().and_then(|n| tok["duration"][n].as_f64()) {
                durs.insert(role.to_string(), d);
            }
        }
        durs.insert("stagger".into(), tok["roles"]["stagger"]["item"].as_f64().unwrap_or(0.133));
        durs.insert("beat".into(), tok["roles"]["stagger"]["beat"].as_f64().unwrap_or(0.43));
        let mut d = crate::design::Design::default();
        d.enter_distance = tok["roles"]["enter"]["distance_px"].as_f64().unwrap_or(28.0);
        Some(Theme {
            name: name.into(),
            colors,
            bg,
            font: "Inter".into(),
            mono: "Noto Sans Mono".into(),
            sizes,
            durs,
            eases,
            radius: 18.0,
            safe: (0.06, 0.06),
            min_font: 20.0,
            min_contrast: 3.0,
            no_emdash: true,
            taste: crate::taste::TasteCfg::default(),
            d,
        })
    }

    /// Resolve the document's theme: `video theme=<builtin>` then a top-level `theme` block.
    pub fn from_doc(doc: &Document, diags: &mut Vec<Diag>) -> Theme {
        let video = doc.nodes.iter().find(|n| n.kind == "video");
        let base = video.and_then(|v| v.props.get("theme")).map(|v| v.raw()).unwrap_or_else(|| "studio-dark".into());
        let mut th = Theme::builtin(&base).unwrap_or_else(|| {
            if let Some(v) = video {
                diags.push(Diag::error("E_THEME", format!("unknown theme `{base}`"), "use theme=studio-dark or theme=studio-light (or define tokens in a `theme` block)", v.span));
            }
            Theme::builtin("studio-dark").unwrap()
        });
        // design specs first (video design=<lens>, `design` nodes), then `theme` blocks override
        let mut designs: Vec<Node> = vec![];
        if let Some(l) = video.and_then(|v| v.props.get("design")) {
            let mut n = Node { kind: "design".into(), span: video.map(|v| v.span).unwrap_or_default(), ..Default::default() };
            let raw = l.raw();
            let key = if raw.ends_with(".reel") { "src" } else { "lens" };
            n.props.insert(key.into(), Value::Str(raw));
            designs.push(n);
        }
        designs.extend(doc.nodes.iter().filter(|n| n.kind == "design").cloned());
        for d in &designs {
            th.apply_design(d, diags, 0);
        }
        for t in doc.nodes.iter().filter(|n| n.kind == "theme") {
            if let Some(name) = t.args.first().and_then(|v| v.as_str()) {
                if let Some(b) = Theme::builtin(name) {
                    let dd = th.d.clone();
                    th = b;
                    th.d = dd;
                }
            }
            for line in &t.children {
                th.apply_line(line, diags);
            }
        }
        th.type_scale();
        if let Some(b) = th.d.body_font.clone() {
            th.font = b;
        }
        th
    }

    /// Apply a `design` node: its source file (src= / lens=) first, then its own lines.
    pub fn apply_design(&mut self, n: &Node, diags: &mut Vec<Diag>, depth: usize) {
        if depth > 4 {
            return;
        }
        if let Some(b) = n.props.get("base").map(|v| v.raw()) {
            match Theme::builtin(&b) {
                Some(t) => {
                    let dd = self.d.clone();
                    *self = t;
                    self.d = dd;
                }
                None => diags.push(Diag::error("E_DESIGN", format!("unknown base `{b}`"), "base=dark|light (studio-dark, studio-light)", n.span)),
            }
        }
        // inside a loaded design file, `lens=`/`src=` on its own header are metadata, not includes
        // (a lens file names itself: following it would load it again and again)
        let src = if depth == 0 { crate::design::source_of(n) } else { None };
        match src {
            Some(Ok(p)) => {
                for m in crate::design::load_file(&p, n.span, diags) {
                    if m.kind == "design" {
                        self.apply_design(&m, diags, depth + 1);
                    } else {
                        for line in &m.children {
                            self.apply_line(line, diags);
                        }
                    }
                }
            }
            Some(Err(e)) => diags.push(Diag::error("E_DESIGN", e, "design src=path/design.reel or design lens=<name>", n.span)),
            None => {}
        }
        if let Some(name) = n.args.first().and_then(|v| v.as_str()) {
            self.d.name = Some(name.to_string());
        }
        if let Some(l) = n.props.get("lens") {
            self.d.lens = Some(l.raw());
        }
        self.d.loaded = true;
        for line in &n.children {
            self.apply_line(line, diags);
        }
    }

    /// Fill the type sizes from `type base= ratio=` (sizes a design set explicitly win).
    fn type_scale(&mut self) {
        let (Some(b), Some(r)) = (self.d.base, self.d.ratio) else { return };
        let r = r.clamp(1.05, 2.0);
        for (k, v) in [("body", b), ("h3", b * r), ("h2", b * r * r), ("h1", b * r.powi(3)), ("display", b * r.powi(4)), ("small", b / r), ("label", (b / r.powf(1.5)).max(20.0))] {
            if !self.d.explicit_sizes.iter().any(|e| e == k) {
                self.sizes.insert(k.to_string(), v.round());
            }
        }
    }

    /// Font family for a text role (`display` for titles and headings, `body` otherwise).
    pub fn role_font(&self, display: bool) -> String {
        if display { self.d.display_font.clone().unwrap_or_else(|| self.font.clone()) } else { self.d.body_font.clone().unwrap_or_else(|| self.font.clone()) }
    }

    fn apply_line(&mut self, n: &Node, diags: &mut Vec<Diag>) {
        if self.d.apply_line(n) {
            for (what, hint, span) in std::mem::take(&mut self.d.problems) {
                diags.push(Diag::error("E_DESIGN_TOKEN", format!("bad design token {what}"), hint, span));
            }
            return;
        }
        if !crate::design::LINE_KINDS.contains(&n.kind.as_str()) && n.kind != "design" {
            let hint = crate::values::suggest(&n.kind, crate::design::LINE_KINDS).map(|x| format!("did you mean `{x}`?")).unwrap_or(format!("lines: {}", crate::design::LINE_KINDS.join(", ")));
            diags.push(Diag::error("E_DESIGN_TOKEN", format!("unknown theme/design line `{}`", n.kind), hint, n.span));
            return;
        }
        for (k, v) in &n.props {
            let raw = v.raw();
            let bad = |diags: &mut Vec<Diag>, what: &str, hint: &str| diags.push(Diag::error("E_THEME_TOKEN", format!("bad {what} token `{k}={raw}`"), hint, n.span));
            match n.kind.as_str() {
                "colors" | "color" => match parse_color(&raw) {
                    Ok(ColorRef::Lit(c)) => {
                        self.colors.insert(k.clone(), c);
                    }
                    Ok(ColorRef::Token(t)) => {
                        if let Some(c) = self.colors.get(&t).cloned() {
                            self.colors.insert(k.clone(), c);
                        }
                    }
                    Err(e) => bad(diags, "color", &e.hint),
                },
                "type" | "sizes" if self.d.type_key(k, &raw) => {}
                "type" | "sizes" => match k.as_str() {
                    "font" => self.font = raw,
                    "mono" => self.mono = raw,
                    "min" => self.min_font = raw.trim_end_matches("px").parse().unwrap_or(self.min_font),
                    _ => match raw.trim_end_matches("px").parse::<f64>() {
                        Ok(px) => {
                            self.sizes.insert(k.clone(), px);
                            self.d.explicit_sizes.push(k.clone());
                        }
                        Err(_) => bad(diags, "size", "write a px size like `h1=92`"),
                    },
                },
                "motion" | "durs" | "eases" if k == "distance" => self.d.enter_distance = crate::design::num(&raw).unwrap_or(28.0),
                "motion" | "durs" | "eases" => {
                    if let Ok(d) = parse_dur(&raw) {
                        if raw.ends_with('s') || raw.ends_with('f') {
                            self.durs.insert(k.clone(), d.secs(30.0));
                            continue;
                        }
                    }
                    match parse_ease(&raw) {
                        Ok(e) => {
                            self.eases.insert(k.clone(), e);
                        }
                        Err(e) => bad(diags, "motion", &e.hint),
                    }
                }
                "layout" => match k.as_str() {
                    "radius" => self.radius = raw.trim_end_matches("px").parse().unwrap_or(self.radius),
                    "safe" => {
                        let p = raw.trim_end_matches('%').parse::<f64>().unwrap_or(6.0) / 100.0;
                        self.safe = (p, p);
                    }
                    "safe-x" => self.safe.0 = raw.trim_end_matches('%').parse::<f64>().unwrap_or(6.0) / 100.0,
                    "safe-y" => self.safe.1 = raw.trim_end_matches('%').parse::<f64>().unwrap_or(6.0) / 100.0,
                    "radius-sm" => self.d.radius_sm = crate::design::num(&raw),
                    "radius-pill" => {}
                    "margin" => {
                        let p = raw.trim_end_matches('%').parse::<f64>().unwrap_or(6.0) / 100.0;
                        self.safe = (p, p.max(0.05));
                    }
                    "grid" | "cols" => self.d.grid_cols = raw.parse().unwrap_or(12),
                    "gutter" => self.d.gutter = crate::design::num(&raw).unwrap_or(24.0),
                    _ => bad(diags, "layout", "layout tokens: radius, radius-sm, safe, safe-x, safe-y, margin, grid, gutter"),
                },
                "lint" => match k.as_str() {
                    "min-contrast" => self.min_contrast = raw.parse().unwrap_or(self.min_contrast),
                    "no-emdash" => self.no_emdash = raw != "false",
                    t @ ("motion" | "min-area" | "max-still" | "max-still-silent" | "small-span" | "end-window" | "end-still") => {
                        if let Err(e) = self.taste.set(t, &raw) {
                            bad(diags, "lint", &e);
                        }
                    }
                    _ => bad(diags, "lint", "lint tokens: min-contrast, no-emdash, motion, min-area, max-still, max-still-silent, small-span, end-window, end-still"),
                },
                _ => {}
            }
        }
        if n.kind == "bg" {
            if let Some(a) = n.args.first() {
                match parse_paint(&a.raw()) {
                    Ok(p) => self.bg = p,
                    Err(e) => diags.push(Diag::error(&e.code, e.message, e.hint, n.span)),
                }
            }
        }
    }

    pub fn color(&self, c: &ColorRef) -> Option<Rgba> {
        match c {
            ColorRef::Lit(c) => Some(c.clone()),
            ColorRef::Token(t) => self.colors.get(t).cloned(),
        }
    }
    /// Resolve tokens inside a paint into literals.
    pub fn paint(&self, p: &Paint) -> Paint {
        let fix = |c: &ColorRef| ColorRef::Lit(self.color(c).unwrap_or_else(|| hex("#ff00ff")));
        let stops = |s: &Vec<Stop>| s.iter().map(|s| Stop { color: fix(&s.color), pos: s.pos }).collect();
        match p {
            Paint::Solid(c) => Paint::Solid(fix(c)),
            Paint::Linear { angle_deg, stops: s } => Paint::Linear { angle_deg: *angle_deg, stops: stops(s) },
            Paint::Radial { cx, cy, stops: s } => Paint::Radial { cx: *cx, cy: *cy, stops: stops(s) },
            Paint::Conic { angle_deg, stops: s } => Paint::Conic { angle_deg: *angle_deg, stops: stops(s) },
            Paint::Mesh { cols, rows, colors } => Paint::Mesh { cols: *cols, rows: *rows, colors: colors.iter().map(fix).collect() },
            Paint::Image { .. } => p.clone(),
        }
    }
    /// A representative solid color of a paint (for contrast checks).
    pub fn paint_avg(&self, p: &Paint) -> Option<Rgba> {
        let avg = |cs: Vec<Rgba>| {
            if cs.is_empty() {
                return None;
            }
            let n = cs.len() as f64;
            // alpha-weighted color, mean alpha (translucent gradients composite over what is below)
            let wa: f64 = cs.iter().map(|c| c.a).sum::<f64>().max(1e-9);
            Some(Rgba { r: cs.iter().map(|c| c.r * c.a).sum::<f64>() / wa, g: cs.iter().map(|c| c.g * c.a).sum::<f64>() / wa, b: cs.iter().map(|c| c.b * c.a).sum::<f64>() / wa, a: wa / n })
        };
        match p {
            Paint::Solid(c) => self.color(c),
            Paint::Linear { stops, .. } | Paint::Radial { stops, .. } | Paint::Conic { stops, .. } => avg(stops.iter().filter_map(|s| self.color(&s.color)).collect()),
            Paint::Mesh { colors, .. } => avg(colors.iter().filter_map(|c| self.color(c)).collect()),
            Paint::Image { .. } => None,
        }
    }
    pub fn size(&self, name: &str) -> Option<f64> {
        self.sizes.get(name).copied()
    }
    pub fn dur(&self, name: &str) -> f64 {
        self.durs.get(name).copied().unwrap_or(0.5)
    }
    pub fn ease(&self, name: &str) -> Ease {
        self.eases.get(name).cloned().unwrap_or(Ease::Cubic(0.65, 0.0, 0.35, 1.0))
    }
}
