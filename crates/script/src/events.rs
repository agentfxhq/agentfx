//! Pass 2: timed events -> tracks. Story verbs compile into motion-level tracks here.
use crate::ast::{Node, Span, Value};
use crate::ir::*;
use crate::lower::*;
use crate::theme::hex;
use crate::values::*;

pub enum Target {
    El(usize),
    Hot(usize, [f64; 4]),
    Browser(usize),
    Camera(usize),
}

fn praw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}
const STD: Ease = Ease::Cubic(0.65, 0.0, 0.35, 1.0);
const OUT: Ease = Ease::Cubic(0.33, 1.0, 0.68, 1.0);
/// Chart build clock (design/motion-tokens.json): axes/gridlines fade over `duration.short4`
/// (rounded to 0.35 s), the data draw-on starts one
/// `roles.stagger.item` later, and the first line marker waits for a 2 % stub of the stroke.
const AXES_D: f64 = 0.35;
const STAGGER_ITEM: f64 = 0.133;
const DOT_LEAD: f64 = 0.02;

pub const ENTER: &[&str] = &["fade", "rise", "drop", "left", "right", "pop", "zoom", "blur", "wipe", "iris", "draw", "type", "flip", "count", "none", "show"];
pub const EMPH: &[&str] = &["pulse", "shake", "glow", "bounce", "wobble", "flash"];

impl<'a> Ctx<'a> {
    // ------------------------------------------------------------ collection
    pub fn collect_events(&mut self, si: usize, sn: &Node) {
        let start = self.scenes[si].start;
        let mut say_ix = 0usize;
        let children = sn.children.clone();
        for n in &children {
            if n.kind == "say" {
                for c in &n.children {
                    if c.kind == "at" {
                        self.at_node(si, Some(say_ix), c, None);
                    }
                }
                say_ix += 1;
            } else if n.kind == "at" {
                self.at_node(si, None, n, None);
            } else if is_verb(&n.kind) || crate::world::director_verb(n).is_some() {
                self.push_ev(start, si, n.clone(), None);
            } else {
                self.element_events(si, n, start, None);
            }
        }
    }

    fn push_ev(&mut self, t: f64, si: usize, node: Node, target: Option<String>) {
        let order = self.events.len();
        self.events.push(Ev { t, scene: si, node, order, target });
    }

    fn at_node(&mut self, si: usize, say: Option<usize>, at: &Node, target: Option<String>) {
        let expr = at.args.first().map(|v| v.raw()).unwrap_or_default();
        // `prev` = the previous event in source order (as in the timing solver), not the latest time
        let prev = self.events.iter().rev().find(|e| e.scene == si).map(|e| e.t).unwrap_or(self.scenes[si].start);
        let t = match self.tl.anchor_time(si, say, &expr, prev, at.span) {
            Ok(t) => t,
            Err(_) => return, // reported by timing
        };
        for v in &at.children {
            if v.kind == "at" {
                self.at_node(si, say, v, target.clone());
            } else {
                self.push_ev(t, si, v.clone(), target.clone());
            }
        }
    }

    /// in=/out= props, nested verbs/anims of elements.
    fn element_events(&mut self, si: usize, n: &Node, scene_t: f64, parent_stagger: Option<(f64, usize)>) {
        let id = id_arg(n);
        let mut t0 = scene_t;
        if let Some(at) = praw(n, "at") {
            match self.tl.anchor_time(si, None, at.trim_start_matches('@'), scene_t, n.span) {
                Ok(t) => t0 = t,
                Err(d) => self.diags.push(d),
            }
        } else if let Some((st, k)) = parent_stagger {
            t0 = scene_t + st * k as f64;
        }
        if let Some(d) = self.dur(n, "delay") {
            t0 += d;
        }
        if let (Some(p), Some(id)) = (praw(n, "in"), id.clone()) {
            let mut e = Node { kind: "enter".into(), args: vec![Value::Str(format!("#{id}")), Value::Str(p)], span: n.span, ..Default::default() };
            for k in ["dur", "ease", "stagger", "to", "value", "from"] {
                if let Some(v) = n.props.get(k) {
                    e.props.insert(k.into(), v.clone());
                }
            }
            self.push_ev(t0, si, e, None);
        } else if n.props.contains_key("in") {
            self.warn("W_IN_NO_ID", "in= needs an element id to animate", "add an id, e.g. `text #hello \"...\" in=rise`", n.span);
        }
        if let (Some(p), Some(id)) = (praw(n, "out"), id.clone()) {
            let t1 = match praw(n, "until") {
                Some(u) => self.tl.anchor_time(si, None, u.trim_start_matches('@'), t0, n.span).unwrap_or(self.scenes[si].start + self.scenes[si].dur - 0.5),
                None => self.scenes[si].start + self.scenes[si].dur - 0.5,
            };
            let e = Node { kind: "exit".into(), args: vec![Value::Str(format!("#{id}")), Value::Str(p)], span: n.span, ..Default::default() };
            self.push_ev(t1, si, e, None);
        }
        if n.kind == "particles" {
            if let (Some(em), Some(id)) = (praw(n, "emit"), id.clone()) {
                if let Ok(t) = self.tl.anchor_time(si, None, em.trim_start_matches('@'), scene_t, n.span) {
                    let e = Node { kind: "burst".into(), args: vec![Value::Str(format!("#{id}"))], span: n.span, ..Default::default() };
                    self.push_ev(t, si, e, None);
                }
            }
        }
        let st = self.dur(n, "stagger");
        let mut k = 0usize;
        for c in &n.children {
            if c.kind == "at" {
                self.at_node(si, None, c, id.clone());
            } else if is_verb(&c.kind) {
                self.push_ev(t0, si, c.clone(), id.clone());
            } else {
                let ps = st.map(|s| (s, k));
                if c.props.contains_key("in") {
                    k += 1;
                }
                self.element_events(si, c, t0, ps);
            }
        }
    }

    // ------------------------------------------------------------ targets
    pub fn target(&mut self, e: &Ev, required: bool) -> Option<Target> {
        let n = &e.node;
        let name = id_arg(n).or_else(|| e.target.clone());
        let Some(name) = name else {
            if required {
                self.err("E_TARGET", format!("`{}` needs a target id", n.kind), format!("write `{} #id`", n.kind), n.span);
            }
            return None;
        };
        let si = e.scene;
        if name == "camera" {
            if let Some(nm) = self.names.get(&format!("camera@{si}")) {
                return Some(Target::Camera(nm.idx));
            }
            let w = self.scenes[si].world;
            return Some(Target::Camera(w));
        }
        if let Some(nm) = self.names.get(&format!("{name}@{si}")) {
            let i = nm.idx;
            if let Some(b) = self.browsers.iter().position(|b| b.scene == si && b.name == name) {
                return Some(Target::Browser(b));
            }
            return Some(Target::El(i));
        }
        for (bi, b) in self.browsers.iter().enumerate() {
            if b.scene == si {
                if let Some(r) = b.hot.get(&name) {
                    return Some(Target::Hot(bi, *r));
                }
            }
        }
        // unknown: suggest
        let mut cands: Vec<String> = self.names.iter().filter(|(_, v)| v.scene == si).map(|(k, _)| k.split('@').next().unwrap().to_string()).collect();
        for b in self.browsers.iter().filter(|b| b.scene == si) {
            cands.extend(b.hot.keys().cloned());
        }
        cands.sort();
        cands.dedup();
        let refs: Vec<&str> = cands.iter().map(|s| s.as_str()).collect();
        let hint = match suggest(&name, &refs) {
            Some(s) => format!("did you mean `#{s}`?"),
            None if refs.is_empty() => "this scene has no ids; give an element an id like `text #title \"...\"`".to_string(),
            None => format!("ids in this scene: {}", refs.iter().take(12).map(|s| format!("#{s}")).collect::<Vec<_>>().join(" ")),
        };
        let other = self.names.get(&name).map(|nm| nm.scene);
        let msg = match other {
            Some(os) if os != si => format!("`#{name}` belongs to scene `{}`, not `{}`", self.scenes[os].id, self.scenes[si].id),
            _ => format!("unknown id `#{name}` in scene `{}`", self.scenes[si].id),
        };
        self.diags.push(crate::diag::Diag::error("E_ID", msg, hint, n.span).with_id(name).with_at(e.t));
        None
    }

