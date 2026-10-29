//! Deliverables besides MP4 (E6): ProRes 4444 / 422 HQ (`.mov`), VP9 (`.webm`, with
//! alpha), PNG sequences, GIF (two-pass palette) and animated WebP, chosen by the
//! `--out` extension. `--alpha` keeps the transparent background.
//!
//! Frames leave the renderer as premultiplied RGBA. At this boundary they become
//! straight alpha (only the damaged rects are reconverted into a persistent buffer);
//! without `--alpha` ffmpeg reads them as `rgb0`, i.e. composited over black, which is
//! what the NV12 path shows too. The MP4 encoder (`encode.rs`) is not involved.
//!
//! Every segment pipes CFR rawvideo into its own ffmpeg. ProRes and PNG write the
//! deliverable directly (ProRes segments are joined with `-c copy`). VP9, GIF and WebP
//! are staged: segments write lossless FFV1 parts, then one final ffmpeg reads them
//! through the concat demuxer (GIF: palettegen pass, then paletteuse pass). Copy-joined
//! VP9 loses its Matroska AlphaMode element, and GIF needs one palette over all frames.
use anyhow::{Context, Result, bail, ensure};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `.mov`: ProRes 4444 `yuva444p10le` with alpha, ProRes 422 HQ without.
    ProRes,
    /// `.webm`: VP9 (`libvpx-vp9`), `yuva420p` with alpha.
    Vp9,
    /// `.png`: one file per frame (`x_%05d.png`, or the `%` pattern given in `--out`).
    Png,
    /// `.gif`: palettegen / paletteuse, 1-bit transparency with alpha.
    Gif,
    /// `.webp`: animated WebP (`libwebp_anim`).
    Webp,
}

impl Format {
    /// The format of an output path by extension; `None` for `.mp4` (the encoder path).
    pub fn from_out(out: &Path) -> Result<Option<Format>> {
        let ext = out.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        Ok(Some(match ext.as_str() {
            "mp4" => return Ok(None),
            "mov" => Format::ProRes,
            "webm" => Format::Vp9,
            "png" => Format::Png,
            "gif" => Format::Gif,
            "webp" => Format::Webp,
            e => bail!("unsupported output extension {e:?} (mp4|mov|webm|png|gif|webp)"),
        }))
    }

    /// Segments write lossless FFV1 parts and one final encode follows.
    pub fn staged(self) -> bool { matches!(self, Format::Vp9 | Format::Gif | Format::Webp) }
}

/// How often GIF / WebP play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loop {
    Forever,
    /// Total plays (1 = once).
    Times(u32),
}

impl Loop {
    pub fn parse(s: &str) -> Result<Loop> {
        Ok(match s {
            "forever" | "0" | "inf" => Loop::Forever,
            "once" | "off" => Loop::Times(1),
            n => Loop::Times(n.parse::<u32>().ok().filter(|&n| (1..=65535).contains(&n)).with_context(|| format!("--loop {n}: forever|once|N (total plays)"))?),
        })
    }
}

#[derive(Clone, Debug)]
pub struct ExportOpts {
    pub format: Format,
    /// Keep the transparent background (straight alpha in the deliverable).
    pub alpha: bool,
    /// Output frame rate (`fps` filter; staged formats only).
    pub fps: Option<u32>,
    /// Output width in px (height keeps the aspect, even); premultiplied scaling.
    pub width: Option<u32>,
    pub looping: Loop,
    /// VP9 crf (default 30), WebP quality 0..100 (default 80).
    pub quality: Option<u32>,
    /// Lossless WebP.
    pub lossless: bool,
}

impl ExportOpts {
    /// File a segment's ffmpeg writes: the deliverable, a ProRes part or an FFV1 part.
    pub fn segment_out(&self, out: &Path, i: usize, multi: bool) -> PathBuf {
        if self.format.staged() {
            return sibling(out, &format!("part{i:02}.mkv"));
        }
        match self.format {
            Format::Png => png_pattern(out),
            _ if multi => sibling(out, &format!("seg{i:02}.mov")),
            _ => out.to_path_buf(),
        }
    }

