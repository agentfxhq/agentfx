use agentfx_script::{diag::Severity, direct, lower, parse, timing};
use std::path::Path;
use std::process::Command;

#[test]
fn video_layers_lower_with_timing_and_survive_frame_ranges() {
    let dir = Path::new(env!("AGENTFX_ROOT"))
        .join("target/test-out/video")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("clip.mkv");
    let made = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=16x12:rate=2",
            "-frames:v",
            "4",
            "-c:v",
            "ffv1",
        ])
        .arg(&clip)
        .status();
    if made.as_ref().is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return;
    }
    assert!(made.unwrap().success(), "ffmpeg could not make test clip");
    let src = r#"video "test" size=320x180 fps=12 captions=off
scene first dur=1s bg=#000
  video #loop src=clip.mkv x=0 y=0 w=160 h=100 fit=contain in=0.5s rate=1.5 loop
scene second dur=1s bg=#000
  video #hold src=clip.mkv x=160 y=0 w=160 h=100 fit=fill hold
"#;
    let (doc, errors) = parse::parse(src);
    assert!(errors.is_empty(), "{errors:?}");
    let (timeline, _) = timing::solve(&doc, &Default::default(), None);
    let compiled = lower::compile(&doc, &timeline, None, &dir);
    assert!(compiled.diags.iter().all(|d| d.severity != Severity::Error), "{:?}", compiled.diags);
    let out = direct::compile(&compiled.ir, &dir.join("compiled"), false).unwrap();
    assert!(out.diags.iter().all(|d| d.severity != Severity::Error), "{:?}", out.diags);
    let scene: agentfx_ir::Scene = serde_json::from_value(out.scene.clone()).unwrap();
    let videos: Vec<_> = scene
        .nodes
        .iter()
        .filter_map(|n| match &n.content {
            Some(agentfx_ir::Content::Video { start, offset, rate, looped, hold, .. }) => {
                Some((*start, *offset, *rate, *looped, *hold))
            }
            _ => None,
        })
        .collect();
    assert_eq!(videos, vec![(0.0, 0.5, 1.5, true, false), (1.0, 0.0, 1.0, false, true)]);
    let ranged: agentfx_ir::Scene = serde_json::from_value(direct::range_ir(&out.scene, 12, 13)).unwrap();
    let starts: Vec<_> = ranged
        .nodes
        .iter()
        .filter_map(|n| match &n.content {
            Some(agentfx_ir::Content::Video { start, .. }) => Some(*start),
            _ => None,
        })
        .collect();
    assert_eq!(starts, vec![-1.0, 0.0]);
}
