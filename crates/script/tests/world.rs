// World canvas + camera director (X9): worlds lower to reel-ir `world` nodes, director verbs bake
// full-precision per-frame camera keys (zoom ratios of 1e4+), lint explains framing mistakes.
use agentfx_ir::Scene;
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

fn agentfx_ir(ir: &agentfx_script::ir::Ir) -> (Scene, serde_json::Value) {
    let dir = std::env::temp_dir().join(format!("reel-world-{}", std::process::id()));
    let c = agentfx_script::direct::compile(ir, &dir, false).unwrap();
    let errs: Vec<_> = c.diags.iter().filter(|x| x.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let s: Scene = serde_json::from_value(c.scene.clone()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (s, c.scene)
}

fn scales(v: &serde_json::Value, out: &mut Vec<f64>) {
    match v {
        serde_json::Value::Object(m) => {
            if let Some(s) = m.get("scale").and_then(|s| s.as_array()) {
                out.extend(s.iter().filter_map(|x| x.as_f64()).take(1));
            }
            m.values().for_each(|x| scales(x, out));
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| scales(x, out)),
        _ => {}
    }
}

#[test]
fn world_zoom_lowers_with_a_deep_camera_path() {
    let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("examples/features/world-zoom.reel")).unwrap();
    let (ir, diags) = compile_src(&src);
    let errs: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    assert!(!diags.iter().any(|d| d.code.starts_with("W_CAMERA")), "{diags:#?}");
    let (s, raw) = agentfx_ir(&ir);
    let worlds: Vec<usize> = (0..s.nodes.len()).filter(|&i| s.nodes[i].world.is_some()).collect();
    assert_eq!(worlds.len(), 1);
    // the camera (the world's parent) zooms over more than four orders of magnitude
    let cam = s.nodes.iter().position(|n| n.children.contains(&worlds[0])).unwrap();
    let tr = raw["tracks"].as_array().unwrap().iter().find(|t| t["node"].as_u64() == Some(cam as u64) && t["property"] == "transform").expect("camera track");
    let mut sc = vec![];
    scales(&tr["keyframes"], &mut sc);
    let (lo, hi) = (sc.iter().cloned().fold(f64::MAX, f64::min), sc.iter().cloned().fold(0.0, f64::max));
    assert!(hi / lo > 1e4, "zoom ratio {}", hi / lo);
    // per-frame keys at full precision (not the 1/4096 scale quantum)
    assert!(sc.iter().any(|v| (v * 4096.0 - (v * 4096.0).round()).abs() > 1e-3));
    // deterministic
    let (ir2, _) = compile_src(&src);
    assert_eq!(serde_json::to_string(&agentfx_ir(&ir2).1).unwrap(), serde_json::to_string(&raw).unwrap());
}

const BASE: &str = "video \"t\" size=1920x1080 fps=30 captions=off\nscene s dur=12s\n  world #w 16000x9000\n    place #a at 2000,2000 w=1920 h=1080\n      text #ta \"alpha\" size=200\n    place #b at 12000,7000 scale=0.01 w=1920 h=1080\n      text #tb \"beta\" size=200\n";

fn diags_for(extra: &str) -> Vec<Diag> {
    compile_src(&format!("{BASE}{extra}")).1
}

#[test]
fn director_lint_explains_mistakes() {
    let d = diags_for("  @0s camera tour #a -> #bb\n");
    let e = d.iter().find(|x| x.code == "E_CAMERA_TARGET").expect("target error");
    assert!(e.hint.contains("#b"), "{}", e.hint);
    let d = diags_for("  @0s camera tuor #a -> #b\n");
    assert!(d.iter().any(|x| x.code == "E_CAMERA" && x.hint.contains("tour")), "{d:#?}");
    let d = diags_for("  @0s camera tour #a -> #b dwel=1s\n");
    assert!(d.iter().any(|x| x.code == "W_CAMERA_PROP" && x.hint.contains("dwell")), "{d:#?}");
    // 100x zoom plus a long pan in 0.4 s is too fast to read
    let d = diags_for("  @1s camera zoom-through #a -> #b dur=0.4s\n");
    assert!(d.iter().any(|x| x.code == "W_CAMERA_FAST"), "{d:#?}");
    // a calm version is fine
    let d = diags_for("  @0s camera zoom-through #a -> #b dur=9s\n");
    assert!(!d.iter().any(|x| x.code.starts_with("W_CAMERA") || x.code.starts_with("E_CAMERA")), "{d:#?}");
    // below the max zoom's reach
    let d = compile_src(&format!("{}  @0s camera tour #a -> #b\n", BASE.replace("16000x9000", "16000x9000 max-zoom=10").replace("scale=0.01", "scale=0.0001"))).1;
    assert!(d.iter().any(|x| x.code == "E_CAMERA_TINY"), "{d:#?}");
}

#[test]
fn tour_and_focus_move_the_camera_and_dim() {
    let (ir, d) = compile_src(&format!("{BASE}  @0s camera tour #a -> #b dwell=1s\n  @4s camera focus #ta overlay=#000c for=1s\n"));
    assert!(!d.iter().any(|x| x.severity == Severity::Error), "{d:#?}");
    let (s, _) = agentfx_ir(&ir);
    assert!(s.nodes.iter().any(|n| n.name.starts_with("focus-")), "overlay group");
    assert!(s.tracks.iter().any(|t| t.property == "opacity" && s.nodes[t.node].name.starts_with("focus-")));
}

// ------------------------------------------------------------ nest + camera loop (D5)

fn track_at(ir: &agentfx_script::ir::Ir, layer: &str, ch: agentfx_script::ir::Ch, t: f64) -> f64 {
    ir.tracks.iter().find(|x| x.layer == layer && x.ch == ch).map(|x| x.eval(t).n()).unwrap_or(ch.default_value().n())
}

fn loop_src(nest: &str, child: &str, extra: &str, cam: &str) -> String {
    format!("video \"t\" size=1920x1080 fps=30 captions=off\nscene s dur=6s bg=#000\n  {nest}\n    {child}\n{extra}  {cam}\n")
}

const SQ: &str = "rect x=40 y=40 w=1840 h=1840 radius=120 stroke=$cycle sw=10";

#[test]
fn neon_tunnel_loops_seamlessly() {
    use agentfx_script::ir::Ch;
    let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("examples/features/neon-tunnel.reel")).unwrap();
    let (ir, diags) = compile_src(&src);
    let bad: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error || d.code.contains("LOOP") || d.code.starts_with("W_CAMERA") || d.code.starts_with("W_NEST")).collect();
    assert!(bad.is_empty(), "{bad:#?}");
    // 28 levels per nest, scaled 0.72^i and turned 4deg * i, at full precision
    let mut levels = vec![];
    ir.root.walk(&mut |l| if l.name.as_deref().is_some_and(|n| n.starts_with("core-L")) { levels.push(l.id.clone()) });
    assert_eq!(levels.len(), 28);
    assert!((track_at(&ir, "u-core-L27-0", Ch::Scale, 0.0) - 0.72f64.powi(27)).abs() < 1e-15);
    assert!((track_at(&ir, "u-core-L27-0", Ch::Rot, 0.0) - 108.0).abs() < 1e-9);
    // camera: constant log-speed, six levels per 6 s, wraps exactly at the loop
    let z = |t: f64| track_at(&ir, "world-tunnel", Ch::Scale, t);
    assert!((z(0.0) - 1.0).abs() < 1e-12);
    for f in [1.0, 45.0, 90.0, 179.0] {
        let want = (1.0 / 0.72f64).powf(6.0 * f / 180.0);
        assert!((z(f / 30.0) / want - 1.0).abs() < 1e-4, "frame {f}: {} vs {want}", z(f / 30.0));
    }
    let r = |t: f64| track_at(&ir, "u-tunnel-0", Ch::Rot, t);
    assert!((r(179.0 / 30.0) + 24.0 * 179.0 / 180.0).abs() < 1e-3);
    // the halo's blur is counter-scaled (constant on screen)
    let b = |t: f64| track_at(&ir, "u-halo-0", Ch::Blur, t);
    assert!((b(3.0) * z(3.0) - b(0.0)).abs() < 0.1, "{} {}", b(3.0), z(3.0));
    // no lint warnings either (glow cost, frame checks)
    let (doc, _) = parse::parse(&src);
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, _) = timing::solve(&doc, &tts, None);
    let st = agentfx_script::lint::static_rules(&doc, &tl, &ir);
    assert!(!st.iter().any(|d| d.code == "W_GLOW_COST" || d.severity == Severity::Error), "{st:#?}");
}

