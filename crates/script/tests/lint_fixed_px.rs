// Tests for the I_FIXED_PX lint rule.
use agentfx_script::{lint, lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

fn lint_fixed_px(src: &str) -> Vec<agentfx_script::diag::Diag> {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors: {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc)
        .iter()
        .map(|r| (r.key.clone(), timing::estimate(r)))
        .collect();
    let (tl, _) = timing::solve(&doc, &tts, None);
    let base = Path::new(env!("AGENTFX_ROOT"));
    let c = lower::compile(&doc, &tl, Some("16:9"), base);
    lint::fixed_px_rules(&doc, &c.ir)
}

#[test]
fn multi_format_video_fires_on_large_fixed_px() {
    let src = r#"
video "Test" size=1920x1080 formats=[16:9, 9:16] captions=off
theme studio-light
  bg #222

scene intro dur=2s
  text #big "Hello" size=80 color=#fff w=800 y=50% anchor=center
"#;
    let d = lint_fixed_px(src);
    assert!(!d.is_empty(), "should fire on large fixed-px element in multi-format video");
    assert!(d.iter().all(|x| x.code == "I_FIXED_PX"));
    // at most one per element
    let ids: Vec<_> = d.iter().filter_map(|x| x.id.as_ref()).collect();
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(ids.len(), unique.len(), "should not duplicate per element");
}

#[test]
fn single_format_video_is_silent() {
    let src = r#"
video "Test" size=1920x1080 captions=off
theme studio-light
  bg #222

scene intro dur=2s
  text #big "Hello" size=80 color=#fff w=800 y=50% anchor=center
"#;
    let d = lint_fixed_px(src);
    assert!(d.is_empty(), "single-format video should not fire I_FIXED_PX, got: {d:?}");
}

#[test]
fn element_with_when_override_is_excluded() {
    let src = r#"
video "Test" size=1920x1080 formats=[16:9, 9:16] captions=off
theme studio-light
  bg #222

when portrait
  set #big w=400

scene intro dur=2s
  text #big "Hello" size=80 color=#fff w=800 y=50% anchor=center
"#;
    let d = lint_fixed_px(src);
    let big_hits: Vec<_> = d.iter().filter(|x| x.id.as_deref() == Some("big")).collect();
    assert!(big_hits.is_empty(), "element with `when` override should be excluded, got: {big_hits:?}");
}

#[test]
fn cap_at_five_plus_summary() {
    // create many large fixed-px elements
    let mut src = String::from(r#"
video "Test" size=1920x1080 formats=[16:9, 9:16] captions=off
theme studio-light
  bg #222

scene intro dur=2s
"#);
    for i in 0..10 {
        src.push_str(&format!("  box #item{i} w=800 h=600 fill=#333\n"));
    }
    let d = lint_fixed_px(&src);
    // 5 individual + 1 summary = 6 max
    assert!(d.len() <= 6, "should cap at 5+summary, got {}", d.len());
    // last one should be the summary
    if d.len() == 6 {
        assert!(d[5].message.contains("more elements"), "last diag should be summary: {}", d[5].message);
    }
}