    /// ffmpeg log of segment i (next to the deliverable; removed on success).
    pub fn segment_log(&self, out: &Path, i: usize) -> PathBuf {
        let base = if self.format == Format::Png { out.with_file_name(out.file_name().unwrap_or_default().to_string_lossy().replace('%', "")) } else { out.to_path_buf() };
        sibling(&base, &format!("seg{i:02}.ffmpeg.log"))
    }

    pub fn describe(&self) -> String {
        format!("{:?}{}", self.format, if self.alpha { "+alpha" } else { "" })
    }
}

fn sibling(out: &Path, suffix: &str) -> PathBuf {
    let stem = out.file_stem().unwrap_or_default().to_string_lossy();
    out.with_file_name(format!("{stem}.{suffix}"))
}

/// `frames/f.png` -> `frames/f_%05d.png`; a path that already holds a `%` pattern stays.
pub fn png_pattern(out: &Path) -> PathBuf {
    let name = out.file_name().unwrap_or_default().to_string_lossy();
    if name.contains('%') {
        return out.to_path_buf();
    }
    out.with_file_name(format!("{}_%05d.png", out.file_stem().unwrap_or_default().to_string_lossy()))
}

/// Premultiplied -> straight RGBA for the pixels of `rects` (clamped to the frame).
/// Pixels outside the rects keep their previous value in `dst`.
pub fn unpremultiply(src: &[u8], dst: &mut [u8], w: usize, h: usize, rects: &[[i32; 4]]) {
    use rayon::prelude::*;
    assert_eq!(src.len(), w * h * 4);
    assert_eq!(dst.len(), w * h * 4);
    for &[x, y, rw, rh] in rects {
        let x0 = (x as i64).clamp(0, w as i64) as usize;
        let y0 = (y as i64).clamp(0, h as i64) as usize;
        let x1 = (x as i64 + rw as i64).clamp(0, w as i64) as usize;
        let y1 = (y as i64 + rh as i64).clamp(0, h as i64) as usize;
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let row = |(yy, d): (usize, &mut [u8])| {
            let s = &src[((y0 + yy) * w + x0) * 4..((y0 + yy) * w + x1) * 4];
            for (d, s) in d[x0 * 4..x1 * 4].chunks_exact_mut(4).zip(s.chunks_exact(4)) {
                let a = s[3] as u32;
                match a {
                    255 => d.copy_from_slice(s),
                    0 => d.copy_from_slice(&[0, 0, 0, 0]),
                    _ => {
                        for c in 0..3 {
                            d[c] = ((s[c] as u32 * 255 + a / 2) / a).min(255) as u8;
                        }
                        d[3] = a as u8;
                    }
                }
            }
        };
        let rows = &mut dst[y0 * w * 4..y1 * w * 4];
        if (x1 - x0) * (y1 - y0) >= 64 * 1024 {
            rows.par_chunks_mut(w * 4).enumerate().for_each(row);
        } else {
            rows.chunks_mut(w * 4).enumerate().for_each(row);
        }
    }
}

fn s(xs: &[&str]) -> Vec<String> { xs.iter().map(|x| x.to_string()).collect() }

const BT709: [&str; 8] = ["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv"];

/// One segment's ffmpeg: CFR rawvideo in, the segment's file out.
pub struct Sink {
    child: Child,
    stdin: Option<ChildStdin>,
    log: PathBuf,
    alpha: bool,
    /// --alpha without scaling: the straight-alpha frame, updated per damage
    straight: Option<Vec<u8>>,
    w: usize,
    h: usize,
    pub frames: u64,
    pub bytes: u64,
}

