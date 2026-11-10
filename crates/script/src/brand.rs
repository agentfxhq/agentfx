//! Brand kits (P5): one JSON file with a company's colours, fonts, logo variants, motion and
//! layout tokens. `brand "brand.json"` (top level) expands, before compiling, into the plain
//! theme lines and `font` declarations it stands for (so every backend, lint and the catalog
//! see ordinary tokens): colours become `$primary $secondary $highlight $fg $bg` + the named
//! ones and drive the theme roles (`accent accent2 mint text muted surface border ...`), logo
//! files become asset tokens (`src=$logo`, the `logo` element). Paths are relative to the
//! brand file. Theme lines written after the `brand` line override it.
//!
//! Also here: `agentfx brand init` (template) and `agentfx brand from-figma` (Figma variables
//! JSON, REST or MCP export, or W3C design tokens -> brand kit).
use crate::ast::{Document, Node, Span, Value};
use crate::diag::Diag;
use crate::values::{parse_color, suggest, ColorRef, Rgba};
use serde_json::{json, Map, Value as J};
use std::path::{Path, PathBuf};

pub const KEYS: &[&str] = &["name", "base", "colors", "background", "fonts", "logo", "assets", "motion", "layout", "type", "$schema"];
pub const COLOR_ROLES: &[&str] = &["primary", "secondary", "accent", "bg", "fg"];
pub const FONT_ROLES: &[&str] = &["display", "body", "mono"];
pub const LOGO_KEYS: &[&str] = &["src", "on-dark", "on-light", "mark", "mark-on-dark", "mark-on-light"];
pub const MOTION_KEYS: &[&str] = &["ease", "ease-enter", "ease-exit", "ease-move", "enter", "exit", "move", "fast", "base", "slow", "hold", "stagger", "beat", "distance"];
pub const LAYOUT_KEYS: &[&str] = &["safe", "safe-x", "safe-y", "margin", "radius", "radius-sm", "grid", "gutter"];

fn opaque(c: &Rgba) -> Rgba {
    Rgba { a: 1.0, ..c.clone() }
}
fn mix(a: &Rgba, b: &Rgba, t: f64) -> Rgba {
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}
fn alpha(c: &Rgba, a: f64) -> Rgba {
    Rgba { a, ..c.clone() }
}
fn css(c: &Rgba) -> String {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    if c.a >= 0.999 {
        format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b), b(c.a))
    }
}

/// Token names: lower-case, `[a-z0-9-]` (spaces, `/`, `_`, `.` become `-`).
pub fn token_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

struct Ex<'a> {
    diags: &'a mut Vec<Diag>,
    span: Span,
    dir: PathBuf,
    file: String,
}
impl Ex<'_> {
    fn err(&mut self, code: &str, msg: String, hint: impl Into<String>) {
        self.diags.push(Diag::error(code, format!("brand {}: {msg}", self.file), hint, self.span));
    }
    fn warn(&mut self, code: &str, msg: String, hint: impl Into<String>) {
        self.diags.push(Diag::warn(code, format!("brand {}: {msg}", self.file), hint, self.span));
    }
    fn unknown(&mut self, what: &str, k: &str, valid: &[&str]) {
        let hint = suggest(k, valid).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("{what} keys: {}", valid.join(", ")));
        self.err("E_BRAND_KEY", format!("unknown {what} key `{k}`"), hint);
    }
    /// An existing file, relative to the brand file (absolute string), or an error.
    fn file(&mut self, key: &str, rel: &str, exts: &[&str]) -> Option<String> {
        let p = self.dir.join(rel);
        if !p.is_file() {
            self.err("E_BRAND_FILE", format!("{key}: file `{rel}` not found"), format!("paths are relative to the brand file ({}); check the name", self.dir.display()));
            return None;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !exts.is_empty() && !exts.contains(&ext.as_str()) {
            self.err("E_BRAND_FILE", format!("{key}: `{rel}` is not a {} file", exts.join("/")), format!("use a .{} file", exts.join(", .")));
            return None;
        }
        let abs = agentfx_ir::paths::canonicalize(&p).unwrap_or(p);
        Some(abs.to_string_lossy().into_owned())
    }
    fn color(&mut self, key: &str, v: &J) -> Option<Rgba> {
        let s = match v {
            J::String(s) => s.clone(),
            _ => {
                self.err("E_BRAND_COLOR", format!("colors.{key}: expected a color string"), "write a color like \"#0a5cff\"");
                return None;
            }
        };
        match parse_color(&s) {
            Ok(ColorRef::Lit(c)) => Some(c),
            Ok(ColorRef::Token(_)) => {
                self.err("E_BRAND_COLOR", format!("colors.{key}: `{s}` is a token"), "brand colors are literal colors (#rrggbb, rgb(...), hsl(...))");
                None
            }
            Err(e) => {
                self.err("E_BRAND_COLOR", format!("colors.{key}: invalid color `{s}`"), e.hint);
                None
            }
        }
    }
}

