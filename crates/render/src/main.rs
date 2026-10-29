//! reel CLI.
//!   agentfx html   <scene_dir> --out-dir out/x
//!       HTML frontend (native, Blitz): writes out/x/scene.ir.json (IR with display lists,
//!       fonts/ and images/ next to it), out/x/geom.json, out/x/html-stats.json
//!   reel stats  <scene.ir.json> [--out stats.json]       schedule/damage stats
//!   agentfx render <scene.ir.json> --out x.mp4 [--mode cfr|vfr] [--lossless x.mkv]
//!               [--filter bilinear|bicubic] [--no-mips] [--stats render.json]
//!               [--encoder auto|x264|nvenc|nvenc-hevc|vaapi|qsv] [--qp N] [--rc cqp|icq|qvbr]
//!               [--low-power 0|1] [--compression-level N] [--segments N]
//!               [--nv-preset 1..7] [--aq none|spatial|temporal|both] [--10bit]
//!               [--gpu[=on|off|auto]]  GPU compute for heavy effects (AGENTFX_GPU, default off)
//!               [--alpha] [--fps N] [--width N] [--loop forever|once|N] [--quality N]
//!               [--webp-lossless] [--hide name,name]
//!       --out picks the deliverable: .mp4 (encoders), .mov (ProRes 4444 with --alpha,
//!       else 422 HQ), .webm (VP9), .png (sequence), .gif, .webp (animated); see export.rs.
//!       Core: IR -> frames -> x264. Works for HTML-mode and direct-mode IR alike.
mod blitzfe;
mod cloud;
mod color;
mod digital;
mod composite;
mod css;
mod dither;
mod cssval;
mod effects;
mod fluid;
mod gpu;
mod encode;
mod export;
mod html;
mod jobstatus;
mod lcd;
mod light;
mod look;
mod pixel;
mod m2;
mod plan2;
mod prof;
mod model;
mod pathx;
mod record;
mod regress;
mod render;
mod schedule;
mod schedule_v2;
mod sdft;
mod text;
mod textfx;
mod timefx;
mod vraster;

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn load_ir(path: &Path) -> Result<agentfx_ir::Scene> {
    let scene = agentfx_ir::Scene::load(path).with_context(|| format!("loading IR {}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    match agentfx_ir::validate::validate(&scene, base) {
        Ok(warnings) => {
            for w in warnings {
                eprintln!("warning: {w}");
            }
        }
        Err(errs) => bail!("invalid IR {}:\n  - {}", path.display(), errs.join("\n  - ")),
    }
    Ok(scene)
}

/// HTML mode: CSS timeline (css.rs) + Blitz layout/geometry (blitzfe.rs) -> planner
/// (html.rs) -> native display lists per paint unit / repaint state (blitzfe.rs).
#[allow(dead_code)]
fn html_frontend(scene_dir: &Path, out_dir: &Path) -> Result<(html::HtmlOut, serde_json::Value)> {
    let (o, s, _) = html_frontend_res(scene_dir, out_dir)?;
    Ok((o, s))
}

/// `--encoder auto|x264|nvenc|nvenc-hevc|vaapi|qsv` (default auto: NVENC H.264 when its
/// probe encode succeeds, else VAAPI, else x264, each step with a one-line reason),
/// `--qp N` (VAAPI qp/ICQ/QVBR, QSV global_quality, NVENC -cq; default per codec),
/// `--rc cqp|icq|qvbr`, `--low-power 0|1`, `--compression-level N`; NVENC:
/// `--nv-preset 1..7`, `--aq none|spatial|temporal|both`, `--10bit` (nvenc-hevc main10).
fn encoder_opts(args: &[String]) -> Result<(encode::Codec, encode::HwOpts)> {
    use encode::{Codec, HwOpts, RateControl};
    let want = arg(args, "--encoder").unwrap_or_else(|| "auto".into());
    let rgba = args.iter().any(|a| a == "--rgba-pipe");
    let no_encode = args.iter().any(|a| a == "--no-encode");
    let mut hw = HwOpts::default();
    if let Some(q) = arg(args, "--qp") { hw.qp = q.parse()?; }
    if let Some(r) = arg(args, "--rc") {
        hw.rc = match r.as_str() { "cqp" => RateControl::Cqp, "icq" => RateControl::Icq, "qvbr" => RateControl::Qvbr, _ => bail!("unknown --rc {r}") };
    }
    if let Some(l) = arg(args, "--low-power") { hw.low_power = l == "1"; }
    if let Some(c) = arg(args, "--compression-level") { hw.compression_level = Some(c.parse()?); }
    if let Some(p) = arg(args, "--nv-preset") { hw.nv_preset = p.trim_start_matches('p').parse()?; }
    if let Some(a) = arg(args, "--aq") {
        (hw.spatial_aq, hw.temporal_aq) = match a.as_str() {
            "none" => (false, false), "spatial" => (true, false), "temporal" => (false, true), "both" => (true, true),
            _ => bail!("unknown --aq {a} (none|spatial|temporal|both)"),
        };
    }
    hw.ten_bit = args.iter().any(|a| a == "--10bit");
    let explicit_q = arg(args, "--qp").is_some();
    let with_quality = |codec: Codec, mut hw: HwOpts| { if !explicit_q { hw.qp = codec.default_quality(hw.ten_bit); } (codec, hw) };
    let codec = match want.as_str() {
        "x264" => return Ok((Codec::X264, hw)),
        "vaapi" => Codec::Vaapi,
        "qsv" => Codec::Qsv,
        "nvenc" => Codec::Nvenc,
        "nvenc-hevc" => Codec::NvencHevc,
        "auto" => {
            if rgba || no_encode { return Ok((Codec::X264, hw)); }
            // --10bit asks for HEVC main10; otherwise auto stays H.264
            let order: &[Codec] = if hw.ten_bit { &[Codec::NvencHevc] } else { &[Codec::Nvenc, Codec::Vaapi] };
            for &c in order {
                match encode::detect(c) {
                    Ok(a) => { hw.access = a; return Ok(with_quality(c, hw)); }
                    Err(e) => eprintln!("agentfx-render: {c:?} unavailable ({e})"),
                }
            }
            eprintln!("agentfx-render: -> using libx264");
            hw.ten_bit = false;
            return Ok((Codec::X264, hw));
        }
        e => bail!("unknown --encoder {e} (auto|x264|nvenc|nvenc-hevc|vaapi|qsv)"),
    };
    if rgba { bail!("--rgba-pipe works only with --encoder x264 (hardware encoders get YUV from reel)"); }
    if hw.ten_bit && codec != Codec::NvencHevc { bail!("--10bit needs --encoder nvenc-hevc"); }
    hw.access = encode::detect(codec).map_err(|e| anyhow::anyhow!("--encoder {want}: {e}"))?;
    Ok(with_quality(codec, hw))
}

/// Non-MP4 deliverables by `--out` extension (`export.rs`): `--alpha` (transparent
/// background), `--fps N` (.webm/.gif/.webp), `--width N`, `--loop forever|once|N`
/// (.gif/.webp), `--quality N` (VP9 crf, WebP quality), `--webp-lossless`.
fn export_opts(args: &[String], out: &Path) -> Result<Option<export::ExportOpts>> {
    use export::{Format, Loop};
    let has = |f: &str| args.iter().any(|a| a == f);
    let alpha = has("--alpha");
    let Some(format) = Format::from_out(out)? else {
        if alpha {
            bail!("--alpha needs a .mov (ProRes 4444), .webm (VP9), .png, .gif or .webp output (H.264 MP4 has no alpha)");
        }
        for f in ["--fps", "--width", "--loop", "--quality", "--webp-lossless"] {
            if has(f) {
                bail!("{f} applies to .mov/.webm/.png/.gif/.webp output");
            }
        }
        return Ok(None);
    };
    let num = |f: &str| arg(args, f).map(|v| v.parse::<u32>().with_context(|| format!("{f} {v}"))).transpose();
    let fps = num("--fps")?;
    if fps.is_some() && !format.staged() {
        bail!("--fps works with .webm/.gif/.webp (ProRes and PNG keep the scene rate)");
    }
    if fps == Some(0) {
        bail!("--fps must be positive");
    }
    let width = num("--width")?;
    if width.is_some_and(|w| w < 2) {
        bail!("--width must be at least 2");
    }
    if has("--loop") && !matches!(format, Format::Gif | Format::Webp) {
        bail!("--loop applies to .gif/.webp");
    }
    let quality = num("--quality")?;
    if quality.is_some() && !matches!(format, Format::Vp9 | Format::Webp) {
        bail!("--quality applies to .webm (VP9 crf 0..63) and .webp (0..100)");
    }
    match (format, quality) {
        (Format::Vp9, Some(q)) if q > 63 => bail!("--quality for VP9 is a crf in 0..63"),
        (Format::Webp, Some(q)) if q > 100 => bail!("--quality for WebP is 0..100"),
        _ => {}
    }
    let lossless = has("--webp-lossless");
    if lossless && format != Format::Webp {
        bail!("--webp-lossless applies to .webp");
    }
    for f in ["--encoder", "--qp", "--rc", "--10bit", "--rgba-pipe", "--crf", "--preset"] {
        if has(f) {
            bail!("{f} applies to .mp4 output only");
        }
    }
    let looping = arg(args, "--loop").map(|l| Loop::parse(&l)).transpose()?.unwrap_or(Loop::Forever);
    Ok(Some(export::ExportOpts { format, alpha, fps, width, looping, quality, lossless }))
}

/// `--hide a,b`: mark the named nodes hidden (e.g. a backdrop for `--alpha`).
fn hide_nodes(scene: &mut agentfx_ir::Scene, names: &str) -> Result<()> {
    for name in names.split(',').filter(|n| !n.is_empty()) {
        let mut found = false;
        for n in scene.nodes.iter_mut().filter(|n| n.name == name) {
            n.hidden = true;
            found = true;
        }
        if !found {
            bail!("--hide: no node named {name:?}");
        }
    }
    Ok(())
}

/// A visible, opaque, full-frame rect under the root (the usual backdrop).
fn opaque_backdrop(scene: &agentfx_ir::Scene) -> Option<String> {
    let (w, h) = (scene.width as f64, scene.height as f64);
    scene.nodes.first()?.children.iter().map(|&c| &scene.nodes[c]).find(|n| {
        !n.hidden && n.opacity >= 1.0 && matches!(&n.content, Some(agentfx_ir::Content::Rect { rect, color })
            if color[3] >= 1.0 && rect.rect[0] <= 0.0 && rect.rect[1] <= 0.0 && rect.rect[0] + rect.rect[2] >= w && rect.rect[1] + rect.rect[3] >= h)
    }).map(|n| n.name.clone())
}

fn render_opts(args: &[String]) -> Result<render::RenderOpts> {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut segments: usize = arg(args, "--segments").map(|s| s.parse()).transpose()?.unwrap_or(1);
    let out = PathBuf::from(arg(args, "--out").context("--out required")?);
    let export = export_opts(args, &out)?;
    // exports run their own ffmpeg (no encoder probe)
    let (codec, hw) = if export.is_some() { (encode::Codec::X264, encode::HwOpts::default()) } else { encoder_opts(args)? };
    if codec.is_nvenc() {
        // one NVENC session per segment; the driver caps concurrent sessions
        let cap = encode::nvenc_session_cap();
        if segments > cap {
            eprintln!("agentfx-render: NVENC allows {cap} concurrent sessions -> {cap} segments (was {segments}; AGENTFX_NVENC_SESSIONS overrides)");
            segments = cap;
        }
    }
    Ok(render::RenderOpts {
        out,
        mode: match arg(args, "--mode").as_deref().unwrap_or("vfr") {
            "cfr" => encode::Mode::Cfr,
            "vfr" => encode::Mode::Vfr,
            m => bail!("unknown mode {m}"),
        },
        backend: match arg(args, "--backend").as_deref().unwrap_or("vello") {
            "vello" => render::BackendKind::Vello,
            "tiny-skia" => render::BackendKind::TinySkia,
            b => bail!("unknown backend {b}"),
        },
        filter: match arg(args, "--filter").as_deref().unwrap_or("bilinear") {
            "bilinear" => composite::Filter::Bilinear,
            "bicubic" => composite::Filter::Bicubic,
            f => bail!("unknown filter {f}"),
        },
        mips: !args.iter().any(|a| a == "--no-mips"),
        lossless: arg(args, "--lossless").map(PathBuf::from),
        segments,
        threads: arg(args, "--threads").map(|s| s.parse()).transpose()?.unwrap_or(0),
        preset: arg(args, "--preset").unwrap_or_else(|| "veryfast".into()),
        crf: arg(args, "--crf").map(|s| s.parse()).transpose()?.unwrap_or(18),
        x264_threads: arg(args, "--x264-threads").map(|s| s.parse()).transpose()?.unwrap_or(if segments > 1 { (cores / segments).max(1) as u32 } else { 0 }),
        yuv: !args.iter().any(|a| a == "--rgba-pipe"),
        codec,
        hw,
        no_encode: args.iter().any(|a| a == "--no-encode"),
        export,
    })
}

/// HTML frontend: the M2 probe planner (default) or the M1 layer planner (`--planner m1`).
fn frontend(args: &[String], scene_dir: &Path, out_dir: &Path) -> Result<(agentfx_ir::Scene, serde_json::Value, record::Resources)> {
    if arg(args, "--planner").as_deref() == Some("m1") {
        let (o, s, r) = html_frontend_res(scene_dir, out_dir)?;
        return Ok((o.scene, s, r));
    }
    let (p, r) = plan2::build(scene_dir, out_dir)?;
    Ok((p.scene, p.stats, r))
}

fn html_frontend_res(scene_dir: &Path, out_dir: &Path) -> Result<(html::HtmlOut, serde_json::Value, record::Resources)> {
    let t0 = Instant::now();
    let compiled = css::compile_scene(scene_dir)
        .map_err(|errs| anyhow::anyhow!("unsupported CSS in {}:\n  - {}", scene_dir.display(), errs.join("\n  - ")))?;
    let t_css = t0.elapsed().as_secs_f64();
    let mut bz = blitzfe::BlitzScene::load(scene_dir, &compiled)?;
    let mut res = record::Resources::new(out_dir);
    bz.image_sources(&mut res);
    let t1 = Instant::now();
    let geom = bz.geom(&compiled, &mut res)?;
    let t_geom = t1.elapsed().as_secs_f64();
    std::fs::write(out_dir.join("geom.json"), serde_json::to_string(&geom)?)?;
    let t2 = Instant::now();
    let mut out = html::build(&compiled, &geom)?;
    let t_plan = t2.elapsed().as_secs_f64();
    let paint = bz.paint_units(&compiled, &geom, &mut out, &mut res)?;
    let mut stats = out.stats.clone();
    stats["css_s"] = serde_json::json!(t_css);
    stats["geom_s"] = serde_json::json!(t_geom);
    stats["plan_s"] = serde_json::json!(t_plan);
    stats["paint"] = paint;
    stats["blitz"] = serde_json::json!(bz.timings);
    stats["frontend_s"] = serde_json::json!(t0.elapsed().as_secs_f64());
    Ok((out, stats, res))
}

/// CPU time (user + sys) of this process, all threads (children excluded).
fn self_cpu_s() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let f: Vec<&str> = s.rsplit(')').next().unwrap_or("").split_whitespace().collect();
    let tick = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (tick(11) + tick(12)) / 100.0
}

