//! `cloud`: an instanced point cloud (reel-ir `cloud` content). Thousands of points that
//! morph between formations (`form` lines), evaluated per frame by the renderer: one layer,
//! a few KB of IR for any count.
//!
//! ```text
//! cloud #swarm n=20000 seed=7 r=2.4 palette=[paper, #E8442E] weights=[0.64, 0.36] mblur=8
//!   form 0s field flow=[60, -10] noise=40
//!   form 1s text "3000" weight=800 dur=1s stagger=0.6s by=x ease=spring(150,15) color=dir angle=30 split=[0.64, 0.36]
//!   form 3.4s spiral r=40% squash=0.62 spin=25 dur=1.1s stagger=0.4s by=distance dot=0.8
//! ```
//! Pass 1 (`Ctx::cloud`) checks and records the description (times absolute, shape props
//! raw); the reel-ir backend resolves lengths against the laid-out box (`resolve`) and, in a
//! 3D context or with `persp=`, bakes the projection per frame.
use crate::ast::{Node, Value};
use crate::ir::*;
use crate::elements::praw_n;
use crate::lower::*;
use crate::values::*;
use serde_json::{json, Value as J};
use std::collections::BTreeMap;

pub const SHAPES: &[&str] = &["field", "grid", "spiral", "galaxy", "cluster", "gaussian", "sphere", "terrain", "text", "points"];
const FORM_PROPS: &[&str] = &["dur", "stagger", "by", "ease", "assign", "noise", "noise-scale", "noise-speed", "dot", "opacity", "color", "split", "angle", "wave"];
const BY: &[&str] = &["index", "random", "distance", "x", "y", "angle"];
const ASSIGN: &[&str] = &["curve", "angle", "index", "random"];
const COLOR_BY: &[&str] = &["random", "index", "x", "y", "z", "distance", "angle", "dir"];

fn shape_props(shape: &str) -> &'static [&'static str] {
    match shape {
        "field" => &["x", "y", "w", "h", "depth", "flow"],
        "grid" => &["x", "y", "w", "h", "cols", "jitter"],
        "spiral" => &["x", "y", "r", "arms", "twist", "spread", "squash", "spin"],
        "cluster" => &["x", "y", "z", "sx", "sy", "sz"],
        "sphere" => &["x", "y", "z", "r", "spin"],
        "terrain" => &["x", "y", "w", "h", "rows", "amp", "scale", "speed", "z"],
        "text" => &["x", "y", "w", "font", "weight", "size", "tracking", "lh", "align", "outline"],
        "points" => &["points", "src"],
        _ => &[],
    }
}
/// props whose value is a length (px or % of the box)
fn is_len(k: &str) -> bool {
    matches!(k, "x" | "y" | "w" | "h" | "r" | "sx" | "sy" | "sz" | "depth" | "size" | "z" | "amp" | "scale")
}

fn canon_shape(s: &str) -> &str {
    match s {
        "galaxy" => "spiral",
        "gaussian" => "cluster",
        _ => s,
    }
}

fn list_nums(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::List(l) => l.iter().map(|x| match x {
            Value::Num(n) => Some(*n),
            Value::Str(s) => s.trim_end_matches("px").parse().ok(),
            _ => None,
        }).collect(),
        Value::Num(n) => Some(vec![*n]),
        Value::Str(s) => s.split([',', ' ']).filter(|x| !x.is_empty()).map(|x| x.trim_end_matches("px").parse().ok()).collect(),
    }
}

