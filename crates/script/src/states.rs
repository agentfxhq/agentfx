//! Design layer L3: per-step detail for explainers (the S1 gap to HyperFrames): a done state with a
//! check badge on nodes, a progress component, a detail panel whose pages swap per step, a chart
//! frame (title, source line, annotation on the highlighted value), and HTML/CSS skins for leaf
//! components, rendered through the native `html` block path (design/skins/<component>/<name>.html).
use crate::ast::{Node, Value};
use crate::ir::*;
use crate::lower::*;
use crate::theme::hex;
use crate::values::*;

fn praw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}
fn solid(c: &Rgba) -> Paint {
    Paint::Solid(ColorRef::Lit(c.clone()))
}
fn alpha(c: &Rgba, a: f64) -> Rgba {
    Rgba { a: c.a * a, ..c.clone() }
}
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
fn css(c: &Rgba) -> String {
    c.to_css()
}

impl<'a> Ctx<'a> {
    fn cc(&self, k: &str) -> Rgba {
        self.th.colors.get(k).cloned().unwrap_or(hex("#f0f"))
    }

    /// Hidden check badge (`done #id` pops it in): a disc in the accent color with a check, on the
    /// top-right corner of `gi`.
    pub fn done_badge(&mut self, gi: usize, size: f64) {
        let id = self.lid(gi);
        let mut b = Layer::new(format!("{id}-done"), Kind::Group);
        b.layout.pos = Pos::Abs;
        b.layout.x = Some(Len::pct(100.0));
        b.layout.y = Some(Len::px(0.0));
        b.layout.anchor = (0.62, 0.38);
        b.layout.w = Some(Len::px(size));
        b.layout.h = Some(Len::px(size));
        b.layout.flow = "row";
        b.style.radius = size;
        b.style.fill = Some(solid(&self.cc("accent2")));
        b.style.border = Some(((size / 12.0).round().max(2.0), self.cc("bg")));
        b.style.origin = (0.5, 0.5);
        let bi = self.add(Some(gi), b);
        let paper = self.cc("paper");
        self.icon_l(bi, format!("{id}-done-i"), "check", (size * 0.58).round(), &paper, 3.0, &Node::default());
        self.set_base(bi, Ch::Opacity, TV::N(0.0));
    }

    /// `progress #p n=6 value=0 label="Schritt"`: segmented progress with a counter "4 / 6".
    /// `progress #p to=4` (verb) fills the segments up to 4 and counts.
    pub fn progress_c(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let count = self.num(n, "n").or_else(|| self.num(n, "steps")).unwrap_or(5.0).max(1.0) as usize;
        let value = self.num(n, "value").unwrap_or(0.0) as usize;
        let k = self.k;
        let w = self.num(n, "w").unwrap_or(960.0 * k).min(self.w * (1.0 - 2.0 * self.th.safe.0));
        let mut g = Layer::new(lid, Kind::Group);
        g.layout.flow = "col";
        g.layout.gap = (14.0 * k).round();
        g.layout.w = Some(Len::px(w));
        g.tags.push("progress".into());
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        // header: label (kicker) left, "value / n" right
        let mut hdr = Layer::new(format!("{id}-hdr"), Kind::Group);
        hdr.layout.flow = "row";
        hdr.layout.justify = "between".into();
        hdr.layout.align = "end".into();
        hdr.layout.w = Some(Len::pct(100.0));
        let hi = self.add(Some(gi), hdr);
        let label = praw(n, "label").or_else(|| text_arg(n)).unwrap_or_else(|| "Schritt".into());
        let mc = self.cc("muted");
        self.text_l(hi, format!("{id}-label"), &label, crate::components::TextSpec { size: 24.0, weight: 700.0, color: mc, align: "left", tracking: Some(0.08), upper: true, nowrap: true, lh: None });
        let mut ctr = Layer::new(format!("{id}-count"), Kind::Counter { prefix: String::new(), suffix: format!(" / {count}") });
        ctr.tags.push("text".into());
        ctr.style.size = Some((34.0 * k).round());
        ctr.style.weight = Some(self.th.d.w_heading);
        ctr.style.font = Some(self.th.role_font(true));
        ctr.style.color = Some(self.cc("text"));
        ctr.style.nowrap = true;
        let ci = self.add(Some(hi), ctr);
        self.set_base(ci, Ch::Count, TV::N(value as f64));
        // segments
        let mut row = Layer::new(format!("{id}-segs"), Kind::Group);
        row.layout.flow = "row";
        row.layout.gap = (10.0 * k).round();
        row.layout.w = Some(Len::pct(100.0));
        let ri = self.add(Some(gi), row);
        let segh = (12.0 * k).round();
        let rad = self.th.d.radius_sm.unwrap_or(segh / 2.0);
        for s in 0..count {
            let mut sg = Layer::new(format!("{id}-s{s}"), Kind::Group);
            sg.layout.grow = 1.0;
            sg.layout.w = Some(Len::px(0.0));
            sg.layout.h = Some(Len::px(segh));
            sg.style.radius = rad;
            sg.style.fill = Some(solid(&alpha(&self.cc("text"), 0.14)));
            sg.style.overflow_clip = true;
            let si = self.add(Some(ri), sg);
            let mut f = Layer::new(format!("{id}-s{s}-fill"), Kind::Rect);
            f.layout.pos = Pos::Fill;
            f.style.fill = Some(solid(&self.cc("accent2")));
            f.style.origin = (0.0, 0.5);
            f.tags.push("seg-fill".into());
            let fi = self.add(Some(si), f);
            if s >= value {
                self.set_base(fi, Ch::Sx, TV::N(0.0));
            }
        }
        gi
    }