fn str_of(v: &J) -> Option<String> {
    match v {
        J::String(s) => Some(s.clone()),
        J::Number(n) => Some(n.to_string()),
        J::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn line(kind: &str, props: Vec<(String, String)>, span: Span) -> Node {
    let mut n = Node { kind: kind.into(), span, ..Default::default() };
    for (k, v) in props {
        n.props.insert(k, Value::Str(v));
    }
    n
}

/// The brand file a `brand` node names (`brand "x.json"` or `brand src=x.json`).
fn src_of(n: &Node) -> Option<String> {
    n.props.get("src").map(|v| v.raw()).or_else(|| n.args.first().map(|v| v.raw())).map(|s| s.trim_matches('"').to_string())
}

/// Replace every top-level `brand` node by the theme block and `font` lines it stands for.
/// `base`: the document's directory. Idempotent (expanded documents have no `brand` nodes).
pub fn expand(doc: &mut Document, base: &Path, diags: &mut Vec<Diag>) {
    if !doc.nodes.iter().any(|n| n.kind == "brand") {
        return;
    }
    let video_theme = doc.nodes.iter().find(|n| n.kind == "video").is_some_and(|v| v.props.contains_key("theme") || v.props.contains_key("design"));
    let has_design = doc.nodes.iter().any(|n| n.kind == "design");
    let mut out = Vec::with_capacity(doc.nodes.len() + 4);
    let mut fams: Vec<String> = doc.nodes.iter().filter(|n| n.kind == "font").filter_map(|n| n.args.first().map(|a| a.raw().trim_matches('"').to_string())).collect();
    for n in std::mem::take(&mut doc.nodes) {
        if n.kind != "brand" {
            out.push(n);
            continue;
        }
        let Some(src) = src_of(&n) else {
            diags.push(Diag::error("E_BRAND", "`brand` needs a file", "write `brand \"brand.json\"` (create one with `agentfx brand init`)", n.span));
            continue;
        };
        let path = base.join(&src);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => {
                diags.push(Diag::error("E_BRAND", format!("brand file `{src}` not found"), "paths are relative to the .reel file; create one with `agentfx brand init -o brand.json`", n.span));
                continue;
            }
        };
        let j: J = match serde_json::from_str(&text) {
            Ok(j) => j,
            Err(e) => {
                diags.push(Diag::error("E_BRAND", format!("brand `{src}` is not valid JSON: {e}"), "fix the JSON (line/column above); `agentfx brand init` writes a valid template", n.span));
                continue;
            }
        };
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| base.to_path_buf());
        let mut ex = Ex { diags, span: n.span, dir, file: src.clone() };
        let (theme, fonts) = kit(&j, &mut ex, !video_theme && !has_design, &mut fams);
        out.extend(fonts);
        out.push(theme);
    }
    doc.nodes = out;
}

