//! Audio reactivity (X10): offline music analysis at compile time and its lowering to
//! ordinary elements + linear keyframe tracks. The renderer needs no audio code.
//!
//! - `analyze`: ffmpeg decodes to 16 kHz mono f32, then per output frame: RMS level, onset
//!   strength (spectral flux) and 8 log-spaced band energies (40 Hz..8 kHz), each normalized
//!   to the track's peak (0..1), plus a 64-point waveform. A beat grid (tempo + beat times)
//!   comes from onset autocorrelation with a 120 bpm prior, refined against a comb filter;
//!   `bpm=` overrides the tempo (the phase is still fitted). Cached by content hash under
//!   `<doc>/cache/audio/`.
//! - `anchors`: `beat:N` / `bar:N` (4 beats per bar) join every scene's anchors.
//! - `expand`: `<channel>=audio(...)` props, `pulse ... on=beat|bar` and `visualizer` become
//!   `anim` / `layer` / `rect` nodes before the normal compiler runs.
use crate::ast::{Document, Node, Span, Value};
use crate::diag::Diag;
use crate::ir::Ch;
use crate::timing::{AnchorT, Timeline};
use crate::values::parse_dur;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path, process::Command};

pub const RATE: usize = 16000;
const FFT: usize = 1024;
const HOP: usize = 128;
const WAVE: usize = 64;
const VERSION: &str = "audio-v6";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Features {
    pub fps: f64,
    pub duration: f64,
    pub bpm: f64,
    pub beats: Vec<f64>,
    pub frames: Vec<Frame>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub level: f64,
    pub onset: f64,
    pub bands: [f64; 8],
    pub wave: Vec<f64>,
}
impl Features {
    fn frame(&self, t: f64) -> Option<&Frame> {
        if t < 0.0 { return None; }
        self.frames.get((t * self.fps).round() as usize)
    }
}

fn raw(n: &Node, k: &str) -> Option<String> { n.props.get(k).map(Value::raw) }
fn error(msg: impl Into<String>, span: Span) -> Diag {
    Diag::error("E_AUDIO", msg, "audio reactivity needs one top-level `music src=<file>` that ffmpeg can decode (optional bpm=30..300) and valid audio(...) parameters", span)
}
fn grid_ref(s: &str) -> bool {
    let at = |s: &str| ["beat:", "bar:"].iter().any(|p| s.strip_prefix(p).is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit())));
    at(s.trim_start_matches('@')) || s.match_indices('@').any(|(i, _)| at(&s[i + 1..]))
}
fn reactive(n: &Node) -> bool {
    n.kind == "visualizer" || (n.kind == "pulse" && matches!(raw(n, "on").as_deref(), Some("beat" | "bar")))
        || n.props.values().any(|v| v.raw().starts_with("audio("))
}
/// Does the document use audio features at all? Documents that do not never touch ffmpeg.
pub fn needed(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| reactive(n) || n.props.values().chain(n.args.iter()).any(|v| grid_ref(&v.raw())) || needed(&n.children))
}

/// Decode + analyze, cached by source bytes, algorithm version, fps and explicit tempo.
pub fn analyze(path: &Path, cache: &Path, fps: f64, bpm: Option<f64>) -> Result<Features, String> {
    if !fps.is_finite() || !(1.0..=240.0).contains(&fps) { return Err("audio analysis needs fps 1..240".into()); }
    if bpm.is_some_and(|v| !v.is_finite() || !(30.0..=300.0).contains(&v)) { return Err("bpm must be 30..300".into()); }
    let mut input = fs::File::open(path).map_err(|e| format!("cannot read music `{}`: {e}", path.display()))?;
    let mut hash = Sha256::new();
    hash.update(format!("{VERSION}/{fps}/{bpm:?}/"));
    let mut buf = [0u8; 65536];
    loop {
        let n = input.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        hash.update(&buf[..n]);
    }
    let key = format!("{:x}", hash.finalize());
    let dest = cache.join(format!("{key}.json"));
    if let Ok(bytes) = fs::read(&dest) {
        if let Ok(f) = serde_json::from_slice(&bytes) { return Ok(f); }
    }
    let output = Command::new(crate::run::ffmpeg())
        .args(["-v", "error", "-nostdin", "-threads", "1", "-i"]).arg(path)
        .args(["-t", "601", "-vn", "-ac", "1", "-ar", &RATE.to_string(), "-f", "f32le", "pipe:1"])
        .output().map_err(|e| format!("cannot run ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(format!("cannot decode `{}`: {}", path.display(), String::from_utf8_lossy(&output.stderr).trim()));
    }
    let samples: Vec<f64> = output.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64).collect();
    if samples.is_empty() || samples.iter().any(|v| !v.is_finite()) { return Err(format!("`{}` has no decodable audio", path.display())); }
    if samples.len() > RATE * 600 { return Err("music longer than 10 minutes is not analysed; trim it".into()); }
    let f = features(&samples, fps, bpm);
    fs::create_dir_all(cache).map_err(|e| format!("audio cache: {e}"))?;
    let temp = cache.join(format!("{key}.{}.tmp", std::process::id()));
    fs::write(&temp, serde_json::to_vec(&f).unwrap()).map_err(|e| e.to_string())?;
    fs::rename(temp, dest).map_err(|e| e.to_string())?;
    Ok(f)
}

