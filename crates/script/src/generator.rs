//! `gen #id kind=julia [w= h= x= y=] <param>=<number>... palette=neon|linear(#a, #b, ...) seed=`:
//! generator layers (reel-ir `gen` content). Kinds and their parameters come from
//! `agentfx_ir::generator::KINDS`; every parameter is a channel (`anim #g zoom 0s:1 8s:400`)
//! that lowers to a `gen.<param>` track. Direct mode only.
use crate::ast::{Node, Span};
use crate::diag::Diag;
use crate::ir::*;
use crate::lower::*;
use crate::values::*;
use agentfx_ir::generator as g;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct GenCfg {
    pub kind: String,
    /// parameters as written (the kind's defaults fill the rest)
    pub params: BTreeMap<String, f64>,
    pub palette: Vec<Rgba>,
    pub seed: u64,
    /// start of the layer's scene, s: time-dependent kinds run on scene time (t = 0 at the cut)
    pub start: f64,
}

fn praw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}

/// Props that size / place a `gen` layer (without any: it fills its parent).
const LAYOUT_KEYS: [&str; 5] = ["w", "h", "x", "y", "place"];

impl<'a> Ctx<'a> {
    pub fn gen_el(&mut self, parent: usize, si: usize, n: &Node, lid: String) -> usize {
        let names: Vec<&str> = g::KINDS.iter().map(|k| k.name).collect();
        let raw = praw(n, "kind").unwrap_or_default();
        let kind = match g::kind(&raw) {
            Some(k) => k,
            None => {
                let hint = suggest(&raw, &names).map(|s| format!("did you mean `kind={s}`?")).unwrap_or_else(|| format!("kinds: {}", names.join(" ")));
                self.err("E_GEN", format!("gen: unknown kind `{raw}`"), hint, n.span);
                &g::KINDS[0]
            }
        };
        let mut params = BTreeMap::new();
        let all = g::param_names();
        // life: `rule="B36/S23"` (a B/S rule string) sets the born / survive bitmasks
        let bs_rule = kind.name == "life" && matches!(n.props.get("rule"), Some(crate::ast::Value::Str(_)));
        if bs_rule {
            let raw = praw(n, "rule").unwrap_or_default();
            match parse_bs(&raw) {
                Some((b, sv)) => {
                    params.insert("born".to_string(), b as f64);
                    params.insert("survive".to_string(), sv as f64);
                }
                None => self.err("E_GEN", format!("gen life: rule=\"{raw}\" is not a B/S rule"), "e.g. rule=\"B3/S23\" (Conway), \"B36/S23\" (HighLife), or mode=1 rule=30 (1D)", n.span),
            }
        }
        // named values for their index: lsystem `preset=dragon`, kifs `mode=sierpinski`
        let named: &[(&str, &str, &[&str])] = &[("lsystem", "preset", g::LSYSTEM_PRESETS), ("kifs", "mode", g::KIFS_MODES)];
        let named = named.iter().find(|(kn, p, _)| *kn == kind.name && matches!(n.props.get(*p), Some(crate::ast::Value::Str(_))));
        if let Some((_, pn, names)) = named {
            let raw = praw(n, pn).unwrap_or_default();
            match names.iter().position(|p| *p == raw) {
                Some(i) => {
                    params.insert(pn.to_string(), i as f64);
                }
                None => {
                    let hint = suggest(&raw, names).map(|s| format!("did you mean `{pn}={s}`?")).unwrap_or_else(|| format!("{pn}: {}", names.join(" ")));
                    self.err("E_GEN", format!("gen {}: unknown {pn} `{raw}`", kind.name), hint, n.span);
                }
            }
        }
        // attractor: `system=lorenz` (a system name) for its index, plus that system's a b c d
        // (explicit coefficients below still win)
        let named_system = kind.name == "attractor" && matches!(n.props.get("system"), Some(crate::ast::Value::Str(_)));
        if named_system {
            let raw = praw(n, "system").unwrap_or_default();
            let names: Vec<&str> = g::ATTRACTOR_SYSTEMS.iter().map(|(s, _)| *s).collect();
            match names.iter().position(|s| *s == raw) {
                Some(i) => {
                    params.insert("system".to_string(), i as f64);
                    for (k, v) in ["a", "b", "c", "d"].into_iter().zip(g::ATTRACTOR_SYSTEMS[i].1) {
                        params.insert(k.to_string(), v);
                    }
                }
                None => {
                    let hint = suggest(&raw, &names).map(|s| format!("did you mean `system={s}`?")).unwrap_or_else(|| format!("systems: {}", names.join(" ")));
                    self.err("E_GEN", format!("gen attractor: unknown system `{raw}`"), hint, n.span);
                }
            }
        }
        // reaction: `preset=coral` sets feed and kill (explicit values below still win); the
        // preset itself is not a parameter
        let reaction_preset = kind.name == "reaction" && n.props.contains_key("preset");
        if reaction_preset {
            let raw = praw(n, "preset").unwrap_or_default();
            let names: Vec<&str> = g::REACTION_PRESETS.iter().map(|(s, _)| *s).collect();
            match names.iter().position(|s| *s == raw) {
                Some(i) => {
                    let [feed, kill] = g::REACTION_PRESETS[i].1;
                    params.insert("feed".to_string(), feed);
                    params.insert("kill".to_string(), kill);
                }
                None => {
                    let hint = suggest(&raw, &names).map(|s| format!("did you mean `preset={s}`?")).unwrap_or_else(|| format!("presets: {}", names.join(" ")));
                    self.err("E_GEN", format!("gen reaction: unknown preset `{raw}`"), hint, n.span);
                }
            }
        }
        for k in n.props.keys() {
            if (bs_rule && k == "rule") || named.is_some_and(|(_, pn, _)| k == pn) || (named_system && k == "system") || (reaction_preset && k == "preset") || matches!(k.as_str(), "seed" | "palette" | "kind") {
                continue;
            }
            match kind.param(k) {
                Some(p) => {
                    // a literal keeps every digit (`raw()` prints 6 decimals)
                    let lit = match n.props.get(k) {
                        Some(crate::ast::Value::Num(v)) => Some(*v),
                        _ => None,
                    };
                    if let Some(v) = lit.or_else(|| self.num(n, k)) {
                        if v.is_finite() && (p.min..=p.max).contains(&v) {
                            params.insert(p.name.to_string(), v);
                        } else {
                            self.err("E_GEN", format!("gen {}: {k}={v} outside {}..{}", kind.name, p.min, p.max), format!("{k}: {} (default {})", p.doc, p.default), n.span);
                        }
                    }
                }
                // layout keys place the layer like any other (`x` is no near miss of `cx`)
                None if LAYOUT_KEYS.contains(&k.as_str()) => {}
                // another kind's parameter or a near miss of this kind's
                None => {
                    let own: Vec<&str> = kind.params.iter().map(|p| p.name).collect();
                    if all.contains(&k.as_str()) || suggest(k, &own).is_some() {
                        let hint = suggest(k, &own).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("`{}` takes: {}", kind.name, own.join(" ")));
                        self.warn("W_GEN_PROP", format!("gen {}: unknown parameter `{k}`", kind.name), hint, n.span);
                    }
                }
            }
        }
        let palette = match praw(n, "palette") {
            None => vec![],
            Some(p) => self.gen_palette(&p, n.span),
        };
        let seed = self.num(n, "seed").unwrap_or(0.0).max(0.0) as u64;
        let mut l = Layer::new(lid, Kind::Gen(Box::new(GenCfg { kind: kind.name.to_string(), params, palette, seed, start: self.tl.scenes.get(si).map_or(0.0, |s| s.start) })));
        l.tags.push("gen".into());
        let sized = LAYOUT_KEYS.iter().any(|k| n.props.contains_key(*k));
        if !sized {
            l.layout.pos = Pos::Fill;
        }
        self.add(Some(parent), l)
    }

    /// `neon` (named) or `linear(#a, #b, ...)` (stops evenly spaced, cyclic).
    fn gen_palette(&mut self, raw: &str, span: Span) -> Vec<Rgba> {
        if let Some(stops) = g::palette(raw) {
            return stops.iter().map(|c| Rgba { r: c[0], g: c[1], b: c[2], a: c[3] }).collect();
        }
        let names: Vec<&str> = g::PALETTES.iter().map(|p| p.0).collect();
        let stops = match parse_paint(raw).map(|p| self.th.paint(&p)) {
            Ok(Paint::Linear { stops, .. } | Paint::Radial { stops, .. } | Paint::Conic { stops, .. }) => stops,
            _ => {
                let hint = suggest(raw, &names).map(|s| format!("did you mean `palette={s}`?")).unwrap_or_else(|| format!("palettes: {} or `linear(#a, #b, ...)`", names.join(" ")));
                self.err("E_GEN", format!("gen: unknown palette `{raw}`"), hint, span);
                return vec![];
            }
        };
        if stops.len() > g::PALETTE_MAX {
            self.err("E_GEN", format!("gen: palette has {} stops", stops.len()), format!("at most {}", g::PALETTE_MAX), span);
        }
        stops.iter().take(g::PALETTE_MAX).filter_map(|s| self.th.color(&s.color)).collect()
    }
}

