//! Streaming RGBA to H.264/MP4, with an optional sparse presentation timeline.
use anyhow::{Context, Result, bail, ensure};
use std::{fs::File, io::{Read, Seek, SeekFrom, Write}, path::PathBuf, process::{Child, ChildStdin, Command, Stdio}};

/// B-frames (explicit, so the VFR tail rule in `finish` matches the encoder). x264
/// `ultrafast` and the VAAPI low-power entry point encode without B-frames.
const B_FRAMES: u32 = 3;

/// Which H.264 encoder ffmpeg runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// libx264 (CPU; `AGENTFX_FFMPEG` or `ffmpeg` on `PATH`).
    X264,
    /// Intel Quick Sync through VAAPI (`h264_vaapi`; render node from `AGENTFX_VAAPI_DEVICE`).
    Vaapi,
    /// Intel Quick Sync through oneVPL (`h264_qsv`).
    Qsv,
    /// NVIDIA NVENC H.264 (`h264_nvenc`; B-frames on Pascal).
    Nvenc,
    /// NVIDIA NVENC HEVC (`hevc_nvenc`; Pascal: no B-frames, no temporal AQ; main10 with `ten_bit`).
    NvencHevc,
}

impl Codec {
    /// Any hardware encoder (NV12/10-bit input from reel, ffmpeg from [`hw_ffmpeg`]).
    #[allow(dead_code)]
    pub fn is_hw(self) -> bool { self != Codec::X264 }
    /// NVIDIA NVENC (system-memory frames, no hwupload, session-capped).
    pub fn is_nvenc(self) -> bool { matches!(self, Codec::Nvenc | Codec::NvencHevc) }
    /// Default quality when `--qp` is not given: VAAPI CQP 21 (M3); NVENC CQ
    /// (matches x264 crf 18 in encode SSIM on a recorded product tour).
    pub fn default_quality(self, ten_bit: bool) -> u32 {
        match self { Codec::Nvenc => NVENC_CQ_H264, Codec::NvencHevc if ten_bit => NVENC_CQ_HEVC10, Codec::NvencHevc => NVENC_CQ_HEVC, _ => 21 }
    }
}

/// NVENC constant-quality defaults (`-rc vbr -cq N -b:v 0`, p6, no AQ): the smallest
/// files whose encode SSIM on a recorded product tour (vs reel's lossless render) matches x264 veryfast
/// crf 18 (0.9966-0.9968): H.264 cq 26 0.99656, HEVC cq 26 0.99685, main10 cq 27 0.99672.
pub const NVENC_CQ_H264: u32 = 26;
pub const NVENC_CQ_HEVC: u32 = 26;
pub const NVENC_CQ_HEVC10: u32 = 27;

/// VAAPI rate control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateControl {
    /// Constant QP (`-qp`; I/B frames scaled by the driver's factors).
    Cqp,
    /// Intelligent constant quality (`-global_quality`; not with low power).
    Icq,
    /// Quality-defined VBR (`-global_quality` under a bitrate cap; not with low power).
    Qvbr,
}

/// Hardware encoder settings (ignored by x264).
#[derive(Clone, Debug)]
pub struct HwOpts {
    /// How ffmpeg reaches the GPU (from `detect`).
    pub access: Access,
    /// VAAPI rate control.
    pub rc: RateControl,
    /// Quality value: qp (CQP) or global_quality (ICQ/QVBR, QSV).
    pub qp: u32,
    /// VAAPI low-power entry point (EncSliceLP: CQP only, no B-frames).
    pub low_power: bool,
    /// `-compression_level` (VAAPI: 1 best .. 7 fastest); None = driver default.
    pub compression_level: Option<u32>,
    /// NVENC preset p1 (fastest) .. p7 (best).
    pub nv_preset: u32,
    /// NVENC spatial AQ.
    pub spatial_aq: bool,
    /// NVENC temporal AQ (H.264 only on Pascal; ignored for HEVC).
    pub temporal_aq: bool,
    /// HEVC main10: 10-bit 4:2:0 input converted by reel from RGBA (NVENC HEVC only).
    pub ten_bit: bool,
}

/// Measured on a recorded 80 s product tour: CQP qp 21 with B-frames matches
/// x264 veryfast crf 18 in SSIM (0.9973 vs 0.9973 against the lossless source).
impl Default for HwOpts {
    fn default() -> Self {
        Self { access: Access::Direct, rc: RateControl::Cqp, qp: 21, low_power: false, compression_level: None,
            nv_preset: NVENC_PRESET, spatial_aq: NVENC_SPATIAL_AQ, temporal_aq: NVENC_TEMPORAL_AQ, ten_bit: false }
    }
}

/// NVENC defaults (p6, AQ off; measured like the CQ defaults above).
pub const NVENC_PRESET: u32 = 6;
pub const NVENC_SPATIAL_AQ: bool = false;
pub const NVENC_TEMPORAL_AQ: bool = false;

/// The NVIDIA device node checked before probing NVENC.
pub const NVIDIA_NODE: &str = "/dev/nvidia0";

/// Concurrent NVENC sessions this process may open (parallel segments are clamped to
/// it). GeForce drivers cap sessions system-wide: 3 before 530, 5 from 530 (probed:
/// driver 535 opens 5, the 6th fails with "OpenEncodeSessionEx failed: out of memory"),
/// 8 from 550. `AGENTFX_NVENC_SESSIONS=N` overrides (e.g. when other processes encode).
pub fn nvenc_session_cap() -> usize {
    if let Some(n) = agentfx_ir::env::var("AGENTFX_NVENC_SESSIONS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|&n| n > 0) {
        return n;
    }
    #[cfg(target_os = "linux")]
    {
        let ver = std::fs::read_to_string("/proc/driver/nvidia/version").unwrap_or_default();
        driver_session_cap(&ver)
    }
    #[cfg(not(target_os = "linux"))]
    {
        3
    }
}

/// Session cap from the text of `/proc/driver/nvidia/version` (3 when unknown).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn driver_session_cap(version_text: &str) -> usize {
    let major = version_text.lines().next().unwrap_or("").split_whitespace()
        .find_map(|w| w.split('.').next().filter(|_| w.contains('.')).and_then(|m| m.parse::<u32>().ok()));
    match major { Some(m) if m >= 550 => 8, Some(m) if m >= 530 => 5, _ => 3 }
}

/// How the hardware ffmpeg is started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The render node is open-able by this process.
    Direct,
    /// Through `sg render -c 'exec ffmpeg ...'` (user in group `render`, but this login
    /// session predates it).
    Sg,
}

/// Default DRM render node for VAAPI/QSV (`AGENTFX_VAAPI_DEVICE` overrides).
pub const DEFAULT_RENDER_NODE: &str = "/dev/dri/renderD128";

/// The DRM render node used for VAAPI/QSV: `$AGENTFX_VAAPI_DEVICE`, else `/dev/dri/renderD128`.
pub fn render_node() -> String {
    agentfx_ir::env::var("AGENTFX_VAAPI_DEVICE").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| DEFAULT_RENDER_NODE.into())
}

/// Every executable called `name` on `PATH`, in `PATH` order, without duplicates.
pub fn which_all(name: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let Some(path) = std::env::var_os("PATH") else { return out };
    for dir in std::env::split_paths(&path) {
        let mut cands = vec![dir.join(name)];
        if cfg!(windows) { cands.push(dir.join(format!("{name}.exe"))); }
        for c in cands {
            if c.is_file() && !out.iter().any(|o| same_file(o, &c)) { out.push(c); }
        }
    }
    out
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) { (Ok(x), Ok(y)) => x == y, _ => a == b }
}

/// `$var` when set (and non-empty), else the first `name` on `PATH`. None when neither exists.
fn find_tool(var: &str, name: &str) -> Option<PathBuf> {
    if let Some(v) = agentfx_ir::env::var_os(var).filter(|v| !v.is_empty()) { return Some(PathBuf::from(v)); }
    which_all(name).into_iter().next()
}

/// The ffmpeg executable: `$AGENTFX_FFMPEG`, else `ffmpeg` on `PATH` (None when not found).
pub fn find_ffmpeg() -> Option<PathBuf> { find_tool("AGENTFX_FFMPEG", "ffmpeg") }
/// The ffprobe executable: `$AGENTFX_FFPROBE`, else `ffprobe` on `PATH` (None when not
/// found). Only the tests use ffprobe today.
#[cfg_attr(not(test), allow(dead_code))]
pub fn find_ffprobe() -> Option<PathBuf> { find_tool("AGENTFX_FFPROBE", "ffprobe") }
/// [`find_ffmpeg`], falling back to the bare name (the spawn error then names it).
pub fn ffmpeg() -> PathBuf { find_ffmpeg().unwrap_or_else(|| PathBuf::from("ffmpeg")) }

/// ffmpeg builds tried for a hardware encoder, in order: `$AGENTFX_FFMPEG_HW` alone when set,
/// else `$AGENTFX_FFMPEG` and then every `ffmpeg` on `PATH` (a static ffmpeg early on `PATH`
/// often lacks VAAPI/QSV/NVENC while a distribution build later on `PATH` has them).
fn hw_ffmpeg_candidates() -> Vec<PathBuf> {
    if let Some(v) = agentfx_ir::env::var_os("AGENTFX_FFMPEG_HW").filter(|v| !v.is_empty()) { return vec![PathBuf::from(v)]; }
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(v) = agentfx_ir::env::var_os("AGENTFX_FFMPEG").filter(|v| !v.is_empty()) { out.push(PathBuf::from(v)); }
    for p in which_all("ffmpeg") {
        if !out.iter().any(|o| same_file(o, &p)) { out.push(p); }
    }
    out
}

/// POSIX shell single-quoting of one word.
pub fn shell_quote(word: &str) -> String {
    if !word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./=:,+@%".contains(&b)) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

fn user_in_render_group() -> bool {
    let user = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_default();
    if user.is_empty() { return false; }
    std::fs::read_to_string("/etc/group").map(|g| g.lines().any(|l| {
        let f: Vec<&str> = l.split(':').collect();
        f.len() >= 4 && f[0] == "render" && f[3].split(',').any(|m| m == user)
    })).unwrap_or(false)
}

/// Build the ffmpeg command for `codec` via `access` (direct, or wrapped in `sg render`).
fn ffmpeg_command(ffmpeg: &std::path::Path, access: Access, args: &[String]) -> Command {
    match access {
        Access::Direct => { let mut c = Command::new(ffmpeg); c.args(args); c }
        Access::Sg => {
            let mut line = format!("exec {}", shell_quote(&ffmpeg.to_string_lossy()));
            for a in args { line.push(' '); line.push_str(&shell_quote(a)); }
            let mut c = Command::new("sg");
            c.args(["render", "-c", &line]);
            c
        }
    }
}

fn hw_input_args(codec: Codec) -> Vec<String> {
    match codec {
        Codec::Vaapi => vec!["-vaapi_device".into(), render_node()],
        Codec::Qsv => ["-init_hw_device", &format!("qsv=hw:{}", render_node()), "-filter_hw_device", "hw"].map(String::from).to_vec(),
        Codec::X264 | Codec::Nvenc | Codec::NvencHevc => vec![],
    }
}

fn hw_upload(codec: Codec) -> &'static str {
    match codec { Codec::Qsv => "format=nv12,hwupload=extra_hw_frames=64,format=qsv", _ => "format=nv12,hwupload" }
}

