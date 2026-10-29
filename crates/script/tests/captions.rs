// Caption pack: transcript import as timing beats, styles, layout and lint.
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::ir::{Ch, Ir, Layer};
use agentfx_script::{lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

/// `design::set_doc_base` is process-global: tests that resolve `src=` paths take turns.
static BASE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn root() -> &'static Path {
    Path::new(env!("AGENTFX_ROOT"))
}

/// Compile source text whose relative paths resolve against the crate root.
fn compile_src(src: &str, fmt: Option<&str>) -> (Ir, timing::Timeline, Vec<Diag>) {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors {errs:?}");
    let _g = BASE.lock().unwrap_or_else(|e| e.into_inner());
    agentfx_script::design::set_doc_base(root());
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, mut diags) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, fmt, root());
    diags.extend(c.diags);
    (c.ir, tl, diags)
}

fn compile_file(file: &str, fmt: Option<&str>) -> (Ir, timing::Timeline, Vec<Diag>) {
    let p = root().join(file);
    let src = std::fs::read_to_string(&p).unwrap();
    let (doc, errs) = parse::parse(&src);
    assert!(errs.is_empty(), "{file}: parse errors {errs:?}");
    let _g = BASE.lock().unwrap_or_else(|e| e.into_inner());
    agentfx_script::design::set_doc_base(p.parent().unwrap());
    let (tl, mut diags) = timing::solve(&doc, &HashMap::new(), None);
    let c = lower::compile(&doc, &tl, fmt, p.parent().unwrap());
    diags.extend(c.diags);
    (c.ir, tl, diags)
}

fn layers(ir: &Ir) -> Vec<Layer> {
    let mut v = vec![];
    ir.root.walk(&mut |l| v.push(l.clone()));
    v
}

fn codes(d: &[Diag]) -> Vec<String> {
    d.iter().map(|x| x.code.clone()).collect()
}

fn errors(d: &[Diag]) -> Vec<&Diag> {
    d.iter().filter(|x| x.severity == Severity::Error).collect()
}

#[test]
fn examples_compile_clean_in_every_format() {
    for f in ["examples/captions/captions-9x16.reel", "examples/captions/captions-16x9.reel"] {
        for fmt in ["9:16", "1:1", "16:9"] {
            let (ir, tl, d) = compile_file(f, Some(fmt));
            assert!(errors(&d).is_empty(), "{f} {fmt}: {:#?}", errors(&d));
            assert!(!d.iter().any(|x| x.code.starts_with("W_CAPTION")), "{f} {fmt}: {:#?}", d.iter().filter(|x| x.code.starts_with("W_CAPTION")).collect::<Vec<_>>());
            // every scene plays its voice file and lasts past its last word
            let voices = ir.audio.iter().filter(|a| a.kind == "voice").count();
            assert_eq!(voices, tl.scenes.len(), "{f}");
            for s in &tl.scenes {
                let last = s.says.iter().flat_map(|y| y.words.iter()).map(|w| w.end).fold(0.0, f64::max);
                assert!(last > s.start && last < s.start + s.dur, "{f}: scene {} words end {last} outside {}+{}", s.id, s.start, s.dur);
            }
            let ls = layers(&ir);
            assert!(ls.iter().filter(|l| l.id.contains("-w") && l.id.ends_with("-t")).count() >= 25, "{f}: too few word layers");
        }
    }
}

#[test]
fn pack_uses_no_color_tracks() {
    let (ir, _, _) = compile_file("examples/captions/captions-9x16.reel", Some("9:16"));
    let cap: Vec<_> = ir.tracks.iter().filter(|t| t.layer.starts_with("cpk-")).collect();
    assert!(cap.len() > 40, "only {} caption tracks", cap.len());
    for t in cap {
        assert!(matches!(t.ch, Ch::Opacity | Ch::Scale | Ch::Y | Ch::Reveal), "{} animates {:?}", t.layer, t.ch);
    }
}

