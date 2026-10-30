// Stylised 3D (scene3d): the showcase lowers to reel-ir that validates (objects, screens pointing
// at hidden nodes, fonts, baked camera moves); verbs produce the tracks they promise; lint
// explains mistakes; the text form round-trips.
use agentfx_ir::eval::eval_track;
use agentfx_ir::s3d::Shape3d;
use agentfx_ir::{Content, Sampled, Scene};
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::{lower, parse, print, timing};
use std::collections::HashMap;
use std::path::Path;

fn compile_src(src: &str) -> (agentfx_script::ir::Ir, Vec<Diag>) {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, mut diags) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, None, Path::new(env!("AGENTFX_ROOT")).join("examples/features").as_path());
    diags.extend(c.diags);
    (c.ir, diags)
}

fn scene_json(ir: &agentfx_script::ir::Ir) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("reel-s3d-{}", std::process::id()));
    let c = agentfx_script::direct::compile(ir, &dir, false).unwrap();
    let errs: Vec<_> = c.diags.iter().filter(|x| x.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let _ = std::fs::remove_dir_all(&dir);
    c.scene
}

fn num(s: &Scene, node: usize, prop: &str, t: f64) -> Option<f64> {
    s.tracks.iter().filter(|tr| tr.node == node && tr.property == prop).find_map(|tr| match eval_track(tr, t) {
        Some(Sampled::Number(x)) => Some(x),
        _ => None,
    })
}

const HEAD: &str = "video \"x\" size=640x360 fps=30 captions=off\nscene a dur=3s\n";

#[test]
fn showcase_lowers_to_valid_reel_ir() {
    let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("examples/features/3d.reel")).unwrap();
    let (ir, diags) = compile_src(&src);
    let errs: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    assert!(!diags.iter().any(|d| d.code.starts_with("W_S3D") || d.code.starts_with("E_S3D")), "{diags:#?}");
    let js = scene_json(&ir);
    let s: Scene = serde_json::from_value(js.clone()).unwrap();
    let find = |name: &str| s.nodes.iter().position(|n| n.name == name).unwrap_or_else(|| panic!("node {name}"));
    let find3 = |name: &str| s.nodes.iter().position(|n| n.name == name && matches!(n.content, Some(Content::Scene3d(_)))).unwrap_or_else(|| panic!("scene3d {name}"));
    let s3 = |name: &str| match &s.nodes[find3(name)].content {
        Some(Content::Scene3d(c)) => c.clone(),
        _ => unreachable!(),
    };
    // the mark: two extruded SVG paths (chevron + spark) in a group, a torus ring, the floor
    let blocks = s3("logo");
    let paths = blocks.objects.iter().filter(|o| matches!(&o.shape, Shape3d::Extrude { text: None, .. })).count();
    assert_eq!(paths, 2, "chevron and spark are path extrusions");
    for o in &blocks.objects {
        if let (Shape3d::Extrude { .. }, Some(p)) = (&o.shape, o.parent) {
            assert!(matches!(blocks.objects[p].shape, Shape3d::Group), "the mark sits in its group");
        }
    }
    assert!(blocks.objects.iter().any(|o| matches!(o.shape, Shape3d::Torus { .. })), "a ring");
    assert!(blocks.objects.iter().any(|o| o.receive), "a floor receives shadows");
    assert!(blocks.bloom.is_some() && blocks.fog.is_some());
    // the orbit is baked into camera tracks: the camera moves around the target, distance kept
    let n = find3("logo");
    let (x0, z0) = (num(&s, n, "s3d.cam.x", 0.0).unwrap(), num(&s, n, "s3d.cam.z", 0.0).unwrap());
    let (x1, z1) = (num(&s, n, "s3d.cam.x", 2.7).unwrap(), num(&s, n, "s3d.cam.z", 2.7).unwrap());
    assert!((x1 - x0).abs() > 1.0, "orbit moved the camera: {x0} -> {x1}");
    let (r0, r1) = ((x0 * x0 + z0 * z0).sqrt(), (x1 * x1 + z1 * z1).sqrt());
    assert!((r0 - r1).abs() < 0.6, "orbit keeps the distance roughly (pitch changes it a little): {r0} {r1}");
    // in=drop: the block falls (y track above its rest at the start, resting later)
    let yi = blocks.objects.iter().position(|o| o.id == "spark").unwrap();
    let y_start = num(&s, n, &format!("s3d.obj.{yi}.y"), 0.56).unwrap();
    let y_rest = num(&s, n, &format!("s3d.obj.{yi}.y"), 2.5).unwrap();
    assert!(y_start > y_rest + 3.0, "drop starts high: {y_start} vs {y_rest}");
    // ship: stack piles the containers of a group (bottom on the group's origin)
    let sea = s3("sea");
    let ns = find3("sea");
    let c1 = sea.objects.iter().position(|o| o.id == "c1").unwrap();
    let kids: Vec<usize> = (0..sea.objects.len()).filter(|&i| sea.objects[i].parent == Some(c1)).collect();
    assert_eq!(kids.len(), 3);
    let ys: Vec<f64> = kids.iter().map(|&k| num(&s, ns, &format!("s3d.obj.{k}.y"), 7.6).unwrap()).collect();
    assert!((ys[0] - 0.3).abs() < 1e-3 && (ys[1] - 0.93).abs() < 1e-3 && (ys[2] - 1.56).abs() < 1e-3, "stacked: {ys:?}");
    // keynote: the screen shows the hidden 2D layer `slides`
    let stage = s3("stage");
    let slides = find("slides");
    assert!(s.nodes[slides].hidden);
    let scr = stage.objects.iter().find_map(|o| match &o.shape {
        Shape3d::Screen { node, rect, .. } => Some((*node, *rect)),
        _ => None,
    }).expect("screen");
    assert_eq!(scr.0, slides);
    assert_eq!(scr.1, [0.0, 0.0, 1920.0, 1080.0]);
    assert!(stage.lights.iter().any(|l| l.kind == "point"));
    match agentfx_ir::validate::validate(&s, Path::new("/")) {
        Ok(_) => {}
        Err(e) => assert!(e.iter().all(|m| m.contains("font") || m.contains("asset")), "{e:#?}"),
    }
}

