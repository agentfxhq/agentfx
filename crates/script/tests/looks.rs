// Print looks and light (src/look.rs): emitted reel-ir effects, baked light tracks,
// knockout mattes, validation by reel-ir, and lint diagnostics.
use agentfx_ir::{Effect, Scene};
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::{lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

fn compile_src(src: &str, base: &Path) -> (Option<agentfx_script::ir::Ir>, Vec<Diag>) {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, mut diags) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, None, base);
    diags.extend(c.diags);
    (Some(c.ir), diags)
}

fn scene_of(file: &str) -> (Scene, std::path::PathBuf) {
    let p = Path::new(env!("AGENTFX_ROOT")).join(file);
    let src = std::fs::read_to_string(&p).unwrap();
    let (ir, diags) = compile_src(&src, p.parent().unwrap());
    let errs: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{file}: {errs:#?}");
    let dir = std::env::temp_dir().join(format!("reel-looks-{}-{}", file.replace('/', "_"), std::process::id()));
    let c = agentfx_script::direct::compile(&ir.unwrap(), &dir, false).unwrap();
    let errs: Vec<_> = c.diags.iter().filter(|x| x.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let s: Scene = serde_json::from_value(c.scene).unwrap();
    (s, dir)
}

fn errors(src: &str) -> Vec<Diag> {
    let base = Path::new(env!("AGENTFX_ROOT")).join("examples/features");
    compile_src(src, &base).1.into_iter().filter(|d| d.severity == Severity::Error).collect()
}

#[test]
fn riso_scene_and_halftone_emit_effects() {
    let (s, dir) = scene_of("examples/features/riso.reel");
    let riso: Vec<&Effect> = s.nodes.iter().flat_map(|n| &n.effects).filter(|e| matches!(e, Effect::Riso { .. })).collect();
    assert_eq!(riso.len(), 1, "one riso on the scene root");
    let Effect::Riso { inks, misreg, drift, paper, .. } = riso[0] else { unreachable!() };
    assert_eq!(inks.len(), 3);
    assert!(*misreg > 0.0 && *drift > 0.0, "misregistration that drifts");
    assert!(paper.is_some(), "scene bg is the riso paper");
    assert!(agentfx_ir::validate::validate(&s, &dir).is_ok());
    let (h, dir) = scene_of("examples/features/halftone.reel");
    let (k, node) = h.nodes.iter().enumerate().find_map(|(i, n)| n.effects.iter().position(|e| matches!(e, Effect::Halftone { .. })).map(|k| (k, i))).expect("halftone effect");
    assert!(h.tracks.iter().any(|t| t.node == node && t.property == format!("effects.{k}.cell")), "animated cell");
    assert!(agentfx_ir::validate::validate(&h, &dir).is_ok());
}

#[test]
fn light_is_baked_into_shadow_tracks_and_knockout_mattes() {
    let (s, dir) = scene_of("examples/features/light.reel");
    let shadows: Vec<(usize, usize)> = s.nodes.iter().enumerate().flat_map(|(i, n)| n.effects.iter().enumerate().filter(|(_, e)| matches!(e, Effect::Shadow { .. })).map(move |(k, _)| (i, k))).collect();
    assert!(shadows.len() >= 5, "{shadows:?}");
    for (i, k) in &shadows {
        // the moving light: every shadow gets lx/ly tracks
        let lx = s.tracks.iter().find(|t| t.node == *i && t.property == format!("effects.{k}.lx")).expect("lx track");
        let a = agentfx_ir::eval::eval_track(lx, 0.5);
        let b = agentfx_ir::eval::eval_track(lx, 4.0);
        assert_ne!(format!("{a:?}"), format!("{b:?}"), "light moves");
    }
    // the sweep verb animates `pos`
    assert!(s.nodes.iter().enumerate().any(|(i, n)| n.effects.iter().enumerate().any(|(k, e)| matches!(e, Effect::Sweep { .. }) && s.tracks.iter().any(|t| t.node == i && t.property == format!("effects.{k}.pos")))));
    // knockout: hidden child + alpha-inverted matte on the plate, before its rays
    let (pi, plate) = s.nodes.iter().enumerate().find(|(_, n)| n.effects.iter().any(|e| matches!(e, Effect::Rays { .. }))).expect("rays plate");
    let Effect::Matte { source, mode } = &plate.effects[0] else { panic!("matte first: {:?}", plate.effects) };
    assert_eq!(mode, "alpha-inverted");
    assert!(s.nodes[*source].hidden);
    assert!(s.parents()[*source] == Some(pi) || plate.children.contains(source));
    assert!(agentfx_ir::validate::validate(&s, &dir).is_ok(), "{:?}", agentfx_ir::validate::validate(&s, &dir));
}

#[test]
fn deterministic_emit() {
    let (a, _) = scene_of("examples/features/light.reel");
    let (b, _) = scene_of("examples/features/light.reel");
    assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
}

#[test]
fn lint_looks() {
    let head = "video \"x\" size=640x360 fps=30 captions=off\nscene a dur=1s\n";
    let has = |src: &str, code: &str, hint: &str| {
        let e = errors(&format!("{head}{src}"));
        assert!(e.iter().any(|d| d.code == code && d.hint.contains(hint)), "{src}: want {code} with `{hint}`, got {e:#?}");
    };
    has("  rect #r w=10 h=10 riso=[#f00, #00f] riso-drfit=2\n", "E_LOOK", "riso-drift");
    has("  rect #r w=10 h=10 grain=0.1 grain-mode=lve\n", "E_VALUE", "live");
    has("  rect #r w=10 h=10 halftone=8 halftone-mode=cymk\n", "E_VALUE", "cmyk");
    has("  light #sun x=10% y=10%\n  rect #r w=10 h=10 shadow=soft(#snu)\n", "E_LIGHT", "#sun");
    has("  light #sun\n  rect #r w=10 h=10 shadow=soft(#sun, hieght=4)\n", "E_LOOK", "height");
    has("  rect #r w=10 h=10 riso=[#f00, #0f0, #00f, #ff0, #0ff]\n", "E_LOOK", "riso=");
    assert!(errors(&format!("{head}  light #sun x=10% y=10%\n  rect #r w=10 h=10 shadow=soft(#sun) rays=#sun\n")).is_empty());
}

#[test]
fn digital_looks_and_all_numeric_tracks_validate() {
    let (s, dir) = scene_of("examples/features/looks.reel");
    let v = serde_json::to_value(&s).unwrap();
    for (kind, fields) in [("rgbsplit", &["amount", "dx", "dy"][..]), ("glitch", &["amount", "rate", "block"][..]), ("vhs", &["amount"][..]), ("dither", &["levels"][..]), ("ascii", &["cell"][..])] {
        let (node, effect) = v["nodes"].as_array().unwrap().iter().enumerate().find_map(|(n, x)| x["effects"].as_array()?.iter().position(|e| e["type"] == kind).map(|e| (n, e))).unwrap();
        for field in fields {
            assert!(s.tracks.iter().any(|t| t.node == node && t.property == format!("effects.{effect}.{field}") && agentfx_ir::eval::eval_track(t, 0.0) != agentfx_ir::eval::eval_track(t, 1.9)), "{kind}.{field}");
        }
    }
    assert!(agentfx_ir::validate::validate(&s, &dir).is_ok());
}

#[test]
fn digital_look_parameters_are_checked() {
    let head = "video \"x\" size=640x360 fps=30 captions=off\nscene a dur=1s\n";
    for props in ["rgbsplit=2", "rgbsplit=1 rgbsplit-mdoe=radial", "glitch=1 glitch-rate=-1", "vhs=1 vhs-seed=3", "dither=3 dither-size=3", "dither=3 dither-palette=[#000]", "ascii=5", "ascii=8 ascii-charset=xyz", "ascii=8 ascii-color=rainbow"] {
        assert!(!errors(&format!("{head}  rect w=10 h=10 {props}\n")).is_empty(), "{props}");
    }
}
