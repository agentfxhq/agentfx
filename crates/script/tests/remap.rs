// Time remap (`rewind`, `timemap`) and motion blur. The reverse proof: inside the rewound
// window the evaluated reel-ir (every node's transform, opacity, trim, ... as the renderer
// evaluates it) equals the evaluated reel-ir at the mirrored forward time, frame by frame.
use agentfx_ir::eval::eval_track;
use agentfx_ir::{Sampled, Scene};
use agentfx_script::diag::Severity;
use agentfx_script::{lower, parse, timing};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

fn build(file: &str) -> agentfx_script::ir::Ir {
    let p = Path::new(env!("AGENTFX_ROOT")).join(file);
    let src = std::fs::read_to_string(&p).unwrap();
    let (doc, errs) = parse::parse(&src);
    assert!(errs.is_empty(), "{file}: parse errors {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, mut diags) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, None, p.parent().unwrap());
    diags.extend(c.diags);
    let errs: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{file}: {errs:#?}");
    c.ir
}

fn agentfx_ir(ir: &agentfx_script::ir::Ir, tag: &str) -> Scene {
    let dir = std::env::temp_dir().join(format!("reel-remap-{tag}-{}", std::process::id()));
    let c = agentfx_script::direct::compile(ir, &dir, false).unwrap();
    let errs: Vec<_> = c.diags.iter().filter(|x| x.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let s: Scene = serde_json::from_value(c.scene).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    s
}

/// What the renderer evaluates per (node, property) at time t: the winning track (highest
/// order with an effect, later wins on ties), as numbers.
fn state(s: &Scene, t: f64) -> BTreeMap<(usize, String), Vec<f64>> {
    let mut win: BTreeMap<(usize, String), (i64, Vec<f64>)> = BTreeMap::new();
    for tr in &s.tracks {
        let Some(v) = eval_track(tr, t) else { continue };
        let v = match v {
            Sampled::Number(x) => vec![x],
            Sampled::Matrix(m) => vec![m.a, m.b, m.c, m.d, m.e, m.f],
            Sampled::Sig(sig) => vec![sig.len() as f64, sig.bytes().map(|b| b as f64).sum()],
            Sampled::Color(c) => c.to_vec(),
        };
        let key = (tr.node, tr.property.clone());
        let o = tr.order as i64;
        if win.get(&key).map(|w| o >= w.0).unwrap_or(true) {
            win.insert(key, (o, v));
        }
    }
    win.into_iter().map(|(k, v)| (k, v.1)).collect()
}

fn diff(a: &BTreeMap<(usize, String), Vec<f64>>, b: &BTreeMap<(usize, String), Vec<f64>>) -> Vec<String> {
    let mut out = vec![];
    for (k, va) in a {
        let Some(vb) = b.get(k) else {
            out.push(format!("{k:?} only on one side"));
            continue;
        };
        for (i, (x, y)) in va.iter().zip(vb).enumerate() {
            // only the emitter's 4-decimal value rounding (r4), far below the renderer's commit
            // steps (1/16 px, 1/255 opacity)
            let tol = 1.01e-4;
            if (x - y).abs() > tol {
                out.push(format!("{k:?}[{i}]: {x} vs {y}"));
            }
        }
    }
    out
}

#[test]
fn reverse_proof_rewound_frames_equal_mirrored_forward_frames() {
    let ir = build("crates/script/tests/fixtures/time-remap.reel");
    let s = agentfx_ir(&ir, "proof");
    let fps = s.fps as f64;
    // rewind at 3.0 s, 4x, 0.5 s: parent 3.0 + k/fps shows local 3.0 - 4k/fps
    let (t0, speed, n) = (3.0, 4.0, (0.5 * fps).round() as i64);
    let mut compared = 0;
    let mut moving = 0;
    for k in 1..=n {
        let t = t0 + k as f64 / fps;
        let mirror = t0 - speed * k as f64 / fps;
        let (a, b) = (state(&s, t), state(&s, mirror));
        let d = diff(&a, &b);
        assert!(d.is_empty(), "frame at {t:.4}s != forward frame at {mirror:.4}s:\n{}", d.join("\n"));
        compared += a.len();
        // damage: a rewound frame changes exactly when its mirrored forward frames differ
        if k > 1 {
            let changed = diff(&a, &state(&s, t - 1.0 / fps)).len() > 0;
            let fwd_changed = diff(&b, &state(&s, mirror + speed / fps)).len() > 0;
            assert_eq!(changed, fwd_changed, "change at {t:.4}s vs forward {mirror:.4}s");
            moving += changed as i64;
        }
    }
    assert!(compared > 100, "too few tracks compared: {compared}");
    assert!(moving >= n / 2, "the rewound window should mostly move ({moving}/{n})");
    // everything was in the window: particles, split glyphs, spring/steps/cubic, trim, yoyo
    let names: Vec<&str> = s.tracks.iter().map(|t| s.nodes[t.node].name.as_str()).collect();
    for want in ["spray-0-p3", "word-0-", "dot", "box", "line", "st"] {
        assert!(names.iter().any(|n| if want == "st" { *n == "st" } else { n.contains(want) }), "no track for {want}: {names:?}");
    }
    assert!(s.tracks.iter().any(|t| t.property == "trim-end"), "no trim track");
    // after the rewind the film holds: no change, no damage
    let held = state(&s, 3.5 + 1.0 / fps);
    for k in 2..10 {
        let d = diff(&held, &state(&s, 3.5 + k as f64 / fps));
        assert!(d.is_empty(), "held film changed: {d:?}");
    }
}

#[test]
fn frozen_remap_produces_no_change() {
    let ir = build("crates/script/tests/fixtures/time-remap.reel");
    let s = agentfx_ir(&ir, "freeze");
    let fps = s.fps as f64;
    // #ball moves from 4.0 s; #side is frozen from 4.3 s on
    assert_ne!(state(&s, 4.1), state(&s, 4.2), "ball should move before the freeze");
    let at = state(&s, 4.3);
    for k in 1..20 {
        assert_eq!(at, state(&s, 4.3 + k as f64 / fps), "frozen subtree changed at +{k} frames");
    }
}

#[test]
fn rewind_composes_and_keeps_frames_before_it() {
    use agentfx_script::remap::TimeMap;
    use agentfx_script::values::Ease;
    let ir = build("crates/script/tests/fixtures/time-remap.reel");
    // frames before the rewind are untouched (same values as without the remap)
    let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("crates/script/tests/fixtures/time-remap.reel")).unwrap();
    let plain = src.replace("  @3.0s rewind #film speed=4 dur=0.5s then=hold\n", "");
    assert_ne!(plain, src);
    let (doc, _) = parse::parse(&plain);
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, _) = timing::solve(&doc, &tts, None);
    let ir0 = lower::compile(&doc, &tl, None, Path::new(env!("AGENTFX_ROOT")).join("crates/script/tests/fixtures").as_path()).ir;
    let by = |ir: &agentfx_script::ir::Ir| -> HashMap<(String, String), agentfx_script::ir::Track> { ir.tracks.iter().map(|t| ((t.layer.clone(), t.ch.name()), t.clone())).collect() };
    let (a, b) = (by(&ir), by(&ir0));
    for f in 0..=90 {
        let t = f as f64 / 30.0;
        for (k, ta) in &a {
            if let Some(tb) = b.get(k) {
                let (x, y) = (ta.eval(t).n(), tb.eval(t).n());
                assert!((x - y).abs() < 0.02, "{k:?} at {t:.3}: {x} vs {y}");
            }
        }
    }
    // nested maps compose: an outer rewind replays an inner one backwards
    let mut outer = TimeMap::default();
    outer.push(2.0, vec![(0.0, 0.0, Ease::Linear), (1.0, -2.0, Ease::Linear)], 0.0);
    let mut inner = TimeMap::default();
    inner.push(1.0, vec![(0.0, 0.0, Ease::Linear)], 0.0); // freeze at 1.0
    assert!((inner.eval(outer.eval(2.5)) - 1.0).abs() < 1e-12); // outer at 1.5 -> inner frozen at 1.0
    assert!((inner.eval(outer.eval(3.0)) - 0.0).abs() < 1e-12); // outer at 0.0 -> before the freeze
}

#[test]
fn motion_blur_reaches_reel_ir_and_is_validated() {
    let ir = build("examples/features/motion-blur.reel");
    let s = agentfx_ir(&ir, "mblur");
    let mb: Vec<(String, u32, f64)> = s.nodes.iter().filter_map(|n| n.motion_blur.map(|m| (n.name.clone(), m.samples, m.shutter))).collect();
    assert!(mb.iter().any(|(n, s, sh)| n.contains("a") && *s == 16 && (*sh - 0.5).abs() < 1e-9), "{mb:?}");
    assert!(mb.iter().any(|(_, s, sh)| *s == 24 && (*sh - 1.0).abs() < 1e-9), "anim mblur=24 shutter=360deg: {mb:?}");
    assert_eq!(mb.len(), 2, "only the two blurred layers: {mb:?}");
    // lint: samples 2..64, shutter (0, 4 frames]
    for (src, code) in [("mblur=1", "E_VALUE"), ("mblur=65", "E_VALUE"), ("mblur=8 shutter=0deg", "E_VALUE"), ("mblur=8 shutter=5f", "E_VALUE")] {
        let text = format!("video \"t\" size=1280x720\nscene a dur=1s\n  rect #r w=10 h=10 {src}\n");
        let (doc, _) = parse::parse(&text);
        let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
        let (tl, _) = timing::solve(&doc, &tts, None);
        let c = lower::compile(&doc, &tl, None, Path::new("."));
        assert!(c.diags.iter().any(|d| d.code == code), "{src}: expected {code}, got {:?}", c.diags.iter().map(|d| &d.code).collect::<Vec<_>>());
    }
}
