//! Caption pack: word-by-word social captions.
//!
//! Words come from the TTS narration (`say`) or from a transcript (`captions src=talk.srt|.vtt|.json`,
//! see `capsrc.rs`; the timeline turns each source cue into a narration beat). They are grouped
//! into phrases (timing gaps, punctuation, source cues, capacity), broken into balanced lines with
//! the real font metrics (Parley over the registered fonts), placed per format (9:16, 1:1, 16:9
//! keep their own safe areas) and animated by a named preset.
//!
//! Everything is composite-stage motion (transform and opacity of per-word layers, rounded rects,
//! drop-shadow effects): no color tracks. A highlight is a second text layer in the highlight
//! color over the base word, faded in and out (a crossfade of duplicates). Outlines are a chain of
//! four crisp drop shadows (a 3x3 dilation) plus a soft one.
use crate::ast::{Node, Span, Value};
use crate::diag::Diag;
use crate::ir::*;
use crate::lower::{flag, Ctx};
use crate::theme::hex;
use crate::values::{suggest, ColorRef, Ease, Paint, Rgba};
use std::collections::HashMap;

/// Named presets (`captions style=<name>`).
pub const PRESETS: &[&str] = &["karaoke", "pop", "pill", "hormozi", "typewriter", "rise", "outline", "safe"];

/// Props a `captions` node understands.
pub const PROPS: &[&str] = &[
    "src", "audio", "style", "offset", "pre", "post", "gain", "font", "size", "weight", "lh", "tracking", "upper", "color",
    "hi", "key", "box", "stroke", "sw", "shadow", "place", "y", "w", "maxchars", "lines", "maxwords", "gap", "lead",
    "hold", "minword", "mincue", "keywords", "autokey", "cues", "emoji",
];

/// Font aliases for `font=`.
const FONT_ALIASES: &[(&str, &str)] = &[
    ("inter", "Inter"),
    ("sans", "Inter"),
    ("mono", "JetBrains Mono"),
    ("serif", "Source Serif 4"),
    ("rounded", "Nunito"),
    ("grotesk", "Space Grotesk"),
    ("news", "Libre Franklin"),
    ("display", "Fraunces"),
];

/// Small words that are never auto keywords.
const STOP: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "your", "you", "are", "was", "were", "have", "has", "will",
    "what", "when", "then", "than", "into", "just", "they", "them", "their", "there", "about", "would", "could",
    "should", "which", "while", "der", "die", "das", "und", "mit", "eine", "einen", "einem", "einer", "nicht", "auch",
    "sich", "wird", "sind", "ist", "dass", "wenn", "aber", "oder", "noch", "dann", "denn", "weil",
];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fmt {
    Portrait,
    Square,
    Landscape,
}
impl Fmt {
    pub fn of(w: f64, h: f64) -> Fmt {
        if h > w * 1.05 {
            Fmt::Portrait
        } else if w > h * 1.05 {
            Fmt::Landscape
        } else {
            Fmt::Square
        }
    }
    fn pick<T: Copy>(self, p: T, s: T, l: T) -> T {
        match self {
            Fmt::Portrait => p,
            Fmt::Square => s,
            Fmt::Landscape => l,
        }
    }
    /// Social safe area (fractions x0, y0, x1, y1): 9:16 keeps clear of the status bar, the right
    /// action rail and the bottom caption/UI band of TikTok, Reels and Shorts.
    pub fn safe(self, th: (f64, f64)) -> [f64; 4] {
        match self {
            Fmt::Portrait => [0.06, 0.10, 0.86, 0.80],
            Fmt::Square => [0.06, 0.06, 0.94, 0.92],
            Fmt::Landscape => [th.0, th.1, 1.0 - th.0, 1.0 - th.1],
        }
    }
}

/// Resolved caption style (preset defaults + props).
#[derive(Clone, Debug)]
pub struct Style {
    pub preset: String,
    pub font: String,
    pub size: f64,
    pub weight: f64,
    pub lh: f64,
    pub tracking: f64,
    pub upper: bool,
    pub color: Rgba,
    pub hi: Rgba,
    pub key: Rgba,
    pub boxc: Rgba,
    pub stroke: Option<Rgba>,
    pub sw: f64,
    pub shadow: bool,
    /// bottom | center | top: which edge of the text block sits at `y`
    pub place: String,
    pub y: f64,
    pub w: f64,
    pub maxchars: usize,
    pub lines: usize,
    pub maxwords: usize,
    pub gap: f64,
    pub lead: f64,
    pub hold: f64,
    pub minword: f64,
    pub mincue: f64,
    pub autokey: bool,
    pub keywords: Vec<String>,
    pub cues: bool,
    pub emoji_keep: bool,
}

impl Style {
    /// Preset defaults for a format.
    pub fn preset(name: &str, fmt: Fmt, accent: Rgba) -> Style {
        let white = hex("#FFFFFF");
        let yellow = hex("#FFD60A");
        let mut s = Style {
            preset: name.to_string(),
            font: "Inter".into(),
            size: fmt.pick(64.0, 56.0, 52.0),
            weight: 750.0,
            lh: 1.18,
            tracking: -0.01,
            upper: false,
            color: white.clone(),
            hi: yellow.clone(),
            key: yellow.clone(),
            boxc: accent,
            stroke: None,
            sw: 0.0,
            shadow: true,
            place: "bottom".into(),
            y: fmt.pick(0.76, 0.88, 0.9),
            // centered: 9:16 stays clear of the right action rail (safe area ends at 86 %)
            w: fmt.pick(0.72, 0.84, 0.72),
            maxchars: fmt.pick(20, 26, 40),
            lines: 2,
            maxwords: fmt.pick(6, 8, 12),
            gap: 0.45,
            lead: 0.08,
            hold: 0.35,
            minword: 0.16,
            mincue: 0.7,
            autokey: false,
            keywords: vec![],
            cues: true,
            emoji_keep: false,
        };
        match name {
            "karaoke" => {
                s.stroke = Some(hex("#000000"));
                s.sw = 3.0;
            }
            "pop" => {
                s.size = fmt.pick(72.0, 62.0, 58.0);
                s.weight = 850.0;
                s.maxwords = fmt.pick(4, 5, 7);
                s.maxchars = fmt.pick(16, 20, 30);
                s.place = "center".into();
                s.y = fmt.pick(0.64, 0.74, 0.8);
                // words are read as they are spoken: short phrases are the style
                s.mincue = 0.5;
                s.stroke = Some(hex("#000000"));
                s.sw = 4.0;
            }
            "pill" => {
                s.weight = 800.0;
                s.maxwords = fmt.pick(5, 6, 9);
                s.hi = white.clone();
                s.stroke = None;
            }
            "hormozi" => {
                s.size = fmt.pick(88.0, 74.0, 72.0);
                s.weight = 900.0;
                s.lh = 1.08;
                s.tracking = 0.0;
                s.upper = true;
                s.maxwords = fmt.pick(4, 4, 6);
                s.maxchars = fmt.pick(14, 18, 26);
                s.place = "center".into();
                s.y = fmt.pick(0.62, 0.72, 0.78);
                s.hi = hex("#FFE14D");
                s.key = hex("#3EE07A");
                s.stroke = Some(hex("#000000"));
                s.sw = 6.0;
                s.autokey = true;
                s.mincue = 0.5;
            }
            "typewriter" => {
                s.font = "JetBrains Mono".into();
                s.size = fmt.pick(50.0, 46.0, 42.0);
                s.weight = 600.0;
                s.tracking = 0.0;
                s.maxchars = fmt.pick(18, 24, 38);
                s.boxc = hex("#0B0B0ED9");
                s.shadow = false;
            }
            "rise" => {
                s.weight = 700.0;
            }
            "outline" => {
                s.weight = 800.0;
                s.stroke = Some(hex("#000000"));
                s.sw = 4.0;
                s.maxwords = fmt.pick(7, 9, 14);
                s.maxchars = fmt.pick(22, 28, 42);
            }
            "safe" => {
                s.weight = 600.0;
                s.boxc = hex("#000000B3");
                s.shadow = false;
                s.maxwords = fmt.pick(7, 9, 14);
                s.maxchars = fmt.pick(22, 28, 42);
            }
            _ => {}
        }
        s
    }
}

