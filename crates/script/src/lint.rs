//! Lint: static rules (no browser) + measured rules (layout measured once in Chrome at
//! sample times). Every diagnostic has a fix hint.
use crate::ast::{Document, Node, Span};
use crate::diag::{Diag, Severity};
use crate::ir::*;
use crate::theme::Theme;
use crate::timing::Timeline;
use crate::values::{contrast_ratio, parse_aspect, Paint, Rgba};
use std::collections::HashMap;

pub fn formats(doc: &Document, one: Option<String>, all: bool) -> Vec<String> {
    if let Some(f) = one {
        return vec![f];
    }
    let video = doc.nodes.iter().find(|n| n.kind == "video");
    let listed: Vec<String> = match video.and_then(|v| v.props.get("formats")) {
        Some(crate::ast::Value::List(l)) => l.iter().map(|v| v.raw()).collect(),
        Some(v) => vec![v.raw()],
        None => vec![],
    };
    if all || !listed.is_empty() {
        if listed.is_empty() {
            return vec!["16:9".into(), "9:16".into()];
        }
        return listed;
    }
    let size = video.and_then(|v| v.props.get("size")).map(|v| v.raw()).unwrap_or("1920x1080".into());
    let (w, h) = crate::values::parse_size(&size).unwrap_or((1920, 1080));
    let g = gcd(w, h);
    vec![format!("{}:{}", w / g, h / g)]
}
fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

fn walk_nodes<'a>(nodes: &'a [Node], out: &mut Vec<&'a Node>) {
    for n in nodes {
        out.push(n);
        walk_nodes(&n.children, out);
    }
}