/// Hann-windowed power spectrum of `FFT` samples starting at `start` (zero padded).
fn spectrum(samples: &[f64], start: usize) -> Vec<f64> {
    static TABLES: std::sync::OnceLock<(Vec<f64>, Vec<(f64, f64)>)> = std::sync::OnceLock::new();
    let (window, twiddle) = TABLES.get_or_init(|| (
        (0..FFT).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (FFT - 1) as f64).cos()).collect(),
        (0..FFT / 2).map(|k| (-std::f64::consts::TAU * k as f64 / FFT as f64).sin_cos()).collect(),
    ));
    let mut re = vec![0.0; FFT];
    let mut im = vec![0.0; FFT];
    for (i, x) in samples.iter().skip(start).take(FFT).enumerate() { re[i] = x * window[i]; }
    for i in 0..FFT {
        let j = i.reverse_bits() >> (usize::BITS - FFT.ilog2());
        if j > i { re.swap(i, j); }
    }
    let mut len = 2;
    while len <= FFT {
        let step = FFT / len;
        for start in (0..FFT).step_by(len) {
            for j in 0..len / 2 {
                let (s, c) = twiddle[j * step];
                let (a, b) = (start + j, start + j + len / 2);
                let (r, q) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                re[b] = re[a] - r;
                im[b] = im[a] - q;
                re[a] += r;
                im[a] += q;
            }
        }
        len *= 2;
    }
    (0..FFT / 2).map(|i| re[i] * re[i] + im[i] * im[i]).collect()
}

fn band_of(bin: usize) -> Option<usize> {
    let hz = bin as f64 * RATE as f64 / FFT as f64;
    if hz < 40.0 { return None; }
    let b = ((hz / 40.0).ln() / (200.0_f64.ln() / 8.0)).floor() as usize;
    (b < 8).then_some(b)
}

/// Onset envelopes at `RATE / HOP` Hz: half-wave rectified flux of log-compressed magnitudes,
/// full band (tempo, onset feature) and weighted toward kick/bass/mids (beat phase, so off-beat
/// hi-hats do not pull the grid half a beat).
fn onset_envelopes(samples: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let hops = samples.len().div_ceil(HOP);
    let weight: Vec<f64> = (0..FFT / 2).map(|k| {
        let khz = k as f64 * RATE as f64 / FFT as f64 / 1000.0;
        1.0 / (1.0 + khz * khz)
    }).collect();
    let mut prev: Vec<f64> = vec![0.0; FFT / 2];
    let (mut full, mut low) = (Vec::with_capacity(hops), Vec::with_capacity(hops));
    for h in 0..hops {
        let start = (h * HOP).saturating_sub(FFT / 2);
        let mag: Vec<f64> = spectrum(samples, start).iter().map(|p| (1.0 + 100.0 * p.sqrt()).ln()).collect();
        let rise = mag.iter().zip(&prev).map(|(a, b)| if h == 0 { 0.0 } else { (a - b).max(0.0) });
        let (f, l) = rise.zip(&weight).fold((0.0, 0.0), |(f, l), (r, w)| (f + r, l + w * r));
        full.push(f);
        low.push(l);
        prev = mag;
    }
    (detrend(&full), detrend(&low))
}

/// Remove the slow trend so sustained loudness does not read as onsets.
fn detrend(env: &[f64]) -> Vec<f64> {
    let w = 16;
    (0..env.len()).map(|i| {
        let (a, b) = (i.saturating_sub(w), (i + w + 1).min(env.len()));
        (env[i] - env[a..b].iter().sum::<f64>() / (b - a) as f64).max(0.0)
    }).collect()
}

/// Comb score of a beat grid (period and phase in envelope samples).
fn comb(env: &[f64], period: f64, phase: f64) -> f64 {
    let mut t = phase;
    let mut sum = 0.0;
    while (t as usize) < env.len() {
        let i = t.round() as usize;
        sum += env.get(i).copied().unwrap_or(0.0).max(env.get(i.wrapping_sub(1)).copied().unwrap_or(0.0)).max(env.get(i + 1).copied().unwrap_or(0.0));
        t += period;
    }
    sum
}

