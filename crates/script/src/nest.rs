//! `nest #id levels=N scale=k rot=r [cycle=[a, b, c]]`: a self-similar stack. The nest's children
//! are one level (drawn in a box the size of the frame, or `w=`/`h=`, centered at `at x,y` or the
//! frame center); the nest places `levels` copies of it, copy i scaled by k^i and rotated by r*i
//! degrees about the box center. `camera loop #id` then zooms exactly one level per loop.
//!
//! Expanded on the AST (in `lower::specialize`), so verbs nested under copied elements follow
//! their copies. Inside a copy, ids get the suffix `-L<i>` (`#dot` -> `#dot-L0`, `#dot-L1`, ...),
//! `$cycle` becomes `cycle[i mod len]` and `$level` becomes i.
use crate::ast::{Node, Value};
use crate::ir::*;
use crate::lower::*;

pub const DEFAULT_LEVELS: usize = 12;
pub const MAX_LEVELS: usize = 64;
pub const DEFAULT_SCALE: f64 = 0.8;

/// Parsed nest parameters (clamped; `lower` reports bad values).
#[derive(Debug, Clone, PartialEq)]
pub struct NestP {
    pub levels: usize,
    pub scale: f64,
    pub rot: f64,
    pub cycle: Vec<Value>,
}

fn pnum(n: &Node, k: &str) -> Option<f64> {
    n.props.get(k).and_then(|v| match v {
        Value::Num(x) => Some(*x),
        Value::Str(s) => s.trim().trim_end_matches("deg").parse().ok(),
        _ => None,
    })
}

pub fn params(n: &Node) -> NestP {
    let levels = pnum(n, "levels").map(|v| v.round().max(1.0) as usize).unwrap_or(DEFAULT_LEVELS).min(MAX_LEVELS);
    let scale = pnum(n, "scale").filter(|k| *k > 0.0 && k.is_finite() && (*k - 1.0).abs() > 1e-9).unwrap_or(DEFAULT_SCALE);
    let rot = pnum(n, "rot").unwrap_or(0.0);
    let cycle = match n.props.get("cycle") {
        Some(Value::List(l)) => l.clone(),
        Some(v) => v.raw().split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| Value::Str(s.to_string())).collect(),
        None => vec![],
    };
    NestP { levels, scale, rot, cycle }
}

/// Levels per self-similar period: a `cycle=` of length n repeats every n levels.
pub fn period(p: &NestP) -> usize {
    p.cycle.len().max(1)
}

fn ids(nodes: &[Node], out: &mut Vec<String>) {
    for n in nodes {
        if let Some(Value::Str(s)) = n.args.first() {
            if let Some(id) = s.strip_prefix('#') {
                if !id.is_empty() && !crate::lower::is_verb(&n.kind) && n.kind != "at" && n.kind != "set" {
                    out.push(id.to_string());
                }
            }
        }
        ids(&n.children, out);
    }
}

fn subst(v: &mut Value, i: usize, p: &NestP, local: &[String]) {
    match v {
        Value::Str(s) => {
            if let Some(id) = s.strip_prefix('#') {
                if local.iter().any(|l| l == id) {
                    *s = format!("#{id}-L{i}");
                    return;
                }
            }
            if s.contains("$cycle") && !p.cycle.is_empty() {
                *s = s.replace("$cycle", &p.cycle[i % p.cycle.len()].raw());
            }
            if s.contains("$level") {
                if s == "$level" {
                    *v = Value::Num(i as f64);
                    return;
                }
                *s = s.replace("$level", &i.to_string());
            }
        }
        Value::List(l) => l.iter_mut().for_each(|x| subst(x, i, p, local)),
        Value::Num(_) => {}
    }
}

fn copy(n: &Node, i: usize, p: &NestP, local: &[String]) -> Node {
    let mut m = n.clone();
    m.args.iter_mut().for_each(|a| subst(a, i, p, local));
    m.props.values_mut().for_each(|v| subst(v, i, p, local));
    m.children = n.children.iter().map(|c| copy(c, i, p, local)).collect();
    m
}

/// True if any arg/prop under `nodes` spells `$cycle`.
pub fn uses_cycle(nodes: &[Node]) -> bool {
    fn has(v: &Value) -> bool {
        match v {
            Value::Str(s) => s.contains("$cycle"),
            Value::List(l) => l.iter().any(has),
            Value::Num(_) => false,
        }
    }
    nodes.iter().any(|n| n.args.iter().chain(n.props.values()).any(has) || uses_cycle(&n.children))
}

/// Replace each `nest`'s children with its generated levels (innermost first, so outer levels
/// paint on top). The nest keeps its props; `nest-template-cycle` records whether `$cycle` was
/// used (for lint).
pub fn expand(nodes: &mut [Node]) {
    for n in nodes.iter_mut() {
        expand(&mut n.children);
        if n.kind != "nest" || n.props.contains_key("nest-generated") {
            continue;
        }
        let p = params(n);
        let nest_id = n.args.first().and_then(|a| a.as_str()).and_then(|s| s.strip_prefix('#')).map(|s| s.to_string());
        let mut local = vec![];
        ids(&n.children, &mut local);
        let template = std::mem::take(&mut n.children);
        let mut levels = vec![];
        for i in (0..p.levels).rev() {
            let mut l = Node { kind: "layer".into(), span: n.span, ..Default::default() };
            if let Some(id) = &nest_id {
                l.args.push(Value::Str(format!("#{id}-L{i}")));
            }
            // full precision (Value::Num prints 6 decimals; deep levels are 1e-4 and smaller)
            l.props.insert("scale".into(), Value::Str(format!("{}", p.scale.powi(i as i32))));
            if p.rot != 0.0 {
                l.props.insert("rot".into(), Value::Str(format!("{}", p.rot * i as f64)));
            }
            l.children = template.iter().map(|c| copy(c, i, &p, &local)).collect();
            levels.push(l);
        }
        if uses_cycle(&template) {
            n.props.insert("nest-uses-cycle".into(), Value::Str("true".into()));
        }
        n.props.insert("nest-generated".into(), Value::Str("true".into()));
        n.children = levels;
    }
}

