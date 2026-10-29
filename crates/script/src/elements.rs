//! Pass 1: scenes and elements -> layers (layout constraints + static style).
use crate::ast::{Node, Value};
use crate::ir::*;
use crate::lower::*;
use crate::theme::hex;
use crate::values::*;

pub const ELEMENTS: &[&str] = &[
    "text", "title", "display", "h1", "h2", "h3", "body", "small", "label", "stack", "row", "col", "grid", "box", "card",
    "spacer", "image", "video", "svg", "html", "icon", "browser", "phone", "terminal", "code", "chart", "counter",
    "particles", "layer", "group", "rect", "ellipse", "circle", "path", "line", "poly", "star", "lower3", "toast",
    "chapter", "link", "diagram", "node", "camera", "lottie", "shader", "textpath", "stat", "note", "steps", "pill", "quote",
    "list", "window", "device", "progress", "panel", "cloud", "presenter", "light", "world", "place", "fluid", "nest",
];

const ICONS: &[(&str, &str)] = &[
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
];

const KEYWORDS: &[&str] = &[
    "const", "let", "var", "function", "return", "if", "else", "for", "while", "import", "export", "from", "async", "await", "new",
    "class", "fn", "pub", "use", "mut", "impl", "struct", "enum", "match", "def", "self", "None", "True", "False", "null",
    "true", "false", "type", "interface", "extends", "in", "of", "try", "catch", "throw", "static", "package", "func", "go",
    "select", "where", "and", "or", "not", "SELECT", "FROM", "WHERE", "JOIN", "ON", "GROUP", "BY", "ORDER",
];

impl<'a> Ctx<'a> {
    pub fn build_scene(&mut self, stage: usize, si: usize, sn: &Node) {
        let st = &self.tl.scenes[si];
        let id = st.id.clone();
        let mut root = Layer::new(format!("scene-{id}"), Kind::Group);
        root.layout.pos = Pos::Fill;
        root.style.overflow_clip = true;
        root.style.fill = Some(self.paint(sn, "bg").unwrap_or_else(|| self.th.bg.clone()));
        root.tags.push("scene".into());
        root.tags.push(format!("start:{:.4}", st.start));
        let root = self.add(Some(stage), root);
        let mut world = Layer::new(format!("world-{id}"), Kind::Group);
        world.layout.pos = Pos::Fill;
        let world = self.add(Some(root), world);
        let (sx, sy) = (self.th.safe.0 * self.w, self.th.safe.1 * self.h);
        let mut content = Layer::new(format!("content-{id}"), Kind::Group);
        content.layout.pos = Pos::Fill;
        content.layout.flow = "col";
        content.layout.gap = praw_n(sn, "gap").unwrap_or(36.0);
        content.layout.pad = [sy, sx, sy + self.caption_reserve(sn), sx];
        content.layout.align = sn.props.get("align").map(|v| v.raw()).unwrap_or("center".into());
        content.layout.justify = sn.props.get("justify").map(|v| v.raw()).unwrap_or("center".into());
        let content = self.add(Some(world), content);
        let mut overlay = Layer::new(format!("overlay-{id}"), Kind::Group);
        overlay.layout.pos = Pos::Fill;
        let overlay = self.add(Some(root), overlay);
        self.scenes.push(SceneL { id: id.clone(), root, world, content, overlay, start: st.start, dur: st.dur, nocap: flag(sn, "nocaptions") });
        // camera (2.5D)
        if let Some(cam) = sn.children.iter().find(|n| n.kind == "camera" && crate::world::director_verb(n).is_none()) {
            let persp = self.num(cam, "persp").unwrap_or(1200.0);
            self.layers[root].style.perspective = Some(persp);
            self.layers[world].style.preserve3d = true;
            self.layers[content].style.preserve3d = true;
            self.layers[world].name = Some("camera".into());
            self.names.insert(format!("camera@{si}"), Named { idx: world, scene: si, kind: "camera".into() });
            for (k, ch, sign) in [("x", Ch::X, -1.0), ("y", Ch::Y, -1.0), ("z", Ch::Z, 1.0), ("rotx", Ch::RotX, -1.0), ("roty", Ch::RotY, -1.0), ("zoom", Ch::Scale, 1.0)] {
                if let Some(v) = self.num(cam, k) {
                    self.set_base(world, ch, TV::N(v * sign));
                }
            }
        }
        let first = self.layers.len();
        self.looks(root, sn);
        for n in &sn.children {
            self.element(content, si, n, false);
        }
        self.check_lights(si, first.min(root));
    }

    fn caption_reserve(&self, sn: &Node) -> f64 {
        // caption pack: its own band (bottom placement) or none (center/top float over content)
        if let Some(cn) = sn.children.iter().find(|n| n.kind == "captions") {
            let accent = self.th.colors.get("accent").cloned().unwrap_or(crate::theme::hex("#17845a"));
            return crate::captions::reserve(cn, self.w, self.h, accent).unwrap_or(0.0);
        }
        let has_say = sn.children.iter().any(|n| n.kind == "say");
        if has_say && self.captions != "off" && !flag(sn, "nocaptions") {
            self.th.size("caption").unwrap_or(33.0) * 2.0 * 1.3 + 24.0
        } else {
            0.0
        }
    }

