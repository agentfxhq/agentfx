//! Compile HTML, CSS animations, and optional text tracks into property tracks.
use std::{collections::{BTreeMap, BTreeSet, HashMap}, fs, path::Path};
use anyhow::{Result, anyhow, bail, ensure};
use lightningcss::{declaration::DeclarationBlock, properties::Property, rules::{CssRule, CssRuleList, keyframes::{KeyframesRule, KeyframeSelector}}, stylesheet::{StyleSheet, ParserOptions, PrinterOptions}, traits::ToCss};
use scraper::{Html, Selector, ElementRef};
use lightningcss::{properties::transform::Transform, values::length::{LengthPercentage, LengthValue}};
use serde::Deserialize;
use crate::model::*;

fn css<T: ToCss>(v: &T) -> String {
    v.to_css_string(PrinterOptions::default()).expect("serialization into String")
}
fn prop_name(p: &Property<'_>) -> String { css(&p.property_id()) }
fn prop_value(p: &Property<'_>) -> String {
    p.value_to_css_string(PrinterOptions::default()).expect("serialization into String")
}
fn animation(s: &str) -> bool { s == "animation" || s.starts_with("animation-") || s.starts_with("-webkit-animation") || s.starts_with("-moz-animation") || s.starts_with("-o-animation") }
const LONGHANDS: [&str; 8] = ["animation-name", "animation-duration", "animation-timing-function", "animation-delay", "animation-fill-mode", "animation-iteration-count", "animation-direction", "animation-play-state"];
const DEFAULTS: [&str; 8] = ["none", "0s", "ease", "0s", "none", "1", "normal", "running"];

// Split only at the outer level, preserving commas and spaces inside functions/strings.
fn split_list(s: &str) -> Vec<String> {
    let mut depth = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut start = 0;
    let mut result = Vec::new();
    for (i,c) in s.char_indices() {
        if escaped { escaped = false; continue; }
        if c == '\\' { escaped = true; continue; }
        if let Some(q) = quote { if c == q { quote = None; } continue; }
        match c {
            '\'' | '"' => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => { result.push(s[start..i].trim().to_owned()); start = i + 1; }
            _ => {}
        }
    }
    result.push(s[start..].trim().to_owned());
    result
}
fn number(s: &str) -> Result<f64> {
    let n: f64 = s.trim().parse().map_err(|_| anyhow!("expected number, got {s}"))?;
    ensure!(n.is_finite(), "number must be finite: {s}");
    Ok(n)
}
fn ratio(s: &str) -> Result<f64> { if let Some(n) = s.strip_suffix('%') { Ok(number(n)? / 100.0) } else { number(s) } }
fn time(s: &str) -> Result<f64> {
    let t = if let Some(n) = s.strip_suffix("ms") { number(n)? / 1000.0 }
        else if let Some(n) = s.strip_suffix('s') { number(n)? } else { bail!("expected seconds or milliseconds, got {s}") };
    Ok((t * 1e6).round() / 1e6)
}
fn easing(s: &str) -> Result<Easing> {
    Ok(match s.trim() {
        "linear" => Easing::Linear,
        "ease" => Easing::CubicBezier(0.25, 0.1, 0.25, 1.0),
        "ease-in" => Easing::CubicBezier(0.42, 0.0, 1.0, 1.0),
        "ease-out" => Easing::CubicBezier(0.0, 0.0, 0.58, 1.0),
        "ease-in-out" => Easing::CubicBezier(0.42, 0.0, 0.58, 1.0),
        "step-start" => Easing::Steps(1, StepPos::JumpStart),
        "step-end" => Easing::Steps(1, StepPos::JumpEnd),
        s if s.starts_with("steps(") && s.ends_with(')') => {
            let args = split_list(&s[6..s.len()-1]);
            ensure!((1..=2).contains(&args.len()), "invalid steps: {s}");
            let n: u32 = args[0].parse().map_err(|_| anyhow!("invalid steps count: {s}"))?;
            let pos = match args.get(1).map(String::as_str).unwrap_or("jump-end") {
                "start" | "jump-start" => StepPos::JumpStart,
                "end" | "jump-end" => StepPos::JumpEnd,
                "jump-none" => StepPos::JumpNone,
                "jump-both" => StepPos::JumpBoth,
                _ => bail!("invalid steps position: {s}"),
            };
            ensure!(n > 0 && !(n == 1 && pos == StepPos::JumpNone), "invalid steps count: {s}");
            Easing::Steps(n,pos)
        }
        s if s.starts_with("cubic-bezier(") && s.ends_with(')') => {
            let a = split_list(&s[13..s.len()-1]).iter().map(|s| number(s)).collect::<Result<Vec<_>>>()?;
            ensure!(a.len() == 4 && (0.0..=1.0).contains(&a[0]) && (0.0..=1.0).contains(&a[2]), "invalid cubic-bezier: {s}");
            Easing::CubicBezier(a[0],a[1],a[2],a[3])
        }
        _ => bail!("unsupported timing function: {s}"),
    })
}
fn px(s: &str) -> Result<f64> {
    if let Some(n) = s.strip_suffix("px") { number(n) }
    else if number(s).ok() == Some(0.0) { Ok(0.0) }
    else { bail!("translate requires px (or zero), got {s}") }
}
fn angle(s: &str) -> Result<f64> {
    for (unit, factor) in [("deg",1.0), ("grad",0.9), ("rad",180.0/std::f64::consts::PI), ("turn",360.0)] {
        if let Some(n) = s.strip_suffix(unit) { return Ok(number(n)? * factor); }
    }
    if number(s).ok() == Some(0.0) { return Ok(0.0); }
    bail!("unsupported rotate angle: {s}")
}
fn transforms(mut s: &str) -> Result<Vec<TransformFn>> {
    if s == "none" { return Ok(vec![]); }
    let mut result = Vec::new();
    while !s.trim().is_empty() {
        s = s.trim_start();
        let open = s.find('(').ok_or_else(|| anyhow!("invalid transform: {s}"))?;
        let close = s.find(')').ok_or_else(|| anyhow!("invalid transform: {s}"))?;
        let name = &s[..open];
        let args = split_list(&s[open+1..close]);
        let a = args.first().ok_or_else(|| anyhow!("missing transform argument"))?;
        let b = args.get(1).map(String::as_str);
        let f = match name {
            "translate" if args.len() <= 2 => TransformFn::Translate(px(a)?, px(b.unwrap_or("0"))?),
            "translateX" if args.len() == 1 => TransformFn::Translate(px(a)?,0.0),
            "translateY" if args.len() == 1 => TransformFn::Translate(0.0,px(a)?),
            "scale" if args.len() <= 2 => TransformFn::Scale(ratio(a)?,ratio(b.unwrap_or(a))?),
            "scaleX" if args.len() == 1 => TransformFn::Scale(ratio(a)?,1.0),
            "scaleY" if args.len() == 1 => TransformFn::Scale(1.0,ratio(a)?),
            "rotate" if args.len() == 1 => TransformFn::Rotate(angle(a)?),
            _ => bail!("unsupported transform function: {name}"),
        };
        result.push(f);
        s = &s[close+1..];
    }
    ensure!(!result.is_empty(), "empty transform value");
    Ok(result)
}
/// Individual transform property value (`translate: x y`, `rotate: a`, `scale: x y`).
fn individual(name: &str, s: &str) -> Result<Vec<TransformFn>> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if s.trim() == "none" {
        return Ok(vec![match name { "translate" => TransformFn::Translate(0.0, 0.0), "rotate" => TransformFn::Rotate(0.0), _ => TransformFn::Scale(1.0, 1.0) }]);
    }
    Ok(vec![match name {
        "translate" => {
            ensure!((1..=3).contains(&parts.len()), "translate: {s}");
            if let Some(z) = parts.get(2) { ensure!(px(z)? == 0.0, "3D translate unsupported: {s}"); }
            TransformFn::Translate(px(parts[0])?, px(parts.get(1).copied().unwrap_or("0"))?)
        }
        "rotate" => {
            ensure!(parts.len() == 1 || (parts.len() == 2 && parts[0] == "z"), "only 2D rotate supported: {s}");
            TransformFn::Rotate(angle(parts[parts.len() - 1])?)
        }
        _ => {
            ensure!((1..=3).contains(&parts.len()), "scale: {s}");
            if let Some(z) = parts.get(2) { ensure!(ratio(z)? == 1.0, "3D scale unsupported: {s}"); }
            TransformFn::Scale(ratio(parts[0])?, ratio(parts.get(1).copied().unwrap_or(parts[0]))?)
        }
    }])
}
/// Properties whose changes only repaint (no layout). Everything else that is not
/// composite is treated as layout (re-laid out natively per state).
const PAINT_PROPS: [&str; 22] = ["color", "background-color", "background-image", "border-color", "border-top-color", "border-right-color", "border-bottom-color", "border-left-color", "border-block-color", "border-inline-color", "border-block-start-color", "border-block-end-color", "border-inline-start-color", "border-inline-end-color", "box-shadow", "text-shadow", "filter", "outline-color", "fill", "stroke", "text-decoration-color", "caret-color"];
fn value(p: &Property<'_>) -> Result<(Stage, Value)> {
    let name = prop_name(p);
    let s = prop_value(p);
    if let Property::Transform(list, _) = p {
        // Serialization drops zero units and zero second arguments. Validate
        // lengths first so even 0% and 0em cannot accidentally become pixels.
        let check_px = |v: &LengthPercentage| -> Result<()> {
            ensure!(matches!(v, LengthPercentage::Dimension(LengthValue::Px(_))),
                "translate requires px (or unitless zero), got {}", css(v));
            Ok(())
        };
        let mut ok = true;
        for function in &list.0 {
            match function {
                Transform::Translate(x, y) => { ok &= check_px(x).is_ok() && check_px(y).is_ok(); }
                Transform::TranslateX(x) | Transform::TranslateY(x) => ok &= check_px(x).is_ok(),
                _ => {}
            }
        }
        // percent translates, matrices, ...: held as discrete repaint states (step easing only)
        return match (ok, transforms(&s)) {
            (true, Ok(t)) => Ok((Stage::Composite, Value::Transform(t))),
            _ => Ok((Stage::Layout, Value::Discrete(s))),
        };
    }
    if matches!(name.as_str(), "translate" | "rotate" | "scale") && !matches!(p, Property::Unparsed(_)) {
        if let Ok(t) = individual(&name, &s) { return Ok((Stage::Composite, Value::Transform(t))); }
        return Ok((Stage::Layout, Value::Discrete(s)));
    }
    if name == "opacity" {
        if let Ok(v) = ratio(&s) { return Ok((Stage::Composite, Value::Number(v))); }
    }
    let stage = if PAINT_PROPS.contains(&name.as_str()) { Stage::Paint } else { Stage::Layout };
    Ok((stage, Value::Discrete(s)))
}