/// Tempo (bpm) and first-beat time from an onset envelope.
fn beat_grid(env: &[f64], phase_env: &[f64], duration: f64, override_bpm: Option<f64>) -> (f64, f64) {
    let rate = RATE as f64 / HOP as f64;
    let n = env.len();
    let period = match override_bpm {
        Some(b) => 60.0 * rate / b,
        None => {
            let (lo, hi) = ((60.0 * rate / 200.0) as usize, (60.0 * rate / 50.0).ceil() as usize);
            if n <= hi + 2 { 60.0 * rate / 120.0 } else {
                let ac = |lag: usize| (lag..n).map(|i| env[i] * env[i - lag]).sum::<f64>() / (n - lag) as f64;
                let scores: Vec<f64> = (lo..=hi).map(|lag| {
                    let bpm = 60.0 * rate / lag as f64;
                    // log-gaussian tempo prior around 120 bpm resolves octave ambiguity
                    ac(lag) * (-0.5 * ((bpm / 120.0).log2() / 1.0).powi(2)).exp()
                }).collect();
                let k = (0..scores.len()).max_by(|&a, &b| scores[a].total_cmp(&scores[b]).then(b.cmp(&a))).unwrap_or(0);
                // parabolic peak interpolation for a sub-sample period
                let frac = if k > 0 && k + 1 < scores.len() {
                    let (a, b, c) = (scores[k - 1], scores[k], scores[k + 1]);
                    let d = a - 2.0 * b + c;
                    if d.abs() > 1e-18 { (0.5 * (a - c) / d).clamp(-0.5, 0.5) } else { 0.0 }
                } else { 0.0 };
                let coarse = (lo + k) as f64 + frac;
                // refine against the comb over the whole track (+-2 %, 0.02 % steps)
                let best = |p: f64| (0..p.ceil() as usize).map(|ph| comb(env, p, ph as f64)).fold(0.0, f64::max);
                (-100..=100).map(|s| coarse * (1.0 + s as f64 * 0.0002))
                    .map(|p| (p, best(p))).fold((coarse, f64::MIN), |acc, x| if x.1 > acc.1 + 1e-12 { x } else { acc }).0
            }
        }
    };
    let phases = period.ceil().max(1.0) as usize;
    let phase = (0..phases).map(|p| (p, comb(phase_env, period, p as f64)))
        .fold((0usize, f64::MIN), |acc, x| if x.1 > acc.1 + 1e-12 { x } else { acc }).0 as f64;
    // least-squares fit of the onset peaks near each grid beat: sub-sample period and phase
    let mut pts = vec![];
    let mut k = 0usize;
    loop {
        let t = phase + k as f64 * period;
        if t as usize >= n { break; }
        let (a, b) = ((t - period / 4.0).max(0.0) as usize, ((t + period / 4.0) as usize + 1).min(n));
        if let Some(i) = (a..b).max_by(|&x, &y| phase_env[x].total_cmp(&phase_env[y]).then(y.cmp(&x))) {
            if phase_env[i] > 0.0 {
                // parabolic peak position
                let (l, c, r) = (phase_env.get(i.wrapping_sub(1)).copied().unwrap_or(0.0), phase_env[i], phase_env.get(i + 1).copied().unwrap_or(0.0));
                let d = l - 2.0 * c + r;
                let off = if d.abs() > 1e-18 { (0.5 * (l - r) / d).clamp(-0.5, 0.5) } else { 0.0 };
                pts.push((k as f64, i as f64 + off, c));
            }
        }
        k += 1;
    }
    let sw: f64 = pts.iter().map(|p| p.2).sum();
    let (period, phase) = if sw <= 0.0 || pts.len() < 2 { (period, phase) } else if override_bpm.is_some() {
        (period, pts.iter().map(|(k, t, w)| w * (t - k * period)).sum::<f64>() / sw)
    } else {
        let (mk, mt) = (pts.iter().map(|p| p.2 * p.0).sum::<f64>() / sw, pts.iter().map(|p| p.2 * p.1).sum::<f64>() / sw);
        let var = pts.iter().map(|p| p.2 * (p.0 - mk).powi(2)).sum::<f64>();
        let slope = if var > 0.0 { pts.iter().map(|p| p.2 * (p.0 - mk) * (p.1 - mt)).sum::<f64>() / var } else { period };
        // trust the fit only near the comb's period
        let slope = if (slope / period - 1.0).abs() < 0.03 { slope } else { period };
        (slope, mt - slope * mk)
    };
    let phase = phase.rem_euclid(period);
    let mut first = phase / rate;
    // a beat up to 40 ms before the start (encoder delay, a cut just after the beat) counts as beat 1
    if first - period / rate >= -0.04 { first = (first - period / rate).max(0.0); }
    let first = first.min(duration.max(0.0));
    (60.0 * rate / period, first)
}

pub fn features(samples: &[f64], fps: f64, override_bpm: Option<f64>) -> Features {
    let duration = samples.len() as f64 / RATE as f64;
    let (env, phase_env) = onset_envelopes(samples);
    let count = (duration * fps).ceil() as usize;
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        let start = ((i as f64 * RATE as f64 / fps).round() as usize).min(samples.len());
        let end = (((i + 1) as f64 * RATE as f64 / fps).round() as usize).min(samples.len());
        let block = &samples[start..end.max(start)];
        let level = (block.iter().map(|x| x * x).sum::<f64>() / block.len().max(1) as f64).sqrt();
        let center = (start + end) / 2;
        let power = spectrum(samples, center.saturating_sub(FFT / 2));
        let mut bands = [0.0; 8];
        for (k, p) in power.iter().enumerate().skip(1) {
            if let Some(b) = band_of(k) { bands[b] += p; }
        }
        for b in &mut bands { *b = b.sqrt(); }
        let (h0, h1) = (start / HOP, (end.div_ceil(HOP)).max(start / HOP + 1).min(env.len()));
        let onset = env.get(h0..h1).map(|s| s.iter().copied().fold(0.0, f64::max)).unwrap_or(0.0);
        let wave = (0..WAVE).map(|j| block.get(j * block.len() / WAVE).copied().unwrap_or(0.0)).collect();
        frames.push(Frame { level, onset, bands, wave });
    }
    let peak = |get: &dyn Fn(&Frame) -> f64| frames.iter().map(get).fold(0.0, f64::max).max(1e-9);
    let (pl, po) = (peak(&|f| f.level), peak(&|f| f.onset));
    let pw = peak(&|f| f.wave.iter().fold(0.0, |a: f64, b| a.max(b.abs())));
    let pb: Vec<f64> = (0..8).map(|b| peak(&|f| f.bands[b])).collect();
    let q = |v: f64| (v * 1e4).round() / 1e4;
    for f in &mut frames {
        f.level = q(f.level / pl);
        f.onset = q(f.onset / po);
        for (b, p) in f.bands.iter_mut().zip(&pb) { *b = q(*b / p); }
        for w in &mut f.wave { *w = q(*w / pw); }
    }
    let (bpm, first) = beat_grid(&env, &phase_env, duration, override_bpm);
    let period = 60.0 / bpm;
    let beats = (0..).map(|i| first + i as f64 * period).take_while(|t| *t < duration).map(|t| (t * 1e4).round() / 1e4).collect();
    Features { fps, duration, bpm: (bpm * 100.0).round() / 100.0, beats, frames }
}