const USAGE: &str = "agentfx-render: AgentFX IR / HTML scene -> video (the AgentFX renderer)

USAGE:
  agentfx-render run    <html_scene_dir> --out x.mp4 [render flags] [--write-ir]   HTML mode, frontend in-process
  agentfx-render html   <html_scene_dir> --out-dir out/x      HTML frontend only: writes scene.ir.json
  agentfx-render render <scene.ir.json> --out x.mp4 [--mode cfr|vfr] [--lossless x.mkv]
                     [--filter bilinear|bicubic] [--no-mips] [--stats render.json]
                     [--encoder auto|x264|nvenc|nvenc-hevc|vaapi|qsv] [--qp N] [--rc cqp|icq|qvbr]
                     [--low-power 0|1] [--compression-level N] [--segments N]
                     [--nv-preset 1..7] [--aq none|spatial|temporal|both] [--10bit]
                     [--gpu[=on|off|auto]]  GPU compute for heavy effects (AGENTFX_GPU, default off)
                     [--hide name,name]  hide nodes by name (e.g. the backdrop, for --alpha)
                     [--jobs-dir DIR]  write a job status JSON there (or AGENTFX_JOBS_DIR; off by default)
         --out x.mov|x.webm|x.png|x.gif|x.webp  ProRes / VP9 / PNG sequence / GIF / animated WebP:
                     [--alpha] transparent background (ProRes 4444, VP9 yuva420p, PNG RGBA,
                     GIF 1-bit, WebP)  [--width N] [--fps N] (.webm/.gif/.webp)
                     [--loop forever|once|N] (.gif/.webp)  [--quality N] (VP9 crf / WebP 0..100)
                     [--webp-lossless]
  agentfx-render stats  <scene.ir.json> [--out stats.json]   schedule/damage stats
  agentfx-render snap   <input> --out-dir d --frames a,b,c
  agentfx-render --version | --help

