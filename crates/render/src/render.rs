//! Core renderer: IR scene + schedule -> frames (damaged rects only) -> encoder.
//!
//! Backends (trait `Backend`): `vello` (native vello_cpu painter, all IR content, effects)
//! and `tiny-skia` (the M0 raster compositor; raster contents only, kept as reference /
//! fallback). Parallelism: the frame range is split into segments (preferring
//! full-damage frames as cut points); each segment renders and encodes independently
//! and the MP4 segments are concatenated with `-c copy`.
use crate::composite::{Clip, Compositor, Filter, IRect, LayerImage, Node};
use crate::encode;
use crate::schedule::Schedule;
use crate::vraster::{Info, Painter, Prepared};
use anyhow::{Context, Result, bail};
use agentfx_ir::{Affine, BlendMode, Content, Scene};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Vello,
    TinySkia,
}

#[derive(Clone, Debug)]
pub struct RenderOpts {
    pub out: PathBuf,
    pub mode: encode::Mode,
    pub backend: BackendKind,
    /// tiny-skia only
    pub filter: Filter,
    pub mips: bool,
    pub lossless: Option<PathBuf>,
    /// number of parallel segments (render + encode each)
    pub segments: usize,
    /// vello_cpu worker threads per segment (0 = single-threaded)
    pub threads: u16,
    pub preset: String,
    pub crf: u32,
    /// x264 threads per segment (0 = x264 default)
    pub x264_threads: u32,
    /// convert to yuv420p in Rust (else ffmpeg converts RGBA)
    pub yuv: bool,
    /// encoder (x264 or hardware, resolved: never "auto" here)
    pub codec: encode::Codec,
    /// hardware encoder settings
    pub hw: encode::HwOpts,
    /// render only, no encoder (benchmark)
    pub no_encode: bool,
    /// non-MP4 deliverable (ProRes, VP9, PNG, GIF, WebP; `export.rs`); None = MP4 encoder
    pub export: Option<crate::export::ExportOpts>,
}

/// A backend writes the pixels of `rects` of frame `f` into `frame` (RGBA, premultiplied).
pub trait Backend {
    fn ctx_created(&self) -> usize { 0 }
    /// world canvas tile cache stats (X9), when a world was drawn
    fn world(&self) -> Option<crate::vraster::world_paint::WorldStats> { None }

    fn render(&mut self, f: usize, rects: &[[i32; 4]], frame: &mut [u8]) -> Result<()>;
}

pub struct VelloBackend<'a> {
    pub painter: Painter<'a>,
    width: usize,
}

impl Backend for VelloBackend<'_> {
    fn ctx_created(&self) -> usize { self.painter.stats.ctx_created }
    fn world(&self) -> Option<crate::vraster::world_paint::WorldStats> {
        let w = self.painter.world_stats;
        (w.tile_frames + w.tiles_rastered > 0).then_some(w)
    }

    fn render(&mut self, f: usize, rects: &[[i32; 4]], frame: &mut [u8]) -> Result<()> {
        for &r in rects {
            let px = self.painter.render_rect(f, r)?;
            let src = px.data_as_u8_slice();
            let rw = r[2] as usize * 4;
            for y in 0..r[3] as usize {
                let d = ((r[1] as usize + y) * self.width + r[0] as usize) * 4;
                frame[d..d + rw].copy_from_slice(&src[y * rw..y * rw + rw]);
            }
        }
        Ok(())
    }
}

fn blend(b: BlendMode) -> tiny_skia::BlendMode {
    use tiny_skia::BlendMode as T;
    match b {
        BlendMode::Normal => T::SourceOver,
        BlendMode::Multiply => T::Multiply,
        BlendMode::Screen => T::Screen,
        BlendMode::Overlay => T::Overlay,
        BlendMode::Darken => T::Darken,
        BlendMode::Lighten => T::Lighten,
        BlendMode::Plus => T::Plus,
        BlendMode::ColorDodge => T::ColorDodge,
        BlendMode::ColorBurn => T::ColorBurn,
        BlendMode::HardLight => T::HardLight,
        BlendMode::SoftLight => T::SoftLight,
        BlendMode::Difference => T::Difference,
        BlendMode::Exclusion => T::Exclusion,
        BlendMode::Hue => T::Hue,
        BlendMode::Saturation => T::Saturation,
        BlendMode::Color => T::Color,
        BlendMode::Luminosity => T::Luminosity,
        BlendMode::SrcAtop => T::SourceAtop,
    }
}

