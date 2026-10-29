//! L0 principles as lint rules (design/PRINCIPLES.md). Measurable visual and motion rules for the
//! video medium; taste beyond them is the critic's job (`agentfx critique`).
//!
//! Static (document + motion IR): focal point per beat, one orchestrated move per beat, text density /
//! reading time, readable type size, consistency of treatment per role, exits faster than entrances,
//! stagger range, design spec validation (contrast, fonts, scale).
//! Measured (native layout samples, see `lint::measure_native`): scale fill (content too small in the
//! frame), small type in a big frame.
use crate::ast::{Document, Node, Span};
use crate::diag::Diag;
use crate::theme::Theme;
use crate::timing::Timeline;
use crate::values::{contrast_ratio, parse_dur, Rgba};
use std::collections::HashMap;

/// Words a viewer reads per second while also listening (on-screen text competes with narration).
pub const READ_WPS: f64 = 3.0;
/// Readable text at 1080p (HyperFrames' floor is 28 px = 2.6 % of the short side; ours for body copy).
pub const READ_MIN_PX: f64 = 28.0;

const EMPH: &[&str] = &["activate", "glow", "pulse", "emph", "highlight", "spotlight", "flash", "bounce", "shake", "callout", "done"];
const MOVES: &[&str] = &["enter", "move", "zoom", "pan", "exit", "draw", "dolly", "orbit", "swap"];
const TEXT_KINDS: &[&str] = &["text", "display", "h1", "h2", "h3", "body", "small", "label", "title", "card", "note", "quote", "list", "node", "stat", "steps", "panel", "lower3", "toast"];

fn words(s: &str) -> usize {
    s.split_whitespace().filter(|w| w.chars().any(|c| c.is_alphanumeric())).count()
}

fn texts_of(n: &Node) -> Vec<String> {
    let mut v = vec![];
    if TEXT_KINDS.contains(&n.kind.as_str()) {
        for a in &n.args {
            let r = a.raw();
            if !r.starts_with('#') {
                v.push(r);
            }
        }
        for k in ["sub", "title", "kicker", "label", "by"] {
            if let Some(x) = n.props.get(k) {
                v.push(x.raw());
            }
        }
    }
    v
}

fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a Node>) {
    for n in nodes {
        out.push(n);
        walk(&n.children, out);
    }
}

fn dur_prop(n: &Node, k: &str, fps: f64) -> Option<f64> {
    n.props.get(k).and_then(|v| parse_dur(&v.raw()).ok()).map(|d| d.secs(fps))
}

/// W_COLD_OPEN: the first keyframed change starts later than 2 frames into the video.
pub fn cold_open(ir: &crate::ir::Ir) -> Option<Diag> {
    if ir.tracks.is_empty() {
        return None;
    }
    let mut first: Option<f64> = None;
    for track in &ir.tracks {
        let mut track_first: Option<f64> = None;
        let keys = &track.keys;
        let mut previous: Option<&crate::ir::Key> = None;
        for key in keys {
            // a lone key that differs from the base is an instant set, not motion
            let changed = previous.is_some_and(|prev| format!("{:?}", prev.v) != format!("{:?}", key.v));
            if changed {
                let start = previous.map_or(key.t, |prev| prev.t);
                track_first = Some(match track_first {
                    Some(current) if current <= start => current,
                    _ => start,
                });
            }
            previous = Some(key);
        }
        if let Some(start) = track_first {
            first = Some(match first {
                Some(current) if current <= start => current,
                _ => start,
            });
        }
    }
    if let Some(first) = first {
        if first > 2.0 / ir.fps {
            return Some(Diag::warn("W_COLD_OPEN", format!("nothing moves until {:.2}s (frame {})", first, (first * ir.fps).round() as i64), "start motion on frame 0: begin the first move at 0s or add a slow camera drift from 0s", Span::default()));
        }
    }
    None
}