/// `B3/S23` (any case, either order, `/` optional) -> (born, survive) neighbour-count bitmasks.
pub fn parse_bs(raw: &str) -> Option<(u32, u32)> {
    let (mut b, mut s, mut cur, mut seen) = (0u32, 0u32, None::<char>, (false, false));
    for c in raw.trim().chars() {
        match c.to_ascii_uppercase() {
            'B' => {
                cur = Some('B');
                seen.0 = true;
            }
            'S' => {
                cur = Some('S');
                seen.1 = true;
            }
            '/' | ' ' => {}
            d @ '0'..='8' => {
                let bit = 1 << (d as u32 - '0' as u32);
                match cur? {
                    'B' => b |= bit,
                    _ => s |= bit,
                }
            }
            _ => return None,
        }
    }
    (seen.0 && seen.1).then_some((b, s))
}

#[test]
fn bs_rules() {
    assert_eq!(parse_bs("B3/S23"), Some((8, 12)));
    assert_eq!(parse_bs("b36/s23"), Some((72, 12)));
    assert_eq!(parse_bs("B2/S"), Some((4, 0)));
    assert_eq!(parse_bs("S23/B3"), Some((8, 12)));
    assert_eq!(parse_bs("B9/S1"), None);
    assert_eq!(parse_bs("30"), None);
}