fn probe(ffmpeg: &std::path::Path, codec: Codec, access: Access) -> Result<(), String> {
    let enc = match codec { Codec::Qsv => "h264_qsv", Codec::Nvenc => "h264_nvenc", Codec::NvencHevc => "hevc_nvenc", _ => "h264_vaapi" };
    let mut args = vec!["-hide_banner".to_string(), "-v".into(), "error".into()];
    args.extend(hw_input_args(codec));
    // NVENC HEVC rejects frames below ~128x64; B-frames off (Pascal HEVC has none)
    let vf = if codec.is_nvenc() { "format=nv12" } else { hw_upload(codec) };
    args.extend(["-f", "lavfi", "-i", "color=c=gray:s=128x128:d=0.2", "-vf", vf, "-c:v", enc].map(String::from));
    if codec.is_nvenc() { args.extend(["-bf", "0"].map(String::from)); }
    args.extend(["-f", "null", "-"].map(String::from));
    let out = ffmpeg_command(ffmpeg, access, &args)
        .stdin(Stdio::null()).output().map_err(|e| format!("cannot run {}: {e}", ffmpeg.display()))?;
    if out.status.success() { Ok(()) } else {
        Err(format!("{enc} probe with {} failed ({}): {}", ffmpeg.display(), out.status, String::from_utf8_lossy(&out.stderr).lines().take(3).collect::<Vec<_>>().join(" | ")))
    }
}

/// Decide whether (and how) `codec` can run on this machine: the render node directly,
/// else (VAAPI/QSV on Linux) `sg render` when the node is not open-able, `sg` exists and
/// the user is in group `render` but this process is not; then a tiny probe encode with
/// each candidate ffmpeg (see [`hw_ffmpeg`]). Cached per process (parallel segments probe
/// once). `AGENTFX_HW_FORCE_SG=1` forces the sg path (testing).
pub fn detect(codec: Codec) -> Result<Access, String> {
    detect_full(codec).map(|(a, _)| a)
}

/// The ffmpeg that passed the probe for `codec` (falls back to [`ffmpeg`] when the
/// codec is unavailable or software).
pub fn hw_ffmpeg(codec: Codec) -> PathBuf {
    detect_full(codec).map(|(_, p)| p).unwrap_or_else(|_| ffmpeg())
}

fn detect_full(codec: Codec) -> Result<(Access, PathBuf), String> {
    use std::sync::{Mutex, OnceLock};
    type Entry = (Codec, Result<(Access, PathBuf), String>);
    static CACHE: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    if codec == Codec::X264 { return Ok((Access::Direct, ffmpeg())); }
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let mut c = cache.lock().unwrap();
    if let Some((_, r)) = c.iter().find(|(k, _)| *k == codec) { return r.clone(); }
    let r = (|| {
        let cands = hw_ffmpeg_candidates();
        if cands.is_empty() { return Err("ffmpeg not found (set AGENTFX_FFMPEG_HW or AGENTFX_FFMPEG, or put ffmpeg on PATH)".into()); }
        let access = if codec.is_nvenc() {
            if cfg!(target_os = "linux") && !std::path::Path::new(NVIDIA_NODE).exists() { return Err(format!("no NVIDIA device ({NVIDIA_NODE})")); }
            Access::Direct
        } else {
            let node = render_node();
            let force_sg = agentfx_ir::env::var("AGENTFX_HW_FORCE_SG").is_ok_and(|v| v == "1");
            let open = std::fs::OpenOptions::new().read(true).write(true).open(&node);
            match open {
                Ok(_) if !force_sg => Access::Direct,
                other => {
                    let why = other.err().map(|e| format!("cannot open {node}: {e}")).unwrap_or_else(|| "AGENTFX_HW_FORCE_SG=1".into());
                    if which_all("sg").is_empty() { return Err(format!("{why}; no `sg` to retry through group render")); }
                    if !user_in_render_group() { return Err(format!("{why}; not in group render")); }
                    Access::Sg
                }
            }
        };
        let mut errs = Vec::new();
        for ff in cands {
            match probe(&ff, codec, access) {
                Ok(()) => return Ok((access, ff)),
                Err(e) => errs.push(e),
            }
        }
        Err(errs.join("; "))
    })();
    c.push((codec, r.clone()));
    r
}

/// Whether repeated images are sent at every tick or held by timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Send every frame, including repeated images.
    Cfr,
    /// Send changed images and, when necessary, a final hold endpoint.
    Vfr,
}

/// Encoder configuration. Dimensions must be positive and even for yuv420p.
#[derive(Clone, Debug)]
pub struct EncoderOpts {
    /// Path to the ffmpeg executable.
    pub ffmpeg: PathBuf,
    /// Destination MP4 file, overwritten by ffmpeg.
    pub out: PathBuf,
    /// Image width in pixels (positive and even).
    pub width: u32,
    /// Image height in pixels (positive and even).
    pub height: u32,
    /// Integer timeline rate, from 1 to 1000 frames per second.
    pub fps: u32,
    /// Repeated-image handling.
    pub mode: Mode,
    /// x264 constant rate factor, from 0 to 51.
    pub crf: u32,
    /// Standard x264 speed preset, such as "medium".
    pub preset: String,
    /// Destination for ffmpeg stderr, truncated at startup.
    pub log: PathBuf,
    /// Encoder (x264 uses `ffmpeg`; hardware codecs use the system ffmpeg and NV12 input).
    pub codec: Codec,
    /// Hardware encoder settings (VAAPI/QSV only).
    pub hw: HwOpts,
    /// x264 threads (0 = x264 default: 1.5 x cores).
    pub threads: u32,
    /// Input frames are YUV 4:2:0 converted by reel (x264: I420, hardware: NV12)
    /// instead of RGBA. Hardware codecs require it.
    pub yuv: bool,
    /// Total ticks if known (VFR: the last B_FRAMES + 1 ticks are always sent, see
    /// `finish`); 0 = unknown (only the tail after the last image is densified).
    pub frames: u64,
}

/// BT.709 limited-range integer coefficients (x256): Y, Cb, Cr rows over R, G, B.
/// Each chroma row sums to 0 (greys map to exactly 128), Y rows to 220 (16..235).
const MY: [i32; 3] = [47, 157, 16];
const MU: [i32; 3] = [-26, -86, 112];
const MV: [i32; 3] = [112, -102, -10];

/// Persistent BT.709 limited-range 4:2:0 planes (I420, or NV12 for the hardware
/// encoders) for a fixed, even frame size.
/// Initialize with a full-frame update before applying incremental damage.
pub struct I420Frame {
    w: usize,
    h: usize,
    nv12: bool,
    /// yuv420p10le (planar, 16-bit little-endian samples, 10 significant bits).
    p10: bool,
    bytes: Vec<u8>,
}

impl I420Frame {
    /// Allocate planes for positive, even dimensions. Panics on invalid sizes.
    pub fn new(w: usize, h: usize) -> Self {
        let len = i420_len(w, h);
        Self { w, h, nv12: false, p10: false, bytes: vec![0; len] }
    }

    /// Planar 10-bit 4:2:0 (`yuv420p10le`) for HEVC main10, converted from RGBA at
    /// 10-bit precision (an 8-bit dither keeps its sub-LSB levels).
    pub fn new_p10(w: usize, h: usize) -> Self {
        let len = i420_len(w, h) * 2;
        Self { w, h, nv12: false, p10: true, bytes: vec![0; len] }
    }

    /// The frame layout `codec` takes from reel (I420, NV12 or 10-bit planar).
    pub fn for_codec(codec: Codec, hw: &HwOpts, w: usize, h: usize) -> Self {
        match codec {
            Codec::X264 => Self::new(w, h),
            Codec::NvencHevc if hw.ten_bit && ten_bit_native() => Self::new_p10(w, h),
            _ => Self::new_nv12(w, h),
        }
    }

    /// Same, but the chroma plane is NV12 (one interleaved U/V plane).
    pub fn new_nv12(w: usize, h: usize) -> Self {
        Self { nv12: true, ..Self::new(w, h) }
    }

    /// Reconvert damage, expanded to complete 2x2 chroma blocks and clamped.
    /// Empty rectangles are ignored; negative extents and incorrect RGBA lengths
    /// panic. Pixels outside the damage must be unchanged since the last update.
    pub fn update(&mut self, rgba: &[u8], rects: &[[i32; 4]]) {
        assert_eq!(rgba.len(), self.w * self.h * 4, "incorrect RGBA length");
        let avx2 = has_avx2();
        for &[x, y, w, h] in rects {
            assert!(w >= 0 && h >= 0, "negative damage extent");
            if w == 0 || h == 0 { continue; }
            // Widen before adding, including for rectangles outside the frame.
            let x0 = (i64::from(x) & !1).clamp(0, self.w as i64) as usize;
            let y0 = (i64::from(y) & !1).clamp(0, self.h as i64) as usize;
            let x1 = ((i64::from(x) + i64::from(w) + 1) & !1)
                .clamp(0, self.w as i64) as usize;
            let y1 = ((i64::from(y) + i64::from(h) + 1) & !1)
                .clamp(0, self.h as i64) as usize;
            if x0 < x1 && y0 < y1 && self.p10 {
                convert_rect_p10(rgba, self.w, self.h, &mut self.bytes, [x0, y0, x1, y1]);
            } else if x0 < x1 && y0 < y1 {
                convert_rect(rgba, self.w, self.h, &mut self.bytes, [x0, y0, x1, y1], avx2, self.nv12);
            }
        }
    }

    /// Contiguous Y, U, then V planes (NV12: Y, then interleaved UV), ready for the encoder.
    pub fn data(&self) -> &[u8] { &self.bytes }
}

fn i420_len(w: usize, h: usize) -> usize {
    assert!(w > 0 && h > 0 && w % 2 == 0 && h % 2 == 0,
        "I420 requires positive even dimensions");
    assert!(w <= i32::MAX as usize && h <= i32::MAX as usize,
        "I420 dimensions exceed damage coordinate limits");
    let rgba_len = w.checked_mul(h).and_then(|n| n.checked_mul(4))
        .expect("frame size overflow");
    rgba_len / 4 * 3 / 2
}

fn has_avx2() -> bool {
    #[cfg(target_arch = "x86_64")]
    { std::is_x86_feature_detected!("avx2") }
    #[cfg(not(target_arch = "x86_64"))]
    { false }
}

/// RGBA -> I420, BT.709 limited range with rounded 2x2 box chroma.
/// Panics on zero/odd dimensions, overflowing sizes or incorrect RGBA length.
#[allow(dead_code)]
pub fn rgba_to_i420(rgba: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    out.resize(i420_len(w, h), 0);
    assert_eq!(rgba.len(), w * h * 4, "incorrect RGBA length");
    convert_rect(rgba, w, h, out, [0, 0, w, h], has_avx2(), false);
}

