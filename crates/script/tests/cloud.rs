// Point clouds (`cloud` + `form`): compile to one reel-ir `cloud` content node of a few KB
// (no per-point tracks), lint with fix hints, the scene camera becomes a projection, and cut-out
// frame ranges keep the cloud's time.
use agentfx_ir::{Content, Scene};
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::{lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

fn compile_src(src: &str, base: &Path) -> (agentfx_script::ir::Ir, Vec<Diag>) {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, mut diags) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, None, base);
    diags.extend(c.diags);
    (c.ir, diags)
}

fn file(f: &str) -> (agentfx_script::ir::Ir, Vec<Diag>) {
    let p = Path::new(env!("AGENTFX_ROOT")).join(f);
    compile_src(&std::fs::read_to_string(&p).unwrap(), p.parent().unwrap())
}

fn agentfx_ir(ir: &agentfx_script::ir::Ir, tag: &str) -> (serde_json::Value, Scene) {
    let dir = std::env::temp_dir().join(format!("reel-cloud-{tag}-{}", std::process::id()));
    let c = agentfx_script::direct::compile(ir, &dir, false).unwrap();
    let errs: Vec<_> = c.diags.iter().filter(|x| x.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let s: Scene = serde_json::from_value(c.scene.clone()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (c.scene, s)
}

fn clouds(s: &Scene) -> Vec<(usize, &agentfx_ir::CloudContent)> {
    s.nodes.iter().enumerate().filter_map(|(i, n)| match &n.content {
        Some(Content::Cloud(c)) => Some((i, c)),
        _ => None,
    }).collect()
}

#[test]
fn swarm_is_one_small_cloud_node() {
    let (ir, d) = file("examples/cloud/swarm.reel");
    let bad: Vec<_> = d.iter().filter(|x| x.severity != Severity::Info).collect();
    assert!(bad.is_empty(), "{bad:#?}");
    let (json, s) = agentfx_ir(&ir, "swarm");
    assert!(agentfx_ir::validate::validate(&s, Path::new("/")).is_ok(), "{:?}", agentfx_ir::validate::validate(&s, Path::new("/")));
    let cs = clouds(&s);
    assert_eq!(cs.len(), 1);
    let (i, c) = cs[0];
    assert_eq!(c.count, 20000);
    assert_eq!(c.forms.len(), 5);
    assert!(c.projection.is_empty(), "2D scene: no projection");
    // absolute times, in order; text forms point at a font of the scene
    let at: Vec<f64> = c.forms.iter().map(|f| f.at).collect();
    assert_eq!(at, vec![0.0, 0.6, 2.2, 3.6, 4.8]);
    for f in &c.forms {
        if let agentfx_ir::CloudShape::Text(t) = &f.shape {
            assert!(s.fonts[t.font].path.ends_with("Inter.ttf"));
        }
    }
    assert_eq!(s.nodes[i].motion_blur.map(|m| m.samples), Some(8));
    // no per-point anything: the whole IR stays tiny and the cloud has no tracks
    assert!(json.to_string().len() < 20_000, "{}", json.to_string().len());
    assert!(!s.tracks.iter().any(|t| t.node == i));
}

#[test]
fn camera_scenes_bake_a_projection() {
    let (ir, d) = file("examples/cloud/terrain3d.reel");
    assert!(d.iter().all(|x| x.severity != Severity::Error), "{d:#?}");
    let (_, s) = agentfx_ir(&ir, "3d");
    let (i, c) = clouds(&s)[0];
    // the camera tilts over the whole scene: one key per frame (runs collapse)
    assert!(c.projection.len() > 100, "{}", c.projection.len());
    assert!(c.projection.windows(2).all(|w| w[0].t < w[1].t));
    // a perspective: w depends on z
    assert!(c.projection[0].m[10].abs() > 1e-5);
    // the projection carries the cloud's transform: none on the node
    assert!(!s.tracks.iter().any(|t| t.node == i && t.property == "transform"));
    assert!(matches!(c.forms[1].shape, agentfx_ir::CloudShape::Terrain { deep: true, .. }));
}

#[test]
fn own_perspective_without_camera() {
    let src = "video \"t\" size=640x360 fps=30\nscene a dur=2s\n  cloud #c n=500 persp=900\n    form 0s sphere r=30% spin=20\n";
    let (ir, d) = compile_src(src, Path::new("."));
    assert!(d.iter().all(|x| x.severity != Severity::Error), "{d:#?}");
    let (_, s) = agentfx_ir(&ir, "persp");
    let (_, c) = clouds(&s)[0];
    assert_eq!(c.projection.len(), 1, "static camera: one key");
    assert!((c.projection[0].m[10] + 1.0 / 900.0).abs() < 1e-9);
}

#[test]
fn frame_ranges_shift_the_cloud_clock() {
    let (ir, _) = file("examples/cloud/swarm.reel");
    let (json, _) = agentfx_ir(&ir, "range");
    let r = agentfx_script::direct::range_ir(&json, 45, 46);
    let c = r["nodes"].as_array().unwrap().iter().find(|n| n["content"]["type"] == "cloud").unwrap();
    assert!((c["content"]["time_offset"].as_f64().unwrap() - 1.5).abs() < 1e-9);
    let _: Scene = serde_json::from_value(r.clone()).unwrap();
}

fn codes(src: &str) -> Vec<(String, String, String)> {
    let (_, d) = compile_src(src, Path::new("."));
    d.into_iter().map(|x| (x.code, x.message, x.hint)).collect()
}

#[test]
fn lint_explains_cloud_mistakes() {
    let head = "video \"t\"\nscene a dur=3s\n  cloud #c n=1000\n";
    // unknown shape: did you mean
    let d = codes(&format!("{head}    form 0s spirl\n"));
    assert!(d.iter().any(|(c, m, h)| c == "E_CLOUD" && m.contains("spirl") && h.contains("spiral")), "{d:?}");
    // unknown prop on a shape: did you mean
    let d = codes(&format!("{head}    form 0s field\n    form 1s spiral arsm=3\n"));
    assert!(d.iter().any(|(c, m, h)| c == "W_CLOUD_PROP" && m.contains("arsm") && h.contains("arms")), "{d:?}");
    // bad orders and rules
    let d = codes(&format!("{head}    form 0s field\n    form 1s grid by=distnce color=heigth\n"));
    assert!(d.iter().any(|(c, _, h)| c == "E_CLOUD" && h.contains("distance")), "{d:?}");
    assert!(d.iter().any(|(c, m, _)| c == "E_CLOUD" && m.contains("heigth")), "{d:?}");
    // text form without text, forms out of order, no forms, form outside a cloud
    let d = codes(&format!("{head}    form 0s field\n    form 2s text\n"));
    assert!(d.iter().any(|(c, m, _)| c == "E_CLOUD" && m.contains("without text")), "{d:?}");
    let d = codes(&format!("{head}    form 2s field\n    form 1s grid\n"));
    assert!(d.iter().any(|(c, m, _)| c == "E_CLOUD" && m.contains("earlier")), "{d:?}");
    let d = codes(head);
    assert!(d.iter().any(|(c, m, _)| c == "E_CLOUD" && m.contains("without forms")), "{d:?}");
    let d = codes("video \"t\"\nscene a dur=3s\n  form 0s field\n");
    assert!(d.iter().any(|(c, m, _)| c == "E_CLOUD" && m.contains("outside")), "{d:?}");
    // bad lengths and counts
    let d = codes(&format!("{head}    form 0s spiral r=big\n"));
    assert!(d.iter().any(|(c, m, _)| c == "E_CLOUD" && m.contains("r=big")), "{d:?}");
    let d = codes("video \"t\"\nscene a dur=3s\n  cloud #c n=0\n    form 0s field\n");
    assert!(d.iter().any(|(c, _, _)| c == "E_CLOUD"), "{d:?}");
    // a clean cloud with an anchor-timed form and theme color names
    let d = codes("video \"t\"\ntheme studio-dark\nscene a\n  say \"Dann [[kommt]] die Zahl.\"\n  cloud #c n=800 palette=[paper, accent]\n    form 0s field\n    form @kommt text \"42\" dur=0.5s stagger=0.3s by=x\n");
    assert!(d.iter().all(|(c, _, _)| !c.starts_with('E') && !c.starts_with("W_CLOUD")), "{d:?}");
}
