use std::collections::HashMap;
use agentfx_script::ast::{Document, Span};
use agentfx_script::parse::parse;
use agentfx_script::timing::{estimate, solve, tts_requests, TtsReq, TtsResult, TtsWord};

fn parse_doc(source: &str) -> Document {
    let (doc, errors) = parse(source);
    assert!(errors.is_empty(), "{errors:?}");
    doc
}
fn near(a: f64, b: f64) { assert!((a - b).abs() < 1e-8, "{a} != {b}"); }
fn audio(text: &str, dur: f64) -> TtsResult {
    TtsResult { mp3: Some("voice.mp3".into()), dur, estimated: false,
        words: text.split_whitespace().enumerate().map(|(i, text)| TtsWord {
            text: text.into(), start: 0.1 + i as f64 * 0.3, end: 0.3 + i as f64 * 0.3,
        }).collect() }
}
const EXAMPLE: &str = r#"video "Neu im Studio" size=1920x1080 fps=30 voice=en_US-ljspeech-medium rate=+6% lead=2f tail=6f
pronounce "staging.example.test" "staging punkt example punkt test"
scene intro pre=0.5s post=0.4s
  title "Neu im Studio"
  say "Neu im Studio: Panels, Veröffentlichen und der Seiten-Abgleich."
scene panels pre=1.3s
  browser #app
  say "Das rechte Panel hat jetzt [[Reiter]]. Zahlen zeigen, wo etwas [[offen ist|offen]]." min=5s
    @Reiter zoom #tabs
    @offen.end+0.2s pulse #badge
  pause 0.8s
  say "Ein Klick [[wechselt]] den Reiter."
  @2.5s highlight #strip
scene fixed dur=4s
  @1s enter #x
"#;

#[test]
fn example_timing_and_anchors() {
    let doc = parse_doc(EXAMPLE);
    let reqs = tts_requests(&doc);
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[1].key, "panels-0");
    assert_eq!(reqs[0].rate, "+6%");
    let results = reqs.iter().zip([3.0, 4.0, 2.0])
        .map(|(r, dur)| (r.key.clone(), audio(&r.text, dur))).collect();
    let (timeline, diags) = solve(&doc, &results, None);
    assert!(!diags.iter().any(|d| d.code.starts_with("E_")), "{diags:?}");
    let intro = &timeline.scenes[0];
    let panels = &timeline.scenes[1];
    let fixed = &timeline.scenes[2];
    near(intro.start, 0.0);
    near(intro.dur, 0.5 + 2.0 / 30.0 + 3.0 + 6.0 / 30.0 + 0.4);
    near(panels.start, intro.dur);
    near(panels.says[0].start, panels.start + 1.3);
    near(panels.says[0].end - panels.says[0].start, 5.0);
    near(panels.says[1].start, panels.says[0].end + 0.8);
    near(panels.dur, 1.3 + 5.0 + 0.8 + 2.0 / 30.0 + 2.0 + 0.2 + 0.5);
    near(fixed.start, panels.start + panels.dur);
    near(fixed.dur, 4.0);
    near(timeline.total, fixed.start + 4.0);
    let at = |s, say, expr| timeline.anchor_time(s, say, expr, 0.0, Span::default()).unwrap();
    near(at(1, Some(0), "Reiter"), panels.says[0].audio_at + 1.6);
    near(at(1, Some(0), "offen.end+0.2s"), panels.says[0].audio_at + 3.6 + 0.2);
    near(at(1, None, "2.5s"), panels.start + 2.5);
    near(at(2, None, "end-6f"), timeline.total - 0.2);
    near(at(2, None, "start"), fixed.start);
    let err = timeline.anchor_time(1, None, "Reitr", 0.0, Span { line: 42, col: 3 }).unwrap_err();
    assert_eq!(err.code, "E_ANCHOR");
    assert!(err.hint.contains("did you mean") && err.hint.contains("Reiter"));
    assert_eq!((err.line, err.col), (42, 3));
    assert_eq!(timeline.scene_at(-1.0), 0);
    assert_eq!(timeline.scene_at(panels.start), 1);
    assert_eq!(timeline.scene_at(timeline.total + 1.0), 2);
    assert_eq!(diags.iter().filter(|d| d.code == "W_ANCHOR_UNUSED").count(), 1);
}

