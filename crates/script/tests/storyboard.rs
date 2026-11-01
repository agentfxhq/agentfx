// `agentfx storyboard`: note syntax, shot detection, arrow derivation, deterministic pixels.
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::ir::Ir;
use agentfx_script::storyboard::{scene_transitions, Layout, Opts, Storyboard};
use agentfx_script::{lint, lower, parse, timing};
use std::collections::HashMap;
use std::path::Path;

fn compile(src: &str) -> (Ir, agentfx_script::ast::Document, Vec<Diag>) {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "parse errors {errs:?}");
    let tts: HashMap<String, timing::TtsResult> = timing::tts_requests(&doc).iter().map(|r| (r.key.clone(), timing::estimate(r))).collect();
    let (tl, mut diags) = timing::solve(&doc, &tts, None);
    let c = lower::compile(&doc, &tl, None, Path::new(env!("AGENTFX_ROOT")));
    diags.extend(c.diags);
    diags.extend(lint::static_rules(&doc, &tl, &c.ir));
    (c.ir, doc, diags)
}

fn layout(ir: &Ir) -> Layout {
    let fonts = Path::new(env!("AGENTFX_ROOT")).join("assets/fonts");
    let tmp = std::env::temp_dir();
    let nat = agentfx_script::native::Native::load(ir, agentfx_script::native::snap_at(ir, 0.0), &fonts, &tmp, false).unwrap();
    Layout::from_native(ir, &nat)
}

fn board(src: &str) -> (agentfx_script::storyboard::Board, Vec<Diag>) {
    let (ir, doc, d) = compile(src);
    assert!(!d.iter().any(|x| x.severity == Severity::Error), "{d:#?}");
    let lay = layout(&ir);
    let sb = Storyboard::new(&ir, &lay, &scene_transitions(&doc), Opts { title: "t".into(), ..Default::default() });
    (sb.board.clone(), d)
}

const HEAD: &str = "video \"t\" size=640x360 fps=12 captions=off\n";

#[test]
fn notes_are_metadata() {
    let src = format!("{HEAD}scene a dur=6s note=\"opening\"\n  rect #box x=100 y=100 w=80 h=80 fill=#e33\n  note #n \"a real callout\" title=\"Hi\"\n  @3s note \"box drops\"\n");
    let (ir, _, d) = compile(&src);
    assert!(!d.iter().any(|x| x.severity == Severity::Error), "{d:#?}");
    assert_eq!(ir.notes.len(), 2);
    assert_eq!((ir.notes[0].text.as_str(), ir.notes[0].timed), ("opening", false));
    assert_eq!((ir.notes[1].text.as_str(), ir.notes[1].timed), ("box drops", true));
    assert!((ir.notes[1].at - 3.0).abs() < 1e-6);
    // the callout component is untouched
    let mut ids = vec![];
    ir.root.walk(&mut |l| ids.push(l.id.clone()));
    assert!(ids.iter().any(|i| i.starts_with("u-n-")), "{ids:?}");
}

#[test]
fn note_lints() {
    let (_, _, d) = compile(&format!("{HEAD}scene a dur=4s\n  rect #b x=0 y=0 w=10 h=10 fill=#fff\n  @1s note\n"));
    assert!(d.iter().any(|x| x.code == "E_NOTE"), "{d:#?}");
    let long = "x".repeat(120);
    let (_, _, d) = compile(&format!("{HEAD}scene a dur=4s note=\"{long}\"\n  rect #b x=0 y=0 w=10 h=10 fill=#fff\n  note \"bare\"\n"));
    assert!(d.iter().any(|x| x.code == "W_NOTE_LONG"), "{d:#?}");
    assert!(d.iter().any(|x| x.code == "I_NOTE_CALLOUT"), "{d:#?}");
}

#[test]
fn scenes_are_shots_with_their_transition() {
    let src = format!("{HEAD}scene a dur=3s\n  rect #r x=100 y=100 w=200 h=120 fill=#e33\nscene b dur=3s in=fade\n  ellipse #c x=200 y=100 w=150 h=150 fill=#3e3\nscene c dur=3s in=zoom\n  rect #q x=10 y=10 w=100 h=100 fill=#33e\n");
    let (b, _) = board(&src);
    assert_eq!(b.shots.len(), 3, "{:#?}", b.shots);
    assert_eq!(b.shots[0].trans, None);
    assert_eq!(b.shots[1].trans.as_deref(), Some("DISSOLVE"));
    assert_eq!(b.shots[2].trans.as_deref(), Some("ZOOM THROUGH"));
    assert!((b.shots[1].start - 3.0).abs() < 0.05 && (b.shots[1].end - 6.0).abs() < 0.05);
}

