//! Compiler: specialized AST + Timeline -> Motion IR.
//! Pass 1 builds the layer tree per scene (layout + static style) and registers ids.
//! Pass 2 collects every timed event (verbs under `@anchor`, element `in=`/`out=`, transitions,
//! captions) and applies them in global time order, so "later event wins, starting from the
//! current value" holds for every channel.
use crate::ast::{Document, Node, Span, Value};
use crate::diag::Diag;
use crate::ir::*;
use crate::theme::{hex, Theme};
use crate::timing::Timeline;
use crate::values::*;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Named {
    pub idx: usize,
    pub scene: usize,
    pub kind: String,
}

pub struct Browser {
    pub name: String,
    pub scene: usize,
    pub viewport: usize,
    pub cam: usize,
    pub shots: usize,
    pub marks: usize,
    pub cursor: Option<(usize, usize)>,
    pub shot_w: f64,
    pub shot_h: f64,
    pub hot: HashMap<String, [f64; 4]>,
    pub cur_img: Option<usize>,
    pub cur_pos: (f64, f64),
    pub view_w: f64,
    /// visible region of the shot at zoom 1 (shot px): x, y, w, h
    pub crop: [f64; 4],
}

pub struct SceneL {
    pub id: String,
    pub root: usize,
    pub world: usize,
    pub content: usize,
    pub overlay: usize,
    pub start: f64,
    pub dur: f64,
    pub nocap: bool,
}

#[derive(Clone)]
pub struct Ev {
    pub t: f64,
    pub scene: usize,
    pub node: Node,
    pub order: usize,
    /// default target for verbs nested under an element
    pub target: Option<String>,
}

pub struct Ctx<'a> {
    pub th: Theme,
    pub fps: f64,
    pub w: f64,
    pub h: f64,
    pub fmt: String,
    pub tl: &'a Timeline,
    pub base: PathBuf,
    pub diags: Vec<Diag>,
    pub layers: Vec<Layer>,
    pub kids: Vec<Vec<usize>>,
    pub tracks: IndexMap<(String, Ch), Track>,
    pub names: HashMap<String, Named>,
    pub browsers: Vec<Browser>,
    pub scenes: Vec<SceneL>,
    pub hooks: Vec<Hook>,
    pub audio: Vec<AudioClip>,
    pub events: Vec<Ev>,
    pub seq: usize,
    pub captions: String,
    pub cur_scene: usize,
    pub stagger_ix: HashMap<usize, usize>,
    pub presenters: Vec<crate::presenter::Rig>,
    pub particle_cfg: HashMap<String, Node>,
    /// S1: layout geometry from a first native layout pass (zoom #id, move to=#id, split=line)
    pub geo: Option<Geo>,
    pub need_geo: bool,
    /// layer indices whose line boxes the second pass needs (split=line)
    pub want_lines: Vec<usize>,
    /// design layer: size factor of the component being built (scale components/diagram/chart)
    pub k: f64,
    /// time remaps per layer id (`rewind`, `timemap`), baked into the subtree's tracks
    pub remaps: HashMap<String, crate::remap::TimeMap>,
    /// layers drawn under one parent but timed by another (particles: world vs declared parent)
    pub time_parent: HashMap<String, String>,
    /// world canvases (X9) and the camera director events (applied after all other events)
    pub worlds: Vec<crate::world::WorldL>,
    pub director: Vec<Ev>,
}

/// Geometry of a first compile pass, laid out natively (`native.rs`): boxes of named
/// elements ("name@scene", scene px at rest) and the line texts of `split=line` texts.
#[derive(Default, Clone)]
pub struct Geo {
    pub rects: HashMap<String, [f64; 4]>,
    pub lines: HashMap<String, Vec<String>>,
}

// ---------------------------------------------------------------- prop helpers
pub fn id_arg(n: &Node) -> Option<String> {
    n.args.iter().filter_map(|a| a.as_str()).find(|s| s.starts_with('#') && s.len() > 1).map(|s| s[1..].to_string())
}
pub fn text_arg(n: &Node) -> Option<String> {
    n.args.iter().find(|a| match a {
        Value::Str(s) => !s.starts_with('#') || s.len() == 1,
        Value::Num(_) => true,
        _ => false,
    }).map(|v| v.raw())
}
pub fn flag(n: &Node, f: &str) -> bool {
    n.args.iter().any(|a| a.as_str() == Some(f)) || n.props.get(f).map(|v| v.raw() != "false").unwrap_or(false)
}
fn praw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}