#[test]
fn camera_loop_dir_out_and_wrap_off() {
    use agentfx_script::ir::Ch;
    let (ir, d) = compile_src(&loop_src("nest #n levels=30 scale=0.75 w=1920 h=1920", "rect x=40 y=40 w=1840 h=1840 stroke=#fff sw=8", "", "camera loop #n dir=out dur=3s"));
    assert!(!d.iter().any(|x| x.severity == Severity::Error || x.code.contains("LOOP")), "{d:#?}");
    let z = |t: f64| track_at(&ir, "world-s", Ch::Scale, t);
    // out: starts one level in, ends at rest, loops twice in the scene
    assert!((z(0.0) - 1.0 / 0.75).abs() < 1e-9 && (z(3.0) - 1.0 / 0.75).abs() < 1e-9);
    assert!((z(89.0 / 30.0) / (1.0 / 0.75f64).powf(1.0 / 90.0) - 1.0).abs() < 1e-4, "{}", z(89.0 / 30.0));
    let (ir, _) = compile_src(&loop_src("nest #n levels=30 scale=0.75 w=1920 h=1920", "rect x=40 y=40 w=1840 h=1840 stroke=#fff sw=8", "", "camera loop #n dur=3s wrap=off"));
    let z = |t: f64| track_at(&ir, "world-s", Ch::Scale, t);
    assert!((z(6.0) - (1.0 / 0.75f64).powi(2)).abs() < 1e-9);
}