/// One brand kit -> (theme node, font nodes).
fn kit(j: &J, ex: &mut Ex, set_base: bool, fams: &mut Vec<String>) -> (Node, Vec<Node>) {
    let span = ex.span;
    let mut theme = Node { kind: "theme".into(), span, ..Default::default() };
    let mut fonts = vec![];
    let Some(o) = j.as_object() else {
        ex.err("E_BRAND", "the file must hold a JSON object".into(), "start from `agentfx brand init`");
        return (theme, fonts);
    };
    for k in o.keys() {
        if !KEYS.contains(&k.as_str()) {
            ex.unknown("brand", k, KEYS);
        }
    }
    // colours
    let mut named: Vec<(String, Rgba)> = vec![];
    let (mut primary, mut secondary, mut accent, mut bg, mut fg) = (None, None, None, None, None);
    if let Some(c) = o.get("colors") {
        match c.as_object() {
            Some(c) => {
                for (k, v) in c {
                    if k == "named" {
                        for (nk, nv) in v.as_object().into_iter().flatten() {
                            if let Some(col) = ex.color(&format!("named.{nk}"), nv) {
                                named.push((token_name(nk), col));
                            }
                        }
                        continue;
                    }
                    let Some(col) = ex.color(k, v) else { continue };
                    match k.as_str() {
                        "primary" => primary = Some(col),
                        "secondary" => secondary = Some(col),
                        "accent" | "highlight" => accent = Some(col),
                        "bg" | "background" => bg = Some(col),
                        "fg" | "foreground" | "text" => fg = Some(col),
                        _ => named.push((token_name(k), col)),
                    }
                }
            }
            None => ex.err("E_BRAND", "`colors` must be an object".into(), "write \"colors\": {\"primary\": \"#0a5cff\", \"bg\": \"#0b1020\", \"fg\": \"#ffffff\"}"),
        }
    }
    // the canvas: colors.bg, else the literal colors of the `background` paint
    let canvas_lum = bg.as_ref().map(|c| c.luminance()).or_else(|| {
        let p = crate::values::parse_paint(o.get("background")?.as_str()?).ok()?;
        let cs: Vec<Rgba> = match p {
            crate::values::Paint::Solid(ColorRef::Lit(c)) => vec![c],
            crate::values::Paint::Linear { stops, .. } | crate::values::Paint::Radial { stops, .. } | crate::values::Paint::Conic { stops, .. } => {
                stops.into_iter().filter_map(|s| if let ColorRef::Lit(c) = s.color { Some(c) } else { None }).collect()
            }
            _ => vec![],
        };
        (!cs.is_empty()).then(|| cs.iter().map(|c| c.luminance()).sum::<f64>() / cs.len() as f64)
    });
    let dark = match o.get("base").and_then(J::as_str) {
        Some("dark") => true,
        Some("light") => false,
        Some(b) => {
            ex.err("E_BRAND_KEY", format!("base `{b}`"), "base is \"dark\" or \"light\" (default: from the bg color)");
            canvas_lum.map(|l| l < 0.4).unwrap_or(true)
        }
        None => canvas_lum.map(|l| l < 0.4).unwrap_or(true),
    };
    if set_base {
        theme.args.push(Value::Str(if dark { "studio-dark".into() } else { "studio-light".into() }));
    }
    let mut cols: Vec<(String, String)> = vec![];
    let white = Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    let black = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    if let Some(p) = &primary {
        let sec = secondary.clone().unwrap_or_else(|| mix(p, if dark { &white } else { &black }, 0.25));
        cols.push(("accent".into(), css(p)));
        cols.push(("accent2".into(), css(&sec)));
        cols.push(("primary".into(), css(p)));
        cols.push(("secondary".into(), css(&sec)));
        let hi = accent.clone().unwrap_or_else(|| if secondary.is_some() { sec.clone() } else { mix(p, if dark { &white } else { &black }, 0.45) });
        cols.push(("mint".into(), css(&hi)));
        cols.push(("highlight".into(), css(&hi)));
    } else {
        if let Some(s) = &secondary {
            cols.push(("accent2".into(), css(s)));
            cols.push(("secondary".into(), css(s)));
        }
        if let Some(a) = &accent {
            cols.push(("mint".into(), css(a)));
            cols.push(("highlight".into(), css(a)));
        }
    }
    let fg_c = fg.clone().unwrap_or_else(|| if dark { white.clone() } else { Rgba { r: 0.082, g: 0.094, b: 0.114, a: 1.0 } });
    if let Some(b) = &bg {
        let b = opaque(b);
        cols.push(("bg".into(), css(&b)));
        cols.push(("bg2".into(), css(&mix(&b, &fg_c, 0.06))));
        cols.push(("code-bg".into(), css(&mix(&b, &black, 0.35))));
        if dark {
            cols.push(("surface".into(), css(&alpha(&fg_c, 0.08))));
            cols.push(("surface2".into(), css(&alpha(&fg_c, 0.14))));
            cols.push(("border".into(), css(&alpha(&fg_c, 0.16))));
        } else {
            cols.push(("surface".into(), css(&mix(&b, &white, 0.7))));
            cols.push(("surface2".into(), css(&mix(&b, &fg_c, 0.04))));
            cols.push(("border".into(), css(&mix(&b, &fg_c, 0.15))));
        }
        cols.push(("muted".into(), css(&mix(&fg_c, &b, 0.28))));
        cols.push(("faint".into(), css(&alpha(&fg_c, 0.55))));
    }
    if let Some(f) = &fg {
        cols.push(("text".into(), css(f)));
        cols.push(("fg".into(), css(f)));
    }
    for (k, c) in &named {
        if k.is_empty() {
            continue;
        }
        cols.push((k.clone(), css(c)));
    }
    if !cols.is_empty() {
        theme.children.push(line("colors", cols, span));
    }
    // background paint
    match o.get("background").and_then(J::as_str) {
        Some(p) => match crate::values::parse_paint(p) {
            Ok(_) => theme.children.push(Node { kind: "bg".into(), args: vec![Value::Str(p.into())], span, ..Default::default() }),
            Err(e) => ex.err("E_BRAND_COLOR", format!("background `{p}`: {}", e.message), e.hint),
        },
        None => {
            if let Some(b) = &bg {
                theme.children.push(Node { kind: "bg".into(), args: vec![Value::Str(css(&opaque(b)))], span, ..Default::default() });
            }
        }
    }
    // fonts
    let mut type_props: Vec<(String, String)> = vec![];
    if let Some(f) = o.get("fonts") {
        for (role, v) in f.as_object().into_iter().flatten() {
            if !FONT_ROLES.contains(&role.as_str()) {
                ex.unknown("fonts", role, FONT_ROLES);
                continue;
            }
            let (family, src) = match v {
                J::String(s) => (s.clone(), None),
                J::Object(m) => (m.get("family").and_then(J::as_str).unwrap_or("").to_string(), m.get("src").and_then(J::as_str).map(str::to_string)),
                _ => (String::new(), None),
            };
            if family.is_empty() || family.contains(['\'', '"', ';', '{', '}']) {
                ex.err("E_BRAND_FONT", format!("fonts.{role}: needs a family name"), "write \"display\": {\"family\": \"Acme Sans\", \"src\": \"fonts/AcmeSans.woff2\"}");
                continue;
            }
            if let Some(src) = src {
                if let Some(abs) = ex.file(&format!("fonts.{role}.src"), &src, &["ttf", "otf", "ttc", "woff", "woff2"]) {
                    if !fams.contains(&family) {
                        fams.push(family.clone());
                        fonts.push(Node { kind: "font".into(), args: vec![Value::Str(family.clone())], props: [("src".to_string(), Value::Str(abs))].into_iter().collect(), span, ..Default::default() });
                    }
                }
            }
            match role.as_str() {
                "mono" => type_props.push(("mono".into(), family)),
                r => type_props.push((r.into(), family)),
            }
        }
    }
    if let Some(t) = o.get("type").and_then(J::as_object) {
        for (k, v) in t {
            if let Some(s) = str_of(v) {
                type_props.push((k.clone(), s));
            }
        }
    }
    if !type_props.is_empty() {
        theme.children.push(line("type", type_props, span));
    }
    // motion
    if let Some(m) = o.get("motion").and_then(J::as_object) {
        // eases and durations share names (`enter` is both): one line each
        let mut mp: Vec<(String, String)> = vec![];
        let mut me: Vec<(String, String)> = vec![];
        for (k, v) in m {
            let Some(s) = str_of(v) else { continue };
            match k.as_str() {
                "ease" => {
                    if crate::values::parse_ease(&s).is_err() {
                        ex.err("E_BRAND_MOTION", format!("motion.ease `{s}` is not an ease"), "e.g. \"out-cubic\", \"power3.out\", \"spring(170, 20)\", \"cubic(0.2, 0, 0, 1)\"");
                        continue;
                    }
                    for r in ["enter", "out", "std", "move"] {
                        me.push((r.into(), s.clone()));
                    }
                }
                "ease-enter" | "ease-exit" | "ease-move" => {
                    if crate::values::parse_ease(&s).is_err() {
                        ex.err("E_BRAND_MOTION", format!("motion.{k} `{s}` is not an ease"), "e.g. \"out-cubic\", \"power3.out\", \"spring(170, 20)\"");
                        continue;
                    }
                    me.push((k[5..].to_string(), s));
                }
                "distance" => mp.push((k.clone(), s)),
                k2 if MOTION_KEYS.contains(&k2) => {
                    // durations: bare numbers are seconds
                    let d = if s.parse::<f64>().is_ok() { format!("{s}s") } else { s };
                    if crate::values::parse_dur(&d).is_err() {
                        ex.err("E_BRAND_MOTION", format!("motion.{k} `{d}` is not a duration"), "write \"0.5s\", \"300ms\" or \"12f\"");
                        continue;
                    }
                    mp.push((k.clone(), d));
                }
                _ => ex.unknown("motion", k, MOTION_KEYS),
            }
        }
        for props in [mp, me] {
            // a later key wins (`ease-enter` after `ease`)
            let mut dedup: Vec<(String, String)> = vec![];
            for (k, v) in props {
                dedup.retain(|(a, _)| a != &k);
                dedup.push((k, v));
            }
            if !dedup.is_empty() {
                theme.children.push(line("motion", dedup, span));
            }
        }
    }
    // layout
    if let Some(l) = o.get("layout").and_then(J::as_object) {
        let mut lp = vec![];
        for (k, v) in l {
            match (LAYOUT_KEYS.contains(&k.as_str()), str_of(v)) {
                (true, Some(s)) => lp.push((k.clone(), s)),
                (true, None) => ex.err("E_BRAND_KEY", format!("layout.{k}: expected a number or string"), "e.g. \"safe\": \"6%\", \"radius\": 16"),
                _ => ex.unknown("layout", k, LAYOUT_KEYS),
            }
        }
        if !lp.is_empty() {
            theme.children.push(line("layout", lp, span));
        }
    }
    // logo + assets -> asset tokens
    let mut assets: Vec<(String, String)> = vec![];
    match o.get("logo") {
        Some(J::String(s)) => {
            if let Some(a) = ex.file("logo", s, &["svg", "png", "jpg", "jpeg"]) {
                assets.push(("logo".into(), a));
            }
        }
        Some(J::Object(m)) => {
            for (k, v) in m {
                if !LOGO_KEYS.contains(&k.as_str()) {
                    ex.unknown("logo", k, LOGO_KEYS);
                    continue;
                }
                let Some(s) = v.as_str() else { continue };
                if let Some(a) = ex.file(&format!("logo.{k}"), s, &["svg", "png", "jpg", "jpeg"]) {
                    let key = if k == "src" { "logo".to_string() } else if k.starts_with("mark") { k.clone() } else { format!("logo-{k}") };
                    assets.push((key, a));
                }
            }
        }
        Some(_) => ex.err("E_BRAND", "`logo` must be a path or an object".into(), "write \"logo\": {\"src\": \"logo.svg\", \"on-dark\": \"logo-white.svg\"}"),
        None => {}
    }
    // `$logo` follows the background: the on-dark/on-light variant when there is one
    let pick = |assets: &Vec<(String, String)>, base: &str| {
        let want = format!("{base}-on-{}", if dark { "dark" } else { "light" });
        assets.iter().find(|(k, _)| *k == want).or_else(|| assets.iter().find(|(k, _)| k == base)).map(|x| x.1.clone())
    };
    let mut resolved = assets.clone();
    for base in ["logo", "mark"] {
        if let Some(p) = pick(&assets, base) {
            resolved.retain(|(k, _)| k != base);
            resolved.push((base.to_string(), p));
        }
    }
    if let Some(m) = o.get("assets").and_then(J::as_object) {
        for (k, v) in m {
            if let Some(s) = v.as_str() {
                if let Some(a) = ex.file(&format!("assets.{k}"), s, &[]) {
                    resolved.push((token_name(k), a));
                }
            }
        }
    }
    if o.contains_key("logo") && !resolved.iter().any(|(k, _)| k == "logo") && ex.diags.iter().all(|d| d.code != "E_BRAND_FILE") {
        ex.warn("W_BRAND_LOGO", "no default logo (`src`) and no variant for this background".into(), "add \"src\" (or \"on-dark\"/\"on-light\") to `logo`");
    }
    if !resolved.is_empty() {
        theme.children.push(line("assets", resolved, span));
    }
    (theme, fonts)
}

