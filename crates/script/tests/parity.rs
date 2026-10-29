// Direct-mode parity: color adjust, mesh and image fills, path color
// tracks, gradient strokes, relative path data, svg viewBox fit, blend modes. Every scene is
// emitted, parsed by reel-ir and validated by reel-ir's own validator.
use agentfx_ir::{BlendMode, Brush, Content, Draw, Effect, Scene};
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

/// Emitted scene, its diagnostics (compile + emit) and the output dir.
fn emit(src: &str, tag: &str) -> (Scene, Vec<Diag>, std::path::PathBuf) {
    let base = Path::new(env!("AGENTFX_ROOT")).join("examples/features");
    let (ir, mut diags) = compile_src(src, &base);
    let errs: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    let dir = std::env::temp_dir().join(format!("reel-parity-{tag}-{}", std::process::id()));
    let c = agentfx_script::direct::compile(&ir.unwrap(), &dir, false).unwrap();
    let errs: Vec<_> = c.diags.iter().filter(|x| x.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    diags.extend(c.diags);
    let s: Scene = serde_json::from_value(c.scene).unwrap();
    assert!(agentfx_ir::validate::validate(&s, &dir).is_ok(), "{:?}", agentfx_ir::validate::validate(&s, &dir));
    (s, diags, dir)
}

fn example(name: &str) -> (Scene, Vec<Diag>) {
    let p = Path::new(env!("AGENTFX_ROOT")).join("examples/features").join(name);
    let (s, d, _) = emit(&std::fs::read_to_string(p).unwrap(), name);
    (s, d)
}

fn node<'a>(s: &'a Scene, name: &str) -> (usize, &'a agentfx_ir::Node) {
    s.nodes.iter().enumerate().find(|(_, n)| n.name == name).unwrap_or_else(|| panic!("node {name}"))
}

fn unsupported(d: &[Diag]) -> Vec<String> {
    d.iter().filter(|x| x.code == "W_IR_UNSUPPORTED").map(|x| x.message.clone()).collect()
}

#[test]
fn color_adjust_is_a_reel_ir_effect_with_tracks() {
    let (s, d) = example("color.reel");
    assert!(unsupported(&d).is_empty(), "{:?}", unsupported(&d));
    let color = |name: &str| {
        let (i, n) = node(&s, name);
        let k = n.effects.iter().position(|e| matches!(e, Effect::Color { .. })).unwrap_or_else(|| panic!("{name}: no color effect"));
        (i, k, n.effects[k].clone())
    };
    let (i, k, e) = color("sat");
    assert!(matches!(e, Effect::Color { saturate, .. } if saturate == 0.0));
    assert!(s.tracks.iter().any(|t| t.node == i && t.property == format!("effects.{k}.saturate")));
    let (i, k, _) = color("hue");
    let tr = s.tracks.iter().find(|t| t.node == i && t.property == format!("effects.{k}.hue")).expect("hue track");
    assert_eq!(format!("{:?}", agentfx_ir::eval::eval_track(tr, 6.0)), "Some(Number(360.0))");
    let (i, k, _) = color("punch");
    for f in ["brightness", "contrast"] {
        assert!(s.tracks.iter().any(|t| t.node == i && t.property == format!("effects.{k}.{f}")), "{f}");
    }
    assert!(matches!(color("tint").2, Effect::Color { tint: Some(_), tint_amount, .. } if (tint_amount - 0.85).abs() < 1e-9));
    assert!(matches!(color("levels").2, Effect::Color { levels: Some(l), .. } if l == [0.2, 0.8, 1.4, 0.0, 1.0]));
    assert!(matches!(color("lut").2, Effect::Color { lut: Some(p), .. } if p.ends_with("teal-orange.cube")));
}

#[test]
fn grade_props_validate() {
    let base = Path::new(env!("AGENTFX_ROOT")).join("examples/features");
    let errs = |src: &str| compile_src(src, &base).1.into_iter().filter(|d| d.severity == Severity::Error).map(|d| d.code).collect::<Vec<_>>();
    let head = "video \"x\" size=640x360 fps=30 captions=off\nscene a dur=1s\n";
    assert_eq!(errs(&format!("{head}  rect #r w=100 h=100 levels=0.9,0.1\n")), ["E_VALUE"]);
    assert_eq!(errs(&format!("{head}  rect #r w=100 h=100 levels=1\n")), ["E_VALUE"]);
    assert_eq!(errs(&format!("{head}  rect #r w=100 h=100 lut=missing.cube\n")), ["E_ASSET"]);
    assert!(errs(&format!("{head}  rect #r w=100 h=100 levels=10,245,1.2 tint=#f00 lut=assets/teal-orange.cube contrast=1.2\n")).is_empty());
}

