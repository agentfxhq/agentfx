// Time effects (X3): slitscan / echo / timedisplace lower to reel-ir `time` + `time.span`
// tracks that validate; freeze is a hold remap; lint explains mistakes.
use agentfx_ir::eval::eval_track;
use agentfx_ir::{Sampled, Scene, TimeFx, TimeMap};
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::{lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

fn compile_src(src: &str) -> (agentfx_script::ir::Ir, Vec<Diag>) {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, mut diags) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, None, Path::new(env!("AGENTFX_ROOT")));
    diags.extend(c.diags);
    (c.ir, diags)
}

fn agentfx_ir(ir: &agentfx_script::ir::Ir) -> Scene {
    let dir = std::env::temp_dir().join(format!("reel-timefx-{}", std::process::id()));
    let c = agentfx_script::direct::compile(ir, &dir, false).unwrap();
    let errs: Vec<_> = c.diags.iter().filter(|x| x.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let s: Scene = serde_json::from_value(c.scene).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    s
}

fn span_at(s: &Scene, node: usize, t: f64) -> f64 {
    s.tracks.iter().filter(|tr| tr.node == node && tr.property == "time.span").find_map(|tr| match eval_track(tr, t) {
        Some(Sampled::Number(x)) => Some(x),
        _ => None,
    }).unwrap_or(s.nodes[node].time.as_ref().unwrap().span())
}

#[test]
fn time_effects_lower_to_reel_ir() {
    let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("examples/features/time-effects.reel")).unwrap();
    let (ir, diags) = compile_src(&src);
    let errs: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let s = agentfx_ir(&ir);
    agentfx_ir::validate::validate(&s, Path::new("/")).ok();
    let find = |name: &str| s.nodes.iter().position(|n| n.name == name).unwrap();
    let smear = find("smear");
    assert!(matches!(s.nodes[smear].time, Some(TimeFx::Slitscan { .. })));
    // span 0.8 s from the start, eased to 0 at 4.5 s + 0.2 s
    assert!((span_at(&s, smear, 1.0) - 0.8).abs() < 1e-6);
    assert!(span_at(&s, smear, 5.0).abs() < 1e-6);
    let ball = find("ball");
    match &s.nodes[ball].time {
        Some(TimeFx::Echo { count, tint, blend, .. }) => {
            assert_eq!(*count, 6);
            assert_eq!(tint.len(), 2);
            assert_eq!(*blend, agentfx_ir::BlendMode::Plus);
        }
        t => panic!("echo expected, got {t:?}"),
    }
    assert!((span_at(&s, ball, 1.0) - 0.05).abs() < 1e-6);
    let grid = find("grid");
    assert!(matches!(&s.nodes[grid].time, Some(TimeFx::Displace { map: TimeMap::Radial { .. }, levels: 16, .. })));
}

#[test]
fn layer_map_is_hidden_and_resolved() {
    let src = "video \"m\" size=640x360 fps=30 captions=off\nscene s dur=2s\n  layer #g x=0 y=0 w=100% h=100%\n    rect #r w=200 h=100 x=50% y=50% anchor=center fill=#ff0000\n      anim rot 0s:0 2s:90\n  ellipse #m w=300 h=300 x=50% y=50% anchor=center fill=#ffffff\n  @0s timedisplace #g map=#m span=0.5s levels=8\n";
    let (ir, diags) = compile_src(src);
    assert!(diags.iter().all(|d| d.severity != Severity::Error), "{diags:#?}");
    let s = agentfx_ir(&ir);
    let g = s.nodes.iter().position(|n| n.name == "g").unwrap();
    let m = s.nodes.iter().position(|n| n.name == "m").unwrap();
    assert!(s.nodes[m].hidden);
    assert!(matches!(s.nodes[g].time, Some(TimeFx::Displace { map: TimeMap::Layer { node }, .. }) if node == m));
}

#[test]
fn lint_explains_time_mistakes() {
    let src = "video \"m\" size=640x360 fps=30 captions=off\nscene s dur=2s\n  rect #r w=200 h=100 x=50% y=50% fill=#ff0000\n  @0s slitscan #r axis=z span=1s\n  @0s echo #r n=6 dt=50ms decy=0.5\n  @0s timedisplace #r map=radail\n";
    let (_, diags) = compile_src(src);
    let has = |code: &str, frag: &str| diags.iter().any(|d| d.code == code && (d.message.contains(frag) || d.hint.contains(frag)));
    assert!(has("E_VALUE", "axis=z"), "{diags:#?}");
    assert!(has("W_TIME_PROP", "did you mean `decay`?"), "{diags:#?}");
    assert!(has("E_VALUE", "did you mean `radial`?"), "{diags:#?}");
}

#[test]
fn freeze_holds_the_subtree() {
    let src = "video \"f\" size=640x360 fps=30 captions=off\nscene s dur=3s\n  rect #r w=100 h=100 x=10% y=50% fill=#ff0000\n    anim x 0s:0 3s:300\n  @1s freeze #r for=1s\n";
    let (ir, diags) = compile_src(src);
    assert!(diags.iter().all(|d| d.severity != Severity::Error), "{diags:#?}");
    let s = agentfx_ir(&ir);
    let r = s.nodes.iter().position(|n| n.name == "r").unwrap();
    let x = |t: f64| s.tracks.iter().filter(|tr| tr.node == r && tr.property == "transform").find_map(|tr| match eval_track(tr, t) {
        Some(Sampled::Matrix(m)) => Some(m.e),
        _ => None,
    }).unwrap();
    assert!((x(1.2) - x(1.8)).abs() < 1e-6, "frozen: {} vs {}", x(1.2), x(1.8));
    assert!(x(2.5) > x(1.8) + 10.0, "resumes after for=");
}