impl<'a> Ctx<'a> {
    pub fn err(&mut self, code: &str, msg: impl Into<String>, hint: impl Into<String>, span: Span) {
        self.diags.push(Diag::error(code, msg, hint, span));
    }
    pub fn warn(&mut self, code: &str, msg: impl Into<String>, hint: impl Into<String>, span: Span) {
        self.diags.push(Diag::warn(code, msg, hint, span));
    }
    pub fn num(&mut self, n: &Node, k: &str) -> Option<f64> {
        let raw = praw(n, k)?;
        if let Some(t) = raw.strip_prefix('$') {
            if let Some(v) = self.th.size(t) {
                return Some(v);
            }
        }
        match parse_number(raw.trim_end_matches("px").trim_end_matches("deg")) {
            Ok(v) => Some(v),
            Err(e) => {
                self.err(&e.code, format!("`{k}={raw}`: {}", e.message), e.hint, n.span);
                None
            }
        }
    }
    pub fn len(&mut self, n: &Node, k: &str) -> Option<Len> {
        let raw = praw(n, k)?;
        if let Some(t) = raw.strip_prefix('$') {
            return self.th.size(t).map(Len::px);
        }
        match parse_length(&raw) {
            Ok(l) => Some(match l.unit {
                Unit::Pct => Len::pct(l.v),
                Unit::Em => Len { v: l.v, unit: "em" },
                Unit::Vw => Len::px(l.v * self.w / 100.0),
                Unit::Vh => Len::px(l.v * self.h / 100.0),
                Unit::Px => Len::px(l.v),
            }),
            Err(e) => {
                self.err(&e.code, format!("`{k}={raw}`: {}", e.message), e.hint, n.span);
                None
            }
        }
    }
    pub fn dur(&mut self, n: &Node, k: &str) -> Option<f64> {
        let raw = praw(n, k)?;
        if let Some(t) = raw.strip_prefix('$') {
            return Some(self.th.dur(t));
        }
        match parse_dur(&raw) {
            Ok(d) => Some(d.secs(self.fps)),
            Err(e) => {
                self.err(&e.code, format!("`{k}={raw}`: {}", e.message), e.hint, n.span);
                None
            }
        }
    }
    pub fn color_s(&mut self, raw: &str, span: Span) -> Option<Rgba> {
        match parse_color(raw) {
            Ok(c) => match self.th.color(&c) {
                Some(c) => Some(c),
                None => {
                    let names: Vec<&str> = self.th.colors.keys().map(|s| s.as_str()).collect();
                    let hint = suggest(raw.trim_start_matches('$'), &names).map(|s| format!("did you mean `${s}`?")).unwrap_or_else(|| format!("theme colors: {}", names.join(", ")));
                    self.err("E_TOKEN", format!("unknown color token `{raw}`"), hint, span);
                    None
                }
            },
            Err(e) => {
                self.err(&e.code, format!("`{raw}`: {}", e.message), e.hint, span);
                None
            }
        }
    }
    pub fn color(&mut self, n: &Node, k: &str) -> Option<Rgba> {
        let raw = praw(n, k)?;
        self.color_s(&raw, n.span)
    }
    pub fn paint(&mut self, n: &Node, k: &str) -> Option<Paint> {
        let raw = praw(n, k)?;
        if raw == "$bg" {
            return Some(self.th.bg.clone());
        }
        match parse_paint(&raw) {
            Ok(p) => {
                if let Paint::Solid(ColorRef::Token(t)) = &p {
                    if !self.th.colors.contains_key(t) {
                        self.color_s(&raw, n.span);
                        return None;
                    }
                }
                // image fills resolve against the .reel file (both backends)
                let p = match p {
                    Paint::Image { src, fit } if !src.starts_with("http") && !src.starts_with("data:") && !std::path::Path::new(&src).is_absolute() => {
                        let abs = self.base.join(&src);
                        if !abs.exists() {
                            self.err("E_ASSET", format!("image `{src}` not found"), "paths are relative to the .reel file", n.span);
                        }
                        Paint::Image { src: abs.to_string_lossy().to_string(), fit }
                    }
                    p => p,
                };
                if let Paint::Image { src, .. } = &p {
                    if image_size(Path::new(src)).is_none() {
                        self.err("E_ASSET", format!("unsupported or invalid image `{src}`"), "use a local PNG or JPEG image", n.span);
                    }
                }
                Some(self.th.paint(&p))
            }
            Err(e) => {
                self.err(&e.code, format!("`{k}={raw}`: {}", e.message), e.hint, n.span);
                None
            }
        }
    }
    pub fn ease(&mut self, n: &Node, k: &str, dflt: Ease) -> Ease {
        let Some(raw) = praw(n, k) else { return dflt };
        if let Some(t) = raw.strip_prefix('$') {
            return self.th.ease(t);
        }
        match parse_ease(&raw) {
            Ok(e) => e,
            Err(e) => {
                self.err(&e.code, format!("`{k}={raw}`: {}", e.message), e.hint, n.span);
                dflt
            }
        }
    }