    /// `panel #det title="Pipeline #482" kicker="Details" w=900` with `pane #id title=".."` children,
    /// each holding `item "text" chip="bestanden" status=ok|run|wait|fail icon=..` lines (or text args).
    /// Only the first page shows; `page #det #id` (verb) swaps to another page (blur swap).
    pub fn panel_c(&mut self, parent: usize, si: usize, n: &Node, lid: String, user: Option<String>) -> usize {
        let k = self.k;
        let w = self.num(n, "w").unwrap_or(900.0 * k).min(self.w * (1.0 - 2.0 * self.th.safe.0));
        let mut g = Layer::new(lid, Kind::Group);
        g.layout.flow = "col";
        g.layout.gap = (16.0 * k).round();
        g.layout.pad = [(30.0 * k).round(), (36.0 * k).round(), (32.0 * k).round(), (36.0 * k).round()];
        g.layout.w = Some(Len::px(w));
        g.layout.align = "stretch".into();
        g.tags.push("panel".into());
        self.surface_pub(&mut g, self.th.radius);
        let gi = self.add(Some(parent), g);
        let id = self.lid(gi);
        if let Some(kk) = praw(n, "kicker") {
            let c = self.cc("mint");
            self.text_l(gi, format!("{id}-kicker"), &kk, crate::components::TextSpec { size: 24.0, weight: 700.0, color: c, align: "left", tracking: Some(0.08), upper: true, nowrap: true, lh: None });
        }
        let pages: Vec<&Node> = n.children.iter().filter(|c| c.kind == "pane").collect();
        let loose: Vec<&Node> = n.children.iter().filter(|c| c.kind == "item").collect();
        let row_h = (58.0 * k).round();
        let max_items = pages.iter().map(|p| p.children.iter().filter(|c| c.kind == "item").count() + p.args.iter().filter(|a| !a.raw().starts_with('#')).count()).max().unwrap_or(loose.len()).max(1);
        let has_title = pages.iter().any(|p| p.props.contains_key("title")) || n.props.contains_key("title");
        let body_h = max_items as f64 * row_h + if has_title { (60.0 * k).round() } else { 0.0 };
        let mut body = Layer::new(format!("{id}-body"), Kind::Group);
        body.layout.w = Some(Len::pct(100.0));
        body.layout.h = Some(Len::px(body_h));
        let bi = self.add(Some(gi), body);
        let mut page_list: Vec<(String, Node)> = pages.iter().enumerate().map(|(i, p)| (id_arg(p).unwrap_or(format!("p{}", i + 1)), (*p).clone())).collect();
        if page_list.is_empty() {
            let mut p = Node { kind: "pane".into(), children: loose.iter().map(|c| (*c).clone()).collect(), ..Default::default() };
            if let Some(t) = n.props.get("title") {
                p.props.insert("title".into(), t.clone());
            }
            page_list.push(("p1".into(), p));
        }
        for (pi, (pid, p)) in page_list.iter().enumerate() {
            let mut pg = Layer::new(format!("{id}-page-{pid}"), Kind::Group);
            pg.layout.pos = Pos::Abs;
            pg.layout.x = Some(Len::px(0.0));
            pg.layout.y = Some(Len::px(0.0));
            pg.layout.w = Some(Len::pct(100.0));
            pg.layout.flow = "col";
            pg.layout.gap = (10.0 * k).round();
            pg.tags.push("page".into());
            let pgi = self.add(Some(bi), pg);
            if pi > 0 {
                self.set_base(pgi, Ch::Opacity, TV::N(0.0));
            }
            if let Some(u) = &user {
                let name = format!("{u}.{pid}");
                self.layers[pgi].name = Some(name.clone());
                self.names.insert(format!("{name}@{si}"), Named { idx: pgi, scene: si, kind: "page".into() });
            }
            if let Some(t) = praw(p, "title").or_else(|| if pi == 0 { praw(n, "title") } else { None }) {
                let tc = self.cc("text");
                self.text_l(pgi, format!("{id}-page-{pid}-title"), &t, crate::components::TextSpec { size: 40.0, weight: 720.0, color: tc, align: "left", tracking: Some(-0.01), upper: false, nowrap: true, lh: Some(1.2) });
            }
            let mut items: Vec<Node> = p.args.iter().filter(|a| !a.raw().starts_with('#')).map(|a| Node { kind: "item".into(), args: vec![a.clone()], ..Default::default() }).collect();
            items.extend(p.children.iter().filter(|c| c.kind == "item").cloned());
            for (ii, it) in items.iter().enumerate() {
                self.panel_item(pgi, format!("{id}-page-{pid}-i{ii}"), it, row_h);
            }
        }
        gi
    }