/// M0 compositor: images per asset plus rasterized Rect contents.
pub struct TinySkiaBackend<'a> {
    scene: &'a Scene,
    sched: &'a Schedule,
    list: Vec<LayerImage>,
    asset: Vec<usize>,
    rect_node: Vec<Option<usize>>,
    comp: Compositor,
}

impl<'a> TinySkiaBackend<'a> {
    pub fn new(scene: &'a Scene, base: &Path, sched: &'a Schedule, filter: Filter, mips: bool) -> Result<Self> {
        let mut asset_rect: Vec<Option<[f64; 4]>> = vec![None; scene.assets.len()];
        for n in &scene.nodes {
            if n.hidden || !n.effects.is_empty() || n.motion_blur.is_some() || n.time.is_some() {
                bail!("tiny-skia backend: node {:?} uses effects/motion blur/hidden (use --backend vello)", n.name);
            }
            match &n.content {
                Some(Content::Image { asset, rect, variants }) => {
                    asset_rect[*asset] = Some(*rect);
                    for v in variants {
                        asset_rect[*v] = Some(*rect);
                    }
                }
                Some(Content::Sequence { rect, frames }) => {
                    for (_, a) in frames {
                        asset_rect[*a] = Some(*rect);
                    }
                }
                Some(Content::Rect { .. }) | None => {}
                Some(_) => bail!("tiny-skia backend renders raster IR only (node {:?}); use --backend vello", n.name),
            }
        }
        let mut list = Vec::new();
        let mut asset = Vec::new();
        for (i, a) in scene.assets.iter().enumerate() {
            let rect = asset_rect[i].with_context(|| format!("asset {i} ({}) unused", a.path))?;
            let img = LayerImage::load_png(&base.join(&a.path), rect)?;
            asset.push(list.len());
            list.push(img);
        }
        let mut rect_node = vec![None; scene.nodes.len()];
        for (i, n) in scene.nodes.iter().enumerate() {
            if let Some(Content::Rect { rect, color }) = &n.content {
                let s = ((sched.max_scale[i].max(1.0) * 4.0).ceil() / 4.0).min(4.0) as f32;
                let r = rect.rect;
                let (pw, ph) = (((r[2] as f32) * s).ceil().max(1.0) as u32, ((r[3] as f32) * s).ceil().max(1.0) as u32);
                let mut pm = tiny_skia::Pixmap::new(pw, ph).context("rect pixmap")?;
                let mut paint = tiny_skia::Paint::default();
                paint.set_color(tiny_skia::Color::from_rgba(color[0] as f32, color[1] as f32, color[2] as f32, color[3] as f32).context("bad color")?);
                paint.anti_alias = true;
                let path = rrect_path(0.0, 0.0, r[2] as f32 * s, r[3] as f32 * s, rect.radius.map(|x| x as f32 * s));
                pm.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
                rect_node[i] = Some(list.len());
                list.push(LayerImage::from_pixmap(pm, [r[0], r[1], pw as f64 / s as f64, ph as f64 / s as f64]));
            }
        }
        Ok(Self { scene, sched, list, asset, rect_node, comp: Compositor::new(scene.width, scene.height, filter, mips) })
    }