    // ------------------------------------------------------------ apply
    pub fn apply(&mut self, e: &Ev) {
        let n = &e.node;
        let t = e.t;
        let k = n.kind.as_str();
        if self.s3d_apply(e) {
            return;
        }
        match k {
            "gesture" | "mood" => self.presenter_event(e),
            "enter" | "show" => {
                let preset = if k == "show" { "show".to_string() } else { preset_arg(n).unwrap_or_else(|| "fade".into()) };
                match self.target(e, true) {
                    Some(Target::El(i)) => {
                        let d = self.dur(n, "dur");
                        let ease = n.props.get("ease").map(|_| self.ease(n, "ease", OUT));
                        let st = self.dur(n, "stagger");
                        self.enter(i, &preset, t, d, ease, st, n);
                    }
                    Some(Target::Hot(b, r)) => self.mark(b, r, t, n, "highlight"),
                    Some(Target::Browser(b)) => {
                        let wi = self.parent_of(self.browsers[b].viewport);
                        self.enter(wi, &preset, t, None, None, None, n);
                    }
                    _ => {}
                }
            }
            "exit" | "hide" => {
                let preset = if k == "hide" { "none".to_string() } else { preset_arg(n).unwrap_or_else(|| "fade".into()) };
                let i = match self.target(e, true) {
                    Some(Target::El(i)) => i,
                    Some(Target::Browser(b)) => self.parent_of(self.browsers[b].viewport),
                    _ => return,
                };
                let d = self.dur(n, "dur").unwrap_or(self.th.durs.get("exit-dur").copied().unwrap_or(self.th.dur("exit")));
                self.exit(i, &preset, t, d);
            }
            "emph" | "pulse" | "shake" | "glow" | "bounce" | "flash" => {
                let preset = if k == "emph" { preset_arg(n).unwrap_or_else(|| "pulse".into()) } else { k.to_string() };
                match self.target(e, true) {
                    Some(Target::El(i)) => self.emph(i, &preset, t, n),
                    Some(Target::Hot(b, r)) => self.mark(b, r, t, n, "pulse"),
                    _ => {}
                }
            }
            "highlight" | "spotlight" | "callout" => match self.target(e, true) {
                Some(Target::Hot(b, r)) => self.mark(b, r, t, n, k),
                Some(Target::El(i)) => {
                    if k == "highlight" {
                        self.emph(i, "glow", t, n);
                    } else if k == "callout" {
                        self.warn("W_CALLOUT_LAYOUT", "callout on a laid-out element is placed at the top of the scene in S0", "use callout on a screenshot hotspot, or a lower3/text element", n.span);
                        let r = [0.0, 0.0, 0.0, 0.0];
                        let _ = r;
                    } else {
                        self.emph(i, "glow", t, n);
                    }
                }
                _ => {}
            },
            "zoom" | "pan" => self.zoom(e),
            "cursor" | "click" | "dblclick" | "drag" => self.cursor_verb(e),
            "type" => match self.target(e, true) {
                Some(Target::Hot(b, r)) => self.type_hot(b, r, t, n),
                Some(Target::El(i)) => {
                    let d = self.dur(n, "dur");
                    self.enter(i, "type", t, d, None, None, n)
                }
                _ => {}
            },
            "swap" => self.swap(e),
            "count" => {
                if let Some(Target::El(i)) = self.target(e, true) {
                    let i = self.counter_in(i);
                    let to = self.num(n, "to").or_else(|| n.args.iter().find_map(|a| if let Value::Num(v) = a { Some(*v) } else { None })).unwrap_or(0.0);
                    let d = self.dur(n, "dur").unwrap_or(1.2);
                    let ease = self.ease(n, "ease", OUT);
                    self.track(i, Ch::Count).to(t, t + d, TV::N(to), ease);
                }
            }
            "sweep" => {
                if let Some(Target::El(i)) = self.target(e, true) {
                    self.sweep_verb(i, t, n);
                }
            }
            "draw" => {
                if let Some(Target::El(i)) = self.target(e, true) {
                    let d = self.dur(n, "dur");
                    self.enter(i, "draw", t, d, None, None, n);
                }
            }
            "diff" => {
                if let Some(Target::El(i)) = self.target(e, true) {
                    self.diff(i, t, n);
                }
            }
            "burst" => {
                if let Some(Target::El(i)) = self.target(e, true) {
                    self.burst(i, t, e);
                }
            }
            "move" => {
                if let Some(Target::El(i)) = self.target(e, true) {
                    let d = self.dur(n, "dur").unwrap_or(self.th.durs.get("move-dur").copied().unwrap_or(self.th.dur("move")));
                    let mv = self.th.ease("move");
                    let ease = self.ease(n, "ease", mv);
                    // move to=#other: center onto the other element's box (native layout, S1); x/y add on top
                    let mut base = (0.0, 0.0);
                    let mut to_ok = false;
                    if let Some(to) = n.props.get("to").map(|v| v.raw().trim_start_matches('#').to_string()) {
                        let me = id_arg(n).or_else(|| e.target.clone()).unwrap_or_default();
                        let si = e.scene;
                        match &self.geo {
                            None => self.need_geo = true,
                            Some(g) => match (g.rects.get(&format!("{me}@{si}")), g.rects.get(&format!("{to}@{si}"))) {
                                (Some(a), Some(b)) => {
                                    base = (b[0] + b[2] / 2.0 - a[0] - a[2] / 2.0, b[1] + b[3] / 2.0 - a[1] - a[3] / 2.0);
                                    to_ok = true;
                                }
                                _ => self.err("E_ID", format!("move to=#{to}: no laid-out element `#{to}` in this scene"), "use the id of a visible element of the same scene", n.span),
                            },
                        }
                    }
                    for (k, ch) in [("x", Ch::X), ("y", Ch::Y), ("z", Ch::Z), ("rot", Ch::Rot), ("scale", Ch::Scale)] {
                        let off = match ch { Ch::X => base.0, Ch::Y => base.1, _ => 0.0 };
                        match self.num(n, k) {
                            Some(v) => self.track(i, ch).to(t, t + d, TV::N(v + off), ease.clone()),
                            None if to_ok && matches!(ch, Ch::X | Ch::Y) => self.track(i, ch).to(t, t + d, TV::N(off), ease.clone()),
                            None => {}
                        }
                    }
                }
            }
            "set" => {
                if let Some(Target::El(i)) = self.target(e, true) {
                    for (k, v) in n.props.clone() {
                        if let Some(ch) = Ch::parse(&k) {
                            if let Some(tv) = self.tv(ch, &v.raw(), n.span) {
                                self.track(i, ch).jump(t, tv);
                            }
                        }
                    }
                }
            }
            "anim" => self.anim(e),
            "rewind" | "timemap" => self.remap_verb(e),
            "drop" | "pile" | "push" | "swing" | "shatter" => self.physics_verb(e),
            "slitscan" | "echo" | "timedisplace" | "freeze" => self.time_verb(e),
            "animate" => self.animate_verb(e),
            "pour" | "stir" | "ink" | "reveal" => self.fluid_verb(e),
            "dolly" | "orbit" => {
                let si = e.scene;
                let w = self.names.get(&format!("camera@{si}")).map(|n| n.idx).unwrap_or(self.scenes[si].world);
                let d = self.dur(n, "dur").unwrap_or(2.0);
                let ease = self.ease(n, "ease", STD);
                for (k, ch, sign) in [("x", Ch::X, -1.0), ("y", Ch::Y, -1.0), ("z", Ch::Z, 1.0), ("rotx", Ch::RotX, -1.0), ("roty", Ch::RotY, -1.0), ("zoom", Ch::Scale, 1.0)] {
                    if let Some(v) = self.num(n, k) {
                        self.track(w, ch).to(t, t + d, TV::N(v * sign), ease.clone());
                    }
                }
            }
            "activate" | "deactivate" => {
                // components (node, card, stat, steps item) carry a hidden accent state layer `<id>-active`
                if let Some(Target::El(i)) = self.target(e, true) {
                    let on = k == "activate";
                    let d = self.dur(n, "dur").unwrap_or(0.35);
                    let act = self.kids[i].iter().copied().find(|&c| self.layers[c].id.ends_with("-active"));
                    match act {
                        Some(a) => {
                            self.track(a, Ch::Opacity).to(t, t + d, TV::N(if on { 1.0 } else { 0.0 }), OUT);
                            if self.layers[i].style.glow_color.is_none() {
                                self.layers[i].style.glow_color = Some(self.th.colors["accent2"].clone());
                            }
                            self.track(i, Ch::Glow).to(t, t + d, TV::N(if on { 22.0 } else { 0.0 }), OUT);
                            self.track(i, Ch::Scale).to(t, t + d, TV::N(if on { 1.04 } else { 1.0 }), OUT);
                            if let (true, Some(hold)) = (on, self.dur(n, "for")) {
                                self.track(a, Ch::Opacity).to(t + d + hold, t + 2.0 * d + hold, TV::N(0.0), STD);
                                self.track(i, Ch::Glow).to(t + d + hold, t + 2.0 * d + hold, TV::N(0.0), STD);
                                self.track(i, Ch::Scale).to(t + d + hold, t + 2.0 * d + hold, TV::N(1.0), STD);
                            }
                        }
                        None => self.err("E_TARGET", format!("`{k}` needs a node, card or stat (they have an active state)"), "use `glow #id for=..` on other elements", n.span),
                    }
                }
            }
            "done" | "undone" => {
                // check badge state (node `-done` layer, skin part `done`); `done` also ends an active state
                if let Some(Target::El(i)) = self.target(e, true) {
                    let on = k == "done";
                    let badge = self.kids[i].iter().copied().find(|&c| self.layers[c].id.ends_with("-done") || self.layers[c].id.ends_with("-part-done"));
                    let act = self.kids[i].iter().copied().find(|&c| self.layers[c].id.ends_with("-active"));
                    match badge {
                        Some(b) => {
                            let pop = self.th.ease("pop");
                            if on {
                                self.track(b, Ch::Opacity).to(t, t + 0.12, TV::N(1.0), OUT);
                                let tr = self.track(b, Ch::Scale);
                                if tr.keys.is_empty() {
                                    tr.base = TV::N(1.0);
                                }
                                tr.from_to(t, t + 0.5, TV::N(0.4), TV::N(1.0), pop);
                            } else {
                                self.track(b, Ch::Opacity).to(t, t + 0.2, TV::N(0.0), OUT);
                            }
                            if on {
                                if let Some(a) = act {
                                    if self.track(a, Ch::Opacity).eval(t).n() > 0.01 {
                                        self.track(a, Ch::Opacity).to(t, t + 0.3, TV::N(0.0), OUT);
                                        self.track(i, Ch::Glow).to(t, t + 0.3, TV::N(0.0), OUT);
                                        self.track(i, Ch::Scale).to(t, t + 0.3, TV::N(1.0), OUT);
                                    }
                                }
                            }
                        }
                        None => self.err("E_TARGET", format!("`{k}` needs a node (or a skin with a `done` part)"), "write `done #node` on a diagram node", n.span),
                    }
                }
            }
            "advance" => {
                // progress #p: fill segments up to `to=` and count
                if let Some(Target::El(i)) = self.target(e, true) {
                    let to = self.num(n, "to").unwrap_or(1.0);
                    let mut all = vec![];
                    self.walk_idx(i, &mut all);
                    let fills: Vec<usize> = all.iter().copied().filter(|&j| self.layers[j].tags.iter().any(|x| x == "seg-fill")).collect();
                    if fills.is_empty() {
                        self.err("E_TARGET", "`advance` needs a progress component", "write `progress #p n=6` and `advance #p to=4`", n.span);
                    } else {
                        let st = 0.08;
                        let mut k2 = 0;
                        for (s_, f) in fills.iter().enumerate() {
                            let want = if (s_ as f64) < to { 1.0 } else { 0.0 };
                            let cur = self.track(*f, Ch::Sx).eval(t).n();
                            if (cur - want).abs() > 0.01 {
                                let a = t + st * k2 as f64;
                                let ease = self.th.ease("enter");
                                self.track(*f, Ch::Sx).to(a, a + 0.45, TV::N(want), ease);
                                k2 += 1;
                            }
                        }
                        let ci = self.counter_in(i);
                        if ci != i || matches!(self.layers[ci].kind, Kind::Counter { .. }) {
                            self.track(ci, Ch::Count).to(t, t + 0.4, TV::N(to), OUT);
                        }
                    }
                }
            }
            "page" => {
                // panel #det: swap to page #det.<id> (blur swap: out 0.2 s, in 0.35 s rising 16 px)
                if let Some(Target::El(i)) = self.target(e, true) {
                    let want = n.args.get(1).map(|a| a.raw().trim_start_matches('#').to_string()).or_else(|| praw(n, "to").map(|s| s.trim_start_matches('#').to_string())).unwrap_or_default();
                    let mut all = vec![];
                    self.walk_idx(i, &mut all);
                    let pages: Vec<usize> = all.iter().copied().filter(|&j| self.layers[j].tags.iter().any(|x| x == "page")).collect();
                    let target = pages.iter().copied().find(|&j| self.layers[j].id.ends_with(&format!("-page-{want}")));
                    match target {
                        Some(tp) => {
                            for p in pages {
                                if p == tp {
                                    continue;
                                }
                                if self.track(p, Ch::Opacity).eval(t).n() > 0.01 {
                                    let ex = self.th.ease("exit");
                                    self.track(p, Ch::Opacity).to(t, t + 0.2, TV::N(0.0), ex);
                                }
                            }
                            let ease = self.th.ease("enter");
                            self.fromto(tp, Ch::Opacity, TV::N(0.0), TV::N(1.0), t + 0.12, 0.3, ease.clone());
                            self.fromto(tp, Ch::Y, TV::N(16.0), TV::N(0.0), t + 0.12, 0.4, ease);
                        }
                        None => self.err("E_TARGET", format!("panel has no page `{want}`"), "write `page #panel #pageid` with an id from `page #pageid` lines inside the panel", n.span),
                    }
                }
            }
            "sfx" => {
                let name = praw(n, "src").or_else(|| n.args.first().map(|a| a.raw())).unwrap_or("click".into());
                let src = if name.contains('.') { self.asset(&name, n) } else { format!("builtin:{name}") };
                let gain = self.num(n, "gain").unwrap_or(0.6);
                self.audio.push(AudioClip { src, at: t, dur: 0.3, gain, duck: None, kind: "sfx" });
            }
            "camera" if crate::world::director_verb(n).is_some() => self.director.push(e.clone()),
            "focus" | "play" => self.warn("W_PLANNED", format!("`{k}` is planned (spec v0) and has no effect in S0"), "remove it for now", n.span),
            _ => {
                let mut all: Vec<&str> = VERBS.to_vec();
                all.push("hide");
                let hint = suggest(k, &all).map(|s| format!("did you mean `{s}`?")).unwrap_or("see docs/SPEC.md §Verbs".into());
                self.err("E_VERB", format!("`{k}` is not a verb (inside an @event only verbs are allowed)"), hint, n.span);
            }
        }
    }

