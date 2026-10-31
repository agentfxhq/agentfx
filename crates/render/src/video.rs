//! Deterministic native video decoding through the required ffmpeg binary.
//!
//! The decoder's unit is an input frame index, never a wall-clock position. A forward
//! request reads the next RGBA frame from one pipe. A jump creates a new pipe with ffmpeg's
//! `select` filter, which starts output at exactly the requested decoded frame index.
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;
use vello_cpu::{PixelMetadata, Pixmap};

#[derive(Clone, Copy, Debug)]
pub struct Meta {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frames: u64,
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<ProbeStream>,
}

#[derive(Deserialize)]
struct ProbeStream {
    width: u32,
    height: u32,
    r_frame_rate: String,
    nb_frames: Option<String>,
    duration: Option<String>,
}

/// Source frame selected at a scene time. The epsilon only corrects binary rounding of a
/// frame boundary such as `(1 / 30) * 30`; it does not turn a time seek into a wall-clock seek.
pub fn frame_index(meta: Meta, scene_t: f64, start: f64, offset: f64, rate: f64, looped: bool, hold: bool) -> Option<u64> {
    let source = offset + (scene_t - start) * rate;
    if source < 0.0 || meta.frames == 0 {
        return None;
    }
    let frame = ((source * meta.fps) + 1e-9).floor() as u64;
    if frame < meta.frames {
        Some(frame)
    } else if looped {
        Some(frame % meta.frames)
    } else if hold {
        Some(meta.frames - 1)
    } else {
        None
    }
}

fn ffmpeg() -> String {
    agentfx_ir::env::var("AGENTFX_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}
fn ffprobe() -> String {
    agentfx_ir::env::var("AGENTFX_FFPROBE").unwrap_or_else(|_| "ffprobe".into())
}

fn counted_frames(path: &Path) -> Result<Option<u64>> {
    let out = Command::new(ffprobe())
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .with_context(|| format!("counting frames in {}", path.display()))?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().parse().ok().filter(|&n| n > 0))
}

pub fn probe(path: &Path) -> Result<Meta> {
    let out = Command::new(ffprobe())
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,nb_frames,duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .with_context(|| format!("starting ffprobe for {}", path.display()))?;
    if !out.status.success() {
        bail!(
            "ffprobe {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let parsed: Probe = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing ffprobe output for {}", path.display()))?;
    let stream = parsed.streams.first().context("ffprobe returned no video stream")?;
    let width = stream.width;
    let height = stream.height;
    let fps = stream
        .r_frame_rate
        .split_once('/')
        .map(|(a, b)| a.parse::<f64>().unwrap_or(0.0) / b.parse::<f64>().unwrap_or(1.0))
        .unwrap_or_else(|| stream.r_frame_rate.parse().unwrap_or(0.0));
    if width == 0 || height == 0 || !fps.is_finite() || fps <= 0.0 {
        bail!(
            "ffprobe {} returned invalid video dimensions or frame rate",
            path.display()
        );
    }
    let frames = stream
        .nb_frames
        .as_deref()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .or_else(|| counted_frames(path).ok().flatten())
        .unwrap_or_else(|| {
            (stream.duration.as_deref().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) * fps)
                .round()
                .max(1.0) as u64
        });
    Ok(Meta {
        width,
        height,
        fps,
        frames,
    })
}

/// Forward jumps up to this many source frames read through the pipe instead of re-seeking.
const MAX_SKIP: u64 = 120;

pub struct Decoder {
    path: std::path::PathBuf,
    pub meta: Meta,
    child: Option<Child>,
    out: Option<ChildStdout>,
    next: u64,
    last: Option<(u64, Arc<Pixmap>)>,
}

impl Decoder {
    pub fn new(path: std::path::PathBuf) -> Result<Self> {
        let meta = probe(&path)?;
        Ok(Self {
            path,
            meta,
            child: None,
            out: None,
            next: 0,
            last: None,
        })
    }

