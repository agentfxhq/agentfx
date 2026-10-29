// Design layer tests: lens loading into the theme, design validation, harvest of a style frame.
use agentfx_script::{parse, theme::Theme};
use std::path::Path;

fn theme_of(src: &str) -> (Theme, Vec<agentfx_script::diag::Diag>) {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "{errs:?}");
    agentfx_script::design::set_doc_base(Path::new(env!("AGENTFX_ROOT")));
    let mut d = vec![];
    let th = Theme::from_doc(&doc, &mut d);
    (th, d)
}

#[test]
fn lens_extends_theme_tokens() {
    let (th, d) = theme_of("video \"x\"\ndesign lens=editorial\n");
    assert!(d.is_empty(), "{d:?}");
    assert_eq!(th.d.display_font.as_deref(), Some("Source Serif 4"));
    assert_eq!(th.font, "Libre Franklin");
    assert_eq!(th.d.skins.get("node").map(|s| s.as_str()), Some("tag"));
    assert!(th.colors["bg"].r > 0.9, "editorial is a light paper ground");
    // the type scale fills unset sizes from base * ratio^k
    assert!(th.size("display").unwrap() > th.size("h1").unwrap());
}

#[test]
fn theme_block_overrides_design() {
    let (th, _) = theme_of("video \"x\"\ndesign lens=swiss\ntheme\n  colors accent=#00ff00\n");
    assert!(th.colors["accent"].g > 0.9);
    assert_eq!(th.d.surface, "outline");
}

#[test]
fn bad_design_tokens_are_errors() {
    let (_, d) = theme_of("video \"x\"\ndesign \"t\"\n  surface style=wobbly\n  tpye base=40\n");
    let codes: Vec<&str> = d.iter().map(|x| x.code.as_str()).collect();
    assert!(codes.iter().filter(|c| **c == "E_DESIGN_TOKEN").count() >= 2, "{codes:?}");
}

#[test]
fn every_lens_loads_cleanly() {
    // the five public lenses, plus any other lens folder present in this checkout
    let mut lenses: Vec<String> = ["swiss", "editorial", "keynote", "flat", "blueprint"].map(String::from).to_vec();
    for e in std::fs::read_dir(Path::new(env!("AGENTFX_ROOT")).join("design/lenses")).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if e.path().join("design.reel").is_file() && !lenses.contains(&n) {
            lenses.push(n);
        }
    }
    for l in &lenses {
        let (th, d) = theme_of(&format!("video \"x\"\ndesign lens={l}\n"));
        assert!(d.is_empty(), "{l}: {d:?}");
        assert!(agentfx_script::principles::design_rules(&th).iter().all(|x| x.code != "E_DESIGN_FONT"), "{l}");
    }
}

#[test]
fn harvest_reads_computed_styles() {
    let f = Path::new(env!("AGENTFX_ROOT")).join("design/lenses/editorial/frame-data.html");
    let v = agentfx_script::harvest::extract(&f, 1920, 1080, &Path::new(env!("AGENTFX_ROOT")).join("assets/fonts")).unwrap();
    let els = v["elements"].as_array().unwrap();
    assert!(els.iter().any(|e| e["family"] == "Source Serif 4" && e["size"].as_f64().unwrap() >= 80.0), "serif headline");
    assert!(els.iter().any(|e| e["upper"] == true), "uppercase kicker");
}