/// Analyze the document's music when the document uses audio features. Errors go to `diags`.
pub fn load(doc: &Document, base: &Path, fps: f64, diags: &mut Vec<Diag>) -> Option<Features> {
    if !needed(&doc.nodes) { return None; }
    let music: Vec<_> = doc.nodes.iter().filter(|n| n.kind == "music").collect();
    let Some(n) = music.first() else {
        diags.push(error("audio reactivity (audio(), on=beat, visualizer, beat:/bar: anchors) needs a top-level `music src=`", Span::default()));
        return None;
    };
    if music.len() > 1 {
        diags.push(Diag::warn("W_AUDIO_MUSIC", "several music nodes; audio reactivity follows the first", "keep one music bed", music[1].span));
    }
    let result = (|| {
        let src = raw(n, "src").ok_or("music needs src=")?;
        let bpm = raw(n, "bpm").map(|s| s.parse::<f64>().map_err(|_| format!("invalid bpm `{s}`"))).transpose()?;
        analyze(&base.join(&src), &base.join("cache/audio"), fps, bpm)
    })();
    match result {
        Ok(f) => Some(f),
        Err(e) => { diags.push(error(e, n.span)); None }
    }
}

/// `beat:1..` and `bar:1..` (every 4th beat) as anchors of every scene (absolute times).
pub fn anchors(tl: &mut Timeline, f: &Features) {
    for scene in &mut tl.scenes {
        for (i, &t) in f.beats.iter().enumerate() {
            scene.anchors.push(AnchorT { name: format!("beat:{}", i + 1), start: t, end: t, say: usize::MAX });
            if i % 4 == 0 {
                scene.anchors.push(AnchorT { name: format!("bar:{}", i / 4 + 1), start: t, end: t, say: usize::MAX });
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drive { pub band: Option<f64>, pub onset: bool, pub gain: f64, pub smooth: f64, pub offset: f64 }
impl Drive {
    pub fn parse(s: &str, fps: f64) -> Result<Self, String> {
        let inner = s.trim().strip_prefix("audio(").and_then(|s| s.strip_suffix(')'))
            .ok_or("write audio(level), audio(onset) or audio(band=0..7, gain=1, smooth=80ms, offset=0)")?;
        let mut d = Self { band: None, onset: false, gain: 1.0, smooth: 0.0, offset: 0.0 };
        for part in inner.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part {
                "level" => {}
                "onset" | "beat" => d.onset = true,
                _ => {
                    let (k, v) = part.split_once('=').ok_or(format!("unknown audio feature `{part}` (level, onset, band=)"))?;
                    let v = v.trim();
                    match k.trim() {
                        "band" => {
                            let b = v.parse::<f64>().map_err(|_| "band must be 0..7")?;
                            if !(0.0..=7.0).contains(&b) { return Err("band must be 0..7".into()); }
                            d.band = Some(b);
                        }
                        "gain" => d.gain = v.parse().map_err(|_| format!("invalid gain `{v}`"))?,
                        "offset" => d.offset = v.parse().map_err(|_| format!("invalid offset `{v}`"))?,
                        "smooth" => d.smooth = parse_dur(v).map_err(|_| format!("invalid smooth `{v}` (write 80ms)"))?.secs(fps),
                        k => return Err(format!("unknown audio parameter `{k}` (band, gain, smooth, offset)")),
                    }
                }
            }
        }
        if !d.gain.is_finite() || !d.offset.is_finite() || !d.smooth.is_finite() || d.smooth < 0.0 {
            return Err("audio gain/offset must be finite numbers and smooth >= 0".into());
        }
        if d.onset && d.band.is_some() { return Err("use either onset or band=, not both".into()); }
        Ok(d)
    }
    fn raw_value(&self, v: &Frame) -> f64 {
        match self.band {
            Some(b) => {
                let (i, t) = (b.floor() as usize, b.fract());
                v.bands[i] * (1.0 - t) + v.bands[(i + 1).min(7)] * t
            }
            None if self.onset => v.onset,
            None => v.level,
        }
    }
    /// One value per output frame of [start, start+dur]. The one-pole smoother runs from the
    /// track start, so scene cuts never reset the envelope.
    pub fn values(&self, f: &Features, start: f64, dur: f64) -> Vec<f64> {
        let alpha = if self.smooth == 0.0 { 1.0 } else { 1.0 - (-1.0 / (f.fps * self.smooth)).exp() };
        let mut prev = 0.0;
        let filtered: Vec<f64> = f.frames.iter().map(|v| { prev += alpha * (self.raw_value(v) - prev); prev }).collect();
        (0..=(dur * f.fps).ceil() as usize).map(|i| {
            let k = ((start + i as f64 / f.fps) * f.fps).round();
            let x = if k >= 0.0 { filtered.get(k as usize).copied().unwrap_or(0.0) } else { 0.0 };
            self.offset + self.gain * x
        }).collect()
    }
}

/// Keyframes `t:v` (one per frame, ease=linear), dropping keys that linear interpolation
/// between their neighbours reproduces within `1e-3` of the channel's range.
fn anim(id: &str, ch: &str, values: &[f64], fps: f64, span: Span) -> Node {
    let range = values.iter().fold(f64::MIN, |a, &b| a.max(b)) - values.iter().fold(f64::MAX, |a, &b| a.min(b));
    let eps = 1e-3 * range.max(1e-3);
    let mut keep = vec![0usize];
    for i in 1..values.len() {
        let last = *keep.last().unwrap();
        let end = i + 1;
        if end >= values.len() { keep.push(i); break; }
        // keep i if dropping it would bend any key between `last` and `end` off the line
        let ok = (last + 1..end).all(|j| {
            let t = (j - last) as f64 / (end - last) as f64;
            (values[last] + (values[end] - values[last]) * t - values[j]).abs() <= eps
        });
        if !ok { keep.push(i); }
    }
    let mut n = Node { kind: "anim".into(), args: vec![Value::Str(format!("#{id}")), Value::Str(ch.into())], span, ..Default::default() };
    n.args.extend(keep.into_iter().map(|i| Value::Str(format!("{}s:{}", crate::ast::fmt_num(i as f64 / fps), crate::ast::fmt_num(values[i])))));
    n.props.insert("ease".into(), Value::Str("linear".into()));
    n
}

/// Signal-only WCAG 2.3.1 screen: more than three flash pairs (opposing changes of at least
/// 0.1, i.e. 10 % of full opacity/brightness) within any one second.
pub fn flash(values: &[f64], fps: f64) -> bool {
    let mut turns = Vec::new();
    let mut extreme = values.first().copied().unwrap_or(0.0);
    let mut up = true;
    for (i, &v) in values.iter().enumerate() {
        if (up && v < extreme - 0.1) || (!up && v > extreme + 0.1) { turns.push(i); up = !up; extreme = v; }
        else if (up && v > extreme) || (!up && v < extreme) { extreme = v; }
    }
    turns.windows(7).any(|w| (w[6] - w[0]) as f64 / fps <= 1.0)
}

/// Lower audio features of a document. `None` when the document has none (output unchanged).
pub fn expand(doc: &Document, tl: &Timeline, base: &Path, diags: &mut Vec<Diag>) -> Option<Document> {
    if !needed(&doc.nodes) { return None; }
    // analysis errors were reported by the timing solver already
    let f = load(doc, base, tl.fps, &mut vec![]);
    let mut doc = doc.clone();
    expand_with(&mut doc, tl, f.as_ref(), diags);
    Some(doc)
}

/// Expansion against given features (`None`: analysis failed; reactive nodes degrade silently).
pub fn expand_with(doc: &mut Document, tl: &Timeline, f: Option<&Features>, diags: &mut Vec<Diag>) {
    for (si, scene) in doc.nodes.iter_mut().filter(|n| n.kind == "scene").enumerate() {
        let Some(sc) = tl.scenes.get(si) else { break };
        let mut extra = Vec::new();
        expand_nodes(&mut scene.children, None, f, sc.start, sc.dur, &mut extra, diags);
        scene.children.extend(extra);
    }
}

fn beat_pulse(n: &Node, id: &str, f: &Features, start: f64, dur: f64) -> Result<Node, String> {
    let amount = raw(n, "amount").unwrap_or("0.15".into()).parse::<f64>().map_err(|_| "pulse amount must be a number")?;
    let decay = raw(n, "decay").map(|s| parse_dur(&s).map(|d| d.secs(f.fps))).transpose().map_err(|_| "pulse decay must be a duration (220ms)")?.unwrap_or(0.22);
    if !amount.is_finite() || !decay.is_finite() || decay <= 0.0 { return Err("pulse needs a finite amount and decay > 0".into()); }
    let every = if raw(n, "on").as_deref() == Some("bar") { 4 } else { 1 };
    let hits: Vec<f64> = f.beats.iter().step_by(every).copied().collect();
    let values: Vec<f64> = (0..=(dur * f.fps).ceil() as usize).map(|i| {
        let t = start + i as f64 / f.fps;
        let since = hits.iter().rev().find(|b| **b <= t + 1e-9).map(|b| t - b);
        // quadratic decay: instant attack on the beat, smooth settle
        1.0 + amount * since.map(|s| (1.0 - s / decay).max(0.0).powi(2)).unwrap_or(0.0)
    }).collect();
    let ch = raw(n, "channel").unwrap_or("scale".into());
    Ch::parse(&ch).filter(|c| !c.is_color()).ok_or(format!("pulse channel `{ch}` is not numeric"))?;
    Ok(anim(id, &ch, &values, f.fps, n.span))
}

fn expand_nodes(nodes: &mut Vec<Node>, parent: Option<&str>, f: Option<&Features>, start: f64, dur: f64, extra: &mut Vec<Node>, diags: &mut Vec<Diag>) {
    nodes.retain_mut(|n| {
        let own = crate::lower::id_arg(n);
        if n.kind == "visualizer" {
            let r = match f { Some(f) => visualizer(n, f, start, dur, extra), None => Err(String::new()) };
            if let Err(e) = r {
                if !e.is_empty() { diags.push(error(format!("visualizer: {e}"), n.span)); }
                // keep its box in the layout so the rest of the scene does not jump
                n.kind = "layer".into();
                n.children.clear();
            }
        }
        if n.kind == "pulse" && matches!(raw(n, "on").as_deref(), Some("beat" | "bar")) {
            let Some(f) = f else { return false };
            let target = own.clone().or(parent.map(str::to_owned));
            match target.ok_or_else(|| "pulse on=beat needs #target (or nest it under an element with an id)".to_string())
                .and_then(|id| beat_pulse(n, &id, f, start, dur)) {
                Ok(a) => extra.push(a),
                Err(e) => diags.push(error(e, n.span)),
            }
            return false;
        }
        let drives: Vec<(String, String)> = n.props.iter().filter(|(_, v)| v.raw().starts_with("audio(")).map(|(k, v)| (k.clone(), v.raw())).collect();
        for (key, expr) in drives {
            n.props.shift_remove(&key);
            let Some(f) = f else { continue };
            let result = (|| {
                let id = own.clone().ok_or("audio-driven props need an element #id")?;
                let ch = Ch::parse(&key).filter(|c| !c.is_color()).ok_or(format!("`{key}` is not a numeric animatable channel"))?;
                let drive = Drive::parse(&expr, f.fps)?;
                let mut values = drive.values(f, start, dur);
                if ch == Ch::Opacity { for v in &mut values { *v = v.clamp(0.0, 1.0); } }
                if matches!(ch, Ch::Opacity | Ch::Brightness) && flash(&values, f.fps) {
                    diags.push(Diag::warn("W_AUDIO_FLASH", format!("audio-driven {key} on #{id} flashes more than three times per second (WCAG 2.3.1 flash risk)"),
                        "raise smooth= (e.g. 150ms), lower gain=, or drive scale instead; this is a signal check, not a full area/luminance assessment", n.span).with_id(id.clone()));
                }
                extra.push(anim(&id, &key, &values, f.fps, n.span));
                Ok::<_, String>(())
            })();
            if let Err(e) = result { diags.push(error(e, n.span)); }
        }
        let p = own.as_deref().or(parent).map(str::to_owned);
        expand_nodes(&mut n.children, p.as_deref(), f, start, dur, extra, diags);
        true
    });
}

fn prop(n: &mut Node, k: &str, v: impl ToString) { n.props.insert(k.into(), Value::Str(v.to_string())); }
fn rect(id: &str, color: &str, span: Span) -> Node {
    let mut r = Node { kind: "rect".into(), args: vec![Value::Str(format!("#{id}"))], span, ..Default::default() };
    prop(&mut r, "fill", color);
    r
}

/// `visualizer #id type=bars|radial|circle|waveform count= w= h= gap= radius= colors=[..]
/// mirror gain= smooth= thick=` -> a `layer` of rects with per-frame tracks.
fn visualizer(n: &mut Node, f: &Features, start: f64, dur: f64, extra: &mut Vec<Node>) -> Result<(), String> {
    let id = crate::lower::id_arg(n).ok_or("visualizer needs an #id")?;
    let number = |key: &str, default: f64| -> Result<f64, String> {
        let v = raw(n, key).map(|s| s.trim_end_matches("px").parse::<f64>()).transpose().map_err(|_| format!("{key} must be a number"))?.unwrap_or(default);
        if v.is_finite() { Ok(v) } else { Err(format!("{key} must be finite")) }
    };
    let mode = raw(n, "type").unwrap_or("bars".into());
    if !["bars", "circle", "radial", "waveform"].contains(&mode.as_str()) { return Err("type must be bars, radial (alias circle) or waveform".into()); }
    let round = mode != "bars";
    let (w, h) = (number("w", if round { 600.0 } else { 960.0 })?, number("h", if round { 600.0 } else { 280.0 })?);
    let count = number("count", if mode == "waveform" { 64.0 } else { 32.0 })?;
    if count.fract() != 0.0 || !(2.0..=128.0).contains(&count) { return Err("count must be an integer 2..128".into()); }
    let count = count as usize;
    let gap = number("gap", 6.0)?;
    let radius = number("radius", w.min(h) * 0.25)?;
    let thick = number("thick", 4.0)?;
    if w <= 0.0 || h <= 0.0 || gap < 0.0 || thick <= 0.0 { return Err("w, h, thick must be > 0 and gap >= 0".into()); }
    if mode == "bars" && gap >= w / count as f64 { return Err(format!("gap {gap} leaves no room for {count} bars in w={w}")); }
    if round && (radius < 0.0 || radius >= w.min(h) * 0.5) { return Err("radius must be 0..min(w,h)/2".into()); }
    let colors = match n.props.get("colors").or(n.props.get("color")) {
        Some(Value::List(v)) => v.iter().map(Value::raw).collect::<Vec<_>>(),
        Some(v) => vec![v.raw()],
        None => vec!["$accent".into()],
    };
    if colors.is_empty() { return Err("colors must not be empty".into()); }
    let mirror = matches!(raw(n, "mirror").as_deref(), Some("true")) || crate::lower::flag(n, "mirror");
    let gain = number("gain", 1.0)?;
    let smooth = raw(n, "smooth").unwrap_or("80ms".into());
    n.kind = "layer".into();
    for key in ["type", "count", "colors", "color", "gap", "radius", "mirror", "gain", "smooth", "thick"] { n.props.shift_remove(key); }
    n.args.retain(|v| v.raw() != "mirror");
    prop(n, "w", w);
    prop(n, "h", h);
    let frames = (dur * f.fps).ceil() as usize;
    if mode == "waveform" {
        // a polyline of `count - 1` rotated segments through the frame's waveform samples
        let dx = w / (count - 1) as f64;
        let amp = h * 0.45 * gain;
        let mut ys: Vec<Vec<f64>> = vec![Vec::with_capacity(frames + 1); count];
        for i in 0..=frames {
            let fr = f.frame(start + i as f64 / f.fps);
            for (j, y) in ys.iter_mut().enumerate() {
                let k = j * (WAVE - 1) / (count - 1);
                let edge = (std::f64::consts::PI * j as f64 / (count - 1) as f64).sin().sqrt(); // pinned ends
                y.push(fr.map(|v| -v.wave[k] * amp * edge).unwrap_or(0.0));
            }
        }
        for j in 0..count - 1 {
            let child = format!("{id}.s{}", j + 1);
            let mut seg = rect(&child, &colors[j % colors.len()], n.span);
            // left-center pivot sits on the polyline vertex; length via sx, slope via rot
            prop(&mut seg, "x", j as f64 * dx);
            prop(&mut seg, "y", h / 2.0 - thick / 2.0);
            prop(&mut seg, "w", dx + thick * 0.5);
            prop(&mut seg, "h", thick);
            prop(&mut seg, "radius", thick / 2.0);
            prop(&mut seg, "origin", "left");
            let (a, b) = (&ys[j], &ys[j + 1]);
            let rot: Vec<f64> = a.iter().zip(b).map(|(a, b)| (b - a).atan2(dx).to_degrees()).collect();
            let sx: Vec<f64> = a.iter().zip(b).map(|(a, b)| dx.hypot(b - a) / dx).collect();
            extra.push(anim(&child, "y", a, f.fps, n.span));
            extra.push(anim(&child, "rot", &rot, f.fps, n.span));
            extra.push(anim(&child, "sx", &sx, f.fps, n.span));
            n.children.push(seg);
        }
        return Ok(());
    }
    for j in 0..count {
        let child = format!("{id}.b{}", j + 1);
        let mut bar = rect(&child, &colors[j % colors.len()], n.span);
        // spread 8 bands over the bars (fractional bands blend neighbours); mirror puts bass in the middle
        let pos = match (mirror, mode == "bars") {
            (true, true) => {
                let half = (count - 1) as f64 / 2.0;
                (j as f64 - half).abs() / half.max(1.0)
            }
            // radial mirror: bass at 12 o'clock, highs at 6, symmetric left/right
            (true, false) => j.min(count - j) as f64 / (count / 2) as f64,
            _ => j as f64 / (count - 1) as f64,
        };
        let band = (pos * 7.0 * 1e3).round() / 1e3;
        let drive = Drive { band: Some(band), onset: false, gain, smooth: parse_dur(&smooth).map_err(|_| format!("invalid smooth `{smooth}`"))?.secs(f.fps), offset: 0.0 };
        let floor = 0.03;
        let values: Vec<f64> = drive.values(f, start, dur).into_iter().map(|v| (floor + (1.0 - floor) * v).clamp(floor, 1.0)).collect();
        if mode == "bars" {
            let bw = w / count as f64 - gap;
            prop(&mut bar, "x", j as f64 * w / count as f64 + gap / 2.0);
            prop(&mut bar, "y", 0);
            prop(&mut bar, "w", bw);
            prop(&mut bar, "h", h);
            prop(&mut bar, "radius", (bw / 2.0).min(8.0));
            prop(&mut bar, "origin", if mirror { "center" } else { "bottom" });
            extra.push(anim(&child, "sy", &values, f.fps, n.span));
        } else {
            let angle = std::f64::consts::TAU * j as f64 / count as f64 - std::f64::consts::FRAC_PI_2;
            let length = w.min(h) * 0.5 - radius;
            let bh = (std::f64::consts::TAU * radius / count as f64 - gap).max(2.0);
            let (px, py) = (w / 2.0 + radius * angle.cos(), h / 2.0 + radius * angle.sin());
            prop(&mut bar, "x", px);
            prop(&mut bar, "y", py - bh / 2.0);
            prop(&mut bar, "w", length);
            prop(&mut bar, "h", bh);
            prop(&mut bar, "radius", (bh / 2.0).min(6.0));
            prop(&mut bar, "origin", "left");
            prop(&mut bar, "rot", angle.to_degrees());
            extra.push(anim(&child, "sx", &values, f.fps, n.span));
        }
        n.children.push(bar);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clicks(bpm: f64, secs: f64, first: f64) -> Vec<f64> {
        let mut s = vec![0.0; (secs * RATE as f64) as usize];
        let mut t = first;
        let mut k = 0usize;
        while t < secs {
            let at = (t * RATE as f64) as usize;
            // a short decaying 1 kHz burst plus a low thump on every 4th beat
            for i in 0..800.min(s.len().saturating_sub(at)) {
                let x = i as f64 / RATE as f64;
                s[at + i] += (-x * 60.0).exp() * ((std::f64::consts::TAU * 1000.0 * x).sin() * 0.5 + if k % 4 == 0 { (std::f64::consts::TAU * 80.0 * x).sin() * 0.5 } else { 0.0 });
            }
            t += 60.0 / bpm;
            k += 1;
        }
        s
    }

    #[test]
    fn tempo_and_phase_from_click_track() {
        for (bpm, first) in [(128.0, 0.0), (100.0, 0.25), (90.0, 0.1), (140.0, 0.05)] {
            let f = features(&clicks(bpm, 20.0, first), 30.0, None);
            assert!((f.bpm - bpm).abs() < 0.5, "detected {} for {bpm}", f.bpm);
            assert!((f.beats[0] - first).abs() < 0.03, "first beat {} vs {first}", f.beats[0]);
            // no drift: the 30th beat still lands on a click
            let want = first + 29.0 * 60.0 / bpm;
            assert!((f.beats[29] - want).abs() < 0.03, "beat 30 at {} vs {want}", f.beats[29]);
        }
    }

    #[test]
    fn bpm_override_and_normalized_features() {
        let f = features(&clicks(120.0, 6.0, 0.5), 25.0, Some(60.0));
        assert_eq!(f.bpm, 60.0);
        // half tempo of the clicks: both phases are valid, every beat must sit on a click
        assert!(f.beats.len() >= 5 && f.beats.iter().all(|b| ((b - 0.5) / 0.5 - ((b - 0.5) / 0.5).round()).abs() * 0.5 < 0.03), "{:?}", f.beats);
        assert!((f.beats[1] - f.beats[0] - 1.0).abs() < 1e-3);
        assert_eq!(f.frames.len(), 150);
        for fr in &f.frames {
            assert!((0.0..=1.0).contains(&fr.level) && (0.0..=1.0).contains(&fr.onset));
            assert!(fr.bands.iter().all(|b| (0.0..=1.0).contains(b)) && fr.wave.len() == WAVE);
        }
        assert!(f.frames.iter().any(|fr| fr.level == 1.0));
    }

    #[test]
    fn silence_is_deterministic() {
        let a = features(&vec![0.0; RATE * 2], 30.0, None);
        let b = features(&vec![0.0; RATE * 2], 30.0, None);
        assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
        assert!(a.frames.iter().all(|f| f.level == 0.0));
    }

    #[test]
    fn drive_syntax() {
        let d = Drive::parse("audio(band=3, gain=1.5, smooth=80ms)", 30.0).unwrap();
        assert_eq!(d.band, Some(3.0));
        assert_eq!(d.gain, 1.5);
        assert!((d.smooth - 0.08).abs() < 1e-9);
        assert!(Drive::parse("audio(level)", 30.0).unwrap().band.is_none());
        assert!(Drive::parse("audio(onset)", 30.0).unwrap().onset);
        assert!(Drive::parse("audio(band=9)", 30.0).is_err());
        assert!(Drive::parse("audio(onset, band=1)", 30.0).is_err());
        assert!(Drive::parse("audio(loud)", 30.0).is_err());
        assert!(Drive::parse("audio(smooth=-1s)", 30.0).is_err());
    }

    #[test]
    fn flash_screen() {
        let fps = 30.0;
        let square: Vec<f64> = (0..60).map(|i| if (i / 3) % 2 == 0 { 0.0 } else { 1.0 }).collect(); // 5 Hz
        assert!(flash(&square, fps));
        let slow: Vec<f64> = (0..120).map(|i| if (i / 15) % 2 == 0 { 0.0 } else { 1.0 }).collect(); // 1 Hz
        assert!(!flash(&slow, fps));
        let tiny: Vec<f64> = (0..60).map(|i| if i % 2 == 0 { 0.5 } else { 0.55 }).collect();
        assert!(!flash(&tiny, fps));
    }

    #[test]
    fn decimation_keeps_shape() {
        let lin: Vec<f64> = (0..31).map(|i| i as f64 / 30.0).collect();
        let a = anim("x", "opacity", &lin, 30.0, Span::default());
        assert_eq!(a.args.len(), 2 + 2, "a straight ramp needs two keys: {:?}", a.args);
        let wave: Vec<f64> = (0..31).map(|i| (i % 2) as f64).collect();
        assert_eq!(anim("x", "opacity", &wave, 30.0, Span::default()).args.len(), 2 + 31);
    }

    #[test]
    fn needs_detection() {
        let doc = crate::parse::parse("video \"t\"\nscene a\n  say \"Der beat: vier\"\n").0;
        assert!(!needed(&doc.nodes));
        let doc = crate::parse::parse("video \"t\"\nscene a\n  at beat:4\n    pulse #x\n").0;
        assert!(needed(&doc.nodes));
        let doc = crate::parse::parse("video \"t\"\nscene a\n  rect #x opacity=audio(level)\n").0;
        assert!(needed(&doc.nodes));
    }

    #[test]
    fn expansion_lowers_to_tracks() {
        let src = "video \"t\" fps=30\nmusic src=x.wav\nscene a dur=2s\n  rect #logo w=100 h=100 opacity=audio(level, smooth=200ms)\n  pulse #logo on=beat amount=0.2\n  visualizer #viz count=8 mirror\n  visualizer #ring type=radial count=12\n  visualizer #wave type=waveform count=6\n  box #card w=10 h=10\n    pulse on=bar\n";
        let (mut doc, errs) = crate::parse::parse(src);
        assert!(errs.is_empty(), "{errs:?}");
        let tts = std::collections::HashMap::new();
        let (tl, _) = crate::timing::solve(&doc, &tts, None);
        let f = features(&clicks(120.0, 3.0, 0.0), 30.0, None);
        let mut diags = vec![];
        expand_with(&mut doc, &tl, Some(&f), &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
        let scene = &doc.nodes[2];
        let anims: Vec<_> = scene.children.iter().filter(|n| n.kind == "anim").collect();
        let target = |id: &str, ch: &str| anims.iter().any(|a| a.args[0].raw() == format!("#{id}") && a.args[1].raw() == ch);
        assert!(target("logo", "opacity") && target("logo", "scale") && target("card", "scale"));
        assert!(target("viz.b1", "sy") && target("viz.b8", "sy") && target("ring.b12", "sx"));
        assert!(target("wave.s5", "rot") && target("wave.s5", "sx") && target("wave.s1", "y"));
        assert!(!scene.children.iter().any(|n| n.kind == "pulse" || n.kind == "visualizer"));
        let logo = scene.children.iter().find(|n| n.kind == "rect").unwrap();
        assert!(!logo.props.contains_key("opacity"));
        // mirrored bars are symmetric
        let keys = |id: &str| anims.iter().find(|a| a.args[0].raw() == format!("#{id}")).unwrap().args[2..].to_vec();
        assert_eq!(keys("viz.b1"), keys("viz.b8"));
        // every key time lies inside the scene
        for a in &anims {
            for k in &a.args[2..] {
                let t: f64 = k.raw().split_once("s:").unwrap().0.parse().unwrap();
                assert!((0.0..=2.0 + 1e-9).contains(&t));
            }
        }
    }

    #[test]
    fn beat_anchors_resolve() {
        let doc = crate::parse::parse("video \"t\"\nscene a dur=4s\n  rect #x w=1 h=1\n").0;
        let (mut tl, _) = crate::timing::solve(&doc, &Default::default(), None);
        let f = Features { fps: 30.0, duration: 4.0, bpm: 120.0, beats: (0..8).map(|i| i as f64 * 0.5).collect(), frames: vec![] };
        anchors(&mut tl, &f);
        assert_eq!(tl.anchor_time(0, None, "beat:3", 0.0, Span::default()).unwrap(), 1.0);
        assert_eq!(tl.anchor_time(0, None, "bar:2+0.1s", 0.0, Span::default()).unwrap(), 2.1);
        let e = tl.anchor_time(0, None, "beat:40", 0.0, Span::default()).unwrap_err();
        assert_eq!(e.code, "E_BEAT_RANGE");
    }
}