    // ------------------------------------------------------------ arena
    pub fn add(&mut self, parent: Option<usize>, mut l: Layer) -> usize {
        if l.id.is_empty() {
            self.seq += 1;
            l.id = format!("g{}", self.seq);
        }
        self.layers.push(l);
        self.kids.push(vec![]);
        let i = self.layers.len() - 1;
        if let Some(p) = parent {
            self.kids[p].push(i);
        }
        i
    }
    pub fn lid(&self, i: usize) -> String {
        self.layers[i].id.clone()
    }
    pub fn track(&mut self, i: usize, ch: Ch) -> &mut Track {
        let id = self.layers[i].id.clone();
        self.tracks.entry((id.clone(), ch)).or_insert_with(|| Track::new(&id, ch, ch.default_value()))
    }
    pub fn set_base(&mut self, i: usize, ch: Ch, v: TV) {
        let t = self.track(i, ch);
        t.base = v;
    }
    fn build_tree(&self, i: usize) -> Layer {
        let mut l = self.layers[i].clone();
        l.children = self.kids[i].iter().map(|&k| self.build_tree(k)).collect();
        l
    }

    // ------------------------------------------------------------ text runs
    pub fn runs(&self, s: &str) -> Vec<Run> {
        // **bold**, {accent}, `code`
        let mut out = vec![];
        let mut cur = Run::default();
        let (mut bold, mut accent, mut code) = (false, false, false);
        let mut it = s.chars().peekable();
        let flush = |cur: &mut Run, out: &mut Vec<Run>, b: bool, a: bool, c: bool| {
            if !cur.text.is_empty() {
                out.push(std::mem::take(cur));
            }
            cur.bold = b;
            cur.accent = a;
            cur.code = c;
        };
        while let Some(ch) = it.next() {
            match ch {
                '*' if it.peek() == Some(&'*') => {
                    it.next();
                    bold = !bold;
                    flush(&mut cur, &mut out, bold, accent, code);
                }
                '{' => {
                    accent = true;
                    flush(&mut cur, &mut out, bold, accent, code);
                }
                '}' if accent => {
                    accent = false;
                    flush(&mut cur, &mut out, bold, accent, code);
                }
                '`' => {
                    code = !code;
                    flush(&mut cur, &mut out, bold, accent, code);
                }
                _ => cur.text.push(ch),
            }
        }
        if !cur.text.is_empty() {
            out.push(cur);
        }
        out
    }
}

/// Raster formats accepted by direct mode. Inspect content, not the filename.
pub fn image_size(p: &Path) -> Option<(f64, f64)> {
    let reader = image::ImageReader::open(p).ok()?.with_guessed_format().ok()?;
    if !matches!(reader.format(), Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)) {
        return None;
    }
    let (w, h) = reader.into_dimensions().ok()?;
    (w > 0 && h > 0).then_some((w as f64, h as f64))
}

pub fn png_size(p: &Path) -> Option<(f64, f64)> {
    let b = std::fs::read(p).ok()?;
    if b.len() < 24 || &b[1..4] != b"PNG" {
        return None;
    }
    let w = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
    let h = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
    Some((w as f64, h as f64))
}

