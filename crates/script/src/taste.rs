//! Taste lints over the motion-energy map (`energy.rs`): the failure modes an operator spots by
//! eye in a finished film. Each rule is a pure function of the energy map (and the narration
//! timeline); all thresholds live in `TasteCfg` (theme `lint` tokens override the defaults).
//!
//! Codes: W_SMALL_MOTION, W_DEAD_AIR, W_STATIC_END. (W_COLD_OPEN lives in `principles.rs`.)
use crate::diag::Diag;
use crate::energy::{EnergyMap, Run};
use crate::timing::Timeline;

#[derive(Debug, Clone)]
pub struct TasteCfg {
    /// master switch (`lint motion=false`)
    pub enabled: bool,
    /// energy at or below this is "nothing changes" (fraction of frame area)
    pub still_eps: f32,
    /// energy below this counts as dead for dead-air (a blinking caret is not motion worth keeping alive)
    pub dead_eps: f32,
    /// a moving span must at some point move at least this fraction of the frame (`min-area`)
    pub min_area: f32,
    /// shortest span (s) that small-motion judges (`small-span`)
    pub small_span: f64,
    /// dead air while narration plays: longest still stretch (s) (`max-still`)
    pub max_still: f64,
    /// dead air without narration under it: longest still stretch (s) (`max-still-silent`)
    pub max_still_silent: f64,
    /// final window (s) the ending rules look at (`end-window`)
    pub end_window: f64,
    /// longest still tail (s) before the film counts as ending static (`end-still`)
    pub end_still: f64,
}

impl Default for TasteCfg {
    fn default() -> Self {
        TasteCfg { enabled: true, still_eps: 0.0001, dead_eps: 0.003, min_area: 0.08, small_span: 1.5, max_still: 1.0, max_still_silent: 2.0, end_window: 2.0, end_still: 2.0 }
    }
}

impl TasteCfg {
    /// Apply one `lint` token. Areas are `8%` or `0.08`; durations are `0.8s`, `800ms` or `0.8`.
    pub fn set(&mut self, key: &str, raw: &str) -> Result<(), String> {
        let secs = |r: &str| -> Option<f64> {
            let r = r.trim();
            if let Some(ms) = r.strip_suffix("ms") { return ms.trim().parse::<f64>().ok().map(|v| v / 1000.0); }
            r.trim_end_matches('s').trim().parse::<f64>().ok()
        };
        let frac = |r: &str| -> Option<f32> {
            let r = r.trim();
            match r.strip_suffix('%') { Some(p) => p.trim().parse::<f32>().ok().map(|v| v / 100.0), None => r.parse::<f32>().ok() }
        };
        let bad = || format!("lint {key}={raw}: expected a {}", if key == "min-area" { "percentage such as 8%" } else if key == "motion" { "boolean" } else { "duration such as 1.2s" });
        match key {
            "motion" => self.enabled = raw != "false" && raw != "off",
            "min-area" => self.min_area = frac(raw).ok_or_else(bad)?,
            "max-still" => self.max_still = secs(raw).ok_or_else(bad)?,
            "max-still-silent" => self.max_still_silent = secs(raw).ok_or_else(bad)?,
            "small-span" => self.small_span = secs(raw).ok_or_else(bad)?,
            "end-window" => self.end_window = secs(raw).ok_or_else(bad)?,
            "end-still" => self.end_still = secs(raw).ok_or_else(bad)?,
            _ => return Err(format!("unknown taste token {key}")),
        }
        Ok(())
    }
}

/// All taste rules over one energy map.
pub fn rules(e: &EnergyMap, tl: &Timeline, cfg: &TasteCfg) -> Vec<Diag> {
    if !cfg.enabled || e.is_empty() {
        return vec![];
    }
    let mut d = vec![];
    d.extend(small_motion(e, cfg));
    d.extend(dead_air(e, tl, cfg));
    d.extend(static_ending(e, cfg));
    d
}

