//! Print looks (risograph, halftone, grain, paper) and light on flat design (soft shadows,
//! light sweeps, god rays): element props -> `Style::looks` (pass 1), `light` elements, and
//! the reel-ir effects they compile to (`reelir.rs`, reel `IR.md` "Looks and light").
//! Direct mode only: the HTML/Chrome backend ignores looks.
//!
//! ```text
//! layer #poster riso=[#FF48B0, #0078BF, #FFE800] misreg=3 riso-drift=1.5 riso-grain=0.35 seed=4
//! image #shot src=ui.png halftone=10 halftone-angle=45deg halftone-mode=cmyk
//! text #t "PRINT" grain=0.06 grain-mode=live paper=0.6 bleed=1.5 rough=0.5
//! light #sun x=20% y=-10% z=900 radius=60
//! card #c shadow=soft(#sun, height=40, opacity=0.5)
//! rect #plate rays=#sun rays-length=0.6 rays-color=#FFE9C4
//! @2s sweep #c dur=1.4s angle=20deg width=25% color=#FFF1D6 strength=0.35
//! ```
use crate::ast::{Node, Span, Value};
use crate::ir::*;
use crate::lower::*;
use crate::values::*;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "look", rename_all = "kebab-case")]
pub enum Look {
    Rgbsplit { dx: f64, dy: f64, amount: f64, mode: String },
    Glitch { amount: f64, seed: u64, rate: f64, block: f64 },
    Vhs { amount: f64, seed: u64 },
    Dither { levels: f64, mode: String, size: u32, palette: Option<Vec<Rgba>> },
    Ascii { cell: f64, charset: String, color: String, bg: Rgba },
    Riso { inks: Vec<Rgba>, seed: u64, misreg: f64, drift: f64, speed: f64, grain: f64, density: f64, paper: Option<Rgba> },
    Halftone { cell: f64, angle: f64, cmyk: bool, ink: Rgba, paper: Option<Rgba> },
    Grain { amount: f64, seed: u64, live: bool, size: f64 },
    Paper { texture: f64, bleed: f64, rough: f64, seed: u64, color: Rgba },
    /// soft shadow from a named `light` element
    Shadow { light: String, height: f64, opacity: f64, color: Rgba, span: Span },
    /// light band (verb `sweep`); width px, or % of the layer width when `width_pct`
    Sweep { angle: f64, width: f64, width_pct: bool, color: Rgba, strength: f64 },
    Rays { light: String, length: f64, strength: f64, color: Rgba, span: Span },
}

impl Look {
    /// Effect order in the reel-ir list: print looks first (they shape the ink), then the
    /// light (shadows of the printed layer, rays over everything).
    pub fn order(&self) -> u8 {
        match self {
            Look::Riso { .. } => 0,
            Look::Halftone { .. } => 1,
            Look::Paper { .. } => 2,
            Look::Grain { .. } => 3,
            Look::Sweep { .. } => 4,
            Look::Shadow { .. } => 5,
            Look::Rays { .. } => 6,
            Look::Rgbsplit { .. } => 7,
            Look::Glitch { .. } => 8,
            Look::Vhs { .. } => 9,
            Look::Dither { .. } => 10,
            Look::Ascii { .. } => 11,
        }
    }
}

/// A light source (`light #id`): its layer's center is the light's x/y, animated like any
/// element; `z` = height above the page (px), `radius` = size of the light (penumbra).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LightCfg {
    pub z: f64,
    pub radius: f64,
}

/// Every look prop key (lint: unknown `riso-*`/`halftone-*`/... keys get "did you mean").
pub const LOOK_KEYS: &[&str] = &[
    "knockout", "riso", "misreg", "riso-drift", "riso-speed", "riso-grain", "riso-density", "riso-paper", "seed", "halftone", "halftone-angle",
    "halftone-mode", "halftone-ink", "halftone-paper", "grain", "grain-mode", "grain-size", "paper", "bleed", "rough", "paper-color",
    "rgbsplit", "rgbsplit-dx", "rgbsplit-dy", "rgbsplit-mode",
    "glitch", "glitch-rate", "glitch-block", "vhs", "dither", "dither-mode", "dither-size", "dither-palette",
    "ascii", "ascii-charset", "ascii-color", "ascii-bg",
    "shadow", "rays", "rays-length", "rays-strength", "rays-color",
];
const PREFIXES: &[&str] = &["riso-", "halftone-", "grain-", "paper-", "rays-", "light-", "rgbsplit-", "glitch-", "vhs-", "dither-", "ascii-"];
const SHADOW_ARGS: &[&str] = &["light", "height", "opacity", "color"];

