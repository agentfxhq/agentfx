use agentfx_script::ast::Document;
use agentfx_script::diag::{Diag, Severity};
use agentfx_script::{html, json, lint, lower, parse, print, run, timing};
use std::path::{Path, PathBuf};
use std::time::Instant;

const USAGE: &str = "agentfx - motion graphics and video for AI agents (Reel Script compiler, spec v0)

USAGE:
  agentfx parse  <file.reel|file.json> [--to json|text] [-o out]
  agentfx lint   <file> [--format 9:16] [--measure] [--motion] [--tts piper|edge|none] [--no-tts] [--all-formats]
  agentfx energy <file> [--format F] [--bins 100] [--json]   (motion-energy map: share of the frame changing per frame)
  agentfx timing <file> [--tts piper|edge|none] [--no-tts]
  agentfx ir     <file> [--format 9:16] [--tts piper|edge|none] [--no-tts] [--target reel-ir -o x.ir.json]
  agentfx html   <file> [--format 9:16] [-o out.html] [--no-embed] [--tts piper|edge|none] [--no-tts]
  agentfx frame  <file> --at <anchor|scene:anchor|12.5s|12.5> [--format F] [-o frame.png]
  agentfx sheet  <file> [--n 12 | --every 2s | --at a,b,c | --events] [--cols 4] [--energy] [--ref ref.mp4] [--format F] [-o sheet.png]
  agentfx render <file> [--format F] [-o out.mp4] [--segments 1 | --cache] [--fps 30] [--mode cfr|vfr] [--alpha] [--backend reel|chrome]
  (frame/sheet/render use direct mode (native layout -> AgentFX IR -> agentfx-render) by default;
   --backend chrome renders the HTML backend in Chrome, the reference)
  agentfx motion mine <recorder-out-dir>... [--json out.json]   (motion statistics -> motion tokens)
  agentfx design harvest <frame.html>... [-o design.reel] [--name N] [--json stats.json]   (style frames -> design spec)
  agentfx design extract <frame.html>...   (computed styles as JSON)
  agentfx design pitch <brief.md|text> [--n 4] [--lenses a,b] [--render]   (prompt pack + style frames + contact sheet)
  agentfx critique <sheet.png|video.mp4> [--rubric visual|motion|all] [--model haiku|sonnet] [--rounds n]
  agentfx picks add|list ...        (design pick memory, design/picks.jsonl)
  agentfx catalog [--md]
  agentfx schema
  agentfx --version | --help

Renderer: $AGENTFX_RENDERER, else agentfx-render next to this binary, else on PATH.
Data dir: $AGENTFX_HOME, else <exe>/../share/agentfx, else the source tree; caches: $AGENTFX_CACHE,
else the data dir ($AGENTFX_HOME / source tree) or ~/.cache/agentfx; ffmpeg/ffprobe:
$AGENTFX_FFMPEG/$AGENTFX_FFPROBE or PATH. Every AGENTFX_* variable also accepts its old REEL_*
name (docs/BUILD.md).
TTS: --tts piper|edge|none (or $AGENTFX_TTS); default piper (local, voice en_US-ljspeech-medium),
none when piper is missing (estimated timings, silent); --no-tts = --tts none. edge is opt-in.
Diagnostics are JSON on stdout: [{code, severity, message, hint, line, col, id?, at?}]";

/// Motion-energy map of a compiled IR (native layout for the boxes, no paint).
fn energy_map(ir: &agentfx_script::ir::Ir) -> Result<agentfx_script::energy::EnergyMap, String> {
    let tmp = run::tmp_dir();
    let _ = std::fs::create_dir_all(&tmp);
    let nat = agentfx_script::native::Native::load(ir, agentfx_script::native::snap_at(ir, 0.0), &run::home().join("assets/fonts"), &tmp, false).map_err(|e| format!("native layout failed: {e:#}"))?;
    Ok(agentfx_script::energy::compute_native(ir, &nat))
}

