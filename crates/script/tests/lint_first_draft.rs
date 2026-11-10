// First-draft lint rules found by the agent evals (evals/): W_SCENE_SHORT, W_NUMBER_FORMAT.
use agentfx_script::{lint, lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

fn codes(src: &str) -> Vec<String> {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors: {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, _) = timing::solve(&doc, &tts, None);
    let base = Path::new(env!("AGENTFX_ROOT"));
    let c = lower::compile(&doc, &tl, Some("16:9"), base);
    lint::static_rules(&doc, &tl, &c.ir).into_iter().map(|d| d.code).collect()
}

#[test]
fn scene_without_dur_or_narration_is_flagged() {
    let src = "video \"T\" size=1920x1080 captions=off\n\nscene budget\n  text #t \"Where our budget goes\" size=120\n";
    assert!(codes(src).contains(&"W_SCENE_SHORT".to_string()));
}

#[test]
fn scene_with_dur_say_or_pause_is_not_flagged() {
    for body in ["scene a dur=4s\n  text #t \"Hi\"\n", "scene a\n  text #t \"Hi\"\n  say \"Hello there, this is a test.\"\n", "scene a\n  text #t \"Hi\"\n  pause 3s\n"] {
        let src = format!("video \"T\" size=1920x1080 captions=off\n\n{body}");
        assert!(!codes(&src).contains(&"W_SCENE_SHORT".to_string()), "{body}");
    }
}

#[test]
fn big_counter_without_sep_and_decimal_chart_without_locale_are_flagged() {
    let src = "video \"T\" size=1920x1080 captions=off\n\nscene a dur=4s\n  counter #n from=0 to=10000\n  chart #c type=bar data=[2.1, 3.5] labels=[A, B]\n";
    let c = codes(src);
    assert_eq!(c.iter().filter(|x| *x == "W_NUMBER_FORMAT").count(), 2, "{c:?}");
}

#[test]
fn particles_emitted_from_below_the_frame_are_not_out_of_frame_errors() {
    let src = "video \"T\" size=1080x1920 captions=off\n\nscene a dur=3s\n  particles #b count=40 seed=3 x=50% y=105% speed=500 spread=40 gravity=-200 life=3s shape=dot emit=@start\n";
    assert!(!codes(src).contains(&"E_OUT_OF_FRAME".to_string()));
    // a placed element outside the frame still is one
    let src = "video \"T\" size=1080x1920 captions=off\n\nscene a dur=3s\n  text #t \"Hi\" x=50% y=120%\n";
    assert!(codes(src).contains(&"E_OUT_OF_FRAME".to_string()));
}

#[test]
fn count_resolves_its_target_in_its_own_scene() {
    // same id in two scenes: only the second has sep=, and the count is in the second
    let src = "video \"T\" size=1920x1080 captions=off\n\nscene a dur=2s\n  counter #n from=0 to=10\n\nscene b dur=3s\n  counter #n from=0 to=0 sep=\",\"\n  @0s count #n to=10000\n";
    assert!(!codes(src).contains(&"W_NUMBER_FORMAT".to_string()));
}

#[test]
fn explicit_sep_and_locale_are_silent() {
    let src = "video \"T\" size=1920x1080 captions=off\n\nscene a dur=4s\n  counter #n from=0 to=10000 sep=\",\"\n  chart #c type=bar data=[2.1, 3.5] labels=[A, B] locale=en\n  stat #s value=12480 sep=none label=\"Users\"\n  counter #small to=4300\n";
    assert!(!codes(src).contains(&"W_NUMBER_FORMAT".to_string()));
}