pub fn static_rules(doc: &Document, tl: &Timeline, ir: &Ir) -> Vec<Diag> {
    let mut d = vec![];
    let mut dd = vec![];
    let th = Theme::from_doc(doc, &mut dd);
    let mut all = vec![];
    walk_nodes(&doc.nodes, &mut all);
    // house rule: no em-dashes in user-facing copy
    if th.no_emdash {
        for n in &all {
            for a in n.args.iter().chain(n.props.values()) {
                if a.raw().contains('\u{2014}') {
                    d.push(Diag::warn("W_EMDASH", "em-dash in user-facing copy", "replace \u{2014} with a comma, colon or period (house rule; disable with `theme` > `lint no-emdash=false`)", n.span));
                }
            }
        }
    }
    crate::textv2::lint_families(&all, &ir.fonts, &mut d);
    // events: outside their scene, cursor clicks too dense, zooms overlapping
    let scenes: Vec<&Node> = doc.nodes.iter().filter(|n| n.kind == "scene").collect();
    for (si, sn) in scenes.iter().enumerate() {
        let st = &tl.scenes[si];
        let (a, b) = (st.start, st.start + st.dur);
        let mut evs: Vec<(f64, &Node, Span)> = vec![];
        let mut say_ix = 0;
        for c in &sn.children {
            let (say, ats): (Option<usize>, Vec<&Node>) = if c.kind == "say" {
                let r = (Some(say_ix), c.children.iter().filter(|x| x.kind == "at").collect());
                say_ix += 1;
                r
            } else if c.kind == "at" {
                (None, vec![c])
            } else {
                (None, vec![])
            };
            for at in ats {
                let expr = at.args.first().map(|v| v.raw()).unwrap_or_default();
                if let Ok(t) = tl.anchor_time(si, say, &expr, a, at.span) {
                    if t < a - 1e-3 || t > b + 1e-3 {
                        d.push(Diag::error("E_EVENT_OUTSIDE", format!("event @{expr} resolves to {t:.2}s, outside scene `{}` ({a:.2}-{b:.2}s)", st.id), "anchor it to a word of this scene, or use a smaller offset", at.span).with_at(t));
                    }
                    for v in &at.children {
                        evs.push((t, v, at.span));
                    }
                }
            }
        }
        evs.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let mut last_click: Option<f64> = None;
        let mut last_zoom: Option<(f64, f64)> = None;
        for (t, v, span) in &evs {
            match v.kind.as_str() {
                "click" | "dblclick" => {
                    if let Some(lc) = last_click {
                        if t - lc < 0.45 {
                            d.push(Diag::warn("W_TIMING_CLICKS", format!("two clicks {:.2}s apart; the cursor cannot travel between them", t - lc), "anchor the second click to a later word or add `@x+0.5s`", *span).with_at(*t));
                        }
                    }
                    last_click = Some(*t);
                }
                "zoom" | "pan" => {
                    if let Some((lz, dur)) = last_zoom {
                        if t - lz < dur - 1e-3 {
                            d.push(Diag::warn("W_TIMING_CONFLICT", format!("zoom starts {:.2}s after the previous zoom, which runs {:.2}s; the later one cuts it", t - lz, dur), "move the second zoom to a later anchor or shorten dur=", *span).with_at(*t));
                        }
                    }
                    let dur = v.props.get("dur").and_then(|x| crate::values::parse_dur(&x.raw()).ok()).map(|x| x.secs(tl.fps)).unwrap_or(1.0);
                    last_zoom = Some((*t, dur));
                }
                _ => {}
            }
        }
        // scene with narration but nothing happening on screen for long stretches
        if evs.is_empty() && st.dur > 12.0 && sn.children.iter().any(|c| c.kind == "say") {
            d.push(Diag::warn("W_STATIC_SCENE", format!("scene `{}` runs {:.1}s without any event", st.id, st.dur), "anchor a few verbs to narration words ([[word]] + @word)", sn.span));
        }
    }
    for s in &tl.scenes {
        for say in &s.says {
            if say.estimated {
                d.push(Diag { severity: Severity::Info, ..Diag::warn("I_TTS_ESTIMATED", format!("timing for `{}` is estimated (no TTS)", say.key), "run with network/TTS for real word timings", say.span) });
            }
        }
    }
    // static layout: min font, contrast on known fills, absolute boxes outside the frame
    let scale = ir.w.min(ir.h) as f64 / 1080.0;
    fn rec(l: &Layer, fills: &mut Vec<Option<Rgba>>, in_browser: bool, th: &Theme, ir: &Ir, scale: f64, d: &mut Vec<Diag>) {
        // translucent fills composite over the effective background below them
        let fill = l.style.fill.as_ref().and_then(|p| match p {
            Paint::Image { .. } => None,
            p => th.paint_avg(p),
        }).filter(|c| c.a > 0.02 && !l.style.text_fill).map(|c| match fills.iter().rev().find_map(|f| f.clone()) {
            Some(under) if c.a < 0.999 => Rgba { r: c.r * c.a + under.r * (1.0 - c.a), g: c.g * c.a + under.g * (1.0 - c.a), b: c.b * c.a + under.b * (1.0 - c.a), a: 1.0 },
            _ => c,
        });
        let pushed = fill.is_some();
        if pushed {
            fills.push(fill.clone());
        }
        // world canvases (X9) are framed by the camera director: no frame/min-size checks inside
        let ib = in_browser || l.tags.iter().any(|t| t == "browser" || t == "world");
        // knocked-out text is a hole, not ink: no contrast to read
        if let (Kind::Text { .. } | Kind::Counter { .. }, false, false) = (&l.kind, ib, l.style.knockout) {
            if let Some(sz) = l.style.size {
                if sz * scale < th.min_font * scale - 1e-6 {
                    d.push(Diag::warn("W_MIN_FONT", format!("text `{}` is {sz}px, below the minimum {}px", l.name.clone().unwrap_or(l.id.clone()), th.min_font), format!("use size>={} or a larger style (body/h3)", th.min_font), Span::default()).with_id(l.name.clone().unwrap_or(l.id.clone())));
                }
            }
            let (col, bgi) = if l.style.text_fill { (fill.clone(), fills.len().saturating_sub(2)) } else { (l.style.color.clone(), fills.len().saturating_sub(1)) };
            if let (Some(c), Some(Some(bg))) = (&col, fills.get(bgi)) {
                // a text stroke (outline) reads against the background too
                let r = contrast_ratio(c, bg).max(crate::textv2::stroke_of(l).and(l.style.stroke.as_ref()).map_or(0.0, |s| contrast_ratio(s, bg)));
                if r < th.min_contrast {
                    d.push(Diag::warn("W_CONTRAST", format!("text `{}` has contrast {r:.1}:1 on its background", l.name.clone().unwrap_or(l.id.clone())), format!("need >= {}:1; use color=$text or a darker/lighter fill", th.min_contrast), Span::default()).with_id(l.name.clone().unwrap_or(l.id.clone())));
                }
            }
        }
        // Bone offsets are local to their parent pivot, not frame coordinates;
        // lights may sit outside the frame (a low sun, a light behind the edge)
        // caption pack internals are placed in their phrase's box (the caption lint checks the frame)
        if l.layout.pos == Pos::Abs && !ib && !l.tags.iter().any(|t| t == "presenter-bone" || t == "cap-local") && l.style.light.is_none() {
            let out = |v: &Option<Len>, full: f64| v.as_ref().map(|x| match x.unit {
                "%" => x.v < -0.5 || x.v > 100.5,
                "px" => x.v < -1.0 || x.v > full + 1.0,
                _ => false,
            }).unwrap_or(false);
            if out(&l.layout.x, ir.w as f64) || out(&l.layout.y, ir.h as f64) {
                d.push(Diag::error("E_OUT_OF_FRAME", format!("`{}` is placed outside the frame", l.name.clone().unwrap_or(l.id.clone())), "use place=... or x/y within 0..100%", Span::default()).with_id(l.name.clone().unwrap_or(l.id.clone())));
            }
        }
        for c in &l.children {
            rec(c, fills, ib, th, ir, scale, d);
        }
        if pushed {
            fills.pop();
        }
    }
    rec(&ir.root, &mut vec![th.paint_avg(&th.bg)], false, &th, ir, scale, &mut d);
    d.extend(glow_cost(ir));
    // L0 principles (design/PRINCIPLES.md)
    d.extend(crate::principles::static_rules(doc, tl, &th));
    d.extend(crate::principles::cold_open(ir));
    d
}