    fn panel_item(&mut self, parent: usize, id: String, it: &Node, row_h: f64) {
        let k = self.k;
        let mut r = Layer::new(id.clone(), Kind::Group);
        r.layout.flow = "row";
        r.layout.align = "center".into();
        r.layout.gap = (16.0 * k).round();
        r.layout.w = Some(Len::pct(100.0));
        r.layout.h = Some(Len::px(row_h - (10.0 * k).round()));
        let ri = self.add(Some(parent), r);
        let status = praw(it, "status").unwrap_or_default();
        let (sc, icon) = match status.as_str() {
            "ok" | "done" => (self.cc("accent2"), "check"),
            "run" | "active" => (self.cc("warn"), "sync"),
            "fail" | "err" => (self.cc("danger"), "x"),
            "wait" => (self.cc("faint"), "clock"),
            _ => (self.cc("mint"), "arrow"),
        };
        let icon = praw(it, "icon").unwrap_or_else(|| icon.to_string());
        let ds = (34.0 * k).round();
        let mut dot = Layer::new(format!("{id}-st"), Kind::Group);
        dot.layout.w = Some(Len::px(ds));
        dot.layout.h = Some(Len::px(ds));
        dot.layout.flow = "row";
        dot.style.radius = ds;
        dot.style.fill = Some(solid(&alpha(&sc, 0.2)));
        let di = self.add(Some(ri), dot);
        self.icon_l(di, format!("{id}-st-i"), &icon, (ds * 0.6).round(), &sc, 2.6, it);
        let txt = text_arg(it).unwrap_or_default();
        let mono = it.props.get("mono").is_some() || txt.contains('/') || txt.contains(".astro") || txt.starts_with('$');
        let tc = self.cc("text");
        let ti = self.text_l(ri, format!("{id}-t"), &txt, crate::components::TextSpec { size: 30.0, weight: 520.0, color: tc, align: "left", tracking: None, upper: false, nowrap: true, lh: None });
        self.layers[ti].layout.grow = 1.0;
        if mono {
            self.layers[ti].style.font = Some(self.th.mono.clone());
            self.layers[ti].style.size = Some((27.0 * k).round());
        }
        if let Some(chip) = praw(it, "chip") {
            let tcol = if self.th.name.contains("light") || self.th.d.surface == "outline" { sc.clone() } else { Rgba { r: (sc.r + 1.0) / 2.0, g: (sc.g + 1.0) / 2.0, b: (sc.b + 1.0) / 2.0, a: 1.0 } };
            let mut p = Layer::new(format!("{id}-chip"), Kind::Text { runs: self.runs(&chip) });
            p.tags.push("text".into());
            p.style.size = Some((22.0 * k).round());
            p.style.weight = Some(self.th.d.w_label);
            p.style.font = Some(self.th.role_font(false));
            p.style.uppercase = self.th.d.label_upper;
            p.style.tracking = Some(0.05);
            p.style.color = Some(tcol);
            p.style.fill = Some(solid(&alpha(&sc, 0.18)));
            p.style.radius = self.th.d.radius_sm.unwrap_or(999.0);
            p.style.nowrap = true;
            p.layout.pad = [(5.0 * k).round(), (14.0 * k).round(), (5.0 * k).round(), (14.0 * k).round()];
            self.add(Some(ri), p);
        }
    }