    fn start(&mut self, first: u64) -> Result<()> {
        self.stop();
        // select operates on decoded frame numbers. It makes a jump exact even for clips
        // with sparse keyframes, unlike input-side -ss.
        let filter = format!("select='gte(n,{first})'");
        let mut child = Command::new(ffmpeg())
            .args(["-v", "error", "-i"])
            .arg(&self.path)
            .args([
                "-vf", &filter, "-vsync", "0", "-an", "-sn", "-pix_fmt", "rgba", "-f", "rawvideo",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // not piped: an unread stderr pipe can fill up and stall ffmpeg
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("starting ffmpeg for {}", self.path.display()))?;
        self.out = child.stdout.take();
        self.child = Some(child);
        self.next = first;
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.out = None;
    }

    pub fn frame(&mut self, index: u64) -> Result<Arc<Pixmap>> {
        if let Some((n, px)) = &self.last
            && *n == index
        {
            return Ok(px.clone());
        }
        // a short forward skip (source fps above the render fps, rate > 1) reads through
        // the pipe; only backward or long jumps restart ffmpeg
        if self.out.is_none() || index < self.next || index - self.next > MAX_SKIP {
            self.start(index)?;
        }
        let bytes = self.meta.width as usize * self.meta.height as usize * 4;
        let mut data = vec![0; bytes];
        let out = self.out.as_mut().context("ffmpeg stdout missing")?;
        for _ in self.next..index {
            if let Err(e) = out.read_exact(&mut data) {
                bail!("skipping to video frame {index} in {}: {e}", self.path.display());
            }
        }
        if let Err(e) = out.read_exact(&mut data) {
            bail!(
                "decoding video frame {index} from {}: {e}",
                self.path.display()
            );
        }
        // ffmpeg returns straight RGBA; the painter expects premultiplied input.
        for p in data.chunks_exact_mut(4) {
            let a = u16::from(p[3]);
            p[0] = ((u16::from(p[0]) * a + 127) / 255) as u8;
            p[1] = ((u16::from(p[1]) * a + 127) / 255) as u8;
            p[2] = ((u16::from(p[2]) * a + 127) / 255) as u8;
        }
        let px = Arc::new(Pixmap::from_parts(
            data,
            self.meta.width as u16,
            self.meta.height as u16,
            PixelMetadata {
                alpha_type: peniko::ImageAlphaType::AlphaPremultiplied,
                may_have_transparency: true,
            },
        ));
        self.next = index + 1;
        self.last = Some((index, px.clone()));
        Ok(px)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_reads_exact_frames_after_jumps() {
        let path = std::env::temp_dir().join(format!(
            "agentfx-video-{}-{}.mkv",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let status = Command::new(ffmpeg())
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc=size=16x12:rate=2", "-frames:v", "4", "-c:v", "ffv1"])
            .arg(&path)
            .status();
        if status.as_ref().is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
            return;
        }
        assert!(status.unwrap().success(), "ffmpeg could not make test clip");
        let mut decoder = Decoder::new(path.clone()).unwrap();
        assert_eq!((decoder.meta.width, decoder.meta.height, decoder.meta.frames), (16, 12, 4));
        let first = decoder.frame(0).unwrap();
        let third = decoder.frame(2).unwrap();
        let fourth = decoder.frame(3).unwrap();
        let third_again = decoder.frame(2).unwrap();
        assert_ne!(first.data(), third.data());
        assert_ne!(third.data(), fourth.data());
        assert_eq!(third.data(), third_again.data());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn frame_selection_honors_scene_start_loop_and_hold() {
        let meta = Meta { width: 1, height: 1, fps: 2.0, frames: 4 };
        assert_eq!(frame_index(meta, 2.0, 2.0, 0.0, 1.0, false, false), Some(0));
        assert_eq!(frame_index(meta, 2.5, 2.0, 0.0, 1.0, false, false), Some(1));
        assert_eq!(frame_index(meta, 4.0, 2.0, 0.0, 1.0, false, false), None);
        assert_eq!(frame_index(meta, 4.0, 2.0, 0.0, 1.0, true, false), Some(0));
        assert_eq!(frame_index(meta, 4.0, 2.0, 0.0, 1.0, false, true), Some(3));
        assert_eq!(frame_index(meta, 1.5, 2.0, 0.0, 1.0, false, false), None);
    }
}
