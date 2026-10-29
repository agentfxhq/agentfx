//! Time effects (X3): `slitscan`, `echo`, `timedisplace` (renderer: reel-ir node `time`,
//! the subtree evaluated at other times) and `freeze` (a hold remap, baked like `timemap`).
//! Span (`span=` / `dt=`) is the track channel `time-span` -> reel-ir `time.span`: later
//! verbs on the same layer animate it (`slitscan #g span=0 dur=0.2s` snaps it sharp).
use crate::ast::Node;
use crate::ir::*;
use crate::lower::*;
use crate::values::*;
use crate::events::Target;
use serde::Serialize;

/// Static part of a layer's time effect (positions are fractions of the layer's box,
/// resolved to screen coordinates by `reelir.rs`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TimeFxSpec {
    Slitscan { axis: String, band: f64, reverse: bool },
    Echo { count: u32, decay: f64, blend: String, tint: Vec<Rgba> },
    Displace { map: MapSpec, levels: u32 },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum MapSpec {
    /// center as box fractions, radius in px (None = to the farthest box corner)
    Radial { cx: f64, cy: f64, radius: Option<f64> },
    /// direction of increasing delay, degrees (0 = left to right)
    Linear { angle: f64 },
    Noise { scale: f64, seed: u64 },
    /// layer id (resolved to its node; the layer becomes a hidden map source)
    Layer { id: String },
}

const SLIT_PROPS: &[&str] = &["axis", "span", "band", "dir", "dur", "ease"];
const ECHO_PROPS: &[&str] = &["n", "dt", "decay", "blend", "colors", "color", "dur", "ease"];
const DISP_PROPS: &[&str] = &["map", "span", "levels", "center", "radius", "angle", "scale", "seed", "dur", "ease"];
const FREEZE_PROPS: &[&str] = &["for"];
/// Above this many subtree renders per frame the effect is slow (lint W_TIME_COST).
const RENDERS_WARN: f64 = 400.0;

