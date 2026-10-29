// Fluid (X5): `fluid` + pour/stir/ink/reveal lower to reel-ir `fluid` content that validates;
// ink sources and reveal mattes point at the right nodes; a rewind bakes `fluid.time`; a cut-out
// frame range keeps the ink source alive and shifts the simulation clock; lint explains mistakes.
use agentfx_ir::eval::eval_track;
use agentfx_ir::{Content, Effect, FluidEmitter, Sampled, Scene};
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

fn scene_json(ir: &agentfx_script::ir::Ir) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("reel-fluid-{}", std::process::id()));
    let c = agentfx_script::direct::compile(ir, &dir, false).unwrap();
    let errs: Vec<_> = c.diags.iter().filter(|x| x.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let _ = std::fs::remove_dir_all(&dir);
    c.scene
}

fn fluid_time(s: &Scene, node: usize, t: f64) -> Option<f64> {
    s.tracks.iter().filter(|tr| tr.node == node && tr.property == "fluid.time").find_map(|tr| match eval_track(tr, t) {
        Some(Sampled::Number(x)) => Some(x),
        _ => None,
    })
}

#[test]
fn fluid_example_lowers_to_reel_ir() {
    let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("examples/features/fluid.reel")).unwrap();
    let (ir, diags) = compile_src(&src);
    let errs: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let js = scene_json(&ir);
    let s: Scene = serde_json::from_value(js.clone()).unwrap();
    let find = |name: &str| s.nodes.iter().position(|n| n.name == name).unwrap();
    let (ink, word, stain, card) = (find("ink"), find("word"), find("stain"), find("card"));
    let Some(Content::Fluid(fl)) = &s.nodes[ink].content else { panic!("fluid content") };
    assert!((fl.res - 0.25).abs() < 1e-9);
    assert!(fl.emitters.iter().any(|e| matches!(e, FluidEmitter::Pour { .. })));
    assert!(fl.emitters.iter().any(|e| matches!(e, FluidEmitter::Stir { .. })));
    let src_node = fl.emitters.iter().find_map(|e| match e { FluidEmitter::Source { node, .. } => Some(*node), _ => None }).expect("ink source");
    assert_eq!(src_node, word);
    // the rewind (3.8 s, 2x for 1.6 s, hold) bakes the simulation clock: forward before, back after
    assert!((fluid_time(&s, ink, 2.0).unwrap() - 2.0).abs() < 1e-3);
    let back = fluid_time(&s, ink, 5.4).unwrap();
    assert!((back - 0.6).abs() < 0.05, "fluid time at 5.4 s = {back}");
    // reveal: the card is matted by the stain, which draws only its rim
    assert!(s.nodes[card].effects.iter().any(|e| matches!(e, Effect::Matte { source, .. } if *source == stain)));
    let Some(Content::Fluid(st)) = &s.nodes[stain].content else { panic!("stain") };
    assert!(st.edge > 0.0);
    // the fluid of the first scene carries no time track it does not need
    assert!(fluid_time(&s, stain, 7.0).is_none());
    let w = agentfx_ir::validate::validate(&s, Path::new("/"));
    if let Err(e) = &w {
        assert!(e.iter().all(|m| m.contains("font") || m.contains("asset")), "{e:#?}");
    }
    // a frame long after the ink: the word is invisible but stays whole, the fluid's clock shifts
    let fr = agentfx_script::direct::frame_ir(&js, 60);
    let sf: Scene = serde_json::from_value(fr).unwrap();
    assert!(sf.nodes[word].content.is_some(), "ink source pruned");
    let Some(Content::Fluid(f2)) = &sf.nodes[ink].content else { panic!() };
    assert!((f2.time_offset - 2.0).abs() < 1e-9);
}

#[test]
fn fluid_lint_explains_mistakes() {
    let src = "video \"x\" size=640x360 fps=30 captions=off\nscene a dur=2s\n  fluid #ink colr=#f00\n  text #t \"Hi\"\n  @0s pour #inc at=10%,10%\n  @0.5s ink #t into #nope\n  @0.5s stir #ink at=bad\n";
    let (_, diags) = compile_src(src);
    let has = |code: &str, frag: &str| diags.iter().any(|d| d.code == code && (d.message.contains(frag) || d.hint.contains(frag)));
    assert!(has("W_FLUID_PROP", "color"), "{diags:#?}");
    assert!(has("E_FLUID", "#ink"), "{diags:#?}");
    assert!(has("E_FLUID", "nope"), "{diags:#?}");
    assert!(has("E_FLUID", "at="), "{diags:#?}");
}