#[test]
fn verbs_write_the_tracks_they_promise() {
    let src = format!("{HEAD}  scene3d #w camera=(0, 0, 10)\n    box #b size=1\n    group #g\n      box size=1\n      box size=1\n  @0s spin #b by=90deg dur=1s ease=linear\n  @0s dolly #w by=4 dur=1s ease=linear\n  @1s truck #w by=2 dur=1s ease=linear\n  @2s assemble #g dur=0.5s\n");
    let (ir, diags) = compile_src(&src);
    assert!(!diags.iter().any(|d| d.severity == Severity::Error), "{diags:#?}");
    let s: Scene = serde_json::from_value(scene_json(&ir)).unwrap();
    let n = s.nodes.iter().position(|n| n.name == "w").unwrap();
    // spin: ry 0 -> 90
    assert!((num(&s, n, "s3d.obj.0.ry", 0.5).unwrap() - 45.0).abs() < 1e-6);
    assert!((num(&s, n, "s3d.obj.0.ry", 1.5).unwrap() - 90.0).abs() < 1e-6);
    // dolly by=4: the camera moves 4 units toward its target
    assert!((num(&s, n, "s3d.cam.z", 1.0).unwrap() - 6.0).abs() < 1e-3);
    // truck by=2: camera and target slide sideways (to the camera's right: +x here)
    assert!((num(&s, n, "s3d.cam.x", 2.0).unwrap() - 2.0).abs() < 1e-3);
    assert!((num(&s, n, "s3d.cam.tx", 2.0).unwrap() - 2.0).abs() < 1e-3);
    // assemble: pieces fly in from elsewhere and end where they were
    let p = 2; // first box of the group (objects: b, g, box, box)
    let far = (0..3).map(|k| num(&s, n, &format!("s3d.obj.{p}.{}", ["x", "y", "z"][k]), 2.0).unwrap_or(0.0).abs()).sum::<f64>();
    let rest = (0..3).map(|k| num(&s, n, &format!("s3d.obj.{p}.{}", ["x", "y", "z"][k]), 2.9).unwrap_or(0.0).abs()).sum::<f64>();
    assert!(far > 1.0 && rest < 1e-3, "assemble from {far} to {rest}");
}