impl<'a> Ctx<'a> {
    fn time_props(&mut self, n: &Node, ok: &[&str]) {
        for k in n.props.keys() {
            if !ok.contains(&k.as_str()) {
                let hint = suggest(k, ok).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("`{}` takes: {}", n.kind, ok.join(" ")));
                self.warn("W_TIME_PROP", format!("{}: unknown prop `{k}`", n.kind), hint, n.span);
            }
        }
    }

    /// `slitscan #g axis=y span=1.2s band=4 [dir=down|up] [dur= ease=]`,
    /// `echo #g n=6 dt=60ms decay=0.6 blend=normal|add|multiply|screen colors=#f00,#00f`,
    /// `timedisplace #g map=radial|linear|noise|#layer span=0.8s levels=24 ...`,
    /// `freeze #g [for=1s]`.
    pub fn time_verb(&mut self, e: &Ev) {
        let n = e.node.clone();
        let t = e.t;
        let k = n.kind.as_str();
        let Some(Target::El(i)) = self.target(e, true) else {
            if self.target(e, false).is_some() {
                self.err("E_TARGET", format!("`{k}` needs an element (a layer subtree)"), format!("write `{k} #group ...`"), n.span);
            }
            return;
        };
        if k == "freeze" {
            self.time_props(&n, FREEZE_PROPS);
            let id = self.lid(i);
            let hold = self.dur(&n, "for");
            let m = self.remaps.entry(id).or_default();
            m.push(t, vec![(0.0, 0.0, Ease::Linear)], 0.0);
            if let Some(h) = hold {
                if h > 0.0 {
                    m.push(t + h, vec![], 1.0);
                }
            }
            return;
        }
        let (spec, span_key, ok) = match k {
            "slitscan" => {
                let axis = praw_s(&n, "axis").unwrap_or_else(|| "y".into());
                if !matches!(axis.as_str(), "x" | "y") {
                    let h = suggest(&axis, &["x", "y"]).map(|s| format!("did you mean `{s}`?")).unwrap_or("axis=y (rows, top to bottom) or axis=x (columns)".into());
                    self.err("E_VALUE", format!("slitscan axis={axis}"), h, n.span);
                    return;
                }
                let band = self.num(&n, "band").unwrap_or(4.0);
                if !(1.0..=64.0).contains(&band) {
                    self.err("E_VALUE", format!("slitscan band={band}: px per time sample, 1..64"), "write `band=4` (smaller = smoother, slower)", n.span);
                    return;
                }
                let dir = praw_s(&n, "dir").unwrap_or_else(|| "down".into());
                let reverse = match dir.as_str() {
                    "down" | "right" => false,
                    "up" | "left" => true,
                    x => {
                        let h = suggest(x, &["down", "up", "right", "left"]).map(|s| format!("did you mean `{s}`?")).unwrap_or("dir=down|up (axis y) or right|left (axis x): where the delay grows".into());
                        self.err("E_VALUE", format!("slitscan dir={x}"), h, n.span);
                        return;
                    }
                };
                (TimeFxSpec::Slitscan { axis, band, reverse }, "span", SLIT_PROPS)
            }
            "echo" => {
                let count = self.num(&n, "n").unwrap_or(6.0);
                if !(1.0..=32.0).contains(&count) || count.fract() != 0.0 {
                    self.err("E_VALUE", format!("echo n={count}: whole number of copies, 1..32"), "write `n=6`", n.span);
                    return;
                }
                let decay = self.num(&n, "decay").unwrap_or(0.6);
                if !(decay > 0.0 && decay <= 1.0) {
                    self.err("E_VALUE", format!("echo decay={decay}: opacity factor per copy in (0, 1]"), "write `decay=0.6`", n.span);
                    return;
                }
                let blend = praw_s(&n, "blend").unwrap_or_else(|| "normal".into());
                let blend = match blend.as_str() {
                    "normal" => "normal",
                    "add" | "plus" => "plus",
                    "multiply" => "multiply",
                    "screen" => "screen",
                    x => {
                        let h = suggest(x, &["normal", "add", "multiply", "screen"]).map(|s| format!("did you mean `{s}`?")).unwrap_or("blend=normal|add|multiply|screen".into());
                        self.err("E_VALUE", format!("echo blend={x}"), h, n.span);
                        return;
                    }
                };
                let mut tint = vec![];
                let raw = praw_s(&n, "colors").or_else(|| praw_s(&n, "color"));
                if let Some(raw) = raw {
                    for c in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        if let Some(c) = self.color_s(c, n.span) {
                            tint.push(c);
                        }
                    }
                    if tint.len() > 2 {
                        self.err("E_VALUE", "echo colors: at most 2 (newest copy, oldest copy)", "write `colors=#ff3b30,#2979ff`", n.span);
                        tint.truncate(2);
                    }
                }
                (TimeFxSpec::Echo { count: count as u32, decay, blend: blend.into(), tint }, "dt", ECHO_PROPS)
            }
            _ => {
                let levels = self.num(&n, "levels").unwrap_or(24.0);
                if !(2.0..=64.0).contains(&levels) {
                    self.err("E_VALUE", format!("timedisplace levels={levels}: time samples, 2..64"), "write `levels=24`", n.span);
                    return;
                }
                let map_raw = praw_s(&n, "map").unwrap_or_else(|| "radial".into());
                let map = if let Some(id) = map_raw.strip_prefix('#') {
                    let si = e.scene;
                    match self.names.get(&format!("{id}@{si}")).map(|nm| nm.idx) {
                        Some(mi) if mi != i => {
                            self.layers[mi].style.time_map = true;
                            MapSpec::Layer { id: self.lid(mi) }
                        }
                        _ => {
                            self.err("E_ID", format!("timedisplace map=#{id}: no other element `#{id}` in this scene"), "the map is any element of the scene (its luma, black = now, white = span ago); it is hidden", n.span);
                            return;
                        }
                    }
                } else {
                    match map_raw.as_str() {
                        "radial" => {
                            let (cx, cy) = match praw_s(&n, "center") {
                                None => (0.5, 0.5),
                                Some(c) => match parse_pair(&c) {
                                    Some(p) => p,
                                    None => {
                                        self.err("E_VALUE", format!("center={c}"), "box fractions, e.g. `center=0.5,0.5`", n.span);
                                        return;
                                    }
                                },
                            };
                            MapSpec::Radial { cx, cy, radius: self.num(&n, "radius") }
                        }
                        "linear" => MapSpec::Linear { angle: self.num(&n, "angle").unwrap_or(0.0) },
                        "noise" => MapSpec::Noise { scale: self.num(&n, "scale").unwrap_or(160.0).max(1.0), seed: self.num(&n, "seed").unwrap_or(1.0).max(0.0) as u64 },
                        x => {
                            let h = suggest(x, &["radial", "linear", "noise"]).map(|s| format!("did you mean `{s}`?")).unwrap_or("map=radial|linear|noise|#layer".into());
                            self.err("E_VALUE", format!("timedisplace map={x}"), h, n.span);
                            return;
                        }
                    }
                };
                (TimeFxSpec::Displace { map, levels: levels as u32 }, "span", DISP_PROPS)
            }
        };
        self.time_props(&n, ok);
        let only_span = n.props.keys().all(|k| matches!(k.as_str(), "span" | "dt" | "dur" | "ease"));
        match &self.layers[i].style.time_fx {
            Some(old) if std::mem::discriminant(old) != std::mem::discriminant(&spec) => {
                self.err("E_VALUE", format!("`{k}` on a layer that already has another time effect"), "one time effect per layer: wrap the element in a group for a second one", n.span);
                return;
            }
            Some(_) if only_span => {}
            _ => {
                // cost: subtree renders per frame (slit-scan strips are small: ~2x the box area in all)
                let renders = match &spec {
                    TimeFxSpec::Slitscan { axis, band, .. } => (if axis == "x" { self.w } else { self.h }) / band + 1.0,
                    TimeFxSpec::Echo { count, .. } => *count as f64 + 1.0,
                    TimeFxSpec::Displace { levels, .. } => *levels as f64 * 2.0,
                };
                if renders > RENDERS_WARN {
                    self.warn("W_TIME_COST", format!("`{k}` renders the subtree up to {renders:.0} times per frame"), "raise band= / lower n= or levels=, or apply it to a smaller group", n.span);
                }
                self.layers[i].style.time_fx = Some(spec);
            }
        }
        // span track: off before the first verb, then jump or ease to the new value
        let span = match self.dur(&n, span_key) {
            Some(v) => v,
            None => match self.num(&n, span_key) {
                Some(v) => v,
                None => if k == "echo" { 0.06 } else { 1.0 },
            },
        };
        if !span.is_finite() || span.abs() > 30.0 {
            self.err("E_VALUE", format!("{span_key}={span}: at most 30 s"), "write e.g. `span=1.2s`", n.span);
            return;
        }
        let d = self.dur(&n, "dur").unwrap_or(0.0);
        let ease = self.ease(&n, "ease", Ease::Linear);
        let tr = self.track(i, Ch::TimeSpan);
        if d > 0.0 {
            tr.to(t, t + d, TV::N(span), ease);
        } else {
            tr.jump(t, TV::N(span));
        }
    }
}