/// More glow/blur layers than this in one scene is a cost smell (one blur pass each per frame).
pub const GLOW_LAYERS_MAX: usize = 8;

/// W_GLOW_COST: every glow/blur layer is its own blur pass whenever it changes (every frame under a
/// moving camera). Nests multiply them: a glow on a nest child is one pass per level.
pub fn glow_cost(ir: &Ir) -> Vec<Diag> {
    let blurs: std::collections::HashSet<&str> = ir.tracks.iter().filter(|t| matches!(t.ch, Ch::Glow | Ch::Blur) && (t.base.n() > 0.0 || t.keys.iter().any(|k| k.v.n() > 0.0))).map(|t| t.layer.as_str()).collect();
    let mut d = vec![];
    let scenes: Vec<&Layer> = ir.root.children.iter().filter(|l| l.tags.iter().any(|t| t == "scene")).collect();
    for sc in scenes {
        let mut n = 0;
        let mut first: Vec<String> = vec![];
        sc.walk(&mut |l| {
            if blurs.contains(l.id.as_str()) {
                n += 1;
                if first.len() < 3 {
                    first.push(l.name.clone().map(|s| format!("#{s}")).unwrap_or(l.id.clone()));
                }
            }
        });
        if n > GLOW_LAYERS_MAX {
            let scene = sc.id.trim_start_matches("scene-");
            d.push(Diag::warn("W_GLOW_COST", format!("scene `{scene}` has {n} glow/blur layers ({}, ...): each is a separate blur pass per changed frame", first.join(", ")), "put one glow=/blur= on their common parent (a nest, a layer) instead of on every child", Span::default()));
        }
    }
    d
}

