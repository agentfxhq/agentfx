// Motion paths (`move #x along=`) and path morph (`morph #p to=`, `morph=[...]`): compile,
// emit to reel-ir, validate.
use agentfx_ir::{Content, Scene};
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::ir::Ch;
use agentfx_script::{lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

fn compile(src: &str) -> lower::Compiled {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors {errs:?}");
    let (tl, errs) = timing::solve(&doc, &HashMap::new(), None);
    assert!(errs.is_empty(), "{errs:?}");
    lower::compile(&doc, &tl, None, &Path::new(env!("AGENTFX_ROOT")).join("examples/features"))
}

fn errors(d: &[Diag]) -> Vec<&Diag> {
    d.iter().filter(|x| x.severity == Severity::Error).collect()
}

fn example(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("examples/features").join(name)).unwrap()
}

fn emit(c: &lower::Compiled, tag: &str) -> Scene {
    let dir = std::env::temp_dir().join(format!("agentfx-motionpath-{tag}-{}", std::process::id()));
    let out = agentfx_script::direct::compile(&c.ir, &dir, false).unwrap();
    assert!(errors(&out.diags).is_empty(), "{:#?}", out.diags);
    assert!(!out.diags.iter().any(|d| d.code == "W_IR_UNSUPPORTED"), "{:#?}", out.diags);
    let s: Scene = serde_json::from_value(out.scene).unwrap();
    agentfx_ir::validate::validate(&s, &dir).unwrap();
    s
}

fn track<'a>(c: &'a lower::Compiled, layer: &str, ch: Ch) -> &'a agentfx_script::ir::Track {
    c.ir.tracks.iter().find(|t| t.layer.contains(layer) && t.ch == ch).unwrap_or_else(|| panic!("{layer} {ch:?}"))
}

#[test]
fn motion_path_example_follows_the_path() {
    let c = compile(&example("motion-path.reel"));
    assert!(errors(&c.diags).is_empty(), "{:#?}", c.diags);
    // relative path data: the plane ends 1500 px right of where it started, level again
    let (x, y, r) = (track(&c, "plane", Ch::X), track(&c, "plane", Ch::Y), track(&c, "plane", Ch::Rot));
    assert!((x.eval(5.5).n() - 1500.0).abs() < 0.05);
    assert!(y.eval(5.5).n().abs() < 0.05);
    // nothing moves or turns before the move starts at 0.4 s
    assert_eq!(x.eval(0.3).n(), 0.0);
    assert_eq!(r.eval(0.3).n(), 0.0);
    // orient=90deg on a path that starts and ends heading +x: 90 at the start, 90 - 360 after
    // the counter-clockwise loop (unwrapped, no snap back)
    assert!((r.eval(0.41).n() - 90.0).abs() < 1.0, "{}", r.eval(0.41).n());
    assert!((r.eval(5.5).n() - (-270.0)).abs() < 1.0, "{}", r.eval(5.5).n());
    // (the turn to the tangent at the start of the move is a deliberate snap)
    for w in r.keys.windows(2).filter(|w| w[0].t >= 0.4 - 1e-9) {
        assert!((w[1].v.n() - w[0].v.n()).abs() < 45.0, "rot jumps at {}", w[0].t);
    }
    // along=#road: the car's center ends on the road's end point (road box 1200x600 centered at
    // 960,648; car box 90x60 centered at x=18.75% = 360, y=648): end (1560, 648) -> x = +1200
    let (x, y) = (track(&c, "car", Ch::X), track(&c, "car", Ch::Y));
    assert!((x.eval(5.5).n() - 1200.0).abs() < 0.1, "{}", x.eval(5.5).n());
    assert!(y.eval(5.5).n().abs() < 0.1, "{}", y.eval(5.5).n());
    // the road's end tangent is (200, 300): about 56.3 degrees
    assert!((track(&c, "car", Ch::Rot).eval(5.5).n() - 56.31).abs() < 0.1);
    // linear ease on the wave: equal time, equal distance (constant speed)
    let (dx, dy) = (track(&c, "dot", Ch::X), track(&c, "dot", Ch::Y));
    let pos = |t: f64| (dx.eval(t).n(), dy.eval(t).n());
    let step = |a: f64, b: f64| {
        let (p, q) = (pos(a), pos(b));
        (q.0 - p.0).hypot(q.1 - p.1)
    };
    let (s1, s2) = (step(1.0, 1.1), step(3.0, 3.1));
    assert!((s1 - s2).abs() / s1 < 0.05, "{s1} {s2}");
    // deterministic
    let again = compile(&example("motion-path.reel"));
    assert_eq!(serde_json::to_string(&c.ir.tracks).unwrap(), serde_json::to_string(&again.ir.tracks).unwrap());
    emit(&c, "motion");
}

#[test]
fn path_morph_example_emits_targets_and_a_track() {
    let c = compile(&example("path-morph.reel"));
    assert!(errors(&c.diags).is_empty(), "{:#?}", c.diags);
    let s = emit(&c, "morph");
    let node = |name: &str| s.nodes.iter().enumerate().find(|(_, n)| n.name.contains(name)).unwrap_or_else(|| panic!("{name}"));
    let (bi, blob) = node("blob");
    let Some(Content::Path(p)) = &blob.content else { panic!("blob is not a path") };
    // star (another element, scene space), heart and ring (data), back to its own d
    assert_eq!(p.morph.len(), 4);
    assert_eq!(p.morph[3], p.d);
    let t = s.tracks.iter().find(|t| t.node == bi && t.property == "morph").expect("morph track");
    let last = t.keyframes.last().unwrap();
    assert_eq!(last.value, agentfx_ir::Value::Number(4.0));
    // manual targets on the element + the channel
    let (wi, wave) = node("wave");
    let Some(Content::Path(p)) = &wave.content else { panic!() };
    assert_eq!(p.morph.len(), 2);
    assert!(s.tracks.iter().any(|t| t.node == wi && t.property == "morph"));
    assert!(s.tracks.iter().any(|t| t.node == wi && t.property == "trim-end"));
}

#[test]
fn morph_errors_have_hints() {
    let base = "video \"m\" size=640x360 captions=off\n\nscene a dur=2s\n  path #p d=\"M0 0 L100 0 L50 80 Z\" w=100 h=100 stroke=#fff\n  rect #r w=100 h=100\n";
    for (line, code) in [
        ("  @0.5s morph #r to=\"M0 0 L10 0\"", "E_MORPH"),
        ("  @0.5s morph #p", "E_MORPH"),
        ("  @0.5s morph #p to=#nope", "E_ID"),
        ("  @0.5s morph #p to=#r", "E_MORPH"),
        ("  @0.5s morph #p to=\"M0 0\"", "E_MORPH"),
        ("  @0.5s move #r along=\"Mxx\"", "E_PATH"),
        ("  @0.5s move #r along=#nope", "E_ID"),
    ] {
        let c = compile(&format!("{base}{line}\n"));
        let d = c.diags.iter().find(|d| d.code == code).unwrap_or_else(|| panic!("{line}: {:#?}", c.diags));
        assert!(!d.hint.is_empty(), "{line}");
    }
    // a good one is clean
    let c = compile(&format!("{base}  @0.5s morph #p to=\"M0 0 L100 0 L100 100 L0 100 Z\" dur=1s\n"));
    assert!(errors(&c.diags).is_empty(), "{:#?}", c.diags);
}