    /// Build one element (and its children). Non-element kinds (say, at, anim, ...) are skipped here.
    pub fn element(&mut self, parent: usize, si: usize, n: &Node, in_abs: bool) -> Option<usize> {
        let k = n.kind.as_str();
        if matches!(k, "say" | "at" | "pause" | "camera" | "set" | "when" | "mark" | "captions") || is_verb(k) {
            return None;
        }
        if k == "form" {
            self.err("E_CLOUD", "`form` outside a cloud", "indent it under `cloud #id n=...`", n.span);
            return None;
        }
        if !ELEMENTS.contains(&k) {
            let mut all: Vec<&str> = ELEMENTS.to_vec();
            all.extend_from_slice(VERBS);
            all.extend_from_slice(&["say", "pause", "at", "camera"]);
            let hint = suggest(k, &all).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| "see docs/SPEC.md catalog for element kinds".into());
            self.err("E_KIND", format!("unknown element kind `{k}`"), hint, n.span);
            return None;
        }
        let user = id_arg(n);
        let lid = match &user {
            Some(u) => {
                let lid = format!("u-{u}-{si}");
                if self.names.contains_key(u) && self.names[u].scene == si {
                    self.err("E_DUP_ID", format!("id `#{u}` is used twice in scene `{}`", self.scenes[si].id), "give each element a unique id within its scene", n.span);
                }
                lid
            }
            None => String::new(),
        };
        let parent = if flag(n, "hud") { self.scenes[si].overlay } else { parent };
        let comp = crate::components::COMPONENTS.contains(&k);
        let idx = if comp { self.component(parent, si, n, lid.clone()).unwrap() } else { match k {
            "text" | "display" | "h1" | "h2" | "h3" | "body" | "small" | "label" => self.text_el(parent, n, lid),
            "title" => self.title_el(parent, si, n, lid),
            "stack" | "row" | "col" | "grid" | "box" | "card" | "diagram" | "node" | "layer" | "group" | "spacer" => self.container(parent, si, n, lid),
            "image" | "video" => {
                let src = praw(n, "src").or_else(|| text_arg(n)).unwrap_or_default();
                let src = self.asset(&src, n);
                if k == "image" && image_size(std::path::Path::new(&src)).is_none() {
                    self.err("E_ASSET", format!("unsupported or invalid image `{src}`"), "use a local PNG or JPEG image", n.span);
                }
                let kind = if k == "image" { Kind::Image { src, fit: praw(n, "fit").unwrap_or("cover".into()) } } else { Kind::Video { src } };
                self.add(Some(parent), Layer::new(lid, kind))
            }
            "svg" | "html" => {
                let markup = match (&n.body, praw(n, "src")) {
                    (Some(b), _) => b.clone(),
                    (None, Some(src)) => std::fs::read_to_string(self.base.join(&src)).unwrap_or_else(|_| {
                        self.err("E_ASSET", format!("cannot read `{src}`"), "paths are relative to the .reel file", n.span);
                        String::new()
                    }),
                    _ => String::new(),
                };
                // relative src= in raw markup resolve against the .reel file (both backends)
                let markup = absolutize_src(&markup, &self.base);
                let kind = if k == "svg" { Kind::Svg { markup } } else { Kind::Html { markup } };
                self.add(Some(parent), Layer::new(lid, kind))
            }
            "icon" => {
                let name = praw(n, "name").or_else(|| text_arg(n)).unwrap_or_default();
                let body = match ICONS.iter().find(|(k, _)| *k == name) {
                    Some((_, b)) => *b,
                    None => {
                        let names: Vec<&str> = ICONS.iter().map(|(k, _)| *k).collect();
                        let hint = suggest(&name, &names).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("icons: {}", names.join(", ")));
                        self.err("E_ICON", format!("unknown icon `{name}`"), hint, n.span);
                        ""
                    }
                };
                let col = self.color(n, "color").unwrap_or(hex("#fff"));
                let sw = self.num(n, "sw").unwrap_or(1.8);
                let markup = format!("<svg viewBox='0 0 24 24' width='100%' height='100%' fill='none' stroke='{}' stroke-width='{sw}' stroke-linecap='round' stroke-linejoin='round'>{body}</svg>", col.to_css());
                let mut l = Layer::new(lid, Kind::Svg { markup });
                let s = self.num(n, "size").unwrap_or(48.0);
                l.layout.w = Some(Len::px(s));
                l.layout.h = Some(Len::px(s));
                self.add(Some(parent), l)
            }
            "browser" | "phone" => self.browser(parent, si, n, lid),
            "terminal" => self.terminal(parent, n, lid),
            "code" => self.code(parent, n, lid),
            "chart" => self.chart(parent, si, n, lid),
            "counter" => {
                let mut l = Layer::new(lid, Kind::Counter { prefix: praw(n, "prefix").unwrap_or_default(), suffix: praw(n, "suffix").unwrap_or_default() });
                if let Some(sp) = praw(n, "sep") {
                    l.tags.push(format!("sep:{sp}"));
                }
                l.tags.push("text".into());
                self.text_style(&mut l, n, "h1");
                let i = self.add(Some(parent), l);
                let from = self.num(n, "from").unwrap_or(0.0);
                self.set_base(i, Ch::Count, TV::N(from));
                i
            }
            "presenter" => self.presenter(parent, si, n, lid),
            "world" => self.world_el(si, n, lid),
            "place" => self.place_el(parent, n, lid),
            "nest" => self.nest_el(parent, n, lid),
            "cloud" => self.cloud(parent, si, n, lid),
            "fluid" => self.fluid(parent, si, n, lid),
            "light" => self.light_el(si, n, lid),
            "particles" => {
                let mut l = Layer::new(lid, Kind::Group);
                l.layout.pos = Pos::Fill;
                l.tags.push("particles".into());
                let world = self.scenes[si].world;
                let pi = self.add(Some(world), l);
                let key = self.lid(pi);
                self.particle_cfg.insert(key.clone(), n.clone());
                // drawn in the world (scene coordinates), but timed with the declared parent:
                // a time remap (`rewind`/`timemap`) of the parent also remaps the burst
                if parent != world {
                    let pid = self.lid(parent);
                    self.time_parent.insert(key, pid);
                }
                pi
            }
            "rect" | "ellipse" | "circle" | "path" | "line" | "poly" | "star" => self.shape(parent, n, lid),
            "lower3" => self.lower3(si, n, lid),
            "toast" => {
                let mut l = Layer::new(lid, Kind::Text { runs: self.runs(&text_arg(n).unwrap_or_default()) });
                l.tags.push("text".into());
                l.style.fill = Some(Paint::Solid(ColorRef::Lit(hex("#15181d"))));
                l.style.color = Some(hex("#fff"));
                l.style.size = Some(self.num(n, "size").unwrap_or(16.0));
                l.style.radius = 10.0;
                l.layout.pad = [10.0, 16.0, 10.0, 16.0];
                l.style.shadows.push(Shadow { x: 0.0, y: 12.0, blur: 32.0, color: Some(hex("#00000059")), ..Default::default() });
                l.layout.pos = Pos::Abs;
                l.layout.x = Some(Len::pct(50.0));
                l.layout.y = Some(Len::pct(94.0));
                l.layout.anchor = (0.5, 1.0);
                l.style.nowrap = true;
                self.add(Some(parent), l)
            }
            "chapter" => self.chapter_card(si, n, lid),
            "link" => self.link(si, n, lid),
            "lottie" | "shader" | "textpath" => {
                self.warn("W_PLANNED", format!("`{k}` is planned (spec v0) and not rendered in S0"), "remove it or replace with svg/video for now", n.span);
                return None;
            }
            _ => return None,
        } };
        if let Some(u) = &user {
            self.layers[idx].name = Some(u.clone());
            self.names.insert(u.clone(), Named { idx, scene: si, kind: k.to_string() });
            // also allow scene-qualified lookup
            self.names.insert(format!("{u}@{si}"), Named { idx, scene: si, kind: k.to_string() });
        }
        self.common(idx, n, in_abs);
        if let (Some(u), true) = (&user, matches!(self.layers[idx].kind, Kind::Html { .. })) {
            // addressable parts of a raw block: data-part="x" -> layer #u.x (animatable with verbs)
            if let Kind::Html { markup } = self.layers[idx].kind.clone() {
                let mut rest = markup.as_str();
                let mut seen = vec![];
                while let Some(p) = rest.find("data-part=\"") {
                    rest = &rest[p + 11..];
                    let Some(e) = rest.find('"') else { break };
                    let part = rest[..e].to_string();
                    if part.is_empty() || seen.contains(&part) {
                        continue;
                    }
                    seen.push(part.clone());
                    let mut pl = Layer::new(format!("{}-part-{}", self.lid(idx), part.replace(|c: char| !c.is_ascii_alphanumeric(), "_")), Kind::Group);
                    pl.tags.push("part".into());
                    let name = format!("{u}.{part}");
                    pl.name = Some(name.clone());
                    let pi = self.add(Some(idx), pl);
                    // state parts of skins start hidden (activate / done reveal them)
                    if matches!(part.as_str(), "active" | "done") {
                        self.set_base(pi, Ch::Opacity, TV::N(0.0));
                    }
                    self.names.insert(format!("{name}@{si}"), Named { idx: pi, scene: si, kind: "part".into() });
                }
            }
        }
        if let (Some(u), "chart") = (&user, k) {
            // parts are addressable: #rev.b1 (bar), #rev.d1 (point), #rev.s1 (slice), 1-based
            let mut all = vec![];
            self.walk_parts(idx, &mut all);
            for j in all {
                let lid = self.lid(j);
                let tags = self.layers[j].tags.clone();
                let part = if tags.iter().any(|t| t == "bar") { "b" } else if tags.iter().any(|t| t == "slice") { "s" } else if matches!(self.layers[j].kind, Kind::Ellipse) && tags.iter().any(|t| t == "dot") { "d" } else { continue };
                if let Some(num) = lid.rsplit(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse::<usize>().ok()) {
                    let name = format!("{u}.{part}{}", num + 1);
                    self.layers[j].name = Some(name.clone());
                    self.names.insert(format!("{name}@{si}"), Named { idx: j, scene: si, kind: "part".into() });
                }
            }
        }
        // nested children: elements; verbs/anim under elements apply to the element
        if !matches!(k, "browser" | "phone" | "title" | "chart" | "code" | "terminal" | "link" | "steps" | "list" | "pill" | "quote" | "device" | "panel" | "progress" | "cloud") && !matches!(self.layers[idx].kind, Kind::Html { .. }) {
            // a window's children go into its body
            let host = if k == "window" { self.kids[idx].iter().copied().find(|&c| self.layers[c].id.ends_with("-body")).unwrap_or(idx) } else { idx };
            let abs = self.layers[host].layout.flow == "none";
            for c in &n.children {
                self.element(host, si, c, abs);
            }
            // nest levels: full-precision transforms (deep levels scale by 1e-4 and less)
            if k == "nest" {
                for c in self.kids[idx].clone() {
                    self.layers[c].tags.push("cam-precise".into());
                }
            }
        }
        Some(idx)
    }

    pub fn asset(&mut self, src: &str, n: &Node) -> String {
        if src.starts_with("http") || src.starts_with("data:") {
            return src.to_string();
        }
        let p = self.base.join(src);
        if !p.exists() {
            self.err("E_ASSET", format!("asset `{src}` not found"), "paths are relative to the .reel file", n.span);
        }
        p.to_string_lossy().to_string()
    }

    /// Layout + style props shared by all elements.
    fn common(&mut self, i: usize, n: &Node, in_abs: bool) {
        for (k, f) in [("w", 0), ("h", 1), ("maxw", 2)] {
            if let Some(l) = self.len(n, k) {
                match f {
                    0 => self.layers[i].layout.w = Some(l),
                    1 => self.layers[i].layout.h = Some(l),
                    _ => self.layers[i].layout.maxw = Some(l),
                }
            }
        }
        let has_xy = n.props.contains_key("x") || n.props.contains_key("y");
        if has_xy || in_abs {
            let lay = &mut self.layers[i].layout;
            if lay.pos == Pos::Flow {
                lay.pos = Pos::Abs;
            }
        }
        if let Some(x) = self.len(n, "x") {
            self.layers[i].layout.x = Some(x);
        }
        if let Some(y) = self.len(n, "y") {
            self.layers[i].layout.y = Some(y);
        }
        if let Some(a) = praw(n, "anchor") {
            self.layers[i].layout.anchor = match a.as_str() {
                "center" => (0.5, 0.5),
                "top" => (0.5, 0.0),
                "bottom" => (0.5, 1.0),
                "left" => (0.0, 0.5),
                "right" => (1.0, 0.5),
                "top-right" => (1.0, 0.0),
                "bottom-left" => (0.0, 1.0),
                "bottom-right" => (1.0, 1.0),
                _ => (0.0, 0.0),
            };
        }
        if let Some(p) = praw(n, "place") {
            self.place(i, &p, n);
        }
        if let Some(p) = self.paint(n, "fill") {
            let is_text = matches!(self.layers[i].kind, Kind::Text { .. } | Kind::Counter { .. }) || self.layers[i].tags.iter().any(|t| t == "split");
            if is_text && !matches!(p, Paint::Solid(_)) {
                self.layers[i].style.text_fill = true;
            }
            if is_text && matches!(p, Paint::Solid(_)) {
                if let Paint::Solid(ColorRef::Lit(c)) = &p {
                    self.layers[i].style.color = Some(c.clone());
                }
            } else {
                self.layers[i].style.fill = Some(p.clone());
            }
            if let Paint::Linear { stops, angle_deg } = &p {
                // expose gradient channels for animation
                for (k, s) in stops.iter().enumerate().take(4) {
                    if let ColorRef::Lit(c) = &s.color {
                        let ch = [Ch::Grad0, Ch::Grad1, Ch::Grad2, Ch::Grad3][k];
                        self.set_base(i, ch, TV::C(c.clone()));
                    }
                }
                self.set_base(i, Ch::GradAngle, TV::N(*angle_deg));
            }
        }
        let grad_stroke = praw(n, "stroke").is_some_and(|v| ["linear(", "radial(", "conic("].iter().any(|p| v.trim_start().to_ascii_lowercase().starts_with(p)));
        if grad_stroke {
            if let Some(p) = self.paint(n, "stroke") {
                let first = match &p {
                    Paint::Linear { stops, .. } | Paint::Radial { stops, .. } | Paint::Conic { stops, .. } => stops.first().and_then(|s| self.th.color(&s.color)),
                    _ => None,
                };
                self.layers[i].style.stroke = Some(first.unwrap_or(hex("#ffffff")));
                self.layers[i].style.stroke_paint = Some(p);
                self.layers[i].style.stroke_w = self.num(n, "sw").unwrap_or(4.0);
            }
        } else if let Some(c) = self.color(n, "stroke") {
            self.layers[i].style.stroke = Some(c);
            self.layers[i].style.stroke_w = self.num(n, "sw").unwrap_or(4.0);
        }
        if let Some(v) = self.num(n, "radius") {
            self.layers[i].style.radius = v;
        }
        if let Some(b) = praw(n, "border") {
            let parts: Vec<&str> = b.split_whitespace().collect();
            let (w, col) = if parts.len() == 2 { (parts[0].trim_end_matches("px").parse().unwrap_or(1.0), parts[1].to_string()) } else { (2.0, b.clone()) };
            if let Some(c) = self.color_s(&col, n.span) {
                self.layers[i].style.border = Some((w, c));
            }
        }
        if let Some(s) = praw(n, "shadow").filter(|s| !s.starts_with("soft")) {
            let sh = match s.as_str() {
                "sm" => Shadow { y: 4.0, blur: 12.0, color: Some(hex("#00000040")), ..Default::default() },
                "md" => Shadow { y: 12.0, blur: 32.0, color: Some(hex("#00000059")), ..Default::default() },
                "lg" | "true" => Shadow { y: 30.0, blur: 80.0, color: Some(hex("#00000073")), ..Default::default() },
                "inner" => Shadow { y: 4.0, blur: 18.0, color: Some(hex("#00000066")), inner: true, ..Default::default() },
                _ => {
                    // "x y blur color"
                    let p: Vec<&str> = s.split_whitespace().collect();
                    let f = |k: usize| p.get(k).and_then(|v| v.trim_end_matches("px").parse::<f64>().ok()).unwrap_or(0.0);
                    let col = p.get(3).and_then(|c| self.color_s(c, n.span));
                    Shadow { x: f(0), y: f(1), blur: f(2), color: col, ..Default::default() }
                }
            };
            self.layers[i].style.shadows.push(sh);
        }
        for (k, ch) in [("opacity", Ch::Opacity), ("blur", Ch::Blur), ("glow", Ch::Glow), ("rot", Ch::Rot), ("scale", Ch::Scale), ("z", Ch::Z), ("rotx", Ch::RotX), ("roty", Ch::RotY), ("brightness", Ch::Brightness), ("saturate", Ch::Saturate), ("contrast", Ch::Contrast), ("hue", Ch::Hue), ("wght", Ch::Wght), ("dx", Ch::X), ("dy", Ch::Y)] {
            // a nest's scale=/rot= are per level (see nest.rs), not its own transform
            if n.kind == "nest" && matches!(k, "scale" | "rot") {
                continue;
            }
            if let Some(v) = self.num(n, k) {
                self.set_base(i, ch, TV::N(v));
            }
        }
        if n.props.contains_key("glow") {
            self.layers[i].style.glow_color = Some(self.color(n, "glow-color").unwrap_or_else(|| self.th.colors["accent2"].clone()));
        }
        self.motion_blur_props(i, n);
        self.looks(i, n);
        self.grade_props(i, n);
        if let Some(b) = praw(n, "blend") {
            self.layers[i].style.blend = Some(b);
        }
        if let Some(m) = self.paint(n, "mask") {
            self.layers[i].style.mask = Some(m);
            self.layers[i].style.mask_luma = praw(n, "mask-mode").as_deref() == Some("luma");
        }
        if let Some(c) = praw(n, "clip") {
            self.layers[i].style.overflow_clip = c == "true" || c == "box";
        }
        if let Some(o) = praw(n, "origin") {
            self.layers[i].style.origin = match o.as_str() {
                "top" => (0.5, 0.0),
                "bottom" => (0.5, 1.0),
                "left" => (0.0, 0.5),
                "right" => (1.0, 0.5),
                "top-left" => (0.0, 0.0),
                "center" => (0.5, 0.5),
                "top-right" => (1.0, 0.0),
                "bottom-left" => (0.0, 1.0),
                "bottom-right" => (1.0, 1.0),
                _ => {
                    let pair = o.split_once(',').and_then(|(x, y)| {
                        let pct = |s: &str| s.trim().strip_suffix('%')?.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v / 100.0);
                        Some((pct(x)?, pct(y)?))
                    });
                    pair.unwrap_or_else(|| {
                        self.err("E_VALUE", format!("invalid origin `{o}`"), "use center, top, bottom, left, right, a corner, or 30%,70%", n.span);
                        (0.5, 0.5)
                    })
                }
            };
        }
        if let Some(v) = self.num(n, "grow") {
            self.layers[i].layout.grow = v;
        }
        if let Some(raw) = n.props.get("aspect").map(|v| v.raw()) {
            // `16:9`, `4:5` or a number (w/h)
            let v = parse_aspect(&raw).map(|(a, b)| a as f64 / b.max(1) as f64).ok().or_else(|| self.num(n, "aspect"));
            self.layers[i].layout.aspect = v;
        }
        if let Some(p) = self.len(n, "pad") {
            self.layers[i].layout.pad = [p.v; 4];
        }
        if let Some(v) = self.num(n, "grain") {
            self.layers[i].style.grain = Some(v);
        }
        if let Some(v) = self.num(n, "vignette") {
            self.layers[i].style.vignette = Some(v);
        }
        if flag(n, "fit") {
            let id = self.lid(i);
            self.hooks.push(Hook::Fit { layer: id });
        }
    }

    fn place(&mut self, i: usize, p: &str, _n: &Node) {
        let (sx, sy) = (self.th.safe.0 * 100.0, self.th.safe.1 * 100.0);
        let (x, ax) = if p.contains("left") { (sx, 0.0) } else if p.contains("right") { (100.0 - sx, 1.0) } else { (50.0, 0.5) };
        let (y, ay) = if p.contains("top") { (sy, 0.0) } else if p.contains("bottom") { (100.0 - sy, 1.0) } else { (50.0, 0.5) };
        let l = &mut self.layers[i].layout;
        l.pos = Pos::Abs;
        l.x = Some(Len::pct(x));
        l.y = Some(Len::pct(y));
        l.anchor = (ax, ay);
    }

    pub fn text_style(&mut self, l: &mut Layer, n: &Node, dflt: &str) {
        let style = praw(n, "style").unwrap_or_else(|| dflt.to_string());
        let size = self.num(n, "size").or_else(|| self.th.size(&style)).unwrap_or(33.0);
        l.style.size = Some(size);
        let w = match style.as_str() {
            "display" | "h1" => 800.0,
            "h2" => 750.0,
            "h3" => 700.0,
            "label" => 650.0,
            _ => 500.0,
        };
        let d = self.th.d.clone();
        let w = match style.as_str() {
            "display" | "h1" => d.w_display,
            "h2" | "h3" => d.w_heading.max(if style == "h3" { 0.0 } else { w - 50.0 }).min(d.w_display),
            "label" => d.w_label,
            _ => d.w_body,
        };
        if matches!(style.as_str(), "display" | "h1" | "h2" | "h3") && d.display_font.is_some() {
            l.style.font = Some(self.th.role_font(true));
        }
        if let (Some(lh), "display" | "h1") = (d.lh_display, style.as_str()) {
            l.style.line_height = Some(lh);
        }
        l.style.weight = Some(self.num(n, "weight").unwrap_or(w));
        let col = match style.as_str() {
            "small" | "label" => "muted",
            _ => "text",
        };
        l.style.color = Some(self.color(n, "color").unwrap_or_else(|| self.th.colors[col].clone()));
        l.style.accent = Some(self.color(n, "accent").unwrap_or_else(|| self.th.colors["mint"].clone()));
        let d = self.th.d.clone();
        l.style.tracking = self.num(n, "tracking").or(match style.as_str() {
            "display" | "h1" => Some(d.tr_display),
            "h2" => Some((d.tr_display + d.tr_heading) / 2.0),
            "label" => Some(d.tr_label),
            _ => None,
        });
        l.style.uppercase = flag(n, "upper") || (style == "label" && d.label_upper);
        l.style.text_align = praw(n, "align").or(Some("center".into()));
        if let Some(lh) = self.num(n, "lh") {
            l.style.line_height = Some(lh);
        }
        l.style.nowrap = flag(n, "nowrap");
        crate::textv2::text_props(self, l, n);
        if let Some(f) = praw(n, "font") {
            l.style.font = Some(if f == "mono" { self.th.mono.clone() } else { f });
        }
    }

    fn text_el(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let txt = text_arg(n).unwrap_or_default();
        if txt.is_empty() {
            self.warn("W_EMPTY_TEXT", "text element without text", "write `text \"...\"`", n.span);
        }
        let dflt = match n.kind.as_str() {
            "text" => "body",
            k => k,
        };
        let split = praw(n, "split");
        let mut l = Layer::new(lid, Kind::Text { runs: self.runs(&txt) });
        l.tags.push("text".into());
        self.text_style(&mut l, n, dflt);
        let by_line = split.as_deref().is_some_and(|sp| sp.starts_with("line"));
        if by_line && self.geo.is_none() && !txt.contains('\n') {
            // pass 1: lay the text out whole; pass 2 splits it at the native line breaks
            let i = self.add(Some(parent), l);
            self.need_geo = true;
            self.want_lines.push(i);
            return i;
        }
        if let Some(sp) = split {
            // kinetic type: one child text layer per word/char, inline flow
            let mut g = l.clone();
            g.kind = Kind::Group;
            g.tags = vec!["text".into(), "split".into()];
            g.layout.flow = "inline";
            g.layout.justify = match g.style.text_align.as_deref() {
                Some("left") => "start".into(),
                Some("right") => "end".into(),
                _ => "center".into(),
            };
            if by_line {
                // one line per child, stacked like the text's own lines (S1: native line breaks)
                g.layout.flow = "col";
                g.layout.gap = 0.0;
                g.layout.align = g.layout.justify.clone();
                g.layout.justify = "start".into();
            }
            let gi = self.add(Some(parent), g);
            let geo_lines = self.geo.as_ref().and_then(|g| g.lines.get(&self.lid(gi)).cloned());
            let pieces: Vec<String> = match sp.as_str() {
                "char" | "chars" | "glyph" => txt.chars().map(|c| c.to_string()).collect(),
                "line" | "lines" => match geo_lines {
                    Some(ls) if !txt.contains('\n') && !ls.is_empty() => ls,
                    _ => txt.split('\n').map(|s| s.to_string()).collect(),
                },
                _ => {
                    if sp != "word" && sp != "words" {
                        self.warn("W_SPLIT", format!("split={sp} unknown"), "use split=word, split=char or split=line", n.span);
                    }
                    txt.split_whitespace().map(|s| s.to_string()).collect()
                }
            };
            let ws = sp.starts_with("word");
            let mut accent = false;
            for (k, p) in pieces.iter().enumerate() {
                let mut text = p.clone();
                if ws && k + 1 < pieces.len() {
                    text.push(' ');
                }
                let mut runs = self.runs(&text);
                // carry {accent} across split words
                for r in runs.iter_mut() {
                    if accent {
                        r.accent = true;
                    }
                }
                if p.contains('{') {
                    accent = !p.contains('}');
                    for r in runs.iter_mut() {
                        r.accent = true;
                    }
                } else if p.contains('}') {
                    accent = false;
                }
                let mut c = Layer::new(format!("{}-p{k}", self.lid(gi)), Kind::Text { runs });
                c.tags.push("piece".into());
                c.style.nowrap = true;
                c.style.text_align = None;
                c.layout.pos = Pos::Flow;
                self.add(Some(gi), c);
            }
            return gi;
        }
        self.add(Some(parent), l)
    }

    fn title_el(&mut self, parent: usize, si: usize, n: &Node, lid: String) -> usize {
        let mut g = Layer::new(lid.clone(), Kind::Group);
        g.layout.flow = "col";
        g.layout.gap = 20.0;
        let gi = self.add(Some(parent), g);
        if let Some(k) = praw(n, "kicker") {
            let kn = Node { kind: "label".into(), args: vec![Value::Str(k)], ..Default::default() };
            let mut l = Layer::new(format!("{}-kicker", self.lid(gi)), Kind::Text { runs: self.runs(&kn.args[0].raw()) });
            l.tags.push("text".into());
            self.text_style(&mut l, &kn, "label");
            l.style.color = Some(self.th.colors["mint"].clone());
            l.style.size = Some(30.0);
            self.add(Some(gi), l);
        }
        let mut main = Node { kind: "display".into(), args: n.args.clone(), props: n.props.clone(), ..Default::default() };
        main.props.shift_remove("sub");
        main.props.shift_remove("kicker");
        for k in ["w", "h", "x", "y", "place", "in", "out", "at", "until", "dur", "stagger", "fill", "pad", "radius", "border", "shadow"] {
            main.props.shift_remove(k);
        }
        main.args.retain(|a| !a.raw().starts_with('#'));
        let row = if n.children.iter().any(|c| matches!(c.kind.as_str(), "svg" | "image" | "icon")) {
            let mut r = Layer::new(format!("{}-row", self.lid(gi)), Kind::Group);
            r.layout.flow = "row";
            r.layout.gap = 30.0;
            let ri = self.add(Some(gi), r);
            for c in &n.children {
                self.element(ri, si, c, false);
            }
            ri
        } else {
            gi
        };
        let mut ml = Layer::new(format!("{}-main", self.lid(gi)), Kind::Text { runs: self.runs(&text_arg(&main).unwrap_or_default()) });
        ml.tags.push("text".into());
        self.text_style(&mut ml, &main, "display");
        self.add(Some(row), ml);
        if let Some(s) = praw(n, "sub") {
            let sn = Node { kind: "h3".into(), args: vec![Value::Str(s.clone())], ..Default::default() };
            let mut sl = Layer::new(format!("{}-sub", self.lid(gi)), Kind::Text { runs: self.runs(&s) });
            sl.tags.push("text".into());
            self.text_style(&mut sl, &sn, "h3");
            sl.style.color = Some(self.color(n, "sub-color").unwrap_or_else(|| self.th.colors["mint"].clone()));
            self.add(Some(gi), sl);
        }
        gi
    }

    fn container(&mut self, parent: usize, _si: usize, n: &Node, lid: String) -> usize {
        let k = n.kind.as_str();
        let mut l = Layer::new(lid, Kind::Group);
        let portrait = self.h > self.w;
        l.layout.flow = match k {
            "row" | "diagram" => "row",
            "col" | "box" | "card" | "node" => "col",
            "grid" => "grid",
            "layer" | "group" | "spacer" => "none",
            _ => "col",
        };
        if let Some(d) = praw(n, "dir") {
            l.layout.flow = match d.as_str() {
                "row" => "row",
                "col" | "column" => "col",
                "auto" => {
                    if portrait {
                        "col"
                    } else {
                        "row"
                    }
                }
                other => {
                    self.err("E_VALUE", format!("dir={other} is not supported"), "dir=row|col|auto", n.span);
                    l.layout.flow
                }
            };
        }
        if k == "diagram" && portrait && praw(n, "dir").is_none() {
            l.layout.flow = "col";
        }
        l.layout.gap = praw_n(n, "gap").unwrap_or(match k {
            "diagram" => 90.0,
            "layer" | "group" => 0.0,
            _ => 24.0,
        });
        l.layout.cols = praw_n(n, "cols").map(|v| v as u32).unwrap_or(if portrait { 1 } else { 3 });
        if let Some(c) = praw_n(n, "cols") {
            if portrait && praw(n, "cols-portrait").is_none() {
                l.layout.cols = (c as u32).min(2);
            }
        }
        l.layout.align = praw(n, "align").unwrap_or_else(|| if l.layout.flow == "row" { "center".into() } else { "center".into() });
        l.layout.justify = praw(n, "justify").unwrap_or_else(|| "center".into());
        l.layout.wrap = flag(n, "wrap");
        if matches!(k, "box" | "card" | "node") {
            l.style.fill = Some(Paint::Solid(ColorRef::Lit(self.th.colors["surface"].clone())));
            l.style.border = Some((1.0, self.th.colors["border"].clone()));
            l.style.radius = self.th.radius;
            l.layout.pad = [30.0, 30.0, 30.0, 30.0];
            l.layout.gap = praw_n(n, "gap").unwrap_or(10.0);
            if k == "node" {
                l.layout.pad = [24.0, 34.0, 24.0, 34.0];
            }
            // a text arg on a box/node becomes its label
        }
        if matches!(k, "layer" | "group") {
            l.layout.pos = Pos::Abs;
            if !n.props.contains_key("w") && !n.props.contains_key("h") && !n.props.contains_key("x") && !n.props.contains_key("y") {
                l.layout.pos = Pos::Fill;
            }
        }
        if k == "spacer" {
            l.layout.grow = 1.0;
        }
        let gi = self.add(Some(parent), l);
        if matches!(k, "box" | "card" | "node") {
            if let Some(t) = text_arg(n) {
                let tn = Node { kind: "h3".into(), args: vec![Value::Str(t.clone())], props: n.props.iter().filter(|(k, _)| ["size", "color", "weight"].contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(), ..Default::default() };
                let mut tl = Layer::new(format!("{}-label", self.lid(gi)), Kind::Text { runs: self.runs(&t) });
                tl.tags.push("text".into());
                self.text_style(&mut tl, &tn, if k == "node" { "h3" } else { "h3" });
                self.add(Some(gi), tl);
            }
            if let Some(s) = praw(n, "sub") {
                let tn = Node { kind: "body".into(), ..Default::default() };
                let mut tl = Layer::new(format!("{}-sub", self.lid(gi)), Kind::Text { runs: self.runs(&s) });
                tl.tags.push("text".into());
                self.text_style(&mut tl, &tn, "small");
                self.add(Some(gi), tl);
            }
        }
        gi
    }

    fn shape(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let k = n.kind.as_str();
        let w = self.num(n, "w").or(self.num(n, "size")).unwrap_or(200.0);
        let h = self.num(n, "h").or(self.num(n, "size")).unwrap_or(if k == "line" { 4.0 } else { w });
        let kind = match k {
            "rect" => Kind::Rect,
            "ellipse" | "circle" => Kind::Ellipse,
            "line" => Kind::Path { d: format!("M0 {} L{} {}", h / 2.0, w, h / 2.0), closed: false, vb: (w, h) },
            "poly" | "star" => {
                let sides = self.num(n, if k == "star" { "points" } else { "sides" }).unwrap_or(if k == "star" { 5.0 } else { 6.0 }) as usize;
                let inner = if k == "star" { self.num(n, "inner").unwrap_or(0.45) } else { 1.0 };
                let mut d = String::new();
                let m = if k == "star" { sides * 2 } else { sides };
                for i in 0..m {
                    let r = if k == "star" && i % 2 == 1 { inner } else { 1.0 };
                    let a = -std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::TAU / m as f64;
                    d.push_str(&format!("{}{:.2} {:.2} ", if i == 0 { "M" } else { "L" }, w / 2.0 + r * w / 2.0 * a.cos(), h / 2.0 + r * h / 2.0 * a.sin()));
                }
                d.push('Z');
                Kind::Path { d, closed: true, vb: (w, h) }
            }
            _ => {
                let d = praw(n, "d").or_else(|| text_arg(n)).unwrap_or_default();
                if d.is_empty() {
                    self.err("E_PATH", "path needs d=\"M ...\"", "write `path d=\"M0 0 C 50 0 50 100 100 100\" w=100 h=100`", n.span);
                }
                Kind::Path { closed: d.trim_end().ends_with(['Z', 'z']), d, vb: (w, h) }
            }
        };
        let mut l = Layer::new(lid, kind);
        l.layout.w = Some(Len::px(w));
        l.layout.h = Some(Len::px(h));
        if k == "circle" || (k == "rect" && !n.props.contains_key("fill") && !n.props.contains_key("stroke")) {
            l.style.fill = Some(Paint::Solid(ColorRef::Lit(self.th.colors["accent2"].clone())));
        }
        if let Some(d) = praw(n, "dash") {
            let p: Vec<f64> = d.trim_matches(|c| c == '[' || c == ']').split([',', ' ']).filter(|x| !x.is_empty()).filter_map(|x| x.trim_end_matches("px").parse().ok()).collect();
            if p.len() == 2 {
                l.style.dash = Some((p[0], p[1]));
            }
        }
        let i = self.add(Some(parent), l);
        if let Some(t) = self.num(n, "trim") {
            self.set_base(i, Ch::TrimEnd, TV::N(t));
        }
        i
    }

    fn lower3(&mut self, si: usize, n: &Node, lid: String) -> usize {
        let ov = self.scenes[si].overlay;
        let mut g = Layer::new(lid, Kind::Group);
        g.layout.pos = Pos::Abs;
        g.layout.x = Some(Len::pct(self.th.safe.0 * 100.0));
        g.layout.y = Some(Len::pct(100.0 - self.th.safe.1 * 100.0 - 16.0));
        g.layout.anchor = (0.0, 1.0);
        g.layout.flow = "col";
        g.layout.align = "start".into();
        g.layout.gap = 6.0;
        g.layout.pad = [18.0, 28.0, 18.0, 28.0];
        g.style.fill = Some(Paint::Solid(ColorRef::Lit(hex("#000000a6"))));
        g.style.radius = 14.0;
        g.style.border = Some((4.0, self.th.colors["accent2"].clone()));
        let gi = self.add(Some(ov), g);
        let mut a = Layer::new(format!("{}-name", self.lid(gi)), Kind::Text { runs: self.runs(&text_arg(n).unwrap_or_default()) });
        a.tags.push("text".into());
        self.text_style(&mut a, &Node::default(), "h3");
        a.style.text_align = Some("left".into());
        self.add(Some(gi), a);
        if let Some(s) = praw(n, "sub") {
            let mut b = Layer::new(format!("{}-role", self.lid(gi)), Kind::Text { runs: self.runs(&s) });
            b.tags.push("text".into());
            self.text_style(&mut b, &Node::default(), "small");
            b.style.text_align = Some("left".into());
            self.add(Some(gi), b);
        }
        gi
    }

    fn chapter_card(&mut self, si: usize, n: &Node, lid: String) -> usize {
        let ov = self.scenes[si].overlay;
        let mut g = Layer::new(lid, Kind::Group);
        g.layout.pos = Pos::Fill;
        g.layout.flow = "col";
        g.layout.align = "start".into();
        g.layout.justify = "center".into();
        g.layout.pad = [0.0, self.w * 0.115, 0.0, self.w * 0.115];
        g.style.fill = Some(self.th.bg.clone());
        let gi = self.add(Some(ov), g);
        let mut inner = Layer::new(format!("{}-in", self.lid(gi)), Kind::Group);
        inner.layout.flow = "col";
        inner.layout.align = "start".into();
        inner.layout.gap = 10.0;
        let ii = self.add(Some(gi), inner);
        let label = praw(n, "label").unwrap_or_else(|| format!("Kapitel {}", praw(n, "n").unwrap_or_default()));
        let mut a = Layer::new(format!("{}-k", self.lid(gi)), Kind::Text { runs: self.runs(&label) });
        a.tags.push("text".into());
        self.text_style(&mut a, &Node::default(), "label");
        a.style.size = Some(30.0);
        a.style.color = Some(self.th.colors["mint"].clone());
        a.style.text_align = Some("left".into());
        self.add(Some(ii), a);
        let mut b = Layer::new(format!("{}-t", self.lid(gi)), Kind::Text { runs: self.runs(&text_arg(n).unwrap_or_default()) });
        b.tags.push("text".into());
        self.text_style(&mut b, &Node::default(), "h1");
        b.style.text_align = Some("left".into());
        self.add(Some(ii), b);
        // timing: rise in, hold, fade out
        let s = self.scenes[si].start;
        let hold = self.dur(n, "for").unwrap_or(1.2);
        let fps = self.fps;
        let t = self.track(ii, Ch::Y);
        t.from_to(s, s + 16.0 / fps, TV::N(30.0), TV::N(0.0), Ease::Cubic(0.65, 0.0, 0.35, 1.0));
        let t = self.track(ii, Ch::Opacity);
        t.base = TV::N(0.0);
        t.from_to(s, s + 14.0 / fps, TV::N(0.0), TV::N(1.0), Ease::Cubic(0.65, 0.0, 0.35, 1.0));
        let t = self.track(gi, Ch::Opacity);
        t.to(s + hold, s + hold + 14.0 / fps, TV::N(0.0), Ease::Linear);
        gi
    }

    fn link(&mut self, si: usize, n: &Node, lid: String) -> usize {
        let ids: Vec<String> = n.args.iter().filter_map(|a| a.as_str()).filter(|s| s.starts_with('#')).map(|s| s[1..].to_string()).collect();
        let (a, b) = match ids.as_slice() {
            [a, b] => (a.clone(), b.clone()),
            [_, a, b] => (a.clone(), b.clone()),
            _ => {
                self.err("E_LINK", "link needs two ids: `link #a -> #b`", "write `link #a -> #b` (optionally `link #name #a -> #b`)", n.span);
                return self.add(Some(self.scenes[si].overlay), Layer::new(lid, Kind::Group));
            }
        };
        let world = self.scenes[si].world;
        let mut l = Layer::new(lid, Kind::Path { d: String::new(), closed: false, vb: (self.w, self.h) });
        l.layout.pos = Pos::Fill;
        l.style.stroke = Some(self.color(n, "color").unwrap_or_else(|| self.th.colors["mint"].clone()));
        l.style.stroke_w = self.num(n, "sw").unwrap_or(4.0);
        if flag(n, "dashed") {
            l.style.dash = Some((10.0, 10.0));
        }
        l.tags.push("link".into());
        let i = self.add(Some(world), l);
        let id = self.lid(i);
        self.hooks.push(Hook::Link { path: id, a, b, curve: flag(n, "curve") });
        i
    }

    // ------------------------------------------------------------ browser / phone mock with screenshot + hotspots
    fn browser(&mut self, parent: usize, si: usize, n: &Node, lid: String) -> usize {
        let phone = n.kind == "phone";
        let shot = praw(n, "shot").unwrap_or_default();
        let shot_path = self.base.join(&shot);
        let (sw, sh) = png_size(&shot_path).unwrap_or_else(|| {
            self.err("E_ASSET", format!("shot `{shot}` not found or not a PNG"), "shots are PNG screenshots, relative to the .reel file", n.span);
            (1280.0, 720.0)
        });
        let hot_path = praw(n, "hotspots").map(|h| self.base.join(h)).unwrap_or_else(|| shot_path.with_extension("json"));
        let hot = load_hotspots(&hot_path).unwrap_or_default();
        let max_w = self.w * (1.0 - 2.0 * self.th.safe.0);
        let vw = self.num(n, "w").unwrap_or(if phone { 420.0 } else { max_w.min(1600.0) }).min(max_w);
        let bar = if phone { 0.0 } else { 40.0 };
        let mut win = Layer::new(lid, Kind::Group);
        win.layout.flow = "col";
        win.layout.w = Some(Len::px(vw));
        win.layout.align = "stretch".into();
        win.layout.justify = "start".into();
        win.style.radius = if phone { 46.0 } else { 14.0 };
        win.style.overflow_clip = true;
        win.style.fill = Some(Paint::Solid(ColorRef::Lit(hex("#ffffff"))));
        win.style.shadows.push(Shadow { y: 30.0, blur: 80.0, color: Some(hex("#00000073")), ..Default::default() });
        win.style.shadows.push(Shadow { blur: 0.0, spread: 1.0, color: Some(hex("#ffffff14")), ..Default::default() });
        if phone {
            win.style.border = Some((12.0, hex("#15181d")));
        }
        win.tags.push("browser".into());
        let wi = self.add(Some(parent), win);
        if !phone {
            let mut tb = Layer::new(format!("{}-bar", self.lid(wi)), Kind::Group);
            tb.layout.flow = "row";
            tb.layout.h = Some(Len::px(bar));
            tb.layout.gap = 8.0;
            tb.layout.justify = "start".into();
            tb.layout.pad = [0.0, 16.0, 0.0, 16.0];
            tb.style.fill = Some(Paint::Solid(ColorRef::Lit(hex("#e9ebef"))));
            let ti = self.add(Some(wi), tb);
            for c in ["#ff5f57", "#febc2e", "#28c840"] {
                let mut d = Layer::new("", Kind::Ellipse);
                d.layout.w = Some(Len::px(12.0));
                d.layout.h = Some(Len::px(12.0));
                d.style.fill = Some(Paint::Solid(ColorRef::Lit(hex(c))));
                self.add(Some(ti), d);
            }
            let url = praw(n, "url").unwrap_or_default();
            let mut u = Layer::new(format!("{}-url", self.lid(wi)), Kind::Text { runs: vec![Run { text: format!("\u{1F512}\u{FE0E}  {url}"), ..Default::default() }] });
            u.style.size = Some(16.0);
            u.style.color = Some(hex("#5d6572"));
            u.style.fill = Some(Paint::Solid(ColorRef::Lit(hex("#ffffff"))));
            u.style.radius = 7.0;
            u.style.text_align = Some("left".into());
            u.style.nowrap = true;
            u.layout.h = Some(Len::px(26.0));
            u.layout.maxw = Some(Len::px(720.0));
            u.layout.grow = 1.0;
            u.layout.pad = [3.0, 12.0, 3.0, 12.0];
            u.layout.x = None;
            self.add(Some(ti), u);
        }
        // crop=[x, y, w, h] (shot px): the viewport shows this region at zoom 1 (portrait framing of wide shots)
        let crop = match n.props.get("crop") {
            Some(Value::List(v)) => v.iter().filter_map(|x| match x { Value::Num(f) => Some(*f), Value::Str(s) => s.parse().ok(), _ => None }).collect::<Vec<f64>>(),
            Some(v) => v.raw().split([',', ' ']).filter_map(|x| x.trim().parse().ok()).collect(),
            None => vec![],
        };
        let crop = if crop.len() == 4 && crop[2] > 0.0 && crop[3] > 0.0 {
            [crop[0].clamp(0.0, sw), crop[1].clamp(0.0, sh), crop[2].min(sw), crop[3].min(sh)]
        } else {
            if n.props.contains_key("crop") {
                self.err("E_CROP", "crop needs four numbers", "write `crop=[x, y, w, h]` in shot pixels", n.span);
            }
            [0.0, 0.0, sw, sh]
        };
        let mut vp = Layer::new(format!("{}-vp", self.lid(wi)), Kind::Group);
        vp.layout.aspect = Some(crop[2] / crop[3]);
        vp.style.overflow_clip = true;
        let vpi = self.add(Some(wi), vp);
        let mut cam = Layer::new(format!("{}-cam", self.lid(wi)), Kind::Group);
        cam.layout.pos = Pos::Fill;
        if crop != [0.0, 0.0, sw, sh] {
            cam.layout.pos = Pos::Abs;
            cam.layout.x = Some(Len::pct(-crop[0] / crop[2] * 100.0));
            cam.layout.y = Some(Len::pct(-crop[1] / crop[3] * 100.0));
            cam.layout.w = Some(Len::pct(sw / crop[2] * 100.0));
            cam.layout.h = Some(Len::pct(sh / crop[3] * 100.0));
        }
        cam.style.origin = (0.0, 0.0);
        let cami = self.add(Some(vpi), cam);
        let mut shots = Layer::new(format!("{}-shots", self.lid(wi)), Kind::Group);
        shots.layout.pos = Pos::Fill;
        let shi = self.add(Some(cami), shots);
        let mut img = Layer::new(format!("{}-s0", self.lid(wi)), Kind::Image { src: shot_path.to_string_lossy().to_string(), fit: "fill".into() });
        img.layout.pos = Pos::Fill;
        let ii = self.add(Some(shi), img);
        let mut marks = Layer::new(format!("{}-marks", self.lid(wi)), Kind::Group);
        marks.layout.pos = Pos::Fill;
        let mi = self.add(Some(cami), marks);
        self.browsers.push(Browser {
            name: id_arg(n).unwrap_or_default(),
            scene: si,
            viewport: vpi,
            cam: cami,
            shots: shi,
            marks: mi,
            cursor: None,
            shot_w: sw,
            shot_h: sh,
            hot,
            cur_img: Some(ii),
            cur_pos: (0.5, 0.5),
            view_w: vw * sw / crop[2],
            crop,
        });
        // children: overlays inside the viewport (toast etc.) and manual hotspots
        for c in &n.children {
            if c.kind == "hot" {
                let r: Vec<f64> = c.args.iter().filter_map(|a| if let Value::Num(v) = a { Some(*v) } else { None }).collect();
                if let (Some(id), 4) = (id_arg(c), r.len()) {
                    let b = self.browsers.last_mut().unwrap();
                    b.hot.insert(id, [r[0], r[1], r[2], r[3]]);
                }
            } else {
                self.element(vpi, si, c, true);
            }
        }
        wi
    }

    fn terminal(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let mut g = Layer::new(lid, Kind::Group);
        g.layout.flow = "col";
        g.layout.align = "stretch".into();
        g.layout.justify = "start".into();
        g.layout.gap = 6.0;
        g.layout.pad = [56.0, 28.0, 28.0, 28.0];
        g.layout.w = Some(Len::px(self.num(n, "w").unwrap_or(1100.0).min(self.w * 0.88)));
        g.style.fill = Some(Paint::Solid(ColorRef::Lit(hex("#0d1117"))));
        g.style.radius = 14.0;
        g.style.shadows.push(Shadow { y: 30.0, blur: 80.0, color: Some(hex("#00000073")), ..Default::default() });
        g.style.font = Some(self.th.mono.clone());
        g.tags.push("terminal".into());
        let gi = self.add(Some(parent), g);
        for (k, c) in ["#ff5f57", "#febc2e", "#28c840"].iter().enumerate() {
            let mut d = Layer::new("", Kind::Ellipse);
            d.layout.pos = Pos::Abs;
            d.layout.x = Some(Len::px(24.0 + k as f64 * 20.0));
            d.layout.y = Some(Len::px(22.0));
            d.layout.w = Some(Len::px(12.0));
            d.layout.h = Some(Len::px(12.0));
            d.style.fill = Some(Paint::Solid(ColorRef::Lit(hex(c))));
            self.add(Some(gi), d);
        }
        let size = self.num(n, "size").unwrap_or(26.0);
        for (k, line) in n.body.clone().unwrap_or_default().lines().enumerate() {
            let cmd = line.starts_with("$ ");
            let mut runs = vec![];
            if cmd {
                runs.push(Run { text: "$ ".into(), color: Some(hex("#3cbf87")), ..Default::default() });
                runs.push(Run { text: line[2..].to_string(), color: Some(hex("#e6edf3")), ..Default::default() });
            } else {
                runs.push(Run { text: line.to_string(), color: Some(hex("#8b949e")), ..Default::default() });
            }
            let mut l = Layer::new(format!("{}-l{k}", self.lid(gi)), Kind::Text { runs });
            l.style.size = Some(size);
            l.style.font = Some(self.th.mono.clone());
            l.style.text_align = Some("left".into());
            l.style.nowrap = true;
            l.style.line_height = Some(1.45);
            l.tags.push(if cmd { "cmd".into() } else { "out".into() });
            l.tags.push("text".into());
            self.add(Some(gi), l);
        }
        gi
    }

    fn code(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let mut g = Layer::new(lid, Kind::Group);
        g.layout.flow = "col";
        g.layout.align = "stretch".into();
        g.layout.justify = "start".into();
        g.layout.pad = [28.0, 32.0, 28.0, 32.0];
        g.style.fill = Some(Paint::Solid(ColorRef::Lit(self.th.colors["code-bg"].clone())));
        g.style.radius = 16.0;
        g.style.border = Some((1.0, self.th.colors["border"].clone()));
        g.style.font = Some(self.th.mono.clone());
        g.tags.push("code".into());
        if let Some(w) = self.num(n, "w") {
            g.layout.w = Some(Len::px(w));
        }
        let gi = self.add(Some(parent), g);
        let size = self.num(n, "size").unwrap_or(self.th.size("code").unwrap_or(28.0));
        let body = n.body.clone().unwrap_or_default();
        let diff = flag(n, "diff") || body.lines().any(|l| l.starts_with("+ ") || l.starts_with("- "));
        let lh = size * 1.5;
        for (k, raw) in body.lines().enumerate() {
            let (mark, line) = if diff && (raw.starts_with('+') || raw.starts_with('-')) {
                (raw.chars().next(), raw[1..].strip_prefix(' ').unwrap_or(&raw[1..]))
            } else if diff {
                (None, raw.strip_prefix("  ").unwrap_or(raw))
            } else {
                (None, raw)
            };
            let mut l = Layer::new(format!("{}-l{k}", self.lid(gi)), Kind::Text { runs: highlight(line) });
            l.style.size = Some(size);
            l.style.font = Some(self.th.mono.clone());
            l.style.text_align = Some("left".into());
            l.style.nowrap = true;
            l.style.line_height = Some(1.5);
            l.layout.h = Some(Len::px(lh));
            l.layout.pad = [0.0, 10.0, 0.0, 10.0];
            l.style.radius = 6.0;
            l.style.color = Some(hex("#e6f2ec"));
            l.tags.push("codeline".into());
            l.tags.push("text".into());
            match mark {
                Some('+') => {
                    l.tags.push("add".into());
                    l.style.fill = Some(Paint::Solid(ColorRef::Lit(hex("#3cbf8700"))));
                }
                Some('-') => {
                    l.tags.push("del".into());
                    l.style.fill = Some(Paint::Solid(ColorRef::Lit(hex("#ff6b6b00"))));
                }
                _ => {}
            }
            let li = self.add(Some(gi), l);
            if mark == Some('+') {
                self.set_base(li, Ch::LineH, TV::N(0.0));
                self.set_base(li, Ch::Opacity, TV::N(0.0));
            }
        }
        gi
    }

    fn chart(&mut self, parent: usize, _si: usize, n: &Node, lid: String) -> usize {
        let ty = praw(n, "type").unwrap_or("bar".into());
        let data: Vec<f64> = match n.props.get("data") {
            Some(Value::List(v)) => v.iter().filter_map(|x| match x {
                Value::Num(f) => Some(*f),
                Value::Str(s) => s.parse().ok(),
                _ => None,
            }).collect(),
            _ => {
                self.err("E_CHART", "chart needs data=[...]", "write `chart type=bar data=[3, 5, 8] labels=[\"A\", \"B\", \"C\"]`", n.span);
                vec![]
            }
        };
        let labels: Vec<String> = match n.props.get("labels") {
            Some(Value::List(v)) => v.iter().map(|x| x.raw()).collect(),
            _ => vec![],
        };
        if !labels.is_empty() && labels.len() != data.len() {
            self.err("E_CHART", format!("{} labels for {} values", labels.len(), data.len()), "give one label per data value", n.span);
        }
        let w = self.num(n, "w").unwrap_or(1100.0).min(self.w * (1.0 - 2.0 * self.th.safe.0));
        let h = self.num(n, "h").unwrap_or(520.0);
        let max = self.num(n, "max").unwrap_or_else(|| data.iter().cloned().fold(0.0, f64::max).max(1e-9));
        let col = self.color(n, "color").unwrap_or_else(|| self.th.colors["accent2"].clone());
        let unit = praw(n, "unit").unwrap_or_default();
        let mut g = Layer::new(lid, Kind::Group);
        g.layout.w = Some(Len::px(w));
        g.layout.h = Some(Len::px(h));
        g.tags.push(format!("chart-{ty}"));
        let gi = self.add(Some(parent), g);
        let label_h = 44.0;
        let val_h = 44.0;
        let plot_h = h - label_h - val_h;
        let nn = data.len().max(1) as f64;
        match ty.as_str() {
            "bar" => {
                let mut row = Layer::new(format!("{}-bars", self.lid(gi)), Kind::Group);
                row.layout.pos = Pos::Fill;
                row.layout.flow = "row";
                row.layout.align = "end".into();
                row.layout.gap = (w / nn * 0.25).min(40.0);
                let ri = self.add(Some(gi), row);
                for (k, v) in data.iter().enumerate() {
                    let mut c = Layer::new(format!("{}-c{k}", self.lid(gi)), Kind::Group);
                    c.layout.flow = "col";
                    c.layout.grow = 1.0;
                    c.layout.justify = "end".into();
                    c.layout.align = "stretch".into();
                    c.layout.gap = 8.0;
                    let ci = self.add(Some(ri), c);
                    let mut val = Layer::new(format!("{}-v{k}", self.lid(gi)), Kind::Counter { prefix: String::new(), suffix: unit.clone() });
                    val.style.size = Some(30.0);
                    val.style.weight = Some(700.0);
                    val.style.color = Some(self.th.colors["text"].clone());
                    val.style.text_align = Some("center".into());
                    val.tags.push("text".into());
                    let vi = self.add(Some(ci), val);
                    self.set_base(vi, Ch::Count, TV::N(*v));
                    let mut b = Layer::new(format!("{}-b{k}", self.lid(gi)), Kind::Rect);
                    b.layout.h = Some(Len::px((v / max).clamp(0.0, 1.0) * plot_h));
                    b.style.fill = Some(Paint::Linear { angle_deg: 180.0, stops: vec![Stop { color: ColorRef::Lit(col.clone()), pos: Some(0.0) }, Stop { color: ColorRef::Lit(Rgba { a: 0.55, ..col.clone() }), pos: Some(1.0) }] });
                    b.style.radius = 10.0;
                    b.style.origin = (0.5, 1.0);
                    b.tags.push("bar".into());
                    self.add(Some(ci), b);
                    let mut lab = Layer::new(format!("{}-lab{k}", self.lid(gi)), Kind::Text { runs: vec![Run { text: labels.get(k).cloned().unwrap_or_default(), ..Default::default() }] });
                    lab.style.size = Some(24.0);
                    lab.style.color = Some(self.th.colors["muted"].clone());
                    lab.style.text_align = Some("center".into());
                    lab.style.nowrap = true;
                    lab.tags.push("text".into());
                    self.add(Some(ci), lab);
                }
            }
            "line" | "area" => {
                let pts: Vec<(f64, f64)> = data.iter().enumerate().map(|(k, v)| {
                    let x = if data.len() > 1 { k as f64 / (data.len() - 1) as f64 * (w - 40.0) + 20.0 } else { w / 2.0 };
                    (x, val_h + plot_h * (1.0 - (v / max).clamp(0.0, 1.0)))
                }).collect();
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
                if ty == "area" {
                    let mut a = Layer::new(format!("{}-area", self.lid(gi)), Kind::Path { d: format!("{d} L{:.1} {:.1} L{:.1} {:.1} Z", pts.last().map(|p| p.0).unwrap_or(0.0), val_h + plot_h, pts[0].0, val_h + plot_h), closed: true, vb: (w, h) });
                    a.layout.pos = Pos::Fill;
                    a.style.fill = Some(Paint::Linear { angle_deg: 180.0, stops: vec![Stop { color: ColorRef::Lit(Rgba { a: 0.45, ..col.clone() }), pos: Some(0.0) }, Stop { color: ColorRef::Lit(Rgba { a: 0.0, ..col.clone() }), pos: Some(1.0) }] });
                    a.style.clip = Some(ClipShape::Wipe("left".into()));
                    a.tags.push("area".into());
                    self.add(Some(gi), a);
                }
                let mut l = Layer::new(format!("{}-line", self.lid(gi)), Kind::Path { d, closed: false, vb: (w, h) });
                l.layout.pos = Pos::Fill;
                l.style.stroke = Some(col.clone());
                l.style.stroke_w = 6.0;
                l.tags.push("line".into());
                self.add(Some(gi), l);
                for (k, (x, y)) in pts.iter().enumerate() {
                    let mut dot = Layer::new(format!("{}-d{k}", self.lid(gi)), Kind::Ellipse);
                    dot.layout.pos = Pos::Abs;
                    dot.layout.x = Some(Len::px(*x));
                    dot.layout.y = Some(Len::px(*y));
                    dot.layout.anchor = (0.5, 0.5);
                    dot.layout.w = Some(Len::px(18.0));
                    dot.layout.h = Some(Len::px(18.0));
                    dot.style.fill = Some(Paint::Solid(ColorRef::Lit(self.th.colors["text"].clone())));
                    dot.style.border = Some((4.0, col.clone()));
                    dot.tags.push("dot".into());
                    self.add(Some(gi), dot);
                    if flag(n, "values") {
                        let mut t = Layer::new(format!("{}-val{k}", self.lid(gi)), Kind::Text { runs: vec![Run { text: format!("{}{}", fmt_v(data[k]), unit), ..Default::default() }] });
                        t.layout.pos = Pos::Abs;
                        t.layout.x = Some(Len::px(*x));
                        t.layout.y = Some(Len::px(y - 18.0));
                        t.layout.anchor = (0.5, 1.0);
                        t.style.size = Some(26.0);
                        t.style.weight = Some(650.0);
                        t.style.color = Some(self.th.colors["text"].clone());
                        t.style.nowrap = true;
                        t.tags.push("text".into());
                        t.tags.push("dot".into());
                        self.add(Some(gi), t);
                    }
                    if let Some(lab) = labels.get(k) {
                        let mut t = Layer::new(format!("{}-lab{k}", self.lid(gi)), Kind::Text { runs: vec![Run { text: lab.clone(), ..Default::default() }] });
                        t.layout.pos = Pos::Abs;
                        t.layout.x = Some(Len::px(*x));
                        t.layout.y = Some(Len::px(h - 8.0));
                        t.layout.anchor = (0.5, 1.0);
                        t.style.size = Some(24.0);
                        t.style.color = Some(self.th.colors["muted"].clone());
                        t.style.nowrap = true;
                        t.tags.push("text".into());
                        self.add(Some(gi), t);
                    }
                }
            }
            "pie" | "donut" => {
                let total: f64 = data.iter().sum::<f64>().max(1e-9);
                let r = (h.min(w) / 2.0 - 30.0).max(10.0);
                let (cx, cy) = (r + 20.0, h / 2.0);
                let palette = ["accent2", "mint", "warn", "danger", "muted", "accent"];
                let mut acc = 0.0;
                for (k, v) in data.iter().enumerate() {
                    let a0 = acc / total * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
                    acc += v;
                    let a1 = acc / total * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
                    let rr = r * 0.78;
                    let large = if a1 - a0 > std::f64::consts::PI { 1 } else { 0 };
                    let d = format!("M{:.1} {:.1} A{rr:.1} {rr:.1} 0 {large} 1 {:.1} {:.1}", cx + rr * a0.cos(), cy + rr * a0.sin(), cx + rr * a1.cos(), cy + rr * a1.sin());
                    let mut s = Layer::new(format!("{}-s{k}", self.lid(gi)), Kind::Path { d, closed: false, vb: (w, h) });
                    s.layout.pos = Pos::Fill;
                    let c = self.th.colors[palette[k % palette.len()]].clone();
                    s.style.stroke = Some(c.clone());
                    s.style.stroke_w = if ty == "pie" { r * 0.44 } else { r * 0.3 };
                    s.tags.push("slice".into());
                    self.add(Some(gi), s);
                    if let Some(lab) = labels.get(k) {
                        let mut t = Layer::new(format!("{}-lab{k}", self.lid(gi)), Kind::Text { runs: vec![Run { text: format!("{lab}  {}{}", fmt_v(*v), unit), ..Default::default() }] });
                        t.layout.pos = Pos::Abs;
                        t.layout.x = Some(Len::px(2.0 * r + 80.0));
                        t.layout.y = Some(Len::px(h / 2.0 + (k as f64 - (nn - 1.0) / 2.0) * 56.0));
                        t.layout.anchor = (0.0, 0.5);
                        t.style.size = Some(30.0);
                        t.style.color = Some(c);
                        t.style.weight = Some(650.0);
                        t.style.nowrap = true;
                        t.tags.push("text".into());
                        self.add(Some(gi), t);
                    }
                }
            }
            _ => self.err("E_CHART", format!("unknown chart type `{ty}`"), "type=bar|line|area|pie|donut", n.span),
        }
        gi
    }
}

