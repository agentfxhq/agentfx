use agentfx_ir::{Easing, Scene, timing::ease, validate::validate};
use std::path::Path;

#[test]
fn direct_example_validates_and_roundtrips() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ir/direct/lower-third.ir.json");
    let scene = Scene::load(&p).unwrap();
    validate(&scene, p.parent().unwrap()).expect("example must validate");
    let s = serde_json::to_string(&scene).unwrap();
    let back: Scene = serde_json::from_str(&s).unwrap();
    assert_eq!(scene, back);
}

#[test]
fn spring_endpoints_and_overshoot() {
    let under = Easing::Spring { mass: 1.0, stiffness: 120.0, damping: 6.0, velocity: 0.0 };
    let crit = Easing::Spring { mass: 1.0, stiffness: 100.0, damping: 20.0, velocity: 0.0 };
    for e in [under, crit] {
        assert_eq!(ease(&e, 0.0, false), 0.0);
        assert_eq!(ease(&e, 1.0, false), 1.0);
    }
    let max = (0..=100).map(|i| ease(&under, i as f64 / 100.0, false)).fold(0.0, f64::max);
    assert!(max > 1.01, "underdamped spring should overshoot, max {max}");
    let mut prev = 0.0;
    for i in 0..=100 {
        let v = ease(&crit, i as f64 / 100.0, false);
        assert!(v >= prev - 1e-12, "critically damped must be monotonic");
        prev = v;
    }
}

#[test]
fn validation_lists_unsupported_features() {
    let mut scene: Scene = serde_json::from_str(r#"{"version":0,"width":64,"height":64,"fps":30,"frames":10,
        "nodes":[{"name":"root","children":[1]},{"name":"t","content":{"type":"text","text":"x","font":3,"size":10,"pos":[0,0],"brush":{"solid":[1,1,1,1]}},
        "effects":[{"type":"matte","source":9}]}],
        "tracks":[{"node":1,"property":"color","delay":0,"duration":1,"fill":"both","easing":"linear",
        "keyframes":[{"offset":0,"value":{"number":0}},{"offset":1,"value":{"number":1}}]}]}"#).unwrap();
    scene.camera = Some(agentfx_ir::Camera { perspective: 800.0 });
    let errs = validate(&scene, Path::new(".")).unwrap_err();
    for needle in ["camera", "color", "matte source", "font 3"] {
        assert!(errs.iter().any(|e| e.contains(needle)) , "missing error about {needle}: {errs:?}");
    }
    assert!(errs.iter().any(|e| e.contains("out of range")), "{errs:?}");
}

#[test]
fn color_tracks_effect_and_blends_validate() {
    let ok: Scene = serde_json::from_str(r##"{"version":1,"width":64,"height":64,"fps":30,"frames":10,
        "nodes":[{"name":"root","children":[1],"effects":[{"type":"color","saturate":0,"hue":90,"tint":[1,0.8,0.2,1],"levels":[0.1,0.9,1.2,0,1]}]},
        {"name":"p","blend":"soft-light","content":{"type":"path","d":"M0 0 L10 10","fill":{"solid":[1,0,0,1]},"stroke":{"width":2,"brush":{"solid":[0,0,1,1]}}}}],
        "tracks":[{"node":1,"property":"stroke.color","delay":0,"duration":1,"fill":"both","easing":"linear",
        "keyframes":[{"offset":0,"value":{"color":[0,0,1,1]}},{"offset":1,"value":{"color":[1,1,0,1]}}]},
        {"node":0,"property":"effects.0.hue","delay":0,"duration":1,"fill":"both","easing":"linear",
        "keyframes":[{"offset":0,"value":{"number":0}},{"offset":1,"value":{"number":180}}]}]}"##).unwrap();
    validate(&ok, Path::new(".")).expect("valid");
    let back: Scene = serde_json::from_str(&serde_json::to_string(&ok).unwrap()).unwrap();
    assert_eq!(ok, back);
    // a number on a color property, a color track on a node without that brush, a bad LUT
    let mut bad = ok.clone();
    bad.tracks[0].keyframes[0].value = agentfx_ir::Value::Number(1.0);
    bad.tracks.push(agentfx_ir::Track { property: "fill.color".into(), node: 0, ..ok.tracks[0].clone() });
    if let agentfx_ir::Effect::Color { lut, .. } = &mut bad.nodes[0].effects[0] { *lut = Some("missing.cube".into()); }
    let errs = validate(&bad, Path::new(".")).unwrap_err().join("\n");
    assert!(errs.contains("value type does not match"), "{errs}");
    assert!(errs.contains("does not apply"), "{errs}");
    assert!(errs.contains("lut"), "{errs}");
}