#[test]
fn a_hard_change_inside_a_scene_is_a_cut() {
    // the picture is replaced at 5 s: big red plate out, big blue plate in
    let src = format!("{HEAD}scene one dur=10s\n  rect #red x=40 y=40 w=560 h=280 fill=#e33\n  rect #blue x=60 y=60 w=520 h=240 fill=#33e opacity=0\n  @5s hide #red ; show #blue\n");
    let (b, _) = board(&src);
    assert_eq!(b.shots.len(), 2, "{:#?}", b.shots);
    assert!((b.shots[1].start - 5.0).abs() <= 0.15, "break at {}", b.shots[1].start);
    assert_eq!(b.shots[1].trans.as_deref(), Some("CUT"));
    assert!(b.shots.iter().all(|s| s.auto));
}

#[test]
fn timed_notes_define_the_shots() {
    let src = format!("{HEAD}scene one dur=9s note=\"wide\"\n  rect #red x=40 y=40 w=560 h=280 fill=#e33\n  rect #blue x=60 y=60 w=520 h=240 fill=#33e opacity=0\n  @5s hide #red ; show #blue\n  @3s note \"closer\"\n  @7s note \"blue holds\"\n");
    let (b, _) = board(&src);
    let starts: Vec<f64> = b.shots.iter().map(|s| (s.start * 10.0).round() / 10.0).collect();
    assert_eq!(starts, vec![0.0, 3.0, 7.0], "{:#?}", b.shots);
    let notes: Vec<&str> = b.shots.iter().map(|s| s.note.as_str()).collect();
    assert_eq!(notes, vec!["wide", "closer", "blue holds"]);
    assert!(b.shots.iter().all(|s| !s.auto));
}

#[test]
fn arrows_from_tracks() {
    let src = format!("{HEAD}scene one dur=4s\n  rect #puck x=40 y=150 w=60 h=60 fill=#e33\n  ellipse #ball x=300 y=40 w=60 h=60 fill=#3e3\n  rect #wheel x=470 y=200 w=80 h=80 fill=#33e\n  @0s anim #puck x 0s:0 3.5s:420\n  @0s anim #ball scale 0s:1 3.5s:2.6\n  @0s anim #wheel rot 0s:0 3.5s:360\n");
    let (b, _) = board(&src);
    assert_eq!(b.shots.len(), 1);
    let s = &b.shots[0];
    let find = |k: &str| s.moves.iter().find(|m| m.kind == k).unwrap_or_else(|| panic!("no {k} move in {:#?}", s.moves));
    let p = find("path");
    assert_eq!(p.name, "puck");
    let (a, z) = (p.path.first().unwrap(), p.path.last().unwrap());
    assert!(z[0] - a[0] > 300.0 && (z[1] - a[1]).abs() < 5.0, "{:?} -> {:?}", a, z);
    let g = find("grow");
    assert_eq!(g.name, "ball");
    assert!(g.amount > 2.0);
    let r = find("rotate");
    assert_eq!(r.name, "wheel");
    assert!((r.amount - 360.0).abs() < 30.0, "{}", r.amount);
    assert!(s.note.contains("puck slides right"), "{}", s.note);
    assert!(s.camera.is_empty(), "{:#?}", s.camera);
}

#[test]
fn a_full_frame_zoom_is_a_camera_push() {
    let src = format!("{HEAD}scene one dur=4s\n  layer #world x=0 y=0 w=640 h=360\n    rect #a x=40 y=40 w=200 h=120 fill=#e33\n    rect #b x=360 y=200 w=200 h=120 fill=#33e\n  @0s anim #world scale 0s:1 3.5s:2\n");
    let (b, _) = board(&src);
    let c = &b.shots[0].camera;
    assert_eq!(c.len(), 1, "{c:#?}");
    assert_eq!(c[0].kind, "push-in");
    let inner = c[0].inner.unwrap();
    assert!((inner[2] - 320.0).abs() < 20.0 && (inner[3] - 180.0).abs() < 12.0, "{inner:?}");
    assert!(b.shots[0].moves.iter().all(|m| m.kind != "grow"), "{:#?}", b.shots[0].moves);
    assert!(b.shots[0].note.contains("camera pushes in"), "{}", b.shots[0].note);
}

#[test]
fn rendering_is_deterministic() {
    let src = format!("{HEAD}scene a dur=3s note=\"title rises\"\n  text #t \"HELLO\" x=200 y=140 size=60\n  rect #r x=60 y=60 w=100 h=60 fill=#e33\n  @0s anim #t y 0s:60 2s:0\nscene b dur=3s in=fade\n  ellipse #c x=200 y=100 w=150 h=150 fill=#fff\n  @0s anim #c scale 0s:0.5 2.5s:1.5\n");
    let (ir, doc, _) = compile(&src);
    let lay = layout(&ir);
    let png = |seed: u64| {
        let sb = Storyboard::new(&ir, &lay, &scene_transitions(&doc), Opts { seed, title: "det".into(), ..Default::default() });
        sb.render().pm.encode_png().unwrap()
    };
    let (a, b, c) = (png(1), png(1), png(2));
    assert_eq!(a, b, "same seed, same pixels");
    assert_ne!(a, c, "the seed drives the jitter");
}