#[test]
fn camera_loop_lint_explains_seams() {
    let codes = |src: String| compile_src(&src).1;
    let nest = "nest #n levels=30 scale=0.72 cycle=[#f0f, #0ff] w=1920 h=1920";
    // a clean loop
    let d = codes(loop_src(nest, SQ, "", "camera loop #n"));
    assert!(!d.iter().any(|x| x.code.contains("LOOP") || x.severity == Severity::Error), "{d:#?}");
    // not a nest: needs scale=, then unchecked
    let d = codes(loop_src("layer #n", "rect w=100 h=100 fill=#fff", "", "camera loop #n"));
    assert!(d.iter().any(|x| x.code == "E_CAMERA" && x.hint.contains("nest")), "{d:#?}");
    let d = codes(loop_src("layer #n", "rect w=100 h=100 fill=#fff", "", "camera loop #n scale=0.5"));
    assert!(d.iter().any(|x| x.code == "W_LOOP_SELF_SIMILAR"), "{d:#?}");
    // levels not a multiple of the cycle
    let d = codes(loop_src(nest, SQ, "", "camera loop #n levels=3"));
    assert!(d.iter().any(|x| x.code == "W_LOOP_SELF_SIMILAR" && x.hint.contains("levels=4")), "{d:#?}");
    // other content moves with the camera; hud is fine
    let d = codes(loop_src(nest, SQ, "  text #title \"Hi\" size=80\n", "camera loop #n"));
    assert!(d.iter().any(|x| x.code == "W_LOOP_CONTENT" && x.message.contains("#title")), "{d:#?}");
    let d = codes(loop_src(nest, SQ, "  text #title \"Hi\" size=80 hud\n", "camera loop #n"));
    assert!(!d.iter().any(|x| x.code == "W_LOOP_CONTENT"), "{d:#?}");
    // too few levels: the center pops; suggests a count
    let d = codes(loop_src("nest #n levels=10 scale=0.72 cycle=[#f0f, #0ff] w=1920 h=1920", SQ, "", "camera loop #n"));
    let w = d.iter().find(|x| x.code == "W_LOOP_SEAM" && x.message.contains("innermost")).expect("inner seam");
    assert!(w.hint.contains("levels="), "{}", w.hint);
    // a small box: the edge pops
    let d = codes(loop_src("nest #n levels=30 scale=0.9 w=600 h=600", "rect w=600 h=600 stroke=#fff sw=8", "", "camera loop #n"));
    assert!(d.iter().any(|x| x.code == "W_LOOP_SEAM" && x.message.contains("outer")), "{d:#?}");
    // an animation inside the nest that does not repeat with the loop
    let d = codes(loop_src(nest, "rect #r x=40 y=40 w=1840 h=1840 stroke=$cycle sw=10\n      anim #r opacity 0s:1 6s:0.2", "", "camera loop #n"));
    assert!(d.iter().any(|x| x.code == "W_LOOP_CONTENT" && x.message.contains("repeat")), "{d:#?}");
    // nest props
    let d = codes(loop_src("nest #n levels=0 scale=1", "rect w=10 h=10", "", "camera loop #n"));
    assert_eq!(d.iter().filter(|x| x.code == "E_NEST").count(), 2, "{d:#?}");
    let d = codes(loop_src("nest #n levels=30 scale=0.72 cycle=[#f0f, #0ff] w=1920 h=1920", "rect x=40 y=40 w=1840 h=1840 stroke=#fff sw=10", "", "camera loop #n levels=2"));
    assert!(d.iter().any(|x| x.code == "W_NEST_CYCLE"), "{d:#?}");
}

#[test]
fn glow_on_every_nest_level_warns_about_cost() {
    let src = loop_src("nest #n levels=30 scale=0.72 w=1920 h=1920", "rect x=40 y=40 w=1840 h=1840 stroke=#f0f sw=10 glow=12", "", "camera loop #n");
    let (ir, _) = compile_src(&src);
    let (doc, _) = parse::parse(&src);
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, _) = timing::solve(&doc, &tts, None);
    let d = agentfx_script::lint::static_rules(&doc, &tl, &ir);
    let w = d.iter().find(|x| x.code == "W_GLOW_COST").expect("glow cost");
    assert!(w.message.contains("30 glow/blur layers"), "{}", w.message);
    // one glow on the nest is fine
    let src = loop_src("nest #n levels=30 scale=0.72 w=1920 h=1920 glow=12", "rect x=40 y=40 w=1840 h=1840 stroke=#f0f sw=10", "", "camera loop #n");
    let (ir, _) = compile_src(&src);
    assert!(!agentfx_script::lint::static_rules(&doc, &tl, &ir).iter().any(|x| x.code == "W_GLOW_COST"));
}
