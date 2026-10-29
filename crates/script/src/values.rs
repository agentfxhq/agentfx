//! Typed Reel Script values and deterministic easing curves.
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValueError { pub code: String, pub message: String, pub hint: String }
fn err(code: &str, message: &str, hint: &str) -> ValueError {
    ValueError { code: code.into(), message: message.into(), hint: hint.into() }
}
fn finite(s: &str) -> Option<f64> { s.trim().parse::<f64>().ok().filter(|v| v.is_finite()) }
fn numeric(s: &str, code: &str, hint: &str) -> Result<f64, ValueError> {
    finite(s).ok_or_else(|| err(code, "Expected a finite number", hint))
}
fn suffixed(s: &str, suffixes: &[&str]) -> Option<(f64, usize)> {
    let s = s.trim().to_lowercase();
    suffixes.iter().enumerate().find_map(|(i, suffix)| {
        s.strip_suffix(suffix).and_then(finite).map(|v| (v, i))
    })
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Dur { Secs(f64), Frames(f64) }
impl Dur { pub fn secs(&self, fps: f64) -> f64 { match self { Self::Secs(v) => *v, Self::Frames(v) => v / fps } } }
pub fn parse_dur(s: &str) -> Result<Dur, ValueError> {
    match suffixed(s, &["ms", "min", "s", "f", ""]) {
        Some((v, 0)) => Ok(Dur::Secs(v / 1000.0)),
        Some((v, 1)) if (v * 60.0).is_finite() => Ok(Dur::Secs(v * 60.0)),
        Some((v, 2 | 4)) => Ok(Dur::Secs(v)), Some((v, 3)) => Ok(Dur::Frames(v)),
        _ => Err(err("E_DUR", "Invalid duration", "write `0.4s` (seconds) or `12f` (frames)")),
    }
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum AnchorBase { Name(String), Time(Dur), SceneStart, SceneEnd, Prev }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Anchor { pub base: AnchorBase, pub end_edge: bool, pub offset: Option<Dur> }
fn name(s: &str) -> bool {
    // music beat grid anchors: `beat:32`, `bar:8` (audio.rs)
    if let Some((k, n)) = s.split_once(':') {
        return matches!(k, "beat" | "bar") && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit());
    }
    !s.is_empty() && !s.chars().next().is_some_and(char::is_numeric)
        && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}
fn anchor_base(s: &str) -> Option<(AnchorBase, bool)> {
    let s = s.trim();
    let lower = s.to_lowercase();
    let (base, end_edge) = if lower.ends_with(".start") { (&s[..s.len()-6], false) }
        else if lower.ends_with(".end") { (&s[..s.len()-4], true) } else { (s, false) };
    let base = base.trim();
    let value = match base.to_lowercase().as_str() {
        "start" => AnchorBase::SceneStart, "end" => AnchorBase::SceneEnd, "prev" => AnchorBase::Prev,
        _ => if let Ok(d) = parse_dur(base) { AnchorBase::Time(d) }
            else if name(base) { AnchorBase::Name(base.into()) } else { return None; },
    };
    Some((value, end_edge))
}
pub fn parse_anchor(s: &str) -> Result<Anchor, ValueError> {
    let s = s.trim().strip_prefix('@').unwrap_or(s.trim()).trim();
    if let Ok(d) = parse_dur(s) { return Ok(Anchor { base: AnchorBase::Time(d), end_edge: false, offset: None }); }
    // A trailing signed duration wins over a hyphen in a name; other hyphens remain literal.
    for (i, c) in s.char_indices().rev() {
        if i == 0 || (c != '+' && c != '-') { continue; }
        if let (Some((base, end_edge)), Ok(mut offset)) = (anchor_base(&s[..i]), parse_dur(&s[i+1..])) {
            if s[i+1..].trim_start().starts_with(['+', '-']) { continue; }
            if c == '-' { match &mut offset { Dur::Secs(v) | Dur::Frames(v) => *v = -*v } }
            return Ok(Anchor { base, end_edge, offset: Some(offset) });
        }
    }
    anchor_base(s).map(|(base, end_edge)| Anchor { base, end_edge, offset: None })
        .ok_or_else(|| err("E_ANCHOR", "Invalid anchor expression", "write `@Reiter.end+0.2s` or `2.5s`"))
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rgba { pub r: f64, pub g: f64, pub b: f64, pub a: f64 }
impl Rgba {
    pub fn to_css(&self) -> String {
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        if self.a >= 1.0 { format!("#{:02x}{:02x}{:02x}", byte(self.r), byte(self.g), byte(self.b)) }
        else { format!("rgb({} {} {} / {})", byte(self.r), byte(self.g), byte(self.b), self.a.clamp(0.0, 1.0)) }
    }
    /// Relative luminance of RGB channels; alpha is not composited.
    pub fn luminance(&self) -> f64 {
        let linear = |v: f64| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }
}
pub fn contrast_ratio(a: &Rgba, b: &Rgba) -> f64 {
    let (a, b) = (a.luminance(), b.luminance()); (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum ColorRef { Lit(Rgba), Token(String) }
fn color_error() -> ValueError {
    err("E_COLOR", "Invalid color", "write `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(255 0 0 / 50%)`, or `$accent`")
}
fn function(s: &str) -> Option<(&str, &str)> {
    let i = s.find('(')?;
    s.strip_suffix(')').map(|s| (s[..i].trim(), &s[i+1..]))
}
fn channel(s: &str, scale: f64) -> Option<f64> {
    let v = if let Some(s) = s.strip_suffix('%') { finite(s)? / 100.0 } else { finite(s)? / scale };
    (0.0..=1.0).contains(&v).then_some(v)
}
pub fn parse_color(s: &str) -> Result<ColorRef, ValueError> {
    let s = s.trim();
    if let Some(token) = s.strip_prefix('$') {
        return if name(token) { Ok(ColorRef::Token(token.into())) } else { Err(color_error()) };
    }
    let lower = s.to_lowercase();
    let rgba = |r, g, b, a| Ok(ColorRef::Lit(Rgba { r, g, b, a }));
    if let Some(hex) = lower.strip_prefix('#') {
        if !hex.is_ascii() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) { return Err(color_error()); }
        let expanded = match hex.len() {
            3 | 4 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 | 8 => hex.into(), _ => return Err(color_error()),
        };
        let byte = |i| u8::from_str_radix(&expanded[i..i+2], 16).map(|v| f64::from(v) / 255.0).map_err(|_| color_error());
        return rgba(byte(0)?, byte(2)?, byte(4)?, if expanded.len() == 8 { byte(6)? } else { 1.0 });
    }
    if let Some((kind, body)) = function(&lower) {
        if !["rgb", "rgba", "hsl", "hsla"].contains(&kind) { return Err(color_error()); }
        let slash: Vec<_> = body.split('/').collect();
        if slash.len() > 2 { return Err(color_error()); }
        let components = |s: &str| -> Option<Vec<String>> {
            if s.contains(',') {
                s.split(',').map(|p| { let p = p.trim(); (!p.is_empty() && p.split_whitespace().count() == 1).then(|| p.to_owned()) }).collect()
            } else { Some(s.split_whitespace().map(str::to_owned).collect()) }
        };
        let mut parts = components(slash[0]).ok_or_else(color_error)?;
        let alpha = if slash.len() == 2 { Some(slash[1].trim().to_owned()) }
            else if parts.len() == 4 { parts.pop() } else { None };
        if parts.len() != 3 { return Err(color_error()); }
        let a = alpha.as_deref().map(|s| channel(s, 1.0).ok_or_else(color_error)).transpose()?.unwrap_or(1.0);
        if kind.starts_with("rgb") {
            return rgba(channel(&parts[0], 255.0).ok_or_else(color_error)?, channel(&parts[1], 255.0).ok_or_else(color_error)?, channel(&parts[2], 255.0).ok_or_else(color_error)?, a);
        }
        let h = parse_angle_deg(&parts[0]).map_err(|_| color_error())?.rem_euclid(360.0) / 60.0;
        if !parts[1].ends_with('%') || !parts[2].ends_with('%') { return Err(color_error()); }
        let sat = channel(&parts[1], 1.0).ok_or_else(color_error)?;
        let l = channel(&parts[2], 1.0).ok_or_else(color_error)?;
        let c = (1.0 - (2.0*l - 1.0).abs()) * sat;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 { 0 => (c,x,0.0), 1 => (x,c,0.0), 2 => (0.0,c,x), 3 => (0.0,x,c), 4 => (x,0.0,c), _ => (c,0.0,x) };
        let m = l - c / 2.0; return rgba(r+m, g+m, b+m, a);
    }
    let hex = match lower.as_str() {
        "white" => "#fff", "black" => "#000", "transparent" => "#0000", "red" => "#f00",
        "green" => "#008000", "blue" => "#00f", "yellow" => "#ff0", "orange" => "#ffa500",
        "purple" => "#800080", "gray" | "grey" => "#808080", _ => return Err(color_error()),
    };
    parse_color(hex)
}
// Delimiters inside parentheses or quoted image paths are not separators.
fn separators(s: &str, delimiter: char) -> Option<Vec<usize>> {
    let (mut depth, mut quote, mut escape) = (0usize, None, false);
    let mut positions = Vec::new();
    for (i, c) in s.char_indices() {
        if escape { escape = false; continue; }
        if let Some(q) = quote { if c == '\\' { escape = true; } else if c == q { quote = None; } continue; }
        match c {
            '\'' | '"' => quote = Some(c), '(' => depth += 1,
            ')' => depth = depth.checked_sub(1)?,
            _ if depth == 0 && c == delimiter => positions.push(i), _ => {},
        }
    }
    (depth == 0 && quote.is_none()).then_some(positions)
}
fn split_top(s: &str, delimiter: char) -> Option<Vec<&str>> {
    let mut start = 0;
    let mut parts = Vec::new();
    for i in separators(s, delimiter)? { parts.push(s[start..i].trim()); start = i + delimiter.len_utf8(); }
    parts.push(s[start..].trim()); Some(parts)
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stop { pub color: ColorRef, pub pos: Option<f64> }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Paint {
    Solid(ColorRef), Linear { angle_deg: f64, stops: Vec<Stop> },
    Radial { cx: f64, cy: f64, stops: Vec<Stop> }, Conic { angle_deg: f64, stops: Vec<Stop> },
    Mesh { cols: u32, rows: u32, colors: Vec<ColorRef> }, Image { src: String, fit: String },
}
fn paint_error() -> ValueError { err("E_PAINT", "Invalid paint", "write `linear(180deg, #fff 0%, #000 100%)` with at least two stops") }
fn stop(s: &str) -> Result<Stop, ValueError> {
    if let Ok(color) = parse_color(s) { return Ok(Stop { color, pos: None }); }
    if let Some(i) = s.rfind(char::is_whitespace) {
        let pos = s[i..].trim().strip_suffix('%').and_then(finite).map(|v| v / 100.0);
        if let Some(pos) = pos { if pos >= 0.0 { return Ok(Stop { color: parse_color(s[..i].trim()).map_err(|_| paint_error())?, pos: Some(pos) }); } }
    }
    Err(paint_error())
}
pub fn parse_paint(s: &str) -> Result<Paint, ValueError> {
    let s = s.trim();
    if let Ok(color) = parse_color(s) { return Ok(Paint::Solid(color)); }
    let (kind, body) = function(s).ok_or_else(paint_error)?;
    let kind = kind.to_lowercase();
    let mut parts = split_top(body, ',').ok_or_else(paint_error)?;
    if kind == "image" {
        if parts.len() != 1 || body.trim().is_empty() { return Err(paint_error()); }
        let body = body.trim();
        let (src, fit) = if let Some(i) = body.rfind(char::is_whitespace) {
            let tail = body[i..].trim();
            if ["cover", "contain", "fill", "none", "scale-down"].contains(&tail.to_lowercase().as_str()) { (body[..i].trim(), tail.to_lowercase()) }
            else { (body, "cover".into()) }
        } else { (body, "cover".into()) };
        let src = if src.starts_with(['\'', '"']) {
            if src.len() < 2 || !src.ends_with(src.chars().next().unwrap_or('"')) { return Err(paint_error()); }
            &src[1..src.len()-1]
        } else { if src.contains(char::is_whitespace) { return Err(paint_error()); } src };
        if src.is_empty() { return Err(paint_error()); }
        return Ok(Paint::Image { src: src.into(), fit });
    }
    if kind == "mesh" {
        let (cols, rows) = parse_size(parts.remove(0)).map_err(|_| paint_error())?;
        if cols.checked_mul(rows).map(u64::from) != Some(parts.len() as u64) { return Err(paint_error()); }
        let colors = parts.into_iter().map(parse_color).collect::<Result<_, _>>().map_err(|_| paint_error())?;
        return Ok(Paint::Mesh { cols, rows, colors });
    }
    if !["linear", "radial", "conic"].contains(&kind.as_str()) {
        let hint = suggest(&kind, &["linear", "radial", "conic", "mesh", "image"])
            .map(|v| format!("did you mean `{v}(...)`?"))
            .unwrap_or_else(|| "write `linear(#fff, #000)` or `image(path.png)`".into());
        return Err(err("E_PAINT", "Unknown paint function", &hint));
    }
    let (mut angle, mut cx, mut cy) = (if kind == "conic" { 0.0 } else { 180.0 }, 0.5, 0.5);
    if kind != "radial" {
        if let Ok(v) = parse_angle_deg(parts[0]) { angle = v; parts.remove(0); }
    } else if parts[0].split_whitespace().next().is_some_and(|s| s.eq_ignore_ascii_case("at")) {
        let coords: Vec<_> = parts.remove(0).split_whitespace().collect();
        if coords.len() != 3 { return Err(paint_error()); }
        cx = coords[1].strip_suffix('%').and_then(|s| channel(&format!("{s}%"), 1.0)).ok_or_else(paint_error)?;
        cy = coords[2].strip_suffix('%').and_then(|s| channel(&format!("{s}%"), 1.0)).ok_or_else(paint_error)?;
    }
    if parts.len() < 2 { return Err(paint_error()); }
    let stops = parts.into_iter().map(stop).collect::<Result<_, _>>()?;
    Ok(match kind.as_str() { "linear" => Paint::Linear { angle_deg: angle, stops }, "conic" => Paint::Conic { angle_deg: angle, stops }, _ => Paint::Radial { cx, cy, stops } })
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Ease {
    Linear, Hold, Cubic(f64, f64, f64, f64), Steps(u32),
    Spring { stiffness: f64, damping: f64, mass: f64, velocity: f64 }, Bounce { bounces: u32 },
}
const PRESETS: &[(&str, [f64; 4])] = &[
    ("ease", [0.25,0.1,0.25,1.0]), ("in", [0.42,0.0,1.0,1.0]), ("out", [0.0,0.0,0.58,1.0]), ("inout", [0.42,0.0,0.58,1.0]),
    ("in-sine", [0.12,0.0,0.39,0.0]), ("out-sine", [0.61,1.0,0.88,1.0]), ("inout-sine", [0.37,0.0,0.63,1.0]),
    ("in-quad", [0.11,0.0,0.5,0.0]), ("out-quad", [0.5,1.0,0.89,1.0]), ("inout-quad", [0.45,0.0,0.55,1.0]),
    ("in-cubic", [0.32,0.0,0.67,0.0]), ("out-cubic", [0.33,1.0,0.68,1.0]), ("inout-cubic", [0.65,0.0,0.35,1.0]),
    ("in-quart", [0.5,0.0,0.75,0.0]), ("out-quart", [0.25,1.0,0.5,1.0]), ("inout-quart", [0.76,0.0,0.24,1.0]),
    ("in-expo", [0.7,0.0,0.84,0.0]), ("out-expo", [0.16,1.0,0.3,1.0]), ("inout-expo", [0.87,0.0,0.13,1.0]),
    ("in-back", [0.36,0.0,0.66,-0.56]), ("out-back", [0.34,1.56,0.64,1.0]), ("inout-back", [0.68,-0.6,0.32,1.6]),
];
fn canonical(s: &str) -> String { s.to_lowercase().chars().filter(|c| *c != '-' && *c != '_').collect() }
fn ease_error() -> ValueError { err("E_EASE", "Invalid easing parameters", "write `cubic(0.33, 1, 0.68, 1)`, `steps(4)`, or `spring(170, 26, 1, 0)`; spring stiffness, damping and mass must be positive") }
pub fn parse_ease(s: &str) -> Result<Ease, ValueError> {
    let s = s.trim();
    let key = canonical(s);
    for (label, [a,b,c,d]) in PRESETS { if canonical(label) == key { return Ok(Ease::Cubic(*a,*b,*c,*d)); } }
    let spring = |k, c| Ease::Spring { stiffness: k, damping: c, mass: 1.0, velocity: 0.0 };
    match key.as_str() {
        "linear" => return Ok(Ease::Linear), "hold" => return Ok(Ease::Hold), "step" => return Ok(Ease::Steps(1)),
        "spring" => return Ok(spring(170.0,26.0)), "snappy" => return Ok(spring(300.0,30.0)),
        "soft" => return Ok(spring(120.0,20.0)), "bouncy" => return Ok(spring(180.0,12.0)),
        "bounce" => return Ok(Ease::Bounce { bounces: 3 }), _ => {},
    }
    let (kind, body) = function(s).unwrap_or((s, ""));
    let kind = canonical(kind);
    if ["cubic", "steps", "spring", "bounce"].contains(&kind.as_str()) {
        let args = split_top(body, ',').ok_or_else(ease_error)?;
        if kind == "steps" || kind == "bounce" {
            if args.len() != 1 { return Err(ease_error()); }
            let n = args[0].parse::<u32>().ok().filter(|v| *v > 0).ok_or_else(ease_error)?;
            return Ok(if kind == "steps" { Ease::Steps(n) } else { Ease::Bounce { bounces: n } });
        }
        let v = args.into_iter().map(|s| finite(s).ok_or_else(ease_error)).collect::<Result<Vec<_>, _>>()?;
        if kind == "cubic" && v.len() == 4 && (0.0..=1.0).contains(&v[0]) && (0.0..=1.0).contains(&v[2]) {
            return Ok(Ease::Cubic(v[0],v[1],v[2],v[3]));
        }
        if kind == "spring" && (2..=4).contains(&v.len()) && v[0] > 0.0 && v[1] > 0.0 {
            let mass = v.get(2).copied().unwrap_or(1.0);
            if mass > 0.0 { return Ok(Ease::Spring { stiffness: v[0], damping: v[1], mass, velocity: v.get(3).copied().unwrap_or(0.0) }); }
        }
        return Err(ease_error());
    }
    let mut candidates: Vec<_> = PRESETS.iter().map(|(label, _)| *label).collect();
    candidates.extend(["linear", "hold", "step", "steps", "spring", "snappy", "soft", "bouncy", "bounce", "cubic"]);
    let hint = suggest(&kind, &candidates).map(|v| format!("did you mean `{v}`?"))
        .unwrap_or_else(|| "write `linear`, `out-cubic`, or `spring(170, 26)`".into());
    Err(err("E_EASE", "Unknown easing", &hint))
}
fn bezier(u: f64, a: f64, b: f64) -> f64 { 3.0*(1.0-u).powi(2)*u*a + 3.0*(1.0-u)*u*u*b + u.powi(3) }
fn cubic_sample(t: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    let (mut u, mut lo, mut hi) = (t, 0.0, 1.0);
    for _ in 0..40 {
        let delta = bezier(u,a,c) - t;
        if delta.abs() < 1e-12 { break; }
        if delta < 0.0 { lo = u; } else { hi = u; }
        let slope = 3.0*(1.0-u).powi(2)*a + 6.0*(1.0-u)*u*(c-a) + 3.0*u*u*(1.0-c);
        let next = u - delta / slope;
        u = if slope.abs() > 1e-10 && next > lo && next < hi { next } else { (lo+hi)/2.0 };
    }
    bezier(u,b,d)
}
/// Semi-implicit Euler at 1200 Hz. The energy bound guarantees both settling
/// tolerances thereafter for the physical system. Invalid, numerically unstable,
/// or stalled inputs return infinite duration (and the finite samples obtained).
pub fn spring_curve(k: f64, c: f64, m: f64, v0: f64) -> (f64, Vec<(f64, f64)>) {
    const DT: f64 = 1.0 / 1200.0;
    let (mut x, mut v) = (0.0, v0);
    let mut trace = vec![0.0];
    let mut settled = false;
    if [k,c,m,v0].iter().all(|v| v.is_finite()) && k > 0.0 && c > 0.0 && m > 0.0 {
        loop {
            let previous = (x, v);
            v += (-k*(x-1.0) - c*v) / m * DT;
            x += v * DT;
            if !x.is_finite() || !v.is_finite() { break; }
            trace.push(x);
            let energy = k*(x-1.0).powi(2) + m*v*v;
            if energy / k < 1e-6 && energy / m < 1e-4 { settled = true; break; }
            if (x, v) == previous { break; }
        }
    }
    let duration = if settled { (trace.len()-1) as f64 * DT } else { f64::INFINITY };
    let points = (0..=120).map(|i| {
        let t = f64::from(i) / 120.0;
        let index = t * (trace.len()-1) as f64;
        let lo = index.floor() as usize;
        let hi = (lo+1).min(trace.len()-1);
        (t, trace[lo] + (trace[hi]-trace[lo])*(index-lo as f64))
    }).collect();
    (duration, points)
}
impl Ease {
    pub fn sample(&self, t: f64) -> f64 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0,1.0) };
        if t == 0.0 || t == 1.0 { return t; }
        match self {
            Self::Linear => t, Self::Hold => 0.0, Self::Steps(n) => if *n == 0 { 0.0 } else { (t*f64::from(*n)).floor()/f64::from(*n) },
            Self::Cubic(a,b,c,d) => cubic_sample(t,*a,*b,*c,*d),
            // reel's closed form (the renderer's own easing), so compiler-sampled frames, baked
            // time remaps and the renderer's symbolic spring tracks agree exactly
            Self::Spring { stiffness, damping, mass, velocity } if *stiffness > 0.0 && *damping > 0.0 && *mass > 0.0 && [stiffness, damping, mass, velocity].iter().all(|v| v.is_finite()) => {
                agentfx_ir::timing::ease(&agentfx_ir::Easing::Spring { mass: *mass, stiffness: *stiffness, damping: *damping, velocity: *velocity }, t, false)
            }
            Self::Spring { stiffness, damping, mass, velocity } => {
                let (_, curve) = spring_curve(*stiffness,*damping,*mass,*velocity);
                let index = t*(curve.len()-1) as f64;
                let lo = index.floor() as usize;
                curve[lo].1 + (curve[lo+1].1-curve[lo].1)*(index-lo as f64)
            }
            Self::Bounce { bounces } => {
                // First fall, then n successively smaller parabolic rebounds.
                let n = f64::from(*bounces);
                let fall = 1.0 / (1.0 + n.sqrt());
                if t <= fall { (t/fall).powi(2) } else {
                    let phase = (t-fall)/(1.0-fall)*n;
                    let u = phase.fract();
                    1.0 - 0.5_f64.powf(phase.floor()+1.0)*4.0*u*(1.0-u)
                }
            }
        }
    }
    pub fn natural_duration(&self) -> Option<f64> {
        if let Self::Spring { stiffness, damping, mass, velocity } = self {
            let duration = spring_curve(*stiffness,*damping,*mass,*velocity).0;
            duration.is_finite().then_some(duration)
        } else { None }
    }
    pub fn is_physical(&self) -> bool { matches!(self, Self::Spring { .. }) }
}
pub fn ease_to_css(e: &Ease) -> String {
    match e {
        Ease::Linear => "linear".into(), Ease::Hold => "step-end".into(),
        Ease::Steps(n) => format!("steps({n}, end)"), Ease::Cubic(a,b,c,d) => format!("cubic-bezier({a}, {b}, {c}, {d})"),
        _ => {
            let mut points = vec!["0".to_owned()];
            for i in 1..48 { let t = f64::from(i)/48.0; points.push(format!("{:.4} {:.4}%", e.sample(t), t*100.0)); }
            points.push("1".into()); format!("linear({})", points.join(", "))
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Unit { Px, Pct, Vw, Vh, Em }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Length { pub v: f64, pub unit: Unit }
pub fn parse_length(s: &str) -> Result<Length, ValueError> {
    let (v, unit) = suffixed(s, &["px", "%", "vw", "vh", "em", ""])
        .ok_or_else(|| err("E_LENGTH", "Invalid length", "write `120px`, `50%`, `10vw`, `10vh`, or `2em`"))?;
    Ok(Length { v, unit: match unit { 1 => Unit::Pct, 2 => Unit::Vw, 3 => Unit::Vh, 4 => Unit::Em, _ => Unit::Px } })
}
pub fn parse_angle_deg(s: &str) -> Result<f64, ValueError> {
    let error = || err("E_ANGLE", "Invalid angle", "write `30deg`, `0.25turn`, or `1rad`");
    let (v, unit) = suffixed(s, &["deg", "turn", "rad", ""]).ok_or_else(error)?;
    let v = match unit { 1 => v*360.0, 2 => v.to_degrees(), _ => v };
    if v.is_finite() { Ok(v) } else { Err(error()) }
}
pub fn parse_number(s: &str) -> Result<f64, ValueError> {
    suffixed(s, &["%", "x", ""]).map(|(v,u)| if u == 0 { v/100.0 } else { v })
        .ok_or_else(|| err("E_NUMBER", "Invalid number", "write `1.9`, `1.9x`, or `50%`"))
}
fn pair(s: &str, separators: &[char], code: &str, hint: &str) -> Result<(u32,u32), ValueError> {
    let parts: Vec<_> = s.split(separators).collect();
    let positive = |s: &str| s.trim().parse::<u32>().ok().filter(|v| *v > 0);
    if parts.len() == 2 { if let (Some(a),Some(b)) = (positive(parts[0]),positive(parts[1])) { return Ok((a,b)); } }
    Err(err(code, "Expected two positive integers", hint))
}
pub fn parse_size(s: &str) -> Result<(u32,u32), ValueError> { pair(s, &['x','X'], "E_SIZE", "write `1920x1080`") }
pub fn parse_aspect(s: &str) -> Result<(u32,u32), ValueError> { pair(s, &[':', 'x','X'], "E_ASPECT", "write `16:9` or `9:16`") }
pub fn parse_rate_pct(s: &str) -> Result<f64, ValueError> {
    let hint = "write `+6%` or `-10%`";
    numeric(s.trim().strip_suffix('%').ok_or_else(|| err("E_RATE", "Expected a percentage rate", hint))?, "E_RATE", hint)
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum KeyTime { At(Anchor), Dur(Dur), Pct(f64) }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Key { pub t: KeyTime, pub v: String, pub ease: Option<Ease> }
pub fn parse_key(s: &str) -> Result<Key, ValueError> {
    let error = || err("E_KEY", "Invalid keyframe", "write `0s:0.85~out-cubic` or `@Reiter+0.2s:1~spring(170, 26)`");
    let colons = separators(s, ':').ok_or_else(error)?;
    let i = *colons.first().ok_or_else(error)?;
    let (time, rest) = (s[..i].trim(), s[i+1..].trim());
    let t = if time.starts_with('@') { KeyTime::At(parse_anchor(time)?) }
        else if let Some(pct) = time.strip_suffix('%') { KeyTime::Pct(numeric(pct, "E_KEY", "write `50%:1` for a span-relative keyframe")? / 100.0) }
        else { KeyTime::Dur(parse_dur(time)?) };
    let tildes = separators(rest, '~').ok_or_else(error)?;
    let (v, ease) = if let Some(i) = tildes.last() { (rest[..*i].trim(), Some(parse_ease(&rest[*i+1..])?)) } else { (rest, None) };
    if v.is_empty() { return Err(error()); }
    Ok(Key { t, v: v.into(), ease })
}
/// Case-insensitive Damerau-Levenshtein suggestions; qualifying prefixes win.
pub fn suggest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let word = word.to_lowercase();
    let a: Vec<_> = word.chars().collect();
    if a.is_empty() { return None; }
    let limit = 2.max(a.len()/3);
    let mut best = None;
    for &candidate in candidates {
        let lower = candidate.to_lowercase();
        let b: Vec<_> = lower.chars().collect();
        let max = a.len() + b.len();
        let mut d = vec![vec![0usize; b.len()+2]; a.len()+2];
        d[0][0] = max;
        for i in 0..=a.len() { d[i+1][0] = max; d[i+1][1] = i; }
        for j in 0..=b.len() { d[0][j+1] = max; d[1][j+1] = j; }
        let mut seen = std::collections::HashMap::<char, usize>::new();
        for i in 1..=a.len() {
            let mut last_match = 0;
            for j in 1..=b.len() {
                let i1 = seen.get(&b[j-1]).copied().unwrap_or(0);
                let j1 = last_match;
                let cost = usize::from(a[i-1] != b[j-1]);
                if cost == 0 { last_match = j; }
                d[i+1][j+1] = (d[i][j]+cost).min(d[i+1][j]+1).min(d[i][j+1]+1)
                    .min(d[i1][j1] + (i-i1-1) + 1 + (j-j1-1));
            }
            seen.insert(a[i-1], i);
        }
        let distance = d[a.len()+1][b.len()+1];
        let score = (!lower.starts_with(&word), distance);
        if distance <= limit && best.as_ref().is_none_or(|(old, _)| score < *old) { best = Some((score,candidate)); }
    }
    best.map(|(_,candidate)| candidate)
}
pub fn as_raw(v: &crate::ast::Value) -> String { v.raw() }
