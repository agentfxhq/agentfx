//! `fluid`: a 2D fluid layer (reel-ir `fluid` content, X5). The renderer simulates stable
//! fluids once, sequentially, and caches the dye per frame; this module records the layer's
//! parameters and what drives it (verbs), times absolute, positions raw (resolved against the
//! laid-out box in `reelir.rs`).
//!
//! ```text
//! fluid #ink w=100% h=100% res=1/4 viscosity=0.0005 dissipation=0.99 vorticity=12 seed=3 color=#E8442E
//! @0.2s pour #ink at=30%,80% dir=-80deg rate=4 speed=900 radius=3% dur=1.5s
//! @0.5s stir #ink at=50%,50% radius=20% strength=5 dur=1s
//! @1s ink #title into #ink dur=0.5s speed=300 dir=-90deg noise=500
//! reveal #shot with #ink
//! ```
use crate::ast::Node;
use crate::events::Target;
use crate::ir::*;
use crate::lower::*;
use crate::values::*;
use serde::Serialize;
use serde_json::{json, Value as J};

const FLUID_PROPS: &[&str] = &[
    "w", "h", "x", "y", "place", "anchor", "res", "viscosity", "dissipation", "drag", "vorticity", "buoyancy", "turbulence", "seed", "color", "density", "shade",
    "iterations", "edge", "opacity", "blend",
];
const POUR_PROPS: &[&str] = &["at", "dir", "speed", "radius", "rate", "wobble", "dur"];
const STIR_PROPS: &[&str] = &["at", "radius", "strength", "dur"];
const INK_PROPS: &[&str] = &["dur", "amount", "speed", "dir", "noise", "keep"];
const REVEAL_PROPS: &[&str] = &["mode", "edge"];
/// Above this the per-frame dye cache is large (lint W_FLUID_MEM).
const MEM_WARN: f64 = 256.0 * 1024.0 * 1024.0;

#[derive(Debug, Clone, Serialize)]
pub struct FluidCfg {
    pub res: f64,
    pub viscosity: f64,
    pub dissipation: f64,
    pub drag: f64,
    pub vorticity: f64,
    pub buoyancy: f64,
    pub turbulence: f64,
    pub seed: u64,
    pub color: Rgba,
    pub density: f64,
    pub shade: f64,
    pub iterations: u32,
    /// rim-only display above this coverage (reveals); 0 = off
    pub edge: f64,
    pub emitters: Vec<FluidEv>,
}

/// What drives the fluid; `at`/`radius` raw (px or % of the box).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FluidEv {
    Pour { t0: f64, t1: f64, at: (String, String), dir: f64, speed: f64, radius: String, rate: f64, wobble: f64 },
    Stir { t0: f64, t1: f64, at: (String, String), radius: String, strength: f64 },
    /// another layer's coverage as dye (layer id, resolved to its node index at emit time)
    Source { t0: f64, t1: f64, id: String, amount: f64, speed: f64, dir: f64, noise: f64 },
}

fn praw_s(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}

/// `1/4`, `0.25`, `25%`.
fn parse_frac(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((a, b)) = s.split_once('/') {
        let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        return (b != 0.0).then_some(a / b);
    }
    if let Some(p) = s.strip_suffix('%') {
        return p.trim().parse::<f64>().ok().map(|v| v / 100.0);
    }
    s.parse().ok()
}

/// A raw length (`120`, `120px`, `30%`) against an extent.
pub fn resolve_len(raw: &str, extent: f64) -> Option<f64> {
    let s = raw.trim();
    if let Some(p) = s.strip_suffix('%') {
        return p.trim().parse::<f64>().ok().map(|v| v / 100.0 * extent);
    }
    s.trim_end_matches("px").parse().ok()
}

fn valid_len(raw: &str) -> bool {
    resolve_len(raw, 100.0).is_some_and(f64::is_finite)
}

