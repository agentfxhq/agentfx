// End-to-end compile tests: examples parse, time (estimated TTS), compile to IR + HTML without
// errors in every declared format, and round-trip through the JSON form.
use agentfx_script::diag::Severity;
use agentfx_script::{html, json, lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

fn build(file: &str, fmt: Option<&str>) -> (agentfx_script::ir::Ir, String, Vec<agentfx_script::diag::Diag>) {
    let p = Path::new(env!("AGENTFX_ROOT")).join(file);
    let src = std::fs::read_to_string(&p).unwrap();
    let (doc, errs) = parse::parse(&src);
    assert!(errs.is_empty(), "{file}: parse errors {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, mut diags) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, fmt, p.parent().unwrap());
    diags.extend(c.diags);
    let h = html::emit(&c.ir, &html::HtmlOpts { embed: false, fonts_dir: Path::new(env!("AGENTFX_ROOT")).join("assets/fonts"), snap: None });
    (c.ir, h, diags)
}

#[test]
fn examples_compile_clean_in_all_formats() {
    for (f, fmts) in [("examples/app-tour.reel", vec!["16:9", "9:16"]), ("examples/showcase.reel", vec!["16:9", "9:16"])] {
        for fmt in fmts {
            let (ir, h, d) = build(f, Some(fmt));
            let errs: Vec<_> = d.iter().filter(|x| x.severity == Severity::Error).collect();
            assert!(errs.is_empty(), "{f} {fmt}: {errs:#?}");
            assert!(ir.tracks.len() > 50, "{f}: too few tracks");
            assert!(h.contains("@keyframes"), "{f}: no keyframes");
            assert!(h.contains("window.__reel") || h.contains("__reel"), "{f}: no runtime");
            if fmt == "9:16" {
                assert_eq!((ir.w, ir.h), (1080, 1920));
            }
        }
    }
}

#[test]
fn json_form_roundtrip_of_examples() {
    for f in ["examples/app-tour.reel", "examples/showcase.reel"] {
        let src = std::fs::read_to_string(Path::new(env!("AGENTFX_ROOT")).join(f)).unwrap();
        let (doc, _) = parse::parse(&src);
        let back = json::from_json(&json::to_json(&doc)).unwrap();
        assert_eq!(json::to_json(&doc), json::to_json(&back));
        let (doc2, e2) = parse::parse(&agentfx_script::print::print(&doc));
        assert!(e2.is_empty());
        assert_eq!(json::to_json(&doc), json::to_json(&doc2), "{f}: text round trip");
    }
}

#[test]
fn deterministic_output() {
    let (_, a, _) = build("examples/showcase.reel", None);
    let (_, b, _) = build("examples/showcase.reel", None);
    assert_eq!(a, b, "same source must give byte-identical HTML");
}

#[test]
fn unknown_id_gets_a_suggestion() {
    let src = "video \"t\"\nscene a\n  text #title \"Hallo\"\n  say \"Ein [[Wort]] hier.\"\n    @Wort pulse #titel\n";
    let (doc, _) = parse::parse(src);
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, _) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, None, Path::new("."));
    let d = c.diags.iter().find(|d| d.code == "E_ID").expect("E_ID");
    assert!(d.hint.contains("#title"), "hint was {}", d.hint);
}

#[test]
fn interrupting_a_tween_keeps_earlier_frames() {
    use agentfx_script::ir::{Ch, Track, TV};
    use agentfx_script::values::Ease;
    let mut t = Track::new("x", Ch::X, TV::N(0.0));
    t.to(0.0, 1.0, TV::N(100.0), Ease::Cubic(0.65, 0.0, 0.35, 1.0));
    let before: Vec<f64> = (0..=10).map(|i| t.eval(i as f64 * 0.05).n()).collect();
    t.to(0.5, 1.5, TV::N(-50.0), Ease::Linear); // later event interrupts at 0.5 s
    for (i, b) in before.iter().enumerate() {
        let a = t.eval(i as f64 * 0.05).n();
        assert!((a - b).abs() < 0.6, "frame at {:.2}s changed: {b} -> {a}", i as f64 * 0.05);
    }
}

// S1: direct mode compiles every example to valid reel-ir (reel's own validator) without a browser,
// deterministically, and the layout-aware features resolve.
#[test]
fn direct_mode_emits_valid_reel_ir() {
    for f in ["examples/app-tour.reel", "examples/showcase.reel", "examples/components.reel", "crates/script/tests/fixtures/s1-layout.reel"] {
        for fmt in ["16:9", "9:16"] {
            let (ir, _, d) = build(f, Some(fmt));
            assert!(d.iter().all(|x| x.severity != Severity::Error), "{f} {fmt}: {d:#?}");
            let dir = std::env::temp_dir().join(format!("reel-test-{}", std::process::id()));
            let a = agentfx_script::direct::compile(&ir, &dir, false).unwrap();
            let errs: Vec<_> = a.diags.iter().filter(|x| x.severity == Severity::Error).collect();
            assert!(errs.is_empty(), "{f} {fmt}: {errs:#?}");
            let b = agentfx_script::direct::compile(&ir, &dir, false).unwrap();
            assert_eq!(serde_json::to_string(&a.scene).unwrap(), serde_json::to_string(&b.scene).unwrap(), "{f}: not deterministic");
        }
    }
}