pub fn static_rules(doc: &Document, tl: &Timeline, th: &Theme) -> Vec<Diag> {
    let mut d = vec![];
    let scenes: Vec<&Node> = doc.nodes.iter().filter(|n| n.kind == "scene").collect();
    // role -> (scene, styling signature, span) for consistency
    let mut roles: HashMap<String, Vec<(String, String, Span)>> = HashMap::new();
    for (si, sn) in scenes.iter().enumerate() {
        let Some(st) = tl.scenes.get(si) else { continue };
        let mut all = vec![];
        walk(&sn.children, &mut all);
        // --- text density: on-screen words vs the time the scene gives to read them
        let screen_words: usize = all.iter().filter(|n| n.kind != "say" && n.kind != "at").flat_map(|n| texts_of(n)).map(|t| words(&t)).sum();
        let budget = (st.dur * READ_WPS * 0.5).max(10.0);
        if screen_words as f64 > budget {
            d.push(Diag::warn("W_TEXT_DENSITY", format!("scene `{}` puts {screen_words} words on screen in {:.1}s (readable while listening: about {:.0})", st.id, st.dur, budget), "cut words (one idea per frame, keywords not sentences) or give the scene more time; the narration carries the sentences", sn.span));
        }
        for n in &all {
            for t in texts_of(n) {
                let w = words(&t);
                let chars = t.chars().count();
                if w > 14 && !matches!(n.kind.as_str(), "quote") {
                    d.push(Diag::warn("W_WORDY", format!("`{}` text has {w} words: \"{}\"", n.kind, t.chars().take(50).collect::<String>()), "video type is keywords, not sentences: 6 to 10 words per line, the voice says the rest", n.span));
                } else if chars > 45 && !n.props.contains_key("w") && matches!(n.kind.as_str(), "display" | "h1" | "title") {
                    d.push(Diag::info("I_LINE_LENGTH", format!("headline of {chars} characters"), "keep display lines under about 45 characters (two lines at most)", n.span));
                }
            }
            // explicit small sizes on reading text
            if let Some(sz) = n.props.get("size").and_then(|v| crate::design::num(&v.raw())) {
                let wc: usize = texts_of(n).iter().map(|t| words(t)).sum();
                if sz < READ_MIN_PX && wc >= 3 {
                    d.push(Diag::warn("W_READ_SIZE", format!("`{}` sets size={sz} for {wc} words", n.kind), format!("text meant to be read needs >= {READ_MIN_PX} px at 1080p; cut words instead of shrinking type"), n.span));
                }
            }
            // role consistency: the same kind styled differently across scenes
            if TEXT_KINDS.contains(&n.kind.as_str()) || matches!(n.kind.as_str(), "chart" | "box") {
                let sig: Vec<String> = ["size", "color", "fill", "variant", "radius", "weight", "border", "shadow", "font"].iter().filter_map(|k| n.props.get(*k).map(|v| format!("{k}={}", v.raw()))).collect();
                roles.entry(n.kind.clone()).or_default().push((st.id.clone(), sig.join(" "), n.span));
            }
        }
        // --- events per anchor: focal points and simultaneous moves
        let mut at_groups: Vec<(String, Vec<&Node>, Span)> = vec![];
        for c in &all {
            if c.kind == "at" {
                let expr = c.args.first().map(|v| v.raw()).unwrap_or_default();
                at_groups.push((expr, c.children.iter().collect(), c.span));
            }
        }
        for (expr, verbs, span) in &at_groups {
            let targets = |set: &[&str]| -> Vec<String> {
                let mut v: Vec<String> = verbs.iter().filter(|x| set.contains(&x.kind.as_str()) && !x.props.contains_key("stagger") && !x.props.contains_key("delay")).filter_map(|x| x.args.first().map(|a| a.raw())).filter(|a| a.starts_with('#')).collect();
                v.sort();
                v.dedup();
                v
            };
            let em = targets(EMPH);
            if em.len() >= 3 {
                d.push(Diag::warn("W_FOCAL", format!("@{expr} emphasizes {} elements at once ({})", em.len(), em.join(" ")), "one focal point per beat: emphasize the element the narration names; for an ensemble moment stagger it (`@x+0.15s`) or emphasize their container", *span));
            }
            let mv = targets(MOVES);
            if mv.len() >= 4 {
                d.push(Diag::warn("W_BEAT_MOVES", format!("@{expr} starts {} moves at once ({})", mv.len(), mv.join(" ")), "one orchestrated move per beat: group them in a container and move that, or stagger (stagger=133ms, the measured item stagger)", *span));
            }
        }
        // element in= vs out= durations; stagger range
        for n in &all {
            let fps = tl.fps;
            if n.kind == "exit" {
                if let Some(o) = dur_prop(n, "dur", fps) {
                    let enter = th.durs.get("enter-dur").copied().unwrap_or(th.dur("enter"));
                    if o > enter * 1.2 && o > 0.45 {
                        d.push(Diag::warn("W_EXIT_SLOW", format!("exit takes {o:.2}s, longer than an entrance ({enter:.2}s)"), "exits accelerate and are shorter than entrances (measured 0.73x): use the default or dur<=0.3s", n.span));
                    }
                }
            }
            if let Some(s) = dur_prop(n, "stagger", fps).filter(|_| n.kind != "form") {
                if s < 0.03 || s > 0.5 {
                    d.push(Diag::warn("W_STAGGER", format!("stagger {s:.3}s is outside 30 to 500 ms"), "item staggers read at 60 to 165 ms (measured median 133 ms); beats at about 0.43 s", n.span));
                }
            }
        }
    }
    // consistency: same role, different explicit treatment in different scenes
    for (kind, v) in &roles {
        let mut sigs: Vec<&String> = v.iter().filter(|x| !x.1.is_empty()).map(|x| &x.1).collect();
        sigs.sort();
        sigs.dedup();
        let scenes_n = v.iter().map(|x| &x.0).collect::<std::collections::BTreeSet<_>>().len();
        if sigs.len() >= 2 && scenes_n >= 2 {
            let first = v.iter().find(|x| !x.1.is_empty()).unwrap();
            d.push(Diag::info("I_ROLE_TREATMENT", format!("`{kind}` is styled {} different ways across scenes ({})", sigs.len(), sigs.iter().take(3).map(|s| s.as_str()).collect::<Vec<_>>().join(" | ")), "same role, same treatment across the video: move the styling into the design spec (or the theme) instead of per element", first.2));
        }
    }
    d.extend(design_rules(th));
    d
}