/// The reel-ir `gen` content of a layer laid out at `r`.
pub fn ir_gen(cfg: &GenCfg, r: [f64; 4]) -> Value {
    let r4 = |v: f64| (v * 1e4).round() / 1e4;
    let pal: Vec<Value> = cfg.palette.iter().map(|c| json!([c.r, c.g, c.b, c.a])).collect();
    let mut c = json!({"type": "gen", "kind": cfg.kind, "rect": [r4(r[0]), r4(r[1]), r4(r[2]), r4(r[3])], "params": cfg.params, "palette": pal, "seed": cfg.seed});
    if g::kind(&cfg.kind).is_some_and(|k| k.time) && cfg.start != 0.0 {
        c["time_offset"] = json!(-cfg.start);
    }
    c
}

/// E_GEN (a parameter channel on the wrong layer) and W_GEN_ZOOM (an effective zoom past the
/// f32 GPU kernel's range: the GPU path would show blocky, smeared detail).
pub fn rules(ir: &Ir) -> Vec<Diag> {
    let mut d = vec![];
    let mut gens: BTreeMap<&str, (&Layer, &GenCfg)> = BTreeMap::new();
    ir.root.walk(&mut |l| {
        if let Kind::Gen(c) = &l.kind {
            gens.insert(l.id.as_str(), (l, c));
        }
    });
    let label = |l: &Layer| l.name.clone().unwrap_or(l.id.clone());
    for t in &ir.tracks {
        let Some(p) = t.ch.gen_param() else { continue };
        match gens.get(t.layer.as_str()) {
            Some((_, c)) if g::kind(&c.kind).is_some_and(|k| k.param(&p).is_some()) => {}
            Some((l, c)) => d.push(Diag::error("E_GEN", format!("gen {} `{}` has no parameter `{p}`", c.kind, label(l)), "see `agentfx catalog` > gen for the parameters of each kind", Span::default()).with_id(label(l))),
            // `morph` on a path is the path morph
            None if t.ch == crate::ir::Ch::Morph => {}
            None => d.push(Diag::error("E_GEN", format!("channel `{p}` only animates a gen layer, not `{}`", t.layer), "target the gen layer: `anim #g zoom 0s:1 8s:400`", Span::default()).with_id(t.layer.clone())),
        }
    }
    for (id, (l, c)) in &gens {
        let Some(k) = g::kind(&c.kind) else { continue };
        let Some(max) = k.gpu_zoom_max else { continue };
        let vals = |p: &str| -> Vec<f64> {
            let base = c.params.get(p).copied().or(k.param(p).map(|x| x.default)).unwrap_or(0.0);
            let mut v = vec![base];
            for t in ir.tracks.iter().filter(|t| t.layer == *id && t.ch.gen_param().as_deref() == Some(p)) {
                v.extend(t.keys.iter().map(|k| k.v.n()));
            }
            v
        };
        let zoom = vals("zoom").into_iter().fold(0.0, f64::max) * vals("depth").into_iter().fold(f64::MIN, f64::max).exp2();
        if zoom > max {
            d.push(Diag::warn("W_GEN_ZOOM", format!("gen {} `{}` zooms to {zoom:.3e}x, past the GPU kernel's f32 range (about {max:.0e}x)", c.kind, label(l)), "fine on the CPU (f64, the default); with --gpu the deep frames lose detail: render those on the CPU or zoom less", Span::default()).with_id(label(l)));
        }
    }
    d
}