/// Taste lints (small motion, dead air, static ending) over an energy map, with the document's `lint` tokens.
fn taste_diags(b: &Built, e: &agentfx_script::energy::EnergyMap) -> Vec<Diag> {
    let mut dd = vec![];
    let th = agentfx_script::theme::Theme::from_doc(&b.doc, &mut dd);
    agentfx_script::taste::rules(e, &b.tl, &th.taste)
}

struct Args {
    cmd: String,
    file: Option<PathBuf>,
    opts: Vec<(String, String)>,
    flags: Vec<String>,
}
impl Args {
    fn get(&self, k: &str) -> Option<String> {
        self.opts.iter().rev().find(|(a, _)| a == k).map(|(_, v)| v.clone())
    }
    fn has(&self, k: &str) -> bool {
        self.flags.iter().any(|f| f == k)
    }
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_default();
    let mut a = Args { cmd, file: None, opts: vec![], flags: vec![] };
    let valued = ["--tts", "--mode","--backend", "--segments", "--compat", "--target", "--to", "-o", "--format", "--at", "--n", "--every", "--cols", "--ref", "--jobs", "--fps", "--scale"];
    let rest: Vec<String> = it.collect();
    let mut i = 0;
    while i < rest.len() {
        let s = &rest[i];
        if valued.contains(&s.as_str()) && i + 1 < rest.len() {
            a.opts.push((s.clone(), rest[i + 1].clone()));
            i += 2;
            continue;
        }
        if s.starts_with("--") {
            a.flags.push(s.clone());
        } else if a.file.is_none() {
            a.file = Some(PathBuf::from(s));
        }
        i += 1;
    }
    a
}

fn load(path: &Path) -> (Document, Vec<Diag>) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            std::process::exit(2);
        }
    };
    let is_json = path.extension().and_then(|e| e.to_str()) == Some("json") || src.trim_start().starts_with('{');
    if is_json {
        match json::from_json(&src) {
            Ok(d) => (d, vec![]),
            Err(e) => (Document::default(), vec![Diag::error(&e.code, e.message, e.hint, e.span)]),
        }
    } else {
        let (d, errs) = parse::parse(&src);
        (d, errs.into_iter().map(|e| Diag::error(&e.code, e.message, e.hint, e.span)).collect())
    }
}

fn emit_diags(d: &[Diag]) {
    println!("{}", serde_json::to_string_pretty(d).unwrap());
}

struct Built {
    doc: Document,
    tl: timing::Timeline,
    diags: Vec<Diag>,
    base: PathBuf,
}

fn build(a: &Args) -> Built {
    let file = a.file.clone().unwrap_or_else(|| {
        eprintln!("{USAGE}");
        std::process::exit(2)
    });
    let (doc, mut diags) = load(&file);
    let engine = run::tts_engine(a.get("--tts").as_deref(), a.has("--no-tts")).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let tts = run::tts(&doc, &engine, &mut diags);
    let fps = a.get("--fps").and_then(|f| f.parse().ok());
    let base = file.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let base = if base.as_os_str().is_empty() { PathBuf::from(".") } else { base };
    let base = std::fs::canonicalize(&base).unwrap_or(base);
    // transcript sources (`captions src=`) resolve against the document while timing
    agentfx_script::design::set_doc_base(&base);
    let (tl, td) = timing::solve(&doc, &tts, fps);
    diags.extend(td);
    Built { doc, tl, diags, base }
}

fn compile_html(b: &Built, fmt: Option<&str>, embed: bool) -> (agentfx_script::ir::Ir, String, Vec<Diag>) {
    let c = lower::compile(&b.doc, &b.tl, fmt, &b.base);
    let opts = html::HtmlOpts { embed, fonts_dir: run::home().join("assets/fonts"), snap: None };
    let h = html::emit(&c.ir, &opts);
    (c.ir, h, c.diags)
}

