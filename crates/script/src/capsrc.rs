//! Caption sources: SRT, WebVTT and Whisper JSON (openai-whisper, faster-whisper, WhisperX,
//! whisper.cpp) into timed words.
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct CapWord {
    /// the word as written, punctuation attached
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub words: Vec<CapWord>,
    /// word times came from the source (false: spread evenly over the cue)
    pub timed: bool,
}

/// Dispatch by file extension: `srt`, `vtt`, `json`.
pub fn parse_by_ext(ext: &str, src: &str) -> Result<Vec<Cue>, String> {
    match ext.to_ascii_lowercase().as_str() {
        "srt" => parse_srt(src),
        "vtt" => parse_vtt(src),
        "json" => parse_whisper(src),
        other => Err(format!("unknown caption format `.{other}` (use .srt, .vtt or whisper .json)")),
    }
}

/// Spread `[start, end]` over the words of `text` by character count; words touch.
pub fn distribute(text: &str, start: f64, end: f64) -> Vec<CapWord> {
    let toks: Vec<&str> = text.split_whitespace().collect();
    if toks.is_empty() {
        return vec![];
    }
    let end = end.max(start);
    let total: usize = toks.iter().map(|t| t.chars().count().max(1)).sum();
    let mut at = start;
    let mut acc = 0usize;
    let n = toks.len();
    toks.iter().enumerate().map(|(i, t)| {
        acc += t.chars().count().max(1);
        let e = if i + 1 == n { end } else { start + (end - start) * acc as f64 / total as f64 };
        let w = CapWord { text: t.to_string(), start: at, end: e };
        at = e;
        w
    }).collect()
}

/// `HH:MM:SS,mmm`, `HH:MM:SS.mmm`, `MM:SS.mmm` (1 to 3 fraction digits, or none) to seconds.
pub fn parse_ts(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let mut secs = 0.0;
    for (i, p) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit() || (last && c == '.')) {
            return None;
        }
        let v: f64 = p.parse().ok()?;
        secs = secs * 60.0 + v;
    }
    Some(secs)
}

fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// Blocks separated by blank lines (CRLF tolerant).
fn blocks(src: &str) -> Vec<Vec<String>> {
    let mut out = vec![];
    let mut cur: Vec<String> = vec![];
    for line in strip_bom(src).lines() {
        let l = line.trim_end_matches('\r');
        if l.trim().is_empty() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(l.to_string());
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `a --> b [settings]` to (a, b).
fn timing_line(l: &str) -> Option<(f64, f64)> {
    let (a, rest) = l.split_once("-->")?;
    let b = rest.split_whitespace().next()?;
    Some((parse_ts(a)?, parse_ts(b)?))
}

/// Remove `<...>` tags and `{...}` override blocks.
fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut depth_angle = false;
    let mut depth_brace = false;
    for c in s.chars() {
        match c {
            '<' if !depth_brace => depth_angle = true,
            '>' if depth_angle => depth_angle = false,
            '{' if !depth_angle => depth_brace = true,
            '}' if depth_brace => depth_brace = false,
            _ if depth_angle || depth_brace => {}
            _ => out.push(c),
        }
    }
    out
}

fn norm_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_srt(src: &str) -> Result<Vec<Cue>, String> {
    let mut cues = vec![];
    for b in blocks(src) {
        let Some(ti) = b.iter().position(|l| l.contains("-->")) else { continue };
        let Some((start, end)) = timing_line(&b[ti]) else { continue };
        let text = norm_space(&strip_tags(&b[ti + 1..].join(" ")));
        if text.is_empty() {
            continue;
        }
        cues.push(Cue { start, end: end.max(start), words: distribute(&text, start, end), timed: false });
    }
    if cues.is_empty() {
        return Err("no cues found in SRT".into());
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(cues)
}

pub fn parse_vtt(src: &str) -> Result<Vec<Cue>, String> {
    let body = strip_bom(src);
    if !body.trim_start().starts_with("WEBVTT") {
        return Err("not a WebVTT file (missing WEBVTT header)".into());
    }
    let mut cues: Vec<Cue> = vec![];
    let mut prev_last_line: Option<String> = None;
    for b in blocks(body) {
        let head = b[0].trim_start();
        if head.starts_with("WEBVTT") || head.starts_with("NOTE") || head.starts_with("STYLE") || head.starts_with("REGION") {
            continue;
        }
        let Some(ti) = b.iter().position(|l| l.contains("-->")) else { continue };
        let Some((start, end)) = timing_line(&b[ti]) else { continue };
        let mut lines: Vec<String> = b[ti + 1..].to_vec();
        // YouTube auto captions repeat the previous cue's last line first
        if let (Some(p), Some(first)) = (&prev_last_line, lines.first()) {
            if norm_space(&strip_tags(first)) == *p && lines.len() > 1 {
                lines.remove(0);
            }
        }
        if let Some(l) = b.last() {
            if b.len() > ti + 1 {
                prev_last_line = Some(norm_space(&strip_tags(l)));
            }
        }
        let raw = lines.join(" ");
        let end = end.max(start);
        // inline word timestamps <00:00:01.200>
        let mut segs: Vec<(Option<f64>, String)> = vec![(None, String::new())];
        let mut rest = raw.as_str();
        let mut any_ts = false;
        while let Some(i) = rest.find('<') {
            segs.last_mut().unwrap().1.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            let Some(j) = after.find('>') else {
                rest = "";
                break;
            };
            let tag = &after[..j];
            if let Some(t) = parse_ts(tag) {
                any_ts = true;
                segs.push((Some(t), String::new()));
            }
            rest = &after[j + 1..];
        }
        segs.last_mut().unwrap().1.push_str(rest);
        let text = norm_space(&strip_tags(&raw));
        if text.is_empty() {
            continue;
        }
        if !any_ts {
            cues.push(Cue { start, end, words: distribute(&text, start, end), timed: false });
            continue;
        }
        let mut words: Vec<CapWord> = vec![];
        for (t, s) in segs {
            let t0 = t.unwrap_or(start).clamp(start, end);
            for (k, tok) in strip_tags(&s).split_whitespace().enumerate() {
                // a word is timed by the most recent timestamp before it
                let _ = k;
                words.push(CapWord { text: tok.to_string(), start: t0, end: t0 });
            }
        }
        let n = words.len();
        for i in 0..n {
            let e = if i + 1 < n { words[i + 1].start } else { end };
            words[i].end = e.max(words[i].start);
        }
        cues.push(Cue { start, end, words, timed: true });
    }
    if cues.is_empty() {
        return Err("no cues found in WebVTT".into());
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(cues)
}

fn f(v: &Value) -> Option<f64> {
    v.as_f64()
}

pub fn parse_whisper(src: &str) -> Result<Vec<Cue>, String> {
    let v: Value = serde_json::from_str(src).map_err(|e| format!("invalid JSON: {e}"))?;
    let mut cues = if let Some(segs) = v.get("segments").and_then(|s| s.as_array()) {
        openai(segs)
    } else if let Some(tr) = v.get("transcription").and_then(|s| s.as_array()) {
        whisper_cpp(tr)
    } else if let Some(arr) = v.as_array() {
        word_array(arr)
    } else {
        return Err("unrecognized JSON: expected `segments`, `transcription` or a word array".into());
    };
    cues.retain(|c| !c.words.is_empty());
    if cues.is_empty() {
        return Err("no words found in JSON".into());
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(cues)
}

fn word_text(w: &Value) -> String {
    w.get("word").or_else(|| w.get("text")).and_then(|t| t.as_str()).unwrap_or("").trim().to_string()
}

fn openai(segs: &[Value]) -> Vec<Cue> {
    let mut out = vec![];
    for s in segs {
        let (start, end) = (s.get("start").and_then(f).unwrap_or(0.0), s.get("end").and_then(f).unwrap_or(0.0));
        let end = end.max(start);
        let ws = s.get("words").and_then(|w| w.as_array()).cloned().unwrap_or_default();
        let raw: Vec<(String, Option<f64>, Option<f64>)> = ws.iter().map(|w| (word_text(w), w.get("start").and_then(f), w.get("end").and_then(f))).filter(|w| !w.0.is_empty()).collect();
        if raw.is_empty() {
            let text = s.get("text").and_then(|t| t.as_str()).unwrap_or("");
            out.push(Cue { start, end, words: distribute(text, start, end), timed: false });
            continue;
        }
        let mut words: Vec<CapWord> = vec![];
        for (i, (t, a, b)) in raw.iter().enumerate() {
            let prev_end = words.last().map(|w: &CapWord| w.end).unwrap_or(start);
            let a = a.unwrap_or(prev_end);
            let next_start = raw.get(i + 1).and_then(|n| n.1).unwrap_or(end);
            let b = b.unwrap_or(next_start).max(a);
            words.push(CapWord { text: t.clone(), start: a, end: b });
        }
        out.push(Cue { start, end, words, timed: true });
    }
    out
}

fn whisper_cpp(tr: &[Value]) -> Vec<Cue> {
    let ms = |o: Option<&Value>, k: &str| o.and_then(|o| o.get(k)).and_then(f).map(|v| v / 1000.0);
    let mut out = vec![];
    for s in tr {
        let start = ms(s.get("offsets"), "from").unwrap_or(0.0);
        let end = ms(s.get("offsets"), "to").unwrap_or(start).max(start);
        let toks = s.get("tokens").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        if toks.is_empty() {
            let text = s.get("text").and_then(|t| t.as_str()).unwrap_or("");
            out.push(Cue { start, end, words: distribute(text, start, end), timed: false });
            continue;
        }
        let mut words: Vec<CapWord> = vec![];
        for (i, t) in toks.iter().enumerate() {
            let text = t.get("text").and_then(|x| x.as_str()).unwrap_or("");
            if text.trim().is_empty() || text.starts_with("[_") {
                continue;
            }
            let a = ms(t.get("offsets"), "from").unwrap_or(start);
            let b = ms(t.get("offsets"), "to").unwrap_or(a).max(a);
            let new_word = text.starts_with(' ') || words.is_empty() || i == 0;
            if new_word {
                words.push(CapWord { text: text.trim().to_string(), start: a, end: b });
            } else if let Some(w) = words.last_mut() {
                w.text.push_str(text.trim());
                w.end = b.max(w.end);
            }
        }
        out.push(Cue { start, end, words, timed: true });
    }
    out
}

fn word_array(arr: &[Value]) -> Vec<Cue> {
    let mut out: Vec<Cue> = vec![];
    let mut cur: Vec<CapWord> = vec![];
    for w in arr {
        let t = word_text(w);
        if t.is_empty() {
            continue;
        }
        let a = w.get("start").and_then(f).unwrap_or(cur.last().map(|c| c.end).unwrap_or(0.0));
        let b = w.get("end").and_then(f).unwrap_or(a).max(a);
        if let Some(l) = cur.last() {
            if a - l.end > 0.6 {
                let words = std::mem::take(&mut cur);
                out.push(Cue { start: words[0].start, end: words.last().unwrap().end, words, timed: true });
            }
        }
        cur.push(CapWord { text: t, start: a, end: b });
    }
    if !cur.is_empty() {
        out.push(Cue { start: cur[0].start, end: cur.last().unwrap().end, words: cur, timed: true });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(c: &Cue) -> Vec<&str> {
        c.words.iter().map(|w| w.text.as_str()).collect()
    }

    #[test]
    fn ts_forms() {
        assert_eq!(parse_ts("00:00:01,500"), Some(1.5));
        assert_eq!(parse_ts("01:02:03.25"), Some(3723.25));
        assert_eq!(parse_ts("02:03.5"), Some(123.5));
        assert_eq!(parse_ts("00:00:02"), Some(2.0));
        assert_eq!(parse_ts("abc"), None);
        assert_eq!(parse_ts("1.5"), None);
    }

    #[test]
    fn distribute_by_length() {
        let w = distribute("a bbb", 0.0, 4.0);
        assert_eq!(w.len(), 2);
        assert!((w[0].end - 1.0).abs() < 1e-9 && w[1].start == w[0].end && w[1].end == 4.0);
        assert!(distribute("  ", 0.0, 1.0).is_empty());
    }

    #[test]
    fn srt_basic_tags_crlf_bom() {
        let s = "\u{feff}1\r\n00:00:01,000 --> 00:00:03,500\r\nHello <i>big</i>\r\n{\\an8}world\r\n\r\n2\r\n00:00:04,000 --> 00:00:05,000 X1:0\r\nBye.\r\n";
        let c = parse_srt(s).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(texts(&c[0]), ["Hello", "big", "world"]);
        assert_eq!(c[0].words[0].start, 1.0);
        assert_eq!(c[0].words[2].end, 3.5);
        assert!(!c[0].timed);
        assert!(parse_srt("nothing here").is_err());
    }

    #[test]
    fn vtt_header_note_settings() {
        let s = "WEBVTT Kind: captions\n\nNOTE a comment\nmore\n\nid1\n00:01.000 --> 00:02.000 align:start position:0%\n<v Ann>Hi <c.yellow>there</c>\n";
        let c = parse_vtt(s).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(texts(&c[0]), ["Hi", "there"]);
        assert!(!c[0].timed);
        assert!(parse_vtt("1\n00:01.000 --> 00:02.000\nx\n").is_err());
    }

    #[test]
    fn vtt_inline_timestamps() {
        let s = "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nOne <00:00:01.200><c> two</c><00:00:02.000><c> three</c>\n";
        let c = parse_vtt(s).unwrap();
        assert!(c[0].timed);
        assert_eq!(texts(&c[0]), ["One", "two", "three"]);
        let st: Vec<f64> = c[0].words.iter().map(|w| w.start).collect();
        assert_eq!(st, vec![1.0, 1.2, 2.0]);
        assert_eq!(c[0].words[2].end, 3.0);
    }

    #[test]
    fn vtt_youtube_duplicates() {
        let s = "WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nfirst line\n\n00:00:02.000 --> 00:00:04.000\nfirst line\nsecond line\n";
        let c = parse_vtt(s).unwrap();
        assert_eq!(texts(&c[1]), ["second", "line"]);
    }

    #[test]
    fn whisper_openai_words_and_fallback() {
        let s = r#"{"segments":[{"start":0.5,"end":1.5,"text":" Hi there","words":[{"word":" Hi","start":0.5,"end":0.8},{"word":" there","start":0.9}]},{"start":2,"end":3,"text":"no words here"}]}"#;
        let c = parse_whisper(s).unwrap();
        assert!(c[0].timed && !c[1].timed);
        assert_eq!(texts(&c[0]), ["Hi", "there"]);
        assert_eq!(c[0].words[1].end, 1.5);
        assert_eq!(texts(&c[1]), ["no", "words", "here"]);
    }

    #[test]
    fn whisper_cpp_tokens_merge() {
        let s = r#"{"transcription":[{"offsets":{"from":0,"to":2000},"text":" Hello world","tokens":[
            {"text":"[_BEG_]","offsets":{"from":0,"to":0}},
            {"text":" Hel","offsets":{"from":100,"to":300}},{"text":"lo","offsets":{"from":300,"to":500}},
            {"text":" world","offsets":{"from":600,"to":1000}},{"text":"[_TT_50]","offsets":{"from":1000,"to":1000}}]}]}"#;
        let c = parse_whisper(s).unwrap();
        assert_eq!(texts(&c[0]), ["Hello", "world"]);
        assert_eq!((c[0].words[0].start, c[0].words[0].end), (0.1, 0.5));
        assert_eq!(c[0].end, 2.0);
    }

    #[test]
    fn bare_word_array_splits_on_gaps() {
        let s = r#"[{"word":"a","start":0,"end":0.2},{"word":"b","start":0.3,"end":0.5},{"text":"c","start":2,"end":2.2}]"#;
        let c = parse_whisper(s).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(texts(&c[1]), ["c"]);
        assert!(parse_whisper(r#"{"foo":1}"#).is_err());
    }

    #[test]
    fn by_ext() {
        assert!(parse_by_ext("SRT", "1\n00:00:00,000 --> 00:00:01,000\nx\n").is_ok());
        assert!(parse_by_ext("txt", "").is_err());
    }
}