/// `agentfx brand init`: a documented template (valid JSON; paths relative to the file).
pub fn template(name: &str) -> String {
    let j = json!({
        "name": name,
        "colors": {
            "primary": "#2f6bff",
            "secondary": "#7aa2ff",
            "accent": "#ffb547",
            "bg": "#0d1226",
            "fg": "#f4f6fb",
            "named": { "success": "#2fbf71", "danger": "#ff5a5f" }
        },
        "background": "linear(160deg, #0d1226 0%, #18214a 100%)",
        "fonts": {
            "display": { "family": "Inter" },
            "body": { "family": "Inter" }
        },
        "logo": { "src": "logo.svg" },
        "motion": { "ease": "out-cubic", "enter": "0.45s", "exit": "0.3s", "move": "0.6s", "stagger": "0.08s" },
        "layout": { "safe": "6%", "radius": 18 }
    });
    serde_json::to_string_pretty(&j).unwrap() + "\n"
}

/// The template's logo (a simple mark + wordmark with ids, ready for `svg ... parts`).
pub const TEMPLATE_LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 360 100" width="360" height="100">
  <g id="mark">
    <circle id="ring" cx="50" cy="50" r="38" fill="none" stroke="#2f6bff" stroke-width="12"/>
    <path id="dot" d="M50 34a16 16 0 1 1 0 32a16 16 0 1 1 0-32z" fill="#ffb547"/>
  </g>
  <g id="word">
    <path id="w1" d="M118 72V28h12l14 26 14-26h12v44h-11V47l-15 26-15-26v25z" fill="#f4f6fb"/>
    <path id="w2" d="M188 72V28h34v10h-23v7h21v10h-21v7h23v10z" fill="#f4f6fb"/>
    <path id="w3" d="M232 72V28h11v34h24v10z" fill="#f4f6fb"/>
    <path id="w4" d="M275 72V28h11v34h24v10z" fill="#f4f6fb"/>
  </g>