impl<'a> Ctx<'a> {
    fn fluid_props(&mut self, n: &Node, ok: &[&str], what: &str) {
        for k in n.props.keys() {
            if !ok.contains(&k.as_str()) {
                let hint = suggest(k, ok).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("`{what}` takes: {}", ok.join(" ")));
                self.warn("W_FLUID_PROP", format!("{what}: unknown prop `{k}`"), hint, n.span);
            }
        }
    }

    /// `fluid #id [w= h= x= y=] res=1/4 viscosity= dissipation= drag= vorticity= buoyancy=
    /// turbulence= seed= color= density= shade= iterations=`. Default box: the whole frame.
    pub fn fluid(&mut self, parent: usize, si: usize, n: &Node, lid: String) -> usize {
        self.fluid_props(n, FLUID_PROPS, "fluid");
        let num = |me: &mut Self, k: &str, d: f64, lo: f64, hi: f64, hint: &str| -> f64 {
            match me.num(n, k) {
                Some(v) if v.is_finite() && (lo..=hi).contains(&v) => v,
                Some(v) => {
                    me.err("E_FLUID", format!("fluid {k}={v}: must be {lo}..{hi}"), hint.to_string(), n.span);
                    d
                }
                None => d,
            }
        };
        let res = match praw_s(n, "res") {
            None => 0.25,
            Some(r) => match parse_frac(&r) {
                Some(v) if (0.05..=1.0).contains(&v) => v,
                _ => {
                    self.err("E_FLUID", format!("fluid res={r}: grid cells per px, 1/20..1"), "write `res=1/4` (a quarter of the output resolution)", n.span);
                    0.25
                }
            },
        };
        let viscosity = num(self, "viscosity", 0.0004, 0.0, 1.0, "kinematic viscosity in box-widths^2/s, e.g. `viscosity=0.0005` (0 = water-thin)");
        let dissipation = num(self, "dissipation", 0.99, 0.0, 1.0, "dye kept per 1/30 s, e.g. `dissipation=0.99` (1 = never fades)");
        let drag = num(self, "drag", 0.995, 0.0, 1.0, "velocity kept per 1/30 s, e.g. `drag=0.995`");
        let vorticity = num(self, "vorticity", 12.0, 0.0, 200.0, "curl strength, e.g. `vorticity=12` (0 = off, 30 = very curly)");
        let buoyancy = num(self, "buoyancy", 0.0, -1e5, 1e5, "upward px/s^2 per unit dye (smoke), e.g. `buoyancy=400`");
        let turbulence = num(self, "turbulence", 0.0, 0.0, 1e5, "ambient curl-noise px/s^2, e.g. `turbulence=300`");
        let seed = num(self, "seed", 1.0, 0.0, 1e12, "a whole number, e.g. `seed=3`") as u64;
        let density = num(self, "density", 2.5, 0.01, 100.0, "display gain (coverage = 1 - exp(-density*dye)), e.g. `density=2.5`");
        let shade = num(self, "shade", 0.0, 0.0, 1.0, "darkening of dense dye 0..1, e.g. `shade=0.3`");
        let edge = num(self, "edge", 0.0, 0.0, 0.99, "rim-only display above this coverage, e.g. `edge=0.45` (0 = off)");
        let iterations = num(self, "iterations", 2.0, 1.0, 20.0, "pressure multigrid V-cycles per step, e.g. `iterations=2`") as u32;
        let color = self.color(n, "color").unwrap_or_else(|| self.th.colors.get("accent").cloned().unwrap_or(crate::theme::hex("#E8442E")));
        let cfg = FluidCfg { res, viscosity, dissipation, drag, vorticity, buoyancy, turbulence, seed, color, density, shade, iterations, edge, emitters: vec![] };
        let mut l = Layer::new(lid, Kind::Fluid(Box::new(cfg)));
        l.tags.push("fluid".into());
        let sized = ["w", "h", "x", "y", "place"].iter().any(|k| n.props.contains_key(*k));
        let i = if sized {
            self.add(Some(parent), l)
        } else {
            // fill the enclosing group (so its rewinds/time maps reach the fluid); at the top of
            // a scene: the whole frame, in the scene world
            let mut l = l;
            l.layout.pos = Pos::Fill;
            let p = if parent == self.scenes[si].content { self.scenes[si].world } else { parent };
            self.add(Some(p), l)
        };
        // memory: u8 per grid cell per frame of the whole video (box: the frame when unsized)
        let (bw, bh) = (self.w, self.h);
        let bytes = (bw * res).round() * (bh * res).round() * (self.tl.total * self.fps).ceil();
        if bytes > MEM_WARN {
            self.warn("W_FLUID_MEM", format!("fluid dye cache about {:.0} MB ({:.0}x{:.0} cells x {:.0} frames)", bytes / 1048576.0, bw * res, bh * res, self.tl.total * self.fps), "lower res= (1/4 is plenty: the display is upsampled and blurred) or shorten the video", n.span);
        }
        // identity sim-time track (fluid.time): a rewind/timemap of an ancestor bakes it
        let total = self.tl.total.max(1e-3);
        let tr = self.track(i, Ch::FluidTime);
        tr.keys = vec![crate::ir::Key { t: 0.0, v: TV::N(0.0), ease: Ease::Linear }, crate::ir::Key { t: total, v: TV::N(total), ease: Ease::Linear }];
        i
    }

    fn fluid_target(&mut self, e: &Ev, id: &str) -> Option<usize> {
        let si = e.scene;
        match self.names.get(&format!("{id}@{si}")).map(|nm| nm.idx) {
            Some(i) if matches!(self.layers[i].kind, Kind::Fluid(_)) => Some(i),
            Some(_) => {
                self.err("E_FLUID", format!("`#{id}` is not a fluid"), format!("declare it with `fluid #{id} ...`"), e.node.span);
                None
            }
            None => {
                let fl: Vec<String> = self.names.iter().filter(|(_, v)| v.scene == si && matches!(self.layers[v.idx].kind, Kind::Fluid(_))).map(|(k, _)| k.split('@').next().unwrap().to_string()).collect();
                let refs: Vec<&str> = fl.iter().map(|s| s.as_str()).collect();
                let hint = suggest(id, &refs).map(|s| format!("did you mean `#{s}`?")).unwrap_or_else(|| "declare one in this scene: `fluid #ink color=#E8442E`".into());
                self.err("E_FLUID", format!("no fluid `#{id}` in this scene"), hint, e.node.span);
                None
            }
        }
    }

    fn at_pair(&mut self, n: &Node) -> Option<(String, String)> {
        let raw = praw_s(n, "at").unwrap_or_else(|| "50%,50%".into());
        let raw = raw.trim_start_matches('[').trim_end_matches(']').to_string();
        let parts: Vec<&str> = raw.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).collect();
        if parts.len() == 2 && parts.iter().all(|p| valid_len(p)) {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            self.err("E_FLUID", format!("{} at={raw}", n.kind), "a point in the fluid's box: `at=30%,80%` or `at=400,900` (px)", n.span);
            None
        }
    }

    fn emit(&mut self, i: usize, ev: FluidEv) {
        if let Kind::Fluid(cfg) = &mut self.layers[i].kind {
            cfg.emitters.push(ev);
        }
    }

    /// `pour #f at= dir= speed= radius= rate= wobble= dur=`, `stir #f at= radius= strength= dur=`,
    /// `ink #layer into #f dur= amount= speed= dir= noise= [keep]`, `reveal #layer with #f [mode=]`.
    pub fn fluid_verb(&mut self, e: &Ev) {
        let n = e.node.clone();
        let t = e.t;
        let k = n.kind.as_str();
        let ids: Vec<String> = n.args.iter().filter_map(|a| a.as_str()).filter(|s| s.starts_with('#') && s.len() > 1).map(|s| s[1..].to_string()).collect();
        let words: Vec<String> = n.args.iter().filter_map(|a| a.as_str()).filter(|s| !s.starts_with('#')).map(str::to_string).collect();
        match k {
            "pour" | "stir" => {
                self.fluid_props(&n, if k == "pour" { POUR_PROPS } else { STIR_PROPS }, k);
                let Some(id) = ids.first().cloned().or_else(|| e.target.clone()) else {
                    self.err("E_TARGET", format!("`{k}` needs a fluid"), format!("write `{k} #ink ...`"), n.span);
                    return;
                };
                let Some(i) = self.fluid_target(e, &id) else { return };
                let Some(at) = self.at_pair(&n) else { return };
                let d = self.dur(&n, "dur").unwrap_or(if k == "pour" { 1.0 } else { 0.8 });
                if !(d > 0.0) {
                    self.err("E_FLUID", format!("{k} dur={d}"), "a positive duration, e.g. `dur=1.2s`", n.span);
                    return;
                }
                let radius = praw_s(&n, "radius").unwrap_or_else(|| if k == "pour" { "2.5%".into() } else { "18%".into() });
                if !valid_len(&radius) || resolve_len(&radius, 100.0).unwrap_or(0.0) <= 0.0 {
                    self.err("E_FLUID", format!("{k} radius={radius}"), "px or % of the box width, e.g. `radius=3%`", n.span);
                    return;
                }
                if k == "pour" {
                    let dir = self.num(&n, "dir").unwrap_or(-90.0);
                    let speed = self.num(&n, "speed").unwrap_or(700.0);
                    let rate = self.num(&n, "rate").unwrap_or(3.0);
                    let wobble = self.num(&n, "wobble").unwrap_or(14.0);
                    if rate < 0.0 || !speed.is_finite() {
                        self.err("E_FLUID", "pour rate must be >= 0", "dye per second at the jet's center, e.g. `rate=3`", n.span);
                        return;
                    }
                    self.emit(i, FluidEv::Pour { t0: t, t1: t + d, at, dir, speed, radius, rate, wobble });
                } else {
                    let strength = self.num(&n, "strength").unwrap_or(4.0);
                    self.emit(i, FluidEv::Stir { t0: t, t1: t + d, at, radius, strength });
                }
            }
            "ink" => {
                self.fluid_props(&n, INK_PROPS, k);
                let Some(Target::El(src)) = self.target(e, true) else { return };
                let into = words.iter().position(|w| w == "into");
                let Some(fid) = (if into.is_some() { ids.get(1).cloned() } else { None }) else {
                    self.err("E_FLUID", "`ink` needs a fluid: `ink #title into #ink`", "the layer's shape becomes dye of that fluid", n.span);
                    return;
                };
                let Some(fi) = self.fluid_target(e, &fid) else { return };
                if fi == src {
                    self.err("E_FLUID", "a fluid cannot ink into itself", "ink another layer: `ink #title into #ink`", n.span);
                    return;
                }
                let d = self.dur(&n, "dur").unwrap_or(0.5);
                let amount = self.num(&n, "amount").unwrap_or(1.6);
                let speed = self.num(&n, "speed").unwrap_or(0.0);
                let dir = self.num(&n, "dir").unwrap_or(-90.0);
                let noise = self.num(&n, "noise").unwrap_or(420.0);
                if !(d >= 0.0) || amount < 0.0 {
                    self.err("E_FLUID", "ink dur and amount must be >= 0", "write e.g. `ink #title into #ink dur=0.5s amount=1.6`", n.span);
                    return;
                }
                let sid = self.lid(src);
                self.emit(fi, FluidEv::Source { t0: t, t1: t + d, id: sid, amount, speed, dir, noise });
                if !flag(&n, "keep") {
                    // the layer turns into the ink: fade out while its coverage pours in
                    let tr = self.track(src, Ch::Opacity);
                    if d > 0.0 {
                        tr.to(t, t + d, TV::N(0.0), Ease::Linear);
                    } else {
                        tr.jump(t, TV::N(0.0));
                    }
                }
            }
            _ => {
                // reveal #layer with #fluid
                self.fluid_props(&n, REVEAL_PROPS, k);
                let Some(Target::El(dst)) = self.target(e, true) else { return };
                let with = words.iter().position(|w| w == "with");
                let Some(fid) = (if with.is_some() { ids.get(1).cloned() } else { None }) else {
                    self.err("E_FLUID", "`reveal` needs a fluid: `reveal #shot with #ink`", "the layer shows only where the ink is", n.span);
                    return;
                };
                let Some(fi) = self.fluid_target(e, &fid) else { return };
                let mode = praw_s(&n, "mode").unwrap_or_else(|| "alpha".into());
                if !matches!(mode.as_str(), "alpha" | "alpha-inverted") {
                    let h = suggest(&mode, &["alpha", "alpha-inverted"]).map(|s| format!("did you mean `{s}`?")).unwrap_or("mode=alpha (shown inside the ink) or alpha-inverted (ink erases it)".into());
                    self.err("E_FLUID", format!("reveal mode={mode}"), h, n.span);
                    return;
                }
                if words.iter().any(|w| w == "only") {
                    self.layers[fi].style.fluid_hidden = true;
                } else {
                    // the stain's core shows the layer, the fluid draws the rim above it
                    let edge = self.num(&n, "edge").unwrap_or(0.45);
                    if !(0.0..1.0).contains(&edge) {
                        self.err("E_FLUID", format!("reveal edge={edge}"), "coverage where the dark rim fades into the revealed layer, 0..1, e.g. `edge=0.45` (0 = the whole stain drawn)", n.span);
                        return;
                    }
                    if let Kind::Fluid(cfg) = &mut self.layers[fi].kind {
                        if cfg.edge == 0.0 {
                            cfg.edge = edge;
                        }
                    }
                    if fi < dst {
                        self.warn("W_FLUID_ORDER", format!("reveal: fluid `#{fid}` is declared before (below) `#{}`", self.lid(dst)), "declare the fluid after the revealed layer so its dark rim draws on top", n.span);
                    }
                }
                let fl = self.lid(fi);
                self.layers[dst].style.fluid_matte = Some((fl, mode));
            }
        }
    }
}