pub fn load_hotspots(p: &Path) -> Option<HashMap<String, [f64; 4]>> {
    let s = std::fs::read_to_string(p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let mut m = HashMap::new();
    for (k, r) in v.as_object()? {
        let a = r.as_array()?;
        if a.len() == 4 {
            m.insert(k.clone(), [a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?, a[3].as_f64()?]);
        }
    }
    Some(m)
}

// ---------------------------------------------------------------- format specialization
pub fn format_size(base: (u32, u32), fmt: &str) -> (u32, u32) {
    let short = base.0.min(base.1) as f64;
    let (a, b) = parse_aspect(fmt).unwrap_or((base.0, base.1));
    let (a, b) = (a as f64, b as f64);
    let (w, h) = if a >= b { (short * a / b, short) } else { (short, short * b / a) };
    ((w / 2.0).round() as u32 * 2, (h / 2.0).round() as u32 * 2)
}

fn fmt_matches(sel: &str, fmt: &str, w: u32, h: u32) -> bool {
    match sel {
        "portrait" | "vertical" => h > w,
        "landscape" | "wide" => w > h,
        "square" => w == h,
        s => s.split(',').any(|x| x.trim() == fmt),
    }
}

/// Remove `when <format>` blocks that don't apply, inline those that do, apply `set #id k=v`.
pub fn specialize(doc: &Document, fmt: &str, w: u32, h: u32) -> Document {
    let mut sets: Vec<Node> = vec![];
    fn walk(nodes: &[Node], fmt: &str, w: u32, h: u32, sets: &mut Vec<Node>) -> Vec<Node> {
        let mut out = vec![];
        for n in nodes {
            if n.kind == "when" {
                let sel = n.args.iter().map(|a| a.raw()).collect::<Vec<_>>().join(",");
                if fmt_matches(&sel, fmt, w, h) {
                    for c in walk(&n.children, fmt, w, h, sets) {
                        if c.kind == "set" && c.args.iter().any(|a| a.raw().starts_with('#')) && !c.props.is_empty() {
                            sets.push(c);
                        } else {
                            out.push(c);
                        }
                    }
                }
                continue;
            }
            let mut m = n.clone();
            m.children = walk(&n.children, fmt, w, h, sets);
            out.push(m);
        }
        out
    }
    let mut nodes = walk(&doc.nodes, fmt, w, h, &mut sets);
    fn apply(nodes: &mut [Node], set: &Node) {
        let id = id_arg(set).unwrap_or_default();
        for n in nodes.iter_mut() {
            if n.kind != "at" && n.kind != "set" && id_arg(n).as_deref() == Some(id.as_str()) && !is_verb(&n.kind) {
                for (k, v) in &set.props {
                    n.props.insert(k.clone(), v.clone());
                }
            }
            apply(&mut n.children, set);
        }
    }
    for s in &sets {
        apply(&mut nodes, s);
    }
    crate::nest::expand(&mut nodes);
    Document { reel: doc.reel.clone(), nodes }
}

pub const VERBS: &[&str] = &[
    "enter", "exit", "show", "hide", "emph", "pulse", "shake", "glow", "highlight", "spotlight", "callout", "zoom", "pan",
    "cursor", "click", "dblclick", "drag", "type", "swap", "count", "draw", "diff", "burst", "move", "set", "anim", "sfx",
    "gesture", "mood", "dolly", "orbit", "focus", "play", "activate", "deactivate", "done", "undone", "advance", "page", "rewind", "timemap", "drop", "pile", "push", "swing", "shatter",
    "slitscan", "echo", "timedisplace", "freeze", "sweep", "pour", "stir", "ink", "reveal", "animate",
];
pub fn is_verb(k: &str) -> bool {
    VERBS.contains(&k)
}

// ---------------------------------------------------------------- entry
pub struct Compiled {
    pub ir: Ir,
    pub diags: Vec<Diag>,
}

/// Compile; when the source needs layout geometry (zoom/move to laid-out elements, line
/// splits), a first pass is laid out natively and the second pass uses its boxes.
pub fn compile(doc0: &Document, tl: &Timeline, fmt: Option<&str>, base: &Path) -> Compiled {
    // audio reactivity lowers to plain elements + linear keyframe tracks before anything else
    let mut audio_diags = vec![];
    let expanded = crate::audio::expand(doc0, tl, base, &mut audio_diags);
    let doc0 = expanded.as_ref().unwrap_or(doc0);
    let mut c = compile_inner(doc0, tl, fmt, base);
    c.diags.extend(audio_diags);
    c
}

fn compile_inner(doc0: &Document, tl: &Timeline, fmt: Option<&str>, base: &Path) -> Compiled {
    let (c1, need, names, lines) = compile_pass(doc0, tl, fmt, base, None);
    if !need {
        return c1;
    }
    let fonts = crate::run::home().join("assets/fonts");
    let tmp = std::env::temp_dir();
    let mut geo = Geo::default();
    match crate::native::Native::load(&c1.ir, crate::native::snap_at(&c1.ir, 0.0), &fonts, &tmp, false) {
        Ok(nat) => {
            c1.ir.root.walk(&mut |l| { if let Some(r) = nat.rect(&l.id) { geo.rects.insert(l.id.clone(), r); } });
            for (key, lid) in names {
                if let Some(r) = nat.rect(&lid) {
                    geo.rects.insert(key, r);
                }
            }
            for lid in lines {
                let ls: Vec<String> = nat.lines(&lid).into_iter().map(|l| l.text.trim_end().to_string()).filter(|t| !t.is_empty()).collect();
                geo.lines.insert(lid, ls);
            }
        }
        Err(e) => {
            let mut c = c1;
            c.diags.push(Diag::warn("W_LAYOUT_PASS", format!("native layout pass failed: {e:#}"), "zoom/move to elements and split=line fall back", Span::default()));
            return c;
        }
    }
    compile_pass(doc0, tl, fmt, base, Some(geo)).0
}

fn compile_pass(doc0: &Document, tl: &Timeline, fmt: Option<&str>, base: &Path, geo: Option<Geo>) -> (Compiled, bool, Vec<(String, String)>, Vec<String>) {
    let video = doc0.nodes.iter().find(|n| n.kind == "video").cloned().unwrap_or_default();
    let size = praw(&video, "size").and_then(|s| parse_size(&s).ok()).unwrap_or((1920, 1080));
    let fmt = fmt.map(|s| s.to_string()).unwrap_or_else(|| {
        let g = gcd(size.0, size.1);
        format!("{}:{}", size.0 / g, size.1 / g)
    });
    let (w, h) = format_size(size, &fmt);
    let doc = specialize(doc0, &fmt, w, h);
    let mut diags = vec![];
    crate::design::set_doc_base(base);
    let th = Theme::from_doc(&doc, &mut diags);
    let mut c = Ctx {
        th,
        fps: tl.fps,
        w: w as f64,
        h: h as f64,
        fmt: fmt.clone(),
        tl,
        base: base.to_path_buf(),
        diags,
        layers: vec![],
        kids: vec![],
        tracks: IndexMap::new(),
        names: HashMap::new(),
        browsers: vec![],
        scenes: vec![],
        hooks: vec![],
        audio: vec![],
        events: vec![],
        seq: 0,
        captions: praw(&video, "captions").unwrap_or_else(|| "plain".into()),
        cur_scene: 0,
        stagger_ix: HashMap::new(),
        particle_cfg: HashMap::new(),
        presenters: vec![],
        geo,
        need_geo: false,
        want_lines: vec![],
        k: 1.0,
        remaps: HashMap::new(),
        time_parent: HashMap::new(),
        worlds: vec![],
        director: vec![],
    };
    let mut stage = Layer::new("stage", Kind::Group);
    stage.layout.pos = Pos::Flow;
    stage.style.overflow_clip = true;
    stage.style.font = Some(c.th.font.clone());
    stage.style.color = c.th.colors.get("text").cloned();
    let stage = c.add(None, stage);
    let scenes: Vec<&Node> = doc.nodes.iter().filter(|n| n.kind == "scene").collect();
    for (si, sn) in scenes.iter().enumerate() {
        c.cur_scene = si;
        c.build_scene(stage, si, sn);
    }
    // video-level components
    for n in doc.nodes.iter() {
        match n.kind.as_str() {
            "chapterbar" => c.chapterbar(n, &scenes),
            "music" => c.music(n),
            "video" | "scene" | "theme" | "design" | "pronounce" | "when" | "font" => {}
            k => {
                let hint = "top-level nodes: video, theme, pronounce, music, chapterbar, scene";
                c.err("E_TOPLEVEL", format!("`{k}` is not allowed at top level"), hint, n.span);
            }
        }
    }
    c.presenter_ready();
    // pass 2: events
    for (si, sn) in scenes.iter().enumerate() {
        c.collect_events(si, sn);
    }
    c.scene_transitions(&scenes);
    c.caption_pack(&scenes);
    c.captions_all();
    let mut evs = std::mem::take(&mut c.events);
    evs.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal).then(a.order.cmp(&b.order)));
    for e in &evs {
        c.cur_scene = e.scene;
        c.apply(e);
    }
    // camera director (X9): reads the final tracks of its targets (follow), so it runs last
    for e in std::mem::take(&mut c.director) {
        c.cur_scene = e.scene;
        c.camera_director(&e);
    }
    // voice clips
    for s in &tl.scenes {
        for say in &s.says {
            if let Some(mp3) = &say.mp3 {
                c.audio.push(AudioClip { src: mp3.clone(), at: say.audio_at, dur: say.audio_dur, gain: 1.0, duck: None, kind: "voice" });
            }
        }
    }
    let root = c.build_tree(stage);
    let bg = c.th.paint_avg(&c.th.bg).unwrap_or(hex("#000"));
    let names: Vec<(String, String)> = c.names.iter().map(|(k, v)| (k.clone(), c.lid(v.idx))).collect();
    let lines: Vec<String> = c.want_lines.iter().map(|&i| c.lid(i)).collect();
    let need = c.need_geo && c.geo.is_none();
    let precise: std::collections::HashSet<String> = c.layers.iter().filter(|l| l.tags.iter().any(|t| t == "cam-precise")).map(|l| l.id.clone()).collect();
    let mut tracks: Vec<Track> = c.tracks.into_values().filter(|t| !t.keys.is_empty() || t.base != t.ch.default_value()).collect();
    for t in tracks.iter_mut() {
        normalize_track(t, tl.total, precise.contains(&t.layer));
    }
    if !c.remaps.is_empty() {
        let mut parent: HashMap<String, String> = HashMap::new();
        for (p, ks) in c.kids.iter().enumerate() {
            for &k in ks {
                parent.insert(c.layers[k].id.clone(), c.layers[p].id.clone());
            }
        }
        parent.extend(c.time_parent.iter().map(|(k, v)| (k.clone(), v.clone())));
        crate::remap::apply(&mut tracks, &c.remaps, &parent, tl.fps, tl.total);
    }
    let mut fonts = vec![Font { family: c.th.font.clone(), src: "Inter".into(), file: None }];
    fonts.extend(crate::textv2::custom_fonts(&doc, base, &mut c.diags));
    (Compiled {
        ir: Ir { version: "0", format: fmt, w, h, fps: tl.fps, duration: tl.total, bg, fonts, root, tracks, hooks: c.hooks, audio: c.audio, quant: Quant::default() },
        diags: c.diags,
    }, need, names, lines)
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