    /// The counter an element shows: itself, or its first descendant Counter (stat tiles, donuts).
    pub fn counter_in(&self, i: usize) -> usize {
        if matches!(self.layers[i].kind, Kind::Counter { .. }) {
            return i;
        }
        let mut stack = vec![i];
        while let Some(c) = stack.pop() {
            if matches!(self.layers[c].kind, Kind::Counter { .. }) {
                return c;
            }
            for &k in self.kids[c].iter().rev() {
                stack.push(k);
            }
        }
        i
    }

    pub fn parent_of(&self, i: usize) -> usize {
        self.kids.iter().position(|k| k.contains(&i)).unwrap_or(i)
    }

    pub fn tv(&mut self, ch: Ch, raw: &str, span: Span) -> Option<TV> {
        if ch.is_color() {
            return self.color_s(raw, span).map(TV::C);
        }
        let s = raw.trim();
        let v = if let Some(p) = s.strip_suffix('%') {
            p.parse::<f64>().ok().map(|v| if matches!(ch, Ch::Px | Ch::Py | Ch::Opacity | Ch::Scale | Ch::Sx | Ch::Sy | Ch::TrimEnd | Ch::TrimStart | Ch::Reveal | Ch::MaskX) { v / 100.0 } else { v })
        } else if let Some(tok) = s.strip_prefix('$') {
            self.th.size(tok)
        } else {
            parse_angle_deg(s.trim_end_matches("px").trim_end_matches('x')).ok()
        };
        match v {
            Some(v) => Some(TV::N(v)),
            None => {
                self.err("E_VALUE", format!("bad value `{raw}` for channel `{}`", ch.name()), "numbers with optional unit: 12, 12px, 30deg, 50%, 1.2x", span);
                None
            }
        }
    }

    // ------------------------------------------------------------ presets
    fn fromto(&mut self, i: usize, ch: Ch, a: TV, b: TV, t: f64, d: f64, ease: Ease) {
        let tr = self.track(i, ch);
        if tr.keys.is_empty() {
            tr.base = a.clone();
        }
        tr.from_to(t, t + d, a, b, ease);
    }