    /// Chart frame: kicker + title above, source line below, and an annotation on the highlighted
    /// value (`chart ... title="Umsatz 2025" source="Beispieldaten" note="x2 gegenüber Q1"`).
    /// Returns the outer group; the chart itself is built inside it.
    pub fn chart_frame_open(&mut self, parent: usize, n: &Node, lid: &str) -> usize {
        let k = self.k;
        let mut g = Layer::new(format!("{lid}-frame"), Kind::Group);
        g.layout.flow = "col";
        g.layout.gap = (18.0 * k).round();
        g.layout.align = "start".into();
        g.tags.push("chart-frame".into());
        let gi = self.add(Some(parent), g);
        if let Some(kk) = praw(n, "kicker") {
            let c = self.cc("mint");
            self.text_l(gi, format!("{lid}-frame-kicker"), &kk, crate::components::TextSpec { size: 24.0, weight: 700.0, color: c, align: "left", tracking: Some(0.08), upper: true, nowrap: true, lh: None });
        }
        if let Some(t) = praw(n, "title") {
            let c = self.cc("text");
            self.text_l(gi, format!("{lid}-frame-title"), &t, crate::components::TextSpec { size: 46.0, weight: 720.0, color: c, align: "left", tracking: Some(-0.01), upper: false, nowrap: false, lh: Some(1.15) });
        }
        gi
    }
    pub fn chart_frame_close(&mut self, gi: usize, n: &Node, lid: &str) {
        if let Some(s) = praw(n, "source") {
            let c = self.cc("faint");
            self.text_l(gi, format!("{lid}-frame-source"), &s, crate::components::TextSpec { size: 22.0, weight: 500.0, color: c, align: "left", tracking: None, upper: false, nowrap: true, lh: None });
        }
    }

    // ------------------------------------------------------------------ skins (HTML/CSS, L3)