impl<'a> Ctx<'a> {
    /// `cloud` element: the layer box (default: the whole frame, in the scene world) and its forms.
    pub fn cloud(&mut self, parent: usize, si: usize, n: &Node, lid: String) -> usize {
        let count = praw_n(n, "n").or_else(|| praw_n(n, "count")).unwrap_or(5000.0);
        if !(1.0..=4_000_000.0).contains(&count) {
            self.err("E_CLOUD", format!("cloud n={count}: must be 1..4000000"), "write e.g. `n=20000`", n.span);
        } else if count > 300_000.0 {
            self.warn("W_CLOUD_LARGE", format!("cloud n={count}: render time grows with the point count"), "20k-100k points read as a dense swarm at 1080p", n.span);
        }
        let mut palette = vec![];
        match n.props.get("palette") {
            Some(Value::List(l)) => {
                for v in l {
                    let raw = v.raw();
                    // bare theme token names are allowed (`paper`), like `$paper`
                    let tok = format!("${raw}");
                    let c = if parse_color(&raw).is_err() && self.th.colors.contains_key(&raw) { self.color_s(&tok, n.span) } else { self.color_s(&raw, n.span) };
                    if let Some(c) = c {
                        palette.push(c);
                    }
                }
            }
            Some(v) => {
                let raw = v.raw();
                if let Some(c) = self.color_s(&raw, n.span) {
                    palette.push(c);
                }
            }
            None => {}
        }
        if palette.is_empty() {
            palette.push(self.th.colors.get("text").cloned().unwrap_or(crate::theme::hex("#fff")));
            palette.push(self.th.colors.get("accent").cloned().unwrap_or(crate::theme::hex("#17845a")));
        }
        if palette.len() > 255 {
            self.err("E_CLOUD", "cloud palette: at most 255 colors", "use fewer colors", n.span);
            palette.truncate(255);
        }
        let weights = n.props.get("weights").and_then(list_nums).unwrap_or_default();
        if weights.len() > palette.len() || weights.iter().any(|w| *w < 0.0) {
            self.err("E_CLOUD", "cloud weights: one nonnegative weight per palette color", format!("palette has {} colors", palette.len()), n.span);
        }
        let mut cfg = CloudCfg {
            n: count.clamp(1.0, 4_000_000.0) as u32,
            seed: praw_n(n, "seed").unwrap_or(1.0).max(0.0) as u64,
            r: praw_n(n, "r").or_else(|| praw_n(n, "radius")).unwrap_or(2.0),
            rj: praw_n(n, "jitter").unwrap_or(0.0).clamp(0.0, 1.0),
            palette,
            weights,
            persp: praw_n(n, "persp"),
            forms: vec![],
        };
        if cfg.r <= 0.0 {
            self.err("E_CLOUD", "cloud r= must be positive", "point radius in px, e.g. `r=2.5`", n.span);
            cfg.r = 2.0;
        }
        let scene_start = self.tl.scenes[si].start;
        let mut prev_at = f64::MIN;
        for f in n.children.iter().filter(|c| c.kind == "form") {
            if let Some(fc) = self.cloud_form(si, f, scene_start, cfg.palette.len()) {
                if fc.at < prev_at - 1e-9 {
                    self.err("E_CLOUD", format!("form at {:.2}s is earlier than the previous form", fc.at - scene_start), "write the forms in time order", f.span);
                }
                prev_at = fc.at;
                cfg.forms.push(fc);
            }
        }
        if cfg.forms.is_empty() {
            self.err("E_CLOUD", "cloud without forms", "add children like `form 0s field` and `form 1s text \"Hi\" dur=1s`", n.span);
        }
        let mut l = Layer::new(lid, Kind::Cloud(Box::new(cfg)));
        l.tags.push("cloud".into());
        let sized = ["w", "h", "x", "y", "place"].iter().any(|k| n.props.contains_key(*k));
        if sized {
            return self.add(Some(parent), l);
        }
        // default: the whole frame, drawn in the scene world (scene coordinates)
        l.layout.pos = Pos::Fill;
        let world = self.scenes[si].world;
        self.add(Some(world), l)
    }