    fn draw_node(&self, f: usize, n: usize, out: &mut Vec<Node>) {
        let s = self.sched;
        let st = &s.states[f][n];
        if st.opacity <= 0.0 {
            return;
        }
        let node = &self.scene.nodes[n];
        let mut items: Vec<Node> = Vec::new();
        let image = match &node.content {
            Some(Content::Image { .. }) | Some(Content::Sequence { .. }) => s.content[f][n].map(|a| self.asset[a]),
            Some(Content::Rect { .. }) => self.rect_node[n],
            _ => None,
        };
        if let Some(image) = image {
            items.push(Node::Image { image, transform: st.chain, opacity: 1.0, clips: vec![], blend: tiny_skia::BlendMode::SourceOver });
        }
        for &c in &node.children {
            self.draw_node(f, c, &mut items);
        }
        if items.is_empty() {
            return;
        }
        let pchain = s.parents[n].map(|p| s.states[f][p].chain).unwrap_or(Affine::IDENTITY);
        let clips: Vec<Clip> = node.clips.iter().map(|rr| Clip { rect: rr.rect, radius: rr.radius, transform: pchain }).collect();
        let opacity = st.opacity as f32;
        let bm = blend(node.blend);
        if items.len() == 1 {
            let mut it = items.pop().unwrap();
            match &mut it {
                Node::Image { opacity: o, clips: c, blend: b, .. } | Node::Group { opacity: o, clips: c, blend: b, .. } => {
                    *o *= opacity;
                    c.extend(clips);
                    if *b == tiny_skia::BlendMode::SourceOver {
                        *b = bm;
                    }
                }
            }
            out.push(it);
        } else if !node.isolate && clips.is_empty() && bm == tiny_skia::BlendMode::SourceOver {
            for mut it in items {
                match &mut it {
                    Node::Image { opacity: o, .. } | Node::Group { opacity: o, .. } => *o *= opacity,
                }
                out.push(it);
            }
        } else {
            out.push(Node::Group { opacity, clips, children: items, blend: bm });
        }
    }
}

impl Backend for TinySkiaBackend<'_> {
    fn render(&mut self, f: usize, rects: &[[i32; 4]], frame: &mut [u8]) -> Result<()> {
        let mut nodes = Vec::new();
        self.draw_node(f, 0, &mut nodes);
        let rs: Vec<IRect> = rects.iter().map(|r| IRect { x: r[0], y: r[1], w: r[2] as u32, h: r[3] as u32 }).collect();
        self.comp.compose(&nodes, &self.list, &rs);
        let rgba = self.comp.frame_rgba();
        frame.copy_from_slice(&rgba);
        Ok(())
    }
}

fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: [f32; 4]) -> tiny_skia::Path {
    let f = 1.0f32.min(w / (r[0] + r[1]).max(1e-6)).min(w / (r[3] + r[2]).max(1e-6)).min(h / (r[0] + r[3]).max(1e-6)).min(h / (r[1] + r[2]).max(1e-6));
    let [tl, tr, br, bl] = r.map(|v| v * f);
    let k = 0.552_284_8;
    let mut b = tiny_skia::PathBuilder::new();
    b.move_to(x + tl, y);
    b.line_to(x + w - tr, y);
    b.cubic_to(x + w - tr + k * tr, y, x + w, y + tr - k * tr, x + w, y + tr);
    b.line_to(x + w, y + h - br);
    b.cubic_to(x + w, y + h - br + k * br, x + w - br + k * br, y + h, x + w - br, y + h);
    b.line_to(x + bl, y + h);
    b.cubic_to(x + bl - k * bl, y + h, x, y + h - bl + k * bl, x, y + h - bl);
    b.line_to(x, y + tl);
    b.cubic_to(x, y + tl - k * tl, x + tl - k * tl, y, x + tl, y);
    b.close();
    b.finish().expect("rect path")
}

/// Contiguous frame ranges of roughly equal cost; boundaries snap to full-damage frames
/// within 15 % of a segment length (a cut there costs no extra keyframe bits).
pub fn plan_segments(scene: &Scene, sched: &Schedule, k: usize) -> Vec<(usize, usize)> {
    let n = sched.frames;
    let k = k.clamp(1, n.max(1));
    if k == 1 {
        return vec![(0, n)];
    }
    let full = (scene.width * scene.height) as f64;
    // cost: encode of a sent frame ~ 1, render ~ damage fraction * 2
    let cost: Vec<f64> = (0..n).map(|f| {
        let a: f64 = sched.damage[f].iter().map(|r| (r[2] * r[3]) as f64).sum::<f64>() / full;
        if a == 0.0 { 0.05 } else { 1.0 + 2.0 * a }
    }).collect();
    let total: f64 = cost.iter().sum();
    let cuts_full = sched.full_frames(scene.width, scene.height);
    let mut bounds = vec![0];
    let mut acc = 0.0;
    let mut next = total / k as f64;
    for f in 0..n {
        acc += cost[f];
        if acc >= next && bounds.len() < k && f + 1 < n {
            let mut cut = f + 1;
            let win = (n / k).max(1) * 15 / 100;
            if let Some(&c) = cuts_full.iter().filter(|&&c| c > *bounds.last().unwrap() && (c as i64 - cut as i64).unsigned_abs() as usize <= win).min_by_key(|&&c| (c as i64 - cut as i64).abs()) {
                cut = c;
            }
            if cut > *bounds.last().unwrap() {
                bounds.push(cut);
            }
            next += total / k as f64;
        }
    }
    bounds.push(n);
    bounds.windows(2).map(|w| (w[0], w[1])).filter(|(a, b)| b > a).collect()
}