</svg>
"##;

// ------------------------------------------------------------------ Figma variables

/// Figma variables (REST `GET /v1/files/:key/variables/local`, the Figma MCP
/// `get_variable_defs` map, or W3C / Tokens Studio design tokens) -> a brand kit (JSON).
/// Colour roles come from the variable names (primary, secondary, accent/highlight,
/// background/bg/surface, foreground/fg/text); every colour is also a named token.
/// Returns (kit, notes).
pub fn from_figma(src: &J) -> (J, Vec<String>) {
    let mut notes = vec![];
    let mut colors: Vec<(String, Rgba)> = vec![];
    let mut floats: Vec<(String, f64)> = vec![];
    let mut strings: Vec<(String, String)> = vec![];
    if let Some(vars) = src.pointer("/meta/variables").and_then(J::as_object) {
        // REST: resolve aliases, take each collection's default mode
        let colls = src.pointer("/meta/variableCollections").and_then(J::as_object).cloned().unwrap_or_default();
        let resolve = |v: &J| -> Option<J> {
            let mut cur = v.clone();
            for _ in 0..8 {
                if cur.get("type").and_then(J::as_str) == Some("VARIABLE_ALIAS") {
                    let id = cur.get("id").and_then(J::as_str)?;
                    let tv = vars.get(id)?;
                    cur = default_value(tv, &colls)?;
                } else {
                    return Some(cur);
                }
            }
            None
        };
        let mut items: Vec<(&String, &J)> = vars.iter().collect();
        items.sort_by(|a, b| a.1.get("name").and_then(J::as_str).cmp(&b.1.get("name").and_then(J::as_str)));
        for (_, v) in items {
            if v.get("remote").and_then(J::as_bool) == Some(true) && v.get("hiddenFromPublishing").and_then(J::as_bool) == Some(true) {
                continue;
            }
            let name = v.get("name").and_then(J::as_str).unwrap_or("").to_string();
            let Some(val) = default_value(v, &colls).and_then(|x| resolve(&x)) else { continue };
            match v.get("resolvedType").and_then(J::as_str) {
                Some("COLOR") => {
                    if let Some(c) = rgba_obj(&val) {
                        colors.push((name, c));
                    }
                }
                Some("FLOAT") => {
                    if let Some(f) = val.as_f64() {
                        floats.push((name, f));
                    }
                }
                Some("STRING") => {
                    if let Some(s) = val.as_str() {
                        strings.push((name, s.to_string()));
                    }
                }
                _ => {}
            }
        }
    } else {
        flatten_tokens(src, "", &mut colors, &mut floats, &mut strings);
    }
    let mut kit = Map::new();
    let mut cmap = Map::new();
    let mut named = Map::new();
    let role_of = |name: &str| -> Option<&'static str> {
        let n = name.to_ascii_lowercase();
        let last = n.rsplit(['/', '.']).next().unwrap_or(&n).to_string();
        let has = |w: &str| last.split(|c: char| !c.is_ascii_alphanumeric()).any(|p| p == w) || last == w;
        if has("primary") || has("brand") {
            Some("primary")
        } else if has("secondary") {
            Some("secondary")
        } else if has("accent") || has("highlight") {
            Some("accent")
        } else if has("background") || has("bg") || last == "canvas" {
            Some("bg")
        } else if has("foreground") || has("fg") || last == "text" || last == "on-background" || last == "text-primary" {
            Some("fg")
        } else {
            None
        }
    };
    for (name, c) in &colors {
        let hex = css(c);
        if let Some(r) = role_of(name) {
            if !cmap.contains_key(r) {
                cmap.insert(r.to_string(), json!(hex.clone()));
                notes.push(format!("colors.{r} = {hex} (from `{name}`)"));
            }
        }
        let t = token_name(name);
        if !t.is_empty() && !COLOR_ROLES.contains(&t.as_str()) {
            named.insert(t, json!(hex));
        }
    }
    if !named.is_empty() {
        cmap.insert("named".into(), J::Object(named));
    }
    for r in ["primary", "bg", "fg"] {
        if !cmap.contains_key(r) {
            notes.push(format!("no variable looks like `{r}`: set colors.{r} by hand (any named color can be referenced as $name)"));
        }
    }
    kit.insert("colors".into(), J::Object(cmap));
    // fonts and radii
    let mut fonts = Map::new();
    for (name, s) in &strings {
        let n = name.to_ascii_lowercase();
        if n.contains("font") || n.contains("family") || n.contains("typeface") {
            let role = if n.contains("mono") || n.contains("code") { "mono" } else if n.contains("display") || n.contains("head") || n.contains("title") { "display" } else { "body" };
            if !fonts.contains_key(role) {
                fonts.insert(role.into(), json!({"family": s}));
                notes.push(format!("fonts.{role}.family = {s} (add \"src\": a local .woff2/.ttf the video may embed)"));
            }
        }
    }
    if !fonts.is_empty() {
        kit.insert("fonts".into(), J::Object(fonts));
    }
    if let Some((_, r)) = floats.iter().find(|(n, _)| n.to_ascii_lowercase().contains("radius")) {
        kit.insert("layout".into(), json!({"radius": r}));
    }
    (J::Object(kit), notes)
}