#[test]
fn mesh_and_image_fills_are_raster_brushes() {
    let (s, d) = example("mesh.reel");
    assert!(unsupported(&d).is_empty(), "{:?}", unsupported(&d));
    let image_asset = |b: &Brush| if let Brush::Image { asset, .. } = b { Some(*asset) } else { None };
    // box: Blitz paints the mesh raster as a CSS background image
    let (_, m1) = node(&s, "m1");
    let Some(Content::Paint { items, .. }) = &m1.content else { panic!("m1 paint: {:?}", m1.content) };
    let a = items.iter().find_map(|it| if let Draw::Fill { brush, .. } = it { image_asset(brush) } else { None }).expect("mesh image fill on the box");
    assert!(s.assets[a].path.contains("cache/mesh/"), "{}", s.assets[a].path);
    // path: image brush placed by fill_transform over the box
    for name in ["m2", "i2"] {
        let (_, n) = node(&s, name);
        let Some(Content::Path(p)) = &n.content else { panic!("{name}") };
        assert!(p.fill.as_ref().and_then(image_asset).is_some(), "{name}: {:?}", p.fill);
        assert!(p.fill_transform.is_some(), "{name}");
    }
    // text: image fill composited source-atop over the glyphs
    for name in ["m3", "i3"] {
        let (_, n) = node(&s, name);
        let Some(Content::Paint { items, .. }) = &n.content else { panic!("{name}") };
        assert!(items.iter().any(|it| matches!(it, Draw::Push { blend: BlendMode::SrcAtop, .. })), "{name}");
        assert!(items.iter().any(|it| matches!(it, Draw::Glyphs { .. })), "{name}");
        assert!(items.iter().any(|it| matches!(it, Draw::Fill { brush: Brush::Image { .. }, brush_transform: Some(_), .. })), "{name}");
    }
    // mask: the matte source is a mesh raster
    let (_, src) = node(&s, "u-m4-0-mask");
    let Some(Content::Paint { items, .. }) = &src.content else { panic!() };
    assert!(matches!(&items[0], Draw::Fill { brush: Brush::Image { .. }, brush_transform: Some(_), .. }));
}

#[test]
fn mesh_raster_is_deterministic_and_cached() {
    use agentfx_script::values::Rgba;
    let c = [Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }, Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }];
    let a = agentfx_script::mesh::png(2, 1, &c).unwrap();
    let b = agentfx_script::mesh::png(2, 1, &c).unwrap();
    assert_eq!(a, b);
    assert_eq!(agentfx_script::mesh::raster(2, 1, &c, 32, 8), agentfx_script::mesh::raster(2, 1, &c, 32, 8));
}

#[test]
fn path_colors_are_tracks_and_strokes_take_gradients() {
    let (s, d) = example("paths.reel");
    assert!(unsupported(&d).is_empty(), "{:?}", unsupported(&d));
    let (i, n) = node(&s, "pulse");
    assert!(matches!(n.content, Some(Content::Path(_))), "a path, not switch states");
    for p in ["fill.color", "stroke.color"] {
        let t = s.tracks.iter().find(|t| t.node == i && t.property == p).unwrap_or_else(|| panic!("{p}"));
        assert!(matches!(&t.keyframes[0].value, agentfx_ir::Value::Color(_)));
    }
    let (_, g) = node(&s, "grad");
    let Some(Content::Path(p)) = &g.content else { panic!() };
    assert!(matches!(p.stroke.as_ref().map(|x| &x.brush), Some(Brush::Linear { stops, .. }) if stops.len() == 3));
    // relative data comes out absolute in scene coordinates: no lower-case commands left
    for name in ["tab", "wave", "arcs"] {
        let (_, n) = node(&s, name);
        let Some(Content::Path(p)) = &n.content else { panic!() };
        assert!(!p.d.chars().any(|c| c.is_ascii_lowercase()), "{name}: {}", p.d);
    }
    let (_, tab) = node(&s, "tab");
    let Some(Content::Path(p)) = &tab.content else { panic!() };
    // m20 200 v-120: the second point is 120 px above the first, not at y = -120
    let nums: Vec<f64> = p.d.split(|c: char| c.is_ascii_alphabetic() || c == ' ').filter_map(|x| x.parse().ok()).collect();
    assert!((nums[1] - nums[2] - 120.0).abs() < 0.01, "{}", p.d);
}

#[test]
fn svg_with_size_scales_into_its_box() {
    let (s, _) = example("paths.reel");
    let (_, n) = node(&s, "icon");
    let Some(Content::Paint { rect, .. }) = &n.content else { panic!("{:?}", n.content) };
    // a 24x24 icon in a 240x240 box: its ink fills the box (it used to be cropped at 24 px)
    assert!(rect[2] > 190.0 && rect[3] > 190.0, "{rect:?}");
    let html = agentfx_script::html::fit_svg("<svg width=\"24\" height=\"24\"><path d=\"M0 0\"/></svg>", true, true);
    assert_eq!(html, "<svg viewBox=\"0 0 24 24\" style=\"display:block\" width=\"100%\" height=\"100%\"><path d=\"M0 0\"/></svg>");
    let kept = "<svg viewBox='0 0 24 24' width='100%' height='100%'></svg>";
    assert_eq!(agentfx_script::html::fit_svg(kept, true, true), kept);
}

#[test]
fn every_blend_mode_reaches_reel_ir() {
    let (s, d) = example("blend.reel");
    assert!(unsupported(&d).is_empty(), "{:?}", unsupported(&d));
    let mut seen: Vec<BlendMode> = s.nodes.iter().map(|n| n.blend).collect();
    seen.sort_by_key(|b| format!("{b:?}"));
    seen.dedup();
    assert_eq!(seen.len(), 17, "{seen:?}");
}