fn convert_rect(rgba: &[u8], w: usize, h: usize, out: &mut [u8],
                [x0, y0, x1, y1]: [usize; 4], avx2: bool, nv12: bool) {
    use rayon::prelude::*;
    let (yp, uv) = out.split_at_mut(w * h);
    let yp = &mut yp[y0 * w..y1 * w];
    let rows = |i: usize, ys: &mut [u8], u: &mut [u8], v: &mut [u8]| {
        let y = y0 + i * 2;
        let a = &rgba[(y * w + x0) * 4..(y * w + x1) * 4];
        let b = &rgba[((y + 1) * w + x0) * 4..((y + 1) * w + x1) * 4];
        let (ya, yb) = ys.split_at_mut(w);
        let (ya, yb) = (&mut ya[x0..x1], &mut yb[x0..x1]);
        #[cfg(target_arch = "x86_64")]
        if avx2 {
            // Runtime detection guards target-feature code; all slices cover
            // the same even pixel count and vector loads stay inside them.
            unsafe { avx2_rows(a, b, ya, yb, u, v); }
            return;
        }
        let _ = avx2;
        scalar_rows(a, b, ya, yb, u, v);
    };
    let par = (x1 - x0) * (y1 - y0) >= 64 * 1024;
    if nv12 {
        // One interleaved chroma row of w bytes per row pair. U and V are computed into
        // a stack scratch by the same (SIMD) row kernel and interleaved in place.
        let uvp = &mut uv[y0 / 2 * w..y1 / 2 * w];
        let convert = |(i, (ys, uvr)): (usize, (&mut [u8], &mut [u8]))| {
            let n = (x1 - x0) / 2;
            let mut scratch = [0u8; 2 * 4096];
            let mut heap;
            let (u, v) = if n <= 4096 { scratch[..2 * n].split_at_mut(n) } else { heap = vec![0u8; 2 * n]; heap.split_at_mut(n) };
            rows(i, ys, u, v);
            for (k, d) in uvr[x0..x1].chunks_exact_mut(2).enumerate() {
                d[0] = u[k];
                d[1] = v[k];
            }
        };
        if par {
            yp.par_chunks_mut(2 * w).zip(uvp.par_chunks_mut(w)).enumerate().for_each(convert);
        } else {
            yp.chunks_mut(2 * w).zip(uvp.chunks_mut(w)).enumerate().for_each(convert);
        }
        return;
    }
    let (up, vp) = uv.split_at_mut(w * h / 4);
    let up = &mut up[y0 / 2 * (w / 2)..y1 / 2 * (w / 2)];
    let vp = &mut vp[y0 / 2 * (w / 2)..y1 / 2 * (w / 2)];
    let convert = |(i, ((ys, u), v)): (usize, ((&mut [u8], &mut [u8]), &mut [u8]))| {
        rows(i, ys, &mut u[x0 / 2..x1 / 2], &mut v[x0 / 2..x1 / 2]);
    };
    if par {
        yp.par_chunks_mut(2 * w).zip(up.par_chunks_mut(w / 2))
            .zip(vp.par_chunks_mut(w / 2)).enumerate().for_each(convert);
    } else {
        yp.chunks_mut(2 * w).zip(up.chunks_mut(w / 2))
            .zip(vp.chunks_mut(w / 2)).enumerate().for_each(convert);
    }
}

/// 10-bit BT.709 limited range, x16384, from the exact Kr/Kb (0.2126/0.0722) and
/// 876/896 code ranges; chroma rows sum to 0 (greys map to 512), white to 940.
const MY10: [i32; 3] = [11965, 40252, 4063];
const MU10: [i32; 3] = [-6596, -22188, 28784];
const MV10: [i32; 3] = [28784, -26145, -2639];

/// Whether 10-bit input is converted by reel (default) or sent as NV12 and widened by
/// ffmpeg (`AGENTFX_10BIT_SRC=nv12`, for comparison).
pub fn ten_bit_native() -> bool {
    agentfx_ir::env::var("AGENTFX_10BIT_SRC").map_or(true, |v| v != "nv12")
}

/// RGBA -> yuv420p10le over an even-aligned rect. Chroma from the 2x2 RGB sum (no
/// intermediate rounding).
fn convert_rect_p10(rgba: &[u8], w: usize, h: usize, out: &mut [u8], [x0, y0, x1, y1]: [usize; 4]) {
    use rayon::prelude::*;
    let (yp, uv) = out.split_at_mut(w * h * 2);
    let (up, vp) = uv.split_at_mut(w * h / 2);
    let yp = &mut yp[y0 * w * 2..y1 * w * 2];
    let up = &mut up[y0 / 2 * w..y1 / 2 * w];
    let vp = &mut vp[y0 / 2 * w..y1 / 2 * w];
    let put = |d: &mut [u8], v: i32| d.copy_from_slice(&(v.clamp(0, 1023) as u16).to_le_bytes());
    let convert = |(i, ((ys, u), v)): (usize, ((&mut [u8], &mut [u8]), &mut [u8]))| {
        let y = y0 + i * 2;
        for r in 0..2 {
            let src = &rgba[((y + r) * w) * 4..((y + r + 1) * w) * 4];
            for x in x0..x1 {
                let p = &src[x * 4..x * 4 + 3];
                let s = MY10[0] * p[0] as i32 + MY10[1] * p[1] as i32 + MY10[2] * p[2] as i32;
                put(&mut ys[(r * w + x) * 2..(r * w + x) * 2 + 2], ((s + 8192) >> 14) + 64);
            }
        }
        let (a, b) = (&rgba[y * w * 4..(y + 1) * w * 4], &rgba[(y + 1) * w * 4..(y + 2) * w * 4]);
        for cx in x0 / 2..x1 / 2 {
            let x = cx * 8;
            let sum = |c| a[x + c] as i32 + a[x + 4 + c] as i32 + b[x + c] as i32 + b[x + 4 + c] as i32;
            let (r, g, bl) = (sum(0), sum(1), sum(2));
            put(&mut u[cx * 2..cx * 2 + 2], ((MU10[0] * r + MU10[1] * g + MU10[2] * bl + 32768) >> 16) + 512);
            put(&mut v[cx * 2..cx * 2 + 2], ((MV10[0] * r + MV10[1] * g + MV10[2] * bl + 32768) >> 16) + 512);
        }
    };
    if (x1 - x0) * (y1 - y0) >= 64 * 1024 {
        yp.par_chunks_mut(4 * w).zip(up.par_chunks_mut(w)).zip(vp.par_chunks_mut(w)).enumerate().for_each(convert);
    } else {
        yp.chunks_mut(4 * w).zip(up.chunks_mut(w)).zip(vp.chunks_mut(w)).enumerate().for_each(convert);
    }
}