/// One caption word in absolute (video) time.
#[derive(Clone, Debug)]
pub struct CWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// source cue (or narration sentence) index: phrases do not cross cues when `cues=true`
    pub cue: usize,
    pub key: bool,
}

// ---------------------------------------------------------------- measuring

/// Text widths with the registered fonts (the same files direct mode registers).
pub struct Measure {
    fcx: Option<parley::FontContext>,
    lcx: parley::LayoutContext<()>,
    cache: HashMap<(String, String, i64, i64, i64), f64>,
    files: HashMap<String, String>,
    charmaps: HashMap<String, Option<Vec<u8>>>,
    dir: std::path::PathBuf,
}

impl Default for Measure {
    fn default() -> Self {
        Self::new()
    }
}

impl Measure {
    pub fn new() -> Measure {
        let dir = crate::run::home().join("assets/fonts");
        let mut sources = HashMap::new();
        let fcx = if dir.is_dir() { Some(crate::native::font_ctx_pub(&dir, &mut sources)) } else { None };
        let mut files = HashMap::new();
        if let Some(m) = std::fs::read_to_string(dir.join("fonts.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
            for f in m["fonts"].as_array().cloned().unwrap_or_default() {
                if let (Some(fam), Some(file)) = (f["family"].as_str(), f["file"].as_str()) {
                    files.entry(fam.to_string()).or_insert_with(|| file.to_string());
                }
            }
        }
        Measure { fcx, lcx: parley::LayoutContext::new(), cache: HashMap::new(), files, charmaps: HashMap::new(), dir }
    }
    /// Registered families (fonts.json).
    pub fn families(&self) -> Vec<String> {
        let mut v: Vec<String> = self.files.keys().cloned().collect();
        v.sort();
        v
    }
    /// Advance width of `text` (one line, no wrapping) in px.
    pub fn width(&mut self, text: &str, font: &str, size: f64, weight: f64, tracking_em: f64) -> f64 {
        let key = (text.to_string(), font.to_string(), (size * 64.0) as i64, weight as i64, (tracking_em * 1e4) as i64);
        if let Some(w) = self.cache.get(&key) {
            return *w;
        }
        let w = match self.fcx.as_mut() {
            Some(fcx) => {
                use parley::{FontFamily, FontWeight, StyleProperty};
                let mut b = self.lcx.ranged_builder(fcx, text, 1.0, false);
                b.push_default(StyleProperty::FontFamily(FontFamily::Source(std::borrow::Cow::Owned(font.to_string()))));
                b.push_default(StyleProperty::FontSize(size as f32));
                b.push_default(StyleProperty::FontWeight(FontWeight::new(weight as f32)));
                b.push_default(StyleProperty::LetterSpacing((tracking_em * size) as f32));
                // Blitz lays text out with `font-optical-sizing: auto` (Inter has an opsz axis)
                let opsz = format!("\"opsz\" {size}");
                b.push_default(StyleProperty::FontVariations(parley::FontVariations::Source(std::borrow::Cow::Owned(opsz))));
                let mut l = b.build(text);
                l.break_all_lines(None);
                l.full_width() as f64
            }
            None => text.chars().count() as f64 * size * (0.56 + tracking_em),
        };
        self.cache.insert(key, w);
        w
    }
    /// Whether the family's font file maps `c` to a glyph (unknown families: assume yes).
    pub fn covers(&mut self, font: &str, c: char) -> bool {
        use skrifa::MetadataProvider;
        if c.is_whitespace() {
            return true;
        }
        let Some(file) = self.files.get(font).cloned() else { return true };
        let data = self.charmaps.entry(font.to_string()).or_insert_with(|| std::fs::read(self.dir.join(&file)).ok());
        let Some(data) = data.as_ref() else { return true };
        match skrifa::FontRef::new(data) {
            Ok(f) => f.charmap().map(c).is_some(),
            Err(_) => true,
        }
    }
}

// ---------------------------------------------------------------- grouping and line breaking

fn ends_sentence(w: &str) -> bool {
    let t = w.trim_end_matches(['"', '\'', ')', '\u{201d}', '\u{2019}']);
    t.ends_with(['.', '!', '?', '\u{2026}'])
}
fn ends_clause(w: &str) -> bool {
    w.ends_with([',', ';', ':', '\u{2013}'])
}

/// Break `n` words into at most `max_lines` lines; returns the start index of every line.
/// Fewest lines first, then the smallest widest line (balanced), then bottom-heavy (a shorter
/// first line reads better as a pyramid). `None` when no split fits (a word too wide, or more
/// lines needed).
pub fn break_lines(widths: &[f64], chars: &[usize], space: f64, max_w: f64, max_c: usize, max_lines: usize) -> Option<Vec<usize>> {
    break_lines_dangle(widths, chars, &[], space, max_w, max_c, max_lines)
}

/// `break_lines` that also avoids ending a line on a function word (`dangle[i]` = word i is
/// one): each such line costs 30 % of the box width when splits are compared.
pub fn break_lines_dangle(widths: &[f64], chars: &[usize], dangle: &[bool], space: f64, max_w: f64, max_c: usize, max_lines: usize) -> Option<Vec<usize>> {
    let n = widths.len();
    if n == 0 {
        return Some(vec![]);
    }
    let line = |a: usize, b: usize| -> (f64, usize) {
        let w: f64 = widths[a..b].iter().sum::<f64>() + space * (b - a - 1) as f64;
        let c: usize = chars[a..b].iter().sum::<usize>() + (b - a - 1);
        (w, c)
    };
    let fits = |a: usize, b: usize| {
        let (w, c) = line(a, b);
        (w <= max_w + 0.5 && c <= max_c) || b - a == 1 && w <= max_w + 0.5
    };
    for k in 1..=max_lines.max(1).min(n) {
        let mut best: Option<(f64, f64, Vec<usize>)> = None;
        // enumerate k-1 break positions in 1..n (n is small: phrases are a few words)
        let mut idx: Vec<usize> = (1..k).collect();
        loop {
            let mut starts = vec![0];
            starts.extend(idx.iter().copied());
            let mut ok = true;
            let mut widest: f64 = 0.0;
            let mut penalty = 0.0;
            let mut ws = vec![];
            for (j, &a) in starts.iter().enumerate() {
                let b = starts.get(j + 1).copied().unwrap_or(n);
                if b <= a || !fits(a, b) {
                    ok = false;
                    break;
                }
                let w = line(a, b).0;
                if b < n && dangle.get(b - 1).copied().unwrap_or(false) {
                    penalty += 0.3 * max_w;
                }
                widest = widest.max(w);
                ws.push(w);
            }
            if ok {
                // pyramid preference: penalize upper lines wider than lower ones
                let top_heavy: f64 = ws.windows(2).map(|p| (p[0] - p[1]).max(0.0)).sum();
                let cand = (widest + penalty, top_heavy, starts);
                let better = match &best {
                    None => true,
                    Some((bw, bt, _)) => cand.0 < bw - 0.5 || ((cand.0 - bw).abs() <= 0.5 && cand.1 < *bt),
                };
                if better {
                    best = Some(cand);
                }
            }
            // next combination of k-1 positions from 1..n
            if k == 1 {
                break;
            }
            let m = idx.len();
            let mut i = m;
            while i > 0 && idx[i - 1] == n - (m - (i - 1)) {
                i -= 1;
            }
            if i == 0 {
                break;
            }
            idx[i - 1] += 1;
            for j in i..m {
                idx[j] = idx[j - 1] + 1;
            }
        }
        if let Some((_, _, s)) = best {
            return Some(s);
        }
    }
    None
}

/// Function words a phrase should not end on ("on", "the", "and" ... dangle).
const DANGLE: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "of", "on", "in", "to", "at", "for", "with", "by", "from", "into", "as", "is",
    "it", "its", "my", "your", "our", "their", "this", "that", "so", "if", "der", "die", "das", "den", "dem", "ein",
    "eine", "und", "oder", "mit", "von", "zu", "im", "in", "auf", "an", "am", "fuer", "f\u{fc}r", "dass",
];