#[test]
fn srt_words_are_spread_and_glyphs_cleaned() {
    let src = "video \"t\" size=1080x1920 captions=off\nscene a\n  captions src=crates/script/tests/fixtures/captions/talk.srt style=outline\n";
    let (ir, tl, d) = compile_src(src, None);
    assert!(errors(&d).is_empty(), "{:#?}", errors(&d));
    assert!(codes(&d).contains(&"I_CAPTION_ESTIMATED".to_string()));
    assert!(codes(&d).contains(&"W_CAPTION_GLYPH".to_string()), "{:?}", codes(&d));
    let words: Vec<_> = tl.scenes[0].says.iter().flat_map(|s| s.words.iter()).collect();
    // tags stripped, two cues, words inside their cue
    assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>()[..3], ["Hello", "there,", "world."]);
    assert!((words[0].start - 0.5).abs() < 1e-6);
    assert!((words[2].end - 2.0).abs() < 1e-6);
    // the emoji never reaches a text layer
    let fire = layers(&ir).iter().any(|l| matches!(&l.kind, agentfx_script::ir::Kind::Text { runs } if runs.iter().any(|r| r.text.contains('\u{1F525}'))));
    assert!(!fire);
}

#[test]
fn vtt_inline_word_times() {
    let src = "video \"t\" captions=off\nscene a\n  captions src=crates/script/tests/fixtures/captions/talk.vtt\n";
    let (_, tl, d) = compile_src(src, None);
    assert!(!codes(&d).contains(&"I_CAPTION_ESTIMATED".to_string()), "{:?}", codes(&d));
    let w: Vec<_> = tl.scenes[0].says[0].words.iter().map(|w| (w.text.clone(), w.start)).collect();
    assert_eq!(w.len(), 3);
    assert!((w[1].1 - 0.4).abs() < 1e-6 && (w[2].1 - 0.9).abs() < 1e-6, "{w:?}");
}

#[test]
fn narration_restyle_and_header_style() {
    // a captions node without src restyles the scene's TTS captions (no legacy inline group)
    let src = "video \"t\" size=1080x1920\nscene a\n  say \"This is a fairly short [[sentence]] for the pill.\"\n    @sentence pulse #x\n  text #x \"Hi\"\n  captions style=pill\n";
    let (ir, _, d) = compile_src(src, None);
    assert!(errors(&d).is_empty(), "{:#?}", errors(&d));
    let ls = layers(&ir);
    assert!(ls.iter().any(|l| l.id.ends_with("-pill")));
    assert!(!ls.iter().any(|l| l.id.starts_with("cap-a-0")), "legacy captions still emitted");
    // header style for every narrated scene
    let src = "video \"t\" size=1080x1920 captions=hormozi\nscene a\n  say \"Numbers like 4200 become the keyword.\"\nscene b\n  say \"Second scene talks too.\"\n";
    let (ir, _, d) = compile_src(src, None);
    assert!(errors(&d).is_empty(), "{:#?}", errors(&d));
    let ls = layers(&ir);
    let hi = ls.iter().filter(|l| l.id.ends_with("-hi")).count();
    assert!(hi >= 9, "{hi} highlight layers");
    // the number is the auto keyword: its base text is the key color (green)
    let num = ls.iter().find(|l| l.id.ends_with("-t") && matches!(&l.kind, agentfx_script::ir::Kind::Text { runs } if runs[0].text == "4200")).expect("number word");
    let c = num.style.color.clone().unwrap();
    assert!(c.g > 0.8 && c.r < 0.4, "{c:?}");
}

#[test]
fn lint_style_prop_src() {
    let src = "video \"t\" captions=off\nscene a\n  captions src=crates/script/tests/fixtures/captions/talk.srt style=hormozy colr=#fff\n  captions src=nope.srt\n";
    let (_, _, d) = compile_src(src, None);
    let c = codes(&d);
    assert!(c.contains(&"E_CAPTION_STYLE".to_string()), "{c:?}");
    let st = d.iter().find(|x| x.code == "E_CAPTION_STYLE").unwrap();
    assert!(st.hint.contains("hormozi"), "{}", st.hint);
    assert!(c.contains(&"E_CAPTION_SRC".to_string()), "{c:?}");
    let src = "video \"t\" captions=off\nscene a\n  captions src=crates/script/tests/fixtures/captions/talk.srt colr=#fff\n";
    let (_, _, d) = compile_src(src, None);
    let p = d.iter().find(|x| x.code == "W_CAPTION_PROP").expect("prop warning");
    assert!(p.hint.contains("color"), "{}", p.hint);
}