/// Validate a design spec (L2): contrast of the color roles, fonts, type scale, family count.
pub fn design_rules(th: &Theme) -> Vec<Diag> {
    let mut d = vec![];
    if !th.d.loaded {
        return d;
    }
    let sp = Span::default();
    let bg = th.paint_avg(&th.bg).unwrap_or(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 });
    let c = |k: &str| th.colors.get(k).cloned();
    for (k, min) in [("text", 7.0), ("muted", 4.5), ("mint", 3.0)] {
        if let Some(col) = c(k) {
            let r = contrast_ratio(&col, &bg);
            if r < min {
                d.push(Diag::warn("W_DESIGN_CONTRAST", format!("design color `{k}` has {r:.1}:1 on the background (need {min}:1)"), "adjust the color role in the design spec (text >= 7:1, muted >= 4.5:1, accent text >= 3:1)", sp));
            }
        }
    }
    let manifest: serde_json::Value = std::fs::read_to_string(crate::run::home().join("assets/fonts/fonts.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let fams: Vec<String> = manifest["fonts"].as_array().into_iter().flatten().filter_map(|f| f["family"].as_str().map(|s| s.to_string())).collect();
    let mut used = vec![];
    for f in [th.d.display_font.clone(), th.d.body_font.clone(), Some(th.font.clone())].into_iter().flatten() {
        if !fams.is_empty() && !fams.contains(&f) {
            let names: Vec<&str> = fams.iter().map(|s| s.as_str()).collect();
            let hint = crate::values::suggest(&f, &names).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("families: {}", names.join(", ")));
            d.push(Diag::error("E_DESIGN_FONT", format!("font family `{f}` is not registered"), hint, sp));
        }
        if !used.contains(&f) {
            used.push(f);
        }
    }
    if used.len() > 2 {
        d.push(Diag::warn("W_DESIGN_FAMILIES", format!("{} type families ({})", used.len(), used.join(", ")), "at most two families (display + body) plus a mono", sp));
    }
    if let Some(r) = th.d.ratio {
        if !(1.125..=1.65).contains(&r) {
            d.push(Diag::warn("W_DESIGN_RATIO", format!("type ratio {r}"), "a modular scale between 1.125 and 1.618 keeps steps distinguishable and the display size sane", sp));
        }
    }
    if let Some(b) = th.size("body") {
        if b < READ_MIN_PX {
            d.push(Diag::warn("W_DESIGN_BODY", format!("body size {b}px"), format!("body text at 1080p needs >= {READ_MIN_PX} px (we recommend 36 to 44)"), sp));
        }
    }
    d
}

