// Generators: `gen` lowers to reel-ir `gen` content that validates; parameter channels become
// `gen.<param>` tracks at full precision; named and linear palettes resolve; lint explains mistakes
// (E_GEN, W_GEN_PROP, W_GEN_ZOOM).
use agentfx_ir::eval::eval_track;
use agentfx_ir::{Content, Sampled, Scene};
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::{lint, lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

fn compile_src(src: &str) -> (agentfx_script::ir::Ir, Vec<Diag>) {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, mut diags) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, None, Path::new(env!("AGENTFX_ROOT")));
    diags.extend(c.diags);
    diags.extend(lint::static_rules(&doc, &tl, &c.ir));
    (c.ir, diags)
}

fn scene(ir: &agentfx_script::ir::Ir) -> Scene {
    let dir = std::env::temp_dir().join(format!("reel-gen-{}", std::process::id()));
    let c = agentfx_script::direct::compile(ir, &dir, false).unwrap();
    let errs: Vec<_> = c.diags.iter().filter(|x| x.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let _ = std::fs::remove_dir_all(&dir);
    serde_json::from_value(c.scene).unwrap()
}

fn codes(d: &[Diag]) -> Vec<&str> {
    d.iter().map(|x| x.code.as_str()).collect()
}

#[test]
fn examples_lower_to_valid_reel_ir() {
    for (file, kind) in [("fractal-julia.reel", "julia"), ("fractal-mandelbrot-zoom.reel", "mandelbrot")] {
        let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("examples/features").join(file)).unwrap();
        let (ir, diags) = compile_src(&src);
        assert!(diags.iter().all(|d| d.severity != Severity::Error && !d.code.starts_with("W_GEN")), "{file}: {diags:#?}");
        let s = scene(&ir);
        let n = s.nodes.iter().position(|n| matches!(&n.content, Some(Content::Gen(g)) if g.kind == kind)).expect(file);
        let Some(Content::Gen(g)) = &s.nodes[n].content else { unreachable!() };
        assert_eq!(g.rect, [0.0, 0.0, 1280.0, 720.0]);
        assert_eq!(g.palette.len(), agentfx_ir::generator::palette(if kind == "julia" { "vapor" } else { "neon" }).unwrap().len());
        assert!(s.tracks.iter().any(|t| t.node == n && t.property.starts_with("gen.")), "{file}: no gen tracks");
        if let Err(e) = agentfx_ir::validate::validate(&s, Path::new("/")) {
            assert!(e.iter().all(|m| m.contains("font") || m.contains("asset")), "{file}: {e:#?}");
        }
    }
}

#[test]
fn params_animate_at_full_precision() {
    let src = "video \"g\" size=320x180 fps=30\nscene a dur=4s\n  gen #m kind=mandelbrot re=-0.743643887037 im=0.131825904205 palette=linear(#000000, #ff00aa, #00e5ff)\n  anim #m re 0s:-0.743643887037 4s:-0.743643887012 ease=linear\n  anim #m zoom 0s:1 4s:100 ease=in-expo\n";
    let (ir, diags) = compile_src(src);
    assert!(diags.iter().all(|d| d.severity != Severity::Error), "{diags:#?}");
    let s = scene(&ir);
    let tr = s.tracks.iter().find(|t| t.property == "gen.re").expect("gen.re track");
    let Some(Sampled::Number(v)) = eval_track(tr, 4.0) else { panic!() };
    assert!((v + 0.743643887012).abs() < 1e-15, "{v}");
    let z = s.tracks.iter().find(|t| t.property == "gen.zoom").expect("gen.zoom track");
    let Some(Sampled::Number(v)) = eval_track(z, 2.0) else { panic!() };
    assert!(v > 1.0 && v < 50.0, "in-expo zoom at half time {v}");
    let Some(Content::Gen(g)) = &s.nodes[tr.node].content else { panic!() };
    assert_eq!(g.palette.len(), 3);
    assert_eq!(g.palette[1], [1.0, 0.0, 2.0 / 3.0, 1.0]);
}