#[test]
fn lint_overflow_frame_safe_read() {
    let base = "video \"t\" size=1080x1920 captions=off\nscene a\n  captions src=crates/script/tests/fixtures/captions/";
    // a word wider than the line box
    let (_, _, d) = compile_src(&format!("{base}talk.srt size=300\n"), None);
    let o = d.iter().find(|x| x.code == "E_CAPTION_OVERFLOW").expect("overflow");
    assert_eq!(o.format.as_deref(), Some("9:16"));
    // off frame / outside the social safe area
    let (_, _, d) = compile_src(&format!("{base}talk.srt place=top y=99%\n"), None);
    assert!(codes(&d).contains(&"E_CAPTION_OFF_FRAME".to_string()), "{:?}", codes(&d));
    let (_, _, d) = compile_src(&format!("{base}talk.srt y=92%\n"), None);
    assert!(codes(&d).contains(&"W_CAPTION_SAFE".to_string()), "{:?}", codes(&d));
    // same placement is fine in 16:9 defaults
    let (_, _, d) = compile_src(&format!("{base}talk.srt\n"), Some("16:9"));
    assert!(!codes(&d).iter().any(|c| c.starts_with("W_CAPTION_SAFE") || c.starts_with("E_CAPTION")), "{:?}", codes(&d));
    // "Hi." flashes for 0.14 s before the next phrase
    let (_, _, d) = compile_src(&format!("{base}fast.json\n"), None);
    let r = d.iter().find(|x| x.code == "W_CAPTION_READ").expect("read-time warning");
    assert!(r.message.contains("Hi."), "{}", r.message);
}

#[test]
fn all_styles_compile() {
    for st in agentfx_script::captions::PRESETS {
        for fmt in ["9:16", "1:1", "16:9"] {
            let src = format!("video \"t\" captions=off\nscene a\n  captions src=crates/script/tests/fixtures/captions/fast.json style={st} mincue=0s minword=0s\n");
            let (ir, _, d) = compile_src(&src, Some(fmt));
            assert!(errors(&d).is_empty(), "{st} {fmt}: {:#?}", errors(&d));
            assert!(layers(&ir).iter().any(|l| l.id.ends_with("-w0-t")), "{st} {fmt}");
        }
    }
}

#[test]
fn measured_widths_match_native_layout() {
    // word boxes come from the compiler's own measurement; the native layout (Blitz, the render
    // path) must agree, or pills and spacing drift
    for st in ["pill", "hormozi", "typewriter"] {
        let src = format!("video \"t\" size=1080x1920 captions=off\nscene a\n  captions src=crates/script/tests/fixtures/captions/fast.json style={st}\n");
        let (ir, _, _) = compile_src(&src, None);
        let fonts = root().join("assets/fonts");
        let tmp = std::env::temp_dir();
        let nat = agentfx_script::native::Native::load(&ir, agentfx_script::native::snap_at(&ir, 1.0), &fonts, &tmp, false).unwrap();
        let mut n = 0;
        for l in layers(&ir) {
            if !l.id.ends_with("-t") || !l.id.starts_with("cpk-") {
                continue;
            }
            let group = l.id.trim_end_matches("-t");
            let gw = layers(&ir).into_iter().find(|g| g.id == group).and_then(|g| g.layout.w).map(|w| w.v).unwrap();
            let r = nat.rect(&l.id).unwrap();
            // the native rect is rounded to whole px
            assert!((r[2] - gw).abs() < 3.0, "{st} {}: native {:.1} vs measured {:.1}", l.id, r[2], gw);
            n += 1;
        }
        assert!(n >= 6, "{st}: {n} words checked");
    }
}