fn rgba(r: f64, g: f64, b: f64, a: f64) -> Rgba {
    Rgba { r, g, b, a }
}

/// `soft(#sun, height=40, opacity=0.5, color=#000)` -> (light, [(key, value)]).
pub fn parse_soft(raw: &str) -> Option<(Option<String>, Vec<(String, String)>)> {
    let inner = raw.trim().strip_prefix("soft(")?.strip_suffix(')')?;
    let mut light = None;
    let mut kv = vec![];
    for part in inner.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()) {
        match part.split_once('=') {
            Some((k, v)) => kv.push((k.trim().to_string(), v.trim().to_string())),
            None => light = Some(part.to_string()),
        }
    }
    Some((light, kv))
}

impl<'a> Ctx<'a> {
    fn look_num(&mut self, raw: &str, key: &str, span: Span) -> Option<f64> {
        match parse_number(raw.trim_end_matches("px").trim_end_matches("deg")) {
            Ok(v) => Some(v),
            Err(e) => {
                self.err(&e.code, format!("`{key}={raw}`: {}", e.message), e.hint, span);
                None
            }
        }
    }

    fn look_range(&mut self, n: &Node, key: &str, dflt: f64, lo: f64, hi: f64) -> f64 {
        match self.num(n, key) {
            Some(v) if (lo..=hi).contains(&v) => v,
            Some(v) => {
                self.err("E_VALUE", format!("`{key}={v}` is out of range"), format!("use {lo}..{hi}"), n.span);
                dflt
            }
            None => dflt,
        }
    }

