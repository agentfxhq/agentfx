// Audio reactivity end to end: a synthetic click track goes through ffmpeg, the analysis cache,
// beat anchors in the timing solver and the lowering in the compiler.
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::ir::Ch;
use agentfx_script::{lower, parse, timing};
use std::path::{Path, PathBuf};

fn ffmpeg_available() -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("bin/ffmpeg").exists() || std::process::Command::new("ffmpeg").arg("-version").output().is_ok()
}

/// 16-bit mono WAV with a decaying 1 kHz click on every beat.
fn click_wav(path: &Path, bpm: f64, secs: f64) {
    let rate = 22050u32;
    let n = (secs * rate as f64) as usize;
    let mut s = vec![0.0f64; n];
    let mut t = 0.0;
    while t < secs {
        let at = (t * rate as f64) as usize;
        for i in 0..1200.min(n.saturating_sub(at)) {
            let x = i as f64 / rate as f64;
            s[at + i] += (-x * 50.0).exp() * (std::f64::consts::TAU * 1000.0 * x).sin() * 0.8;
        }
        t += 60.0 / bpm;
    }
    let mut b = Vec::new();
    let data = (n * 2) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for v in s {
        b.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("AGENTFX_ROOT")).join("target/test-out/test-audio").join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn build(base: &Path, src: &str) -> (agentfx_script::ir::Ir, timing::Timeline, Vec<Diag>) {
    let (doc, errs) = parse::parse(src);
    assert!(errs.is_empty(), "{errs:?}");
    agentfx_script::design::set_doc_base(base);
    let (tl, mut diags) = timing::solve(&doc, &Default::default(), None);
    let c = lower::compile(&doc, &tl, None, base);
    diags.extend(c.diags);
    (c.ir, tl, diags)
}

// one test, so the process-wide document base is never shared between threads
#[test]
fn audio_reactivity_end_to_end() {
    if !ffmpeg_available() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let base = dir("e2e");
    click_wav(&base.join("click.wav"), 120.0, 6.0);
    let src = r#"video "t" size=1280x720 fps=30 captions=off
music src=click.wav
scene a dur=4s
  rect #logo w=100 h=100 x=10% y=10% scale=audio(level, gain=0.5, offset=1, smooth=100ms)
  pulse #logo on=bar amount=0.2
  visualizer #viz count=16 w=800 h=200 x=50% y=70% anchor=center
  text #hit "hit" x=50% y=20% anchor=center opacity=0
  at beat:3
    set #hit opacity=1
scene b dur=2s
  visualizer #ring type=radial count=24 w=400 h=400 x=50% y=50% anchor=center
  at bar:3+0.1s
    pulse #ring
"#;
    let (ir, tl, diags) = build(&base, src);
    let errs: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "{errs:#?}");
    // beat grid: 120 bpm click track starting at 0
    let b3 = tl.anchor_time(0, None, "beat:3", 0.0, Default::default()).unwrap();
    assert!((b3 - 1.0).abs() < 0.04, "beat:3 at {b3}");
    let bar2 = tl.anchor_time(1, None, "bar:2", 0.0, Default::default()).unwrap();
    assert!((bar2 - 2.0).abs() < 0.04, "bar:2 at {bar2}");
    // the analysis is cached by content hash
    let cached: Vec<_> = std::fs::read_dir(base.join("cache/audio")).unwrap().flatten().collect();
    assert_eq!(cached.len(), 1);
    // lowered tracks exist on the right layers
    let has = |layer: &str, ch: Ch| ir.tracks.iter().any(|t| t.layer == layer && t.ch == ch && t.keys.len() > 2);
    assert!(has("u-logo-0", Ch::Scale), "logo scale track");
    assert!(has("u-viz.b1-0", Ch::Sy) && has("u-viz.b16-0", Ch::Sy), "bar tracks");
    assert!(has("u-ring.b24-1", Ch::Sx), "radial tracks in scene b");
    let ring_scale = ir.tracks.iter().find(|t| t.layer == "u-ring-1" && t.ch == Ch::Scale).expect("pulse at bar:3");
    assert!(ring_scale.keys.iter().any(|k| (k.t - 4.1).abs() < 0.05), "pulse starts at bar:3+0.1s");

    // a second compile hits the cache and gives the same IR
    let (ir2, _, _) = build(&base, src);
    assert_eq!(serde_json::to_string(&ir.tracks).unwrap(), serde_json::to_string(&ir2.tracks).unwrap());

    // lint: beat beyond the track, flash risk, missing music
    let (_, _, d) = build(&base, "video \"t\" fps=30\nmusic src=click.wav\nscene a dur=4s\n  at beat:99\n    pulse #x\n  rect #x w=10 h=10\n");
    assert!(d.iter().any(|x| x.code == "E_BEAT_RANGE"), "{d:#?}");
    let fast = dir("flash");
    click_wav(&fast.join("fast.wav"), 480.0, 3.0);
    let (_, _, d) = build(&fast, "video \"t\" fps=30\nmusic src=fast.wav\nscene a dur=3s\n  rect #x w=10 h=10 opacity=audio(level)\n");
    assert!(d.iter().any(|x| x.code == "W_AUDIO_FLASH"), "{d:#?}");
    let (_, _, d) = build(&fast, "video \"t\" fps=30\nmusic src=fast.wav\nscene a dur=3s\n  rect #x w=10 h=10 opacity=audio(level, smooth=1s)\n");
    assert!(!d.iter().any(|x| x.code == "W_AUDIO_FLASH"), "{d:#?}");
    let (_, _, d) = build(&fast, "video \"t\" fps=30\nmusic src=missing.wav\nscene a dur=1s\n  visualizer #v\n");
    assert!(d.iter().any(|x| x.code == "E_AUDIO" && x.message.contains("missing.wav")), "{d:#?}");
    let (_, _, d) = build(&fast, "video \"t\" fps=30\nscene a dur=1s\n  rect #x w=1 h=1 sy=audio(band=2)\n");
    assert!(d.iter().any(|x| x.code == "E_AUDIO"), "{d:#?}");
    let (_, _, d) = build(&fast, "video \"t\" fps=30\nmusic src=fast.wav\nscene a dur=1s\n  rect #x w=1 h=1 sy=audio(band=12)\n");
    assert!(d.iter().any(|x| x.code == "E_AUDIO" && x.message.contains("band")), "{d:#?}");
}