fn default_value(v: &J, colls: &Map<String, J>) -> Option<J> {
    let modes = v.get("valuesByMode")?.as_object()?;
    let coll = v.get("variableCollectionId").and_then(J::as_str).and_then(|c| colls.get(c));
    let def = coll.and_then(|c| c.get("defaultModeId")).and_then(J::as_str);
    match def.and_then(|d| modes.get(d)) {
        Some(x) => Some(x.clone()),
        None => modes.values().next().cloned(),
    }
}

/// Figma `{r, g, b, a}` (0..1) or a color string.
fn rgba_obj(v: &J) -> Option<Rgba> {
    if let Some(s) = v.as_str() {
        return match parse_color(s.trim()) {
            Ok(ColorRef::Lit(c)) => Some(c),
            _ => None,
        };
    }
    let f = |k: &str| v.get(k).and_then(J::as_f64);
    Some(Rgba { r: f("r")?, g: f("g")?, b: f("b")?, a: f("a").unwrap_or(1.0) })
}

/// Design tokens (W3C `$value`/`$type`, Tokens Studio `value`/`type`) or a flat
/// `{"Colors/Primary": "#0a5cff"}` map (Figma MCP `get_variable_defs`).
fn flatten_tokens(v: &J, path: &str, colors: &mut Vec<(String, Rgba)>, floats: &mut Vec<(String, f64)>, strings: &mut Vec<(String, String)>) {
    match v {
        J::Object(m) => {
            let val = m.get("$value").or_else(|| m.get("value").filter(|_| m.contains_key("type")));
            if let Some(val) = val {
                let ty = m.get("$type").or_else(|| m.get("type")).and_then(J::as_str).unwrap_or("");
                match (ty, val) {
                    ("color", x) => {
                        if let Some(c) = rgba_obj(x) {
                            colors.push((path.to_string(), c));
                        }
                    }
                    (_, J::Number(n)) => floats.push((path.to_string(), n.as_f64().unwrap_or(0.0))),
                    (_, J::String(s)) => match rgba_obj(val) {
                        Some(c) if ty.is_empty() => colors.push((path.to_string(), c)),
                        _ => {
                            if let Some(n) = s.trim_end_matches("px").parse::<f64>().ok() {
                                floats.push((path.to_string(), n));
                            } else {
                                strings.push((path.to_string(), s.clone()));
                            }
                        }
                    },
                    _ => {}
                }
                return;
            }
            for (k, x) in m {
                if k.starts_with('$') {
                    continue;
                }
                let p = if path.is_empty() { k.clone() } else { format!("{path}/{k}") };
                flatten_tokens(x, &p, colors, floats, strings);
            }
        }
        J::String(s) => {
            if let Some(c) = rgba_obj(v) {
                colors.push((path.to_string(), c));
            } else if let Ok(n) = s.trim_end_matches("px").parse::<f64>() {
                floats.push((path.to_string(), n));
            } else {
                // MCP font descriptors: `Font(family: "Inter", style: Bold, ...)`
                let fam = s.split("family:").nth(1).map(|r| r.trim().trim_start_matches('"').split('"').next().unwrap_or("").to_string());
                strings.push((path.to_string(), fam.filter(|f| !f.is_empty()).unwrap_or_else(|| s.clone())));
            }
        }
        J::Number(n) => floats.push((path.to_string(), n.as_f64().unwrap_or(0.0))),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentfx-brand-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn expands_into_theme_lines_and_fonts() {
        let d = tmpdir("expand");
        std::fs::write(d.join("logo.svg"), TEMPLATE_LOGO).unwrap();
        std::fs::write(d.join("logo-white.svg"), TEMPLATE_LOGO).unwrap();
        std::fs::write(
            d.join("brand.json"),
            r##"{"colors":{"primary":"#ff3300","bg":"#101010","fg":"#fafafa","named":{"Sky Blue":"#7cc4ff"}},
               "logo":{"src":"logo.svg","on-dark":"logo-white.svg"},"motion":{"ease":"power3.out","enter":0.6},"layout":{"safe":"8%"}}"##,
        )
        .unwrap();
        let (mut doc, _) = crate::parse::parse("video \"t\"\nbrand \"brand.json\"\nscene a\n  rect #r fill=$primary\n");
        let mut diags = vec![];
        expand(&mut doc, &d, &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
        assert!(!doc.nodes.iter().any(|n| n.kind == "brand"));
        let th = crate::theme::Theme::from_doc(&doc, &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(th.colors["primary"].to_css(), th.colors["accent"].to_css());
        assert!(th.colors.contains_key("sky-blue"));
        assert!((th.safe.0 - 0.08).abs() < 1e-9);
        assert!((th.dur("enter") - 0.6).abs() < 1e-9);
        // dark background: $logo is the on-dark variant
        assert!(th.assets["logo"].ends_with("logo-white.svg"), "{:?}", th.assets);
    }

    #[test]
    fn light_background_paint_picks_the_light_base() {
        let d = tmpdir("lightbg");
        std::fs::write(d.join("brand.json"), r##"{"background":"linear(180deg, #ffffff 0%, #eef0f4 100%)","colors":{"primary":"#0a5cff"}}"##).unwrap();
        let (mut doc, _) = crate::parse::parse("video \"t\"\nbrand \"brand.json\"\n");
        let mut diags = vec![];
        expand(&mut doc, &d, &mut diags);
        let th = doc.nodes.iter().find(|n| n.kind == "theme").unwrap();
        assert_eq!(th.args[0].raw(), "studio-light");
    }

    #[test]
    fn diagnoses_bad_kits() {
        let d = tmpdir("bad");
        std::fs::write(d.join("brand.json"), r##"{"colours":{},"colors":{"primary":"#zz0000"},"fonts":{"display":{"family":"X","src":"nope.woff2"}},"motion":{"eese":"x"}}"##).unwrap();
        let (mut doc, _) = crate::parse::parse("video \"t\"\nbrand \"brand.json\"\n");
        let mut diags = vec![];
        expand(&mut doc, &d, &mut diags);
        let codes: Vec<&str> = diags.iter().map(|d| d.code.as_str()).collect();
        assert!(codes.contains(&"E_BRAND_KEY") && codes.contains(&"E_BRAND_COLOR") && codes.contains(&"E_BRAND_FILE"), "{codes:?}");
        let k = diags.iter().find(|d| d.message.contains("colours")).unwrap();
        assert!(k.hint.contains("colors"), "{}", k.hint);
        let (mut doc, _) = crate::parse::parse("video \"t\"\nbrand \"missing.json\"\n");
        let mut diags = vec![];
        expand(&mut doc, &d, &mut diags);
        assert_eq!(diags[0].code, "E_BRAND");
    }

    #[test]
    fn figma_rest_variables_to_kit() {
        let j: J = serde_json::from_str(r#"{"meta":{"variableCollections":{"c1":{"defaultModeId":"m1"}},"variables":{
            "v1":{"name":"Brand/Primary","resolvedType":"COLOR","variableCollectionId":"c1","valuesByMode":{"m1":{"r":1,"g":0,"b":0,"a":1}}},
            "v2":{"name":"Surface/Background","resolvedType":"COLOR","variableCollectionId":"c1","valuesByMode":{"m1":{"r":0,"g":0,"b":0,"a":1}}},
            "v3":{"name":"Text/Foreground","resolvedType":"COLOR","variableCollectionId":"c1","valuesByMode":{"m1":{"type":"VARIABLE_ALIAS","id":"v4"}}},
            "v4":{"name":"Base/White","resolvedType":"COLOR","variableCollectionId":"c1","valuesByMode":{"m1":{"r":1,"g":1,"b":1,"a":1}}},
            "v5":{"name":"Font/Display family","resolvedType":"STRING","variableCollectionId":"c1","valuesByMode":{"m1":"Acme Sans"}},
            "v6":{"name":"Radius/Card radius","resolvedType":"FLOAT","variableCollectionId":"c1","valuesByMode":{"m1":12}}}}}"#).unwrap();
        let (kit, _) = from_figma(&j);
        assert_eq!(kit["colors"]["primary"], "#ff0000");
        assert_eq!(kit["colors"]["bg"], "#000000");
        assert_eq!(kit["colors"]["fg"], "#ffffff");
        assert_eq!(kit["colors"]["named"]["base-white"], "#ffffff");
        assert_eq!(kit["fonts"]["display"]["family"], "Acme Sans");
        assert_eq!(kit["layout"]["radius"], 12.0);
    }

    #[test]
    fn figma_mcp_map_and_w3c_tokens() {
        let j: J = serde_json::from_str(r##"{"Colors/Primary":"#0A5CFF","Colors/Accent":"#FFB400","Font/Body":"Font(family: \"Inter\", style: Regular, size: 16)"}"##).unwrap();
        let (kit, _) = from_figma(&j);
        assert_eq!(kit["colors"]["primary"], "#0a5cff");
        assert_eq!(kit["colors"]["accent"], "#ffb400");
        assert_eq!(kit["fonts"]["body"]["family"], "Inter");
        let j: J = serde_json::from_str(r##"{"color":{"bg":{"$type":"color","$value":"#111111"},"text":{"$type":"color","$value":"#eeeeee"}}}"##).unwrap();
        let (kit, _) = from_figma(&j);
        assert_eq!(kit["colors"]["bg"], "#111111");
        assert_eq!(kit["colors"]["fg"], "#eeeeee");
    }

    #[test]
    fn template_is_a_valid_kit() {
        let d = tmpdir("tpl");
        std::fs::write(d.join("brand.json"), template("Acme")).unwrap();
        std::fs::write(d.join("logo.svg"), TEMPLATE_LOGO).unwrap();
        let (mut doc, _) = crate::parse::parse("video \"t\"\nbrand \"brand.json\"\n");
        let mut diags = vec![];
        expand(&mut doc, &d, &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
