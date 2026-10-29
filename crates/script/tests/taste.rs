// Taste lints over hand-made energy maps (no layout, no render).
use agentfx_script::diag::Diag;
use agentfx_script::energy::EnergyMap;
use agentfx_script::taste::{self, TasteCfg};
use agentfx_script::timing::{SayT, SceneT, Timeline};

const FPS: f64 = 10.0;

/// Energy map at 10 fps from `(seconds, energy)` segments, e.g. `&[(2.0, 0.0), (1.0, 0.3)]`.
fn emap(segs: &[(f64, f32)]) -> EnergyMap {
    let mut frames = vec![];
    for (secs, e) in segs {
        frames.extend(std::iter::repeat(*e).take((secs * FPS).round() as usize));
    }
    EnergyMap::new(FPS, 1920, 1080, frames)
}

/// Timeline with one scene and one narration `say` per `(start, end)` range (seconds).
fn tl_say(total: f64, says: &[(f64, f64)]) -> Timeline {
    let says = says
        .iter()
        .enumerate()
        .map(|(i, (s, e))| SayT { key: format!("k{i}"), id: None, start: *s, end: *e, audio_at: *s, audio_dur: e - s, mp3: None, written: String::new(), words: vec![], estimated: true, span: Default::default() })
        .collect();
    Timeline { fps: FPS, total, scenes: vec![SceneT { id: "s".into(), start: 0.0, dur: total, says, anchors: vec![], span: Default::default() }] }
}

fn codes(d: &[Diag]) -> Vec<&str> {
    d.iter().map(|x| x.code.as_str()).collect()
}

#[test]
fn helpers_work() {
    let e = emap(&[(1.0, 0.0), (1.0, 0.5)]);
    assert_eq!(e.len(), 20);
    let t = tl_say(2.0, &[(0.5, 1.5)]);
    assert_eq!(taste::voiced(&t), vec![(0.5, 1.5)]);
    assert!(codes(&[]).is_empty());
    let _ = TasteCfg::default();
}

#[test]
fn small_motion_tiny_peak_warns() {
    let e = emap(&[(1.0, 0.0), (3.0, 0.01), (1.0, 0.0)]);
    let d = taste::small_motion(&e, &TasteCfg::default());
    assert_eq!(codes(&d), vec!["W_SMALL_MOTION"]);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].at, Some(1.0));
}

#[test]
fn small_motion_peak_is_enough() {
    let e = emap(&[(1.0, 0.0), (3.0, 0.30), (1.0, 0.0)]);
    let d = taste::small_motion(&e, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}

#[test]
fn small_motion_span_too_short() {
    let e = emap(&[(1.0, 0.0), (1.0, 0.01), (3.0, 0.0)]);
    let d = taste::small_motion(&e, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}

#[test]
fn small_motion_gap_merges_into_one_warning() {
    let e = emap(&[(1.0, 0.01), (0.3, 0.0), (1.0, 0.01), (2.0, 0.0)]);
    let d = taste::small_motion(&e, &TasteCfg::default());
    assert_eq!(codes(&d), vec!["W_SMALL_MOTION"]);
    assert_eq!(d.len(), 1);
}

#[test]
fn small_motion_fully_still_film() {
    let e = emap(&[(5.0, 0.0)]);
    let d = taste::small_motion(&e, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}

#[test]
fn dead_air_voiced_still_warns() {
    let e = emap(&[(1.0, 0.2), (2.0, 0.0), (1.0, 0.2)]);
    let tl = tl_say(4.0, &[(0.0, 4.0)]);
    let d = taste::dead_air(&e, &tl, &TasteCfg::default());
    assert_eq!(codes(&d), vec!["W_DEAD_AIR"]);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].at, Some(1.0));
}

#[test]
fn dead_air_voiced_overlap_below_threshold_is_ignored() {
    let e = emap(&[(1.0, 0.2), (2.0, 0.0), (1.0, 0.2)]);
    let tl = tl_say(4.0, &[(0.0, 1.0)]);
    let d = taste::dead_air(&e, &tl, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}

#[test]
fn dead_air_without_voiced_warns() {
    let e = emap(&[(1.0, 0.2), (2.5, 0.0), (1.0, 0.2)]);
    let tl = tl_say(4.5, &[]);
    let d = taste::dead_air(&e, &tl, &TasteCfg::default());
    assert_eq!(codes(&d), vec!["W_DEAD_AIR"]);
    assert_eq!(d.len(), 1);
}

#[test]
fn dead_air_without_voiced_short_still_is_ignored() {
    let e = emap(&[(1.0, 0.2), (1.5, 0.0), (1.0, 0.2)]);
    let tl = tl_say(3.5, &[]);
    let d = taste::dead_air(&e, &tl, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}

#[test]
fn dead_air_still_tail_is_static_end() {
    let e = emap(&[(1.0, 0.2), (3.0, 0.0)]);
    let tl = tl_say(4.0, &[(0.0, 4.0)]);
    let d = taste::dead_air(&e, &tl, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}

#[test]
fn dead_air_still_start_is_cold_open() {
    let e = emap(&[(3.0, 0.0), (1.0, 0.2)]);
    let tl = tl_say(4.0, &[(0.0, 4.0)]);
    let d = taste::dead_air(&e, &tl, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}

#[test]
fn dead_air_tiny_motion_counts_as_still() {
    let e = emap(&[(1.0, 0.2), (2.0, 0.001), (1.0, 0.2)]);
    let tl = tl_say(4.0, &[(0.0, 4.0)]);
    let d = taste::dead_air(&e, &tl, &TasteCfg::default());
    assert_eq!(codes(&d), vec!["W_DEAD_AIR"]);
    assert_eq!(d.len(), 1);
}

#[test]
fn static_ending_dead_tail_warns() {
    let e = emap(&[(3.0, 0.3), (2.0, 0.0)]);
    let d = taste::static_ending(&e, &TasteCfg::default());
    assert_eq!(codes(&d), vec!["W_STATIC_END"]);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].at, Some(3.0));
}

#[test]
fn static_ending_short_hold_is_fine() {
    let e = emap(&[(3.0, 0.3), (1.0, 0.0)]);
    let d = taste::static_ending(&e, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}

#[test]
fn static_ending_small_last_move_warns() {
    let e = emap(&[(3.0, 0.3), (2.0, 0.02)]);
    let d = taste::static_ending(&e, &TasteCfg::default());
    assert_eq!(codes(&d), vec!["W_STATIC_END"]);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].at, Some(3.0));
}

#[test]
fn static_ending_motion_is_enough() {
    let e = emap(&[(3.0, 0.3), (2.0, 0.3)]);
    let d = taste::static_ending(&e, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}

#[test]
fn static_ending_whole_film_still_warns() {
    let e = emap(&[(5.0, 0.0)]);
    let d = taste::static_ending(&e, &TasteCfg::default());
    assert_eq!(codes(&d), vec!["W_STATIC_END"]);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].at, Some(0.0));
}

#[test]
fn static_ending_moving_end_ignores_middle_still() {
    let e = emap(&[(3.0, 0.3), (1.0, 0.0), (1.0, 0.2)]);
    let d = taste::static_ending(&e, &TasteCfg::default());
    assert!(codes(&d).is_empty());
}
