//! Opt-in render profiler (`AGENTFX_PROFILE=1`): per-stage wall time and pixel counts.
//!
//! `let _p = prof::scope("fx.riso");` times the rest of the block. Stages nest: a stage's
//! *exclusive* time excludes the stages opened inside it, so the exclusive times of one
//! frame add up to the frame's wall time. Pixels (`prof::px`) count the buffer area a stage
//! processed. Totals are thread-local (one segment = one thread); rayon work inside a stage
//! is wall time of that stage. `render.rs` drains them per frame (per-frame JSONL with
//! `AGENTFX_PROFILE_OUT=path`) and prints a summary table to stderr at the end.
//! Off (the default): one relaxed atomic load per scope.
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| agentfx_ir::env::var("AGENTFX_PROFILE").is_ok_and(|v| v != "0"))
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Acc {
    /// exclusive / inclusive wall seconds
    pub excl: f64,
    pub incl: f64,
    pub calls: u64,
    pub px: u64,
}

struct Open {
    name: &'static str,
    t0: Instant,
    child: f64,
}

thread_local! {
    static STACK: RefCell<Vec<Open>> = const { RefCell::new(Vec::new()) };
    static TOTALS: RefCell<HashMap<&'static str, Acc>> = RefCell::new(HashMap::new());
}

#[must_use]
pub struct Scope(bool);

/// Time the enclosing block as stage `name`.
#[inline]
pub fn scope(name: &'static str) -> Scope {
    if !on() {
        return Scope(false);
    }
    STACK.with(|s| s.borrow_mut().push(Open { name, t0: Instant::now(), child: 0.0 }));
    Scope(true)
}

/// Time the block as `name` and count `px` pixels for it.
#[inline]
pub fn scope_px(name: &'static str, px: u64) -> Scope {
    let s = scope(name);
    if s.0 {
        add_px(name, px);
    }
    s
}

#[inline]
pub fn add_px(name: &'static str, px: u64) {
    if !on() {
        return;
    }
    TOTALS.with(|t| t.borrow_mut().entry(name).or_default().px += px);
}

impl Drop for Scope {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let (name, dt, child) = STACK.with(|s| {
            let mut s = s.borrow_mut();
            let o = s.pop().expect("prof stack");
            let dt = o.t0.elapsed().as_secs_f64();
            if let Some(p) = s.last_mut() {
                p.child += dt;
            }
            (o.name, dt, o.child)
        });
        TOTALS.with(|t| {
            let mut t = t.borrow_mut();
            let a = t.entry(name).or_default();
            a.excl += dt - child;
            a.incl += dt;
            a.calls += 1;
        });
    }
}

/// Drain this thread's totals.
pub fn take() -> HashMap<&'static str, Acc> {
    TOTALS.with(|t| std::mem::take(&mut *t.borrow_mut()))
}

pub fn merge(into: &mut HashMap<&'static str, Acc>, from: &HashMap<&'static str, Acc>) {
    for (k, v) in from {
        let a = into.entry(k).or_default();
        a.excl += v.excl;
        a.incl += v.incl;
        a.calls += v.calls;
        a.px += v.px;
    }
}

/// Summary table: stages by exclusive time, ms per composited frame, share of the total.
pub fn report(totals: &HashMap<&'static str, Acc>, frames: usize, full_frames: usize, composited: usize, wall: f64) -> String {
    let mut v: Vec<_> = totals.iter().collect();
    v.sort_by(|a, b| b.1.excl.partial_cmp(&a.1.excl).unwrap());
    let total: f64 = v.iter().map(|x| x.1.excl).sum();
    let per = composited.max(1) as f64;
    let mut s = format!(
        "agentfx-render profile: {frames} frames, {composited} composited, {full_frames} full-damage ({:.0} %), wall {:.2} s, {:.1} ms/composited frame\n",
        100.0 * full_frames as f64 / frames.max(1) as f64,
        wall,
        1e3 * total / per
    );
    s += &format!("{:<22} {:>10} {:>10} {:>7} {:>10} {:>10} {:>9}\n", "stage", "excl ms/f", "incl ms/f", "share", "calls/f", "Mpx/f", "excl s");
    for (k, a) in v {
        s += &format!(
            "{:<22} {:>10.2} {:>10.2} {:>6.1}% {:>10.1} {:>10.2} {:>9.2}\n",
            k,
            1e3 * a.excl / per,
            1e3 * a.incl / per,
            100.0 * a.excl / total.max(1e-12),
            a.calls as f64 / per,
            a.px as f64 / 1e6 / per,
            a.excl
        );
    }
    s
}

pub fn to_json(t: &HashMap<&'static str, Acc>) -> serde_json::Value {
    let m: serde_json::Map<String, serde_json::Value> = t.iter().map(|(k, a)| (k.to_string(), serde_json::json!([(a.excl * 1e5).round() / 100.0, a.calls, a.px]))).collect();
    serde_json::Value::Object(m)
}