#[derive(Default, Debug, Clone)]
struct SegStats {
    raster_s: f64,
    convert_s: f64,
    encode_s: f64,
    composited: usize,
    repeated: usize,
    dirty_px: u64,
    sent: u64,
    bytes: u64,
    offscreen: usize,
    ctx_created: usize,
    world: Option<crate::vraster::world_paint::WorldStats>,
    /// AGENTFX_PROFILE: stage totals and full-damage frames of this segment
    prof: std::collections::HashMap<&'static str, crate::prof::Acc>,
    full: usize,
}

fn segment_path(out: &Path, i: usize, ext: &str) -> PathBuf {
    let stem = out.file_stem().unwrap().to_string_lossy();
    out.with_file_name(format!("{stem}.seg{i:02}.{ext}"))
}

#[allow(clippy::too_many_arguments)]
fn render_segment(scene: &Scene, base: &Path, prep: &Prepared, info: &Info, sched: &Schedule, opts: &RenderOpts, seg: (usize, usize), out: PathBuf, lossless: Option<PathBuf>, log: PathBuf) -> Result<SegStats> {
    let (w, h) = (scene.width as usize, scene.height as usize);
    let alpha = opts.export.as_ref().is_some_and(|x| x.alpha);
    let mut backend: Box<dyn Backend> = match opts.backend {
        BackendKind::Vello => {
            let mut painter = Painter::new(scene, prep, sched, info, opts.threads);
            // subpixel text needs an opaque backdrop
            if alpha {
                painter.lcd = false;
            }
            Box::new(VelloBackend { painter, width: w })
        }
        BackendKind::TinySkia => Box::new(TinySkiaBackend::new(scene, base, sched, opts.filter, opts.mips)?),
    };
    let ffmpeg = encode::ffmpeg();
    let mut sink = match (&opts.export, opts.no_encode) {
        (Some(x), false) => Some(crate::export::Sink::start(&ffmpeg, x, scene.width, scene.height, scene.fps, seg.0, &out, log)?),
        _ => None,
    };
    let mut enc = if opts.no_encode || opts.export.is_some() {
        None
    } else {
        Some(encode::Encoder::start(encode::EncoderOpts {
            ffmpeg: ffmpeg.clone(),
            out: out.clone(),
            width: scene.width,
            height: scene.height,
            fps: scene.fps,
            mode: opts.mode,
            crf: opts.crf,
            preset: opts.preset.clone(),
            log: out.with_extension("ffmpeg.log"),
            codec: opts.codec,
            hw: opts.hw.clone(),
            threads: opts.x264_threads,
            yuv: opts.yuv,
            frames: (seg.1 - seg.0) as u64,
        })?)
    };
    let mut ll = match &lossless {
        Some(path) => Some(
            std::process::Command::new(&ffmpeg)
                .args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s"])
                .arg(format!("{}x{}", scene.width, scene.height))
                .arg("-r")
                .arg(scene.fps.to_string())
                .args(["-i", "-", "-c:v", "ffv1", "-pix_fmt", "bgr0"])
                .arg(path)
                .stdin(std::process::Stdio::piped())
                .spawn()?,
        ),
        None => None,
    };
    let mut frame = vec![0u8; w * h * 4];
    let mut st = SegStats::default();
    let mut i420 = (opts.yuv && enc.is_some()).then(|| encode::I420Frame::for_codec(opts.codec, &opts.hw, w, h));
    let prof_on = crate::prof::on();
    let mut prof_out = match agentfx_ir::env::var("AGENTFX_PROFILE_OUT") {
        Ok(p) if prof_on => Some(std::io::BufWriter::new(std::fs::OpenOptions::new().create(true).append(true).open(p)?)),
        _ => None,
    };
    if prof_on {
        // setup before the first frame (painter, encoder start) is not a frame stage
        let _ = crate::prof::take();
    }
    // A/B checks: AGENTFX_DUMP=path appends every rendered frame (RGBA8 premultiplied, raw)
    let mut dump = match agentfx_ir::env::var("AGENTFX_DUMP") {
        Ok(p) => Some(std::io::BufWriter::new(std::fs::OpenOptions::new().create(true).append(true).open(p)?)),
        Err(_) => None,
    };
    // profiling: AGENTFX_SAMPLE=k renders every k-th frame in full (with --no-encode)
    let sample: usize = agentfx_ir::env::var("AGENTFX_SAMPLE").ok().and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    if sample > 1 && enc.is_some() {
        bail!("AGENTFX_SAMPLE needs --no-encode");
    }
    for f in (seg.0..seg.1).step_by(sample) {
        let tf = Instant::now();
        let local = (f - seg.0) as u64;
        let full = [[0, 0, w as i32, h as i32]];
        let dmg: &[[i32; 4]] = if f == seg.0 || sample > 1 { &full } else { &sched.damage[f] };
        if dmg.is_empty() {
            if let Some(s) = sink.as_mut() {
                let b = Instant::now();
                s.frame(&frame, &[])?;
                st.encode_s += b.elapsed().as_secs_f64();
            }
            if let Some(e) = enc.as_mut() {
                let b = Instant::now();
                e.repeat(local)?;
                st.encode_s += b.elapsed().as_secs_f64();
            }
            st.repeated += 1;
        } else {
            let a = Instant::now();
            {
                let _p = crate::prof::scope_px("frame", dmg.iter().map(|r| r[2] as u64 * r[3] as u64).sum());
                backend.render(f, dmg, &mut frame)?;
            }
            st.raster_s += a.elapsed().as_secs_f64();
            st.dirty_px += dmg.iter().map(|r| r[2] as u64 * r[3] as u64).sum::<u64>();
            if let Some(e) = enc.as_mut() {
                if opts.yuv {
                    let c = Instant::now();
                    let i420 = i420.as_mut().expect("I420 initialized for YUV encoding");
                    {
                        let _p = crate::prof::scope_px("convert.nv12", dmg.iter().map(|r| r[2] as u64 * r[3] as u64).sum());
                        i420.update(&frame, dmg);
                    }
                    st.convert_s += c.elapsed().as_secs_f64();
                    let b = Instant::now();
                    let _p = crate::prof::scope("encode.pipe");
                    e.frame(local, i420.data())?;
                    st.encode_s += b.elapsed().as_secs_f64();
                } else {
                    let b = Instant::now();
                    let _p = crate::prof::scope("encode.pipe");
                    e.frame(local, &frame)?;
                    st.encode_s += b.elapsed().as_secs_f64();
                }
            }
            if let Some(s) = sink.as_mut() {
                let b = Instant::now();
                let _p = crate::prof::scope("export.pipe");
                s.frame(&frame, dmg)?;
                st.encode_s += b.elapsed().as_secs_f64();
            }
            st.composited += 1;
        }
        crate::jobstatus::frame_done();
        if let Some(l) = ll.as_mut() {
            l.stdin.as_mut().unwrap().write_all(&frame)?;
        }
        if let Some(d) = dump.as_mut() {
            d.write_all(&frame)?;
        }
        if prof_on {
            let fr = crate::prof::take();
            let area: u64 = dmg.iter().map(|r| r[2] as u64 * r[3] as u64).sum();
            let full_dmg = area >= (w * h) as u64;
            st.full += full_dmg as usize;
            if let Some(o) = prof_out.as_mut() {
                let line = serde_json::json!({"f": f, "ms": (tf.elapsed().as_secs_f64() * 1e5).round() / 100.0, "dmg_px": area, "full": full_dmg, "stages": crate::prof::to_json(&fr)});
                writeln!(o, "{line}")?;
            }
            crate::prof::merge(&mut st.prof, &fr);
        }
    }
    if let Some(e) = enc {
        let b = Instant::now();
        let es = e.finish(seg.1 as u64 - seg.0 as u64)?;
        st.encode_s += b.elapsed().as_secs_f64();
        st.sent = es.frames_sent;
        st.bytes = es.bytes_piped;
    }
    if let Some(s) = sink {
        let b = Instant::now();
        (st.sent, st.bytes) = (s.frames, s.bytes);
        s.finish()?;
        st.encode_s += b.elapsed().as_secs_f64();
    }
    if let Some(mut l) = ll {
        drop(l.stdin.take());
        if !l.wait()?.success() {
            bail!("lossless ffmpeg failed");
        }
    }
    st.ctx_created = backend.ctx_created();
    st.world = backend.world();
    Ok(st)
}