    fn look_enum(&mut self, n: &Node, key: &str, opts: &[&str]) -> Option<String> {
        let v = n.props.get(key)?.raw();
        if opts.contains(&v.as_str()) {
            return Some(v);
        }
        let hint = suggest(&v, opts).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("{key}= {}", opts.join("|")));
        self.err("E_VALUE", format!("`{key}={v}` is not a known value"), hint, n.span);
        None
    }

    /// Theme/hex color of a prop (or `None` with a diagnostic).
    fn look_color(&mut self, n: &Node, key: &str) -> Option<Rgba> {
        self.color(n, key)
    }

    /// The solid fill of a layer (default paper for riso/halftone on a layer that has one).
    fn solid_fill(&self, i: usize) -> Option<Rgba> {
        match &self.layers[i].style.fill {
            Some(Paint::Solid(ColorRef::Lit(c))) => Some(c.clone()),
            Some(Paint::Solid(ColorRef::Token(t))) => self.th.colors.get(t).cloned(),
            _ => None,
        }
    }

    /// Look props on an element (or a scene: applied to the whole scene, its background
    /// included). Called from `common` and for scene roots.
    pub fn looks(&mut self, i: usize, n: &Node) {
        // unknown keys with a look prefix: did you mean
        for k in n.props.keys() {
            if PREFIXES.iter().any(|p| k.starts_with(p)) && !LOOK_KEYS.contains(&k.as_str()) {
                let hint = suggest(k, LOOK_KEYS).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| "see docs/SPEC.md §8 Print looks and light".into());
                self.err("E_LOOK", format!("unknown look prop `{k}`"), hint, n.span);
            }
        }
        let seed = self.num(n, "seed").map(|v| v.max(0.0) as u64).unwrap_or(1);
        let mut looks = vec![];
        // riso
        if let Some(v) = n.props.get("riso").cloned() {
            let raws: Vec<String> = match &v {
                Value::List(l) => l.iter().map(|x| x.raw()).collect(),
                other => other.raw().split([',', ' ']).filter(|s| !s.is_empty()).map(str::to_string).collect(),
            };
            let mut inks = vec![];
            for r in &raws {
                // bare theme names are fine (`accent`), like `$accent`
                let r = if !r.starts_with('#') && !r.starts_with('$') && self.th.colors.contains_key(r.as_str()) { format!("${r}") } else { r.clone() };
                if let Some(c) = self.color_s(&r, n.span) {
                    inks.push(c);
                }
            }
            if !(1..=4).contains(&raws.len()) {
                self.err("E_LOOK", format!("riso takes 1 to 4 inks, got {}", raws.len()), "write e.g. `riso=[#FF48B0, #0078BF, #FFE800]` (fluorescent pink, blue, yellow)", n.span);
            } else if inks.len() == raws.len() {
                let misreg = self.look_range(n, "misreg", 2.0, 0.0, 200.0);
                let drift = self.look_range(n, "riso-drift", 0.0, 0.0, 200.0);
                let speed = self.look_range(n, "riso-speed", 0.25, 0.0, 30.0);
                let grain = self.look_range(n, "riso-grain", 0.3, 0.0, 1.0);
                let density = self.look_range(n, "riso-density", 0.2, 0.0, 1.0);
                let paper = self.look_color(n, "riso-paper").or_else(|| self.solid_fill(i));
                self.set_base(i, Ch::Misreg, TV::N(misreg));
                looks.push(Look::Riso { inks, seed, misreg, drift, speed, grain, density, paper });
            }
        }
        // halftone
        if n.props.contains_key("halftone") {
            let cell = self.look_range(n, "halftone", 8.0, 1.0, 1000.0);
            let angle = self.num(n, "halftone-angle").unwrap_or(45.0);
            let cmyk = self.look_enum(n, "halftone-mode", &["mono", "cmyk"]).as_deref() == Some("cmyk");
            let ink = self.look_color(n, "halftone-ink").unwrap_or(rgba(0.08, 0.08, 0.1, 1.0));
            let paper = self.look_color(n, "halftone-paper");
            self.set_base(i, Ch::Halftone, TV::N(cell));
            looks.push(Look::Halftone { cell, angle, cmyk, ink, paper });
        }
        // paper
        if n.props.contains_key("paper") || n.props.contains_key("bleed") || n.props.contains_key("rough") {
            let texture = self.look_range(n, "paper", 0.0, 0.0, 1.0);
            let bleed = self.look_range(n, "bleed", 0.0, 0.0, 100.0);
            let rough = self.look_range(n, "rough", 0.0, 0.0, 1.0);
            let color = self.look_color(n, "paper-color").unwrap_or(rgba(0.97, 0.95, 0.91, 1.0));
            looks.push(Look::Paper { texture, bleed, rough, seed, color });
        }
        // grain (also drawn by the HTML backend as an overlay, see `style.grain`)
        if n.props.contains_key("grain") {
            let amount = self.look_range(n, "grain", 0.06, 0.0, 1.0);
            let live = self.look_enum(n, "grain-mode", &["static", "live"]).as_deref() == Some("live");
            let size = self.look_range(n, "grain-size", 1.0, 0.25, 64.0);
            looks.push(Look::Grain { amount, seed, live, size });
        }
        if n.props.contains_key("rgbsplit") {
            let amount = self.look_range(n, "rgbsplit", 1.0, 0.0, 1.0);
            let dx = self.look_range(n, "rgbsplit-dx", 7.0, -200.0, 200.0);
            let dy = self.look_range(n, "rgbsplit-dy", 0.0, -200.0, 200.0);
            let mode = self.look_enum(n, "rgbsplit-mode", &["linear", "radial"]).unwrap_or("linear".into());
            for (ch, v) in [(Ch::Rgbsplit, amount), (Ch::RgbsplitDx, dx), (Ch::RgbsplitDy, dy)] { self.set_base(i, ch, TV::N(v)); }
            looks.push(Look::Rgbsplit { dx, dy, amount, mode });
        }
        if n.props.contains_key("glitch") {
            let amount = self.look_range(n, "glitch", 0.5, 0.0, 1.0);
            let rate = self.look_range(n, "glitch-rate", 6.0, 0.0, 120.0);
            let block = self.look_range(n, "glitch-block", 22.0, 1.0, 1024.0);
            for (ch, v) in [(Ch::Glitch, amount), (Ch::GlitchRate, rate), (Ch::GlitchBlock, block)] { self.set_base(i, ch, TV::N(v)); }
            looks.push(Look::Glitch { amount, seed, rate, block });
        }
        if n.props.contains_key("vhs") {
            let amount = self.look_range(n, "vhs", 0.5, 0.0, 1.0);
            self.set_base(i, Ch::Vhs, TV::N(amount));
            looks.push(Look::Vhs { amount, seed });
        }
        if n.props.contains_key("dither") {
            let levels = self.look_range(n, "dither", 3.0, 2.0, 256.0);
            let mode = self.look_enum(n, "dither-mode", &["bayer", "bluenoise", "floyd"]).unwrap_or("bayer".into());
            let size = self.look_range(n, "dither-size", 4.0, 2.0, 8.0);
            if ![2.0, 4.0, 8.0].contains(&size) {
                self.err("E_LOOK", "invalid dither-size", "use 2, 4 or 8", n.span);
            }
            let palette = n.props.get("dither-palette").cloned().map(|v| {
                let raw: Vec<String> = match v {
                    Value::List(xs) => xs.iter().map(Value::raw).collect(),
                    v => v.raw().split(',').map(|s| s.trim().to_string()).collect(),
                };
                let colors: Vec<_> = raw.iter().filter_map(|c| self.color_s(c, n.span)).collect();
                if !(2..=16).contains(&colors.len()) || colors.iter().any(|c| c.a != 1.0) {
                    self.err("E_LOOK", "invalid dither-palette", "use 2..16 opaque colors", n.span);
                }
                colors
            });
            self.set_base(i, Ch::Dither, TV::N(levels));
            looks.push(Look::Dither { levels, mode, size: size as u32, palette });
        }
        if n.props.contains_key("ascii") {
            let cell = self.look_range(n, "ascii", 8.0, 6.0, 24.0);
            let charset = n.props.get("ascii-charset").map(Value::raw).unwrap_or(" .:-=+*#%@".into());
            if charset.is_empty() || !charset.chars().all(|c| " .:-=+*#%@".contains(c)) {
                self.err("E_LOOK", "invalid ascii-charset", "use a nonempty string of characters from ` .:-=+*#%@`", n.span);
            }
            let color = self.look_enum(n, "ascii-color", &["mono", "source"]).unwrap_or("source".into());
            let bg = self.look_color(n, "ascii-bg").unwrap_or(rgba(0.0, 0.0, 0.0, 1.0));
            self.set_base(i, Ch::Ascii, TV::N(cell));
            looks.push(Look::Ascii { cell, charset, color, bg });
        }
        // soft shadow (the other `shadow=` forms are box/drop shadows, `common`)
        if let Some(raw) = n.props.get("shadow").map(|v| v.raw()) {
            if raw.starts_with("soft") {
                match parse_soft(&raw) {
                    Some((light, kv)) => {
                        let mut light = light;
                        let (mut height, mut opacity, mut color) = (24.0, 0.45, rgba(0.08, 0.06, 0.12, 1.0));
                        for (k, v) in kv {
                            match k.as_str() {
                                "light" => light = Some(v),
                                "height" => height = self.look_num(&v, "height", n.span).unwrap_or(height).max(0.0),
                                "opacity" => opacity = self.look_num(&v, "opacity", n.span).unwrap_or(opacity).clamp(0.0, 1.0),
                                "color" => color = self.color_s(&v, n.span).unwrap_or(color),
                                other => {
                                    let hint = suggest(other, SHADOW_ARGS).map(|s| format!("did you mean `{s}=`?")).unwrap_or_else(|| format!("soft() takes: {}", SHADOW_ARGS.join(" ")));
                                    self.err("E_LOOK", format!("shadow=soft(...): unknown argument `{other}`"), hint, n.span);
                                }
                            }
                        }
                        match light.map(|l| l.trim_start_matches('#').to_string()) {
                            Some(l) if !l.is_empty() => looks.push(Look::Shadow { light: l, height, opacity, color, span: n.span }),
                            _ => self.err("E_LOOK", "shadow=soft(...) needs a light", "declare `light #sun x=20% y=-10%` and write `shadow=soft(#sun)`", n.span),
                        }
                    }
                    None => self.err("E_LOOK", format!("`shadow={raw}`: expected soft(#light, height=.., opacity=.., color=..)"), "write `shadow=soft(#sun, height=40)`", n.span),
                }
            }
        }
        // rays
        if let Some(raw) = n.props.get("rays").map(|v| v.raw()) {
            let light = raw.trim_start_matches('#').to_string();
            let length = self.look_range(n, "rays-length", 0.5, 0.0, 0.95);
            let strength = self.look_range(n, "rays-strength", 1.0, 0.0, 10.0);
            let color = self.look_color(n, "rays-color").unwrap_or(rgba(1.0, 0.93, 0.78, 1.0));
            self.set_base(i, Ch::Rays, TV::N(strength));
            looks.push(Look::Rays { light, length, strength, color, span: n.span });
        }
        self.layers[i].style.looks.extend(looks);
        if flag(n, "knockout") {
            self.layers[i].style.knockout = true;
        }
    }

    /// `light #sun x= y= z= radius=`: an empty layer in the scene world whose center is the light.
    pub fn light_el(&mut self, si: usize, n: &Node, lid: String) -> usize {
        let mut l = Layer::new(lid, Kind::Group);
        l.layout.pos = Pos::Abs;
        l.layout.w = Some(Len::px(0.0));
        l.layout.h = Some(Len::px(0.0));
        l.layout.anchor = (0.5, 0.5);
        l.tags.push("light".into());
        let z = self.num(n, "z").unwrap_or(900.0);
        let radius = self.num(n, "radius").unwrap_or(60.0).max(0.0);
        if z <= 0.0 {
            self.err("E_VALUE", format!("light z={z}: the light must be above the page"), "write e.g. `z=900` (px above the page; lower = longer shadows)", n.span);
        }
        l.style.light = Some(LightCfg { z: z.max(1.0), radius });
        let world = self.scenes[si].world;
        self.add(Some(world), l)
    }

    /// `sweep #x dur= angle= width= color= strength= ease=`: a soft band of light crosses the
    /// layer once (channel `sweep` 0 -> 1).
    pub fn sweep_verb(&mut self, i: usize, t: f64, n: &Node) {
        let d = self.dur(n, "dur").unwrap_or(1.4);
        let ease = self.ease(n, "ease", Ease::Cubic(0.37, 0.0, 0.63, 1.0));
        let angle = self.num(n, "angle").unwrap_or(20.0);
        let (width, width_pct) = match self.len(n, "width") {
            Some(l) if l.unit == "%" => (l.v, true),
            Some(l) => (l.v, false),
            None => (22.0, true),
        };
        let color = self.color(n, "color").unwrap_or(rgba(1.0, 0.95, 0.84, 1.0));
        let strength = self.num(n, "strength").unwrap_or(0.35).clamp(0.0, 4.0);
        if !self.layers[i].style.looks.iter().any(|l| matches!(l, Look::Sweep { .. })) {
            self.layers[i].style.looks.push(Look::Sweep { angle, width, width_pct, color, strength });
        }
        let tr = self.track(i, Ch::Sweep);
        if tr.keys.is_empty() {
            tr.base = TV::N(0.0);
        }
        tr.from_to(t, t + d, TV::N(0.0), TV::N(1.0), ease);
        // park the band past the ink afterwards (a later sweep starts again from 0)
    }

    /// Lint: every light a look names exists in its scene.
    pub fn check_lights(&mut self, si: usize, first: usize) {
        let mut missing = vec![];
        for idx in first..self.layers.len() {
            for lk in &self.layers[idx].style.looks {
                if let Look::Shadow { light, span, .. } | Look::Rays { light, span, .. } = lk {
                    let ok = self.names.get(&format!("{light}@{si}")).is_some_and(|nm| nm.kind == "light");
                    if !ok {
                        missing.push((light.clone(), *span));
                    }
                }
            }
        }
        if missing.is_empty() {
            return;
        }
        let lights: Vec<String> = self.names.iter().filter(|(k, v)| v.scene == si && v.kind == "light" && !k.contains('@')).map(|(k, _)| k.clone()).collect();
        let refs: Vec<&str> = lights.iter().map(|s| s.as_str()).collect();
        for (l, span) in missing {
            let hint = suggest(&l, &refs).map(|s| format!("did you mean `#{s}`?")).unwrap_or_else(|| format!("declare it in this scene: `light #{l} x=20% y=-10% z=900`"));
            self.err("E_LIGHT", format!("no light `#{l}` in this scene"), hint, span);
        }
    }
}