fn out_path(a: &Args, dflt: &str) -> PathBuf {
    a.get("-o").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(dflt))
}

fn write_tmp_html(h: &str, tag: &str) -> PathBuf {
    let dir = run::tmp_dir();
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(format!("{tag}-{}.html", std::process::id()));
    std::fs::write(&p, h).unwrap();
    p
}

/// Resolve `--at`: a time (`12.5s`/`12.5`), an anchor name (first scene that has it) or `scene:anchor`.
fn resolve_at(tl: &timing::Timeline, s: &str) -> Result<f64, String> {
    if let Ok(v) = s.trim_end_matches('s').parse::<f64>() {
        return Ok(v);
    }
    let (scene, name) = match s.split_once(':') {
        Some((a, _)) if a == "beat" || a == "bar" => (None, s),
        Some((a, b)) => (Some(a), b),
        None => (None, s),
    };
    for (i, sc) in tl.scenes.iter().enumerate() {
        if let Some(sn) = scene {
            if sc.id != sn {
                continue;
            }
            if name.is_empty() || name == "start" {
                return Ok(sc.start + 0.01);
            }
        }
        if let Ok(t) = tl.anchor_time(i, None, name, sc.start, Default::default()) {
            return Ok(t);
        }
    }
    if let Some(sc) = tl.scenes.iter().find(|sc| sc.id == s) {
        return Ok(sc.start + sc.dur * 0.5);
    }
    let names: Vec<String> = tl.scenes.iter().flat_map(|s| s.anchors.iter().map(move |a| format!("{}:{}", s.id, a.name))).collect();
    Err(format!("unknown --at `{s}`; anchors: {}", names.join(" ")))
}

/// Subcommands implemented as Python tools (tools/*.py): `agentfx motion mine`, `agentfx critique`, `agentfx picks`.
fn py_tool(script: &str, rest: &[String]) -> ! {
    let py = run::python();
    let st = std::process::Command::new(py).arg(run::home().join("tools").join(script)).args(rest).status();
    std::process::exit(st.map(|s| s.code().unwrap_or(1)).unwrap_or(2))
}