pub(crate) fn concat(ffmpeg: &Path, parts: &[PathBuf], out: &Path) -> Result<()> {
    let list = out.with_extension("concat.txt");
    // ffmpeg concat syntax: single-quoted, a quote is written as '\''
    let body: String = parts.iter().map(|p| format!("file '{}'\n", std::fs::canonicalize(p).unwrap().display().to_string().replace('\'', "'\\''"))).collect();
    std::fs::write(&list, body)?;
    let st = std::process::Command::new(ffmpeg).args(["-v", "error", "-y", "-f", "concat", "-safe", "0", "-i"]).arg(&list).args(["-c", "copy"]).arg(out).status()?;
    if !st.success() {
        bail!("concat of {} segments failed", parts.len());
    }
    if agentfx_ir::env::var("AGENTFX_KEEP_SEGMENTS").is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&list);
    for p in parts {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(p.with_extension("ffmpeg.log"));
    }
    Ok(())
}

pub fn render(scene: &Scene, base: &Path, prep: &Prepared, info: &Info, sched: &Schedule, opts: &RenderOpts) -> Result<serde_json::Value> {
    let t0 = Instant::now();
    let mut segs = plan_segments(scene, sched, opts.segments);
    // debugging/profiling: AGENTFX_RANGE=a:b renders only frames [a, b) (one segment)
    if let Ok(r) = agentfx_ir::env::var("AGENTFX_RANGE") {
        let (a, b) = r.split_once(':').context("AGENTFX_RANGE=a:b")?;
        segs = vec![(a.parse()?, b.parse()?)];
    }
    crate::jobstatus::set_total(segs.iter().map(|(a, b)| (b - a) as u64).sum());
    let ffmpeg = encode::ffmpeg();
    let multi = segs.len() > 1;
    let outs: Vec<PathBuf> = (0..segs.len()).map(|i| match &opts.export {
        Some(x) => x.segment_out(&opts.out, i, multi),
        None if multi => segment_path(&opts.out, i, "mp4"),
        None => opts.out.clone(),
    }).collect();
    let logs: Vec<PathBuf> = (0..segs.len()).map(|i| opts.export.as_ref().map_or_else(PathBuf::new, |x| x.segment_log(&opts.out, i))).collect();
    let lls: Vec<Option<PathBuf>> = (0..segs.len()).map(|i| opts.lossless.as_ref().map(|l| if multi { segment_path(l, i, "mkv") } else { l.clone() })).collect();
    let results: Vec<Result<SegStats>> = std::thread::scope(|s| {
        let hs: Vec<_> = segs
            .iter()
            .enumerate()
            .map(|(i, &seg)| {
                let (o, l, g) = (outs[i].clone(), lls[i].clone(), logs[i].clone());
                s.spawn(move || render_segment(scene, base, prep, info, sched, opts, seg, o, l, g))
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("segment thread panicked")))).collect()
    });
    let mut stats = Vec::new();
    for r in results {
        stats.push(r?);
    }
    let t_par = t0.elapsed().as_secs_f64();
    if crate::prof::on() {
        let mut tot = std::collections::HashMap::new();
        for s in &stats {
            crate::prof::merge(&mut tot, &s.prof);
        }
        let frames: usize = stats.iter().map(|s| s.composited + s.repeated).sum();
        let comp: usize = stats.iter().map(|s| s.composited).sum();
        let full: usize = stats.iter().map(|s| s.full).sum();
        eprint!("{}", crate::prof::report(&tot, frames, full, comp, t_par));
    }
    let c0 = Instant::now();
    if let (Some(x), false) = (&opts.export, opts.no_encode) {
        crate::export::finish(&ffmpeg, x, &outs, &opts.out)?;
    } else if multi && !opts.no_encode {
        concat(&ffmpeg, &outs, &opts.out)?;
    }
    if multi {
        if let Some(l) = &opts.lossless {
            let parts: Vec<PathBuf> = lls.iter().map(|x| x.clone().unwrap()).collect();
            concat(&ffmpeg, &parts, l)?;
        }
    }
    let t_concat = c0.elapsed().as_secs_f64();
    let world_json = { let ws: Vec<crate::vraster::world_paint::WorldStats> = stats.iter().filter_map(|s| s.world).collect(); if ws.is_empty() { serde_json::Value::Null } else { serde_json::json!({
            "tile_frames": ws.iter().map(|w| w.tile_frames).sum::<usize>(),
            "tiles_rastered": ws.iter().map(|w| w.tiles_rastered).sum::<usize>(),
            "tiles_prefetched": ws.iter().map(|w| w.tiles_prefetched).sum::<usize>(),
            "tiles_empty": ws.iter().map(|w| w.tiles_empty).sum::<usize>(),
            "tile_hits": ws.iter().map(|w| w.tile_hits).sum::<usize>(),
            "evicted": ws.iter().map(|w| w.evicted).sum::<usize>(),
            "peak_mb_per_segment": ws.iter().map(|w| w.peak_mb).fold(0.0, f64::max),
            "tile_raster_s_sum": ws.iter().map(|w| w.raster_s).sum::<f64>(),
            "sample_s_sum": ws.iter().map(|w| w.sample_s).sum::<f64>(),
            "levels": [ws.iter().map(|w| w.min_level).min(), ws.iter().map(|w| w.max_level).max()],
            "scale_range": [ws.iter().map(|w| w.min_scale).fold(f64::MAX, f64::min), ws.iter().map(|w| w.max_scale).fold(0.0, f64::max)],
        }) } };
    let sum = |f: fn(&SegStats) -> f64| stats.iter().map(f).sum::<f64>();
    Ok(serde_json::json!({
        "frames": sched.frames,
        "segments": segs,
        "composited": stats.iter().map(|s| s.composited).sum::<usize>(),
        "repeated": stats.iter().map(|s| s.repeated).sum::<usize>(),
        "frames_sent": stats.iter().map(|s| s.sent).sum::<u64>(),
        "bytes_piped": stats.iter().map(|s| s.bytes).sum::<u64>(),
        "dirty_megapixels": (stats.iter().map(|s| s.dirty_px).sum::<u64>() as f64 / 1e5).round() / 10.0,
        "raster_s_sum": sum(|s| s.raster_s),
        "convert_s_sum": sum(|s| s.convert_s),
        "encode_wait_s_sum": sum(|s| s.encode_s),
        "segments_wall_s": t_par,
        "concat_s": t_concat,
        "ctx_created": stats.iter().map(|s| s.ctx_created).sum::<usize>(),
        "offscreen_groups": stats.iter().map(|s| s.offscreen).sum::<usize>(),
        "world": world_json,
        "gpu": crate::gpu::stats_json(),
        "mode": format!("{:?}", opts.mode), "backend": format!("{:?}", opts.backend), "preset": opts.preset,
        "threads": opts.threads, "x264_threads": opts.x264_threads, "yuv": opts.yuv,
        "encoder": opts.export.as_ref().map_or_else(|| format!("{:?}", opts.codec), |x| x.describe()),
        "hw": if opts.codec == encode::Codec::X264 || opts.export.is_some() { serde_json::Value::Null } else { serde_json::json!({
            "access": format!("{:?}", opts.hw.access), "rc": format!("{:?}", opts.hw.rc), "qp": opts.hw.qp,
            "low_power": opts.hw.low_power, "compression_level": opts.hw.compression_level,
            "nv_preset": opts.hw.nv_preset, "spatial_aq": opts.hw.spatial_aq, "temporal_aq": opts.hw.temporal_aq,
            "ten_bit": opts.hw.ten_bit }) },
    }))
}