#[test]
fn split_line_and_zoom_element_use_native_layout() {
    let (ir, _, d) = build("crates/script/tests/fixtures/s1-layout.reel", Some("16:9"));
    assert!(d.iter().all(|x| x.code != "W_LAYOUT_PASS" && x.code != "W_ZOOM_LAYOUT"), "{d:#?}");
    let mut lines = 0;
    ir.root.walk(&mut |l| if l.id.starts_with("u-poem") && l.id.contains("-p") { lines += 1 });
    assert!(lines >= 2, "split=line made {lines} line layers");
    // zoom #a pans the scene world (x/y tracks), move to=#c moves #a
    assert!(ir.tracks.iter().any(|t| t.layer.starts_with("world-zoomer") && t.ch == agentfx_script::ir::Ch::X));
    assert!(ir.tracks.iter().any(|t| t.layer.starts_with("u-a-") && t.ch == agentfx_script::ir::Ch::X && t.keys.iter().any(|k| k.v.n() > 100.0)));
}

#[test]
fn chart_bar_boxes_are_constant_while_values_count() {
    // counting value labels must never move a bar (fixed column geometry, tabular figures)
    use agentfx_script::ir::*;
    let (ir, _, _) = build("examples/showcase.reel", Some("9:16"));
    let mut bars = vec![];
    ir.root.walk(&mut |l| if l.tags.iter().any(|t| t == "bar") { bars.push(l.id.clone()) });
    assert!(bars.len() >= 5);
    let dyn_layers: Vec<String> = ir.tracks.iter().filter(|t| matches!(t.ch.stage(), Stage::Layout) || matches!(t.ch, Ch::Color | Ch::Fill)).map(|t| t.layer.clone()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    let tmp = std::env::temp_dir().join(format!("reel-test-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let fonts = Path::new(env!("AGENTFX_ROOT")).join("assets/fonts");
    let mut nat = agentfx_script::native::Native::load(&ir, agentfx_script::native::snap_at(&ir, 0.0), &fonts, &tmp, false).unwrap();
    let mut first: HashMap<String, [f64; 4]> = HashMap::new();
    let mut n = 0;
    let mut t = 0.0;
    while t < ir.duration.min(12.0) {
        nat.set_snap(&agentfx_script::native::snap_at(&ir, t), &dyn_layers);
        for b in &bars {
            if let Some(r) = nat.rect(b) {
                let f = first.entry(b.clone()).or_insert(r);
                for k in 0..4 {
                    assert!((f[k] - r[k]).abs() < 0.01, "bar {b} moved at t={t:.2}: {f:?} -> {r:?}");
                }
                n += 1;
            }
        }
        t += 1.0 / 30.0;
    }
    assert!(n > 100);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn chart_line_markers_wait_for_the_stroke_and_follow_the_axes() {
    // line/area charts: no marker is visible while the trimmed path has zero length, and the
    // draw-on starts no later than 0.3 s after the axes/gridlines finish entering
    use agentfx_script::ir::*;
    let (ir, _, _) = build("examples/showcase.reel", Some("9:16"));
    let mut lines = vec![];
    ir.root.walk(&mut |l| if l.tags.iter().any(|t| t == "line") && l.id.contains("-trend") { lines.push(l.id.clone()) });
    assert_eq!(lines.len(), 1, "one trend line expected: {lines:?}");
    let line = &lines[0];
    let chart = line.strip_suffix("-line").unwrap();
    let mut dots = vec![];
    ir.root.walk(&mut |l| if l.id.starts_with(chart) && matches!(l.kind, Kind::Ellipse) && l.tags.iter().any(|t| t == "dot") { dots.push(l.id.clone()) });
    assert_eq!(dots.len(), 7, "{dots:?}");
    let trim = ir.tracks.iter().find(|t| &t.layer == line && t.ch == Ch::TrimEnd).expect("line trim track");
    let scales: Vec<&Track> = dots.iter().map(|d| ir.tracks.iter().find(|t| &t.layer == d && t.ch == Ch::Scale).expect("dot scale track")).collect();
    let grids: Vec<&Track> = ir.tracks.iter().filter(|t| t.layer.starts_with(chart) && (t.layer.contains("-grid") || t.layer.contains("-baseline")) && t.ch == Ch::Opacity).collect();
    assert!(grids.len() >= 3, "axes tracks: {}", grids.len());
    let axes_end = grids.iter().map(|t| t.keys.last().unwrap().t).fold(0.0, f64::max);
    let mut line_start = None;
    let mut frames = 0;
    let mut t = 0.0;
    while t < ir.duration {
        let tr = trim.eval(t).n();
        if tr <= 1e-6 {
            for (d, s) in dots.iter().zip(&scales) {
                assert!(s.eval(t).n() <= 1e-6, "marker {d} visible at t={t:.3} while the path is empty");
            }
            frames += 1;
        } else if line_start.is_none() {
            line_start = Some(t);
        }
        t += 1.0 / ir.fps;
    }
    assert!(frames > 30, "no empty-path frames sampled");
    let line_start = line_start.expect("line never draws");
    assert!(line_start >= axes_end - 1.0 / ir.fps, "line ({line_start:.3}) draws before the axes finish ({axes_end:.3})");
    assert!(line_start - axes_end <= 0.3 + 1.0 / ir.fps, "line starts {:.3} s after the axes finish (limit 0.3 s)", line_start - axes_end);
}

#[test]
fn builtin_theme_names_and_aliases() {
    use agentfx_script::theme::Theme;
    for n in ["studio-dark", "studio-light", "dark", "light", "levels-dark", "levels-light"] {
        assert!(Theme::builtin(n).is_some(), "{n}");
    }
    assert!(Theme::builtin("nope").is_none());
}