    /// Instantiate a skin: design/skins/<component>/<name>.html with {{slots}} filled from the node and
    /// the design tokens as CSS custom properties; laid out and painted natively as an `html` block,
    /// parts (`data-part`) become layers `#id.part` (active, done, ...). Returns None when not found.
    pub fn skin(&mut self, parent: usize, si: usize, n: &Node, lid: String, comp: &str, name: &str) -> Option<usize> {
        let path = crate::run::home().join("design/skins").join(comp).join(format!("{name}.html"));
        let Ok(tpl) = std::fs::read_to_string(&path) else {
            let avail: Vec<String> = std::fs::read_dir(crate::run::home().join("design/skins").join(comp)).map(|rd| rd.flatten().filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string())).collect()).unwrap_or_default();
            self.err("E_SKIN", format!("no skin `{name}` for `{comp}`"), format!("skins for {comp}: {}", if avail.is_empty() { "none".into() } else { avail.join(", ") }), n.span);
            return None;
        };
        let k = self.k;
        // header comment: `size: w=420`
        let dw = tpl.lines().find_map(|l| l.trim().strip_prefix("size:").map(|s| s.trim().trim_start_matches("w=").trim().to_string())).and_then(|s| s.parse::<f64>().ok()).unwrap_or(420.0);
        let w = self.num(n, "w").unwrap_or((dw * k).round());
        let mut out = tpl.clone();
        // remove the header comment
        if let (Some(a), Some(b)) = (out.find("<!--"), out.find("-->")) {
            if a < 5 {
                out.replace_range(a..b + 3, "");
            }
        }
        let icon_svg = |me: &Self, nm: &str, col: &str| -> String {
            crate::components::icon_body(nm).map(|b| format!("<svg viewBox='0 0 24 24' width='100%' height='100%' fill='none' stroke='{col}' stroke-width='{}' stroke-linecap='round' stroke-linejoin='round'>{b}</svg>", me.th.d.icon_stroke)).unwrap_or_default()
        };
        // {{px:34}} -> scaled px
        while let Some(p) = out.find("{{px:") {
            let Some(e) = out[p..].find("}}").map(|e| p + e) else {
                self.err("E_SKIN", format!("skin `{comp}/{name}`: unclosed {{{{px:"), "close every {{px:N}} slot", n.span);
                return None;
            };
            let v: f64 = out[p + 5..e].trim().parse().unwrap_or(0.0);
            out.replace_range(p..e + 2, &format!("{}px", (v * k).round()));
        }
        // {{icon:name:color-token}} and {{icon}}
        while let Some(p) = out.find("{{icon:") {
            let Some(e) = out[p..].find("}}").map(|e| p + e) else {
                self.err("E_SKIN", format!("skin `{comp}/{name}`: unclosed {{{{icon:"), "close every {{icon:name:color}} slot", n.span);
                return None;
            };
            let spec = out[p + 7..e].to_string();
            let mut it = spec.split(':');
            let nm = it.next().unwrap_or("check").to_string();
            let col = it.next().map(|t| self.cc(t).to_css()).unwrap_or("currentColor".into());
            let svg = icon_svg(self, &nm, &col);
            out.replace_range(p..e + 2, &svg);
        }
        let icon = praw(n, "icon").unwrap_or_default();
        let icon_html = if icon.is_empty() { String::new() } else { icon_svg(self, &icon, "currentColor") };
        let label = text_arg(n).unwrap_or_default();
        let slots: Vec<(String, String)> = vec![
            ("label".into(), esc(&label)),
            ("sub".into(), esc(&praw(n, "sub").unwrap_or_default())),
            ("kicker".into(), esc(&praw(n, "kicker").unwrap_or_default())),
            ("n".into(), esc(&praw(n, "n").unwrap_or_default())),
            ("status".into(), esc(&praw(n, "status").unwrap_or_default())),
            ("icon".into(), icon_html),
            ("has-icon".into(), if icon.is_empty() { "no-icon".into() } else { "has-icon".into() }),
        ];
        for (k_, v) in slots {
            out = out.replace(&format!("{{{{{k_}}}}}"), &v);
        }
        // design tokens as custom properties on the skin root
        let d = &self.th.d;
        let mut vars = String::new();
        for c in ["bg", "bg2", "surface", "surface2", "border", "text", "muted", "faint", "mint", "accent", "accent2", "danger", "warn", "ink", "paper"] {
            vars.push_str(&format!("--{c}:{};", css(&self.cc(c))));
        }
        let sh = d.shadows.clone().unwrap_or_default().iter().map(|s| format!("{}px {}px {}px {}", s.x, s.y, s.blur, s.color.as_ref().map(css).unwrap_or("#0000".into()))).collect::<Vec<_>>().join(",");
        vars.push_str(&format!("--radius:{}px;--radius-sm:{}px;--border-w:{}px;--shadow:{};--font-display:'{}';--font-body:'{}';--font-mono:'{}';--w-display:{};--w-heading:{};--w-body:{};--w-label:{};--tr-label:{}em;--label-case:{};",
            self.th.radius, d.radius_sm.unwrap_or(self.th.radius * 0.5), d.border_w, if sh.is_empty() { "none".into() } else { sh }, self.th.role_font(true), self.th.role_font(false), self.th.mono, d.w_display, d.w_heading, d.w_body, d.w_label, d.tr_label, if d.label_upper { "uppercase" } else { "none" }));
        let markup = format!("<div class=\"skin skin-{comp}-{name} surface-{}\" style=\"{vars}width:{w}px\">{out}</div>", d.surface);
        let mut l = Layer::new(lid, Kind::Html { markup });
        l.tags.push(format!("skin-{comp}"));
        l.layout.w = Some(Len::px(w));
        let idx = self.add(Some(parent), l);
        let _ = si;
        Some(idx)
    }

    /// Default skin for a component kind from the design spec (`skin node=tile`) or the node (`skin=`).
    pub fn skin_for(&self, n: &Node) -> Option<String> {
        praw(n, "skin").filter(|s| s != "none" && s != "native").or_else(|| if praw(n, "skin").is_some() { None } else { self.th.d.skins.get(&n.kind).cloned() })
    }
}

/// Text arg helper visible to this module.
pub fn first_text(n: &Node) -> Option<String> {
    n.args.iter().find_map(|v| match v {
        Value::Str(s) if !s.starts_with('#') => Some(s.clone()),
        _ => None,
    })
}
