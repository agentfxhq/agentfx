//! Direct mode driver (S1 default render path): Reel Script -> reel-ir (native layout, no
//! Chrome) -> the reel renderer binary -> video; audio is mixed and muxed here with ffmpeg.
//! Single frames and contact sheets use time-shifted one-frame IRs (and a sheet IR whose
//! cells are those frames plus native text labels), so `frame`/`sheet` need no browser either.
use crate::diag::Diag;
use crate::ir::Ir;
use crate::native::{self, Native};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The renderer binary (`agentfx-render`). First match wins:
/// `$AGENTFX_RENDERER` (or `$REEL_RENDERER`) as an explicit override, then `agentfx-render`
/// next to this executable (for `cargo test` binaries in `target/<profile>/deps`, also one
/// level up), then on `PATH`, then the build tree's `target/release/agentfx-render`.
pub fn renderer() -> PathBuf {
    const NAME: &str = if cfg!(windows) { "agentfx-render.exe" } else { "agentfx-render" };
    if let Some(p) = agentfx_ir::env::var_os("AGENTFX_RENDERER").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok()).and_then(|e| e.parent().map(Path::to_path_buf)) {
        let mut cands = vec![dir.join(NAME)];
        if dir.file_name().is_some_and(|n| n == "deps") {
            if let Some(up) = dir.parent() {
                cands.push(up.join(NAME));
            }
        }
        if let Some(c) = cands.into_iter().find(|c| c.is_file()) {
            return c;
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let c = dir.join(NAME);
            if c.is_file() {
                return c;
            }
        }
    }
    // developer fallback: the renderer built in this source tree
    PathBuf::from(env!("AGENTFX_ROOT")).join("target/release").join(NAME)
}

pub struct Compiled {
    pub scene: Value,
    pub diags: Vec<Diag>,
    pub stats: Value,
}

/// Motion IR -> reel-ir v1 JSON (validated with reel-ir's own validator).
pub fn compile(ir: &Ir, out_dir: &Path, persist: bool) -> Result<Compiled> {
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(out_dir)?;
    let fonts = crate::run::home().join("assets/fonts");
    let mut nat = Native::load(ir, native::snap_at(ir, 0.0), &fonts, out_dir, persist)?;
    let t_layout = t0.elapsed().as_secs_f64();
    let out = crate::reelir::emit(ir, &mut nat);
    // Renderer assets are PNG-only. Normalize JPEG resources after native layout,
    // including images referenced by recorded brushes, without changing author paths.
    let mut out = out;
    if persist {
        for (i, asset) in out.scene["assets"].as_array_mut().into_iter().flatten().enumerate() {
            let path = PathBuf::from(asset["path"].as_str().unwrap_or_default());
            let path = if path.is_absolute() { path } else { out_dir.join(path) };
            let reader = image::ImageReader::open(&path)?.with_guessed_format()?;
            if reader.format() == Some(image::ImageFormat::Jpeg) {
                let png = out_dir.join(format!("jpeg-{i}.png"));
                reader.decode().with_context(|| format!("decoding {}", path.display()))?.save(&png)?;
                asset["path"] = json!(png.canonicalize()?);
            }
        }
    }
    let mut stats = out.stats;
    stats["layout_s"] = json!(t_layout);
    for (k, v) in &nat.stats {
        stats[format!("native_{k}")] = json!(v);
    }
    stats["compile_s"] = json!(t0.elapsed().as_secs_f64());
    // generated resources (fonts/images written by the recorder) are relative to `out_dir`;
    // make them absolute so the IR can be written and sliced anywhere
    let mut scene = out.scene;
    for key in ["assets", "fonts"] {
        if let Some(arr) = scene[key].as_array_mut() {
            for a in arr.iter_mut() {
                if let Some(p) = a["path"].as_str() {
                    if !Path::new(p).is_absolute() {
                        let abs = out_dir.canonicalize().unwrap_or(out_dir.to_path_buf()).join(p);
                        a["path"] = json!(abs);
                    }
                }
            }
        }
    }
    let out = crate::reelir::Out { scene, diags: out.diags, stats: Value::Null };
    let mut diags = out.diags;
    match serde_json::from_value::<agentfx_ir::Scene>(out.scene.clone()) {
        Ok(sc) => {
            if let Err(errs) = agentfx_ir::validate::validate(&sc, out_dir) {
                for e in errs {
                    diags.push(Diag::error("E_IR_INVALID", format!("reel-ir validation: {e}"), "compiler bug: please report with the .reel file", Default::default()));
                }
            }
        }
        Err(e) => diags.push(Diag::error("E_IR_INVALID", format!("reel-ir does not parse: {e}"), "compiler bug", Default::default())),
    }
    Ok(Compiled { scene: out.scene, diags, stats })
}