    fn cloud_form(&mut self, si: usize, f: &Node, scene_start: f64, ncol: usize) -> Option<CloudFormCfg> {
        let mut args = f.args.iter();
        let Some(t) = args.next().map(|v| v.raw()) else {
            self.err("E_CLOUD", "form needs a time and a shape", "write `form 1.2s spiral` (time: scene time or @anchor)", f.span);
            return None;
        };
        let at = match self.tl.anchor_time(si, None, t.trim_start_matches('@'), scene_start, f.span) {
            Ok(v) => v,
            Err(d) => {
                self.diags.push(d);
                return None;
            }
        };
        let shape_raw = args.next().map(|v| v.raw()).unwrap_or_default();
        if !SHAPES.contains(&shape_raw.as_str()) {
            let hint = suggest(&shape_raw, SHAPES).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("shapes: {}", SHAPES.join(", ")));
            self.err("E_CLOUD", format!("unknown cloud shape `{shape_raw}`"), hint, f.span);
            return None;
        }
        let shape = canon_shape(&shape_raw).to_string();
        let rest: Vec<String> = args.map(|v| v.raw()).collect();
        let mut text = None;
        let mut props: BTreeMap<String, String> = BTreeMap::new();
        for a in rest {
            if shape == "text" && text.is_none() && a != "outline" {
                text = Some(a);
            } else if a == "outline" && shape == "text" {
                props.insert("outline".into(), "true".into());
            } else {
                self.warn("W_CLOUD_PROP", format!("form {shape}: stray argument `{a}`"), "props are key=value; a text form takes one quoted text", f.span);
            }
        }
        if shape == "text" && text.as_deref().is_none_or(|s| s.trim().is_empty()) {
            self.err("E_CLOUD", "text form without text", "write `form 2s text \"3000\" weight=800`", f.span);
            return None;
        }
        let sp = shape_props(&shape);
        for (k, v) in &f.props {
            if FORM_PROPS.contains(&k.as_str()) {
                continue;
            }
            if !sp.contains(&k.as_str()) {
                let mut all: Vec<&str> = sp.to_vec();
                all.extend_from_slice(FORM_PROPS);
                let hint = suggest(k, &all).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("{shape} takes: {}", all.join(" ")));
                self.warn("W_CLOUD_PROP", format!("form {shape}: unknown prop `{k}`"), hint, f.span);
                continue;
            }
            let raw = v.raw();
            if is_len(k) && !(shape == "terrain" && k == "z") {
                if let Err(e) = parse_length(&raw) {
                    self.err("E_CLOUD", format!("form {shape} `{k}={raw}`: {}", e.message), e.hint, f.span);
                    continue;
                }
            }
            if matches!(k.as_str(), "flow") && list_nums(v).is_none_or(|l| l.len() != 2) {
                self.err("E_CLOUD", format!("form {shape} `flow={raw}`"), "write `flow=[60, -10]` (px/s)", f.span);
                continue;
            }
            if k == "src" {
                let p = self.base.join(&raw);
                let ok = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<Vec<Vec<f64>>>(&s).ok());
                match ok {
                    Some(pts) if !pts.is_empty() => {
                        props.insert("points".into(), serde_json::to_string(&pts).unwrap());
                    }
                    _ => self.err("E_CLOUD", format!("points src `{raw}`: not a JSON list of [x, y(, z)]"), "paths are relative to the .reel file", f.span),
                }
                continue;
            }
            if k == "points" {
                let pts: Option<Vec<Vec<f64>>> = match v {
                    Value::List(l) => l.iter().map(list_nums).collect(),
                    _ => None,
                };
                match pts {
                    Some(p) if !p.is_empty() && p.iter().all(|q| (2..=3).contains(&q.len())) => {
                        props.insert("points".into(), serde_json::to_string(&p).unwrap());
                    }
                    _ => self.err("E_CLOUD", "points=: a list of [x, y] or [x, y, z]", "write `points=[[100, 200], [300, 400]]` (px in the cloud box)", f.span),
                }
                continue;
            }
            props.insert(k.clone(), if k == "flow" { serde_json::to_string(&list_nums(v).unwrap()).unwrap() } else { raw });
        }
        if shape == "points" && !props.contains_key("points") {
            self.err("E_CLOUD", "points form without points", "write `points=[[x, y], ...]` or `src=points.json`", f.span);
            return None;
        }
        let dur = self.dur(f, "dur").unwrap_or(1.0);
        let stagger = match praw(f, "stagger") {
            Some(s) if s.starts_with('-') => -self.dur_s(&s[1..], f).unwrap_or(0.0),
            Some(s) => self.dur_s(&s, f).unwrap_or(0.0),
            None => 0.0,
        };
        let by = praw(f, "by").unwrap_or("index".into());
        if !BY.contains(&by.as_str()) {
            let hint = suggest(&by, BY).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("by= {}", BY.join("|")));
            self.err("E_CLOUD", format!("unknown stagger order `by={by}`"), hint, f.span);
        }
        let assign = praw(f, "assign").unwrap_or("curve".into());
        if !ASSIGN.contains(&assign.as_str()) {
            self.err("E_CLOUD", format!("unknown `assign={assign}`"), format!("assign= {}", ASSIGN.join("|")), f.span);
        }
        let ease = self.ease(f, "ease", Ease::Cubic(0.65, 0.0, 0.35, 1.0));
        let drift = praw_n(f, "noise").filter(|a| *a != 0.0).map(|a| (a, praw_n(f, "noise-scale").unwrap_or(260.0).max(1.0), praw_n(f, "noise-speed").unwrap_or(0.25)));
        let color = praw(f, "color").map(|by| {
            if !COLOR_BY.contains(&by.as_str()) {
                let hint = suggest(&by, COLOR_BY).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("color= {}", COLOR_BY.join("|")));
                self.err("E_CLOUD", format!("unknown color rule `color={by}`"), hint, f.span);
            }
            let split = f.props.get("split").and_then(list_nums).unwrap_or_default();
            if split.len() > ncol {
                self.err("E_CLOUD", format!("split= has {} parts for {ncol} palette colors", split.len()), "one fraction per palette color", f.span);
            }
            let wave = f.props.get("wave").and_then(list_nums).filter(|w| w.len() == 2).map(|w| (w[0], w[1]));
            CloudColorCfg { by, split, angle: praw_n(f, "angle").unwrap_or(0.0), wave }
        });
        if praw(f, "split").is_some() && color.is_none() {
            self.warn("W_CLOUD_PROP", "split= without color=", "write `color=y split=[0.6, 0.4]` (rule + fractions)", f.span);
        }
        Some(CloudFormCfg {
            at,
            dur: dur.max(0.0),
            stagger,
            by,
            ease,
            assign,
            shape,
            text,
            props,
            drift,
            size: praw_n(f, "dot").unwrap_or(1.0).max(0.0),
            opacity: praw_n(f, "opacity").unwrap_or(1.0).clamp(0.0, 1.0),
            color,
        })
    }

    fn dur_s(&mut self, raw: &str, n: &Node) -> Option<f64> {
        match parse_dur(raw) {
            Ok(d) => Some(d.secs(self.fps)),
            Err(e) => {
                self.err(&e.code, format!("`{raw}`: {}", e.message), e.hint, n.span);
                None
            }
        }
    }
}