impl<'a> Ctx<'a> {
    fn walk_parts(&self, i: usize, out: &mut Vec<usize>) {
        for &k in &self.kids[i] {
            out.push(k);
            self.walk_parts(k, out);
        }
    }
}

fn fmt_v(v: f64) -> String {
    crate::ast::fmt_num(v)
}

fn absolutize_src(markup: &str, base: &std::path::Path) -> String {
    let mut out = String::new();
    let mut rest = markup;
    while let Some(p) = rest.find("src=\"") {
        out.push_str(&rest[..p + 5]);
        rest = &rest[p + 5..];
        let end = rest.find('"').unwrap_or(rest.len());
        let v = &rest[..end];
        if v.starts_with("http") || v.starts_with("data:") || v.starts_with('/') || v.starts_with("file:") || v.is_empty() {
            out.push_str(v);
        } else {
            out.push_str(&format!("file://{}", base.join(v).display()));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

pub fn praw_n(n: &Node, k: &str) -> Option<f64> {
    n.props.get(k).and_then(|v| match v {
        Value::Num(f) => Some(*f),
        Value::Str(s) => s.trim_end_matches("px").parse().ok(),
        _ => None,
    })
}
fn praw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}

/// Tiny generic syntax highlighter (keywords, strings, numbers, comments, calls).
pub fn highlight(line: &str) -> Vec<Run> {
    let col = |c: &str| Some(hex(c));
    let (kw, st, nu, co, fun, tx) = ("#7cc7ff", "#a5e3a1", "#ffc857", "#6b8f80", "#f5a97f", "#e6f2ec");
    let mut out: Vec<Run> = vec![];
    let push = |out: &mut Vec<Run>, t: &str, c: &str| {
        if t.is_empty() {
            return;
        }
        if let Some(l) = out.last_mut() {
            if l.color == Some(hex(c)) {
                l.text.push_str(t);
                return;
            }
        }
        out.push(Run { text: t.to_string(), color: col(c), ..Default::default() });
    };
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let rest: String = chars[i..].iter().collect();
        if rest.starts_with("//") || rest.starts_with("# ") || rest.starts_with("--") {
            push(&mut out, &rest, co);
            break;
        }
        if c == '"' || c == '\'' || c == '`' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c {
                if chars[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            let s: String = chars[i..(j + 1).min(chars.len())].iter().collect();
            push(&mut out, &s, st);
            i = j + 1;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '.' || chars[j] == '_') {
                j += 1;
            }
            push(&mut out, &chars[i..j].iter().collect::<String>(), nu);
            i = j;
            continue;
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '$') {
                j += 1;
            }
            let w: String = chars[i..j].iter().collect();
            let color = if KEYWORDS.contains(&w.as_str()) { kw } else if chars.get(j) == Some(&'(') { fun } else { tx };
            push(&mut out, &w, color);
            i = j;
            continue;
        }
        push(&mut out, &c.to_string(), tx);
        i += 1;
    }
    if out.is_empty() {
        out.push(Run { text: " ".into(), ..Default::default() });
    }
    out
}