fn run_reel(ir_path: &Path, args: &[&str]) -> Result<Value> {
    let bin = renderer();
    if !bin.exists() {
        bail!("renderer not found at {} (build it with `cargo build --release` at the repository root, or run tools/install.sh, which installs agentfx-render next to agentfx; or set AGENTFX_RENDERER)", bin.display());
    }
    let out = Command::new(&bin).arg("render").arg(ir_path).args(args).output().with_context(|| format!("running {}", bin.display()))?;
    if !out.status.success() {
        bail!("agentfx render failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let last = s.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("{}");
    Ok(serde_json::from_str(last).unwrap_or(json!({})))
}

/// Full render: IR file next to the video, reel renders VFR x264, then audio is muxed.
pub fn render(ir: &Ir, scene: &Value, out: &Path, segments: usize, audio: bool) -> Result<Value> {
    render_with_options(ir, scene, out, segments, audio, "vfr", false)
}

pub fn render_with_options(ir: &Ir, scene: &Value, out: &Path, segments: usize, audio: bool, mode: &str, alpha: bool) -> Result<Value> {
    anyhow::ensure!(matches!(mode, "cfr" | "vfr"), "mode must be cfr or vfr");
    let t0 = std::time::Instant::now();
    let dir = out.parent().unwrap_or(Path::new("."));
    let ir_path = out.with_extension("ir.json");
    std::fs::write(&ir_path, serde_json::to_string(scene)?)?;
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    let supports_audio = !matches!(ext.as_str(), "gif" | "webp" | "png");
    if audio && !ir.audio.is_empty() && !supports_audio {
        eprintln!("audio omitted: {ext} does not support an audio stream");
    }
    let video = if audio && supports_audio && !ir.audio.is_empty() { out.with_extension(format!("video.{ext}")) } else { out.to_path_buf() };
    let seg = segments.to_string();
    // the audio mix (ffmpeg) runs while reel renders the video
    let wav = out.with_extension("wav");
    let mix = (video != out).then(|| {
        let ir2 = ir.clone();
        let wav2 = wav.clone();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            (crate::run::mix_audio(&ir2, &wav2), t.elapsed().as_secs_f64())
        })
    });
    let mut args = vec!["--out", video.to_str().unwrap(), "--segments", &seg, "--mode", mode];
    if alpha { args.push("--alpha"); }
    let mut st = run_reel(&ir_path, &args)?;
    st["reel_wall_s"] = json!(t0.elapsed().as_secs_f64());
    if let Some(h) = mix {
        let t1 = std::time::Instant::now();
        let (r, mix_s) = h.join().map_err(|_| anyhow::anyhow!("audio thread panicked"))?;
        st["audio_mix_s"] = json!(mix_s);
        let has = r.map_err(|e| anyhow::anyhow!(e))?;
        if has {
            let mut mux = Command::new(crate::run::ffmpeg());
            mux.args(["-v", "error", "-y", "-i"]).arg(&video).arg("-i").arg(&wav)
                .args(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", if ext == "webm" { "libopus" } else { "aac" }, "-b:a", "192k", "-shortest"]);
            if matches!(ext.as_str(), "mp4" | "mov") { mux.args(["-movflags", "+faststart"]); }
            let s = mux.arg(out).status()?;
            if !s.success() {
                bail!("audio mux failed");
            }
            let _ = std::fs::remove_file(&wav);
            let _ = std::fs::remove_file(&video);
        } else {
            std::fs::rename(&video, out)?;
        }
        st["audio_wait_mux_s"] = json!(t1.elapsed().as_secs_f64());
    }
    let _ = dir;
    st["wall_s"] = json!(t0.elapsed().as_secs_f64());
    Ok(st)
}

/// The scene at one frame as a one-frame IR (see `range_ir`).
pub fn frame_ir(scene: &Value, frame: u32) -> Value {
    range_ir(scene, frame, frame + 1)
}

/// The frames [f0, f1) of a scene as a standalone IR: tracks shifted, switches re-based, subtrees
/// invisible during the whole range pruned (content dropped, tree kept whole), unused assets
/// dropped. Used for single frames, sheets and content-addressed scene segments.
pub fn range_ir(scene: &Value, f0: u32, f1: u32) -> Value {
    let fps = scene["fps"].as_f64().unwrap_or(30.0);
    let t = f0 as f64 / fps;
    let mut s = scene.clone();
    s["frames"] = json!(f1.saturating_sub(f0).max(1));
    let nodes = s["nodes"].as_array().cloned().unwrap_or_default();
    // max opacity per node over the range (a node is dead only if invisible at every frame)
    let base: Vec<f64> = nodes.iter().map(|n| n["opacity"].as_f64().unwrap_or(1.0)).collect();
    let mut op = base.clone();
    let mut by_node: HashMap<usize, agentfx_ir::Track> = HashMap::new();
    if let Some(ts) = s["tracks"].as_array() {
        for tr in ts {
            if tr["property"] != json!("opacity") {
                continue;
            }
            if let Ok(t2) = serde_json::from_value::<agentfx_ir::Track>(tr.clone()) {
                by_node.insert(t2.node, t2);
            }
        }
    }
    for (n, tr) in &by_node {
        let mut m: f64 = 0.0;
        for f in f0..f1.max(f0 + 1) {
            if let Some(agentfx_ir::Sampled::Number(v)) = agentfx_ir::eval::eval_track(tr, f as f64 / fps) {
                m = m.max(v);
            } else {
                m = m.max(base[*n]);
            }
        }
        op[*n] = m;
    }
    let mut dead = vec![false; nodes.len()];
    fn mark(i: usize, nodes: &[Value], op: &[f64], dead: &mut [bool], parent_dead: bool) {
        let d = parent_dead || op[i] <= 1e-4;
        dead[i] = d;
        for c in nodes[i]["children"].as_array().into_iter().flatten() {
            if let Some(c) = c.as_u64() {
                mark(c as usize, nodes, op, dead, d);
            }
        }
    }
    if !nodes.is_empty() {
        mark(0, &nodes, &op, &mut dead, false);
    }
    // fluids simulate from time 0: the nodes they ink from (and those nodes' ancestors, for
    // their transforms) stay whole even when invisible in the range
    let mut parent = vec![None; nodes.len()];
    for (i, nd) in nodes.iter().enumerate() {
        for c in nd["children"].as_array().into_iter().flatten().filter_map(|c| c.as_u64()) {
            if let Some(p) = parent.get_mut(c as usize) {
                *p = Some(i);
            }
        }
    }
    for (i, nd) in nodes.iter().enumerate() {
        if dead[i] || nd["content"]["type"] != json!("fluid") {
            continue;
        }
        for e in nd["content"]["emitters"].as_array().into_iter().flatten() {
            let Some(src) = e["node"].as_u64().map(|v| v as usize).filter(|&v| v < nodes.len()) else { continue };
            let mut st = vec![src];
            while let Some(x) = st.pop() {
                dead[x] = false;
                st.extend(nodes[x]["children"].as_array().into_iter().flatten().filter_map(|c| c.as_u64()).map(|c| c as usize));
            }
            let mut a = parent[src];
            while let Some(p) = a {
                dead[p] = false;
                a = parent[p];
            }
        }
    }
    if let Some(arr) = s["nodes"].as_array_mut() {
        for (i, n) in arr.iter_mut().enumerate() {
            if dead[i] && i != 0 {
                if let Some(o) = n.as_object_mut() {
                    // invisible in this range: no content (children stay: the tree must stay whole)
                    o.remove("content");
                    o.remove("effects");
                    o.remove("motion_blur");
                    // canonical (scene hashes must not see geometry of invisible nodes)
                    o.remove("transform");
                    o.remove("clips");
                    o.insert("origin".into(), json!([0.0, 0.0]));
                    o.insert("name".into(), json!(""));
                    o.insert("opacity".into(), json!(0.0));
                }
            }
            if let Some(c) = n.get_mut("content") {
                if c["type"] == json!("cloud") || c["type"] == json!("fluid") {
                    // clouds evaluate their own time: shift it, keep the motion
                    let o = c["time_offset"].as_f64().unwrap_or(0.0);
                    c["time_offset"] = json!(o + t);
                }
                // text v2 key selectors carry absolute unit times: shift them like the tracks
                if let Some(anims) = c.get_mut("text").and_then(|x| x.get_mut("animators")).and_then(|a| a.as_array_mut()) {
                    for a in anims {
                        if let Some(units) = a["selector"].get_mut("units").and_then(|u| u.as_array_mut()) {
                            for k in units.iter_mut().filter_map(|u| u.as_array_mut()).flatten() {
                                if let Some(t0) = k[0].as_f64() {
                                    k[0] = json!(t0 - t);
                                }
                            }
                        }
                    }
                }
                if c["type"] == json!("switch") {
                    let fr: Vec<(u64, Value)> = c["frames"].as_array().map(|a| a.iter().map(|x| (x[0].as_u64().unwrap_or(0), x[1].clone())).collect()).unwrap_or_default();
                    let first = fr.iter().filter(|x| x.0 <= f0 as u64).last().map(|x| x.1.clone()).unwrap_or(json!(0));
                    let mut out = vec![json!([0, first])];
                    for (f, st) in fr.iter().filter(|x| x.0 > f0 as u64 && x.0 < f1 as u64) {
                        out.push(json!([f - f0 as u64, st]));
                    }
                    c["frames"] = json!(out);
                }
            }
        }
    }
    // drop tracks of dead nodes (their nodes have no content) and shift the rest
    if let Some(ts) = s["tracks"].as_array_mut() {
        ts.retain(|tr| !dead.get(tr["node"].as_u64().unwrap_or(0) as usize).copied().unwrap_or(false) || tr["node"].as_u64() == Some(0));
        for tr in ts.iter_mut() {
            let d = tr["delay"].as_f64().unwrap_or(0.0);
            tr["delay"] = json!(d - t);
        }
    }
    // unused assets
    let mut used: HashMap<u64, u64> = HashMap::new();
    fn collect(v: &Value, used: &mut HashMap<u64, u64>) {
        match v {
            Value::Object(m) => {
                if let Some(a) = m.get("asset").and_then(|a| a.as_u64()) {
                    let n = used.len() as u64;
                    used.entry(a).or_insert(n);
                }
                m.values().for_each(|x| collect(x, used));
            }
            Value::Array(a) => a.iter().for_each(|x| collect(x, used)),
            _ => {}
        }
    }
    for n in s["nodes"].as_array().into_iter().flatten() {
        collect(&n["content"], &mut used);
    }
    fn remap(v: &mut Value, used: &HashMap<u64, u64>) {
        match v {
            Value::Object(m) => {
                if let Some(a) = m.get("asset").and_then(|a| a.as_u64()) {
                    m.insert("asset".into(), json!(used[&a]));
                }
                m.values_mut().for_each(|x| remap(x, used));
            }
            Value::Array(a) => a.iter_mut().for_each(|x| remap(x, used)),
            _ => {}
        }
    }
    if let Some(arr) = s["nodes"].as_array_mut() {
        for n in arr.iter_mut() {
            if let Some(c) = n.get_mut("content") {
                remap(c, &used);
            }
        }
    }
    let old = s["assets"].as_array().cloned().unwrap_or_default();
    let mut na = vec![Value::Null; used.len()];
    for (o, n) in &used {
        na[*n as usize] = old[*o as usize].clone();
    }
    s["assets"] = json!(na);
    s
}

/// Render the given frames to PNGs (parallel, `jobs` reel processes).
pub fn frames(scene: &Value, frames: &[u32], dir: &Path, jobs: usize) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let ffmpeg = crate::run::ffmpeg();
    let work: Vec<(usize, u32)> = frames.iter().copied().enumerate().collect();
    let results: std::sync::Mutex<Vec<Option<Result<PathBuf>>>> = std::sync::Mutex::new((0..frames.len()).map(|_| None).collect());
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|sc| {
        for _ in 0..jobs.max(1).min(frames.len().max(1)) {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if k >= work.len() {
                    break;
                }
                let (i, f) = work[k];
                let r = (|| -> Result<PathBuf> {
                    let ir = frame_ir(scene, f);
                    let p = dir.join(format!("f{i:03}-{f}.ir.json"));
                    std::fs::write(&p, serde_json::to_string(&ir)?)?;
                    let mkv = dir.join(format!("f{i:03}.mkv"));
                    let dummy = dir.join(format!("f{i:03}.mp4"));
                    run_reel(&p, &["--out", dummy.to_str().unwrap(), "--no-encode", "--lossless", mkv.to_str().unwrap()])?;
                    let png = dir.join(format!("f{i:03}.png"));
                    let s = Command::new(&ffmpeg).args(["-v", "error", "-y", "-i"]).arg(&mkv).args(["-frames:v", "1"]).arg(&png).status()?;
                    if !s.success() {
                        bail!("png extract failed");
                    }
                    let _ = std::fs::remove_file(&mkv);
                    let _ = std::fs::remove_file(&p);
                    Ok(png)
                })();
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(|r| r.unwrap_or_else(|| bail!("frame not rendered"))).collect()
}

/// Contact sheet as a reel scene: cells are the frame PNGs (+ optional reference PNGs), labels
/// are native text. One agentfx frame -> PNG.
pub fn sheet(cells: &[(PathBuf, Option<PathBuf>, String)], fw: u32, fh: u32, cols: usize, out: &Path) -> Result<()> {
    sheet_with_energy(cells, None, fw, fh, cols, out)
}

/// Contact sheet with an optional energy bar under each thumbnail: `energy[i]` is the share of the
/// frame (0..1) moving around that cell's time; the bar is drawn on a sqrt scale and the label
/// carries the percentage.
pub fn sheet_with_energy(cells: &[(PathBuf, Option<PathBuf>, String)], energy: Option<&[f32]>, fw: u32, fh: u32, cols: usize, out: &Path) -> Result<()> {
    let cols = cols.max(1);
    let pair = cells.iter().any(|c| c.1.is_some());
    let cw = if pair { 640.0 } else if fw >= fh { 470.0 } else { 300.0 };
    let ch = cw * fh as f64 / fw as f64;
    let per_row = if pair { 1 } else { cols };
    let (gap, lab) = (10.0, 26.0);
    let cell_w = if pair { 2.0 * cw + gap } else { cw };
    let rows = cells.len().div_ceil(per_row);
    let w = (gap + per_row as f64 * (cell_w + gap)).ceil();
    let h = (gap + rows as f64 * (ch + lab + gap)).ceil();
    let fonts = crate::run::home().join("assets/fonts/Inter.ttf");
    let mut nodes = vec![json!({"name": "sheet", "children": [], "content": {"type": "rect", "rect": {"rect": [0, 0, w, h], "radius": [0, 0, 0, 0]}, "color": [0.067, 0.067, 0.067, 1]}})];
    let mut assets = vec![];
    let mut kids = vec![];
    let add_img = |p: &Path, x: f64, y: f64, nodes: &mut Vec<Value>, assets: &mut Vec<Value>, kids: &mut Vec<usize>| -> Result<()> {
        let (pw, _ph) = crate::lower::png_size(p).context("frame png")?;
        let s = pw / cw;
        assets.push(json!({"path": p, "scale": s}));
        nodes.push(json!({"name": "cell", "children": [], "content": {"type": "image", "asset": assets.len() - 1, "rect": [x, y, cw, ch]}}));
        kids.push(nodes.len() - 1);
        Ok(())
    };
    for (i, (img, rf, label)) in cells.iter().enumerate() {
        let (r, c) = (i / per_row, i % per_row);
        let x = gap + c as f64 * (cell_w + gap);
        let y = gap + r as f64 * (ch + lab + gap);
        if let Some(rp) = rf {
            add_img(rp, x, y, &mut nodes, &mut assets, &mut kids)?;
            add_img(img, x + cw + gap, y, &mut nodes, &mut assets, &mut kids)?;
        } else {
            add_img(img, x, y, &mut nodes, &mut assets, &mut kids)?;
        }
        let mut text = if pair { format!("reference (left) · reel (right) · {label}") } else { label.clone() };
        if let Some(e) = energy.and_then(|e| e.get(i)).copied() {
            text = format!("{text} · motion {:.0} %", e * 100.0);
            let full = cell_w;
            nodes.push(json!({"name": "energy-track", "children": [], "content": {"type": "rect", "rect": {"rect": [x, y + ch + lab + 1.0, full, 3.0], "radius": [0, 0, 0, 0]}, "color": [0.2, 0.2, 0.2, 1]}}));
            kids.push(nodes.len() - 1);
            let bw = (full * (e.max(0.0) as f64).sqrt()).max(if e > 1e-4 { 2.0 } else { 0.0 });
            if bw > 0.0 {
                nodes.push(json!({"name": "energy", "children": [], "content": {"type": "rect", "rect": {"rect": [x, y + ch + lab + 1.0, bw, 3.0], "radius": [0, 0, 0, 0]}, "color": [0.32, 0.78, 0.5, 1]}}));
                kids.push(nodes.len() - 1);
            }
        }
        nodes.push(json!({"name": "label", "children": [], "content": {"type": "text", "text": text, "font": 0, "size": 15, "weight": 600, "pos": [x, y + ch + 4.0], "brush": {"solid": [0.87, 0.87, 0.87, 1]}}}));
        kids.push(nodes.len() - 1);
    }
    nodes[0]["children"] = json!(kids);
    let scene = json!({"version": 1, "width": (w as u32 + 1) / 2 * 2, "height": (h as u32 + 1) / 2 * 2, "fps": 30, "frames": 1, "nodes": nodes, "tracks": [], "assets": assets, "fonts": [{"path": fonts}]});
    let dir = out.parent().unwrap_or(Path::new("."));
    let p = dir.join(format!(".sheet-{}.ir.json", std::process::id()));
    std::fs::write(&p, serde_json::to_string(&scene)?)?;
    let mkv = p.with_extension("mkv");
    let dummy = p.with_extension("mp4");
    run_reel(&p, &["--out", dummy.to_str().unwrap(), "--no-encode", "--lossless", mkv.to_str().unwrap()])?;
    let s = Command::new(crate::run::ffmpeg()).args(["-v", "error", "-y", "-i"]).arg(&mkv).args(["-frames:v", "1"]).arg(out).status()?;
    let _ = std::fs::remove_file(&mkv);
    let _ = std::fs::remove_file(&p);
    if !s.success() {
        bail!("sheet png failed");
    }
    Ok(())
}

/// Scene frame ranges from the motion IR (scene roots carry `start:<s>` tags).
pub fn scene_ranges(ir: &Ir) -> Vec<(String, u32, u32)> {
    let nf = (ir.duration * ir.fps).round() as u32;
    let mut starts: Vec<(String, u32)> = ir.root.children.iter().filter_map(|l| {
        let st = l.tags.iter().find_map(|t| t.strip_prefix("start:"))?.parse::<f64>().ok()?;
        Some((l.id.trim_start_matches("scene-").to_string(), (st * ir.fps).round() as u32))
    }).collect();
    starts.sort_by_key(|x| x.1);
    let mut out = vec![];
    for (i, (id, f)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map(|x| x.1).unwrap_or(nf);
        let f0 = if i == 0 { 0 } else { *f };
        if end > f0 {
            out.push((id.clone(), f0, end));
        }
    }
    out
}

/// Content-addressed partial render: every scene range becomes its own IR slice, hashed over its
/// exact JSON (what is drawn in those frames, delays relative to the range) plus the referenced
/// files (path, size, mtime). Cached segments are reused; changed scenes are re-rendered; the
/// segments are concatenated without re-encoding, then audio is muxed.
pub fn render_cached(ir: &Ir, scene: &Value, out: &Path, cache: &Path) -> Result<Value> {
    use sha2::{Digest, Sha256};
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(cache)?;
    let dir = out.parent().unwrap_or(Path::new("."));
    let work = dir.join(format!(".reel-seg-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    let mut segs = vec![];
    let (mut hits, mut miss) = (0, 0);
    let mut detail = vec![];
    for (id, f0, f1) in scene_ranges(ir) {
        let part = range_ir(scene, f0, f1);
        let mut h = Sha256::new();
        h.update(serde_json::to_string(&part)?.as_bytes());
        let mut files: Vec<String> = vec![];
        for a in part["assets"].as_array().into_iter().flatten().chain(part["fonts"].as_array().into_iter().flatten()) {
            if let Some(p) = a["path"].as_str() {
                files.push(p.to_string());
            }
        }
        for f in files {
            if let Ok(m) = std::fs::metadata(&f) {
                h.update(format!("{f}|{}|{:?}", m.len(), m.modified().ok()).as_bytes());
            }
        }
        let key: String = h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect();
        let seg = cache.join(format!("{key}.mp4"));
        let t1 = std::time::Instant::now();
        let hit = seg.exists();
        if !hit {
            let p = work.join(format!("{id}.ir.json"));
            std::fs::write(&p, serde_json::to_string(&part)?)?;
            let tmp = work.join(format!("{id}.mp4"));
            run_reel(&p, &["--out", tmp.to_str().unwrap()])?;
            std::fs::rename(&tmp, &seg).or_else(|_| std::fs::copy(&tmp, &seg).map(|_| ()))?;
            miss += 1;
        } else {
            hits += 1;
        }
        detail.push(json!({"scene": id, "frames": [f0, f1], "hash": key, "cached": hit, "s": t1.elapsed().as_secs_f64()}));
        segs.push(seg);
    }
    let list = work.join("list.txt");
    std::fs::write(&list, segs.iter().map(|s| format!("file '{}'", s.display())).collect::<Vec<_>>().join("\n"))?;
    let video = out.with_extension("video.mp4");
    let st = Command::new(crate::run::ffmpeg()).args(["-v", "error", "-y", "-f", "concat", "-safe", "0", "-i"]).arg(&list).args(["-c", "copy"]).arg(&video).status()?;
    if !st.success() {
        bail!("segment concat failed");
    }
    let wav = out.with_extension("wav");
    if crate::run::mix_audio(ir, &wav).map_err(|e| anyhow::anyhow!(e))? {
        let s = Command::new(crate::run::ffmpeg()).args(["-v", "error", "-y", "-i"]).arg(&video).arg("-i").arg(&wav)
            .args(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart"]).arg(out).status()?;
        if !s.success() {
            bail!("audio mux failed");
        }
        let _ = std::fs::remove_file(&wav);
        let _ = std::fs::remove_file(&video);
    } else {
        std::fs::rename(&video, out)?;
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(json!({"segments": detail, "cached": hits, "rendered": miss, "wall_s": t0.elapsed().as_secs_f64()}))
}