    pub fn enter(&mut self, i: usize, preset: &str, t: f64, d: Option<f64>, ease: Option<Ease>, stagger: Option<f64>, n: &Node) {
        let tags = self.layers[i].tags.clone();
        let base = self.th.dur("base");
        let pop = self.th.ease("pop");
        // composite components
        if tags.iter().any(|t| t == "split") {
            let kids = self.kids[i].clone();
            let st = stagger.unwrap_or(if kids.len() > 30 { 0.03 } else { 0.07 });
            for (k, c) in kids.iter().enumerate() {
                self.enter(*c, preset, t + st * k as f64, d, ease.clone(), None, n);
            }
            return;
        }
        if tags.iter().any(|x| x.starts_with("chart-")) && matches!(preset, "draw" | "rise" | "fade" | "pop" | "grow") {
            self.chart_enter(i, t, d, stagger);
            return;
        }
        if tags.iter().any(|x| x == "terminal") && preset == "type" {
            self.terminal_type(i, t, n);
            return;
        }
        if tags.iter().any(|x| x == "code") && preset == "type" {
            let kids = self.kids[i].clone();
            let st = stagger.unwrap_or(0.18);
            for (k, c) in kids.iter().enumerate() {
                self.fromto(*c, Ch::Opacity, TV::N(0.0), TV::N(1.0), t + st * k as f64, 0.2, OUT);
                self.fromto(*c, Ch::X, TV::N(-16.0), TV::N(0.0), t + st * k as f64, 0.3, OUT);
            }
            return;
        }
        if let Some(st) = stagger {
            if !self.kids[i].is_empty() && !matches!(self.layers[i].kind, Kind::Text { .. }) {
                let kids = self.kids[i].clone();
                self.fromto(i, Ch::Opacity, TV::N(0.0), TV::N(1.0), t, 0.01, Ease::Linear);
                for (k, c) in kids.iter().enumerate() {
                    self.enter(*c, preset, t + st * k as f64, d, ease.clone(), None, n);
                }
                return;
            }
        }
        let _ = base;
        // motion tokens (design/motion-tokens.json, overridable per design: `motion enter=.. enter-dur=..`)
        let d = d.unwrap_or(match preset {
            "pop" => pop.natural_duration().unwrap_or(0.6).min(1.2),
            "draw" => 1.2,
            "wipe" | "iris" => 0.8,
            _ => self.th.durs.get("enter-dur").copied().unwrap_or(self.th.dur("enter")),
        });
        let e = ease.unwrap_or(match preset {
            "pop" => pop.clone(),
            "wipe" | "iris" | "draw" => STD,
            _ => self.th.ease("enter"),
        });
        let dist = self.th.d.enter_distance;
        let fade = |c: &mut Self, dd: f64| c.fromto(i, Ch::Opacity, TV::N(0.0), TV::N(1.0), t, dd, Ease::Cubic(0.33, 1.0, 0.68, 1.0));
        match preset {
            "fade" => fade(self, d),
            "rise" => {
                fade(self, d * 0.8);
                self.fromto(i, Ch::Y, TV::N(dist), TV::N(0.0), t, d, e);
            }
            "drop" => {
                fade(self, d * 0.8);
                self.fromto(i, Ch::Y, TV::N(-dist), TV::N(0.0), t, d, e);
            }
            "left" | "right" => {
                fade(self, d * 0.8);
                let x = if preset == "left" { -dist * 2.5 } else { dist * 2.5 };
                self.fromto(i, Ch::X, TV::N(x), TV::N(0.0), t, d, e);
            }
            "pop" => {
                fade(self, 0.15);
                self.fromto(i, Ch::Scale, TV::N(0.6), TV::N(1.0), t, d, e);
            }
            "zoom" => {
                fade(self, d * 0.8);
                self.fromto(i, Ch::Scale, TV::N(1.25), TV::N(1.0), t, d, e);
            }
            "blur" => {
                fade(self, d * 0.8);
                self.fromto(i, Ch::Blur, TV::N(24.0), TV::N(0.0), t, d, e);
            }
            "flip" => {
                fade(self, d * 0.5);
                self.fromto(i, Ch::RotX, TV::N(-90.0), TV::N(0.0), t, d, e);
            }
            "wipe" | "iris" => {
                if self.layers[i].style.clip.is_none() {
                    self.layers[i].style.clip = Some(if preset == "wipe" { ClipShape::Wipe(praw(n, "from").unwrap_or("left".into())) } else { ClipShape::Iris(0.5, 0.5) });
                }
                self.fromto(i, Ch::Reveal, TV::N(0.0), TV::N(1.0), t, d, e);
            }
            "type" => {
                let chars = match &self.layers[i].kind {
                    Kind::Text { runs } => runs.iter().map(|r| r.text.chars().count()).sum::<usize>(),
                    _ => 20,
                };
                let d = self.dur(n, "dur").unwrap_or(chars as f64 * 0.045);
                self.layers[i].style.clip = Some(ClipShape::Wipe("left".into()));
                self.fromto(i, Ch::Reveal, TV::N(0.0), TV::N(1.0), t, d, Ease::Steps(chars.max(1) as u32));
            }
            "draw" => {
                // drawing something that was hidden makes it visible again
                if self.track(i, Ch::Opacity).eval(t).n() < 0.5 {
                    self.track(i, Ch::Opacity).jump(t, TV::N(1.0));
                }
                let kids = self.kids[i].clone();
                if matches!(self.layers[i].kind, Kind::Path { .. } | Kind::Rect | Kind::Ellipse) {
                    self.fromto(i, Ch::TrimEnd, TV::N(0.0), TV::N(1.0), t, d, e);
                } else {
                    for (k, c) in kids.iter().enumerate() {
                        self.enter(*c, "draw", t + 0.1 * k as f64, Some(d), Some(e.clone()), None, n);
                    }
                }
            }
            "count" => {
                let to = self.num(n, "to").or_else(|| self.num(n, "value"));
                fade(self, 0.2);
                if let Some(to) = to {
                    let ci = self.counter_in(i);
                    let from = self.track(ci, Ch::Count).base.clone();
                    let d = self.dur(n, "dur").unwrap_or(1.4);
                    self.fromto(ci, Ch::Count, from, TV::N(to), t, d, OUT);
                }
            }
            "none" | "show" => {
                let tr = self.track(i, Ch::Opacity);
                if tr.keys.is_empty() {
                    tr.base = TV::N(0.0);
                }
                tr.jump(t, TV::N(1.0));
            }
            other => {
                let hint = suggest(other, ENTER).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("presets: {}", ENTER.join(", ")));
                self.err("E_PRESET", format!("unknown enter preset `{other}`"), hint, n.span);
            }
        }
    }

    pub fn exit(&mut self, i: usize, preset: &str, t: f64, d: f64) {
        // exits accelerate (motion tokens: in-quad family), and are shorter than entrances
        let e = self.th.ease("exit");
        match preset {
            "none" => {
                self.track(i, Ch::Opacity).jump(t, TV::N(0.0));
                return;
            }
            "rise" => self.track(i, Ch::Y).to(t, t + d, TV::N(-40.0), e.clone()),
            "drop" => self.track(i, Ch::Y).to(t, t + d, TV::N(40.0), e.clone()),
            "left" => self.track(i, Ch::X).to(t, t + d, TV::N(-90.0), e.clone()),
            "right" => self.track(i, Ch::X).to(t, t + d, TV::N(90.0), e.clone()),
            "pop" | "zoom" => self.track(i, Ch::Scale).to(t, t + d, TV::N(if preset == "pop" { 0.6 } else { 1.2 }), e.clone()),
            "blur" => self.track(i, Ch::Blur).to(t, t + d, TV::N(24.0), e.clone()),
            "wipe" => {
                if self.layers[i].style.clip.is_none() {
                    self.layers[i].style.clip = Some(ClipShape::Wipe("left".into()));
                }
                self.track(i, Ch::Reveal).to(t, t + d, TV::N(0.0), STD);
                return;
            }
            _ => {}
        }
        self.track(i, Ch::Opacity).to(t, t + d, TV::N(0.0), e);
    }

    pub fn emph(&mut self, i: usize, preset: &str, t: f64, n: &Node) {
        let d = self.dur(n, "dur").unwrap_or(0.6);
        match preset {
            "pulse" => {
                let s = self.num(n, "scale").unwrap_or(1.08);
                let tr = self.track(i, Ch::Scale);
                let cur = tr.eval(t).n();
                tr.to(t, t + d * 0.4, TV::N(cur * s), OUT);
                tr.to(t + d * 0.4, t + d, TV::N(cur), STD);
            }
            "shake" => {
                let tr = self.track(i, Ch::X);
                let x0 = tr.eval(t).n();
                for (k, dx) in [12.0, -10.0, 8.0, -5.0, 0.0].iter().enumerate() {
                    let a = t + d * k as f64 / 5.0;
                    tr.to(a, a + d / 5.0, TV::N(x0 + dx), Ease::Cubic(0.37, 0.0, 0.63, 1.0));
                }
            }
            "glow" | "highlight" | "spotlight" => {
                if self.layers[i].style.glow_color.is_none() {
                    self.layers[i].style.glow_color = Some(self.color(n, "color").unwrap_or_else(|| self.th.colors["accent2"].clone()));
                }
                let hold = self.dur(n, "for").unwrap_or(1.2);
                let tr = self.track(i, Ch::Glow);
                tr.to(t, t + 0.3, TV::N(self_glow(n)), OUT);
                tr.to(t + 0.3 + hold, t + 0.6 + hold, TV::N(0.0), STD);
            }
            "bounce" => {
                let tr = self.track(i, Ch::Y);
                let y0 = tr.eval(t).n();
                tr.to(t, t + d * 0.35, TV::N(y0 - 24.0), OUT);
                tr.to(t + d * 0.35, t + d * 1.4, TV::N(y0), Ease::Bounce { bounces: 2 });
            }
            "wobble" => {
                let tr = self.track(i, Ch::Rot);
                for (k, r) in [6.0, -5.0, 3.0, 0.0].iter().enumerate() {
                    let a = t + d * k as f64 / 4.0;
                    tr.to(a, a + d / 4.0, TV::N(*r), STD);
                }
            }
            "flash" => {
                // a white glow burst + tiny pop (composite/effect channels: same in both backends)
                if self.layers[i].style.glow_color.is_none() {
                    self.layers[i].style.glow_color = Some(crate::theme::hex("#ffffffcc"));
                }
                let tr = self.track(i, Ch::Glow);
                tr.to(t, t + d * 0.3, TV::N(26.0), OUT);
                tr.to(t + d * 0.3, t + d, TV::N(0.0), STD);
                let tr = self.track(i, Ch::Scale);
                let s0 = tr.eval(t).n();
                tr.to(t, t + d * 0.3, TV::N(s0 * 1.04), OUT);
                tr.to(t + d * 0.3, t + d, TV::N(s0), STD);
            }
            other => {
                let hint = suggest(other, EMPH).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("emphasis presets: {}", EMPH.join(", ")));
                self.err("E_PRESET", format!("unknown emphasis `{other}`"), hint, n.span);
            }
        }
    }

    /// Chart build in one orchestrated sequence: the chart is hidden until its enter; frame and
    /// gridlines fade in first, bars grow from the baseline (value labels ride the bar top and count
    /// in sync, same ease), lines draw on with their points popping when the stroke reaches them.
    fn chart_enter(&mut self, i: usize, t: f64, d: Option<f64>, stagger: Option<f64>) {
        let mut all = vec![];
        self.walk_idx(i, &mut all);
        let st = stagger.unwrap_or(0.12);
        let d = d.unwrap_or(0.9);
        let grow = Ease::Cubic(0.16, 1.0, 0.3, 1.0);
        self.fromto(i, Ch::Opacity, TV::N(0.0), TV::N(1.0), t, 0.01, Ease::Linear);
        // chart frame (title, kicker, source around the chart): its texts arrive with the chart
        let fr = self.parent_of(i);
        if fr != i && self.layers[fr].tags.iter().any(|x| x == "chart-frame") {
            for c in self.kids[fr].clone() {
                if c != i && self.track(c, Ch::Opacity).keys.is_empty() {
                    self.fromto(c, Ch::Opacity, TV::N(0.0), TV::N(1.0), t, 0.35, OUT);
                }
            }
        }
        let (mut kb, mut kd, mut ks) = (0usize, 0usize, 0usize);
        let npts = all.iter().filter(|&&j| self.layers[j].tags.iter().any(|x| x == "dot") && !self.layers[j].tags.iter().any(|x| x == "text")).count().max(1);
        // Axes first, data right behind (motion tokens): axes and gridlines fade over AXES_D; the
        // line/area draw-on starts one stagger item after they finish and uses the enter ease
        // (out-cubic), so the stroke moves from its first frame instead of idling through the slow
        // half of an in-out curve. The area reveal (left wipe) runs on the same clock.
        let line_at = t + AXES_D + STAGGER_ITEM;
        let dl = d * 1.8;
        let line_ease = OUT;
        // time (after line_at) at which the eased draw-on reaches arc fraction f (bisection)
        let reach = |f: f64| -> f64 {
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for _ in 0..40 {
                let m = (lo + hi) / 2.0;
                if line_ease.sample(m) < f { lo = m } else { hi = m }
            }
            (lo + hi) / 2.0 * dl
        };
        for j in all {
            if j == i {
                continue;
            }
            let tags = self.layers[j].tags.clone();
            let ride = tags.iter().find_map(|x| x.strip_prefix("ride:")).and_then(|v| v.parse::<f64>().ok());
            let col = tags.iter().find_map(|x| x.strip_prefix("col:")).and_then(|v| v.parse::<usize>().ok());
            if tags.iter().any(|x| x == "grid") {
                self.fromto(j, Ch::Opacity, TV::N(0.0), TV::N(1.0), t, AXES_D, OUT);
            } else if tags.iter().any(|x| x == "bar") {
                let a = t + 0.1 + st * kb as f64;
                self.fromto(j, Ch::Sy, TV::N(0.0), TV::N(1.0), a, d, grow.clone());
                kb += 1;
            } else if let (Some(bh), Some(k), Kind::Counter { .. }) = (ride, col, &self.layers[j].kind) {
                // rides the bar top: offset = bar height x (1 - sy), same ease and timing as the bar
                let a = t + 0.1 + st * k as f64;
                let target = self.track(j, Ch::Count).base.clone();
                self.fromto(j, Ch::Count, TV::N(0.0), target, a, d, grow.clone());
                self.fromto(j, Ch::Y, TV::N(bh), TV::N(0.0), a, d, grow.clone());
                self.fromto(j, Ch::Opacity, TV::N(0.0), TV::N(1.0), a, 0.15, OUT);
            } else if matches!(self.layers[j].kind, Kind::Counter { .. }) {
                let a = t + 0.1;
                let target = self.track(j, Ch::Count).base.clone();
                self.fromto(j, Ch::Count, TV::N(0.0), target, a, d, grow.clone());
                self.fromto(j, Ch::Opacity, TV::N(0.0), TV::N(1.0), a, 0.2, OUT);
            } else if tags.iter().any(|x| x == "line") {
                self.fromto(j, Ch::TrimEnd, TV::N(0.0), TV::N(1.0), line_at, dl, line_ease.clone());
            } else if tags.iter().any(|x| x == "area") {
                self.fromto(j, Ch::Reveal, TV::N(0.0), TV::N(1.0), line_at, dl, line_ease.clone());
            } else if tags.iter().any(|x| x == "dot") {
                // markers pop when the draw-on has reached them: the k-th point sits at arc fraction
                // k/(n-1); the first one waits for a short stub (DOT_LEAD) so no marker is ever
                // visible while the trimmed path has zero length
                let is_label = tags.iter().any(|x| x == "text");
                let k = if is_label { kd.saturating_sub(1) } else { kd };
                let f = (k as f64 / (npts.max(2) - 1) as f64).max(DOT_LEAD);
                let a = line_at + reach(f);
                if is_label {
                    self.fromto(j, Ch::Opacity, TV::N(0.0), TV::N(1.0), a + 0.05, 0.25, OUT);
                } else {
                    self.fromto(j, Ch::Scale, TV::N(0.0), TV::N(1.0), a, 0.5, self.th.ease("pop"));
                    kd += 1;
                }
            } else if tags.iter().any(|x| x == "slice") {
                let a = t + st * 2.0 * ks as f64;
                self.fromto(j, Ch::TrimEnd, TV::N(0.0), TV::N(1.0), a, d, STD);
                ks += 1;
            } else if tags.iter().any(|x| x == "text") {
                self.fromto(j, Ch::Opacity, TV::N(0.0), TV::N(1.0), t + 0.15, 0.4, OUT);
            }
        }
    }

    pub fn walk_idx(&self, i: usize, out: &mut Vec<usize>) {
        out.push(i);
        for &k in &self.kids[i] {
            self.walk_idx(k, out);
        }
    }

    fn terminal_type(&mut self, i: usize, t: f64, n: &Node) {
        let speed = self.dur(n, "cps").map(|v| 1.0 / v).unwrap_or(0.045);
        let mut at = t;
        for c in self.kids[i].clone() {
            let tags = self.layers[c].tags.clone();
            if !tags.iter().any(|x| x == "cmd" || x == "out") {
                continue;
            }
            let chars = match &self.layers[c].kind {
                Kind::Text { runs } => runs.iter().map(|r| r.text.chars().count()).sum::<usize>(),
                _ => 10,
            };
            if tags.iter().any(|x| x == "cmd") {
                self.layers[c].style.clip = Some(ClipShape::Wipe("left".into()));
                self.fromto(c, Ch::Reveal, TV::N(0.0), TV::N(1.0), at, chars as f64 * speed, Ease::Steps(chars.max(1) as u32));
                at += chars as f64 * speed + 0.35;
            } else {
                let tr = self.track(c, Ch::Opacity);
                tr.base = TV::N(0.0);
                tr.jump(at, TV::N(1.0));
                at += 0.12;
            }
        }
    }

    fn diff(&mut self, i: usize, t: f64, n: &Node) {
        let d = self.dur(n, "dur").unwrap_or(0.6);
        let red = hex("#ff6b6b55");
        let green = hex("#3cbf8740");
        for c in self.kids[i].clone() {
            let tags = self.layers[c].tags.clone();
            if tags.iter().any(|x| x == "del") {
                self.track(c, Ch::Fill).base = TV::C(Rgba { a: 0.0, ..red.clone() });
                self.track(c, Ch::Fill).to(t, t + 0.25, TV::C(red.clone()), OUT);
                self.track(c, Ch::Opacity).to(t + 0.45, t + 0.45 + d, TV::N(0.0), STD);
                self.track(c, Ch::LineH).to(t + 0.55, t + 0.55 + d, TV::N(0.0), STD);
            } else if tags.iter().any(|x| x == "add") {
                self.track(c, Ch::Fill).base = TV::C(Rgba { a: 0.0, ..green.clone() });
                self.track(c, Ch::LineH).to(t + 0.55, t + 0.55 + d, TV::N(1.0), STD);
                self.track(c, Ch::Opacity).to(t + 0.75, t + 0.75 + d, TV::N(1.0), STD);
                self.track(c, Ch::Fill).to(t + 0.75, t + 1.0 + d, TV::C(green.clone()), OUT);
            }
        }
    }

    fn burst(&mut self, gi: usize, t: f64, e: &Ev) {
        // particles element props live on the element node; find it in the document via the event's id
        let n = &e.node;
        let src = self.particle_cfg.get(&self.lid(gi)).cloned().unwrap_or_default();
        let get = |k: &str, d: f64| -> f64 { n.props.get(k).or_else(|| src.props.get(k)).and_then(|v| v.raw().trim_end_matches("px").trim_end_matches('s').parse().ok()).unwrap_or(d) };
        let count = get("count", 40.0) as usize;
        let seed = get("seed", 1.0) as u64;
        let speed = get("speed", 900.0);
        let spread = get("spread", 360.0);
        let angle = get("angle", -90.0);
        let gravity = get("gravity", 1400.0);
        let life = get("life", 1.4);
        let size = get("size", 14.0);
        let xf = n.props.get("x").or_else(|| src.props.get("x")).map(|v| v.raw()).unwrap_or("50%".into());
        let yf = n.props.get("y").or_else(|| src.props.get("y")).map(|v| v.raw()).unwrap_or("50%".into());
        let pos = |s: &str, full: f64| if let Some(p) = s.strip_suffix('%') { p.parse::<f64>().unwrap_or(50.0) / 100.0 * full } else { s.trim_end_matches("px").parse().unwrap_or(full / 2.0) };
        let (ox, oy) = (pos(&xf, self.w), pos(&yf, self.h));
        let shape = n.props.get("shape").or_else(|| src.props.get("shape")).map(|v| v.raw()).unwrap_or("confetti".into());
        let colors: Vec<Rgba> = match n.props.get("colors").or_else(|| src.props.get("colors")) {
            Some(Value::List(v)) => v.iter().filter_map(|c| self.color_s(&c.raw(), n.span)).collect(),
            _ => ["accent2", "mint", "warn", "text"].iter().map(|k| self.th.colors[*k].clone()).collect(),
        };
        let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) ^ 0xD1B54A32D192ED03);
        for p in 0..count {
            let a = (angle + (rng.f() - 0.5) * spread).to_radians();
            let v = speed * (0.45 + 0.55 * rng.f());
            let (vx, vy) = (v * a.cos(), v * a.sin());
            let l = life * (0.7 + 0.3 * rng.f());
            let s = size * (0.6 + 0.8 * rng.f());
            let kind = match shape.as_str() {
                "dot" => Kind::Ellipse,
                _ => Kind::Rect,
            };
            let mut layer = Layer::new(format!("{}-p{p}", self.lid(gi)), kind);
            layer.layout.pos = Pos::Abs;
            layer.layout.x = Some(Len::px(ox));
            layer.layout.y = Some(Len::px(oy));
            layer.layout.anchor = (0.5, 0.5);
            layer.layout.w = Some(Len::px(s));
            layer.layout.h = Some(Len::px(if shape == "confetti" { s * 0.5 } else { s }));
            layer.style.fill = Some(Paint::Solid(ColorRef::Lit(colors[p % colors.len().max(1)].clone())));
            layer.style.radius = if shape == "square" { 2.0 } else { 1.5 };
            let li = self.add(Some(gi), layer);
            self.set_base(li, Ch::Opacity, TV::N(0.0));
            self.track(li, Ch::Opacity).jump(t, TV::N(1.0));
            self.track(li, Ch::X).to(t, t + l, TV::N(vx * l * 0.8), Ease::Cubic(0.25, 0.6, 0.5, 1.0));
            // y(t) = vy t + g t^2 / 2, sampled (exact projectile shape, piecewise linear)
            let steps = 8;
            for k in 1..=steps {
                let tt = l * k as f64 / steps as f64;
                let y = vy * tt * 0.8 + 0.5 * gravity * tt * tt;
                let tr = self.track(li, Ch::Y);
                let t0 = t + l * (k - 1) as f64 / steps as f64;
                tr.to(t0, t + tt, TV::N(y), Ease::Linear);
            }
            if shape == "confetti" || shape == "square" {
                let r = (rng.f() - 0.5) * 1440.0;
                self.track(li, Ch::Rot).to(t, t + l, TV::N(r), Ease::Linear);
                let rx = (rng.f() - 0.5) * 1080.0;
                self.track(li, Ch::RotX).to(t, t + l, TV::N(rx), Ease::Linear);
            }
            self.track(li, Ch::Opacity).to(t + l * 0.65, t + l, TV::N(0.0), Ease::Linear);
        }
    }

    // ------------------------------------------------------------ screenshots: zoom, cursor, marks
    fn browser_for(&mut self, e: &Ev) -> Option<(usize, Option<[f64; 4]>)> {
        match self.target(e, false) {
            Some(Target::Hot(b, r)) => Some((b, Some(r))),
            Some(Target::Browser(b)) => Some((b, None)),
            _ => self.browsers.iter().position(|b| b.scene == e.scene).map(|b| (b, None)),
        }
    }

    fn zoom(&mut self, e: &Ev) {
        let n = &e.node;
        let t = e.t;
        let reset = n.args.iter().any(|a| matches!(a.as_str(), Some("out") | Some("reset"))) || n.args.iter().any(|a| a == &Value::Num(1.0));
        // zoom to a laid-out element (S1: native layout geometry): scene camera centers it
        if let Some(name) = id_arg(n) {
            if let Some(Target::El(_)) = self.target(e, false) {
                if self.browsers.iter().all(|b| !(b.scene == e.scene && b.name == name)) {
                    self.zoom_element(e, &name, reset);
                    return;
                }
            }
        }
        let Some((b, rect)) = self.browser_for(e) else {
            // scene camera zoom (no screenshot)
            let si = e.scene;
            let w = self.names.get(&format!("camera@{si}")).map(|n| n.idx).unwrap_or(self.scenes[si].world);
            let z = if reset { 1.0 } else { self.num(n, "z").unwrap_or(1.3) };
            let d = self.dur(n, "dur").unwrap_or(1.0);
            let ease = self.ease(n, "ease", STD);
            if id_arg(n).is_some() {
                self.warn("W_ZOOM_LAYOUT", "zooming to a laid-out element zooms the scene center in S0", "use zoom on screenshot hotspots, or move/scale the element", n.span);
            }
            self.track(w, Ch::Scale).to(t, t + d, TV::N(z), ease.clone());
            if reset {
                // undo a `zoom #element` pan as well
                for ch in [Ch::X, Ch::Y] {
                    if self.tracks.contains_key(&(self.lid(w), ch)) {
                        self.track(w, ch).to(t, t + d, TV::N(0.0), ease.clone());
                    }
                }
            }
            return;
        };
        let (sw, sh) = (self.browsers[b].shot_w, self.browsers[b].shot_h);
        let d = self.dur(n, "dur").unwrap_or(1.0);
        let ease = self.ease(n, "ease", STD);
        let cam = self.browsers[b].cam;
        let (mut cx, mut cy, mut z) = (0.5, 0.5, 1.0);
        if !reset {
            if let Some(r) = rect {
                cx = (r[0] + r[2] / 2.0) / sw;
                cy = (r[1] + r[3] / 2.0) / sh;
                let cr = self.browsers[b].crop;
                let fit = 0.8 / (r[2] / cr[2]).max(r[3] / cr[3]);
                z = self.num(n, "z").unwrap_or(fit.clamp(1.2, 1.9));
            } else {
                z = self.num(n, "z").unwrap_or(1.6);
            }
            if let Some(x) = self.num(n, "x") {
                cx = if x > 1.0 { x / 100.0 } else { x };
            }
            if let Some(y) = self.num(n, "y") {
                cy = if y > 1.0 { y / 100.0 } else { y };
            }
            if let Some(dx) = self.num(n, "dx") {
                cx += dx / sw;
            }
            if let Some(dy) = self.num(n, "dy") {
                cy += dy / sh;
            }
        }
        if n.kind == "pan" {
            z = self.track(cam, Ch::Scale).eval(t).n();
        }
        // viewport coords: p_v = B + A * p_shot (A = shot/crop size, B = -crop offset); keep the shot covering the viewport
        let cr = self.browsers[b].crop;
        let (ax, ay, bx, by) = (sw / cr[2], sh / cr[3], -cr[0] / cr[2], -cr[1] / cr[3]);
        let fit_axis = |c: f64, a: f64, bb: f64| -> f64 {
            let vc = bb + a * c;
            let t = (0.5 - vc * z).clamp(1.0 - z * (bb + a), -z * bb);
            (z * bb + t - bb) / a
        };
        let tx = fit_axis(cx, ax, bx);
        let ty = fit_axis(cy, ay, by);
        self.track(cam, Ch::Scale).to(t, t + d, TV::N(z), ease.clone());
        self.track(cam, Ch::Px).to(t, t + d, TV::N(tx), ease.clone());
        self.track(cam, Ch::Py).to(t, t + d, TV::N(ty), ease);
    }

    /// `zoom #el` on a laid-out element: the scene world scales about the frame center and
    /// translates so the element's center lands in the middle (z defaults to a fit: 60 % of the frame).
    fn zoom_element(&mut self, e: &Ev, name: &str, reset: bool) {
        let n = &e.node;
        let t = e.t;
        let si = e.scene;
        let w = self.names.get(&format!("camera@{si}")).map(|n| n.idx).unwrap_or(self.scenes[si].world);
        let d = self.dur(n, "dur").unwrap_or(1.0);
        let ease = self.ease(n, "ease", STD);
        let Some(g) = self.geo.clone() else {
            self.need_geo = true;
            return;
        };
        let (mut z, mut x, mut y) = (1.0, 0.0, 0.0);
        if !reset {
            let Some(r) = g.rects.get(&format!("{name}@{si}")).copied() else {
                self.err("E_ID", format!("zoom #{name}: element has no layout box"), "zoom to a visible element of this scene", n.span);
                return;
            };
            let fit = (0.6 * self.w / r[2].max(1.0)).min(0.6 * self.h / r[3].max(1.0));
            z = self.num(n, "z").unwrap_or(fit.clamp(1.1, 3.0));
            let (cx, cy) = (r[0] + r[2] / 2.0 + self.num(n, "dx").unwrap_or(0.0), r[1] + r[3] / 2.0 + self.num(n, "dy").unwrap_or(0.0));
            x = -z * (cx - self.w / 2.0);
            y = -z * (cy - self.h / 2.0);
        }
        self.track(w, Ch::Scale).to(t, t + d, TV::N(z), ease.clone());
        self.track(w, Ch::X).to(t, t + d, TV::N(x), ease.clone());
        self.track(w, Ch::Y).to(t, t + d, TV::N(y), ease);
    }

    fn ensure_cursor(&mut self, b: usize, t: f64) -> (usize, usize) {
        if let Some(c) = self.browsers[b].cursor {
            return c;
        }
        let cam = self.browsers[b].cam;
        let mut w = Layer::new(format!("{}-cursor", self.lid(cam)), Kind::Group);
        w.layout.pos = Pos::Fill;
        w.tags.push("cursor".into());
        let wi = self.add(Some(cam), w);
        let markup = "<svg viewBox='0 0 22 28' width='100%' height='100%'><path d='M2 2 L2 22 L7.5 17 L11 25.5 L14.5 24 L11 15.8 L18.5 15.8 Z' fill='#15181d' stroke='#fff' stroke-width='1.6' stroke-linejoin='round'/></svg>".to_string();
        let mut a = Layer::new(format!("{}-arrow", self.lid(cam)), Kind::Svg { markup });
        a.layout.pos = Pos::Abs;
        a.layout.x = Some(Len::px(-2.0));
        a.layout.y = Some(Len::px(-2.0));
        a.layout.w = Some(Len::pct(1.9));
        a.layout.aspect = Some(22.0 / 28.0);
        a.style.origin = (0.0, 0.0);
        a.style.shadows.push(Shadow { y: 2.0, blur: 3.0, color: Some(hex("#00000059")), ..Default::default() });
        let ai = self.add(Some(wi), a);
        let (x, y) = self.browsers[b].cur_pos;
        self.set_base(wi, Ch::Px, TV::N(x));
        self.set_base(wi, Ch::Py, TV::N(y));
        self.set_base(wi, Ch::Opacity, TV::N(0.0));
        self.track(wi, Ch::Opacity).to(t - 0.2, t, TV::N(1.0), OUT);
        self.browsers[b].cursor = Some((wi, ai));
        (wi, ai)
    }

    fn cursor_verb(&mut self, e: &Ev) {
        let n = &e.node;
        let t = e.t;
        let Some((b, rect)) = self.browser_for(e) else {
            self.warn("W_CURSOR", "cursor verbs need a browser/phone screenshot in the scene", "add `browser shot=...` to this scene", n.span);
            return;
        };
        let (sw, sh) = (self.browsers[b].shot_w, self.browsers[b].shot_h);
        let mut target = match rect {
            Some(r) => ((r[0] + r[2] / 2.0) / sw, (r[1] + r[3] / 2.0) / sh),
            None => self.browsers[b].cur_pos,
        };
        if let (Some(x), Some(y)) = (self.num(n, "x"), self.num(n, "y")) {
            target = (x / sw, y / sh);
        }
        target.0 += self.num(n, "dx").unwrap_or(0.0) / sw;
        target.1 += self.num(n, "dy").unwrap_or(0.0) / sh;
        let (wi, ai) = self.ensure_cursor(b, t);
        let md = self.dur(n, "dur").unwrap_or(0.6);
        let ease = self.ease(n, "ease", STD);
        let last = self.tracks.get(&(self.lid(wi), Ch::Px)).and_then(|tr| tr.keys.last().map(|k| k.t)).unwrap_or(0.0);
        let clicky = matches!(n.kind.as_str(), "click" | "dblclick");
        // clicks land ON the anchor: travel before it; plain cursor moves start at it
        let (m0, m1) = if clicky { ((t - md).max(last), t) } else { (t, t + md) };
        if (target.0 - self.browsers[b].cur_pos.0).abs() + (target.1 - self.browsers[b].cur_pos.1).abs() > 1e-4 {
            self.track(wi, Ch::Px).to(m0, m1.max(m0 + 0.05), TV::N(target.0), ease.clone());
            self.track(wi, Ch::Py).to(m0, m1.max(m0 + 0.05), TV::N(target.1), ease.clone());
        }
        self.browsers[b].cur_pos = target;
        let press = |c: &mut Self, at: f64| {
            let tr = c.track(ai, Ch::Scale);
            tr.to(at, at + 0.05, TV::N(0.85), Ease::Linear);
            tr.to(at + 0.05, at + 0.2, TV::N(1.0), OUT);
        };
        match n.kind.as_str() {
            "click" | "dblclick" => {
                press(self, t);
                self.ripple(b, target, t);
                if n.kind == "dblclick" {
                    press(self, t + 0.25);
                    self.ripple(b, target, t + 0.25);
                }
                if flag(n, "sfx") {
                    self.audio.push(AudioClip { src: "builtin:click".into(), at: t, dur: 0.1, gain: 0.5, duck: None, kind: "sfx" });
                }
            }
            "drag" => {
                let dx = self.num(n, "dx").unwrap_or(0.0) / sw;
                let dy = self.num(n, "dy").unwrap_or(0.0) / sh;
                // undo the dx/dy that was applied to the grab point
                let grab = (target.0 - dx, target.1 - dy);
                let dd = self.dur(n, "dur").unwrap_or(1.6);
                self.track(wi, Ch::Px).to(m0, m1, TV::N(grab.0), ease.clone());
                self.track(wi, Ch::Py).to(m0, m1, TV::N(grab.1), ease.clone());
                let tr = self.track(ai, Ch::Scale);
                tr.to(m1, m1 + 0.08, TV::N(0.85), Ease::Linear);
                self.track(wi, Ch::Px).to(m1 + 0.1, m1 + 0.1 + dd, TV::N(grab.0 + dx), STD);
                self.track(wi, Ch::Py).to(m1 + 0.1, m1 + 0.1 + dd, TV::N(grab.1 + dy), STD);
                self.track(ai, Ch::Scale).to(m1 + 0.1 + dd, m1 + 0.25 + dd, TV::N(1.0), OUT);
                self.browsers[b].cur_pos = (grab.0 + dx, grab.1 + dy);
            }
            _ => {}
        }
    }

    fn ripple(&mut self, b: usize, p: (f64, f64), t: f64) {
        let marks = self.browsers[b].marks;
        let mut r = Layer::new("", Kind::Ellipse);
        r.layout.pos = Pos::Abs;
        r.layout.x = Some(Len::pct(p.0 * 100.0));
        r.layout.y = Some(Len::pct(p.1 * 100.0));
        r.layout.anchor = (0.5, 0.5);
        r.layout.w = Some(Len::pct(2.8));
        r.layout.aspect = Some(1.0);
        r.style.border = Some((3.0, hex("#17845ab3")));
        let ri = self.add(Some(marks), r);
        self.set_base(ri, Ch::Opacity, TV::N(0.0));
        self.track(ri, Ch::Opacity).from_to(t, t + 0.47, TV::N(1.0), TV::N(0.0), Ease::Linear);
        self.track(ri, Ch::Scale).from_to(t, t + 0.47, TV::N(0.4), TV::N(1.4), Ease::Linear);
    }

    fn rect_layer(&mut self, b: usize, r: [f64; 4], pad: f64) -> Layer {
        let (sw, sh) = (self.browsers[b].shot_w, self.browsers[b].shot_h);
        let mut l = Layer::new("", Kind::Rect);
        l.layout.pos = Pos::Abs;
        l.layout.x = Some(Len::pct((r[0] - pad) / sw * 100.0));
        l.layout.y = Some(Len::pct((r[1] - pad) / sh * 100.0));
        l.layout.w = Some(Len::pct((r[2] + 2.0 * pad) / sw * 100.0));
        l.layout.h = Some(Len::pct((r[3] + 2.0 * pad) / sh * 100.0));
        l
    }

    fn mark(&mut self, b: usize, r: [f64; 4], t: f64, n: &Node, kind: &str) {
        let marks = self.browsers[b].marks;
        let scale = self.browsers[b].view_w / self.browsers[b].shot_w;
        let hold = self.dur(n, "for").unwrap_or(1.6);
        let col = self.color(n, "color").unwrap_or_else(|| self.th.colors["accent2"].clone());
        match kind {
            "pulse" | "glow" => {
                for k in 0..2 {
                    let mut l = self.rect_layer(b, r, 4.0);
                    l.style.border = Some((3.0, col.clone()));
                    l.style.radius = 10.0 / scale;
                    let li = self.add(Some(marks), l);
                    let a = t + 0.45 * k as f64;
                    self.set_base(li, Ch::Opacity, TV::N(0.0));
                    self.track(li, Ch::Opacity).from_to(a, a + 0.7, TV::N(1.0), TV::N(0.0), Ease::Cubic(0.5, 0.0, 0.75, 0.0));
                    self.track(li, Ch::Scale).from_to(a, a + 0.7, TV::N(1.0), TV::N(1.35), OUT);
                }
            }
            "highlight" => {
                let mut l = self.rect_layer(b, r, 6.0);
                l.style.border = Some((3.0, col.clone()));
                l.style.radius = 10.0;
                l.style.fill = Some(Paint::Solid(ColorRef::Lit(Rgba { a: 0.12, ..col.clone() })));
                l.style.glow_color = Some(col.clone());
                let li = self.add(Some(marks), l);
                self.set_base(li, Ch::Opacity, TV::N(0.0));
                self.track(li, Ch::Opacity).to(t, t + 0.25, TV::N(1.0), OUT);
                self.track(li, Ch::Opacity).to(t + 0.25 + hold, t + 0.55 + hold, TV::N(0.0), STD);
                self.track(li, Ch::Glow).from_to(t, t + 0.4, TV::N(0.0), TV::N(14.0), OUT);
            }
            "spotlight" => {
                let mut l = self.rect_layer(b, r, 8.0);
                l.style.radius = 12.0;
                l.style.shadows.push(Shadow { spread: 4000.0, color: Some(hex("#00000073")), ..Default::default() });
                let li = self.add(Some(marks), l);
                self.set_base(li, Ch::Opacity, TV::N(0.0));
                self.track(li, Ch::Opacity).to(t, t + 0.35, TV::N(1.0), OUT);
                self.track(li, Ch::Opacity).to(t + 0.35 + hold, t + 0.7 + hold, TV::N(0.0), STD);
            }
            "callout" => {
                let text = text_arg(n).unwrap_or_default();
                let side = praw(n, "side").unwrap_or("right".into());
                let (sw, sh) = (self.browsers[b].shot_w, self.browsers[b].shot_h);
                let (x, y, anchor) = match side.as_str() {
                    "left" => (r[0] - 16.0, r[1] + r[3] / 2.0, (1.0, 0.5)),
                    "top" => (r[0] + r[2] / 2.0, r[1] - 16.0, (0.5, 1.0)),
                    "bottom" => (r[0] + r[2] / 2.0, r[1] + r[3] + 16.0, (0.5, 0.0)),
                    _ => (r[0] + r[2] + 16.0, r[1] + r[3] / 2.0, (0.0, 0.5)),
                };
                let mut l = Layer::new("", Kind::Text { runs: self.runs(&text) });
                l.layout.pos = Pos::Abs;
                l.layout.x = Some(Len::pct(x / sw * 100.0));
                l.layout.y = Some(Len::pct(y / sh * 100.0));
                l.layout.anchor = anchor;
                l.layout.pad = [7.0, 12.0, 7.0, 12.0];
                l.style.size = Some(self.num(n, "size").unwrap_or(14.0));
                l.style.color = Some(hex("#ffffff"));
                l.style.fill = Some(Paint::Solid(ColorRef::Lit(hex("#15181d"))));
                l.style.radius = 8.0;
                l.style.nowrap = true;
                l.style.shadows.push(Shadow { y: 10.0, blur: 30.0, color: Some(hex("#00000040")), ..Default::default() });
                l.tags.push("text".into());
                l.tags.push("callout".into());
                let li = self.add(Some(marks), l);
                self.set_base(li, Ch::Opacity, TV::N(0.0));
                self.track(li, Ch::Opacity).to(t, t + 0.2, TV::N(1.0), OUT);
                self.track(li, Ch::Y).from_to(t, t + 0.3, TV::N(6.0), TV::N(0.0), OUT);
                self.track(li, Ch::Opacity).to(t + 0.2 + hold, t + 0.4 + hold, TV::N(0.0), STD);
            }
            _ => {}
        }
    }

    fn type_hot(&mut self, b: usize, r: [f64; 4], t: f64, n: &Node) {
        let text = n.args.iter().filter_map(|a| a.as_str()).find(|s| !s.starts_with('#')).unwrap_or("").to_string();
        let (sw, sh) = (self.browsers[b].shot_w, self.browsers[b].shot_h);
        let scale = self.browsers[b].view_w / sw;
        let marks = self.browsers[b].marks;
        let mut l = Layer::new("", Kind::Text { runs: vec![Run { text: text.clone(), ..Default::default() }] });
        l.layout.pos = Pos::Abs;
        l.layout.x = Some(Len::pct((r[0] + 16.0) / sw * 100.0));
        l.layout.y = Some(Len::pct((r[1] + r[3] / 2.0) / sh * 100.0));
        l.layout.anchor = (0.0, 0.5);
        l.style.size = Some(self.num(n, "size").unwrap_or((r[3] * 0.32 * scale).clamp(8.0, 40.0)));
        l.style.color = Some(self.color(n, "color").unwrap_or(hex("#15181d")));
        l.style.nowrap = true;
        l.style.text_align = Some("left".into());
        l.style.clip = Some(ClipShape::Wipe("left".into()));
        let li = self.add(Some(marks), l);
        let chars = text.chars().count().max(1);
        let d = self.dur(n, "dur").unwrap_or(chars as f64 * 0.05);
        self.set_base(li, Ch::Reveal, TV::N(0.0));
        self.track(li, Ch::Reveal).to(t, t + d, TV::N(1.0), Ease::Steps(chars as u32));
        if let Some(until) = self.dur(n, "for") {
            self.track(li, Ch::Opacity).jump(t + d + until, TV::N(0.0));
        }
    }

    fn swap(&mut self, e: &Ev) {
        let n = &e.node;
        let t = e.t;
        let Some((b, _)) = self.browser_for(e) else {
            self.err("E_SWAP", "swap needs a browser/phone in the scene", "write `swap #app shot=next.png`", n.span);
            return;
        };
        let Some(shot) = praw(n, "shot").or_else(|| n.args.iter().filter_map(|a| a.as_str()).find(|s| s.ends_with(".png")).map(|s| s.to_string())) else {
            self.err("E_SWAP", "swap needs shot=<png>", "write `swap #app shot=assets/next.png`", n.span);
            return;
        };
        let path = self.base.join(&shot);
        if !path.exists() {
            self.err("E_ASSET", format!("shot `{shot}` not found"), "paths are relative to the .reel file", n.span);
        }
        let hot = praw(n, "hotspots").map(|h| self.base.join(h)).unwrap_or_else(|| path.with_extension("json"));
        if let Some(h) = load_hotspots(&hot) {
            self.browsers[b].hot = h;
        }
        let shots = self.browsers[b].shots;
        let k = self.kids[shots].len();
        let mut img = Layer::new(format!("{}-s{k}", self.lid(shots)), Kind::Image { src: path.to_string_lossy().to_string(), fit: "fill".into() });
        img.layout.pos = Pos::Fill;
        let ii = self.add(Some(shots), img);
        let via = praw(n, "via").unwrap_or("fade".into());
        let d = if via == "cut" { 0.0 } else { self.dur(n, "dur").unwrap_or(0.2) };
        self.set_base(ii, Ch::Opacity, TV::N(0.0));
        if d > 0.0 {
            self.track(ii, Ch::Opacity).to(t, t + d, TV::N(1.0), Ease::Linear);
        } else {
            self.track(ii, Ch::Opacity).jump(t, TV::N(1.0));
        }
        if let Some(old) = self.browsers[b].cur_img {
            self.track(old, Ch::Opacity).jump(t + d + 0.01, TV::N(0.0));
        }
        self.browsers[b].cur_img = Some(ii);
    }

    fn anim(&mut self, e: &Ev) {
        let n = &e.node;
        let t0 = e.t;
        let target = self.target(e, true);
        let (i, camera) = match target {
            Some(Target::El(i)) => (i, false),
            Some(Target::Camera(i)) => (i, true),
            Some(Target::Browser(b)) => (self.browsers[b].cam, false),
            _ => return,
        };
        if !camera {
            self.motion_blur_props(i, n);
        }
        let default_ease = self.ease(n, "ease", STD);
        let repeat = self.num(n, "repeat").unwrap_or(1.0).max(1.0) as usize;
        let yoyo = flag(n, "yoyo");
        let mut ch: Option<Ch> = None;
        let mut keys: Vec<(Ch, f64, TV, Option<Ease>)> = vec![];
        for a in &n.args {
            let s = a.raw();
            if s.starts_with('#') {
                continue;
            }
            if let Some(c) = Ch::parse(&s) {
                ch = Some(c);
                continue;
            }
            if s == "yoyo" || s == "loop" {
                continue;
            }
            let Some(c) = ch else {
                let hint = suggest(&s, &Ch::NAMES).map(|x| format!("did you mean channel `{x}`?")).unwrap_or(format!("channels: {}", Ch::NAMES.join(" ")));
                self.err("E_CHANNEL", format!("`{s}` is not a channel"), hint, n.span);
                return;
            };
            // expressions
            if s.starts_with("wiggle(") || s.starts_with("sin(") {
                let d = self.dur(n, "dur").unwrap_or(2.0);
                self.expr(i, c, &s, t0, d, n.span, camera);
                continue;
            }
            match parse_key(&s) {
                Ok(k) => {
                    let kt = match &k.t {
                        KeyTime::Dur(d) => t0 + d.secs(self.fps),
                        KeyTime::Pct(p) => t0 + p / 100.0 * self.dur(n, "dur").unwrap_or(1.0),
                        KeyTime::At(_) => {
                            let expr = s.split(':').next().unwrap_or("").trim_start_matches('@').to_string();
                            match self.tl.anchor_time(e.scene, None, &expr, t0, n.span) {
                                Ok(t) => t,
                                Err(d) => {
                                    self.diags.push(d);
                                    continue;
                                }
                            }
                        }
                    };
                    let Some(mut v) = self.tv(c, &k.v, n.span) else { continue };
                    if camera {
                        if let TV::N(x) = v {
                            if matches!(c, Ch::X | Ch::Y | Ch::RotX | Ch::RotY) {
                                v = TV::N(-x);
                            }
                        }
                    }
                    keys.push((c, kt, v, k.ease));
                }
                Err(err) => self.err(&err.code, format!("keyframe `{s}`: {}", err.message), err.hint, n.span),
            }
        }
        // group by channel
        let mut chans: Vec<Ch> = keys.iter().map(|k| k.0).collect();
        chans.dedup();
        for c in chans {
            let ks: Vec<&(Ch, f64, TV, Option<Ease>)> = keys.iter().filter(|k| k.0 == c).collect();
            if ks.is_empty() {
                continue;
            }
            let span = ks.last().unwrap().1 - ks[0].1;
            for r in 0..repeat {
                let off = span * r as f64;
                let rev = yoyo && r % 2 == 1;
                let seq: Vec<(f64, TV, Ease)> = if rev {
                    let n = ks.len();
                    (0..n).map(|j| (ks[0].1 + (ks[n - 1].1 - ks[n - 1 - j].1), ks[n - 1 - j].2.clone(), ks[n - 1 - j].3.clone().unwrap_or(default_ease.clone()))).collect()
                } else {
                    ks.iter().map(|k| (k.1, k.2.clone(), k.3.clone().unwrap_or(default_ease.clone()))).collect()
                };
                let tr = self.track(i, c);
                for (j, (kt, v, ease)) in seq.iter().enumerate() {
                    let at = kt + off;
                    if j == 0 {
                        if r == 0 && tr.keys.is_empty() {
                            tr.base = v.clone();
                        }
                        tr.jump(at - 1e-3, v.clone());
                    } else {
                        let prev = seq[j - 1].0 + off;
                        tr.to(prev, at, v.clone(), ease.clone());
                    }
                }
            }
        }
    }

    /// `rewind #g speed=4 dur=0.5s [then=play|hold]` and
    /// `timemap #g <dt>:<dlocal>[~ease] ... [then=play|hold]`: remap the local time of a subtree
    /// (`#scene` = everything in the scene's world, camera included). Baked in `remap.rs`.
    fn remap_verb(&mut self, e: &Ev) {
        let n = &e.node;
        let t0 = e.t;
        let i = if id_arg(n).as_deref() == Some("scene") || e.target.as_deref() == Some("scene") {
            self.scenes[e.scene].world
        } else {
            match self.target(e, true) {
                Some(Target::El(i)) => i,
                Some(Target::Camera(i)) => i,
                Some(Target::Browser(b)) => self.parent_of(self.browsers[b].viewport),
                Some(Target::Hot(..)) => {
                    self.err("E_TARGET", format!("`{}` needs an element (a layer subtree), not a screenshot hotspot", n.kind), "target the browser or a group: `rewind #app ...`", n.span);
                    return;
                }
                None => return,
            }
        };
        let tail = match praw(n, "then").as_deref() {
            None | Some("play") => 1.0,
            Some("hold") | Some("freeze") => 0.0,
            Some(x) => {
                self.err("E_VALUE", format!("`then={x}`: after the remap the subtree plays on or holds"), "write `then=play` (default) or `then=hold`", n.span);
                1.0
            }
        };
        let keys: Vec<(f64, f64, Ease)> = if n.kind == "rewind" {
            let speed = self.num(n, "speed").unwrap_or(4.0);
            let d = self.dur(n, "dur").unwrap_or(0.5);
            if !(speed.is_finite() && speed >= 0.0) || d <= 0.0 {
                self.err("E_VALUE", format!("rewind needs speed >= 0 and dur > 0 (got speed={speed}, dur={d})"), "write `rewind #g speed=4 dur=0.5s` (4x backwards for 0.5 s; speed=0 freezes)", n.span);
                return;
            }
            let ease = self.ease(n, "ease", Ease::Linear);
            vec![(0.0, 0.0, ease), (d, -speed * d, Ease::Linear)]
        } else {
            let mut ks = vec![];
            for a in &n.args {
                let s = a.raw();
                if s.starts_with('#') {
                    continue;
                }
                let bad = |me: &mut Self| me.err("E_KEY", format!("timemap key `{s}`: write <parent time>:<local time>"), "times relative to the event, e.g. `timemap #g 0s:0s 0.5s:-2s 1s:-2s` (local time runs back 2 s in 0.5 s, then holds)", n.span);
                let Ok(k) = parse_key(&s) else {
                    bad(self);
                    continue;
                };
                let kt = match &k.t {
                    KeyTime::Dur(d) => d.secs(self.fps),
                    KeyTime::Pct(p) => p / 100.0 * self.dur(n, "dur").unwrap_or(1.0),
                    KeyTime::At(_) => {
                        let expr = s.split(':').next().unwrap_or("").trim_start_matches('@').to_string();
                        match self.tl.anchor_time(e.scene, None, &expr, t0, n.span) {
                            Ok(t) => t - t0,
                            Err(d) => {
                                self.diags.push(d);
                                continue;
                            }
                        }
                    }
                };
                let v = k.v.trim();
                let (neg, mag) = match v.strip_prefix('-') {
                    Some(m) => (true, m),
                    None => (false, v.strip_prefix('+').unwrap_or(v)),
                };
                let Ok(d) = parse_dur(mag) else {
                    bad(self);
                    continue;
                };
                let lv = d.secs(self.fps) * if neg { -1.0 } else { 1.0 };
                if kt < -1e-9 {
                    self.err("E_KEY", format!("timemap key `{s}` lies before the event"), "parent times are offsets >= 0 from the event", n.span);
                    continue;
                }
                ks.push((kt, lv, k.ease.unwrap_or(Ease::Linear)));
            }
            if ks.is_empty() {
                self.err("E_KEY", "timemap needs at least one key", "write `timemap #g 0s:0s 0.5s:-2s` (parent offset : local offset)", n.span);
                return;
            }
            ks
        };
        let id = self.lid(i);
        self.remaps.entry(id).or_default().push(t0, keys, tail);
    }

    fn expr(&mut self, i: usize, c: Ch, s: &str, t0: f64, d: f64, span: Span, _camera: bool) {
        let inner = s.split_once('(').map(|x| x.1.trim_end_matches(')')).unwrap_or("");
        let parts: Vec<&str> = inner.split(',').map(|x| x.trim()).collect();
        let num = |k: usize, d: f64| parts.get(k).and_then(|v| v.trim_end_matches("deg").trim_end_matches("px").parse::<f64>().ok()).unwrap_or(d);
        let freq = num(0, 2.0);
        let amp = num(1, 10.0);
        let seed = num(2, 1.0) as u64;
        if s.starts_with("wiggle(") || s.starts_with("sin(") {
            let base = self.track(i, c).eval(t0).n();
            let steps = ((d * freq * 4.0).ceil() as usize).max(2);
            let mut rng = Rng(seed.wrapping_add(0x51ED));
            let tr_vals: Vec<f64> = (0..=steps).map(|k| {
                let tt = k as f64 / steps as f64 * d;
                if s.starts_with("sin(") { amp * (std::f64::consts::TAU * freq * tt).sin() } else { amp * (rng.f() * 2.0 - 1.0) }
            }).collect();
            let tr = self.track(i, c);
            for (k, v) in tr_vals.iter().enumerate() {
                let a = t0 + k as f64 / steps as f64 * d;
                if k == 0 {
                    tr.jump(a - 1e-3, TV::N(base + v));
                } else {
                    let p = t0 + (k - 1) as f64 / steps as f64 * d;
                    tr.to(p, a, TV::N(base + v), Ease::Cubic(0.37, 0.0, 0.63, 1.0));
                }
            }
        } else {
            self.err("E_EXPR", format!("unknown expression `{s}`"), "expressions: wiggle(freq, amp, seed), sin(freq, amp)", span);
        }
    }

    // ------------------------------------------------------------ scene-level
    pub fn scene_transitions(&mut self, scenes: &[&Node]) {
        let video_default = None::<String>;
        let n = self.scenes.len();
        let mut tds = vec![0.0; n];
        for si in 0..n {
            let root = self.scenes[si].root;
            let start = self.scenes[si].start;
            self.set_base(root, Ch::Opacity, TV::N(0.0));
            self.track(root, Ch::Opacity).jump(start - 1e-3, TV::N(1.0));
            if si == 0 {
                continue;
            }
            let sn = scenes[si];
            let tr = sn.props.get("in").map(|v| v.raw()).or(video_default.clone()).unwrap_or("cut".into());
            if tr == "cut" {
                continue;
            }
            let td = self.dur(sn, "tdur").unwrap_or(0.6);
            tds[si] = td;
            let ease = self.ease(sn, "tease", STD);
            let prev = self.scenes[si - 1].root;
            let w = self.w;
            match tr.as_str() {
                "fade" | "crossfade" | "dissolve" => self.track(root, Ch::Opacity).from_to(start, start + td, TV::N(0.0), TV::N(1.0), ease),
                "push" | "push-left" | "push-right" | "push-up" | "push-down" => {
                    let (ch, d) = match tr.as_str() {
                        "push-right" => (Ch::X, -w),
                        "push-up" => (Ch::Y, self.h),
                        "push-down" => (Ch::Y, -self.h),
                        _ => (Ch::X, w),
                    };
                    self.track(root, ch).from_to(start, start + td, TV::N(d), TV::N(0.0), ease.clone());
                    self.track(prev, ch).from_to(start, start + td, TV::N(0.0), TV::N(-d), ease);
                }
                "slide" | "slide-up" => {
                    let (ch, d) = if tr == "slide-up" { (Ch::Y, self.h) } else { (Ch::X, w) };
                    self.track(root, ch).from_to(start, start + td, TV::N(d), TV::N(0.0), ease);
                }
                "wipe" | "wipe-left" | "wipe-right" | "wipe-up" | "wipe-down" | "iris" => {
                    self.layers[root].style.clip = Some(if tr == "iris" {
                        ClipShape::Iris(0.5, 0.5)
                    } else {
                        ClipShape::Wipe(match tr.as_str() {
                            "wipe-right" => "right",
                            "wipe-up" => "bottom",
                            "wipe-down" => "top",
                            _ => "left",
                        }.into())
                    });
                    self.set_base(root, Ch::Reveal, TV::N(0.0));
                    self.track(root, Ch::Reveal).to(start, start + td, TV::N(1.0), ease);
                }
                "zoom" => {
                    self.track(root, Ch::Opacity).from_to(start, start + td, TV::N(0.0), TV::N(1.0), ease.clone());
                    self.track(root, Ch::Scale).from_to(start, start + td, TV::N(0.7), TV::N(1.0), ease.clone());
                    self.track(prev, Ch::Scale).to(start, start + td, TV::N(1.6), ease.clone());
                    self.track(prev, Ch::Blur).to(start, start + td, TV::N(16.0), ease);
                }
                "magic" => {
                    self.track(root, Ch::Opacity).from_to(start, start + td, TV::N(0.0), TV::N(1.0), Ease::Cubic(0.33, 1.0, 0.68, 1.0));
                    // shared ids between the two scenes morph geometry
                    let pairs: Vec<(usize, usize)> = self.names.iter().filter(|(k, v)| k.ends_with(&format!("@{si}")) && v.scene == si).filter_map(|(k, v)| {
                        let base = k.split('@').next().unwrap();
                        self.names.get(&format!("{base}@{}", si - 1)).map(|p| (p.idx, v.idx))
                    }).collect();
                    if pairs.is_empty() {
                        self.warn("W_MAGIC_NONE", format!("scene `{}` uses in=magic but shares no ids with the previous scene", self.scenes[si].id), "give matching elements the same #id in both scenes", sn.span);
                    }
                    for (a, b) in pairs {
                        let (la, lb) = (self.lid(a), self.lid(b));
                        self.hooks.push(Hook::Magic { from: la, to: lb });
                        self.set_base(b, Ch::Magic, TV::N(0.0));
                        self.track(b, Ch::Magic).to(start, start + td, TV::N(1.0), ease.clone());
                        self.track(a, Ch::Magic).to(start, start + td, TV::N(0.0), ease.clone());
                        self.track(a, Ch::Opacity).to(start, start + td * 0.6, TV::N(0.0), Ease::Linear);
                    }
                }
                other => {
                    let opts = ["cut", "fade", "push", "push-up", "slide", "wipe", "iris", "zoom", "magic"];
                    let hint = suggest(other, &opts).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("transitions: {}", opts.join(", ")));
                    self.err("E_TRANSITION", format!("unknown transition `{other}`"), hint, sn.span);
                }
            }
        }
        for si in 0..n {
            let root = self.scenes[si].root;
            let end = self.scenes[si].start + self.scenes[si].dur + if si + 1 < n { tds[si + 1] } else { 0.0 };
            if si + 1 < n {
                self.track(root, Ch::Opacity).jump(end, TV::N(0.0));
            }
        }
    }

    pub fn captions_all(&mut self) {
        if self.captions == "off" {
            return;
        }
        let karaoke = self.captions == "karaoke";
        let size = self.th.size("caption").unwrap_or(33.0);
        let fps = self.fps;
        for si in 0..self.scenes.len() {
            if self.scenes[si].nocap {
                continue;
            }
            let says = self.tl.scenes[si].says.clone();
            let anchors = self.tl.scenes[si].anchors.clone();
            for (k, say) in says.iter().enumerate() {
                let ov = self.scenes[si].overlay;
                let mut g = Layer::new(format!("cap-{}-{k}", self.scenes[si].id), Kind::Group);
                g.layout.pos = Pos::Abs;
                g.layout.x = Some(Len::pct(50.0));
                g.layout.y = Some(Len::px(self.h - self.th.safe.1 * self.h * 0.55));
                g.layout.anchor = (0.5, 1.0);
                g.layout.maxw = Some(Len::pct(86.0));
                g.layout.w = Some(Len::pct(86.0));
                g.layout.flow = "inline";
                g.layout.justify = "center".into();
                g.style.size = Some(size);
                g.style.weight = Some(550.0);
                g.style.line_height = Some(1.3);
                g.style.color = Some(self.th.colors["text"].clone());
                g.style.tracking = Some(-0.005);
                g.tags.push("caption".into());
                g.tags.push("text".into());
                let gi = self.add(Some(ov), g);
                let muted = Rgba { a: 0.55, ..self.th.colors["text"].clone() };
                let n = say.words.len();
                for (wi, w) in say.words.iter().enumerate() {
                    let txt = if wi + 1 < n { format!("{} ", w.text) } else { w.text.clone() };
                    let in_anchor = anchors.iter().any(|a| a.say == k && w.start >= a.start - 1e-6 && w.end <= a.end + 1e-6);
                    let mut l = Layer::new(format!("{}-w{wi}", self.lid(gi)), Kind::Text { runs: vec![Run { text: txt, ..Default::default() }] });
                    l.style.nowrap = true;
                    l.tags.push("piece".into());
                    let li = self.add(Some(gi), l);
                    let hi = if in_anchor { self.th.colors["mint"].clone() } else { self.th.colors["text"].clone() };
                    if karaoke {
                        self.set_base(li, Ch::Color, TV::C(muted.clone()));
                        self.track(li, Ch::Color).to(w.start, w.start + 0.08, TV::C(hi), Ease::Linear);
                    }
                }
                let (a, b) = (say.start, say.end);
                self.set_base(gi, Ch::Opacity, TV::N(0.0));
                self.track(gi, Ch::Opacity).to(a, a + 7.0 / fps, TV::N(1.0), Ease::Linear);
                self.track(gi, Ch::Opacity).to(b - 7.0 / fps, b, TV::N(0.0), Ease::Linear);
                self.track(gi, Ch::Y).from_to(a, a + 8.0 / fps, TV::N(10.0), TV::N(0.0), Ease::Linear);
            }
        }
    }

    pub fn chapterbar(&mut self, n: &Node, scenes: &[&Node]) {
        let label = n.props.get("label").map(|v| v.raw()).unwrap_or_default();
        let only: Option<Vec<String>> = match n.props.get("scenes") {
            Some(Value::List(v)) => Some(v.iter().map(|x| x.raw()).collect()),
            _ => None,
        };
        let idxs: Vec<usize> = (0..self.scenes.len()).filter(|&i| match &only {
            Some(l) => l.contains(&self.scenes[i].id),
            None => scenes[i].props.contains_key("title"),
        }).collect();
        let total = idxs.len();
        let fps = self.fps;
        for (pos, &si) in idxs.iter().enumerate() {
            let title = scenes[si].props.get("title").map(|v| v.raw()).unwrap_or_default();
            let ov = self.scenes[si].overlay;
            let mut g = Layer::new(format!("cbar-{}", self.scenes[si].id), Kind::Group);
            g.layout.pos = Pos::Abs;
            g.layout.x = Some(Len::pct(8.33));
            g.layout.y = Some(Len::px(26.0));
            g.layout.w = Some(Len::pct(83.34));
            g.layout.h = Some(Len::px(40.0));
            g.layout.flow = "row";
            g.layout.gap = 16.0;
            g.layout.justify = "start".into();
            let gi = self.add(Some(ov), g);
            let mut num = Layer::new("", Kind::Text { runs: vec![Run { text: format!("{}", pos + 1), ..Default::default() }] });
            num.layout.w = Some(Len::px(34.0));
            num.layout.h = Some(Len::px(34.0));
            num.style.radius = 17.0;
            num.style.fill = Some(Paint::Solid(ColorRef::Lit(self.th.colors["accent"].clone())));
            num.style.color = Some(hex("#fff"));
            num.style.size = Some(20.0);
            num.style.weight = Some(800.0);
            num.style.text_align = Some("center".into());
            num.style.line_height = Some(34.0 / 18.0);
            self.add(Some(gi), num);
            let mut tt = Layer::new("", Kind::Text { runs: vec![Run { text: title, ..Default::default() }] });
            tt.style.size = Some(24.0);
            tt.style.weight = Some(700.0);
            tt.style.color = Some(hex("#fff"));
            tt.style.nowrap = true;
            self.add(Some(gi), tt);
            if !label.is_empty() && self.w > self.h {
                let mut lb = Layer::new("", Kind::Text { runs: vec![Run { text: label.clone(), ..Default::default() }] });
                lb.style.size = Some(20.0);
                lb.style.weight = Some(600.0);
                lb.style.color = Some(self.th.colors["mint"].clone());
                lb.style.nowrap = true;
                self.add(Some(gi), lb);
            }
            let mut sp = Layer::new("", Kind::Group);
            sp.layout.grow = 1.0;
            self.add(Some(gi), sp);
            let mut dots = Layer::new("", Kind::Group);
            dots.layout.flow = "row";
            dots.layout.gap = 8.0;
            let di = self.add(Some(gi), dots);
            for k in 0..total {
                let mut d = Layer::new("", Kind::Rect);
                d.layout.w = Some(Len::px(if k == pos { 36.0 } else { 12.0 }));
                d.layout.h = Some(Len::px(6.0));
                d.style.radius = 3.0;
                d.style.fill = Some(Paint::Solid(ColorRef::Lit(if k <= pos { self.th.colors["accent2"].clone() } else { hex("#ffffff40") })));
                self.add(Some(di), d);
            }
            let (s, e) = (self.scenes[si].start, self.scenes[si].start + self.scenes[si].dur);
            self.set_base(gi, Ch::Opacity, TV::N(0.0));
            self.track(gi, Ch::Opacity).to(s + 40.0 / fps, s + 54.0 / fps, TV::N(1.0), Ease::Linear);
            self.track(gi, Ch::Opacity).to(e - 8.0 / fps, e, TV::N(0.0), Ease::Linear);
        }
    }

    pub fn music(&mut self, n: &Node) {
        let Some(src) = n.props.get("src").map(|v| v.raw()).or_else(|| text_arg(n)) else {
            self.err("E_MUSIC", "music needs src=", "write `music src=assets/bed.mp3 vol=0.25 duck=0.08`", n.span);
            return;
        };
        let src = self.asset(&src, n);
        let gain = self.num(n, "vol").unwrap_or(0.25);
        let duck = self.num(n, "duck").or(Some(gain * 0.35));
        self.audio.push(AudioClip { src, at: 0.0, dur: self.tl.total, gain, duck, kind: "music" });
    }
}

fn self_glow(n: &Node) -> f64 {
    n.props.get("glow").and_then(|v| v.raw().trim_end_matches("px").parse().ok()).unwrap_or(22.0)
}

fn preset_arg(n: &Node) -> Option<String> {
    n.args.iter().filter_map(|a| a.as_str()).find(|s| !s.starts_with('#')).map(|s| s.to_string())
}

/// SplitMix64: deterministic, seeded randomness (particles, wiggle).
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    pub fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}