fn main() {
    // before any thread or layout exists (it may set an env var read once by Parley)
    agentfx_script::native::layout_mode();
    let raw: Vec<String> = std::env::args().collect();
    match raw.get(1).map(|s| s.as_str()) {
        Some("--version" | "-V" | "version") => {
            println!("agentfx {} ({}), renderer: {}", env!("CARGO_PKG_VERSION"), env!("AGENTFX_GIT_HASH"), agentfx_script::direct::renderer().display());
            return;
        }
        None | Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            return;
        }
        _ => {}
    }
    match (raw.get(1).map(|s| s.as_str()), raw.get(2).map(|s| s.as_str())) {
        (Some("motion"), Some("mine")) => py_tool("motion_mine.py", &raw[3..]),
        (Some("critique"), _) => py_tool("critique.py", &raw[2..]),
        (Some("picks"), _) => py_tool("picks.py", &raw[2..]),
        (Some("design"), Some("pitch")) => py_tool("pitch.py", &raw[3..]),
        (Some("design"), Some(sub @ ("extract" | "harvest"))) => {
            // computed styles of style frames (Blitz/Stylo), then the aggregation (tools/harvest.py)
            let mut files = vec![];
            let (mut w, mut h) = (1920u32, 1080u32);
            let mut rest: Vec<String> = vec![];
            let mut i = 3;
            while i < raw.len() {
                match raw[i].as_str() {
                    "--w" if i + 1 < raw.len() => { w = raw[i + 1].parse().unwrap_or(1920); i += 1; }
                    "--h" if i + 1 < raw.len() => { h = raw[i + 1].parse().unwrap_or(1080); i += 1; }
                    a if a.ends_with(".html") || a.ends_with(".htm") => files.push(PathBuf::from(a)),
                    a => rest.push(a.to_string()),
                }
                i += 1;
            }
            let mut frames = vec![];
            for f in &files {
                match agentfx_script::harvest::extract(f, w, h, &run::home().join("assets/fonts")) {
                    Ok(v) => frames.push(v),
                    Err(e) => { eprintln!("{}: {e:#}", f.display()); std::process::exit(1) }
                }
            }
            let all = serde_json::json!({"frames": frames});
            if sub == "extract" {
                println!("{}", serde_json::to_string(&all).unwrap());
                return;
            }
            let tmp = run::tmp_dir().join(format!("harvest-{}.json", std::process::id()));
            let _ = std::fs::create_dir_all(tmp.parent().unwrap());
            std::fs::write(&tmp, serde_json::to_string(&all).unwrap()).unwrap();
            let mut args = vec![tmp.to_string_lossy().to_string()];
            args.extend(rest);
            let py = run::python();
            let st = std::process::Command::new(py).arg(run::home().join("tools/harvest.py")).args(&args).status();
            let _ = std::fs::remove_file(&tmp);
            std::process::exit(st.map(|s| s.code().unwrap_or(1)).unwrap_or(2));
        }
        _ => {}
    }
    let a = parse_args();
    match a.cmd.as_str() {
        "parse" => {
            let file = a.file.clone().unwrap_or_else(|| {
                eprintln!("{USAGE}");
                std::process::exit(2)
            });
            let (doc, diags) = load(&file);
            if !diags.is_empty() {
                eprintln!("{}", serde_json::to_string_pretty(&diags).unwrap());
            }
            let to = a.get("--to").unwrap_or_else(|| if file.extension().and_then(|e| e.to_str()) == Some("json") { "text".into() } else { "json".into() });
            let out = if to == "json" { json::to_json(&doc) } else { print::print(&doc) };
            match a.get("-o") {
                Some(p) => std::fs::write(p, out).unwrap(),
                None => print!("{out}"),
            }
            std::process::exit(if diags.iter().any(|d| d.severity == Severity::Error) { 1 } else { 0 });
        }
        "timing" => {
            let b = build(&a);
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({"timeline": b.tl, "diagnostics": b.diags})).unwrap());
        }
        "ir" => {
            let b = build(&a);
            if a.get("--target").as_deref() == Some("reel-ir") {
                // S1: native layout + paint, no Chrome
                let c = lower::compile(&b.doc, &b.tl, a.get("--format").as_deref(), &b.base);
                let o = std::fs::canonicalize(".").unwrap().join(out_path(&a, "out/video.ir.json"));
                let dir = o.parent().unwrap().to_path_buf();
                let t0 = Instant::now();
                let r = agentfx_script::direct::compile(&c.ir, &dir, true).unwrap_or_else(|e| { eprintln!("{e:#}"); std::process::exit(1) });
                std::fs::write(&o, serde_json::to_string(&r.scene).unwrap()).unwrap();
                let mut d = c.diags;
                d.extend(r.diags);
                eprintln!("wrote {} ({} nodes, {} tracks, {} assets) in {:.2}s; stats {}", o.display(), r.scene["nodes"].as_array().map(|a| a.len()).unwrap_or(0), r.scene["tracks"].as_array().map(|a| a.len()).unwrap_or(0), r.scene["assets"].as_array().map(|a| a.len()).unwrap_or(0), t0.elapsed().as_secs_f64(), r.stats);
                emit_diags(&d.into_iter().filter(|x| x.code.starts_with("W_IR") || x.code.starts_with("E_IR") || x.severity == Severity::Error).collect::<Vec<_>>());
                return;
            }
            let c = lower::compile(&b.doc, &b.tl, a.get("--format").as_deref(), &b.base);
            println!("{}", serde_json::to_string_pretty(&c.ir).unwrap());
        }
        "lint" => {
            let b = build(&a);
            let formats = lint::formats(&b.doc, a.get("--format"), a.has("--all-formats"));
            let mut all = b.diags.clone();
            for f in &formats {
                let (ir, h, d) = compile_html(&b, Some(f), false);
                let mut fd = d;
                fd.extend(lint::static_rules(&b.doc, &b.tl, &ir));
                if a.has("--measure") && !a.has("--chrome") {
                    // S1: measured natively (Blitz layout + motion IR transforms), no browser
                    let times = lint::sample_times(&b.tl);
                    let tmp = run::tmp_dir();
                    let _ = std::fs::create_dir_all(&tmp);
                    match agentfx_script::native::Native::load(&ir, agentfx_script::native::snap_at(&ir, 0.0), &run::home().join("assets/fonts"), &tmp, false) {
                        Ok(mut nat) => {
                            let m = lint::measure_native(&ir, &mut nat, &times);
                            fd.extend(lint::measured_rules(&m, &ir, &b.tl, &b.doc));
                            fd.extend(taste_diags(&b, &agentfx_script::energy::compute_native(&ir, &nat)));
                        }
                        Err(e) => fd.push(Diag::warn("W_MEASURE", format!("native layout failed: {e:#}"), "report this; `--chrome` measures in Chrome", Default::default())),
                    }
                    let _ = &h;
                } else if a.has("--measure") {
                    let p = write_tmp_html(&h, "lint");
                    let times = lint::sample_times(&b.tl);
                    match run::node_tool(&serde_json::json!({"cmd": "measure", "html": p, "w": ir.w, "h": ir.h, "times": times})) {
                        Ok(m) => fd.extend(lint::measured_rules(&m, &ir, &b.tl, &b.doc)),
                        Err(e) => fd.push(Diag::warn("W_MEASURE", format!("layout measurement failed: {e}"), "is Chrome available? (tools/capture.mjs)", Default::default())),
                    }
                    let _ = std::fs::remove_file(p);
                }
                if a.has("--motion") && !(a.has("--measure") && !a.has("--chrome")) {
                    // taste lints need the layout boxes (native layout, no paint), not the measured samples
                    match energy_map(&ir) {
                        Ok(e) => fd.extend(taste_diags(&b, &e)),
                        Err(e) => fd.push(Diag::warn("W_MEASURE", format!("motion energy failed: {e}"), "report this", Default::default())),
                    }
                }
                if formats.len() > 1 {
                    for d in fd.iter_mut() {
                        if d.format.is_none() {
                            d.format = Some(f.clone());
                        }
                    }
                }
                all.extend(fd);
            }
            let all = lint::dedup(all);
            emit_diags(&all);
            let n = |s: Severity| all.iter().filter(|d| d.severity == s).count();
            eprintln!("lint: {} errors, {} warnings, {} info ({}{})", n(Severity::Error), n(Severity::Warning), n(Severity::Info), formats.join(", "), if a.has("--measure") { ", measured" } else { "" });
            std::process::exit(if all.iter().any(|d| d.severity == Severity::Error) { 1 } else { 0 });
        }
        "energy" => {
            let b = build(&a);
            let (ir, _h, _d) = compile_html(&b, a.get("--format").as_deref(), false);
            let e = energy_map(&ir).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            });
            if a.has("--json") {
                println!("{}", serde_json::to_string(&e.to_json()).unwrap());
            } else {
                let bins: usize = a.get("--bins").and_then(|s| s.parse().ok()).unwrap_or(100);
                println!("{}", e.strip(bins));
                let peak = e.frames.iter().copied().fold(0.0f32, f32::max);
                let mean = e.frames.iter().sum::<f32>() / e.frames.len().max(1) as f32;
                println!("{:.1}s, {} frames, mean {:.1} % peak {:.1} % of frame; still (<0.01 %) {:.1}s", e.duration(), e.len(), mean * 100.0, peak * 100.0, e.frames.iter().filter(|x| **x <= 1e-4).count() as f64 / e.fps);
            }
        }
        "html" => {
            let b = build(&a);
            let t0 = Instant::now();
            let (ir, h, d) = compile_html(&b, a.get("--format").as_deref(), !a.has("--no-embed"));
            let o = out_path(&a, "out/video.html");
            if let Some(dir) = o.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&o, &h).unwrap();
            let mut diags = b.diags.clone();
            diags.extend(d);
            eprintln!("wrote {} ({} KB, {}x{}, {:.1}s, {} tracks) in {:.0} ms", o.display(), h.len() / 1024, ir.w, ir.h, ir.duration, ir.tracks.len(), t0.elapsed().as_secs_f64() * 1000.0);
            if diags.iter().any(|d| d.severity == Severity::Error) {
                emit_diags(&diags);
                eprintln!("compile errors: the HTML was written for inspection, exit status 1");
                std::process::exit(1);
            }
        }
        "frame" | "sheet" | "render" => {
            let chrome = a.get("--backend").as_deref() == Some("chrome");
            if a.has("--mode") || a.get("--mode").is_some_and(|m| !matches!(m.as_str(), "cfr" | "vfr")) {
                eprintln!("--mode requires cfr or vfr");
                std::process::exit(2);
            }
            if chrome && (a.get("--mode").is_some() || a.has("--alpha")) {
                eprintln!("--mode and --alpha require the direct backend (--backend reel)");
                std::process::exit(2);
            }
            let b = build(&a);
            let (ir, h, d) = compile_html(&b, a.get("--format").as_deref(), false);
            let mut diags = b.diags.clone();
            diags.extend(d);
            if diags.iter().any(|d| d.severity == Severity::Error) {
                eprintln!("{}", serde_json::to_string_pretty(&diags).unwrap());
                eprintln!("compile errors: fix them first (`agentfx lint`)");
                std::process::exit(1);
            }
            // times for frame / sheet
            let times: Vec<f64> = match a.cmd.as_str() {
                "frame" => {
                    let at = a.get("--at").unwrap_or("0".into());
                    vec![resolve_at(&b.tl, &at).unwrap_or_else(|e| {
                        eprintln!("{e}");
                        std::process::exit(2)
                    })]
                }
                "sheet" => {
                    if a.has("--events") {
                        // one frame 0.6 s after every narration anchor (what the agent choreographed), plus scene ends
                        let mut v: Vec<f64> = b.tl.scenes.iter().flat_map(|s| s.anchors.iter().map(|x| x.start + 0.6).chain(std::iter::once(s.start + s.dur - 0.2))).collect();
                        v.sort_by(|x, y| x.partial_cmp(y).unwrap());
                        v.dedup_by(|x, y| (*x - *y).abs() < 0.25);
                        v
                    } else if let Some(at) = a.get("--at") {
                        at.split(',').filter_map(|s| resolve_at(&b.tl, s.trim()).ok()).collect()
                    } else if let Some(ev) = a.get("--every") {
                        let step: f64 = ev.trim_end_matches('s').parse().unwrap_or(2.0);
                        if !(step.is_finite() && step >= 0.05) {
                            eprintln!("--every must be a duration of at least 0.05s");
                            std::process::exit(2);
                        }
                        let mut v = vec![];
                        let mut t = step / 2.0;
                        while t < ir.duration {
                            v.push(t);
                            t += step;
                        }
                        v
                    } else {
                        let n: usize = a.get("--n").and_then(|s| s.parse().ok()).unwrap_or(12);
                        (0..n).map(|k| (k as f64 + 0.5) / n as f64 * ir.duration).collect()
                    }
                }
                _ => vec![],
            };
            let labels: Vec<String> = times.iter().map(|t| {
                let si = b.tl.scene_at(*t);
                format!("{:.2}s · {}", t, b.tl.scenes.get(si).map(|s| s.id.as_str()).unwrap_or(""))
            }).collect();
            let jobs: u32 = a.get("--jobs").and_then(|j| j.parse().ok()).unwrap_or(4);
            if !chrome {
                // S1 default: direct mode (native layout -> reel-ir -> reel), no browser
                let t0 = Instant::now();
                let default_out = match a.cmd.as_str() { "frame" => "out/frame.png", "sheet" => "out/sheet.png", _ => "out/video.mp4" };
                let o = std::fs::canonicalize(".").unwrap().join(out_path(&a, default_out));
                let dir = o.parent().unwrap().to_path_buf();
                let _ = std::fs::create_dir_all(&dir);
                let work = dir.join(format!(".reel-{}-{}", a.cmd, std::process::id()));
                let c = agentfx_script::direct::compile(&ir, &work, true).unwrap_or_else(|e| { eprintln!("{e:#}"); std::process::exit(1) });
                if c.diags.iter().any(|d| d.severity == Severity::Error) {
                    eprintln!("{}", serde_json::to_string_pretty(&c.diags).unwrap());
                    std::process::exit(1);
                }
                let t_compile = t0.elapsed().as_secs_f64();
                let res: Result<serde_json::Value, String> = match a.cmd.as_str() {
                    "render" => {
                        let segs: usize = a.get("--segments").and_then(|s| s.parse().ok()).unwrap_or(1);
                        let mode = a.get("--mode").unwrap_or("vfr".into());
                        let alpha = a.has("--alpha");
                        // The segment cache stores VFR opaque MP4. Other exports bypass it.
                        let cache_compatible = mode == "vfr" && !alpha && o.extension().is_some_and(|e| e == "mp4");
                        let r = if a.has("--cache") && cache_compatible {
                            agentfx_script::direct::render_cached(&ir, &c.scene, &o, &run::cache_dir("segments")).map_err(|e| format!("{e:#}"))
                        } else {
                            agentfx_script::direct::render_with_options(&ir, &c.scene, &o, segs, true, &mode, alpha).map_err(|e| format!("{e:#}"))
                        };
                        r.map(|mut v| {
                            v["compile_s"] = serde_json::json!(t_compile);
                            v["compile"] = c.stats.clone();
                            v["file"] = serde_json::json!(o);
                            v["total_s"] = serde_json::json!(t0.elapsed().as_secs_f64());
                            v
                        })
                    }
                    _ => {
                        let fps = ir.fps;
                        let fr: Vec<u32> = times.iter().map(|t| ((t * fps).round() as u32).min(((ir.duration * fps).round() as u32).saturating_sub(1))).collect();
                        match agentfx_script::direct::frames(&c.scene, &fr, &work, jobs as usize) {
                            Err(e) => Err(format!("{e:#}")),
                            Ok(pngs) if a.cmd == "frame" => std::fs::rename(&pngs[0], &o).map(|_| serde_json::json!({"file": o, "t": times[0], "s": t0.elapsed().as_secs_f64()})).map_err(|e| e.to_string()),
                            Ok(pngs) => {
                                // optional reference frames (e.g. the Chrome render) for a side-by-side sheet
                                let refs: Vec<Option<std::path::PathBuf>> = match a.get("--ref") {
                                    Some(rv) => times.iter().enumerate().map(|(i, t)| {
                                        let p = work.join(format!("ref{i:03}.png"));
                                        let ok = std::process::Command::new(run::ffmpeg()).args(["-v", "error", "-y", "-ss", &format!("{t:.3}"), "-i", &rv, "-frames:v", "1"]).arg(&p).status().map(|s| s.success()).unwrap_or(false);
                                        ok.then_some(p)
                                    }).collect(),
                                    None => vec![None; times.len()],
                                };
                                let cells: Vec<(std::path::PathBuf, Option<std::path::PathBuf>, String)> = pngs.iter().cloned().zip(refs).zip(labels.iter().cloned()).map(|((p, r), l)| (p, r, l)).collect();
                                let cols = a.get("--cols").and_then(|c| c.parse::<usize>().ok()).unwrap_or(4);
                                // --energy: motion-energy bar under every thumbnail (share of the frame moving around that time)
                                let energy: Option<Vec<f32>> = if a.has("--energy") {
                                    match energy_map(&ir) {
                                        Ok(e) => Some(times.iter().map(|t| e.max_in(t - 0.5, t + 0.5)).collect()),
                                        Err(err) => { eprintln!("energy: {err}"); None }
                                    }
                                } else { None };
                                agentfx_script::direct::sheet_with_energy(&cells, energy.as_deref(), ir.w, ir.h, cols, &o).map(|_| serde_json::json!({"file": o, "frames": times.len(), "s": t0.elapsed().as_secs_f64()})).map_err(|e| format!("{e:#}"))
                            }
                        }
                    }
                };
                // keep the work dir only for renders (it holds fonts/images the IR references)
                if a.cmd != "render" {
                    let _ = std::fs::remove_dir_all(&work);
                } else if let Ok(rd) = std::fs::read_dir(&work) {
                    if rd.count() == 0 {
                        let _ = std::fs::remove_dir(&work);
                    }
                }
                match res {
                    Ok(v) => {
                        let warn: Vec<&Diag> = c.diags.iter().filter(|d| d.severity != Severity::Info).collect();
                        if !warn.is_empty() {
                            eprintln!("{}", serde_json::to_string_pretty(&warn).unwrap());
                        }
                        println!("{}", serde_json::to_string_pretty(&v).unwrap())
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1)
                    }
                }
                return;
            }
            let p = write_tmp_html(&h, &a.cmd);
            let res = match a.cmd.as_str() {
                "frame" => {
                    let o = std::fs::canonicalize(".").unwrap().join(out_path(&a, "out/frame.png"));
                    run::node_tool(&serde_json::json!({"cmd": "frames", "html": p, "w": ir.w, "h": ir.h, "times": times, "out": [o]}))
                }
                "sheet" => {
                    let o = std::fs::canonicalize(".").unwrap().join(out_path(&a, "out/sheet.png"));
                    let refv = a.get("--ref").map(|r| std::fs::canonicalize(&r).unwrap_or(PathBuf::from(&r)));
                    run::node_tool(&serde_json::json!({"cmd": "sheet", "html": p, "w": ir.w, "h": ir.h, "times": times, "labels": labels,
                        "cols": a.get("--cols").and_then(|c| c.parse::<u32>().ok()).unwrap_or(4), "out": o, "ref": refv, "ffmpeg": run::ffmpeg()}))
                }
                _ => {
                    let t0 = Instant::now();
                    let o = std::fs::canonicalize(".").unwrap().join(out_path(&a, "out/video.mp4"));
                    let wav = o.with_extension("wav");
                    let has_audio = run::mix_audio(&ir, &wav).unwrap_or_else(|e| {
                        eprintln!("audio: {e}");
                        false
                    });
                    let r = run::node_tool(&serde_json::json!({"cmd": "render", "html": p, "w": ir.w, "h": ir.h, "fps": ir.fps, "duration": ir.duration,
                        "out": o, "jobs": jobs, "audio": if has_audio { Some(&wav) } else { None }, "ffmpeg": run::ffmpeg()}));
                    let _ = std::fs::remove_file(&wav);
                    eprintln!("render total {:.1}s", t0.elapsed().as_secs_f64());
                    r
                }
            };
            let _ = std::fs::remove_file(&p);
            match res {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1)
                }
            }
        }
        "catalog" => {
            if a.has("--md") {
                print!("{}", agentfx_script::catalog::markdown());
            } else {
                println!("{}", serde_json::to_string_pretty(&agentfx_script::catalog::json()).unwrap());
            }
        }
        "schema" => println!("{}", serde_json::to_string_pretty(&agentfx_script::catalog::schema()).unwrap()),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}