#[test]
fn lint_explains_mistakes() {
    let (_, d) = compile_src("video \"g\" size=320x180 fps=30\nscene a dur=2s\n  gen #g kind=julai\n");
    assert!(codes(&d).contains(&"E_GEN"), "{d:#?}");
    let (_, d) = compile_src("video \"g\" size=320x180 fps=30\nscene a dur=2s\n  gen #g kind=julia iter=0\n");
    assert!(codes(&d).contains(&"E_GEN"), "{d:#?}");
    let (_, d) = compile_src("video \"g\" size=320x180 fps=30\nscene a dur=2s\n  gen #g kind=mandelbrot cx=0.3 zom=2\n");
    assert_eq!(codes(&d).iter().filter(|c| **c == "W_GEN_PROP").count(), 2, "{d:#?}");
    let (_, d) = compile_src("video \"g\" size=320x180 fps=30\nscene a dur=2s\n  gen #g kind=julia palette=neonn\n");
    assert!(codes(&d).contains(&"E_GEN"), "{d:#?}");
    let (_, d) = compile_src("video \"g\" size=320x180 fps=30\nscene a dur=2s\n  rect #r w=10 h=10\n  anim #r zoom 0s:1 1s:2\n");
    assert!(codes(&d).contains(&"E_GEN"), "{d:#?}");
    let (_, d) = compile_src("video \"g\" size=320x180 fps=30\nscene a dur=2s\n  gen #g kind=mandelbrot\n  anim #g depth 0s:0 2s:20 ease=linear\n");
    assert!(codes(&d).contains(&"W_GEN_ZOOM"), "{d:#?}");
    let (_, d) = compile_src("video \"g\" size=320x180 fps=30\nscene a dur=2s\n  gen #g kind=mandelbrot\n  anim #g depth 0s:0 2s:10 ease=linear\n");
    assert!(!codes(&d).contains(&"W_GEN_ZOOM"), "{d:#?}");
}

#[test]
fn named_attractor_systems_and_reaction_presets_lower_to_numbers() {
    let src = "video \"g\" size=320x180 fps=30\nscene a dur=2s\n  gen #a kind=attractor system=lorenz c=2\n  anim #a turn 0s:0 2s:90\n  gen #r kind=reaction preset=maze kill=0.058 seed=7\n  anim #r feed 0s:0.029 2s:0.03\n";
    let (ir, diags) = compile_src(src);
    assert!(diags.iter().all(|d| d.severity != Severity::Error), "{diags:#?}");
    let s = scene(&ir);
    let gen = |kind: &str| s.nodes.iter().find_map(|n| match &n.content {
        Some(Content::Gen(g)) if g.kind == kind => Some(g),
        _ => None,
    }).expect(kind);
    let a = gen("attractor");
    assert_eq!(a.params.get("system"), Some(&2.0));
    assert_eq!(a.params.get("a"), Some(&10.0), "the system's own coefficients");
    assert_eq!(a.params.get("b"), Some(&28.0));
    assert_eq!(a.params.get("c"), Some(&2.0), "an explicit coefficient wins");
    let r = gen("reaction");
    assert_eq!(r.params.get("preset"), None, "preset is compile-time only");
    assert_eq!(r.params.get("feed"), Some(&0.029));
    assert_eq!(r.params.get("kill"), Some(&0.058), "an explicit value wins");
    assert!(s.tracks.iter().any(|t| t.property == "gen.turn"));
    assert!(s.tracks.iter().any(|t| t.property == "gen.feed"));
    for bad in ["kind=attractor system=lorenzz", "kind=reaction preset=corall"] {
        let (_, d) = compile_src(&format!("video \"g\" size=320x180 fps=30\nscene a dur=2s\n  gen #g {bad}\n"));
        assert!(codes(&d).contains(&"E_GEN"), "{bad}: {d:#?}");
    }
}