ENVIRONMENT:
  AGENTFX_FFMPEG, AGENTFX_FFPROBE  ffmpeg / ffprobe executables (default: found on PATH)
  AGENTFX_FFMPEG_HW             ffmpeg for hardware encoders (default: AGENTFX_FFMPEG, then each ffmpeg on PATH)
  AGENTFX_VAAPI_DEVICE          DRM render node for vaapi/qsv (default /dev/dri/renderD128)
  AGENTFX_JOBS_DIR              job status directory (same as --jobs-dir)
  (every AGENTFX_* variable also accepts its older REEL_* name; full list: docs/BUILD.md)

Docs: docs/INPUT.md (HTML input) and docs/IR.md (IR format) in the source tree";

fn main() -> Result<()> {
    let r = run();
    // job status file for the render dashboard (no-op unless a render started one)
    let err = r.as_ref().err().map(|e| format!("{e:#}"));
    jobstatus::finish(err.as_deref().map_or(Ok(()), Err));
    r
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    gpu::init_from_args(&args);
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    match cmd {
        "--version" | "-V" | "version" => {
            println!("agentfx-render {} ({})", env!("CARGO_PKG_VERSION"), env!("AGENTFX_GIT_HASH"));
            return Ok(());
        }
        "" | "--help" | "-h" | "help" => {
            println!("{USAGE}");
            return Ok(());
        }
        _ => {}
    }
    let input = PathBuf::from(args.get(2).context("missing input")?);
    match cmd {
        "html" => {
            let t0 = Instant::now();
            let out_dir = PathBuf::from(arg(&args, "--out-dir").context("--out-dir required")?);
            std::fs::create_dir_all(&out_dir)?;
            let (scene, stats, _) = frontend(&args, &input, &out_dir)?;
            std::fs::write(out_dir.join("scene.ir.json"), serde_json::to_string(&scene)?)?;
            let mut stats = stats;
            stats["compile_ms"] = serde_json::json!((t0.elapsed().as_secs_f64() * 1e4).round() / 10.0);
            std::fs::write(out_dir.join("html-stats.json"), serde_json::to_string_pretty(&stats)?)?;
            println!("{}", serde_json::to_string(&stats)?);
            Ok(())
        }
        "stats" => {
            let scene = load_ir(&input)?;
            let base = input.parent().unwrap_or(Path::new("."));
            let prep = vraster::Prepared::new(&scene, base, None, None)?;
            let info = vraster::Info::new(&scene, &prep);
            let t0 = Instant::now();
            let s = schedule::build(&scene, &info)?;
            let mut st = s.stats(&scene);
            st["schedule_ms"] = serde_json::json!((t0.elapsed().as_secs_f64() * 1e4).round() / 10.0);
            st["segments_6"] = serde_json::json!(render::plan_segments(&scene, &s, 6));
            if let Some(o) = arg(&args, "--out") {
                std::fs::write(o, serde_json::to_string_pretty(&st)?)?;
            }
            st.as_object_mut().unwrap().remove("damage_area_per_frame");
            st.as_object_mut().unwrap().remove("damage_rects_per_frame");
            println!("{}", serde_json::to_string_pretty(&st)?);
            Ok(())
        }
        "render" | "run" => {
            let t0 = Instant::now();
            let opts = render_opts(&args)?;
            jobstatus::start(jobstatus::dir(arg(&args, "--jobs-dir")), &args, &input, &opts.out, &format!("{:?}", opts.codec), opts.segments);
            let mut timings = serde_json::Map::new();
            // run: HTML scene dir -> frontend in-process (no IR file round trip)
            let (scene, base, mem) = if cmd == "run" {
                let out_dir = opts.out.parent().unwrap_or(Path::new(".")).to_path_buf();
                std::fs::create_dir_all(&out_dir)?;
                let (scene, fe, res) = frontend(&args, &input, &out_dir)?;
                timings.insert("frontend".into(), fe);
                if args.iter().any(|a| a == "--write-ir") {
                    std::fs::write(out_dir.join("scene.ir.json"), serde_json::to_string(&scene)?)?;
                }
                (scene, out_dir, Some(res))
            } else {
                (load_ir(&input)?, input.parent().unwrap_or(Path::new(".")).to_path_buf(), None)
            };
            let mut scene = scene;
            if let Some(names) = arg(&args, "--hide") {
                hide_nodes(&mut scene, &names)?;
            }
            if opts.export.as_ref().is_some_and(|x| x.alpha) {
                if let Some(n) = opaque_backdrop(&scene) {
                    eprintln!("agentfx-render: --alpha: node {n:?} covers the frame opaquely, the output will not be transparent (--hide {n:?} drops it)");
                }
            }
            let t1 = Instant::now();
            let prep = vraster::Prepared::new(&scene, &base, mem.as_ref().map(|r| r.font_data.as_slice()), mem.as_ref().map(|r| r.image_data.as_slice()))?;
            let t_prep = t1.elapsed().as_secs_f64();
            let info = vraster::Info::new(&scene, &prep);
            let t2 = Instant::now();
            let sched = schedule::build(&scene, &info)?;
            let t_sched = t2.elapsed().as_secs_f64();
            let mut st = render::render(&scene, &base, &prep, &info, &sched, &opts)?;
            st["prepare_s"] = serde_json::json!(t_prep);
            st["schedule_s"] = serde_json::json!(t_sched);
            for (k, v) in timings {
                st[k] = v;
            }
            st["wall_s"] = serde_json::json!(t0.elapsed().as_secs_f64());
            st["self_cpu_s"] = serde_json::json!(self_cpu_s());
            if let Some(o) = arg(&args, "--stats") {
                std::fs::write(o, serde_json::to_string_pretty(&st)?)?;
            }
            println!("{}", serde_json::to_string(&st)?);
            Ok(())
        }
        "snap" => {
            let out_dir = PathBuf::from(arg(&args, "--out-dir").context("--out-dir required")?);
            std::fs::create_dir_all(&out_dir)?;
            let frames: Vec<usize> = arg(&args, "--frames").context("--frames a,b,c")?.split(',').map(|s| s.parse()).collect::<std::result::Result<_, _>>()?;
            let st = m2::snap(&input, &out_dir, &frames)?;
            println!("{}", serde_json::to_string(&st)?);
            Ok(())
        }
        _ => bail!("unknown command {cmd:?}\n\n{USAGE}"),
    }
}