#[derive(Default, Clone)]
struct Frame {
    offset: f64,
    values: BTreeMap<String, (Stage, Value, String)>,
    easing: Option<Easing>,
}
fn keyframes(rule: &KeyframesRule<'_>, errors: &mut Vec<String>) -> Vec<Frame> {
    let name = css(&rule.name);
    let mut frames: Vec<Frame> = Vec::new();
    for k in &rule.keyframes {
        let mut values = BTreeMap::new();
        let mut ease = None;
        for p in &k.declarations.important_declarations {
            errors.push(format!("@keyframes {name} / {}: !important is unsupported", prop_name(p)));
        }
        for p in &k.declarations.declarations {
            let prop = prop_name(p);
            if prop == "animation-timing-function" {
                match easing(&prop_value(p)) { Ok(e) => ease = Some(e), Err(e) => errors.push(format!("@keyframes {name} / {prop}: {e}")) }
            } else {
                match value(p) { Ok((st, v)) => { values.insert(prop, (st, v, prop_value(p))); }, Err(e) => errors.push(format!("@keyframes {name} / {prop}: {e}")) }
            }
        }
        for selector in &k.selectors {
            let offset = match selector {
                KeyframeSelector::From => 0.0,
                KeyframeSelector::To => 1.0,
                KeyframeSelector::Percentage(p) => p.0 as f64,
                _ => { errors.push(format!("@keyframes {name}: timeline-range keyframe selector unsupported")); continue; }
            };
            if !(0.0..=1.0).contains(&offset) { errors.push(format!("@keyframes {name}: offset outside [0,1]")); continue; }
            if let Some(frame) = frames.iter_mut().find(|f| f.offset == offset) {
                frame.values.extend(values.clone());
                if ease.is_some() { frame.easing = ease; }
            } else { frames.push(Frame { offset, values: values.clone(), easing: ease }); }
        }
    }
    frames.sort_by(|a,b| a.offset.total_cmp(&b.offset));
    frames
}