#[test]
fn lint_explains_3d_mistakes() {
    let src = format!(
        "{HEAD}  layer #ui x=0 y=0 w=320 h=180\n  scene3d #w camera=(0, 0, 10) fogg=#000\n    box #b size=1 colr=#f00\n    teapot #t\n    box #behind size=1 z=20\n    box #side size=1 x=500\n    box #near size=1 z=9.5\n    screen #s src=#nope\n    screen #s2\n  sphere #lost r=1\n  @0s spin #ui by=90deg\n"
    );
    let (_, diags) = compile_src(&src);
    let has = |code: &str, frag: &str| diags.iter().any(|d| d.code == code && (d.message.contains(frag) || d.hint.contains(frag)));
    assert!(has("W_S3D_PROP", "fog"), "scene prop typo: {diags:#?}");
    assert!(has("W_S3D_PROP", "color"), "object prop typo");
    assert!(has("E_S3D_KIND", "teapot"), "unknown object kind");
    assert!(has("E_S3D_KIND", "scene3d"), "3D object outside a scene3d");
    assert!(has("W_S3D_VIEW", "#behind") && has("W_S3D_VIEW", "behind the camera"));
    assert!(has("W_S3D_VIEW", "#side") && has("W_S3D_VIEW", "outside the frame"));
    assert!(has("W_S3D_CLIP", "#near"));
    assert!(has("E_S3D_SCREEN", "nope"), "unknown screen source");
    assert!(has("E_S3D_SCREEN", "src=#layer"), "screen without src");
    assert!(has("E_S3D_TARGET", "spin"), "3D verb on a 2D layer");
}

#[test]
fn bad_values_are_errors_with_hints() {
    let src = format!("{HEAD}  scene3d #w camera=(0, 0) fov=500 material=shiny\n    box #b size=-1\n    extrude #e depth=1\n");
    let (_, diags) = compile_src(&src);
    let errs: Vec<&Diag> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.iter().any(|d| d.message.contains("camera=")), "{errs:#?}");
    assert!(errs.iter().any(|d| d.message.contains("fov=")));
    assert!(errs.iter().any(|d| d.message.contains("material") && d.hint.contains("flat")));
    assert!(errs.iter().any(|d| d.message.contains("size=")));
    assert!(errs.iter().any(|d| d.message.contains("extrude needs text")));
}

#[test]
fn scene3d_text_roundtrips() {
    let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("examples/features/3d.reel")).unwrap();
    let (doc, errs) = parse::parse(&src);
    assert!(errs.is_empty());
    let text = print::print(&doc);
    let (again, errs) = parse::parse(&text);
    assert!(errs.is_empty());
    assert_eq!(print::print(&again), text);
    // tuples stay one value
    let scene = doc.nodes.iter().find(|n| n.kind == "scene").unwrap();
    let s3 = scene.children.iter().find(|n| n.kind == "scene3d").unwrap();
    assert_eq!(s3.props["camera"].raw(), "(-2.2, 2.2, 10.5)");
    let json = agentfx_script::json::to_json(&doc);
    let back = agentfx_script::json::from_json(&json).unwrap();
    assert_eq!(print::print(&back), text);
}

#[test]
fn small_examples_compile_clean() {
    for f in ["3d-letters.reel", "3d-screen.reel", "3d-shapes.reel"] {
        let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join("examples/features").join(f)).unwrap();
        let (ir, diags) = compile_src(&src);
        let bad: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error || d.code.contains("S3D")).collect();
        assert!(bad.is_empty(), "{f}: {bad:#?}");
        let s: Scene = serde_json::from_value(scene_json(&ir)).unwrap();
        assert!(s.nodes.iter().any(|n| matches!(n.content, Some(Content::Scene3d(_)))), "{f}");
    }
}