pub fn sample_times(tl: &Timeline) -> Vec<f64> {
    let mut v = vec![];
    for s in &tl.scenes {
        v.push(s.start + (1.2f64).min(s.dur * 0.4));
        for say in &s.says {
            v.push((say.end - 0.25).max(say.start));
        }
        v.push(s.start + s.dur - 0.15);
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v.dedup_by(|a, b| (*a - *b).abs() < 0.3);
    v
}

fn parse_rgb(s: &str) -> Option<Rgba> {
    let inner = s.split_once('(')?.1.trim_end_matches(')');
    let p: Vec<f64> = inner.split([',', ' ', '/']).filter(|x| !x.is_empty()).filter_map(|x| x.trim().parse().ok()).collect();
    if p.len() < 3 {
        return None;
    }
    Some(Rgba { r: p[0] / 255.0, g: p[1] / 255.0, b: p[2] / 255.0, a: *p.get(3).unwrap_or(&1.0) })
}

pub fn measured_rules(m: &serde_json::Value, ir: &Ir, tl: &Timeline, doc: &Document) -> Vec<Diag> {
    let mut d = vec![];
    let mut dd = vec![];
    let th = Theme::from_doc(doc, &mut dd);
    let mut scale_best: std::collections::BTreeMap<String, ((f64, f64, f64), f64)> = Default::default();
    let (w, h) = (ir.w as f64, ir.h as f64);
    // text layers with a stroke: the outline color counts for contrast
    let mut stroked: std::collections::HashMap<String, Rgba> = std::collections::HashMap::new();
    ir.root.walk(&mut |l| {
        if let (Some(_), Some(c)) = (crate::textv2::stroke_of(l), &l.style.stroke) {
            stroked.insert(l.id.clone(), c.clone());
        }
    });
    let (sx, sy) = (th.safe.0 * w, th.safe.1 * h);
    let scale = w.min(h) / 1080.0;
    for sample in m["samples"].as_array().into_iter().flatten() {
        let t = sample["t"].as_f64().unwrap_or(0.0);
        let scene = tl.scenes.get(tl.scene_at(t)).map(|s| s.id.clone()).unwrap_or_default();
        let items: Vec<&serde_json::Value> = sample["items"].as_array().into_iter().flatten().collect();
        if let Some(st) = crate::principles::sample_scale(sample, w, h, th.safe) {
            let e = scale_best.entry(scene.clone()).or_insert((st, t));
            if st.0 * st.1 > e.0 .0 * e.0 .1 {
                *e = (st, t);
            }
        }
        let name = |it: &serde_json::Value| it["name"].as_str().map(|s| s.to_string()).unwrap_or_else(|| it["id"].as_str().unwrap_or("").to_string());
        let texts: Vec<&&serde_json::Value> = items.iter().filter(|it| {
            let c = it["cls"].as_str().unwrap_or("");
            c.contains("t-text") && !c.contains("t-piece") && !it["ib"].as_bool().unwrap_or(false)
        }).collect();
        for it in &texts {
            let (x, y, iw, ih) = (it["x"].as_f64().unwrap_or(0.0), it["y"].as_f64().unwrap_or(0.0), it["w"].as_f64().unwrap_or(0.0), it["h"].as_f64().unwrap_or(0.0));
            let id = name(it);
            let txt = it["text"].as_str().unwrap_or("").replace('\n', " ");
            let cap = it["cap"].as_bool().unwrap_or(false);
            let collapsing = it["cls"].as_str().unwrap_or("").contains("t-codeline");
            if it["overflowX"].as_f64().unwrap_or(0.0) > 2.0 || (!collapsing && it["overflowY"].as_f64().unwrap_or(0.0) > 2.0) {
                d.push(Diag::error("E_OVERFLOW", format!("text \"{txt}\" overflows its box in scene `{scene}`"), "shorten the text, give it more room (w=), or add `fit`", Span::default()).with_id(id.clone()).with_at(t));
            }
            if x < -2.0 || y < -2.0 || x + iw > w + 2.0 || y + ih > h + 2.0 {
                d.push(Diag::error("E_OUT_OF_FRAME", format!("text \"{txt}\" is cut off by the frame edge in scene `{scene}` ({x:.0},{y:.0} {iw:.0}x{ih:.0})"), "reduce its size or place it inside the safe area", Span::default()).with_id(id.clone()).with_at(t));
            } else if !cap && (x < sx - 2.0 || y < sy * 0.5 - 2.0 || x + iw > w - sx + 2.0 || y + ih > h - sy * 0.5 + 2.0) {
                d.push(Diag::warn("W_SAFE_AREA", format!("text \"{txt}\" leaves the safe area in scene `{scene}`"), "keep text inside the safe area (theme layout safe=6%); use place= or a flow container", Span::default()).with_id(id.clone()).with_at(t));
            }
            let fs = it["fs"].as_f64().unwrap_or(99.0);
            if fs < th.min_font * scale - 0.5 && !txt.trim().is_empty() {
                d.push(Diag::warn("W_MIN_FONT", format!("text \"{txt}\" renders at {fs:.0}px (min {:.0}px) in scene `{scene}`", th.min_font * scale), "increase size= or zoom in further", Span::default()).with_id(id.clone()).with_at(t));
            }
            if let (Some(c), Some(bg)) = (it["color"].as_str().and_then(parse_rgb), it["bg"].as_str().and_then(parse_rgb)) {
                if bg.a > 0.9 && c.a > 0.5 {
                    let stroke = stroked.get(it["id"].as_str().unwrap_or("")).map_or(0.0, |s| contrast_ratio(s, &bg));
                    let r = contrast_ratio(&c, &bg).max(stroke);
                    if r < th.min_contrast {
                        d.push(Diag::warn("W_CONTRAST", format!("text \"{txt}\" contrast {r:.1}:1 in scene `{scene}`"), format!("need >= {}:1", th.min_contrast), Span::default()).with_id(id.clone()).with_at(t));
                    }
                }
            }
        }
        for l in sample["links"].as_array().into_iter().flatten() {
            let id = l["name"].as_str().unwrap_or(l["id"].as_str().unwrap_or("")).to_string();
            let (x, y, lw, lh) = (l["x"].as_f64().unwrap_or(0.0), l["y"].as_f64().unwrap_or(0.0), l["w"].as_f64().unwrap_or(0.0), l["h"].as_f64().unwrap_or(0.0));
            if l["d"].as_f64().unwrap_or(0.0) == 0.0 {
                d.push(Diag::error("E_LINK_GEOMETRY", format!("connector `{id}` has no path in scene `{scene}` (endpoints not found)"), "both ends must be ids of visible elements in the same scene", Span::default()).with_id(id.clone()).with_at(t));
            } else if x < -2.0 || y < -2.0 || x + lw > w + 2.0 || y + lh > h + 2.0 {
                d.push(Diag::error("E_OUT_OF_FRAME", format!("connector `{id}` leaves the frame in scene `{scene}`"), "check that both ends are on screen in this format", Span::default()).with_id(id.clone()).with_at(t));
            } else if l["len"].as_f64().unwrap_or(99.0) < 40.0 {
                d.push(Diag::warn("W_LINK_SHORT", format!("connector `{id}` is only {:.0}px long in scene `{scene}`", l["len"].as_f64().unwrap_or(0.0)), "increase the gap between the two elements (gap=) so the arrow reads", Span::default()).with_id(id.clone()).with_at(t));
            }
        }
        // named boxes (nodes, panels, charts) leaving the frame; texts are checked above
        for it in &items {
            let c = it["cls"].as_str().unwrap_or("");
            if c.contains("t-text") || it["ib"].as_bool().unwrap_or(false) || it["name"].is_null() {
                continue;
            }
            let (x, y, iw, ih) = (it["x"].as_f64().unwrap_or(0.0), it["y"].as_f64().unwrap_or(0.0), it["w"].as_f64().unwrap_or(0.0), it["h"].as_f64().unwrap_or(0.0));
            if iw > 0.0 && ih > 0.0 && iw < w * 0.98 && ih < h * 0.98 && (x < -4.0 || y < -4.0 || x + iw > w + 4.0 || y + ih > h + 4.0) {
                d.push(Diag::error("E_OUT_OF_FRAME", format!("`#{}` is cut off by the frame edge in scene `{scene}` ({x:.0},{y:.0} {iw:.0}x{ih:.0})", name(it)), "make it smaller (w=, scale components=), reduce gap=, or stack it differently in this format (when portrait)", Span::default()).with_id(name(it)).with_at(t));
            }
        }
        // overlaps between unrelated visible texts
        for (i, a) in texts.iter().enumerate() {
            for b in texts.iter().skip(i + 1) {
                let r = |v: &serde_json::Value| (v["x"].as_f64().unwrap_or(0.0), v["y"].as_f64().unwrap_or(0.0), v["w"].as_f64().unwrap_or(0.0), v["h"].as_f64().unwrap_or(0.0));
                let (ax, ay, aw, ah) = r(a);
                let (bx, by, bw, bh) = r(b);
                let ix = (ax + aw).min(bx + bw) - ax.max(bx);
                let iy = (ay + ah).min(by + bh) - ay.max(by);
                if ix <= 0.0 || iy <= 0.0 {
                    continue;
                }
                let contains = |p: (f64, f64, f64, f64), q: (f64, f64, f64, f64)| q.0 >= p.0 - 1.0 && q.1 >= p.1 - 1.0 && q.0 + q.2 <= p.0 + p.2 + 1.0 && q.1 + q.3 <= p.1 + p.3 + 1.0;
                if contains((ax, ay, aw, ah), (bx, by, bw, bh)) || contains((bx, by, bw, bh), (ax, ay, aw, ah)) {
                    continue;
                }
                let small = (aw * ah).min(bw * bh).max(1.0);
                if ix * iy / small > 0.15 {
                    d.push(Diag::warn("W_OVERLAP", format!("\"{}\" overlaps \"{}\" in scene `{scene}`", a["text"].as_str().unwrap_or(""), b["text"].as_str().unwrap_or("")), "separate them with a stack/row, or time one to exit first", Span::default()).with_id(name(a)).with_at(t));
                }
            }
        }
    }
    for (scene, (st, t)) in &scale_best {
        d.extend(crate::principles::scale_rule(scene, *st, *t, w.min(h)));
    }
    let _ = parse_aspect("16:9");
    d
}

pub fn dedup(v: Vec<Diag>) -> Vec<Diag> {
    let mut out: Vec<Diag> = vec![];
    for d in v {
        if !out.iter().any(|o| o.code == d.code && o.message == d.message && o.line == d.line && o.format == d.format) {
            out.push(d);
        }
    }
    out.sort_by(|a, b| a.severity.cmp(&b.severity));
    out
}

/// `lint --measure` without a browser (S1): the same sample JSON the Chrome measure produced,
/// from the native layout (Blitz) and the motion IR's transforms at each sample time
/// (layout/paint channels re-resolved at t, 3D projected and linearized like direct mode).
pub fn measure_native(ir: &Ir, nat: &mut crate::native::Native, times: &[f64]) -> serde_json::Value {
    use crate::motion::{self, Chans};
    let ch = Chans::new(ir);
    // load-time hooks the measure needs: fit (font shrink) and links (paths between boxes)
    let fits: Vec<String> = ir.hooks.iter().filter_map(|h| if let Hook::Fit { layer } = h { Some(layer.clone()) } else { None }).collect();
    let mut fit_sizes: HashMap<String, f64> = HashMap::new();
    for id in &fits {
        let Some(mut fs) = nat.text_style(id).map(|s| s.0) else { continue };
        for _ in 0..40 {
            let (ox, oy) = nat.overflow(id);
            if ox <= 1.0 && oy <= 1.0 {
                break;
            }
            fs *= 0.95;
            let mut snap = nat.snap().clone();
            snap.font_size.insert(id.clone(), fs);
            nat.set_snap(&snap, std::slice::from_ref(id));
        }
        fit_sizes.insert(id.clone(), fs);
    }
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut layers: Vec<&Layer> = vec![];
    fn walk<'a>(l: &'a Layer, parent: &mut HashMap<String, String>, out: &mut Vec<&'a Layer>) {
        out.push(l);
        for c in &l.children {
            parent.insert(c.id.clone(), l.id.clone());
            walk(c, parent, out);
        }
    }
    walk(&ir.root, &mut parent, &mut layers);
    let dyn_layers: Vec<String> = ir.tracks.iter().filter(|t| matches!(t.ch.stage(), Stage::Layout) || matches!(t.ch, Ch::Color | Ch::Fill)).map(|t| t.layer.clone()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    let by_id: HashMap<&str, &Layer> = layers.iter().map(|l| (l.id.as_str(), *l)).collect();
    let in_tag = |id: &str, tag: &str| -> bool {
        let mut cur = Some(id.to_string());
        while let Some(c) = cur {
            if by_id.get(c.as_str()).is_some_and(|l| l.tags.iter().any(|t| t == tag)) {
                return true;
            }
            cur = parent.get(&c).cloned();
        }
        false
    };
    let bg_of = |id: &str| -> Option<String> {
        let mut cur = Some(id.to_string());
        while let Some(c) = cur {
            if let Some(l) = by_id.get(c.as_str()) {
                match &l.style.fill {
                    Some(crate::values::Paint::Solid(crate::values::ColorRef::Lit(c))) if c.a > 0.0 && !l.style.text_fill => {
                        return Some(format!("rgba({}, {}, {}, {})", (c.r * 255.0).round(), (c.g * 255.0).round(), (c.b * 255.0).round(), c.a));
                    }
                    Some(p) if !l.style.text_fill && !matches!(p, crate::values::Paint::Solid(_)) => return Some("img:gradient".into()),
                    _ => {}
                }
            }
            cur = parent.get(&c).cloned();
        }
        None
    };
    let text_of = |l: &Layer, t: f64| -> String {
        match &l.kind {
            Kind::Text { runs } => runs.iter().map(|r| r.text.as_str()).collect::<String>(),
            Kind::Counter { prefix, suffix } => format!("{prefix}{}{suffix}", crate::ir::counter_text(l, ch.val(&l.id, Ch::Count, t).round() as i64)),
            _ => {
                let mut s = String::new();
                for c in &l.children {
                    if let Kind::Text { runs } = &c.kind {
                        s.push_str(&runs.iter().map(|r| r.text.as_str()).collect::<String>());
                        s.push(' ');
                    }
                }
                s.trim().to_string()
            }
        }
    };
    let mut samples = vec![];
    let base_snap = nat.snap().clone();
    for &t in times {
        let mut snap = crate::native::snap_at(ir, t);
        snap.font_size = base_snap.font_size.clone();
        nat.set_snap(&snap, &dyn_layers);
        let rect_now: HashMap<String, [f64; 4]> = layers.iter().filter_map(|l| nat.rect(&l.id).map(|r| (l.id.clone(), r))).collect();
        // magic deltas and links from the base layout (load-time hooks)
        let mut magic = HashMap::new();
        for h in &ir.hooks {
            if let Hook::Magic { from, to } = h {
                if let (Some(a), Some(b)) = (rect_now.get(from), rect_now.get(to)) {
                    let ca = (a[0] + a[2] / 2.0, a[1] + a[3] / 2.0);
                    let cb = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
                    magic.insert(to.clone(), (ca.0 - cb.0, ca.1 - cb.1, a[2] / b[2].max(1.0), a[3] / b[3].max(1.0)));
                    magic.insert(from.clone(), (cb.0 - ca.0, cb.1 - ca.1, b[2] / a[2].max(1.0), b[3] / a[3].max(1.0)));
                }
            }
        }
        let rf = |id: &str| rect_now.get(id).copied();
        let world = motion::world(ir, &ch, &rf, &magic, t);
        let mut items = vec![];
        for l in &layers {
            let is_text = l.tags.iter().any(|t| t == "text");
            if !is_text && l.name.is_none() {
                continue;
            }
            let Some(r) = rect_now.get(&l.id) else { continue };
            let Some((m, op)) = world.get(&l.id) else { continue };
            if *op < 0.5 || (r[2] == 0.0 && r[3] == 0.0) {
                continue;
            }
            let b = motion::a2_bbox(m, *r);
            let (fs, col, nowrap) = nat.text_style(&l.id).unwrap_or((16.0, [1.0, 1.0, 1.0, 1.0], false));
            let col = if ch.has(&l.id, Ch::Color) {
                match ch.tv(&l.id, Ch::Color, t) {
                    TV::C(c) => [c.r, c.g, c.b, c.a],
                    _ => col,
                }
            } else {
                col
            };
            let (ox, oy) = nat.overflow(&l.id);
            let mut cls: Vec<String> = l.tags.iter().map(|t| format!("t-{t}")).collect();
            cls.push("L".into());
            let txt: String = if is_text { text_of(l, t).chars().take(60).collect() } else { String::new() };
            // under a moving scene camera (zoom/pan/dolly): frame checks do not apply (intended crop)
            let cam = {
                let mut cur = parent.get(&l.id).cloned();
                let mut moved = false;
                while let Some(c) = cur {
                    if c.starts_with("world-") {
                        let sc = ch.val(&c, Ch::Scale, t);
                        moved = (sc - 1.0).abs() > 0.01 || ch.val(&c, Ch::X, t).abs() > 1.0 || ch.val(&c, Ch::Y, t).abs() > 1.0;
                        break;
                    }
                    cur = parent.get(&c).cloned();
                }
                moved
            };
            let cam = cam || in_tag(&l.id, "world");
            items.push(serde_json::json!({"id": l.id, "name": l.name, "cls": cls.join(" "), "ib": in_tag(&l.id, "browser") || cam, "cam": cam, "cap": in_tag(&l.id, "caption"),
                "x": b[0], "y": b[1], "w": b[2], "h": b[3], "fs": fs * motion::a2_scale(m),
                "color": format!("rgba({}, {}, {}, {})", (col[0] * 255.0).round(), (col[1] * 255.0).round(), (col[2] * 255.0).round(), col[3]),
                "bg": bg_of(&l.id), "overflowX": if nowrap { 0.0 } else { ox }, "overflowY": oy, "text": txt}));
        }
        let mut links = vec![];
        for h in &ir.hooks {
            if let Hook::Link { path, a, b, curve } = h {
                let Some((m, op)) = world.get(path) else { continue };
                if *op < 0.5 {
                    continue;
                }
                let find = |name: &str| -> Option<[f64; 4]> {
                    // same scene first (the path's scene root = the ancestor under the stage)
                    let scene_of = |id: &str| -> Option<String> {
                        let mut cur = Some(id.to_string());
                        while let Some(c) = cur {
                            if parent.get(&c).map(|p| p == "stage").unwrap_or(false) {
                                return Some(c);
                            }
                            cur = parent.get(&c).cloned();
                        }
                        None
                    };
                    let sc = scene_of(path);
                    let cands: Vec<&&Layer> = layers.iter().filter(|l| l.name.as_deref() == Some(name)).collect();
                    let pick = cands.iter().find(|l| scene_of(&l.id) == sc).or(cands.first());
                    pick.and_then(|l| rect_now.get(&l.id).copied())
                };
                let (ra, rb) = (find(a), find(b));
                let name = by_id.get(path.as_str()).and_then(|l| l.name.clone());
                match (ra, rb) {
                    (Some(ra), Some(rb)) => {
                        let d = crate::reelir::link_path(ra, rb, *curve);
                        // length + bounds of the main segment (before the arrowhead)
                        let s = (ra[0] + ra[2] / 2.0, ra[1] + ra[3] / 2.0);
                        let e = (rb[0] + rb[2] / 2.0, rb[1] + rb[3] / 2.0);
                        let nums: Vec<f64> = d.split(|c: char| c.is_ascii_alphabetic() || c == ' ').filter_map(|x| x.parse().ok()).collect();
                        let pts: Vec<(f64, f64)> = nums.chunks(2).filter(|c| c.len() == 2).map(|c| motion::a2_apply(m, c[0], c[1])).collect();
                        let x0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
                        let y0 = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
                        let x1 = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
                        let y1 = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
                        let main: Vec<f64> = nums.iter().take(if *curve { 8 } else { 4 }).copied().collect();
                        let len = if main.len() >= 4 { ((main[main.len() - 2] - main[0]).powi(2) + (main[main.len() - 1] - main[1]).powi(2)).sqrt() * motion::a2_scale(m) } else { 0.0 };
                        let _ = (s, e);
                        links.push(serde_json::json!({"id": path, "name": name, "x": x0, "y": y0, "w": x1 - x0, "h": y1 - y0, "len": len, "d": d.len()}));
                    }
                    _ => links.push(serde_json::json!({"id": path, "name": name, "x": 0, "y": 0, "w": 0, "h": 0, "len": 0, "d": 0})),
                }
            }
        }
        samples.push(serde_json::json!({"t": t, "items": items, "links": links}));
    }
    nat.set_snap(&base_snap, &dyn_layers);
    let _ = fit_sizes;
    serde_json::json!({"samples": samples})
}