fn declarations(block: &DeclarationBlock<'_>, context: &str, errors: &mut Vec<String>) -> BTreeMap<String, Vec<String>> {
    let mut result = BTreeMap::new();
    for p in &block.important_declarations {
        if animation(&prop_name(p)) { errors.push(format!("{context} / {}: !important on animation is unsupported", prop_name(p))); }
    }
    for p in block.declarations.iter().chain(&block.important_declarations) {
        let name = prop_name(p);
        if name == "transition" || name.starts_with("transition-") {
            let nonzero = match p {
                Property::Transition(v,_) => v.iter().any(|t| t.duration.to_ms() != 0.0),
                Property::TransitionDuration(v,_) => v.iter().any(|t| t.to_ms() != 0.0),
                Property::Unparsed(_) | Property::Custom(_) => { errors.push(format!("{context} / {name}: unparsed transition is unsupported")); false }
                _ => false,
            };
            if nonzero { errors.push(format!("{context} / {name}: non-zero transition duration is unsupported")); }
        }
    }
    for p in &block.declarations {
        let name = prop_name(p);
        if !animation(&name) { continue; }
        if matches!(p, Property::Unparsed(_) | Property::Custom(_)) {
            errors.push(format!("{context} / {name}: unsupported or unparsed animation value {}", prop_value(p)));
            continue;
        }
        if let Property::Animation(list, _) = p {
            let mut lists: [Vec<String>; 8] = Default::default();
            for a in list {
                let vals = [css(&a.name), css(&a.duration), css(&a.timing_function), css(&a.delay), css(&a.fill_mode), css(&a.iteration_count), css(&a.direction), css(&a.play_state)];
                for (list,v) in lists.iter_mut().zip(vals) { list.push(v); }
                if css(&a.timeline) != "auto" { errors.push(format!("{context} / animation: non-auto timeline unsupported")); }
            }
            for (name,list) in LONGHANDS.iter().zip(lists) { result.insert((*name).to_owned(),list); }
        } else if LONGHANDS.contains(&name.as_str()) {
            result.insert(name, split_list(&prop_value(p)));
        } else { errors.push(format!("{context} / {name}: unsupported animation property")); }
    }
    result
}