/// Group words into phrases. Hard breaks: pauses longer than `gap`, sentence ends and source
/// cue changes. Each run between hard breaks is then split by dynamic programming into the
/// fewest phrases that respect `maxwords` and `fits(words)` (the line box), preferring balanced
/// phrase lengths, breaks after clause punctuation, and never ending a phrase on a function word
/// or leaving a single word alone when that can be avoided.
pub fn group(words: &[CWord], st: &Style, mut fits: impl FnMut(&[usize]) -> bool) -> Vec<Vec<usize>> {
    let mut runs: Vec<Vec<usize>> = vec![];
    let mut cur: Vec<usize> = vec![];
    for i in 0..words.len() {
        if let Some(&p) = cur.last() {
            let (pw, w) = (&words[p], &words[i]);
            if w.start - pw.end > st.gap || ends_sentence(&pw.text) || (st.cues && w.cue != pw.cue) {
                runs.push(std::mem::take(&mut cur));
            }
        }
        cur.push(i);
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    let maxw = st.maxwords.max(1);
    let mut out = vec![];
    for run in runs {
        let n = run.len();
        let chunks_min = n.div_ceil(maxw).max(1);
        let target = n as f64 / chunks_min as f64;
        // dp[j] = (cost, previous cut) for the first j words of the run
        let mut dp: Vec<(f64, usize)> = vec![(f64::INFINITY, 0); n + 1];
        dp[0] = (0.0, 0);
        for j in 1..=n {
            for i in j.saturating_sub(maxw)..j {
                if !dp[i].0.is_finite() {
                    continue;
                }
                let len = j - i;
                if len > 1 && !fits(&run[i..j]) {
                    continue;
                }
                let last = norm(&words[run[j - 1]].text);
                let mut c = 10.0 + (len as f64 - target).powi(2) * 0.6;
                if j < n {
                    if DANGLE.contains(&last.as_str()) {
                        c += 6.0;
                    }
                    if ends_clause(&words[run[j - 1]].text) {
                        c -= 4.0;
                    }
                    // a short pause inside the run is a good place to cut
                    c -= ((words[run[j]].start - words[run[j - 1]].end) / st.gap.max(0.05)).clamp(0.0, 1.0) * 3.0;
                }
                if len == 1 && n > 1 {
                    c += 5.0;
                }
                // read time: a phrase that would flash by (the W_CAPTION_READ rule) costs extra
                // (inside a run the next phrase takes over at its first word)
                let shown = if j < n { words[run[j]].start - words[run[i]].start } else { words[run[j - 1]].end - words[run[i]].start + st.hold.min(0.2) + st.lead };
                let need = st.mincue.max(len as f64 * st.minword);
                if shown < need {
                    c += 8.0 * (need - shown) / need.max(0.1) + 4.0;
                }
                let total = dp[i].0 + c;
                if total < dp[j].0 {
                    dp[j] = (total, i);
                }
            }
        }
        let mut cuts = vec![];
        let mut j = n;
        while j > 0 {
            let i = dp[j].1;
            cuts.push((i, j));
            j = i;
        }
        cuts.reverse();
        for (i, j) in cuts {
            out.push(run[i..j].to_vec());
        }
    }
    out
}

// ---------------------------------------------------------------- text cleanup

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0xFE0F | 0x200D | 0xE0020..=0xE007F)
}

/// Strip characters the font cannot draw (emoji first among them). Returns the cleaned text and
/// the dropped characters.
pub fn clean_text(m: &mut Measure, font: &str, s: &str, keep_emoji: bool) -> (String, Vec<char>) {
    let mut out = String::new();
    let mut dropped = vec![];
    for c in s.chars() {
        let emoji = is_emoji(c);
        if (emoji && !keep_emoji) || (!emoji && !m.covers(font, c)) {
            if !matches!(c, '\u{FE0F}' | '\u{200D}') {
                dropped.push(c);
            }
            continue;
        }
        out.push(c);
    }
    (out.trim().to_string(), dropped)
}

fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

// ---------------------------------------------------------------- props

fn raw(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}

fn frac(s: &str) -> Option<f64> {
    let t = s.trim();
    if let Some(p) = t.strip_suffix('%') {
        return p.trim().parse::<f64>().ok().map(|v| v / 100.0);
    }
    t.parse::<f64>().ok().map(|v| if v > 1.0 { v / 100.0 } else { v })
}