/// reel-ir `fluid` content of a layer with box `r` (scene px); sources keep their layer id in
/// `node` as a string until `reelir.rs` patches node indices.
pub fn ir_fluid(cfg: &FluidCfg, r: [f64; 4]) -> J {
    let r4 = |v: f64| (v * 1e4).round() / 1e4;
    let pt = |at: &(String, String)| json!([r4(r[0] + resolve_len(&at.0, r[2]).unwrap_or(0.0)), r4(r[1] + resolve_len(&at.1, r[3]).unwrap_or(0.0))]);
    let em: Vec<J> = cfg.emitters.iter().map(|e| match e {
        FluidEv::Pour { t0, t1, at, dir, speed, radius, rate, wobble } => json!({"type": "pour", "start": r4(*t0), "end": r4(*t1), "at": pt(at), "dir": dir, "speed": speed,
            "radius": r4(resolve_len(radius, r[2]).unwrap_or(10.0).max(0.5)), "rate": rate, "wobble": wobble}),
        FluidEv::Stir { t0, t1, at, radius, strength } => json!({"type": "stir", "start": r4(*t0), "end": r4(*t1), "at": pt(at),
            "radius": r4(resolve_len(radius, r[2]).unwrap_or(100.0).max(0.5)), "strength": strength}),
        FluidEv::Source { t0, t1, id, amount, speed, dir, noise } => json!({"type": "source", "start": r4(*t0), "end": r4(*t1), "node": id, "amount": amount, "speed": speed, "dir": dir, "noise": noise}),
    }).collect();
    let c = &cfg.color;
    json!({"type": "fluid", "rect": [r4(r[0]), r4(r[1]), r4(r[2]), r4(r[3])], "res": cfg.res, "viscosity": cfg.viscosity, "dissipation": cfg.dissipation,
        "drag": cfg.drag, "vorticity": cfg.vorticity, "buoyancy": cfg.buoyancy, "turbulence": cfg.turbulence, "seed": cfg.seed,
        "color": [c.r, c.g, c.b, c.a], "density": cfg.density, "shade": cfg.shade, "iterations": cfg.iterations, "edge": cfg.edge, "emitters": em})
}

/// Is the fluid.time track an identity (no remap touched it)?
pub fn identity_time(t: &Track) -> bool {
    t.keys.iter().all(|k| (k.v.n() - k.t).abs() < 1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractions_and_lengths() {
        assert_eq!(parse_frac("1/4"), Some(0.25));
        assert_eq!(parse_frac("50%"), Some(0.5));
        assert_eq!(resolve_len("30%", 200.0), Some(60.0));
        assert_eq!(resolve_len("12px", 200.0), Some(12.0));
        assert!(!valid_len("abc"));
    }
}