// Inspect pseudos even inside :is/:not/:has, ignoring attribute strings.
fn static_selector(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut bracket = 0;
    let mut quote = None;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' { i += 2; continue; }
        if let Some(q) = quote { if c == q { quote = None; } i += 1; continue; }
        if c == b'\'' || c == b'"' { quote = Some(c); }
        if c == b'[' { bracket += 1; }
        if c == b']' { bracket -= 1; }
        if c == b':' && bracket == 0 {
            let start = i + 1;
            i = start;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') { i += 1; }
            let name = &s[start..i];
            if !["root", "empty", "first-child", "last-child", "only-child", "first-of-type", "last-of-type", "only-of-type", "nth-child", "nth-last-child", "nth-of-type", "nth-last-of-type", "is", "where", "not", "has", "lang", "dir"].contains(&name) { return false; }
            continue;
        }
        i += 1;
    }
    true
}

#[derive(Clone)]
struct Winner { specificity: u32, order: usize, values: Vec<String> }
type Cascade = BTreeMap<String, Winner>;
struct Collector<'a, 'h> {
    elements: &'a [ElementRef<'h>],
    cascades: Vec<Cascade>,
    frames: BTreeMap<String, Vec<Frame>>,
    errors: Vec<String>,
    order: usize,
}
impl Collector<'_, '_> {
    fn rules(&mut self, rules: &CssRuleList<'_>, forbidden: Option<&str>) {
        for rule in &rules.0 {
            match rule {
                CssRule::FontFace(_) => {},
                CssRule::Keyframes(k) => {
                    if let Some(ctx) = forbidden { self.errors.push(format!("{ctx} / @keyframes {}: nested keyframes unsupported",css(&k.name))); }
                    else { self.frames.insert(css(&k.name), keyframes(k, &mut self.errors)); }
                }
                CssRule::Style(s) => {
                    let context = css(&s.selectors);
                    let decls = declarations(&s.declarations, &context, &mut self.errors);
                    let has_animation = s.declarations.declarations.iter().chain(&s.declarations.important_declarations).any(|p| animation(&prop_name(p)));
                    if let Some(ctx) = forbidden {
                        if has_animation { self.errors.push(format!("{ctx} / selector {context}: nested animation declarations unsupported")); }
                    } else if has_animation {
                        for selector in &s.selectors.0 {
                            let text = css(selector);
                            if !static_selector(&text) { self.errors.push(format!("selector {text}: pseudo-elements/dynamic pseudo-classes unsupported")); continue; }
                            let parsed = match Selector::parse(&text) { Ok(s) => s, Err(e) => { self.errors.push(format!("selector {text}: {e:?}")); continue; } };
                            let specificity = selector.specificity();
                            for (i,elem) in self.elements.iter().enumerate() {
                                if !parsed.matches(elem) { continue; }
                                for (name,values) in &decls {
                                    let replace = self.cascades[i].get(name).is_none_or(|w| (specificity,self.order) >= (w.specificity,w.order));
                                    if replace { self.cascades[i].insert(name.clone(), Winner { specificity, order: self.order, values: values.clone() }); }
                                }
                            }
                        }
                    }
                    self.order += 1;
                    self.rules(&s.rules, Some("nested style rule"));
                }
                CssRule::Media(r) => self.rules(&r.rules,Some("@media")),
                CssRule::Supports(r) => self.rules(&r.rules,Some("@supports")),
                CssRule::LayerBlock(r) => self.rules(&r.rules,Some("@layer")),
                CssRule::Container(r) => self.rules(&r.rules,Some("@container")),
                CssRule::Scope(r) => self.rules(&r.rules,Some("@scope")),
                CssRule::StartingStyle(r) => self.rules(&r.rules,Some("@starting-style")),
                CssRule::MozDocument(r) => self.rules(&r.rules,Some("@-moz-document")),
                _ => {
                    // Other at-rules can contain arbitrary token blocks. Look for
                    // declaration names, not words inside quoted values.
                    let serialized = css(rule);
                    for name in forbidden_tokens(&serialized) {
                        self.errors.push(format!("unsupported nested rule / {name}: animation declarations or keyframes unsupported"));
                    }
                }
            }
        }
    }
}