impl<'a> Ctx<'a> {
    /// `nest #id [at x,y] levels= scale= rot= [cycle=] [w= h=]`: a box (the frame size by default)
    /// centered at `at x,y` of its parent, or at the parent's center; its children are the
    /// generated levels (`expand`).
    pub fn nest_el(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let p = params(n);
        let raw = |k: &str| n.props.get(k).map(|v| v.raw());
        if let Some(v) = raw("levels") {
            if !v.parse::<f64>().is_ok_and(|x| (1.0..=MAX_LEVELS as f64).contains(&x)) {
                self.err("E_NEST", format!("nest levels={v}: needs a whole number 1..{MAX_LEVELS}"), "write `nest #t levels=24 scale=0.75`", n.span);
            }
        }
        if let Some(v) = raw("scale") {
            if !v.parse::<f64>().is_ok_and(|k| k > 0.0 && k.is_finite() && (k - 1.0).abs() > 1e-9) {
                self.err("E_NEST", format!("nest scale={v}: the per-level factor must be > 0 and not 1"), "write `scale=0.75` (each level 0.75x the previous)", n.span);
            }
        }
        let uses = n.props.contains_key("nest-uses-cycle");
        if !p.cycle.is_empty() && !uses {
            self.warn("W_NEST_CYCLE", "nest cycle= is set but no child uses `$cycle`", "write e.g. `stroke=$cycle` on a child of the nest", n.span);
        }
        if p.cycle.is_empty() && uses {
            self.warn("W_NEST_CYCLE", "a child of the nest uses `$cycle` but the nest has no cycle=", "add `cycle=[#FF2BD6, #22E4FF]` to the nest", n.span);
        }
        let at = n.args.iter().position(|a| a.as_str() == Some("at")).and_then(|i| n.args.get(i + 1)).map(|v| v.raw()).or_else(|| raw("at"));
        let mut l = Layer::new(lid, Kind::Group);
        l.layout.pos = Pos::Abs;
        match at.as_deref().map(crate::world::parse_xy) {
            Some(Some((x, y))) => {
                l.layout.x = Some(Len::px(x));
                l.layout.y = Some(Len::px(y));
            }
            Some(None) => {
                self.err("E_NEST", format!("nest `at {}` is not x,y", at.unwrap_or_default()), "write `nest #t at 960,540` or leave `at` out to center it", n.span);
            }
            None => {
                l.layout.x = Some(Len::pct(50.0));
                l.layout.y = Some(Len::pct(50.0));
            }
        }
        l.layout.anchor = (0.5, 0.5);
        l.layout.w = Some(Len::px(self.w));
        l.layout.h = Some(Len::px(self.h));
        l.layout.flow = "none";
        l.tags.push("nest".into());
        l.tags.push(format!("nest:{}:{}:{}:{}", p.scale, p.rot, p.levels, period(&p)));
        self.add(Some(parent), l)
    }
}

/// Nest parameters from a layer's tag: (scale, rot, levels, period).
pub fn tag_params(tags: &[String]) -> Option<(f64, f64, usize, usize)> {
    let t = tags.iter().find_map(|t| t.strip_prefix("nest:"))?;
    let v: Vec<&str> = t.split(':').collect();
    Some((v.first()?.parse().ok()?, v.get(1)?.parse().ok()?, v.get(2)?.parse().ok()?, v.get(3)?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Value {
        Value::Str(x.into())
    }

    #[test]
    fn expands_levels_with_scale_rot_ids_and_cycle() {
        let mut dot = Node { kind: "circle".into(), args: vec![s("#dot")], ..Default::default() };
        dot.props.insert("fill".into(), s("$cycle"));
        dot.children.push(Node { kind: "anim".into(), args: vec![s("#dot"), s("opacity"), s("0s:1")], ..Default::default() });
        let mut nest = Node { kind: "nest".into(), args: vec![s("#t")], children: vec![dot], ..Default::default() };
        nest.props.insert("levels".into(), Value::Num(3.0));
        nest.props.insert("scale".into(), Value::Num(0.5));
        nest.props.insert("rot".into(), s("10deg"));
        nest.props.insert("cycle".into(), Value::List(vec![s("#f0f"), s("#0ff")]));
        let mut v = vec![nest];
        expand(&mut v);
        let lv = &v[0].children;
        assert_eq!(lv.len(), 3);
        // innermost first
        assert_eq!(lv[0].args[0], s("#t-L2"));
        assert_eq!(lv[0].props["scale"], s("0.25"));
        assert_eq!(lv[0].props["rot"], s("20"));
        assert_eq!(lv[2].props["rot"], s("0"));
        assert_eq!(lv[2].children[0].args[0], s("#dot-L0"));
        assert_eq!(lv[1].children[0].props["fill"], s("#0ff"));
        assert_eq!(lv[0].children[0].props["fill"], s("#f0f"));
        assert_eq!(lv[1].children[0].children[0].args[0], s("#dot-L1"));
        // idempotent
        let before = v.clone();
        expand(&mut v);
        assert_eq!(v, before);
        assert_eq!(period(&params(&v[0])), 2);
    }
}
