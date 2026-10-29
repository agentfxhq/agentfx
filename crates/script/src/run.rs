//! Glue to external tools: TTS (tools/tts.py: piper by default, edge-tts opt-in), Chrome (tools/capture.mjs), ffmpeg.
use crate::ast::Document;
use crate::diag::Diag;
use crate::timing::{estimate, tts_requests, TtsResult, TtsWord};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the data directory came from (decides where caches and scratch files go).
#[derive(Clone, Copy, PartialEq, Debug)]
enum HomeKind {
    /// `$AGENTFX_HOME`
    Env,
    /// `<exe dir>/../share/agentfx`
    Installed,
    /// the source tree (`AGENTFX_ROOT`, set by build.rs), the developer fallback
    BuildTree,
}

fn resolve_home() -> (PathBuf, HomeKind) {
    if let Some(h) = agentfx_ir::env::var_os("AGENTFX_HOME").filter(|h| !h.is_empty()) {
        return (PathBuf::from(h), HomeKind::Env);
    }
    let share = std::env::current_exe()
        .ok()
        .and_then(|e| e.canonicalize().ok())
        .and_then(|e| Some(e.parent()?.parent()?.join("share/agentfx")));
    if let Some(share) = share.as_ref().filter(|d| d.join("assets/fonts/fonts.json").is_file()) {
        return (share.clone(), HomeKind::Installed);
    }
    let tree = PathBuf::from(env!("AGENTFX_ROOT"));
    if tree.is_dir() {
        return (tree, HomeKind::BuildTree);
    }
    // nothing found: name the install location in the resulting "not found" errors
    (share.unwrap_or(tree), HomeKind::Installed)
}

fn home_kind() -> &'static (PathBuf, HomeKind) {
    static HOME: std::sync::OnceLock<(PathBuf, HomeKind)> = std::sync::OnceLock::new();
    HOME.get_or_init(resolve_home)
}

/// Read-only data directory (fonts, design lenses and skins, helper scripts):
/// `$AGENTFX_HOME`, else `<exe dir>/../share/agentfx` (if it holds `assets/fonts/fonts.json`),
/// else the build tree. See docs/INSTALL-NOTES.md.
pub fn home() -> PathBuf {
    home_kind().0.clone()
}

/// Writable root for caches (`cache/<sub>`) and scratch files (`out/tmp`): `$AGENTFX_CACHE`,
/// else the data directory when it is `$AGENTFX_HOME` or the build tree (the layout the build
/// tree has always used), else `$XDG_CACHE_HOME/agentfx` or `~/.cache/agentfx`.
pub fn work_root() -> PathBuf {
    if let Some(c) = agentfx_ir::env::var_os("AGENTFX_CACHE").filter(|c| !c.is_empty()) {
        return PathBuf::from(c);
    }
    let (home, kind) = home_kind();
    if *kind != HomeKind::Installed {
        return home.clone();
    }
    let xdg = std::env::var_os("XDG_CACHE_HOME").filter(|x| !x.is_empty()).map(PathBuf::from);
    xdg.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("agentfx")
}

/// A cache directory (`tts`, `mesh`, `segments`) under [`work_root`].
pub fn cache_dir(name: &str) -> PathBuf {
    work_root().join("cache").join(name)
}

/// Scratch directory for intermediate files under [`work_root`].
pub fn tmp_dir() -> PathBuf {
    work_root().join("out/tmp")
}

/// Python for the helper scripts: `$AGENTFX_PYTHON`, else `<home>/.venv/bin/python`, else `python3`.
pub fn python() -> String {
    if let Ok(p) = agentfx_ir::env::var("AGENTFX_PYTHON") {
        return p;
    }
    let venv = home().join(".venv/bin/python");
    if venv.exists() { venv.to_string_lossy().into() } else { "python3".into() }
}