impl<'a> Ctx<'a> {
    /// Style for a `captions` node (or the header preset when `n` is None).
    fn caption_style(&mut self, n: Option<&Node>, preset: &str) -> Style {
        let fmt = Fmt::of(self.w, self.h);
        let accent = self.th.colors.get("accent").cloned().unwrap_or(hex("#17845a"));
        let mut st = Style::preset(preset, fmt, accent);
        let Some(n) = n else { return st };
        let span = n.span;
        for k in n.props.keys() {
            if !PROPS.contains(&k.as_str()) {
                let hint = suggest(k, PROPS).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("captions props: {}", PROPS.join(", ")));
                self.warn("W_CAPTION_PROP", format!("captions: unknown prop `{k}`"), hint, span);
            }
        }
        if let Some(f) = raw(n, "font") {
            let fams = Measure::new().families();
            let alias = FONT_ALIASES.iter().find(|(a, _)| *a == f.to_lowercase()).map(|(_, b)| b.to_string());
            match alias.or_else(|| fams.iter().find(|x| x.eq_ignore_ascii_case(&f)).cloned()) {
                Some(fam) => st.font = fam,
                None => {
                    let mut names: Vec<&str> = fams.iter().map(|s| s.as_str()).collect();
                    names.extend(FONT_ALIASES.iter().map(|(a, _)| *a));
                    let hint = suggest(&f, &names).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("fonts: {}", names.join(", ")));
                    self.warn("W_CAPTION_FONT", format!("captions: font `{f}` is not registered"), hint, span);
                }
            }
        }
        for (k, dst) in [("size", 0usize), ("weight", 1), ("lh", 2), ("tracking", 3), ("sw", 4)] {
            if let Some(v) = self.num(n, k) {
                match dst {
                    0 => st.size = v,
                    1 => st.weight = v,
                    2 => st.lh = v,
                    3 => st.tracking = v,
                    _ => st.sw = v,
                }
            }
        }
        if n.props.contains_key("upper") || n.args.iter().any(|a| a.as_str() == Some("upper")) {
            st.upper = flag(n, "upper");
        }
        if let Some(c) = self.color(n, "color") {
            st.color = c;
        }
        if let Some(c) = self.color(n, "hi") {
            st.hi = c;
        }
        if let Some(c) = self.color(n, "key") {
            st.key = c;
        }
        if let Some(c) = self.color(n, "box") {
            st.boxc = c;
        }
        match raw(n, "stroke").as_deref() {
            Some("none") | Some("off") | Some("false") => st.stroke = None,
            Some(_) => {
                if let Some(c) = self.color(n, "stroke") {
                    st.stroke = Some(c);
                    if st.sw <= 0.0 {
                        st.sw = (st.size / 16.0).round().max(2.0);
                    }
                }
            }
            None => {}
        }
        if let Some(s) = raw(n, "shadow") {
            st.shadow = !matches!(s.as_str(), "false" | "off" | "none" | "0");
        }
        if let Some(p) = raw(n, "place") {
            match p.as_str() {
                "bottom" | "center" | "top" => {
                    if p != st.place && raw(n, "y").is_none() {
                        st.y = match p.as_str() {
                            "top" => fmt.pick(0.14, 0.1, 0.1),
                            "center" => 0.5,
                            _ => Style::preset("karaoke", fmt, st.boxc.clone()).y,
                        };
                    }
                    st.place = p;
                }
                other => {
                    let hint = suggest(other, &["bottom", "center", "top"]).map(|s| format!("did you mean `{s}`?")).unwrap_or("place=bottom|center|top".into());
                    self.err("E_VALUE", format!("captions: place `{other}` unknown"), hint, span);
                }
            }
        }
        for (k, is_y) in [("y", true), ("w", false)] {
            if let Some(s) = raw(n, k) {
                match frac(&s) {
                    Some(v) if (0.0..=1.0).contains(&v) => {
                        if is_y {
                            st.y = v
                        } else {
                            st.w = v
                        }
                    }
                    _ => self.err("E_VALUE", format!("captions: `{k}={s}` must be a fraction of the frame"), format!("write `{k}=70%`"), span),
                }
            }
        }
        for (k, dst) in [("maxchars", 0usize), ("lines", 1), ("maxwords", 2)] {
            if let Some(v) = self.num(n, k) {
                if v < 1.0 {
                    self.err("E_VALUE", format!("captions: `{k}` must be at least 1"), "", span);
                    continue;
                }
                let v = v.round() as usize;
                match dst {
                    0 => st.maxchars = v,
                    1 => st.lines = v.min(6),
                    _ => st.maxwords = v.min(24),
                }
            }
        }
        for (k, dst) in [("gap", 0usize), ("lead", 1), ("hold", 2), ("minword", 3), ("mincue", 4)] {
            if let Some(v) = self.dur(n, k) {
                match dst {
                    0 => st.gap = v,
                    1 => st.lead = v,
                    2 => st.hold = v,
                    3 => st.minword = v,
                    _ => st.mincue = v,
                }
            }
        }
        if let Some(v) = n.props.get("keywords") {
            st.keywords = match v {
                Value::List(l) => l.iter().map(|x| norm(&x.raw())).collect(),
                other => other.raw().split(',').map(norm).collect(),
            };
            st.keywords.retain(|k| !k.is_empty());
        }
        if n.props.contains_key("autokey") {
            st.autokey = flag(n, "autokey");
        }
        if n.props.contains_key("cues") {
            st.cues = flag(n, "cues");
        }
        if let Some(e) = raw(n, "emoji") {
            st.emoji_keep = e == "keep";
        }
        st
    }

    /// Caption pack for every scene that asks for it: scenes with `captions` nodes, or all
    /// narrated scenes when the header says `captions=<preset>` (other than plain/karaoke/off,
    /// which keep the inline legacy captions). Handled scenes are marked `nocap` so the legacy
    /// captions skip them.
    pub fn caption_pack(&mut self, scenes: &[&Node]) {
        let header = self.captions.clone();
        let header_preset = PRESETS.contains(&header.as_str()) && header != "karaoke";
        if !PRESETS.contains(&header.as_str()) && !matches!(header.as_str(), "plain" | "off") {
            let mut all: Vec<&str> = PRESETS.to_vec();
            all.extend(["plain", "off"]);
            let hint = suggest(&header, &all).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("captions=plain|off|{}", PRESETS.join("|")));
            let span = Span::default();
            self.err("E_CAPTION_STYLE", format!("video captions=`{header}` is not a caption style"), hint, span);
        }
        let mut measure = Measure::new();
        for (si, sn) in scenes.iter().enumerate() {
            let nodes: Vec<&Node> = sn.children.iter().filter(|c| c.kind == "captions").collect();
            let sid = self.scenes[si].id.clone();
            let mut tts_done = false;
            let mut src_j = 0usize;
            for n in &nodes {
                let preset = raw(n, "style").unwrap_or_else(|| if header_preset { header.clone() } else { "karaoke".into() });
                if !PRESETS.contains(&preset.as_str()) {
                    let hint = suggest(&preset, PRESETS).map(|s| format!("did you mean `{s}`?")).unwrap_or(format!("styles: {}", PRESETS.join(", ")));
                    self.err("E_CAPTION_STYLE", format!("captions: unknown style `{preset}`"), hint, n.span);
                    continue;
                }
                let st = self.caption_style(Some(n), &preset);
                let words = if n.props.contains_key("src") {
                    let prefix = format!("{sid}~cap{src_j}~");
                    src_j += 1;
                    self.caption_words(si, |key| key.starts_with(&prefix), &st)
                } else {
                    if tts_done {
                        self.warn("W_CAPTION_DUP", "a second `captions` node without src in this scene restyles the same narration twice", "keep one `captions style=...` per scene for the narration", n.span);
                    }
                    tts_done = true;
                    self.caption_words(si, |key| !key.contains("~cap"), &st)
                };
                if let Some(a) = raw(n, "audio") {
                    let src = self.asset(&a, n);
                    let gain = self.num(n, "gain").unwrap_or(1.0);
                    let at = self.tl.scenes[si].says.iter().find(|s| s.key.starts_with(&format!("{sid}~cap{}~", src_j.saturating_sub(1)))).map(|s| s.audio_at).unwrap_or(self.scenes[si].start);
                    let end = words.last().map(|w| w.end).unwrap_or(at);
                    if !n.props.contains_key("src") {
                        self.warn("W_CAPTION_AUDIO", "captions audio= without src= (narration already has its voice)", "add src=talk.srt or drop audio=", n.span);
                    } else {
                        self.audio.push(AudioClip { src, at, dur: (end - at).max(0.1), gain, duck: None, kind: "voice" });
                    }
                }
                self.caption_emit(si, &words, &st, &mut measure, n.span);
            }
            if nodes.iter().all(|n| n.props.contains_key("src")) && !tts_done && header_preset && !self.scenes[si].nocap {
                let st = self.caption_style(None, &header);
                let words = self.caption_words(si, |key| !key.contains("~cap"), &st);
                self.caption_emit(si, &words, &st, &mut measure, sn.span);
                tts_done = true;
            }
            if tts_done || !nodes.is_empty() {
                self.scenes[si].nocap = true;
            }
        }
    }

    /// Words of the scene's narration beats whose key passes `pick`; anchors and keywords marked.
    fn caption_words(&self, si: usize, pick: impl Fn(&str) -> bool, st: &Style) -> Vec<CWord> {
        let sc = &self.tl.scenes[si];
        let mut out = vec![];
        for (k, say) in sc.says.iter().enumerate() {
            if !pick(&say.key) {
                continue;
            }
            for w in &say.words {
                let anchored = sc.anchors.iter().any(|a| a.say == k && w.start >= a.start - 1e-6 && w.end <= a.end + 1e-6 && a.end - a.start < say.end - say.start - 1e-6);
                let explicit = st.keywords.contains(&norm(&w.text));
                out.push(CWord { text: w.text.clone(), start: w.start, end: w.end.max(w.start), cue: k, key: anchored || explicit });
            }
        }
        out
    }

    /// Lay out and animate one caption track.
    fn caption_emit(&mut self, si: usize, words0: &[CWord], st: &Style, m: &mut Measure, span: Span) {
        if words0.is_empty() {
            return;
        }
        let (fw, fh) = (self.w, self.h);
        let fmt = Fmt::of(fw, fh);
        let fmt_name = self.fmt.clone();
        // clean (glyph coverage, emoji) and case
        let mut dropped: Vec<char> = vec![];
        let mut words: Vec<CWord> = vec![];
        for w in words0 {
            let (t, d) = clean_text(m, &st.font, &w.text, st.emoji_keep);
            dropped.extend(d);
            if t.is_empty() {
                continue;
            }
            let t = if st.upper { t.to_uppercase() } else { t };
            words.push(CWord { text: t, ..w.clone() });
        }
        if !dropped.is_empty() {
            dropped.dedup();
            let list: String = dropped.iter().take(8).collect();
            self.warn("W_CAPTION_GLYPH", format!("captions: {} character(s) the font `{}` cannot draw were dropped ({list})", dropped.len(), st.font), "emoji have no color font in direct mode yet; write the word instead, or `emoji=keep` to keep them (they render as missing glyphs)", span);
        }
        if words.is_empty() {
            return;
        }
        // auto keywords: the most salient word per sentence (numbers, else the longest
        // non-stopword of 5+ letters)
        if st.autokey && !words.iter().any(|w| w.key) {
            let mut i = 0;
            while i < words.len() {
                let mut j = i;
                while j + 1 < words.len() && !ends_sentence(&words[j].text) && words[j + 1].cue == words[i].cue {
                    j += 1;
                }
                let best = (i..=j)
                    .filter(|&k| {
                        let t = norm(&words[k].text);
                        t.chars().count() >= 5 && !STOP.contains(&t.as_str()) || t.chars().any(|c| c.is_ascii_digit())
                    })
                    .max_by_key(|&k| {
                        let t = norm(&words[k].text);
                        (t.chars().any(|c| c.is_ascii_digit()) as usize * 100) + t.chars().count()
                    });
                if let Some(b) = best {
                    words[b].key = true;
                }
                i = j + 1;
            }
        }
        let max_w = st.w * fw;
        let mut space = m.width("x x", &st.font, st.size, st.weight, st.tracking) - m.width("xx", &st.font, st.size, st.weight, st.tracking);
        // Inter's display optical size has a narrow space (about 0.2 em); word-level captions read
        // better with a little more air
        space = space.max(st.size * 0.25);
        // presets that scale the active word up get room for it (and the outline) between words
        match st.preset.as_str() {
            "hormozi" => space += st.size * 0.14,
            // the pill's side padding must not touch the neighbours
            "pill" => space += st.size * 0.16,
            _ => {}
        }
        let widths: Vec<f64> = words.iter().map(|w| m.width(&w.text, &st.font, st.size, st.weight, st.tracking)).collect();
        let chars: Vec<usize> = words.iter().map(|w| w.text.chars().count()).collect();
        // overflow: a single word wider than the line box
        for (i, w) in words.iter().enumerate() {
            if widths[i] > max_w + 0.5 {
                let mut d = Diag::error("E_CAPTION_OVERFLOW", format!("caption word `{}` is {:.0}px wide, the line box is {:.0}px ({fmt_name})", w.text, widths[i], max_w), format!("lower size= (now {:.0}) or widen w= (now {:.0}%)", st.size, st.w * 100.0), span);
                d.format = Some(fmt_name.clone());
                d.at = Some(w.start);
                self.diags.push(d);
            }
        }
        let phrases = group(&words, st, |ix| {
            let ws: Vec<f64> = ix.iter().map(|&i| widths[i]).collect();
            let cs: Vec<usize> = ix.iter().map(|&i| chars[i]).collect();
            ix.len() == 1 || break_lines(&ws, &cs, space, max_w, st.maxchars, st.lines).is_some()
        });
        let lh_px = st.size * st.lh;
        let safe = fmt.safe(self.th.safe);
        let ov = self.scenes[si].overlay;
        let fps = self.fps;
        let scene_end = self.scenes[si].start + self.scenes[si].dur;
        let base_id = format!("cpk-{}-{}", self.scenes[si].id, self.layers.len());
        // phrase times
        let mut times: Vec<(f64, f64)> = vec![];
        for (pi, ph) in phrases.iter().enumerate() {
            let first = &words[ph[0]];
            let last = &words[*ph.last().unwrap()];
            let prev_end = times.last().map(|t| t.1).unwrap_or(self.scenes[si].start);
            let t0 = (first.start - st.lead).max(prev_end).max(self.scenes[si].start);
            let next0 = phrases.get(pi + 1).map(|p| words[p[0]].start - st.lead).unwrap_or(scene_end);
            let mut t1 = (last.end + st.hold).min(next0).min(scene_end);
            // stretch toward the minimum display time when there is room
            if t1 - t0 < st.mincue {
                t1 = (t0 + st.mincue).min(next0.max(t1)).min(scene_end);
            }
            times.push((t0, t1.max(t0 + 1.0 / fps)));
        }
        for (pi, ph) in phrases.iter().enumerate() {
            let (t0, t1) = times[pi];
            // read time lint
            let need = st.mincue.max(ph.len() as f64 * st.minword);
            if t1 - t0 + 1e-6 < need {
                let txt: Vec<&str> = ph.iter().map(|&i| words[i].text.as_str()).collect();
                let mut d = Diag::warn("W_CAPTION_READ", format!("caption \"{}\" shows {:.0} ms, needs {:.0} ms to read ({} words)", txt.join(" "), (t1 - t0) * 1000.0, need * 1000.0, ph.len()), "slow the line down, merge it with a neighbour (fewer cue breaks), or lower minword=/mincue=", span);
                d.at = Some(t0);
                self.diags.push(d);
            }
            // lines
            let ws: Vec<f64> = ph.iter().map(|&i| widths[i]).collect();
            let cs: Vec<usize> = ph.iter().map(|&i| chars[i]).collect();
            let dg: Vec<bool> = ph.iter().map(|&i| DANGLE.contains(&norm(&words[i].text).as_str())).collect();
            let starts = break_lines_dangle(&ws, &cs, &dg, space, max_w, st.maxchars, st.lines).unwrap_or_else(|| {
                // does not fit (single overlong word, or too many lines): one word per line
                (0..ph.len()).collect()
            });
            let nl = starts.len();
            let line_w: Vec<f64> = (0..nl).map(|j| {
                let (a, b) = (starts[j], starts.get(j + 1).copied().unwrap_or(ph.len()));
                ws[a..b].iter().sum::<f64>() + space * (b - a - 1) as f64
            }).collect();
            let bw = line_w.iter().cloned().fold(0.0, f64::max);
            let bh = nl as f64 * lh_px;
            let by = match st.place.as_str() {
                "top" => st.y * fh,
                "center" => st.y * fh - bh / 2.0,
                _ => st.y * fh - bh,
            };
            let bx = (fw - bw) / 2.0;
            // frame and safe-area lint (once per track is enough for overflow of the area)
            let (x0, y0, x1, y1) = (bx, by, bx + bw, by + bh);
            if x0 < -0.5 || y0 < -0.5 || x1 > fw + 0.5 || y1 > fh + 0.5 {
                let mut d = Diag::error("E_CAPTION_OFF_FRAME", format!("caption block {:.0}x{:.0} at y={:.0} leaves the {fmt_name} frame", bw, bh, by), "move it with y= / place=, or fewer lines=", span);
                d.format = Some(fmt_name.clone());
                d.at = Some(t0);
                self.diags.push(d);
            } else if x0 < safe[0] * fw - 0.5 || y0 < safe[1] * fh - 0.5 || x1 > safe[2] * fw + 0.5 || y1 > safe[3] * fh + 0.5 {
                let mut d = Diag::warn("W_CAPTION_SAFE", format!("caption block ({:.0},{:.0})-({:.0},{:.0}) leaves the {fmt_name} social safe area ({:.0},{:.0})-({:.0},{:.0})", x0, y0, x1, y1, safe[0] * fw, safe[1] * fh, safe[2] * fw, safe[3] * fh), "platform UI covers that band; use the default y= for this format or lower size=/w=", span);
                d.format = Some(fmt_name.clone());
                d.at = Some(t0);
                self.diags.push(d);
            }
            // phrase layer
            let mut g = Layer::new(format!("{base_id}-p{pi}"), Kind::Group);
            g.layout.pos = Pos::Abs;
            g.layout.x = Some(Len::px(bx));
            g.layout.y = Some(Len::px(by));
            g.layout.anchor = (0.0, 0.0);
            g.layout.w = Some(Len::px(bw));
            g.layout.h = Some(Len::px(bh));
            g.style.font = Some(st.font.clone());
            g.style.origin = (0.5, 0.5);
            g.tags.push("caption".into());
            let gi = self.add(Some(ov), g);
            // line boxes (safe, typewriter)
            if matches!(st.preset.as_str(), "safe" | "typewriter") {
                let (px, py) = (st.size * 0.32, st.size * 0.1);
                for j in 0..nl {
                    let mut r = Layer::new(format!("{base_id}-p{pi}-box{j}"), Kind::Rect);
                    r.layout.pos = Pos::Abs;
                    r.layout.x = Some(Len::px((bw - line_w[j]) / 2.0 - px));
                    r.layout.y = Some(Len::px(j as f64 * lh_px - py));
                    r.layout.anchor = (0.0, 0.0);
                    r.layout.w = Some(Len::px(line_w[j] + 2.0 * px));
                    r.layout.h = Some(Len::px(lh_px + 2.0 * py));
                    r.style.radius = st.size * 0.18;
                    r.style.fill = Some(Paint::Solid(ColorRef::Lit(st.boxc.clone())));
                    r.tags.push("cap-local".into());
                    self.add(Some(gi), r);
                }
            }
            // phrase in/out
            let fade_in = match st.preset.as_str() {
                "outline" | "safe" | "typewriter" => 3.0 / fps,
                _ => 4.0 / fps,
            };
            self.set_base(gi, Ch::Opacity, TV::N(0.0));
            self.track(gi, Ch::Opacity).to(t0, t0 + fade_in, TV::N(1.0), Ease::Linear);
            self.track(gi, Ch::Opacity).to((t1 - 3.0 / fps).max(t0 + fade_in), t1, TV::N(0.0), Ease::Linear);
            if st.preset == "hormozi" {
                self.track(gi, Ch::Scale).from_to(t0, t0 + 0.32, TV::N(0.86), TV::N(1.0), Ease::Spring { stiffness: 420.0, damping: 22.0, mass: 1.0, velocity: 0.0 });
            }
            if st.preset == "karaoke" || st.preset == "pill" || st.preset == "outline" || st.preset == "safe" {
                self.track(gi, Ch::Y).from_to(t0, t0 + 0.2, TV::N(8.0), TV::N(0.0), Ease::Cubic(0.33, 1.0, 0.68, 1.0));
            }
            // words
            for (li, &a) in starts.iter().enumerate() {
                let b = starts.get(li + 1).copied().unwrap_or(ph.len());
                let mut x = (bw - line_w[li]) / 2.0;
                let y = li as f64 * lh_px;
                for k in a..b {
                    let wi = ph[k];
                    let w = &words[wi];
                    let next_start = if k + 1 < ph.len() { words[ph[k + 1]].start } else { (w.end + st.hold * 0.5).min(t1) };
                    let active_end = next_start.max(w.end).min(t1);
                    self.caption_word(gi, &format!("{base_id}-p{pi}-w{k}"), w, x, y, ws[k], lh_px, st, (t0, t1), active_end);
                    x += ws[k] + space;
                }
            }
        }
    }

    fn caption_text(&mut self, parent: usize, id: String, text: &str, color: &Rgba, st: &Style, h: f64) -> usize {
        let mut l = Layer::new(id, Kind::Text { runs: vec![Run { text: text.to_string(), ..Default::default() }] });
        l.layout.pos = Pos::Abs;
        l.layout.x = Some(Len::px(0.0));
        l.layout.y = Some(Len::px(0.0));
        l.layout.anchor = (0.0, 0.0);
        let _ = h;
        l.style.nowrap = true;
        l.style.size = Some(st.size);
        l.style.weight = Some(st.weight);
        l.style.line_height = Some(st.lh);
        l.style.tracking = Some(st.tracking);
        l.style.color = Some(color.clone());
        l.style.font = Some(st.font.clone());
        if let Some(sc) = &st.stroke {
            let s = st.sw.max(1.0);
            for (dx, dy) in [(s, 0.0), (-s, 0.0), (0.0, s), (0.0, -s)] {
                l.style.shadows.push(Shadow { x: dx, y: dy, blur: 0.0, color: Some(sc.clone()), spread: 0.0, inner: false });
            }
        }
        if st.shadow {
            l.style.shadows.push(Shadow { x: 0.0, y: st.size * 0.06, blur: st.size * 0.22, color: Some(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.55 }), spread: 0.0, inner: false });
        }
        // `piece`: the generic measured text checks skip it (the caption lint owns its layout)
        l.tags.extend(["caption".into(), "text".into(), "piece".into(), "cap-local".into()]);
        self.add(Some(parent), l)
    }

    /// One word: a group (transform) with the base text, an optional highlight duplicate and an
    /// optional pill behind it. `phrase` = (in, out) times; `active_end` = when the next word starts.
    #[allow(clippy::too_many_arguments)]
    fn caption_word(&mut self, phrase_layer: usize, id: &str, w: &CWord, x: f64, y: f64, wpx: f64, hpx: f64, st: &Style, phrase: (f64, f64), active_end: f64) {
        let fps = self.fps;
        let (t0, t1) = phrase;
        let ws = w.start.max(t0);
        let mut g = Layer::new(id.to_string(), Kind::Group);
        g.layout.pos = Pos::Abs;
        g.layout.x = Some(Len::px(x));
        g.layout.y = Some(Len::px(y));
        g.layout.anchor = (0.0, 0.0);
        g.layout.w = Some(Len::px(wpx));
        g.layout.h = Some(Len::px(hpx));
        g.style.origin = (0.5, 0.55);
        g.tags.push("cap-local".into());
        let gi = self.add(Some(phrase_layer), g);
        let p = st.preset.as_str();
        // pill behind the active word
        if p == "pill" {
            let (px, py) = (st.size * 0.2, st.size * 0.02);
            let mut r = Layer::new(format!("{id}-pill"), Kind::Rect);
            r.layout.pos = Pos::Abs;
            r.layout.x = Some(Len::px(-px));
            r.layout.y = Some(Len::px(-py));
            r.layout.anchor = (0.0, 0.0);
            r.layout.w = Some(Len::px(wpx + 2.0 * px));
            r.layout.h = Some(Len::px(hpx + 2.0 * py));
            r.style.radius = st.size * 0.24;
            r.style.fill = Some(Paint::Solid(ColorRef::Lit(st.boxc.clone())));
            r.style.shadows.push(Shadow { x: 0.0, y: st.size * 0.05, blur: st.size * 0.25, color: Some(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.35 }), spread: 0.0, inner: false });
            r.tags.push("cap-local".into());
            let ri = self.add(Some(gi), r);
            self.set_base(ri, Ch::Opacity, TV::N(0.0));
            self.track(ri, Ch::Opacity).to(ws, ws + 2.0 / fps, TV::N(1.0), Ease::Linear);
            self.track(ri, Ch::Opacity).to((active_end - 2.0 / fps).max(ws + 2.0 / fps), active_end, TV::N(0.0), Ease::Linear);
            self.track(ri, Ch::Scale).from_to(ws, ws + 0.25, TV::N(0.82), TV::N(1.0), Ease::Spring { stiffness: 500.0, damping: 24.0, mass: 1.0, velocity: 0.0 });
        }
        let base_color = if w.key && matches!(p, "hormozi" | "pop" | "karaoke" | "rise" | "outline") { st.key.clone() } else { st.color.clone() };
        let bi = self.caption_text(gi, format!("{id}-t"), &w.text, &base_color, st, hpx);
        // highlight duplicate (crossfaded): karaoke keeps it on, pop/hormozi only while active
        let hi_mode = match p {
            "karaoke" => Some(true),
            "pop" | "hormozi" => Some(false),
            _ => None,
        };
        if let Some(stay) = hi_mode {
            let hi = st.hi.clone();
            let hi_i = self.caption_text(gi, format!("{id}-hi"), &w.text, &hi, st, hpx);
            self.set_base(hi_i, Ch::Opacity, TV::N(0.0));
            self.track(hi_i, Ch::Opacity).to(ws, ws + 2.0 / fps, TV::N(1.0), Ease::Linear);
            if !stay {
                self.track(hi_i, Ch::Opacity).to((active_end - 2.0 / fps).max(ws + 2.0 / fps), active_end, TV::N(0.0), Ease::Linear);
            }
        }
        match p {
            "pop" => {
                self.set_base(gi, Ch::Opacity, TV::N(0.0));
                self.track(gi, Ch::Opacity).to(ws, ws + 2.0 / fps, TV::N(1.0), Ease::Linear);
                self.track(gi, Ch::Scale).from_to(ws, ws + 0.4, TV::N(0.5), TV::N(1.0), Ease::Spring { stiffness: 420.0, damping: 16.0, mass: 1.0, velocity: 0.0 });
            }
            "hormozi" => {
                self.track(gi, Ch::Scale).from_to(ws, ws + 0.12, TV::N(1.0), TV::N(1.07), Ease::Cubic(0.33, 1.0, 0.68, 1.0));
                self.track(gi, Ch::Scale).to(active_end.max(ws + 0.13), (active_end + 0.12).max(ws + 0.25).min(t1.max(ws + 0.25)), TV::N(1.0), Ease::Cubic(0.33, 1.0, 0.68, 1.0));
            }
            "rise" => {
                self.set_base(gi, Ch::Opacity, TV::N(0.0));
                self.track(gi, Ch::Opacity).to(ws, ws + 0.22, TV::N(1.0), Ease::Cubic(0.33, 1.0, 0.68, 1.0));
                self.track(gi, Ch::Y).from_to(ws, ws + 0.32, TV::N(st.size * 0.35), TV::N(0.0), Ease::Cubic(0.33, 1.0, 0.68, 1.0));
            }
            "typewriter" => {
                let n = w.text.chars().count().max(1) as u32;
                let d = (w.end - w.start).clamp(n as f64 * 0.025, n as f64 * 0.07).max(1.0 / fps);
                self.layers[bi].style.clip = Some(ClipShape::Wipe("left".into()));
                self.set_base(bi, Ch::Reveal, TV::N(0.0));
                self.track(bi, Ch::Reveal).from_to(ws, ws + d, TV::N(0.0), TV::N(1.0), Ease::Steps(n));
            }
            _ => {}
        }
    }
}