impl<'a> Ctx<'a> {
    /// `mblur=<samples>|on|off shutter=<angle>|<n>f` on an element (or on `anim`): reel-ir
    /// `motion_blur {samples, shutter}` on the layer's node. Shutter angle as in a film camera:
    /// 180deg = half a frame (the default), 360deg = one frame, max 4 frames (1440deg).
    /// `tint=#c tint-amount=0..1`, `levels=in_black,in_white[,gamma[,out_black,out_white]]`
    /// (0..1 or 0..255), `lut=grade.cube`: reel-ir `color` effect (direct mode only).
    pub fn grade_props(&mut self, i: usize, n: &Node) {
        if let Some(c) = self.color(n, "tint") {
            let amount = self.num(n, "tint-amount").unwrap_or(1.0).clamp(0.0, 1.0);
            self.layers[i].style.tint = Some((c, amount));
        }
        // a nest's levels= is its level count (nest.rs)
        if let Some(raw) = praw(n, "levels").filter(|_| n.kind != "nest") {
            let v: Vec<f64> = raw.trim_matches(|c| c == '[' || c == ']').split([',', ' ']).filter(|s| !s.is_empty()).filter_map(|s| s.parse::<f64>().ok()).collect();
            let ok = matches!(v.len(), 2 | 3 | 5);
            if !ok {
                self.err("E_VALUE", format!("`levels={raw}`: expected 2, 3 or 5 numbers"), "write `levels=0.05,0.95` (in black, in white), `levels=0.05,0.95,1.2` (+ gamma) or `levels=0.05,0.95,1.2,0,1` (+ output range)", n.span);
            } else {
                // 0..255 input when any level value (not the gamma) exceeds 1
                let big = v.iter().enumerate().any(|(k, x)| k != 2 && *x > 1.0);
                let s = |x: f64| if big { x / 255.0 } else { x };
                let l = [s(v[0]), s(v[1]), v.get(2).copied().unwrap_or(1.0), v.get(3).map(|x| s(*x)).unwrap_or(0.0), v.get(4).map(|x| s(*x)).unwrap_or(1.0)];
                if l[1] <= l[0] || l[2] <= 0.0 {
                    self.err("E_VALUE", format!("`levels={raw}`: in_white must exceed in_black and gamma be > 0"), "write e.g. `levels=0.05,0.95,1.1`", n.span);
                } else {
                    self.layers[i].style.levels = Some(l);
                }
            }
        }
        if let Some(src) = praw(n, "lut") {
            let p = self.asset(&src, n);
            if std::path::Path::new(&p).exists() {
                match agentfx_ir::cube::Cube::load(std::path::Path::new(&p)) {
                    Ok(_) => self.layers[i].style.lut = Some(p),
                    Err(e) => self.err("E_ASSET", format!("`lut={src}`: {e}"), "a 3D .cube LUT (LUT_3D_SIZE n, n^3 rows of r g b)", n.span),
                }
            }
        }
    }

