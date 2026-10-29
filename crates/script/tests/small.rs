use agentfx_script::{diag::Severity, lower, parse, timing};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let p = Path::new(env!("AGENTFX_ROOT")).join("target/test-out/small/tests").join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn build(body: &str, base: &Path) -> lower::Compiled {
    let (doc, errors) = parse::parse(&format!("video \"test\" size=320x180 fps=12 captions=off\nscene a dur=1s bg=#000\n{body}"));
    assert!(errors.is_empty(), "{errors:?}");
    let (tl, _) = timing::solve(&doc, &Default::default(), None);
    lower::compile(&doc, &tl, None, base)
}

#[test]
fn jpeg_assets_are_normalized_and_unsupported_images_diagnosed() {
    let dir = scratch("jpeg");
    let img = image::RgbImage::from_pixel(16, 8, image::Rgb([240, 30, 20]));
    img.save(dir.join("photo.jpg")).unwrap();
    let c = build("  image #photo src=photo.jpg x=0 y=0 w=160 h=80 fit=contain\n  rect x=160 y=0 w=160 h=80 fill=image(photo.jpg)\n", &dir);
    assert!(c.diags.iter().all(|d| d.severity != Severity::Error), "{:?}", c.diags);
    let out = agentfx_script::direct::compile(&c.ir, &dir.join("compiled"), true).unwrap();
    assert!(out.diags.iter().all(|d| d.severity != Severity::Error), "{:?}", out.diags);
    let assets = out.scene["assets"].as_array().unwrap();
    assert!(!assets.is_empty());
    for a in assets {
        let p = a["path"].as_str().unwrap();
        let reader = image::ImageReader::open(p).unwrap().with_guessed_format().unwrap();
        assert_eq!(reader.format(), Some(image::ImageFormat::Png));
        assert_eq!(reader.decode().unwrap().width(), 16);
    }
    std::fs::write(dir.join("bad.gif"), b"GIF89a").unwrap();
    for body in ["  image src=bad.gif\n", "  rect fill=image(bad.gif)\n"] {
        assert!(build(body, &dir).diags.iter().any(|d| d.code == "E_ASSET"));
    }
}

#[test]
fn split_gradient_paints_glyphs_and_nowrap_keeps_one_row() {
    let dir = scratch("split");
    for nowrap in [false, true] {
        let c = build(&format!("  text #words \"ONE TWO THREE\" x=0 y=0 w=130 size=40 split=word {} fill=linear(90deg, #f00, #00f) origin=30%,70%\n", if nowrap { "nowrap" } else { "" }), &dir);
        assert!(c.diags.iter().all(|d| d.severity != Severity::Error), "{:?}", c.diags);
        let mut pivot = None;
        c.ir.root.walk(&mut |l| { if l.id == "u-words-0" { pivot = Some(l.style.origin); } });
        assert_eq!(pivot, Some((0.3, 0.7)));
        let out = agentfx_script::direct::compile(&c.ir, &dir, false).unwrap();
        assert!(out.diags.iter().all(|d| d.severity != Severity::Error), "{:?}", out.diags);
        let scene: agentfx_ir::Scene = serde_json::from_value(out.scene).unwrap();
        let mut ys = vec![];
        for n in &scene.nodes {
            if let Some(agentfx_ir::Content::Paint { items, .. }) = &n.content {
                for item in items {
                    if let agentfx_ir::Draw::Glyphs { brush, transform, .. } = item {
                        assert!(matches!(brush, agentfx_ir::Brush::Linear { .. }), "{brush:?}");
                        ys.push(transform[5]);
                    }
                }
            }
        }
        assert!(ys.len() >= 3, "{ys:?}");
        let range = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max) - ys.iter().copied().fold(f64::INFINITY, f64::min);
        assert_eq!(range < 1.0, nowrap, "glyph baselines {ys:?}");
    }
}

#[cfg(unix)]
#[test]
fn render_cli_forwards_mode_alpha_and_format_even_with_cache() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("cli");
    let input = dir.join("clip.reel");
    std::fs::write(&input, "video \"x\" size=64x64 fps=12 captions=off\nscene a dur=1s\n  rect w=30 h=30 fill=#f00\n").unwrap();
    let stub = dir.join("renderer");
    let log = dir.join("args");
    std::fs::write(&stub, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$AGENTFX_TEST_ARGS\"\nprintf '{}\\n'\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    for ext in ["mp4", "mov", "webm", "gif", "webp"] {
        let output = dir.join(format!("export.{ext}"));
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_agentfx"));
        cmd.arg("render").arg(&input).args(["--no-tts", "--mode", "cfr", "--cache", "-o"]).arg(&output)
            .env("AGENTFX_RENDERER", &stub).env("AGENTFX_TEST_ARGS", &log);
        if ext != "mp4" { cmd.arg("--alpha"); }
        let r = cmd.output().unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        let args = std::fs::read_to_string(&log).unwrap();
        assert!(args.contains("--mode\ncfr\n"), "{args}");
        assert!(args.contains(output.to_str().unwrap()), "{args}");
        assert_eq!(args.contains("--alpha\n"), ext != "mp4");
    }
    let r = std::process::Command::new(env!("CARGO_BIN_EXE_agentfx")).arg("render").arg(&input).args(["--no-tts", "--mode", "invalid"]).output().unwrap();
    assert!(!r.status.success());
}