fn forbidden_tokens(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\'' || b[i] == b'"' {
            let q = b[i]; i += 1;
            while i < b.len() { if b[i] == b'\\' { i += 2; } else if b[i] == q { i += 1; break; } else { i += 1; } }
        } else if b[i].is_ascii_alphabetic() || b[i] == b'@' || b[i] == b'-' {
            let start = i; i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') { i += 1; }
            let token = s[start..i].to_ascii_lowercase();
            let tail = s[i..].trim_start();
            if token == "@keyframes" || token == "@-webkit-keyframes" || (animation(&token) && tail.starts_with(':')) { result.push(token); }
        } else { i += 1; }
    }
    result
}

// lightningcss folds constant calc() expressions. Preserve the unsupported-input
// check before parsing loses that spelling; only animated transforms are checked.
fn raw_transform_errors(source: &str) -> Vec<String> {
    let mut cleaned = source.as_bytes().to_vec();
    let mut i = 0;
    let mut quote = None;
    while i < cleaned.len() {
        if cleaned[i] == b'\\' { i += 2; continue; }
        if let Some(q) = quote {
            if cleaned[i] == q { quote = None; }
            i += 1; continue;
        }
        if cleaned[i] == b'\'' || cleaned[i] == b'"' { quote = Some(cleaned[i]); }
        if cleaned[i..].starts_with(b"/*") {
            cleaned[i] = b' '; cleaned[i+1] = b' '; i += 2;
            while i < cleaned.len() && !cleaned[i..].starts_with(b"*/") { cleaned[i] = b' '; i += 1; }
            if i + 1 < cleaned.len() { cleaned[i] = b' '; cleaned[i+1] = b' '; i += 2; }
        } else { i += 1; }
    }
    let clean = String::from_utf8(cleaned).expect("comments replaced with ASCII spaces");
    let mut contexts: Vec<Option<String>> = vec![None];
    let mut start = 0;
    let mut depth = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut errors = Vec::new();
    for (i,c) in clean.char_indices() {
        if escaped { escaped = false; continue; }
        if c == '\\' { escaped = true; continue; }
        if let Some(q) = quote { if c == q { quote = None; } continue; }
        match c {
            '\'' | '"' => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '{' if depth == 0 => {
                let prelude = clean[start..i].trim();
                let context = if prelude.to_ascii_lowercase().starts_with("@keyframes ") || prelude.to_ascii_lowercase().starts_with("@-webkit-keyframes ") { Some(prelude.to_owned()) } else { contexts.last().cloned().flatten() };
                contexts.push(context);
                start = i + 1;
            }
            ';' | '}' if depth == 0 => {
                if let Some(Some(context)) = contexts.last() {
                    if let Some((name,value)) = clean[start..i].split_once(':') {
                        if name.trim().eq_ignore_ascii_case("transform") && value.to_ascii_lowercase().contains("calc(") {
                            errors.push(format!("{context} / transform: calc() is unsupported"));
                        }
                    }
                }
                if c == '}' && contexts.len() > 1 { contexts.pop(); }
                start = i + 1;
            }
            _ => {}
        }
    }
    errors
}