/// Does the scene reserve a bottom caption band in its content layout? (`captions` nodes placed
/// at the center or top float over the content instead.)
pub fn reserve(n: &Node, w: f64, h: f64, accent: Rgba) -> Option<f64> {
    let fmt = Fmt::of(w, h);
    let preset = n.props.get("style").map(|v| v.raw()).unwrap_or("karaoke".into());
    let st = Style::preset(if PRESETS.contains(&preset.as_str()) { &preset } else { "karaoke" }, fmt, accent);
    let place = n.props.get("place").map(|v| v.raw()).unwrap_or(st.place.clone());
    if place != "bottom" {
        return Some(0.0);
    }
    let size = n.props.get("size").and_then(|v| v.raw().trim_end_matches("px").parse::<f64>().ok()).unwrap_or(st.size);
    let lines = n.props.get("lines").and_then(|v| v.raw().parse::<f64>().ok()).unwrap_or(st.lines as f64);
    Some((size * st.lh * lines + 24.0 + (1.0 - st.y) * h - h * 0.06).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_balanced_and_bottom_heavy() {
        // 4 words of 100 px, space 10, box 250: needs 2 lines, 2+2
        let s = break_lines(&[100.0; 4], &[5; 4], 10.0, 250.0, 99, 2).unwrap();
        assert_eq!(s, vec![0, 2]);
        // 3 words: 1+2 (pyramid) rather than 2+1
        let s = break_lines(&[100.0; 3], &[5; 3], 10.0, 250.0, 99, 2).unwrap();
        assert_eq!(s, vec![0, 1]);
        // one line when it fits
        assert_eq!(break_lines(&[50.0; 3], &[3; 3], 10.0, 400.0, 99, 2).unwrap(), vec![0]);
        // too many lines needed
        assert!(break_lines(&[100.0; 5], &[5; 5], 10.0, 150.0, 99, 2).is_none());
        // max chars binds before pixels
        assert_eq!(break_lines(&[10.0; 4], &[6; 4], 1.0, 1000.0, 13, 2).unwrap(), vec![0, 2]);
        // a line should not end on "the": 1+3 beats a balanced 2+2 that dangles
        let d = [false, true, false, false];
        assert_eq!(break_lines_dangle(&[100.0; 4], &[5; 4], &d, 10.0, 400.0, 99, 2).unwrap(), vec![0, 1]);
    }

    fn w(t: &str, s: f64, e: f64, cue: usize) -> CWord {
        CWord { text: t.into(), start: s, end: e, cue, key: false }
    }

    #[test]
    fn groups_on_gaps_punctuation_and_cues() {
        let st = Style::preset("karaoke", Fmt::Landscape, hex("#000"));
        let ws = vec![w("Hello", 0.0, 0.3, 0), w("there.", 0.3, 0.6, 0), w("This", 0.7, 0.9, 0), w("is", 0.9, 1.0, 0), w("after", 2.0, 2.3, 0), w("a", 2.3, 2.4, 1), w("cue", 2.4, 2.6, 1)];
        let g = group(&ws, &st, |_| true);
        assert_eq!(g.len(), 4, "{g:?}");
        assert_eq!(g[0], vec![0, 1]);
        assert_eq!(g[1], vec![2, 3]);
        // the gap before "after" splits; the cue change splits "after" from "a cue", and the
        // orphan "after" may not join across the pause
        assert_eq!(g[2], vec![4]);
        assert_eq!(g[3], vec![5, 6]);
    }

    #[test]
    fn groups_respect_capacity_and_orphans() {
        let mut st = Style::preset("hormozi", Fmt::Portrait, hex("#000"));
        st.maxwords = 3;
        let ws: Vec<CWord> = (0..7).map(|i| w("word", i as f64 * 0.3, i as f64 * 0.3 + 0.25, 0)).collect();
        let g = group(&ws, &st, |_| true);
        // 7 words, at most 3 per phrase: three balanced phrases, no single word left alone
        assert_eq!(g.len(), 3, "{g:?}");
        assert!(g.iter().all(|p| (2..=3).contains(&p.len())), "{g:?}");
        assert_eq!(g.iter().map(|p| p.len()).sum::<usize>(), 7);
        // never end a phrase on a function word when another cut exists
        let ws: Vec<CWord> = ["timed", "right", "here", "on", "one", "small", "machine."].iter().enumerate().map(|(i, t)| w(t, i as f64 * 0.3, i as f64 * 0.3 + 0.25, 0)).collect();
        let mut st5 = st.clone();
        st5.maxwords = 5;
        let g = group(&ws, &st5, |_| true);
        assert!(g.iter().all(|p| !["on", "the"].contains(&ws[*p.last().unwrap()].text.as_str())), "{g:?}");
        assert_eq!(g.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![3, 4], "{g:?}");
    }

    #[test]
    fn clause_break_moves_tail() {
        let st = Style::preset("karaoke", Fmt::Landscape, hex("#000"));
        let ws = vec![w("one", 0.0, 0.2, 0), w("two,", 0.2, 0.4, 0), w("three", 0.4, 0.6, 0), w("four", 0.6, 0.8, 0)];
        // capacity: at most 3 words fit
        let g = group(&ws, &st, |ix| ix.len() <= 3);
        assert_eq!(g, vec![vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn emoji_are_dropped() {
        let mut m = Measure::new();
        let (t, d) = clean_text(&mut m, "Inter", "fire\u{1F525}", false);
        assert_eq!(t, "fire");
        assert_eq!(d, vec!['\u{1F525}']);
        let (t, _) = clean_text(&mut m, "Inter", "caf\u{e9}", false);
        assert_eq!(t, "caf\u{e9}");
    }

    #[test]
    fn measures_wider_for_heavier_and_longer() {
        let mut m = Measure::new();
        let a = m.width("Caption", "Inter", 60.0, 400.0, 0.0);
        let b = m.width("Caption", "Inter", 60.0, 900.0, 0.0);
        let c = m.width("Captions", "Inter", 60.0, 400.0, 0.0);
        assert!(a > 100.0 && b > a && c > a, "{a} {b} {c}");
    }
}