fn scalar_rows(a: &[u8], b: &[u8], ya: &mut [u8], yb: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    for (src, dst) in [(a, ya), (b, yb)] {
        for (p, y) in src.chunks_exact(4).zip(dst) {
            *y = (((MY[0] * p[0] as i32 + MY[1] * p[1] as i32 + MY[2] * p[2] as i32 + 128) >> 8) + 16) as u8;
        }
    }
    for (i, (u, v)) in u.iter_mut().zip(v).enumerate() {
        let x = i * 8;
        let avg = |c| (a[x + c] as i32 + a[x + 4 + c] as i32
            + b[x + c] as i32 + b[x + 4 + c] as i32 + 2) >> 2;
        let (r, g, b) = (avg(0), avg(1), avg(2));
        *u = (((MU[0] * r + MU[1] * g + MU[2] * b + 128) >> 8) + 128) as u8;
        *v = (((MV[0] * r + MV[1] * g + MV[2] * b + 128) >> 8) + 128) as u8;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_rows(a: &[u8], b: &[u8], ya: &mut [u8], yb: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    use std::arch::x86_64::*;
    // Each lane holds one unsigned RGB channel in i32, so neither sums nor
    // signed chroma products overflow. Arithmetic shifts match scalar rounding.
    unsafe {
        let mask = _mm256_set1_epi32(255);
        let channels = |p| [
            _mm256_and_si256(p, mask),
            _mm256_and_si256(_mm256_srli_epi32::<8>(p), mask),
            _mm256_and_si256(_mm256_srli_epi32::<16>(p), mask),
        ];
        let matrix = |[r, g, b]: [__m256i; 3], cr, cg, cb, offset| {
            let sum = _mm256_add_epi32(_mm256_mullo_epi32(r, _mm256_set1_epi32(cr)),
                _mm256_add_epi32(_mm256_mullo_epi32(g, _mm256_set1_epi32(cg)),
                    _mm256_mullo_epi32(b, _mm256_set1_epi32(cb))));
            _mm256_add_epi32(_mm256_srai_epi32::<8>(_mm256_add_epi32(sum,
                _mm256_set1_epi32(128))), _mm256_set1_epi32(offset))
        };
        let store = |p: *mut u8, values| {
            let words = _mm_packs_epi32(_mm256_castsi256_si128(values), _mm256_extracti128_si256::<1>(values));
            let bytes = _mm_packus_epi16(words, words);
            _mm_storel_epi64(p.cast(), bytes);
        };
        let n = ya.len() / 16 * 16;
        for x in (0..n).step_by(16) {
            let a0 = channels(_mm256_loadu_si256(a.as_ptr().add(x * 4).cast()));
            let a1 = channels(_mm256_loadu_si256(a.as_ptr().add((x + 8) * 4).cast()));
            let b0 = channels(_mm256_loadu_si256(b.as_ptr().add(x * 4).cast()));
            let b1 = channels(_mm256_loadu_si256(b.as_ptr().add((x + 8) * 4).cast()));
            store(ya.as_mut_ptr().add(x), matrix(a0, MY[0], MY[1], MY[2], 16));
            store(ya.as_mut_ptr().add(x + 8), matrix(a1, MY[0], MY[1], MY[2], 16));
            store(yb.as_mut_ptr().add(x), matrix(b0, MY[0], MY[1], MY[2], 16));
            store(yb.as_mut_ptr().add(x + 8), matrix(b1, MY[0], MY[1], MY[2], 16));
            let avg = std::array::from_fn(|c| {
                let sums = _mm256_hadd_epi32(_mm256_add_epi32(a0[c], b0[c]),
                    _mm256_add_epi32(a1[c], b1[c]));
                let ordered = _mm256_permutevar8x32_epi32(sums, _mm256_setr_epi32(0, 1, 4, 5, 2, 3, 6, 7));
                _mm256_srli_epi32::<2>(_mm256_add_epi32(ordered, _mm256_set1_epi32(2)))
            });
            store(u.as_mut_ptr().add(x / 2), matrix(avg, MU[0], MU[1], MU[2], 128));
            store(v.as_mut_ptr().add(x / 2), matrix(avg, MV[0], MV[1], MV[2], 128));
        }
        scalar_rows(&a[n * 4..], &b[n * 4..], &mut ya[n..], &mut yb[n..], &mut u[n / 2..], &mut v[n / 2..]);
    }
}

/// Successful input calls, transmitted images, repeats, and actual pipe bytes.
/// `frames_sent` includes the optional VFR endpoint image; `frames_repeated`
/// counts repeat() calls. VFR `bytes_piped` includes container overhead.
#[derive(Clone, Debug, Default)]
pub struct EncodeStats {
    /// Number of successful frame() and repeat() calls.
    pub frames_total: u64,
    /// Number of images piped, including any final VFR endpoint.
    pub frames_sent: u64,
    /// Number of successful repeat() calls.
    pub frames_repeated: u64,
    /// Total bytes written, including VFR container metadata.
    pub bytes_piped: u64,
}

/// Owns ffmpeg and its input pipe. Dropping an unfinished encoder kills and reaps it.
pub struct Encoder {
    opts: EncoderOpts,
    child: Child,
    stdin: Option<ChildStdin>,
    last: Vec<u8>,
    last_sent: Option<u64>,
    frame_bytes: usize,
    stats: EncodeStats,
    failed: bool,
    /// B-frames in use (0: no reordering, the VFR tail needs only the endpoint).
    bframes: u32,
}

impl Encoder {
    /// Start ffmpeg, with stderr directed to the configured log file.
    pub fn start(opts: EncoderOpts) -> Result<Self> {
        ensure!(opts.width > 0 && opts.height > 0 && opts.width % 2 == 0 && opts.height % 2 == 0,
            "yuv420p requires positive even dimensions");
        ensure!(opts.width <= i32::MAX as u32 && opts.height <= i32::MAX as u32, "dimensions exceed bitmap header limits");
        ensure!((1..=1000).contains(&opts.fps), "fps must be in 1..=1000 (millisecond timestamp resolution)");
        let hw = opts.codec != Codec::X264;
        if hw {
            ensure!(opts.yuv, "hardware encoders take NV12 from reel (no RGBA pipe)");
            ensure!(opts.hw.qp <= 51, "hardware quality must be in 0..=51");
            ensure!(!(opts.hw.low_power && opts.hw.rc != RateControl::Cqp), "VAAPI low power supports only CQP");
            ensure!(opts.codec != Codec::Qsv || opts.hw.rc == RateControl::Cqp && !opts.hw.low_power, "QSV takes only --qp (global_quality)");
            ensure!(!opts.hw.ten_bit || opts.codec == Codec::NvencHevc, "--10bit needs --encoder nvenc-hevc");
            ensure!(!opts.codec.is_nvenc() || (1..=7).contains(&opts.hw.nv_preset), "NVENC preset must be p1..p7");
        } else {
            ensure!(opts.crf <= 51, "x264 CRF must be in 0..=51");
            ensure!(["ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow", "placebo"].contains(&opts.preset.as_str()), "unsupported x264 preset: {}", opts.preset);
        }
        ensure!(opts.out.extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4")), "output must have .mp4 extension");
        ensure!(opts.out != opts.log, "output and log paths must differ");
        let px = (opts.width as usize).checked_mul(opts.height as usize).context("frame size overflow")?;
        let p10 = opts.hw.ten_bit && opts.codec == Codec::NvencHevc && ten_bit_native();
        let frame_bytes = match (opts.yuv, p10) { (false, _) => px * 4, (true, false) => px * 3 / 2, (true, true) => px * 3 };
        ensure!(frame_bytes <= u32::MAX as usize, "RGBA frame exceeds bitmap size limit");
        let bframes = match opts.codec {
            Codec::X264 => if opts.preset == "ultrafast" { 0 } else { B_FRAMES },
            Codec::Vaapi => if opts.hw.low_power { 0 } else { B_FRAMES },
            Codec::Qsv => B_FRAMES,
            // ffmpeg's nvenc wrapper derives DTS as "input pts - (B-frames) ticks", which
            // holds only on a dense timeline: on reel's sparse VFR timeline it yields
            // dts > pts and ffmpeg rewrites the frame's pts (a change at tick 80 shown at
            // 84). B-frames only with --mode cfr; HEVC on Pascal has none.
            Codec::Nvenc => if opts.mode == Mode::Cfr { B_FRAMES } else { 0 },
            Codec::NvencHevc => 0,
        };
        let mut log = File::create(&opts.log).context("create ffmpeg log")?;
        let mut a: Vec<String> = Vec::new();
        let mut push = |xs: &[&str]| a.extend(xs.iter().map(|x| x.to_string()));
        push(&["-hide_banner", "-y"]);
        let infmt = match (opts.yuv, hw) { (false, _) => "rgba", (true, false) => "yuv420p", (true, true) if p10 => "yuv420p10le", (true, true) => "nv12" };
        // VAAPI/QSV upload to GPU surfaces; NVENC takes system-memory frames like x264
        let upload = hw && !opts.codec.is_nvenc();
        if hw { let h = hw_input_args(opts.codec); a.extend(h); }
        let mut push = |xs: &[&str]| a.extend(xs.iter().map(|x| x.to_string()));
        match opts.mode {
            Mode::Cfr => push(&["-f", "rawvideo", "-pix_fmt", infmt, "-s", &format!("{}x{}", opts.width, opts.height), "-r", &opts.fps.to_string()]),
            Mode::Vfr => push(&["-f", "matroska"]),
        }
        push(&["-i", "pipe:0", "-an"]);
        // settb rounds integer timestamps onto the frame grid without the floating-point
        // truncation that a setpts expression can introduce.
        let mut vf = if opts.mode == Mode::Vfr { vec![format!("settb=expr=1/{}", opts.fps)] } else { vec![] };
        if opts.codec == Codec::NvencHevc && opts.hw.ten_bit {
            vf.push("format=p010le".into());
        }
        if upload {
            // Tag the frames before upload: output -colorspace/-color_* options would make
            // ffmpeg insert a software colour conversion after hwupload (fails with ENOSYS).
            vf.push("format=nv12,setparams=colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv".into());
            vf.push(hw_upload(opts.codec).to_string());
        }
        // (review M3 #7) RGBA pipe: ffmpeg converts; make it BT.709 limited like reel's
        if !opts.yuv { vf.push("scale=out_color_matrix=bt709:out_range=tv".into()); }
        if !vf.is_empty() { push(&["-vf", &vf.join(",")]); }
        match opts.codec {
            // deblock -1,-1 (M4b): slightly less blocky shadow gradients at equal size and SSIM
            Codec::X264 => push(&["-c:v", "libx264", "-crf", &opts.crf.to_string(), "-preset", &opts.preset, "-pix_fmt", "yuv420p", "-x264-params", "deblock=-1,-1"]),
            Codec::Vaapi => {
                push(&["-c:v", "h264_vaapi", "-profile:v", "high", "-low_power", if opts.hw.low_power { "1" } else { "0" }]);
                let q = opts.hw.qp.to_string();
                match opts.hw.rc {
                    RateControl::Cqp => push(&["-rc_mode", "CQP", "-qp", &q]),
                    RateControl::Icq => push(&["-rc_mode", "ICQ", "-global_quality", &q]),
                    RateControl::Qvbr => push(&["-rc_mode", "QVBR", "-global_quality", &q, "-b:v", "6M", "-maxrate", "40M"]),
                }
                if let Some(c) = opts.hw.compression_level { push(&["-compression_level", &c.to_string()]); }
            }
            Codec::Qsv => push(&["-c:v", "h264_qsv", "-profile:v", "high", "-global_quality", &opts.hw.qp.to_string()]),
            Codec::Nvenc | Codec::NvencHevc => {
                let hevc = opts.codec == Codec::NvencHevc;
                if hevc {
                    // hvc1: the sample entry QuickTime/Safari accept
                    push(&["-c:v", "hevc_nvenc", "-profile:v", if opts.hw.ten_bit { "main10" } else { "main" }, "-tag:v", "hvc1"]);
                } else {
                    push(&["-c:v", "h264_nvenc", "-profile:v", "high"]);
                }
                // constant quality: VBR with -cq and no bitrate target
                push(&["-preset", &format!("p{}", opts.hw.nv_preset), "-tune", "hq", "-rc", "vbr", "-cq", &opts.hw.qp.to_string(), "-b:v", "0"]);
                if opts.hw.spatial_aq { push(&["-spatial-aq", "1"]); }
                if opts.hw.temporal_aq && !hevc { push(&["-temporal-aq", "1"]); }
                if bframes == 0 { push(&["-bf", "0"]); }
            }
        }
        if opts.mode == Mode::Vfr { push(&["-fps_mode", "passthrough"]); } else { push(&["-fps_mode", "cfr", "-r", &opts.fps.to_string()]); }
        // M1: B-frames on in both modes (M0 disabled them for VFR); the sparse timeline
        // is verified by ffprobe (duration, frame count) and a CFR resample check.
        if !upload {
            // reel's YUV is BT.709 limited range: tag it so players don't guess
            push(&["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv"]);
        }
        if opts.threads > 0 && !hw { push(&["-threads", &opts.threads.to_string()]); }
        // encoder experiments: extra ffmpeg output args, e.g. AGENTFX_X264="-tune animation -x264-params rc-lookahead=10"
        if let Ok(extra) = agentfx_ir::env::var(if hw { "AGENTFX_HWENC" } else { "AGENTFX_X264" }) {
            a.extend(extra.split_whitespace().map(String::from));
        }
        let mut push = |xs: &[&str]| a.extend(xs.iter().map(|x| x.to_string()));
        if bframes > 0 { push(&["-bf", &bframes.to_string()]); }
        push(&["-enc_time_base", &format!("1/{}", opts.fps), "-video_track_timescale", &opts.fps.to_string(), "-movflags", "+faststart"]);
        a.push(opts.out.to_string_lossy().into_owned());
        let (ffmpeg, access) = if hw { (hw_ffmpeg(opts.codec), opts.hw.access) } else { (opts.ffmpeg.clone(), Access::Direct) };
        let mut cmd = ffmpeg_command(&ffmpeg, access, &a);
        writeln!(log, "agentfx-render: {cmd:?}").context("write ffmpeg log")?;
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::from(log)).spawn().context("start ffmpeg")?;
        let stdin = child.stdin.take();
        let mut encoder = Self { opts, child, stdin, last: Vec::new(), last_sent: None, frame_bytes, stats: EncodeStats::default(), failed: false, bframes };
        if encoder.opts.mode == Mode::Vfr { encoder.write(&header(&encoder.opts))?; }
        Ok(encoder)
    }

    fn check(&mut self, n: u64) -> Result<()> {
        ensure!(!self.failed, "encoder unusable after pipe failure: {}", self.log_tail());
        ensure!(n == self.stats.frames_total, "expected frame {}, got {n}", self.stats.frames_total);
        ensure!(n < u64::MAX && n <= (1u64 << 52), "frame index exceeds supported timestamp range");
        if let Some(status) = self.child.try_wait().context("check ffmpeg status")? {
            self.failed = true;
            bail!("ffmpeg exited early ({status}): {}", self.log_tail());
        }
        Ok(())
    }

    fn log_tail(&self) -> String {
        let read = || -> std::io::Result<String> {
            let mut file = File::open(&self.opts.log)?;
            let len = file.metadata()?.len();
            file.seek(SeekFrom::Start(len.saturating_sub(8192)))?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        };
        read().unwrap_or_else(|e| format!("cannot read {}: {e}", self.opts.log.display()))
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        if let Err(e) = self.stdin.as_mut().context("encoder pipe closed")?.write_all(bytes) {
            self.failed = true;
            self.stdin.take();
            // EPIPE means the reader closed; reap it to capture the complete log.
            if e.kind() != std::io::ErrorKind::BrokenPipe { let _ = self.child.kill(); }
            let _ = self.child.wait();
            bail!("ffmpeg pipe failed: {e}; log tail: {}", self.log_tail());
        }
        self.stats.bytes_piped += bytes.len() as u64;
        Ok(())
    }

    fn send(&mut self, n: u64, rgba: &[u8]) -> Result<()> {
        if self.opts.mode == Mode::Vfr {
            // One cluster per image avoids the signed 16-bit relative-time limit.
            let millis = ((n as u128 * 1000 + self.opts.fps as u128 / 2) / self.opts.fps as u128) as u64;
            let timestamp = uint(0xe7, millis);
            let block_len = rgba.len() as u64 + 4;
            let mut prefix = id(0x1f43b675);
            prefix.extend(size(timestamp.len() as u64 + 1 + size(block_len).len() as u64 + block_len));
            prefix.extend(timestamp);
            prefix.push(0xa3);
            prefix.extend(size(block_len));
            prefix.extend([0x81, 0, 0, 0x80]); // track 1, relative time 0, keyframe
            self.write(&prefix)?;
        }
        self.write(rgba)?;
        self.stats.frames_sent += 1;
        self.last_sent = Some(n);
        Ok(())
    }

    /// Supply new straight-RGBA content at tick n, in consecutive tick order.
    pub fn frame(&mut self, n: u64, rgba: &[u8]) -> Result<()> {
        self.check(n)?;
        ensure!(rgba.len() == self.frame_bytes, "RGBA length {}, expected {}", rgba.len(), self.frame_bytes);
        self.send(n, rgba)?;
        self.last.clear();
        self.last.extend_from_slice(rgba);
        self.stats.frames_total += 1;
        Ok(())
    }

    /// Frame n is identical to frame n-1; n must be positive.
    pub fn repeat(&mut self, n: u64) -> Result<()> {
        self.check(n)?;
        ensure!(n > 0 && !self.last.is_empty(), "repeat requires a previous frame");
        let tail = self.opts.mode == Mode::Vfr && self.bframes > 0 && self.opts.frames > 0 && n + self.bframes as u64 + 1 >= self.opts.frames;
        if self.opts.mode == Mode::Cfr || tail {
            let last = std::mem::take(&mut self.last);
            let result = self.send(n, &last);
            self.last = last;
            result?;
        }
        self.stats.frames_total += 1;
        self.stats.frames_repeated += 1;
        Ok(())
    }

    /// Close input and check ffmpeg's status. total must match all input calls.
    pub fn finish(mut self, total: u64) -> Result<EncodeStats> {
        ensure!(!self.failed, "encoder unusable after pipe failure: {}", self.log_tail());
        ensure!(total == self.stats.frames_total, "finish total {total} differs from {} input calls", self.stats.frames_total);
        ensure!(total > 0, "cannot encode an empty video");
        if self.opts.mode == Mode::Vfr && self.last_sent != Some(total - 1) {
            let last = std::mem::take(&mut self.last);
            // With B-frames the MP4 track duration follows the (compressed) DTS sequence:
            // a lone hold frame far after the previous image loses the gap. Ending on
            // B_FRAMES + 1 consecutive ticks makes the last DTS land on the last tick
            // (measured: duration exact for >= 3 with -bf 3), costing <= 3 skip frames.
            let k: u64 = if self.bframes == 0 { 1 } else { self.bframes as u64 + 1 };
            let from = (total - k.min(total)).max(self.last_sent.map_or(0, |l| l + 1));
            for n in from..total {
                self.send(n, &last)?;
            }
        }
        self.stdin.take();
        let status = self.child.wait().context("wait for ffmpeg")?;
        ensure!(status.success(), "ffmpeg failed ({status}): {}", self.log_tail());
        Ok(self.stats.clone())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.stdin.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn id(value: u32) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    bytes[bytes.iter().position(|&x| x != 0).unwrap_or(3)..].to_vec()
}
fn size(value: u64) -> Vec<u8> {
    let width = (1..=8).find(|&n| value < (1u64 << (7 * n)) - 1).expect("EBML size fits");
    let encoded = value | (1u64 << (7 * width));
    encoded.to_be_bytes()[8 - width..].to_vec()
}
fn elem(tag: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = id(tag);
    out.extend(size(bytes.len() as u64));
    out.extend(bytes);
    out
}
fn uint(tag: u32, value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    elem(tag, &bytes[bytes.iter().position(|&x| x != 0).unwrap_or(7)..])
}
fn header(opts: &EncoderOpts) -> Vec<u8> {
    let mut out = elem(0x1a45dfa3, &[uint(0x4286, 1), uint(0x42f7, 1), uint(0x42f2, 4), uint(0x42f3, 8), elem(0x4282, b"matroska"), uint(0x4287, 4), uint(0x4285, 2)].concat());
    out.extend(id(0x18538067));
    out.extend([0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    out.extend(elem(0x1549a966, &[uint(0x2ad7b1, 1_000_000), elem(0x4d80, b"reel"), elem(0x5741, b"reel")].concat()));
    // Standard BITMAPINFOHEADER with an explicit RGBA fourcc, not implicit BGR.
    let mut bitmap = Vec::new();
    bitmap.extend(40u32.to_le_bytes());
    bitmap.extend(opts.width.to_le_bytes());
    bitmap.extend(opts.height.to_le_bytes());
    bitmap.extend(1u16.to_le_bytes());
    let p10 = opts.yuv && opts.hw.ten_bit && opts.codec == Codec::NvencHevc && ten_bit_native();
    if opts.yuv {
        bitmap.extend(12u16.to_le_bytes());
        bitmap.extend(if opts.codec == Codec::X264 { b"I420" } else { b"NV12" });
        bitmap.extend((opts.width * opts.height * 3 / 2).to_le_bytes());
    } else {
        bitmap.extend(32u16.to_le_bytes());
        bitmap.extend(b"RGBA");
        bitmap.extend((opts.width * opts.height * 4).to_le_bytes());
    }
    bitmap.extend([0u8; 16]);
    let track = if p10 {
        // The VFW mapping has no 10-bit YUV tag; V_UNCOMPRESSED + ColourSpace uses
        // libavcodec's raw tags, where 'Y3',11,10 is yuv420p10le (there is no P010 tag).
        let video = elem(0xe0, &[uint(0xb0, opts.width as u64), uint(0xba, opts.height as u64), elem(0x2eb524, b"Y3\x0b\x0a")].concat());
        elem(0xae, &[uint(0xd7, 1), uint(0x73c5, 1), uint(0x83, 1), uint(0x9c, 0), uint(0x23e383, 1_000_000_000 / opts.fps as u64), elem(0x86, b"V_UNCOMPRESSED"), video].concat())
    } else {
        let video = elem(0xe0, &[uint(0xb0, opts.width as u64), uint(0xba, opts.height as u64)].concat());
        elem(0xae, &[uint(0xd7, 1), uint(0x73c5, 1), uint(0x83, 1), uint(0x9c, 0), uint(0x23e383, 1_000_000_000 / opts.fps as u64), elem(0x86, b"V_MS/VFW/FOURCC"), elem(0x63a2, &bitmap), video].concat())
    };
    out.extend(elem(0x1654ae6b, &track));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar_reference(rgba: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
        use rayon::prelude::*;
        assert!(w > 0 && h > 0 && w % 2 == 0 && h % 2 == 0,
            "I420 requires positive even dimensions");
        let (cw, ch) = (w / 2, h / 2);
        out.resize(w * h + 2 * cw * ch, 0);
        let (y_plane, uv) = out.split_at_mut(w * h);
        let (u_plane, v_plane) = uv.split_at_mut(cw * ch);
        y_plane.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let src = &rgba[y * w * 4..(y + 1) * w * 4];
            for x in 0..w {
                let (r, g, b) = (src[x * 4] as i32, src[x * 4 + 1] as i32, src[x * 4 + 2] as i32);
                row[x] = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
            }
        });
        u_plane.par_chunks_mut(cw).zip(v_plane.par_chunks_mut(cw)).enumerate().for_each(|(cy, (ur, vr))| {
            let r0 = &rgba[(2 * cy) * w * 4..(2 * cy + 1) * w * 4];
            let r1 = &rgba[(2 * cy + 1) * w * 4..(2 * cy + 2) * w * 4];
            for cx in 0..cw {
                let mut s = [0i32; 3];
                for row in [r0, r1] {
                    for dx in 0..2 {
                        let p = &row[(2 * cx + dx) * 4..(2 * cx + dx) * 4 + 3];
                        s[0] += p[0] as i32;
                        s[1] += p[1] as i32;
                        s[2] += p[2] as i32;
                    }
                }
                let (r, g, b) = ((s[0] + 2) >> 2, (s[1] + 2) >> 2, (s[2] + 2) >> 2);
                ur[cx] = (((-26 * r - 86 * g + 112 * b + 128) >> 8) + 128) as u8;
                vr[cx] = (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128) as u8;
            }
        });
    }

    fn random_byte(state: &mut u32) -> u8 {
        *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (*state >> 24) as u8
    }

    #[test]
    fn i420_incremental_matches_scalar() {
        let mut seed = 19;
        // Includes scalar tails, full SIMD vectors, and the parallel threshold.
        for (w, h) in [(2, 2), (18, 10), (66, 48), (322, 258)] {
            let mut rgba: Vec<u8> = (0..w * h * 4).map(|_| random_byte(&mut seed)).collect();
            let mut actual = I420Frame::new(w, h);
            let mut fallback = vec![0; i420_len(w, h)];
            let mut expected = Vec::new();
            let mut full = Vec::new();
            actual.update(&rgba, &[[0, 0, w as i32, h as i32]]);
            for tick in 0..50 {
                let x = random_byte(&mut seed) as usize % w;
                let y = random_byte(&mut seed) as usize % h;
                let rw = 1 + random_byte(&mut seed) as usize % (w - x);
                let rh = 1 + random_byte(&mut seed) as usize % (h - y);
                let rects = [
                    [x as i32, y as i32, rw as i32, rh as i32],
                    [1, 1, 1, 1],
                    [w as i32 - 1, h as i32 - 1, 1, 1],
                    [w as i32 - 2, 1, 2, h as i32 - 1],
                ];
                for [x, y, rw, rh] in rects {
                    for py in y..y + rh {
                        for px in x..x + rw {
                            let at = (py as usize * w + px as usize) * 4;
                            for c in 0..3 { rgba[at + c] = random_byte(&mut seed); }
                            rgba[at + 3] = 255;
                        }
                    }
                }
                actual.update(&rgba, &rects);
                scalar_reference(&rgba, w, h, &mut expected);
                rgba_to_i420(&rgba, w, h, &mut full);
                convert_rect(&rgba, w, h, &mut fallback, [0, 0, w, h], false, false);
                assert_eq!(actual.data(), expected, "incremental {w}x{h}, tick {tick}");
                assert_eq!(full, expected, "full {w}x{h}, tick {tick}");
                assert_eq!(fallback, expected, "fallback {w}x{h}, tick {tick}");
                actual.update(&rgba, &[]);
                actual.update(&rgba, &[[1, 1, 0, 1], [1, 1, 1, 0], [i32::MAX, i32::MAX, i32::MAX, i32::MAX]]);
                assert_eq!(actual.data(), expected);
            }
            // Clipping a rectangle that extends beyond every edge is full damage.
            rgba.fill(255);
            actual.update(&rgba, &[[-3, -5, w as i32 + 9, h as i32 + 11]]);
            scalar_reference(&rgba, w, h, &mut expected);
            assert_eq!(actual.data(), expected);
        }
    }

    #[test]
    fn i420_rejects_invalid_input() {
        for (w, h) in [(0, 2), (2, 0), (3, 2), (2, 3), (usize::MAX, 2)] {
            assert!(std::panic::catch_unwind(|| I420Frame::new(w, h)).is_err());
        }
        assert!(std::panic::catch_unwind(|| I420Frame::new(2, 2).update(&[0; 15], &[])).is_err());
        assert!(std::panic::catch_unwind(|| I420Frame::new(2, 2).update(&[0; 16], &[[0, 0, -1, 1]])).is_err());
    }

    #[test]
    #[ignore = "manual conversion timing on the shared machine"]
    fn i420_bench() {
        use std::{hint::black_box, time::Instant};
        // A bounded, identical pool for both implementations avoids taking over
        // the shared machine and makes the thread count explicit in the output.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        pool.install(|| {
            let (w, h) = (1920, 1080);
            let mut seed = 91;
            let rgba: Vec<u8> = (0..w * h * 4).map(|_| random_byte(&mut seed)).collect();
            let mut old = Vec::new();
            let mut new = I420Frame::new(w, h);
            let full = [[0, 0, w as i32, h as i32]];
            let small = [[101, 103, 200, 100], [701, 401, 200, 100], [1501, 901, 200, 100]];
            let measure = |name: &str, f: &mut dyn FnMut()| {
                for _ in 0..10 { f(); }
                let start = Instant::now();
                for _ in 0..200 { f(); }
                println!("{name}: {:.4} ms/frame", start.elapsed().as_secs_f64() * 1000.0 / 200.0);
            };
            println!("1920x1080, 2 Rayon threads, AVX2={}", has_avx2());
            measure("old scalar", &mut || {
                scalar_reference(black_box(&rgba), w, h, &mut old);
                black_box(&old);
            });
            measure("new full-frame", &mut || {
                new.update(black_box(&rgba), black_box(&full));
                black_box(new.data());
            });
            assert_eq!(new.data(), old);
            measure("new small damage (3 x 200x100, odd origins)", &mut || {
                new.update(black_box(&rgba), black_box(&small));
                black_box(new.data());
            });
        });
    }

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    /// ffmpeg (with libx264) and ffprobe for the encode tests, found like the renderer
    /// finds them (`AGENTFX_FFMPEG` / `AGENTFX_FFPROBE`, else `PATH`). None: tests skip.
    fn tools() -> Option<&'static (PathBuf, PathBuf)> {
        static TOOLS: std::sync::OnceLock<Option<(PathBuf, PathBuf)>> = std::sync::OnceLock::new();
        TOOLS.get_or_init(|| {
            let (ffmpeg, ffprobe) = (find_ffmpeg()?, find_ffprobe()?);
            let enc = Command::new(&ffmpeg).args(["-hide_banner", "-encoders"]).output().ok()?;
            String::from_utf8_lossy(&enc.stdout).contains("libx264").then_some((ffmpeg, ffprobe))
        }).as_ref()
    }

    /// Return early (test passes with a note) when ffmpeg/ffprobe/libx264 are missing.
    macro_rules! need_ffmpeg {
        () => {
            if tools().is_none() {
                eprintln!("skip: ffmpeg with libx264 and ffprobe not found (set AGENTFX_FFMPEG / AGENTFX_FFPROBE or put them on PATH)");
                return Ok(());
            }
        };
    }

    fn ff() -> PathBuf { tools().expect("need_ffmpeg! first").0.clone() }
    fn fp() -> PathBuf { tools().expect("need_ffmpeg! first").1.clone() }

    /// VAAPI through `sg render` with any candidate ffmpeg.
    fn sg_probe() -> std::result::Result<(), String> {
        if which_all("sg").is_empty() { return Err("no sg".into()); }
        let mut errs = Vec::new();
        for f in hw_ffmpeg_candidates() {
            match probe(&f, Codec::Vaapi, Access::Sg) { Ok(()) => return Ok(()), Err(e) => errs.push(e) }
        }
        Err(if errs.is_empty() { "no ffmpeg".into() } else { errs.join("; ") })
    }

    // NVENC HEVC's minimum frame size is about 128x64
    const WIDTH: usize = 128;
    const HEIGHT: usize = 64;
    const CHANGES: [(u64, [u8; 3]); 10] = [
        (0, [220, 30, 30]), (1, [30, 220, 30]), (2, [30, 30, 220]),
        (10, [220, 220, 30]), (11, [220, 30, 220]), (40, [30, 220, 220]),
        (41, [120, 30, 30]), (42, [30, 120, 30]), (43, [30, 30, 120]),
        (80, [180, 180, 180]),
    ];

    fn scratch(label: &str) -> Result<Scratch> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target").join(format!("encode-{label}-{}", std::process::id()));
        std::fs::create_dir(&dir)?;
        Ok(Scratch(dir))
    }

    fn color_at(tick: u64) -> [u8; 3] {
        CHANGES.iter().rev().find(|&&(n, _)| n <= tick).unwrap().1
    }

    fn run(command: &mut Command) -> Result<Vec<u8>> {
        let output = command.output().with_context(|| format!("run {command:?}"))?;
        ensure!(output.status.success(), "{command:?} failed ({}): {}",
            output.status, String::from_utf8_lossy(&output.stderr));
        Ok(output.stdout)
    }

    fn encode_sequence(dir: &std::path::Path, name: &str, start: u64, end: u64,
                       yuv: bool, known_total: bool) -> Result<PathBuf> {
        encode_sequence_with(dir, name, start, end, yuv, known_total, Codec::X264, HwOpts::default())
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_sequence_with(dir: &std::path::Path, name: &str, start: u64, end: u64,
                       yuv: bool, known_total: bool, codec: Codec, hw: HwOpts) -> Result<PathBuf> {
        let out = dir.join(format!("{name}.mp4"));
        let total = end - start;
        let bframes = if codec == Codec::Vaapi && hw.low_power || codec.is_nvenc() { 0 } else { B_FRAMES };
        let mut encoder = Encoder::start(EncoderOpts {
            ffmpeg: ff(), out: out.clone(),
            width: WIDTH as u32, height: HEIGHT as u32, fps: 30, mode: Mode::Vfr,
            crf: 18, preset: "veryfast".into(), log: dir.join(format!("{name}.log")),
            codec, hw: hw.clone(),
            threads: 1, yuv, frames: if known_total { total } else { 0 },
        })?;
        let mut i420 = Vec::new();
        let mut nv12 = I420Frame::for_codec(codec, &hw, WIDTH, HEIGHT);
        let mut unique = 0;
        for tick in start..end {
            if tick == start || CHANGES.iter().any(|&(n, _)| n == tick) {
                let [r, g, b] = color_at(tick);
                let rgba = [r, g, b, 255].repeat(WIDTH * HEIGHT);
                if codec != Codec::X264 {
                    nv12.update(&rgba, &[[0, 0, WIDTH as i32, HEIGHT as i32]]);
                    encoder.frame(tick - start, nv12.data())?;
                } else if yuv {
                    rgba_to_i420(&rgba, WIDTH, HEIGHT, &mut i420);
                    encoder.frame(tick - start, &i420)?;
                } else {
                    encoder.frame(tick - start, &rgba)?;
                }
                unique += 1;
            } else {
                encoder.repeat(tick - start)?;
            }
        }
        let stats = encoder.finish(total)?;
        assert_eq!(stats.frames_total, total);
        assert_eq!(stats.frames_repeated, total - unique);
        // The known-total path sends every tail tick, including repeats before
        // the final unique image; finish can only fill after that last image.
        let last_unique = CHANGES.iter().filter(|&&(n, _)| n >= start && n < end)
            .map(|&(n, _)| n).last().unwrap_or(start);
        let tail_start = (end - u64::from(bframes) - 1)
            .max(if known_total && bframes > 0 { start } else { last_unique + 1 });
        let tail_repeats = (tail_start..end)
            .filter(|&n| n != start && !CHANGES.iter().any(|&(tick, _)| tick == n)).count();
        assert_eq!(stats.frames_sent, unique + tail_repeats as u64);
        Ok(out)
    }

    fn check_sequence(path: &std::path::Path, start: u64, end: u64) -> Result<()> {
        check_sequence_b(path, start, end, true)
    }

    fn check_sequence_b(path: &std::path::Path, start: u64, end: u64, bframes: bool) -> Result<()> {
        let probe = run(Command::new(fp()).args([
            "-v", "error", "-select_streams", "v:0", "-show_entries",
            "stream=duration_ts,time_base,duration,has_b_frames", "-of", "json",
        ]).arg(path))?;
        let probe: serde_json::Value = serde_json::from_slice(&probe)?;
        let stream = &probe["streams"][0];
        let total = end - start;
        // Stream-copy concat may choose a finer MP4 timescale. Compare exact
        // rational durations rather than assuming the encoder's 1/30 base.
        let (num, den) = stream["time_base"].as_str().context("missing time base")?
            .split_once('/').context("invalid time base")?;
        let (num, den): (u64, u64) = (num.parse()?, den.parse()?);
        ensure!(num > 0 && den > 0, "invalid time base: {probe}");
        let ticks = stream["duration_ts"].as_u64().context("missing duration_ts")?;
        assert_eq!(u128::from(ticks) * u128::from(num) * 30,
            u128::from(total) * u128::from(den), "{}: {probe}", path.display());
        let duration: f64 = stream["duration"].as_str().context("missing duration")?.parse()?;
        assert!((duration - total as f64 / 30.0).abs() < 0.000001, "{probe}");
        assert_eq!(stream["has_b_frames"].as_u64().unwrap_or(0) > 0, bframes, "B-frames: {probe}");

        let rgb = run(Command::new(ff()).args([
            "-v", "error", "-threads", "1",
        ]).arg("-i").arg(path).args([
            "-map", "0:v:0", "-an", "-filter_threads", "1", "-vf", "fps=30",
            "-pix_fmt", "rgb24", "-threads", "1", "-f", "rawvideo", "pipe:1",
        ]))?;
        let frame_bytes = WIDTH * HEIGHT * 3;
        assert_eq!(rgb.len(), total as usize * frame_bytes,
            "{}: wrong CFR frame count or partial frame", path.display());
        for (index, frame) in rgb.chunks_exact(frame_bytes).enumerate() {
            let expected = color_at(start + index as u64);
            for channel in 0..3 {
                let mean = frame.chunks_exact(3).map(|p| u64::from(p[channel])).sum::<u64>()
                    as f64 / (WIDTH * HEIGHT) as f64;
                // Allows integer BT.709 rounding and CRF 18 / qp 21 loss, but is much
                // smaller than any change in this sequence's palette.
                assert!((mean - f64::from(expected[channel])).abs() <= 6.0,
                    "{}: tick {}, channel {channel}: mean {mean}, expected {}",
                    path.display(), start + index as u64, expected[channel]);
            }
        }
        Ok(())
    }

    fn vfr_case(yuv: bool, known_total: bool) -> Result<()> {
        need_ffmpeg!();
        let dir = scratch(&format!("vfr-{yuv}-{known_total}"))?;
        let out = encode_sequence(&dir.0, "sequence", 0, 90, yuv, known_total)?;
        check_sequence(&out, 0, 90)
    }

    fn concat_case(yuv: bool, known_total: bool) -> Result<()> {
        need_ffmpeg!();
        let dir = scratch(&format!("concat-{yuv}-{known_total}"))?;
        let first = encode_sequence(&dir.0, "first", 0, 45, yuv, known_total)?;
        let second = encode_sequence(&dir.0, "second", 45, 90, yuv, known_total)?;
        check_sequence(&first, 0, 45)?;
        check_sequence(&second, 45, 90)?;
        let list = dir.0.join("list.txt");
        std::fs::write(&list, "file 'first.mp4'\nfile 'second.mp4'\n")?;
        let joined = dir.0.join("joined.mp4");
        run(Command::new(ff()).args([
            "-v", "error", "-f", "concat", "-safe", "0", "-i",
        ]).arg(list).args(["-c", "copy"]).arg(&joined))?;
        check_sequence(&joined, 0, 90)
    }

    #[test]
    fn vfr_b_frames_rgba_known() -> Result<()> { vfr_case(false, true) }
    #[test]
    fn vfr_b_frames_rgba_unknown() -> Result<()> { vfr_case(false, false) }
    #[test]
    fn vfr_b_frames_i420_known() -> Result<()> { vfr_case(true, true) }
    #[test]
    fn vfr_b_frames_i420_unknown() -> Result<()> { vfr_case(true, false) }
    #[test]
    fn vfr_concat_rgba_known() -> Result<()> { concat_case(false, true) }
    #[test]
    fn vfr_concat_rgba_unknown() -> Result<()> { concat_case(false, false) }
    #[test]
    fn vfr_concat_i420_known() -> Result<()> { concat_case(true, true) }
    #[test]
    fn vfr_concat_i420_unknown() -> Result<()> { concat_case(true, false) }

    /// Hardware VFR/concat cases; skip (with a message) when VAAPI is unavailable.
    fn hw_case(force_sg: bool, low_power: bool, concat: bool) -> Result<()> {
        need_ffmpeg!();
        let access = if force_sg {
            if !user_in_render_group() { eprintln!("skip: not in group render"); return Ok(()); }
            if let Err(e) = sg_probe() { eprintln!("skip: {e}"); return Ok(()); }
            Access::Sg
        } else {
            match detect(Codec::Vaapi) { Ok(a) => a, Err(e) => { eprintln!("skip: VAAPI unavailable: {e}"); return Ok(()); } }
        };
        let hw = HwOpts { access, low_power, ..HwOpts::default() };
        hw_sequences(Codec::Vaapi, hw, &format!("hw-{force_sg}-{low_power}-{concat}"), concat, !low_power)
    }

    /// NVENC VFR/concat cases (no B-frames in VFR, see `Encoder::start`; HEVC also main10);
    /// skipped where no NVIDIA device exists.
    fn nvenc_case(codec: Codec, ten_bit: bool, concat: bool) -> Result<()> {
        need_ffmpeg!();
        let _session = nvenc_test_lock();
        let access = match detect(codec) { Ok(a) => a, Err(e) => { eprintln!("skip: {codec:?} unavailable: {e}"); return Ok(()); } };
        let hw = HwOpts { access, ten_bit, qp: codec.default_quality(ten_bit), ..HwOpts::default() };
        hw_sequences(codec, hw, &format!("{codec:?}-{ten_bit}-{concat}"), concat, false)
    }

    /// Serializes the NVENC tests: the test harness runs more threads than the driver's
    /// session cap (5 on consumer drivers of that time).
    fn nvenc_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn hw_sequences(codec: Codec, hw: HwOpts, label: &str, concat: bool, bframes: bool) -> Result<()> {
        let dir = scratch(label)?;
        for known in [true, false] {
            if !concat {
                let out = encode_sequence_with(&dir.0, &format!("seq-{known}"), 0, 90, true, known, codec, hw.clone())?;
                check_sequence_b(&out, 0, 90, bframes)?;
                continue;
            }
            let first = encode_sequence_with(&dir.0, &format!("first-{known}"), 0, 45, true, known, codec, hw.clone())?;
            let second = encode_sequence_with(&dir.0, &format!("second-{known}"), 45, 90, true, known, codec, hw.clone())?;
            check_sequence_b(&first, 0, 45, bframes)?;
            check_sequence_b(&second, 45, 90, bframes)?;
            let list = dir.0.join(format!("list-{known}.txt"));
            std::fs::write(&list, format!("file 'first-{known}.mp4'\nfile 'second-{known}.mp4'\n"))?;
            let joined = dir.0.join(format!("joined-{known}.mp4"));
            run(Command::new(ff()).args(["-v", "error", "-f", "concat", "-safe", "0", "-i"])
                .arg(list).args(["-c", "copy"]).arg(&joined))?;
            check_sequence_b(&joined, 0, 90, bframes)?;
        }
        Ok(())
    }

    #[test]
    fn vaapi_vfr_b_frames() -> Result<()> { hw_case(false, false, false) }
    #[test]
    fn vaapi_vfr_low_power() -> Result<()> { hw_case(false, true, false) }
    #[test]
    fn vaapi_concat() -> Result<()> { hw_case(false, false, true) }
    #[test]
    fn vaapi_vfr_through_sg() -> Result<()> { hw_case(true, false, false) }
    #[test]
    fn nvenc_vfr() -> Result<()> { nvenc_case(Codec::Nvenc, false, false) }
    #[test]
    fn nvenc_concat() -> Result<()> { nvenc_case(Codec::Nvenc, false, true) }
    #[test]
    fn nvenc_hevc_vfr() -> Result<()> { nvenc_case(Codec::NvencHevc, false, false) }
    #[test]
    fn nvenc_hevc_concat() -> Result<()> { nvenc_case(Codec::NvencHevc, false, true) }
    #[test]
    fn nvenc_hevc_10bit_vfr() -> Result<()> { nvenc_case(Codec::NvencHevc, true, false) }
    #[test]
    fn nvenc_hevc_10bit_concat() -> Result<()> { nvenc_case(Codec::NvencHevc, true, true) }

    /// NVENC H.264 keeps its B-frames on a dense (CFR) timeline.
    #[test]
    fn nvenc_cfr_b_frames() -> Result<()> {
        need_ffmpeg!();
        let _session = nvenc_test_lock();
        let access = match detect(Codec::Nvenc) { Ok(a) => a, Err(e) => { eprintln!("skip: NVENC unavailable: {e}"); return Ok(()); } };
        let dir = scratch("nvenc-cfr")?;
        let out = dir.0.join("cfr.mp4");
        let hw = HwOpts { access, qp: Codec::Nvenc.default_quality(false), ..HwOpts::default() };
        let mut enc = Encoder::start(EncoderOpts {
            ffmpeg: ff(), out: out.clone(), width: WIDTH as u32, height: HEIGHT as u32, fps: 30,
            mode: Mode::Cfr, crf: 18, preset: "veryfast".into(), log: dir.0.join("cfr.log"), codec: Codec::Nvenc, hw: hw.clone(),
            threads: 0, yuv: true, frames: 90,
        })?;
        let mut f = I420Frame::for_codec(Codec::Nvenc, &hw, WIDTH, HEIGHT);
        for tick in 0..90 {
            if tick == 0 || CHANGES.iter().any(|&(n, _)| n == tick) {
                let [r, g, b] = color_at(tick);
                f.update(&[r, g, b, 255].repeat(WIDTH * HEIGHT), &[[0, 0, WIDTH as i32, HEIGHT as i32]]);
                enc.frame(tick, f.data())?;
            } else {
                enc.repeat(tick)?;
            }
        }
        enc.finish(90)?;
        check_sequence_b(&out, 0, 90, true)
    }

    #[test]
    fn nvenc_session_cap_from_driver_version() {
        let v = |s: &str| driver_session_cap(&format!("NVRM version: NVIDIA UNIX x86_64 Kernel Module  {s}  Wed Mar 25 15:26:15 UTC 2026"));
        assert_eq!(v("535.309.01"), 5);
        assert_eq!(v("470.256.02"), 3);
        assert_eq!(v("550.54.14"), 8);
        assert_eq!(driver_session_cap(""), 3);
    }

    /// 10-bit conversion: exact anchors (black 64, white 940, greys 512), every sample
    /// within 0.55 code (0.5 rounding + coefficient rounding) of the float BT.709 reference, damage updates == full conversion.
    #[test]
    fn p10_conversion() {
        let (w, h) = (66usize, 50usize);
        let mut seed = 5;
        let mut rgba: Vec<u8> = (0..w * h * 4).map(|_| random_byte(&mut seed)).collect();
        let sample = |f: &I420Frame, i: usize| u16::from_le_bytes([f.data()[2 * i], f.data()[2 * i + 1]]) as f64;
        let check = |f: &I420Frame, rgba: &[u8]| {
            let px = |x: usize, y: usize, c: usize| rgba[(y * w + x) * 4 + c] as f64;
            for y in 0..h { for x in 0..w {
                let yv = 64.0 + 876.0 / 255.0 * (0.2126 * px(x, y, 0) + 0.7152 * px(x, y, 1) + 0.0722 * px(x, y, 2));
                assert!((sample(f, y * w + x) - yv).abs() <= 0.55, "Y at {x},{y}");
            } }
            for cy in 0..h / 2 { for cx in 0..w / 2 {
                let avg = |c| (0..4).map(|k| px(2 * cx + k % 2, 2 * cy + k / 2, c)).sum::<f64>() / 4.0;
                let (r, g, b) = (avg(0), avg(1), avg(2));
                let yl = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                let (u, v) = (512.0 + 896.0 / 255.0 * (b - yl) / 1.8556, 512.0 + 896.0 / 255.0 * (r - yl) / 1.5748);
                let i = w * h + cy * (w / 2) + cx;
                assert!((sample(f, i) - u).abs() <= 0.55, "U at {cx},{cy}: {} vs {u}", sample(f, i));
                assert!((sample(f, i + w * h / 4) - v).abs() <= 0.55, "V at {cx},{cy}");
            } }
        };
        let mut f = I420Frame::new_p10(w, h);
        f.update(&rgba, &[[0, 0, w as i32, h as i32]]);
        check(&f, &rgba);
        for tick in 0..20 {
            let x = random_byte(&mut seed) as usize % w;
            let y = random_byte(&mut seed) as usize % h;
            let (rw, rh) = (1 + random_byte(&mut seed) as usize % (w - x), 1 + random_byte(&mut seed) as usize % (h - y));
            for py in y..y + rh { for px in x..x + rw { for c in 0..3 { rgba[(py * w + px) * 4 + c] = random_byte(&mut seed); } } }
            f.update(&rgba, &[[x as i32, y as i32, rw as i32, rh as i32]]);
            let mut full = I420Frame::new_p10(w, h);
            full.update(&rgba, &[[0, 0, w as i32, h as i32]]);
            assert_eq!(f.data(), full.data(), "tick {tick}");
        }
        check(&f, &rgba);
        for (c, y) in [(0u8, 64.0f64), (255, 940.0), (128, 64.0 + 876.0 * 128.0 / 255.0)] {
            let mut g = I420Frame::new_p10(2, 2);
            g.update(&[c, c, c, 255].repeat(4), &[[0, 0, 2, 2]]);
            assert_eq!(sample(&g, 0), y.round(), "Y of grey {c}");
            assert_eq!((sample(&g, 4), sample(&g, 5)), (512.0, 512.0), "chroma of grey {c}");
        }
    }

    #[test]
    fn vaapi_drop_kills_ffmpeg_through_sg() -> Result<()> {
        need_ffmpeg!();
        if !user_in_render_group() || sg_probe().is_err() { eprintln!("skip: sg render unavailable"); return Ok(()); }
        let dir = scratch("hw-drop")?;
        let mut enc = Encoder::start(EncoderOpts {
            ffmpeg: ff(), out: dir.0.join("x.mp4"), width: 64, height: 48, fps: 30,
            mode: Mode::Vfr, crf: 18, preset: "veryfast".into(), log: dir.0.join("x.log"), codec: Codec::Vaapi,
            hw: HwOpts { access: Access::Sg, ..HwOpts::default() }, threads: 0, yuv: true, frames: 0,
        })?;
        let mut f = I420Frame::new_nv12(64, 48);
        f.update(&[90u8; 64 * 48 * 4], &[[0, 0, 64, 48]]);
        enc.frame(0, f.data())?;
        let pid = enc.child.id();
        drop(enc);
        // `exec` in the sg command line makes the child ffmpeg itself (same pid).
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists() || std::fs::read_to_string(format!("/proc/{pid}/stat")).map(|s| s.contains(") Z")).unwrap_or(true), "ffmpeg survived drop");
        Ok(())
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(shell_quote("-vf"), "-vf");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("settb=expr=1/30,format=nv12,hwupload"), "settb=expr=1/30,format=nv12,hwupload");
        // round trip through a real shell
        for w in ["plain", "a b", "it's", "$HOME", "`x`", "a\"b", "x;y", "*", "\\n"] {
            let out = Command::new("sh").arg("-c").arg(format!("printf %s {}", shell_quote(w))).output().unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), w);
        }
    }

    #[test]
    fn nv12_incremental_matches_scalar() {
        let mut seed = 7;
        for (w, h) in [(2, 2), (18, 10), (66, 48), (322, 258)] {
            let mut rgba: Vec<u8> = (0..w * h * 4).map(|_| random_byte(&mut seed)).collect();
            let mut actual = I420Frame::new_nv12(w, h);
            let mut fallback = vec![0; i420_len(w, h)];
            let mut expected = Vec::new();
            actual.update(&rgba, &[[0, 0, w as i32, h as i32]]);
            for tick in 0..30 {
                let x = random_byte(&mut seed) as usize % w;
                let y = random_byte(&mut seed) as usize % h;
                let rw = 1 + random_byte(&mut seed) as usize % (w - x);
                let rh = 1 + random_byte(&mut seed) as usize % (h - y);
                let rects = [[x as i32, y as i32, rw as i32, rh as i32], [w as i32 - 1, h as i32 - 1, 1, 1]];
                for [x, y, rw, rh] in rects {
                    for py in y..y + rh {
                        for px in x..x + rw {
                            let at = (py as usize * w + px as usize) * 4;
                            for c in 0..3 { rgba[at + c] = random_byte(&mut seed); }
                        }
                    }
                }
                actual.update(&rgba, &rects);
                scalar_reference(&rgba, w, h, &mut expected);
                let (yp, uv) = expected.split_at(w * h);
                let (up, vp) = uv.split_at(w * h / 4);
                let mut nv: Vec<u8> = yp.to_vec();
                for (u, v) in up.iter().zip(vp) { nv.push(*u); nv.push(*v); }
                convert_rect(&rgba, w, h, &mut fallback, [0, 0, w, h], false, true);
                assert_eq!(actual.data(), nv, "nv12 {w}x{h} tick {tick}");
                assert_eq!(fallback, nv, "nv12 scalar {w}x{h} tick {tick}");
            }
        }
    }

    /// Colour patches through each encoder (x264 I420, VAAPI NV12), decoded by ffmpeg to
    /// RGB honouring the stream's BT.709 tags: every patch within a few levels.
    #[test]
    fn colour_patches_round_trip() -> Result<()> {
        need_ffmpeg!();
        let _session = nvenc_test_lock();
        let patches: [[u8; 3]; 8] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255],
            [20, 20, 20], [53, 145, 56], [240, 200, 40], [30, 60, 120]];
        let (pw, ph) = (32usize, 32usize);
        let (w, h) = (pw * 4, ph * 2);
        let mut rgba = vec![255u8; w * h * 4];
        for (i, c) in patches.iter().enumerate() {
            let (px, py) = (i % 4 * pw, i / 4 * ph);
            for y in py..py + ph { for x in px..px + pw {
                rgba[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(c);
            } }
        }
        let dir = scratch("patches")?;
        // (codec, hw, rgba pipe): the RGBA pipe is converted by ffmpeg, also BT.709 (review M3 #7)
        let mut codecs = vec![(Codec::X264, HwOpts::default(), false), (Codec::X264, HwOpts::default(), true)];
        match detect(Codec::Vaapi) { Ok(a) => codecs.push((Codec::Vaapi, HwOpts { access: a, qp: 10, ..HwOpts::default() }, false)), Err(e) => eprintln!("skip VAAPI: {e}") }
        for (codec, ten_bit) in [(Codec::Nvenc, false), (Codec::NvencHevc, false), (Codec::NvencHevc, true)] {
            match detect(codec) {
                Ok(a) => codecs.push((codec, HwOpts { access: a, qp: 10, ten_bit, ..HwOpts::default() }, false)),
                Err(e) => eprintln!("skip {codec:?}: {e}"),
            }
        }
        for (codec, hw, rgba_pipe) in codecs {
            let name = format!("{codec:?}-{}-{rgba_pipe}", hw.ten_bit);
            let out = dir.0.join(format!("{name}.mp4"));
            let mut enc = Encoder::start(EncoderOpts {
                ffmpeg: ff(), out: out.clone(), width: w as u32, height: h as u32, fps: 30,
                mode: Mode::Vfr, crf: 10, preset: "veryfast".into(), log: dir.0.join(format!("{name}.log")), codec, hw: hw.clone(),
                threads: 1, yuv: !rgba_pipe, frames: 4,
            })?;
            if rgba_pipe {
                enc.frame(0, &rgba)?;
            } else {
                let mut f = I420Frame::for_codec(codec, &hw, w, h);
                f.update(&rgba, &[[0, 0, w as i32, h as i32]]);
                enc.frame(0, f.data())?;
            }
            for n in 1..4 { enc.repeat(n)?; }
            enc.finish(4)?;
            let tags = run(Command::new(fp()).args(["-v", "error", "-select_streams", "v:0",
                "-show_entries", "stream=color_space,color_primaries,color_transfer,color_range", "-of", "csv=p=0"]).arg(&out))?;
            assert_eq!(String::from_utf8_lossy(&tags).trim(), "tv,bt709,bt709,bt709", "{name} tags");
            let rgb = run(Command::new(ff()).args(["-v", "error", "-i"]).arg(&out)
                .args(["-frames:v", "1", "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"]))?;
            for (i, c) in patches.iter().enumerate() {
                // patch centre, away from chroma-subsampling edges
                let (x, y) = (i % 4 * pw + pw / 2, i / 4 * ph + ph / 2);
                let p = &rgb[(y * w + x) * 3..(y * w + x) * 3 + 3];
                for k in 0..3 {
                    assert!((p[k] as i32 - c[k] as i32).abs() <= 4, "{name} patch {c:?} decoded {p:?}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn rgba_to_i420_bt709_limited() {
        // Integer BT.709 limited-range reference values, with rounding.
        for (rgb, expected) in [
            ([0, 0, 0], [16, 128, 128]),
            ([255, 255, 255], [235, 128, 128]),
            ([255, 0, 0], [63, 102, 240]),
            ([0, 255, 0], [172, 42, 26]),
            ([0, 0, 255], [32, 240, 118]),
        ] {
            let [r, g, b] = rgb;
            let mut out = Vec::new();
            rgba_to_i420(&[r, g, b, 255].repeat(8), 4, 2, &mut out);
            assert_eq!(out.len(), 12);
            assert_eq!(&out[..8], &[expected[0]; 8], "Y for {rgb:?}");
            assert_eq!(&out[8..10], &[expected[1]; 2], "U for {rgb:?}");
            assert_eq!(&out[10..], &[expected[2]; 2], "V for {rgb:?}");
        }
    }

    #[test]
    fn rgba_to_i420_rejects_odd_dimensions() {
        // I420 input to Encoder requires even dimensions. Reject instead of
        // silently omitting chroma for the unmatched final row or column.
        for (w, h) in [(3, 2), (2, 3), (3, 3)] {
            let result = std::panic::catch_unwind(|| {
                rgba_to_i420(&vec![255; w * h * 4], w, h, &mut Vec::new());
            });
            assert!(result.is_err(), "accepted odd dimensions {w}x{h}");
        }
    }

    #[test]
    fn timestamps_and_input_validation() -> Result<()> {
        need_ffmpeg!();
        let dir = PathBuf::from(format!("target/encode-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let _cleanup = Scratch(dir.clone());
        // Includes one-image videos, endpoint duplication, a last unique image,
        // and a gap exceeding SimpleBlock's signed relative timestamp range.
        for (case, (fps, total, final_unique)) in [(30, 1, false), (24, 2, false), (60, 61, true), (30, 1201, false), (1000, 5, false)].into_iter().enumerate() {
            let out = dir.join(format!("{case}.mp4"));
            let mut encoder = Encoder::start(EncoderOpts {
                ffmpeg: ff(), out: out.clone(),
                width: 16, height: 16, fps, mode: Mode::Vfr, crf: 18,
                preset: "ultrafast".into(), log: dir.join(format!("{case}.log")), threads: 0, yuv: false, frames: 0,
                codec: Codec::X264, hw: HwOpts::default(),
            })?;
            assert!(encoder.repeat(0).is_err());
            assert!(encoder.frame(1, &[0; 1024]).is_err());
            assert!(encoder.frame(0, &[0; 4]).is_err());
            encoder.frame(0, &[255; 1024])?;
            for n in 1..total {
                if final_unique && n == total - 1 { encoder.frame(n, &[128; 1024])?; }
                else { encoder.repeat(n)?; }
            }
            let stats = encoder.finish(total)?;
            assert_eq!(stats.frames_sent, if total == 1 { 1 } else { 2 });
            let probe = Command::new(fp()).args(["-v", "error", "-show_entries", "stream=duration_ts,time_base", "-of", "default=noprint_wrappers=1"]).arg(out).output()?;
            ensure!(probe.status.success(), "ffprobe failed");
            let result = String::from_utf8(probe.stdout)?;
            ensure!(result.lines().any(|l| l == format!("duration_ts={total}")), "wrong duration: {result}; log: {}", std::fs::read_to_string(dir.join(format!("{case}.log")))?);
            ensure!(result.lines().any(|l| l == format!("time_base=1/{fps}")), "wrong time base: {result}");
        }
        Ok(())
    }
}