#[test]
fn pronunciation_expansion_and_contraction() {
    let doc = parse_doc(r#"pronounce "staging.example.test" "staging punkt example punkt test"
pronounce "New York" "NYC"
scene #links post=0s
  say "Besuche [[staging.example.test|site]] in [[New York|city]]." #speech pre=0s post=0s
    @site pulse #link
"#);
    let requests = tts_requests(&doc);
    assert_eq!(requests[0].text, "Besuche staging punkt example punkt test in NYC.");
    let (timeline, _) = solve(&doc, &HashMap::from([("links-0".into(), audio(&requests[0].text, 3.0))]), None);
    let say = &timeline.scenes[0].says[0];
    assert_eq!(say.id.as_deref(), Some("speech"));
    assert_eq!(say.words.len(), 5);
    assert_eq!(say.words[1].text, "staging.example.test");
    near(say.words[1].start, 0.4);
    near(say.words[1].end, 1.8);
    near(say.words[3].start, 2.2);
    near(say.words[4].end, 2.4);
    near(timeline.anchor_time(0, None, "site.end", 0.0, Span::default()).unwrap(), 1.8);
    near(timeline.anchor_time(0, None, "speech.end", 0.0, Span::default()).unwrap(), 3.0);
    assert_eq!(say.written, "Besuche staging.example.test in New York.");
}

#[test]
fn overrides_overruns_and_diagnostics() {
    let doc = parse_doc(r#"video "Test" fps=30 lead=2f tail=6f
scene dur=1s post=0s
  say "[[Hallo|same]]" pre=3f post=0.2s min=3s
    @same pulse #x
  say "[[Hallo|same]]" pre=0s post=0s
    @same pulse #x
    @missing pulse #x
scene long min=7s dur=4s post=0s
"#);
    let results = HashMap::from([("scene1-0".into(), audio("Hallo", 1.0)), ("scene1-1".into(), audio("Hallo", 1.0))]);
    let (timeline, diags) = solve(&doc, &results, Some(60.0));
    near(timeline.fps, 60.0);
    near(timeline.scenes[0].says[0].audio_at, 0.05);
    near(timeline.scenes[0].dur, 4.0);
    near(timeline.scenes[1].start, 4.0);
    near(timeline.scenes[1].dur, 7.0);
    let at = |say| timeline.anchor_time(0, say, "same", 0.0, Span::default()).unwrap();
    near(at(None), 0.15);
    near(at(Some(1)), 3.1);
    near(timeline.anchor_time(0, None, "prev+6f", 9.0, Span::default()).unwrap(), 9.1);
    let overrun = diags.iter().find(|d| d.code == "E_TIMING_OVERRUN").unwrap();
    assert!(overrun.message.contains("4.000") && overrun.message.contains("1.000"));
    assert_eq!(overrun.hint, "remove dur= so the scene follows its narration, or shorten the narration");
    assert!(diags.iter().any(|d| d.code == "W_ANCHOR_DUP"));
    assert!(!diags.iter().any(|d| d.code == "W_ANCHOR_UNUSED"));
    assert_eq!(diags.iter().find(|d| d.code == "E_ANCHOR").unwrap().line, 7);
}

#[test]
fn requests_estimates_and_empty_alignment() {
    let doc = parse_doc(r#"video "Test" voice=base rate=+6%
pronounce "eins" "one"
pronounce "one" "ONE"
scene test
  say "[[eins]] ↗ …" voice=custom rate=+100%
  say "written words" spoken="  override ↗ … text  "
"#);
    let reqs = tts_requests(&doc);
    assert_eq!(reqs[0].text, "ONE");
    assert_eq!(reqs[0].voice, "custom");
    assert_eq!(reqs[1].voice, "base");
    assert_eq!(reqs[1].text, "override text");
    let estimated = estimate(&reqs[0]);
    near(estimated.dur, 0.1);
    near(estimated.words[0].start, 0.05);
    near(estimated.words[0].end, 0.1);
    assert!(estimated.estimated && estimated.mp3.is_none());
    let (timeline, _) = solve(&doc, &HashMap::new(), None);
    assert!(timeline.scenes[0].says.iter().all(|s| s.estimated));
    let empty = TtsResult { mp3: None, dur: 2.0, words: vec![], estimated: false };
    let (timeline, _) = solve(&doc, &HashMap::from([("test-1".into(), empty)]), None);
    let say = &timeline.scenes[0].says[1];
    near(say.words[0].start, say.audio_at);
    near(say.words[0].end, say.audio_at + 1.0);
    near(say.words[1].end, say.audio_at + 2.0);
    let empty_req = TtsReq { key: "empty".into(), text: String::new(), voice: "base".into(), rate: "+0%".into() };
    let empty = estimate(&empty_req);
    near(empty.dur, 0.0);
    assert!(empty.words.is_empty());
    let (empty, diags) = solve(&parse_doc(""), &HashMap::new(), None);
    assert!(empty.scenes.is_empty() && diags.is_empty());
    assert_eq!(empty.scene_at(0.0), 0);
}