fn praw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}

/// Font file for a family name (`assets/fonts/<Family>.ttf`, case-insensitive), default Inter.
pub fn font_file(family: &str) -> String {
    let dir = crate::run::home().join("assets/fonts");
    let want = family.to_ascii_lowercase().replace([' ', '-'], "");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for p in files {
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase().replace([' ', '-'], "")).unwrap_or_default();
            if stem == want {
                return p.to_string_lossy().into();
            }
        }
    }
    dir.join("Inter.ttf").to_string_lossy().into()
}

fn r4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

/// reel-ir `cloud` content for a laid-out box `b` (scene px). Text forms reference fonts by
/// their index in `fonts` (added on first use).
pub fn resolve(cfg: &CloudCfg, b: [f64; 4], fonts: &mut Vec<String>, easing: &dyn Fn(&Ease) -> J) -> J {
    let (bx, by, bw, bh) = (b[0], b[1], b[2].max(1.0), b[3].max(1.0));
    let len = |p: &BTreeMap<String, String>, k: &str, axis: f64, d: f64| -> f64 {
        p.get(k).and_then(|raw| parse_length(raw).ok()).map(|l| match l.unit {
            Unit::Pct => l.v / 100.0 * axis,
            _ => l.v,
        }).unwrap_or(d)
    };
    let num = |p: &BTreeMap<String, String>, k: &str, d: f64| -> f64 { p.get(k).and_then(|raw| raw.trim_end_matches("px").trim_end_matches("deg").parse().ok()).unwrap_or(d) };
    let m = bw.min(bh);
    let forms: Vec<J> = cfg.forms.iter().map(|f| {
        let p = &f.props;
        // sub-rect of the box (x/y = top-left offset, w/h = size)
        let rect = |dx: f64, dy: f64, dw: f64, dh: f64| -> [f64; 4] {
            [r4(bx + len(p, "x", bw, dx)), r4(by + len(p, "y", bh, dy)), r4(len(p, "w", bw, dw)), r4(len(p, "h", bh, dh))]
        };
        let center = |dz: f64| -> [f64; 3] { [r4(bx + len(p, "x", bw, bw / 2.0)), r4(by + len(p, "y", bh, bh / 2.0)), r4(len(p, "z", m, dz))] };
        let shape = match f.shape.as_str() {
            "field" => {
                let flow: Vec<f64> = p.get("flow").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(vec![0.0, 0.0]);
                // a field overscans the box a little, so streaming points enter from outside
                json!({"type": "field", "rect": rect(-0.04 * bw, -0.05 * bh, 1.08 * bw, 1.1 * bh), "depth": r4(len(p, "depth", m, 0.0)), "flow": [flow[0], flow[1]]})
            }
            "grid" => json!({"type": "grid", "rect": rect(0.1 * bw, 0.1 * bh, 0.8 * bw, 0.8 * bh), "cols": num(p, "cols", 0.0) as u32, "jitter": num(p, "jitter", 0.0)}),
            "spiral" => {
                let c = center(0.0);
                json!({"type": "spiral", "center": [c[0], c[1]], "radius": r4(len(p, "r", m, 0.45 * m)), "arms": num(p, "arms", 3.0).max(1.0) as u32,
                    "twist": num(p, "twist", 1.45), "spread": num(p, "spread", 0.1), "squash": num(p, "squash", 1.0), "spin": num(p, "spin", 0.0)})
            }
            "cluster" => json!({"type": "gaussian", "center": center(0.0), "sigma": [r4(len(p, "sx", bw, 0.18 * bw)), r4(len(p, "sy", bh, 0.18 * bh)), r4(len(p, "sz", m, 0.0))]}),
            "sphere" => json!({"type": "sphere", "center": center(0.0), "radius": r4(len(p, "r", m, 0.35 * m)), "spin": num(p, "spin", 0.0)}),
            "terrain" => {
                let deep = p.get("z").is_some_and(|v| matches!(v.as_str(), "on" | "true" | "1"));
                let r = if deep { rect(-0.5 * bw, 0.85 * bh, 2.0 * bw, 4.0 * bh) } else { rect(0.05 * bw, 0.35 * bh, 0.9 * bw, 0.6 * bh) };
                json!({"type": "terrain", "rect": r, "deep": deep, "rows": num(p, "rows", 0.0) as u32, "amp": r4(len(p, "amp", bh, 60.0)),
                    "scale": r4(len(p, "scale", bw, 240.0)), "speed": num(p, "speed", 0.0)})
            }
            "text" => {
                let family = p.get("font").cloned().unwrap_or("Inter".into());
                let file = font_file(&family);
                let fi = match fonts.iter().position(|x| *x == file) {
                    Some(i) => i,
                    None => {
                        fonts.push(file);
                        fonts.len() - 1
                    }
                };
                let text = f.text.clone().unwrap_or_default();
                let lines = text.split('\n').count().max(1) as f64;
                let w = len(p, "w", bw, bw);
                let size = len(p, "size", bh, (0.42 * bh).min(1.6 * w / (text.chars().count().max(1) as f64 * 0.62)));
                let lh = num(p, "lh", 1.0);
                // the line box centered on y (cap/digit ink sits at about half the line box)
                let cy = by + len(p, "y", bh, bh / 2.0);
                let x0 = bx + len(p, "x", bw, (bw - w) / 2.0);
                json!({"type": "text", "text": text, "font": fi, "size": r4(size), "weight": num(p, "weight", 800.0),
                    "letter_spacing": r4(num(p, "tracking", 0.0) * size), "line_height": lh, "pos": [r4(x0), r4(cy - 0.5 * size * lh * lines)],
                    "max_width": r4(w), "align": p.get("align").cloned().unwrap_or("center".into()), "outline": p.contains_key("outline")})
            }
            _ => {
                let pts: Vec<Vec<f64>> = p.get("points").and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
                let pts: Vec<Vec<f64>> = pts.into_iter().map(|q| {
                    let mut v = vec![r4(bx + q[0]), r4(by + q[1])];
                    if let Some(z) = q.get(2) {
                        v.push(*z);
                    }
                    v
                }).collect();
                json!({"type": "points", "points": pts})
            }
        };
        let mut o = json!({"at": r4(f.at), "dur": r4(f.dur), "stagger": r4(f.stagger), "stagger_by": f.by, "easing": easing(&f.ease),
            "assign": f.assign, "shape": shape, "size": f.size, "opacity": f.opacity});
        if let Some((amp, scale, speed)) = f.drift {
            o["drift"] = json!({"amp": amp, "scale": scale, "speed": speed});
        }
        if let Some(c) = &f.color {
            let mut cj = json!({"by": c.by, "split": c.split, "angle": c.angle});
            if let Some((a, per)) = c.wave {
                cj["wave"] = json!([a, per]);
            }
            o["color"] = cj;
        }
        o
    }).collect();
    let pal: Vec<J> = cfg.palette.iter().map(|c| json!([r4(c.r), r4(c.g), r4(c.b), r4(c.a)])).collect();
    json!({"type": "cloud", "count": cfg.n, "seed": cfg.seed, "radius": cfg.r, "radius_jitter": cfg.rj, "palette": pal, "weights": cfg.weights, "forms": forms})
}
