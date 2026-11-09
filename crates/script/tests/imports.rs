//! P5 imports: brand kits, `svg ... parts`, `logo`, `lottie` through compile and direct mode.
use agentfx_script::{diag::Severity, direct, ir::Kind, lower, parse, timing};
use std::path::{Path, PathBuf};

fn assets() -> PathBuf {
    Path::new(env!("AGENTFX_ROOT")).join("examples/features")
}

fn compile(src: &str) -> lower::Compiled {
    let (mut doc, errors) = parse::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    let mut diags = vec![];
    agentfx_script::brand::expand(&mut doc, &assets(), &mut diags);
    assert!(diags.is_empty(), "{diags:?}");
    let (tl, _) = timing::solve(&doc, &Default::default(), None);
    lower::compile(&doc, &tl, None, &assets())
}

fn errors(c: &lower::Compiled) -> Vec<String> {
    c.diags.iter().filter(|d| d.severity == Severity::Error).map(|d| format!("{} {} ({})", d.code, d.message, d.hint)).collect()
}

fn find<'a>(l: &'a agentfx_script::ir::Layer, name: &str) -> Option<&'a agentfx_script::ir::Layer> {
    if l.name.as_deref() == Some(name) {
        return Some(l);
    }
    l.children.iter().find_map(|c| find(c, name))
}

#[test]
fn svg_parts_are_addressable_path_layers() {
    let c = compile(
        "video \"t\" size=960x540 fps=30 captions=off\nscene a dur=3s\n  svg #logo src=assets/brand/lumen-logo.svg w=540 parts recolor=\"#e30613:#3d5afe\"\n  @0s draw #logo dur=1.5s\n  @1s enter #logo.word rise\n",
    );
    assert!(errors(&c).is_empty(), "{:?}", errors(&c));
    let root = &c.ir.root;
    let logo = find(root, "logo").expect("svg group");
    assert!(matches!(logo.kind, Kind::Group));
    for p in ["logo.mark", "logo.ring", "logo.spark", "logo.word", "logo.l", "logo.n"] {
        assert!(find(root, p).is_some(), "part {p}");
    }
    let ring = find(root, "logo.ring").unwrap();
    assert!(matches!(ring.kind, Kind::Path { .. }));
    // recolor mapped the ring's stroke; 14 user units at 540/540 = 1 px per unit
    let s = ring.style.stroke.as_ref().unwrap();
    assert_eq!(s.to_css().to_ascii_lowercase(), "#3d5afe");
    assert!((ring.style.stroke_w - 14.0).abs() < 1e-6, "{}", ring.style.stroke_w);
    // draw: every path has a trim track; the filled spark also gets fill + stroke color tracks
    let has = |id: &str, ch: &str| c.ir.tracks.iter().any(|t| t.layer == id && format!("{:?}", t.ch).eq_ignore_ascii_case(ch) && t.keys.len() > 1);
    let spark = find(root, "logo.spark").unwrap();
    assert!(has(&ring.id, "TrimEnd") && has(&spark.id, "TrimEnd"));
    assert!(has(&spark.id, "Fill") && has(&spark.id, "Stroke"));
}

#[test]
fn unknown_svg_part_lists_the_parts() {
    let c = compile("video \"t\" size=960x540 fps=30 captions=off\nscene a dur=2s\n  svg #logo src=assets/brand/lumen-logo.svg w=540 parts\n  @0.5s pulse #logo.sprak\n");
    let d = c.diags.iter().find(|d| d.code == "E_SVG_ID").expect("E_SVG_ID");
    assert!(d.hint.contains("#logo.spark"), "{}", d.hint);
}

#[test]
fn raw_svg_without_parts_is_unchanged() {
    let c = compile("video \"t\" size=960x540 fps=30 captions=off\nscene a dur=1s\n  svg #logo src=assets/brand/lumen-logo.svg w=540\n");
    let logo = find(&c.ir.root, "logo").unwrap();
    assert!(matches!(logo.kind, Kind::Svg { .. }));
    assert!(logo.children.is_empty());
}

#[test]
fn brand_kit_drives_tokens_logo_and_lottie() {
    let c = compile(
        "video \"t\" size=960x540 fps=30 captions=off\nbrand \"assets/brand/lumen.json\"\nscene a dur=2s\n  logo #l w=400\n  rect #r w=10 h=10 fill=$coral\n  lottie #s src=assets/brand/spinner.json w=200 loop\n  image #bad src=$logoo\n",
    );
    let errs = errors(&c);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].starts_with("E_TOKEN") && errs[0].contains("$logo"), "{errs:?}");
    // on the kit's dark background the logo is the white variant, imported as parts
    let l = find(&c.ir.root, "l").unwrap();
    assert!(matches!(l.kind, Kind::Group));
    let word = find(&c.ir.root, "l.l").unwrap();
    assert_eq!(word.style.stroke.as_ref().unwrap().to_css().to_ascii_lowercase(), "#ffffff");
    let s = find(&c.ir.root, "s").unwrap();
    assert!(matches!(s.kind, Kind::Lottie { looped: true, .. }));
    // direct mode: lottie content with the scene clock, evenodd/fill fields only where used
    let dir = Path::new(env!("AGENTFX_ROOT")).join("target/test-out/imports").join(std::process::id().to_string());
    let out = direct::compile(&c.ir, &dir, false).unwrap();
    let scene: agentfx_ir::Scene = serde_json::from_value(out.scene.clone()).unwrap();
    assert!(scene.nodes.iter().any(|n| matches!(&n.content, Some(agentfx_ir::Content::Lottie { looped: true, .. }))));
}