impl Sink {
    /// `first_frame`: number of the segment's first frame (PNG file numbering).
    pub fn start(ffmpeg: &Path, x: &ExportOpts, w: u32, h: u32, fps: u32, first_frame: usize, out: &Path, log: PathBuf) -> Result<Sink> {
        if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        // premultiplied scaling: scale the premultiplied frame, then unpremultiply in ffmpeg
        // (scaling straight alpha bleeds the black of transparent pixels into the edges)
        let ff_unpremul = x.alpha && x.width.is_some();
        let infmt = if x.alpha { "rgba" } else { "rgb0" };
        let mut a = s(&["-hide_banner", "-y", "-f", "rawvideo", "-pix_fmt", infmt, "-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "pipe:0", "-an"]);
        let mut vf: Vec<String> = Vec::new();
        if let Some(ow) = x.width {
            // zscale: swscale resamples packed RGBA through YUV (grey fringes on red edges)
            vf.push(format!("zscale=w={ow}:h=-2:filter=bicubic"));
            if ff_unpremul {
                vf.push("format=gbrap,unpremultiply=inplace=1".into());
            }
        }
        match x.format {
            Format::ProRes => {
                vf.push("scale=out_color_matrix=bt709:out_range=tv".into());
                a.extend(s(&["-vf", &vf.join(",")]));
                if x.alpha {
                    a.extend(s(&["-c:v", "prores_ks", "-profile:v", "4444", "-pix_fmt", "yuva444p10le", "-alpha_bits", "16"]));
                } else {
                    a.extend(s(&["-c:v", "prores_ks", "-profile:v", "3", "-pix_fmt", "yuv422p10le"]));
                }
                a.extend(s(&["-vendor", "apl0"]));
                a.extend(s(&BT709));
            }
            Format::Png => {
                if !vf.is_empty() {
                    a.extend(s(&["-vf", &vf.join(",")]));
                }
                a.extend(s(&["-c:v", "png", "-pix_fmt", if x.alpha { "rgba" } else { "rgb24" }, "-start_number", &first_frame.to_string(), "-f", "image2"]));
            }
            // lossless intermediate: straight RGBA (or RGB) as FFV1 BGRA / BGR0
            Format::Vp9 | Format::Gif | Format::Webp => {
                if !vf.is_empty() {
                    a.extend(s(&["-vf", &vf.join(",")]));
                }
                a.extend(s(&["-c:v", "ffv1", "-level", "3", "-slices", "4", "-pix_fmt", if x.alpha { "bgra" } else { "bgr0" }]));
            }
        }
        a.push(out.to_string_lossy().into_owned());
        let logf = std::fs::File::create(&log).with_context(|| format!("create {}", log.display()))?;
        let mut cmd = Command::new(ffmpeg);
        cmd.args(&a);
        writeln!(&logf, "agentfx-render: {cmd:?}")?;
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::from(logf)).spawn().context("start ffmpeg")?;
        let stdin = child.stdin.take();
        let (w, h) = (w as usize, h as usize);
        let straight = (x.alpha && !ff_unpremul).then(|| vec![0u8; w * h * 4]);
        Ok(Sink { child, stdin, log, alpha: x.alpha, straight, w, h, frames: 0, bytes: 0 })
    }

    /// Next frame (premultiplied RGBA); `dmg` are the rects changed since the previous
    /// one (the first call must cover the frame; empty = a repeat).
    pub fn frame(&mut self, premul: &[u8], dmg: &[[i32; 4]]) -> Result<()> {
        let buf: &[u8] = match self.straight.as_mut() {
            Some(st) if self.alpha => {
                unpremultiply(premul, st, self.w, self.h, dmg);
                st
            }
            _ => premul,
        };
        let r = self.stdin.as_mut().context("ffmpeg pipe closed")?.write_all(buf);
        if let Err(e) = r {
            self.stdin.take();
            let _ = self.child.wait();
            bail!("ffmpeg pipe failed: {e}; log tail: {}", log_tail(&self.log));
        }
        self.frames += 1;
        self.bytes += buf.len() as u64;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.stdin.take();
        let st = self.child.wait()?;
        ensure!(st.success(), "ffmpeg failed ({st}): {}", log_tail(&self.log));
        let _ = std::fs::remove_file(&self.log);
        Ok(())
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        self.stdin.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn log_tail(p: &Path) -> String {
    let t = std::fs::read_to_string(p).unwrap_or_default();
    let lines: Vec<&str> = t.lines().collect();
    lines[lines.len().saturating_sub(8)..].join("\n")
}

fn run_ffmpeg(ffmpeg: &Path, args: &[String], log: &Path) -> Result<()> {
    let logf = std::fs::File::create(log)?;
    let mut cmd = Command::new(ffmpeg);
    cmd.args(args);
    writeln!(&logf, "agentfx-render: {cmd:?}")?;
    let st = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(logf)).status().context("run ffmpeg")?;
    ensure!(st.success(), "ffmpeg failed ({st}): {}", log_tail(log));
    let _ = std::fs::remove_file(log);
    Ok(())
}

/// After all segments: join ProRes parts, or run the final encode of a staged format.
pub fn finish(ffmpeg: &Path, x: &ExportOpts, parts: &[PathBuf], out: &Path) -> Result<()> {
    let keep = agentfx_ir::env::var("AGENTFX_KEEP_SEGMENTS").is_ok();
    match x.format {
        Format::Png => return Ok(()),
        Format::ProRes => {
            if parts.len() > 1 {
                crate::render::concat(ffmpeg, parts, out)?;
            }
            return Ok(());
        }
        _ => {}
    }
    let list = sibling(out, "parts.txt");
    let body: String = parts.iter().map(|p| format!("file '{}'\n", std::fs::canonicalize(p).unwrap().display().to_string().replace('\'', "'\\''"))).collect();
    std::fs::write(&list, body)?;
    let input = |a: &mut Vec<String>| a.extend(s(&["-f", "concat", "-safe", "0", "-i"]).into_iter().chain([list.to_string_lossy().into_owned()]));
    let log = sibling(out, "ffmpeg.log");
    let fps = x.fps.map(|f| format!("fps={f}"));
    let mut a = s(&["-hide_banner", "-y"]);
    match x.format {
        Format::Vp9 => {
            input(&mut a);
            let mut vf: Vec<String> = fps.into_iter().collect();
            vf.push("scale=out_color_matrix=bt709:out_range=tv".into());
            let crf = x.quality.unwrap_or(30).to_string();
            a.extend(s(&["-vf", &vf.join(","), "-an", "-c:v", "libvpx-vp9", "-pix_fmt", if x.alpha { "yuva420p" } else { "yuv420p" }, "-crf", &crf, "-b:v", "0", "-deadline", "good", "-cpu-used", "4", "-row-mt", "1"]));
            // libvpx's alt-ref frames do not carry the alpha plane
            if x.alpha {
                a.extend(s(&["-auto-alt-ref", "0"]));
            }
            a.extend(s(&BT709));
            a.push(out.to_string_lossy().into_owned());
            run_ffmpeg(ffmpeg, &a, &log)?;
        }
        Format::Webp => {
            input(&mut a);
            if let Some(f) = &fps {
                a.extend(s(&["-vf", f]));
            }
            let q = x.quality.unwrap_or(80).to_string();
            let plays = match x.looping { Loop::Forever => 0, Loop::Times(n) => n };
            a.extend(s(&["-an", "-c:v", "libwebp_anim", "-pix_fmt", "bgra", "-lossless", if x.lossless { "1" } else { "0" }, "-quality", &q, "-loop", &plays.to_string()]));
            a.push(out.to_string_lossy().into_owned());
            run_ffmpeg(ffmpeg, &a, &log)?;
        }
        Format::Gif => {
            let pal = sibling(out, "palette.png");
            let pre = fps.map(|f| format!("{f},")).unwrap_or_default();
            // pass 1: one palette over all frames (one entry reserved for transparency)
            let mut p1 = a.clone();
            input(&mut p1);
            p1.extend(s(&["-vf", &format!("{pre}palettegen=reserve_transparent={}", x.alpha as u8), "-update", "1", "-frames:v", "1"]));
            p1.push(pal.to_string_lossy().into_owned());
            run_ffmpeg(ffmpeg, &p1, &log)?;
            // pass 2: dither against it; only changed rectangles are re-dithered
            input(&mut a);
            a.extend(s(&["-i"]).into_iter().chain([pal.to_string_lossy().into_owned()]));
            let alpha = if x.alpha { ":alpha_threshold=128" } else { "" };
            let loop_arg = match x.looping { Loop::Forever => 0, Loop::Times(1) => -1, Loop::Times(n) => n as i64 - 1 };
            a.extend(s(&["-lavfi", &format!("[0:v]{pre}null[v];[v][1:v]paletteuse=dither=sierra2_4a:diff_mode=rectangle{alpha}"), "-an", "-loop", &loop_arg.to_string()]));
            a.push(out.to_string_lossy().into_owned());
            let r = run_ffmpeg(ffmpeg, &a, &log);
            if !keep {
                let _ = std::fs::remove_file(&pal);
            }
            r?;
        }
        Format::ProRes | Format::Png => unreachable!(),
    }
    if !keep {
        let _ = std::fs::remove_file(&list);
        for p in parts {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpremultiply_rects_only() {
        let (w, h) = (4, 2);
        let mut src = vec![0u8; w * h * 4];
        // (0,0) opaque, (1,0) half alpha, (2,0) transparent, (3,0) quarter alpha
        src[0..4].copy_from_slice(&[10, 20, 30, 255]);
        src[4..8].copy_from_slice(&[64, 0, 128, 128]);
        src[12..16].copy_from_slice(&[16, 32, 64, 64]);
        let mut dst = vec![7u8; w * h * 4];
        unpremultiply(&src, &mut dst, w, h, &[[0, 0, 4, 1]]);
        assert_eq!(&dst[0..4], &[10, 20, 30, 255]);
        assert_eq!(&dst[4..8], &[128, 0, 255, 128]);
        assert_eq!(&dst[8..12], &[0, 0, 0, 0]);
        assert_eq!(&dst[12..16], &[64, 128, 255, 64]);
        // the second row is outside the damage and untouched
        assert!(dst[16..].iter().all(|&v| v == 7));
        // out-of-frame and empty rects are ignored
        unpremultiply(&src, &mut dst, w, h, &[[-5, -5, 2, 2], [3, 1, 0, 1], [10, 0, 4, 4]]);
        assert!(dst[16..].iter().all(|&v| v == 7));
    }

    #[test]
    fn paths_and_formats() {
        assert_eq!(Format::from_out(Path::new("a/b.mp4")).unwrap(), None);
        assert_eq!(Format::from_out(Path::new("a/b.MOV")).unwrap(), Some(Format::ProRes));
        assert!(Format::from_out(Path::new("a/b.avi")).is_err());
        assert_eq!(png_pattern(Path::new("f/x.png")), PathBuf::from("f/x_%05d.png"));
        assert_eq!(png_pattern(Path::new("f/x%04d.png")), PathBuf::from("f/x%04d.png"));
        assert_eq!(Loop::parse("once").unwrap(), Loop::Times(1));
        assert_eq!(Loop::parse("forever").unwrap(), Loop::Forever);
        assert!(Loop::parse("-3").is_err());
        let x = ExportOpts { format: Format::Gif, alpha: true, fps: None, width: None, looping: Loop::Forever, quality: None, lossless: false };
        assert_eq!(x.segment_out(Path::new("o/v.gif"), 1, true), PathBuf::from("o/v.part01.mkv"));
    }
}

/// Alpha round trip: render a small transparent scene to every format, decode the
/// result with ffmpeg and compare alpha (and colour) at a transparent pixel, an opaque
/// shape, a 50 % alpha fill and a 50 % coverage edge. Skipped without ffmpeg
/// (`AGENTFX_FFMPEG` or `PATH`).
#[cfg(test)]
mod roundtrip {
    use super::*;
    use crate::render::{self, BackendKind, RenderOpts};
    use crate::{composite, encode, schedule, vraster};

    const W: usize = 96;
    const H: usize = 64;
    const FRAMES: usize = 12;

    fn scene_json() -> String {
        let rect = |x: f64, y: f64, w: f64, h: f64, c: &str| format!(r#"{{"type":"rect","rect":{{"rect":[{x},{y},{w},{h}],"radius":[0,0,0,0]}},"color":{c}}}"#);
        let nodes = [
            r#"{"name":"root","children":[1,2,3,4]}"#.to_string(),
            format!(r#"{{"name":"box","content":{}}}"#, rect(8.0, 8.0, 24.0, 24.0, "[1,0,0,1]")),
            format!(r#"{{"name":"half","content":{}}}"#, rect(40.0, 8.0, 16.0, 16.0, "[0,1,0,0.5]")),
            // x = 64.5: column 64 is half covered
            format!(r#"{{"name":"edge","content":{}}}"#, rect(64.5, 8.0, 16.0, 16.0, "[1,1,1,1]")),
            format!(r#"{{"name":"mover","content":{}}}"#, rect(8.0, 44.0, 12.0, 12.0, "[0,0,1,1]")),
        ];
        let track = r#"{"node":4,"property":"transform","delay":0.2,"duration":0.6,"fill":"both","easing":"linear","keyframes":[{"offset":0,"value":{"transform":[{"translate":[0,0]}]}},{"offset":1,"value":{"transform":[{"translate":[60,0]}]}}]}"#;
        format!(r#"{{"version":1,"width":{W},"height":{H},"fps":10,"frames":{FRAMES},"nodes":[{}],"tracks":[{track}]}}"#, nodes.join(","))
    }

    /// ffmpeg with all of `encoders`, else None after a skip note.
    fn ffmpeg_with(encoders: &[&str]) -> Option<PathBuf> {
        let Some(ff) = encode::find_ffmpeg() else {
            eprintln!("skip: ffmpeg not found (set AGENTFX_FFMPEG or put it on PATH)");
            return None;
        };
        let Ok(out) = std::process::Command::new(&ff).args(["-hide_banner", "-encoders"]).output() else {
            eprintln!("skip: cannot run {}", ff.display());
            return None;
        };
        let list = String::from_utf8_lossy(&out.stdout);
        let missing: Vec<&str> = encoders.iter().copied().filter(|e| !list.split_whitespace().any(|w| w == *e)).collect();
        if !missing.is_empty() {
            eprintln!("skip: {} lacks encoder(s) {}", ff.display(), missing.join(", "));
            return None;
        }
        Some(ff)
    }

    struct Ctx {
        scene: agentfx_ir::Scene,
        dir: PathBuf,
    }

    impl Ctx {
        fn new(tag: &str) -> Ctx {
            let scene: agentfx_ir::Scene = serde_json::from_str(&scene_json()).unwrap();
            let dir = std::env::temp_dir().join(format!("reel-alpha-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Ctx { scene, dir }
        }

        fn render(&self, name: &str, x: ExportOpts, segments: usize) -> PathBuf {
            let base = Path::new(env!("CARGO_MANIFEST_DIR"));
            let prep = vraster::Prepared::new(&self.scene, base, None, None).unwrap();
            let info = vraster::Info::new(&self.scene, &prep);
            let sched = schedule::build(&self.scene, &info).unwrap();
            let out = self.dir.join(name);
            let opts = RenderOpts {
                out: out.clone(), mode: encode::Mode::Vfr, backend: BackendKind::Vello, filter: composite::Filter::Bilinear,
                mips: true, lossless: None, segments, threads: 0, preset: "veryfast".into(), crf: 18, x264_threads: 0, yuv: true,
                codec: encode::Codec::X264, hw: encode::HwOpts::default(), no_encode: false, export: Some(x),
            };
            let st = render::render(&self.scene, base, &prep, &info, &sched, &opts).unwrap();
            assert_eq!(st["frames_sent"].as_u64().unwrap(), FRAMES as u64, "{st}");
            out
        }

        /// Premultiplied full render of frame f (reference for the damage path).
        fn reference(&self, f: usize) -> Vec<u8> {
            let base = Path::new(env!("CARGO_MANIFEST_DIR"));
            let prep = vraster::Prepared::new(&self.scene, base, None, None).unwrap();
            let info = vraster::Info::new(&self.scene, &prep);
            let sched = schedule::build(&self.scene, &info).unwrap();
            let mut p = vraster::Painter::new(&self.scene, &prep, &sched, &info, 0);
            p.render_rect(f, [0, 0, W as i32, H as i32]).unwrap().data_as_u8_slice().to_vec()
        }
    }

    impl Drop for Ctx {
        fn drop(&mut self) {
            if let Ok(rd) = std::fs::read_dir(&self.dir) {
                for e in rd.flatten() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            let _ = std::fs::remove_dir(&self.dir);
        }
    }

    /// Decode frame `n` of `path` to straight RGBA at the scene size (or `w` x `h`).
    fn decode(ff: &Path, path: &Path, n: usize, vp9: bool, (w, h): (usize, usize)) -> Vec<u8> {
        let mut c = std::process::Command::new(ff);
        c.args(["-v", "error"]);
        if vp9 {
            // ffmpeg's native VP9 decoder ignores the alpha side channel
            c.args(["-c:v", "libvpx-vp9"]);
        }
        let o = c.arg("-i").arg(path).args(["-vf", &format!("select=eq(n\\,{n}),format=rgba"), "-frames:v", "1", "-f", "rawvideo", "-"]).output().unwrap();
        assert!(o.status.success(), "decode {}: {}", path.display(), String::from_utf8_lossy(&o.stderr));
        assert_eq!(o.stdout.len(), w * h * 4, "decoded size of {}", path.display());
        o.stdout
    }

    fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    /// Straight-alpha checks; `tol` per channel, `soft`: compare 50 % alpha (else GIF:
    /// alpha is 0 or 255).
    fn check(buf: &[u8], tol: u8, soft: bool, what: &str) {
        let near = |got: [u8; 4], want: [u8; 4], rgb: bool, at: &str| {
            let n = if rgb { 4 } else { 1 };
            for c in (0..4).rev().take(n) {
                assert!(got[c].abs_diff(want[c]) <= tol, "{what} {at}: got {got:?}, want {want:?} (tol {tol})");
            }
        };
        assert_eq!(px(buf, W, 2, 2)[3], 0, "{what}: background must be transparent");
        assert_eq!(px(buf, W, 90, 60)[3], 0, "{what}: background must be transparent");
        near(px(buf, W, 20, 20), [255, 0, 0, 255], true, "opaque box");
        near(px(buf, W, 72, 16), [255, 255, 255, 255], true, "opaque edge rect");
        if soft {
            near(px(buf, W, 48, 16), [0, 255, 0, 128], true, "50% fill");
            near(px(buf, W, 64, 16), [0, 0, 0, 128], false, "50% coverage edge");
        } else {
            for (x, y) in [(48, 16), (64, 16)] {
                let a = px(buf, W, x, y)[3];
                assert!(a == 0 || a == 255, "{what}: GIF alpha must be binary, got {a}");
            }
        }
    }

    fn opts(format: Format, alpha: bool) -> ExportOpts {
        ExportOpts { format, alpha, fps: None, width: None, looping: Loop::Forever, quality: None, lossless: false }
    }

    #[test]
    fn png_sequence_matches_full_render() {
        let Some(ff) = ffmpeg_with(&["png"]) else { return };
        let c = Ctx::new("png");
        // two segments write their own frame numbers into one sequence
        let pat = c.render("f.png", opts(Format::Png, true), 2);
        assert_eq!(pat, c.dir.join("f.png"));
        let files: Vec<_> = (0..FRAMES).map(|i| c.dir.join(format!("f_{i:05}.png"))).collect();
        assert!(files.iter().all(|f| f.exists()), "missing PNG frames");
        assert!(!c.dir.join(format!("f_{FRAMES:05}.png")).exists());
        for f in [0, 5, FRAMES - 1] {
            let got = decode(&ff, &files[f], 0, false, (W, H));
            check(&got, 0, true, "png");
            // incremental damage + unpremultiply == straight version of a fresh full render
            let want_pm = c.reference(f);
            let mut want = vec![0u8; W * H * 4];
            unpremultiply(&want_pm, &mut want, W, H, &[[0, 0, W as i32, H as i32]]);
            assert!(got == want, "png frame {f} differs from the full render");
        }
    }

    #[test]
    fn prores_4444_alpha() {
        let Some(ff) = ffmpeg_with(&["prores_ks", "ffv1"]) else { return };
        let c = Ctx::new("mov");
        for segs in [1, 3] {
            let out = c.render(&format!("a{segs}.mov"), opts(Format::ProRes, true), segs);
            assert!(!c.dir.join(format!("a{segs}.seg00.mov")).exists(), "segments joined and removed");
            for f in [0, FRAMES - 1] {
                check(&decode(&ff, &out, f, false, (W, H)), 3, true, "prores");
            }
        }
    }

    #[test]
    fn vp9_alpha() {
        let Some(ff) = ffmpeg_with(&["libvpx-vp9", "ffv1"]) else { return };
        let c = Ctx::new("webm");
        let mut x = opts(Format::Vp9, true);
        x.quality = Some(10);
        let out = c.render("a.webm", x, 2);
        assert!(!c.dir.join("a.part00.mkv").exists(), "FFV1 parts removed");
        for f in [0, FRAMES - 1] {
            check(&decode(&ff, &out, f, true, (W, H)), 12, true, "vp9");
        }
    }

    #[test]
    fn webp_and_gif_alpha() {
        let Some(ff) = ffmpeg_with(&["libwebp_anim", "gif", "ffv1"]) else { return };
        let c = Ctx::new("anim");
        let mut x = opts(Format::Webp, true);
        x.lossless = true;
        let out = c.render("a.webp", x, 2);
        // ffmpeg 7 cannot decode animated WebP (ANIM chunks); check the container instead
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"RIFF") && bytes.windows(4).any(|w| w == b"ANIM") && bytes.windows(4).any(|w| w == b"ALPH" || w == b"VP8L"), "animated WebP with alpha");
        // ... and decode it with Pillow when present (libwebp merges identical frames)
        let py = "import sys\nfrom PIL import Image\nim=Image.open(sys.argv[1]);im.seek(im.n_frames-1)\nsys.stdout.buffer.write(im.convert('RGBA').tobytes())";
        if let Ok(o) = std::process::Command::new("python3").args(["-c", py]).arg(&out).output() {
            if o.status.success() && o.stdout.len() == W * H * 4 {
                check(&o.stdout, 2, true, "webp lossless");
            } else {
                eprintln!("webp decode skipped (no Pillow): {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""));
            }
        }
        let out = c.render("a.gif", opts(Format::Gif, true), 2);
        for f in [0, FRAMES - 1] {
            check(&decode(&ff, &out, f, false, (W, H)), 24, false, "gif");
        }
    }

    #[test]
    fn scaled_alpha_is_premultiplied() {
        let Some(ff) = ffmpeg_with(&["png"]) else { return };
        let c = Ctx::new("scaled");
        let mut x = opts(Format::Png, true);
        x.width = Some(48);
        c.render("s.png", x, 1);
        let got = decode(&ff, &c.dir.join("s_00000.png"), 0, false, (48, 32));
        assert_eq!(px(&got, 48, 1, 1)[3], 0);
        let red = px(&got, 48, 10, 10);
        assert!(red[0] >= 253 && red[3] >= 253, "opaque box {red:?}");
        // scaling straight alpha would pull the transparent black into the edges: the
        // half-covered column of the box stays red, only its alpha drops
        let e = px(&got, 48, 3, 10);
        assert!(e[3] > 8 && e[3] < 247 && e[0] >= 245 && e[1] <= 8, "box edge {e:?}");
        let g = px(&got, 48, 24, 8);
        assert!(g[1] >= 245 && g[3].abs_diff(128) <= 3, "50% fill {g:?}");
    }

    #[test]
    fn opaque_export_is_composited_over_black() {
        let Some(ff) = ffmpeg_with(&["png"]) else { return };
        let c = Ctx::new("opaque");
        let mut x = opts(Format::Png, false);
        x.width = Some(48);
        c.render("o.png", x, 1);
        let got = decode(&ff, &c.dir.join("o_00000.png"), 0, false, (48, 32));
        assert_eq!(px(&got, 48, 1, 1), [0, 0, 0, 255]);
        assert_eq!(px(&got, 48, 10, 10), [255, 0, 0, 255]);
    }
}