/// Content scale of one measured sample: (fill_w, fill_h, median text px) relative to the safe area.
/// `items` are the sample's measured boxes (lint::measure_native JSON).
pub fn sample_scale(sample: &serde_json::Value, w: f64, h: f64, safe: (f64, f64)) -> Option<(f64, f64, f64)> {
    let items: Vec<&serde_json::Value> = sample["items"].as_array().into_iter().flatten().filter(|it| !it["cap"].as_bool().unwrap_or(false)).collect();
    let texts: Vec<&&serde_json::Value> = items.iter().filter(|it| !it["ib"].as_bool().unwrap_or(false) && it["cls"].as_str().unwrap_or("").contains("t-text") && !it["text"].as_str().unwrap_or("").trim().is_empty()).collect();
    if items.len() < 2 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for it in &items {
        let (x, y, iw, ih) = (it["x"].as_f64().unwrap_or(0.0), it["y"].as_f64().unwrap_or(0.0), it["w"].as_f64().unwrap_or(0.0), it["h"].as_f64().unwrap_or(0.0));
        if iw * ih > 0.9 * w * h || iw <= 0.0 || ih <= 0.0 {
            continue; // full-frame backgrounds
        }
        x0 = x0.min(x.max(0.0));
        y0 = y0.min(y.max(0.0));
        x1 = x1.max((x + iw).min(w));
        y1 = y1.max((y + ih).min(h));
    }
    if x1 <= x0 {
        return None;
    }
    let sw = w * (1.0 - 2.0 * safe.0);
    let sh = h * (1.0 - 2.0 * safe.1);
    let mut fs: Vec<f64> = texts.iter().map(|it| it["fs"].as_f64().unwrap_or(0.0)).collect();
    fs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = if fs.is_empty() { 99.0 } else { fs[fs.len() / 2] };
    Some((((x1 - x0) / sw).min(1.0), ((y1 - y0) / sh).min(1.0), med))
}

/// Measured scale fill for a scene, judged on its most built-up sample (largest content area):
/// the hero fills at least 40 % of the frame's width or height, and reading text is not tiny while
/// the frame stays half empty (the S1 diagram problem: small elements in a big frame).
pub fn scale_rule(scene: &str, best: (f64, f64, f64), t: f64, short: f64) -> Option<Diag> {
    let (fw, fh, med) = best;
    let area = fw * fh;
    let scale = short / 1080.0;
    if fw.max(fh) < 0.40 {
        return Some(Diag::warn("W_SCALE_FILL", format!("content fills only {:.0} % x {:.0} % of the safe area in scene `{scene}`", fw * 100.0, fh * 100.0), "the hero of a shot fills at least 40 % of the frame: scale the group up (size=, w=, design `scale components=`), push in (`zoom #group`), or use fewer, larger elements", Span::default()).with_at(t));
    }
    if med < 30.0 * scale && area < 0.5 {
        return Some(Diag::warn("W_SCALE_FILL", format!("small elements in a big frame in scene `{scene}`: median text {med:.0}px, content covers {:.0} % of the safe area", area * 100.0), "make the elements larger (labels >= 34 px, sub-lines >= 28 px at 1080p) rather than adding more of them", Span::default()).with_at(t));
    }
    None
}