/// Sort keys, merge near-duplicates, clamp to [0, total], quantize values (determinism rules).
fn normalize_track(t: &mut Track, total: f64, precise: bool) {
    let q = Quant::default();
    let step = match t.ch {
        Ch::Opacity => q.opacity,
        Ch::X | Ch::Y | Ch::Z | Ch::W | Ch::H | Ch::Blur | Ch::Glow => q.px,
        Ch::Rot | Ch::RotX | Ch::RotY | Ch::Hue | Ch::GradAngle => q.angle,
        Ch::Scale | Ch::Sx | Ch::Sy | Ch::Px | Ch::Py => q.scale,
        _ => 1e-4,
    };
    for k in t.keys.iter_mut() {
        k.t = (k.t.clamp(0.0, total) * 1e4).round() / 1e4;
        // director cameras keep full precision (zoom ratios of 1e4+; quantized steps would jitter)
        if precise {
            continue;
        }
        if let TV::N(v) = &mut k.v {
            *v = (*v / step).round() * step;
        }
    }
    t.keys.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    let mut out: Vec<crate::ir::Key> = vec![];
    for k in t.keys.drain(..) {
        if let Some(last) = out.last_mut() {
            if (k.t - last.t).abs() < 5e-4 {
                *last = k;
                continue;
            }
        }
        out.push(k);
    }
    // drop runs of identical values in the middle (no visible change)
    let mut slim: Vec<crate::ir::Key> = vec![];
    for (i, k) in out.iter().enumerate() {
        let prev_same = i > 0 && out[i - 1].v == k.v;
        let next_same = i + 1 < out.len() && out[i + 1].v == k.v;
        if prev_same && next_same {
            continue;
        }
        slim.push(k.clone());
    }
    t.keys = slim;
}