fn praw_s(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}

fn parse_pair(s: &str) -> Option<(f64, f64)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// reel-ir `time` object of a layer with box `r` (scene px) on a `w` x `h` frame.
pub fn ir_time(spec: &TimeFxSpec, r: [f64; 4], w: f64, h: f64, map_node: Option<usize>) -> serde_json::Value {
    use serde_json::json;
    let r4 = |v: f64| (v * 1e4).round() / 1e4;
    match spec {
        TimeFxSpec::Slitscan { axis, band, reverse } => {
            let (a, b) = if axis == "x" { (r[0], r[0] + r[2]) } else { (r[1], r[1] + r[3]) };
            let (a, b) = if b - a < 8.0 { (0.0, if axis == "x" { w } else { h }) } else { (a, b) };
            // reverse (dir=up|left): the range runs backwards, the delay grows towards a
            let range = if *reverse { [r4(b), r4(a)] } else { [r4(a), r4(b)] };
            json!({"type": "slitscan", "axis": axis, "span": 0.0, "band": band, "range": range})
        }
        TimeFxSpec::Echo { count, decay, blend, tint } => json!({"type": "echo", "count": count, "span": 0.0, "decay": decay, "blend": blend,
            "tint": tint.iter().map(|c| json!([c.r, c.g, c.b, c.a])).collect::<Vec<_>>()}),
        TimeFxSpec::Displace { map, levels } => {
            let m = match map {
                MapSpec::Radial { cx, cy, radius } => {
                    let c = [r[0] + cx * r[2], r[1] + cy * r[3]];
                    let far = [r[0], r[0] + r[2]].iter().flat_map(|&x| [r[1], r[1] + r[3]].map(|y| (x - c[0]).hypot(y - c[1]))).fold(1.0f64, f64::max);
                    json!({"type": "radial", "center": [r4(c[0]), r4(c[1])], "radius": r4(radius.unwrap_or(far).max(1.0))})
                }
                MapSpec::Linear { angle } => {
                    let (s, c) = angle.to_radians().sin_cos();
                    let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
                    let half = (r[2] * c.abs() + r[3] * s.abs()) / 2.0;
                    json!({"type": "linear", "from": [r4(cx - c * half), r4(cy - s * half)], "to": [r4(cx + c * half), r4(cy + s * half)]})
                }
                MapSpec::Noise { scale, seed } => json!({"type": "noise", "scale": scale, "seed": seed, "octaves": 3}),
                MapSpec::Layer { .. } => json!({"type": "layer", "node": map_node.unwrap_or(0)}),
            };
            json!({"type": "displace", "map": m, "span": 0.0, "levels": levels})
        }
    }
}