fn compatible(a: &Value, b: &Value) -> bool {
    match (a,b) {
        (Value::Transform(a), Value::Transform(b)) => a.is_empty() || b.is_empty() || (a.len() == b.len() && a.iter().zip(b).all(|(a,b)| std::mem::discriminant(a) == std::mem::discriminant(b))),
        _ => true,
    }
}
fn build_tracks(elem: usize, cascade: &Cascade, frames: &BTreeMap<String,Vec<Frame>>, tracks: &mut Vec<Track>, errors: &mut Vec<String>) {
    let lists: Vec<Vec<String>> = LONGHANDS.iter().zip(DEFAULTS).map(|(k,d)| cascade.get(*k).map(|w| w.values.clone()).unwrap_or_else(|| vec![d.to_owned()])).collect();
    for (order,name) in lists[0].iter().enumerate() {
        if name == "none" { continue; }
        let ctx = format!("element {elem} / @keyframes {name}");
        let get = |i: usize| lists[i][order % lists[i].len()].as_str();
        let before_errors = errors.len();
        let duration = time(get(1)).and_then(|v| { ensure!(v > 0.0, "must be > 0"); Ok(v) });
        let ease = easing(get(2));
        let delay = time(get(3));
        let fill = match get(4) { "none" => Ok(FillMode::None), "forwards" => Ok(FillMode::Forwards), "backwards" => Ok(FillMode::Backwards), "both" => Ok(FillMode::Both), _ => Err(anyhow!("unsupported fill mode")) };
        for (property, problem) in [("animation-duration", duration.as_ref().err()), ("animation-timing-function", ease.as_ref().err()), ("animation-delay", delay.as_ref().err()), ("animation-fill-mode", fill.as_ref().err())] {
            if let Some(e) = problem { errors.push(format!("{ctx} / {property}: {e}")); }
        }
        for (index, valid, expected) in [(5, number(get(5)).ok() == Some(1.0), "1"), (6, get(6) == "normal", "normal"), (7, get(7) == "running", "running")] {
            if !valid { errors.push(format!("{ctx} / {}: must be {expected}", LONGHANDS[index])); }
        }
        let found = frames.get(name);
        if found.is_none() { errors.push(format!("{ctx}: unknown @keyframes name")); }
        if errors.len() != before_errors { continue; }
        let (duration,easing,delay,fill) = (duration.unwrap(),ease.unwrap(),delay.unwrap(),fill.unwrap());
        let Some(frames) = found else { continue; };
        let properties: BTreeSet<_> = frames.iter().flat_map(|f| f.values.keys().cloned()).collect();
        for property in properties {
            let mut stage = Stage::Composite;
            // a track mixing interpolable and discrete keyframes (e.g. `transform` with px and
            // % translates) is held discretely as a whole
            let mixed = frames.iter().filter_map(|f| f.values.get(&property)).any(|(_, v, _)| matches!(v, Value::Discrete(_)));
            let keys: Vec<_> = frames.iter().filter_map(|f| f.values.get(&property).map(|(s,v,raw)| {
                stage = if mixed && *s == Stage::Composite { Stage::Layout } else { *s };
                let value = if mixed { Value::Discrete(raw.clone()) } else { v.clone() };
                Keyframe { offset: f.offset, value, easing: f.easing }
            })).collect();
            if mixed { stage = Stage::Layout; }
            if keys.first().is_none_or(|k| k.offset != 0.0) || keys.last().is_none_or(|k| k.offset != 1.0) {
                errors.push(format!("{ctx} / {property}: explicit offsets 0 and 1 required")); continue;
            }
            if !keys.windows(2).all(|w| compatible(&w[0].value,&w[1].value)) {
                errors.push(format!("{ctx} / {property}: adjacent transform lists are not interpolable")); continue;
            }
            tracks.push(Track { elem, property, stage, order, source: name.clone(), duration, delay, fill, easing, keyframes: keys });
        }
    }
}