    pub fn motion_blur_props(&mut self, i: usize, n: &Node) {
        let mb = n.props.get("mblur").map(|v| v.raw());
        let sh = n.props.get("shutter").map(|v| v.raw());
        if mb.is_none() && sh.is_none() {
            return;
        }
        let samples: Option<u32> = match mb.as_deref() {
            None | Some("on") | Some("true") => Some(16),
            Some("off") | Some("false") | Some("0") => None,
            Some(s) => match s.parse::<f64>() {
                Ok(v) if v.fract() == 0.0 && (2.0..=64.0).contains(&v) => Some(v as u32),
                _ => {
                    self.err("E_VALUE", format!("`mblur={s}`: motion blur samples must be an integer 2..64"), "write `mblur=16` (or `mblur=on`, 16 samples; `mblur=off`)", n.span);
                    return;
                }
            },
        };
        let shutter = match sh.as_deref() {
            None => 0.5,
            Some(s) => {
                let s = s.trim();
                let v = if let Some(f) = s.strip_suffix('f') { f.parse::<f64>().ok() } else { s.trim_end_matches("deg").parse::<f64>().ok().map(|d| d / 360.0) };
                match v {
                    Some(v) if v > 0.0 && v <= 4.0 => v,
                    _ => {
                        self.err("E_VALUE", format!("`shutter={s}`: shutter must be an angle in (0, 1440deg] or frames in (0, 4f]"), "write `shutter=180deg` (half a frame, the film look) or `shutter=1f`", n.span);
                        return;
                    }
                }
            }
        };
        self.layers[i].style.motion_blur = samples.map(|s| (s, shutter));
    }
}