/// Narration stretches (start, end) in seconds, merged.
pub fn voiced(tl: &Timeline) -> Vec<Run> {
    let mut v: Vec<Run> = tl.scenes.iter().flat_map(|s| s.says.iter().map(|x| (x.start, x.end))).filter(|r| r.1 > r.0).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut out: Vec<Run> = vec![];
    for r in v {
        match out.last_mut() {
            Some(l) if r.0 <= l.1 + 1e-6 => l.1 = l.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

/// Seconds of `[a, b]` covered by the (sorted, merged) runs.
pub fn overlap(runs: &[Run], a: f64, b: f64) -> f64 {
    runs.iter().map(|r| (r.1.min(b) - r.0.max(a)).max(0.0)).sum()
}

/// W_SMALL_MOTION: a span of motion (>= `small_span`) whose peak energy stays under `min_area`.
pub fn small_motion(e: &EnergyMap, cfg: &TasteCfg) -> Vec<Diag> {
    let mut out: Vec<Diag> = Vec::new();
    for (a, b) in e.moving_runs(cfg.still_eps, 0.5) {
        let peak = f64::from(e.max_in(a, b));
        if b - a >= cfg.small_span && peak < f64::from(cfg.min_area) {
            out.push(
                Diag::warn(
                    "W_SMALL_MOTION",
                    format!(
                        "all motion in {:.1}-{:.1}s stays within {:.1} % of the frame (peak), under the {:.0} % minimum",
                        a, b, peak * 100.0, cfg.min_area as f64 * 100.0
                    ),
                    "scale the moving hero up to at least 40 % of the frame, or add a camera move (`zoom`/`dolly`) so the motion reads",
                    crate::ast::Span::default(),
                )
                .with_at(a),
            );
        }
    }
    out
}

/// W_DEAD_AIR: a still stretch (energy <= `dead_eps`) longer than `max_still` under narration, or `max_still_silent` without.
pub fn dead_air(e: &EnergyMap, tl: &Timeline, cfg: &TasteCfg) -> Vec<Diag> {
    let mut out: Vec<Diag> = Vec::new();
    let v = voiced(tl);
    for (a, b) in e.still_runs(cfg.dead_eps, cfg.max_still.min(cfg.max_still_silent)) {
        if a < 1e-6 || b >= e.duration() - 1e-6 {
            continue;
        }
        let len = b - a;
        if !v.is_empty() {
            if overlap(&v[..], a, b) >= cfg.max_still {
                out.push(
                    Diag::warn(
                        "W_DEAD_AIR",
                        format!("nothing moves for {len:.1}s ({a:.1}-{b:.1}s) while the narration continues"),
                        "anchor a beat to a word in that line (`@word` plus an enter, emphasis or move) or add a slow camera drift",
                        crate::ast::Span::default(),
                    )
                    .with_at(a),
                );
            }
        } else if len >= cfg.max_still_silent {
            out.push(
                Diag::warn(
                    "W_DEAD_AIR",
                    format!("nothing moves for {len:.1}s ({a:.1}-{b:.1}s)"),
                    "anchor a beat to a word in that line (`@word` plus an enter, emphasis or move) or add a slow camera drift",
                    crate::ast::Span::default(),
                )
                .with_at(a),
            );
        }
    }
    out
}

/// W_STATIC_END: the film ends on >= `end_still` of stillness, or its last `end_window` only moves a small element.
pub fn static_ending(e: &EnergyMap, cfg: &TasteCfg) -> Vec<Diag> {
    if e.is_empty() {
        return vec![];
    }
    let dur = e.duration();
    let mut tail_frames = 0usize;
    while tail_frames < e.frames.len() && e.frames[e.frames.len() - 1 - tail_frames] <= cfg.dead_eps {
        tail_frames += 1;
    }
    let tail = tail_frames as f64 / e.fps;
    let tail_start = dur - tail;
    if tail >= cfg.end_still {
        return vec![Diag::warn(
            "W_STATIC_END",
            format!("the film ends on {tail:.1}s without motion (from {tail_start:.1}s)"),
            "end on a move: finish the last beat with an exit, a slow push-in or a settle, and keep the final hold under a second",
            crate::ast::Span::default(),
        )
        .with_at(tail_start)];
    }
    let w0 = (dur - cfg.end_window).max(0.0);
    let peak = e.max_in(w0, dur);
    if peak > cfg.dead_eps && peak < cfg.min_area {
        return vec![Diag::warn(
            "W_STATIC_END",
            format!(
                "the last {:.1}s only move {:.1} % of the frame (peak): the film ends on a small element",
                dur - w0,
                peak as f64 * 100.0
            ),
            "end on the hero: scale the last element up (>= 40 % of the frame) or finish with a camera move",
            crate::ast::Span::default(),
        )
        .with_at(w0)];
    }
    vec![]
}