/// `$AGENTFX_FFMPEG`, else `ffmpeg` on PATH.
pub fn ffmpeg() -> String {
    agentfx_ir::env::var("AGENTFX_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}
/// `$AGENTFX_FFPROBE`, else `ffprobe` on PATH.
pub fn ffprobe() -> String {
    agentfx_ir::env::var("AGENTFX_FFPROBE").unwrap_or_else(|_| "ffprobe".into())
}

/// TTS engines `tools/tts.py` knows, plus `none` (estimated timings, silent).
pub const TTS_ENGINES: &[&str] = &["piper", "edge", "none"];

/// The TTS engine: `flag` (`--tts`), else `$AGENTFX_TTS`, else `piper`. `--no-tts` means `none`.
pub fn tts_engine(flag: Option<&str>, no_tts: bool) -> Result<String, String> {
    if no_tts {
        return Ok("none".into());
    }
    let e = flag.map(str::to_string).or_else(|| agentfx_ir::env::var("AGENTFX_TTS").ok().filter(|e| !e.is_empty())).unwrap_or_else(|| "piper".into());
    if TTS_ENGINES.contains(&e.as_str()) { Ok(e) } else { Err(format!("unknown TTS engine `{e}` (piper, edge, none)")) }
}

/// Word timings for every `say` from `engine` (see [`tts_engine`]). Uses the TTS cache; falls back
/// to estimates (silent, with an info when the engine is not installed, a warning when it fails).
pub fn tts(doc: &Document, engine: &str, diags: &mut Vec<Diag>) -> HashMap<String, TtsResult> {
    let reqs = tts_requests(doc);
    let mut out = HashMap::new();
    if reqs.is_empty() {
        return out;
    }
    if engine == "none" {
        for r in &reqs {
            out.insert(r.key.clone(), estimate(r));
        }
        return out;
    }
    // group by voice/rate (tts.py takes one voice per call)
    let mut groups: HashMap<(String, String), Vec<&crate::timing::TtsReq>> = HashMap::new();
    for r in &reqs {
        groups.entry((r.voice.clone(), r.rate.clone())).or_default().push(r);
    }
    let py = python();
    for ((voice, rate), items) in groups {
        let req = serde_json::json!({
            "engine": engine, "voice": voice, "rate": rate, "cache": cache_dir("tts"),
            "items": items.iter().map(|r| serde_json::json!({"key": r.key, "text": r.text})).collect::<Vec<_>>()
        });
        let res = Command::new(&py)
            .arg("-I")
            .arg(home().join("tools/tts.py"))
            .env("AGENTFX_FFMPEG", ffmpeg())
            .env("AGENTFX_FFPROBE", ffprobe())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                c.stdin.take().unwrap().write_all(req.to_string().as_bytes())?;
                c.wait_with_output()
            });
        let parsed: Option<serde_json::Value> = match res {
            Ok(o) if o.status.success() => serde_json::from_slice(&o.stdout).ok(),
            Ok(o) if o.status.code() == Some(3) => {
                let why = String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("").trim_start_matches("tts.py: ").to_string();
                diags.push(Diag::info("I_TTS_NONE", format!("TTS engine `{engine}` unavailable: {why}"), "timings are estimated and the video is silent; install piper (pip install piper-tts) or pick another engine with --tts / $AGENTFX_TTS", Default::default()));
                None
            }
            Ok(o) => {
                diags.push(Diag::warn("W_TTS", format!("TTS failed: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")), "timings are estimated (15 chars/s); run again with network", Default::default()));
                None
            }
            Err(e) => {
                diags.push(Diag::warn("W_TTS", format!("TTS unavailable: {e}"), "timings are estimated (15 chars/s)", Default::default()));
                None
            }
        };
        for r in items {
            let v = parsed.as_ref().and_then(|p| p.get(&r.key));
            let res = v.and_then(|v| {
                let words = v["words"].as_array()?.iter().filter_map(|w| {
                    let a = w.as_array()?;
                    Some(TtsWord { text: a.first()?.as_str()?.to_string(), start: a.get(1)?.as_f64()?, end: a.get(2)?.as_f64()? })
                }).collect();
                Some(TtsResult { mp3: v["mp3"].as_str().map(|s| s.to_string()), dur: v["dur"].as_f64()?, words, estimated: v["estimated"].as_bool().unwrap_or(false) })
            });
            out.insert(r.key.clone(), res.unwrap_or_else(|| estimate(r)));
        }
    }
    out
}

pub fn node_tool(args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let script = home().join("tools/capture.mjs");
    let out = Command::new(agentfx_ir::env::var("AGENTFX_NODE").unwrap_or("node".into()))
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .and_then(|mut c| {
            c.stdin.take().unwrap().write_all(args.to_string().as_bytes())?;
            c.wait_with_output()
        })
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("capture failed ({})", out.status));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("capture output: {e}"))
}

/// Mix all audio clips into one wav (voice + sfx + ducked music).
pub fn mix_audio(ir: &crate::ir::Ir, out: &Path) -> Result<bool, String> {
    if ir.audio.is_empty() {
        return Ok(false);
    }
    let mut cmd = Command::new(ffmpeg());
    cmd.args(["-v", "error", "-y"]);
    let mut filters = vec![];
    let mut labels = vec![];
    let voices: Vec<(f64, f64)> = ir.audio.iter().filter(|a| a.kind == "voice").map(|a| (a.at, a.at + a.dur)).collect();
    let mut k = 0;
    for a in &ir.audio {
        if let Some(name) = a.src.strip_prefix("builtin:") {
            let expr = match name {
                "pop" => "0.6*sin(2*PI*(500+900*t)*t)*exp(-30*t)",
                "whoosh" => "0.25*(random(0)*2-1)*sin(PI*t/0.4)",
                _ => "0.8*(random(0)*2-1)*exp(-120*t)+0.5*sin(2*PI*1800*t)*exp(-90*t)",
            };
            cmd.args(["-f", "lavfi", "-t", "0.4", "-i", &format!("aevalsrc='{expr}':s=48000:c=mono")]);
        } else {
            cmd.args(["-i", &a.src]);
        }
        let ms = (a.at * 1000.0).round() as i64;
        let mut f = format!("[{k}:a]aresample=48000,aformat=channel_layouts=mono,volume={}", a.gain);
        if let Some(d) = a.duck {
            let cond: Vec<String> = voices.iter().map(|(s, e)| format!("between(t,{s:.3},{e:.3})")).collect();
            if !cond.is_empty() {
                f.push_str(&format!(",volume='if({},{},1)':eval=frame", cond.join("+"), d / a.gain.max(1e-6)));
            }
            f.push_str(&format!(",atrim=0:{:.3}", ir.duration));
        }
        f.push_str(&format!(",adelay={ms}|{ms}[a{k}]"));
        filters.push(f);
        labels.push(format!("[a{k}]"));
        k += 1;
    }
    filters.push(format!("{}amix=inputs={}:normalize=0:dropout_transition=0,apad,atrim=0:{:.3},loudnorm=I=-16:TP=-1.5:LRA=11[out]", labels.join(""), k, ir.duration));
    cmd.args(["-filter_complex", &filters.join(";"), "-map", "[out]", "-ar", "48000", "-ac", "2"]);
    cmd.arg(out);
    let st = cmd.status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("audio mix failed".into());
    }
    Ok(true)
}