#[derive(Deserialize)]
struct TextFile { tracks: Vec<RawText> }
#[derive(Deserialize)]
struct RawText { selector: String, property: String, text: String, keys: Vec<RawKey> }
#[derive(Deserialize)]
struct RawKey { t: f64, v: f64, ease: Option<String> }
fn text_tracks(dir: &Path, file: &str, elements: &[ElementRef<'_>], out: &mut Compiled, errors: &mut Vec<String>) {
    let parsed = (|| -> Result<TextFile> { Ok(serde_json::from_str(&fs::read_to_string(dir.join(file))?)?) })();
    let raw = match parsed { Ok(v) => v, Err(e) => { errors.push(format!("{file}: {e}")); return; } };
    for (i,entry) in raw.tracks.into_iter().enumerate() {
        let result = (|| -> Result<(TextTrack,Track)> {
            ensure!(entry.property == "text", "property must be text");
            ensure!(static_selector(&entry.selector), "dynamic selectors/pseudo-elements unsupported");
            let selector = Selector::parse(&entry.selector).map_err(|e| anyhow!("invalid selector: {e:?}"))?;
            let matches: Vec<_> = elements.iter().enumerate().filter(|(_,e)| selector.matches(e)).map(|(i,_)| i).collect();
            ensure!(matches.len() == 1, "selector must match exactly one element (matched {})",matches.len());
            ensure!(entry.keys.len() >= 2, "at least two keys required");
            ensure!(entry.keys.iter().all(|k| k.t.is_finite() && k.v.is_finite()), "keys must be finite");
            ensure!(entry.keys.windows(2).all(|w| w[0].t < w[1].t), "key times must be strictly increasing");
            let delay = entry.keys[0].t;
            let duration = entry.keys.last().unwrap().t - delay;
            let mut keys: Vec<_> = entry.keys.iter().map(|k| Keyframe { offset: (k.t-delay)/duration, value: Value::Number(k.v), easing: None }).collect();
            for (i,k) in entry.keys.iter().enumerate() {
                let e = easing(k.ease.as_deref().unwrap_or("linear"))?;
                if i > 0 { keys[i-1].easing = Some(e); }
            }
            let elem = matches[0];
            Ok((TextTrack { elem, text: entry.text.clone() },Track { elem, property: "text".into(), stage: Stage::Layout, order: 1000+i, source: format!("tracks.json#{i}"), delay, duration, fill: FillMode::Both, easing: Easing::Linear, keyframes: keys }))
        })();
        match result { Ok((text,track)) => { out.texts.push(text); out.tracks.push(track); }, Err(e) => errors.push(format!("{file}#{i} / selector {} / {}: {e}",entry.selector,entry.property)) }
    }
}

/// Read `index.html` and compile its supported animations, reporting all detected problems.
pub fn compile_scene(dir: &Path) -> std::result::Result<Compiled, Vec<String>> {
    let input = fs::read_to_string(dir.join("index.html")).map_err(|e| vec![format!("index.html: {e}")])?;
    let html = Html::parse_document(&input);
    let elements: Vec<_> = html.select(&Selector::parse("*").unwrap()).filter(|e| !e.ancestors().any(|a| matches!(a.value(), scraper::Node::Fragment))).collect();
    let ids: HashMap<_,_> = elements.iter().enumerate().map(|(i,e)| (e.id(),i)).collect();
    let mut out = Compiled { tags: elements.iter().map(|e| e.value().name().to_ascii_lowercase()).collect(), parent: elements.iter().map(|e| e.parent().and_then(|p| ids.get(&p.id()).copied())).collect(), tracks: vec![], texts: vec![], duration: 0.0, fps: 30.0, recorded: false };
    let mut collector = Collector { elements: &elements, cascades: vec![BTreeMap::new(); elements.len()], frames: BTreeMap::new(), errors: vec![], order: 0 };
    let mut duration_seen = false;
    let mut track_file = None;
    for (id,el) in elements.iter().enumerate() {
        // inline `animation` declarations (the recorder emits them): highest specificity
        if let Some(style) = el.attr("style").filter(|s| { let s = s.to_ascii_lowercase(); s.contains("animation") || s.contains("transition") }) {
            match DeclarationBlock::parse_string(style, ParserOptions::default()) {
                Err(e) => collector.errors.push(format!("element {id} <{}> / style: {e}", el.value().name())),
                Ok(block) => {
                    let ctx = format!("element {id} <{}> / style", el.value().name());
                    let decls = declarations(&block, &ctx, &mut collector.errors);
                    for (name, values) in decls {
                        collector.cascades[id].insert(name, Winner { specificity: u32::MAX, order: usize::MAX, values });
                    }
                }
            }
        }
        if el.value().name() == "meta" {
            let name = el.attr("name").unwrap_or("").to_ascii_lowercase();
            let content = el.attr("content").unwrap_or("");
            if name == "reel-duration" || name == "reel-fps" {
                if name == "reel-duration" { duration_seen = true; }
                match number(content) {
                    Ok(v) if v > 0.0 => { if name == "reel-duration" { out.duration = v; } else { out.fps = v; } }
                    _ => collector.errors.push(format!("meta {name}: content must be a positive finite number")),
                }
            } else if name == "reel-tracks" { track_file = Some(content.to_owned()); }
            else if name == "reel-frames" { out.recorded = true; }
        }
        let stylesheet = if el.value().name() == "style" { Some((format!("<style> element {id}"), Ok(el.text().collect::<String>()))) }
            else if el.value().name() == "link" && el.attr("rel").is_some_and(|r| r.split_ascii_whitespace().any(|w| w.eq_ignore_ascii_case("stylesheet"))) {
                let href = el.attr("href").unwrap_or("");
                Some((href.to_owned(), fs::read_to_string(dir.join(href))))
            } else { None };
        if let Some((name,result)) = stylesheet {
            match result {
                Err(e) => collector.errors.push(format!("stylesheet {name}: {e}")),
                Ok(source) => {
                    collector.errors.extend(raw_transform_errors(&source));
                    match StyleSheet::parse(&source, ParserOptions::default()) {
                    Err(e) => collector.errors.push(format!("stylesheet {name}: {e}")),
                    Ok(sheet) => collector.rules(&sheet.rules,None),
                    }
                }
            }
        }
    }
    if !duration_seen { collector.errors.push("required meta reel-duration is missing".into()); }
    for (elem,cascade) in collector.cascades.iter().enumerate() { build_tracks(elem,cascade,&collector.frames,&mut out.tracks,&mut collector.errors); }
    if let Some(file) = track_file { text_tracks(dir,&file,&elements,&mut out,&mut collector.errors); }
    if collector.errors.is_empty() { Ok(out) } else { Err(collector.errors) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(source: &str) -> (Vec<Track>,Vec<String>) {
        let html = Html::parse_document("<div id='x'></div>");
        let elements: Vec<_> = html.select(&Selector::parse("*").unwrap()).collect();
        let mut c = Collector { elements: &elements, cascades: vec![BTreeMap::new(); elements.len()], frames: BTreeMap::new(), errors: raw_transform_errors(source), order: 0 };
        match StyleSheet::parse(source,ParserOptions::default()) {
            Ok(sheet) => c.rules(&sheet.rules,None),
            Err(e) => c.errors.push(e.to_string()),
        }
        let mut tracks = vec![];
        for (i,cascade) in c.cascades.iter().enumerate() { build_tracks(i,cascade,&c.frames,&mut tracks,&mut c.errors); }
        (tracks,c.errors)
    }
    #[test]
    fn easing_validation_and_units() {
        assert_eq!(easing("steps(4)").unwrap(),Easing::Steps(4,StepPos::JumpEnd));
        assert_eq!(easing("steps(4, jump-start)").unwrap(),Easing::Steps(4,StepPos::JumpStart));
        for s in ["steps(0)","steps(1, jump-none)","steps(-2)","linear(0, 1)","cubic-bezier(2, 0, 1, 1)"] { assert!(easing(s).is_err(),"{s}"); }
        assert_eq!(time("2400ms").unwrap(),2.4);
        assert_eq!(time("2.400000095s").unwrap(),2.4);
        assert_eq!(transforms("translate(10px, 0) scale(50%, 200%) rotate(0.25turn)").unwrap(),vec![TransformFn::Translate(10.0,0.0),TransformFn::Scale(0.5,2.0),TransformFn::Rotate(90.0)]);
        assert_eq!(angle("100grad").unwrap(),90.0);
        for s in ["translate(1em)","translate(10%)","matrix(1,0,0,1,0,0)","skew(1deg)","rotateX(1deg)","translate3d(0,0,0)"] { assert!(transforms(s).is_err(),"{s}"); }
    }
    #[test]
    fn independent_animation_errors_and_invalid_lists() {
        let (_,errors) = parse("#x { animation: missing 0s infinite alternate paused }");
        let all = errors.join("\n");
        for name in ["animation-duration","animation-iteration-count","animation-direction","animation-play-state","unknown @keyframes"] { assert!(all.contains(name),"{all}"); }
        let (_,errors) = parse("@keyframes a { from { transform: translateX(0) } to { transform: rotate(1deg) } } #x {animation: a 1s}");
        assert!(errors.iter().any(|e|e.contains("not interpolable")));
        let (_,errors) = parse("@keyframes a { from { transform: translateX(calc(1px + 2px)) } to { transform: none } } #x {animation: a 1s}");
        assert!(errors.iter().any(|e|e.contains("calc()")),"{errors:?}");
    }
    #[test]
    fn zero_units_cannot_bypass_transform_validation() {
        // M2: non-px translates are never read as pixels; the track is held discretely
        for value in ["translate(10px, 0%)", "translate(10px, 0em)", "translateX(0%)", "translateY(0vh)"] {
            let source = format!("@keyframes a {{ from {{transform: {value}}} to {{transform: none}} }} #x {{animation: a 1s}}");
            let (tracks,errors) = parse(&source);
            assert!(errors.is_empty(), "{value}: {errors:?}");
            assert!(tracks.iter().all(|t| t.stage != Stage::Composite && t.keyframes.iter().all(|k| matches!(k.value, Value::Discrete(_)))), "{value}: {tracks:?}");
        }
    }
    #[test]
    fn static_rules_and_quoted_words_are_ignored() {
        let (_,errors) = parse("@media print { div { content: 'animation: a'; color: red } } @supports (display: grid) { p { opacity: 1 } } div {transition: opacity 0s; transform: translateX(calc(1px + 2px))}");
        assert!(errors.is_empty(),"{errors:?}");
        assert!(static_selector("[title=':hover'] > div:nth-child(2):not(.hidden)"));
        assert!(!static_selector("div:is(.ready, :hover)"));
        assert!(!static_selector("div::after"));
        assert_eq!(forbidden_tokens("@unknown { content: 'animation: foo'; animation-name: a; }"),vec!["animation-name"]);
    }
    #[test]
    fn per_property_cascade_and_source_order() {
        let (tracks,errors) = parse("@keyframes a { from { opacity: 0 } to {opacity: 1} } #x {animation-duration: 4s} div {animation: a 1s} #x {animation-delay: 1s} #x {animation-delay: 2s}");
        assert!(errors.is_empty(),"{errors:?}");
        assert_eq!(tracks.len(),1);
        assert_eq!(tracks[0].duration,4.0);
        assert_eq!(tracks[0].delay,2.0);
    }
    #[test]
    fn shorthand_resets_longhands_and_multiple_selectors() {
        let (tracks,errors) = parse("@keyframes a { from {opacity:0} to {opacity:1} } #x {animation-delay: 5s; animation: a 2s linear} div, #x {animation-duration: 3s}");
        assert!(errors.is_empty(),"{errors:?}");
        assert_eq!(tracks.len(),1);
        assert_eq!(tracks[0].delay,0.0);
        assert_eq!(tracks[0].duration,3.0);
    }
}
