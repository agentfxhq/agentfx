//! Narration-driven timing and written-word alignment.
use std::collections::{HashMap, HashSet};
use serde::Serialize;
use crate::ast::{Document, Node, Span};
use crate::diag::Diag;
use crate::values::{parse_anchor, parse_dur, parse_rate_pct, suggest, Anchor, AnchorBase, Dur};

#[derive(Debug, Clone, Serialize)]
pub struct TtsReq { pub key: String, pub text: String, pub voice: String, pub rate: String }
#[derive(Debug, Clone, Serialize)]
pub struct TtsWord { pub text: String, pub start: f64, pub end: f64 }
#[derive(Debug, Clone, Serialize)]
pub struct TtsResult { pub mp3: Option<String>, pub dur: f64, pub words: Vec<TtsWord>, pub estimated: bool }
#[derive(Debug, Clone, Serialize)]
pub struct WordT { pub text: String, pub start: f64, pub end: f64 }
#[derive(Debug, Clone, Serialize)]
pub struct AnchorT { pub name: String, pub start: f64, pub end: f64, pub say: usize }
#[derive(Debug, Clone, Serialize)]
pub struct SayT {
    pub key: String, pub id: Option<String>, pub start: f64, pub end: f64, pub audio_at: f64,
    pub audio_dur: f64, pub mp3: Option<String>, pub written: String, pub words: Vec<WordT>,
    pub estimated: bool, pub span: Span,
}
#[derive(Debug, Clone, Serialize)]
pub struct SceneT {
    pub id: String, pub start: f64, pub dur: f64, pub says: Vec<SayT>,
    pub anchors: Vec<AnchorT>, pub span: Span,
}
#[derive(Debug, Clone, Serialize)]
pub struct Timeline { pub fps: f64, pub total: f64, pub scenes: Vec<SceneT> }

fn prop(node: &Node, key: &str) -> Option<String> { node.props.get(key).map(|v| v.raw()) }
fn arg(node: &Node, i: usize) -> String { node.args.get(i).map(|v| v.raw()).unwrap_or_default() }
fn scene_id(node: &Node, i: usize) -> String {
    node.args.first().map(|v| v.raw().trim_start_matches('#').to_owned())
        .unwrap_or_else(|| format!("scene{}", i + 1))
}
fn pronunciations(doc: &Document) -> Vec<(String, String)> {
    doc.nodes.iter().filter(|n| n.kind == "pronounce" && n.args.len() >= 2)
        .map(|n| (arg(n, 0), arg(n, 1))).filter(|(a, _)| !a.is_empty()).collect()
}
// Markup ranges are byte offsets into the resulting written text.
fn markup(text: &str) -> (String, Vec<(String, usize, usize)>) {
    let (mut written, mut marks) = (String::new(), Vec::new());
    let mut rest = text;
    while let Some(open) = rest.find("[[") {
        written.push_str(&rest[..open]);
        let inner = &rest[open + 2..];
        let Some(close) = inner.find("]]") else { written.push_str(&rest[open..]); return (written, marks); };
        let (words, name) = inner[..close].split_once('|').unwrap_or((&inner[..close], &inner[..close]));
        let start = written.len();
        written.push_str(words);
        marks.push((name.trim().to_owned(), start, written.len()));
        rest = &inner[close + 2..];
    }
    written.push_str(rest);
    (written, marks)
}
fn clean(text: &str) -> String {
    text.replace(['↗', '…'], "").split_whitespace().collect::<Vec<_>>().join(" ")
}
fn request(scene: &str, i: usize, say: &Node, video: Option<&Node>, rules: &[(String, String)]) -> TtsReq {
    let mut text = markup(&arg(say, 0)).0;
    if let Some(spoken) = prop(say, "spoken") { text = spoken; }
    else { for (from, to) in rules { text = text.replace(from, to); } }
    let inherited = |key, default: &str| prop(say, key)
        .or_else(|| video.and_then(|v| prop(v, key))).unwrap_or_else(|| default.into());
    TtsReq { key: format!("{scene}-{i}"), text: clean(&text),
        voice: inherited("voice", ""), rate: inherited("rate", "+0%") }
}
pub fn tts_requests(doc: &Document) -> Vec<TtsReq> {
    let video = doc.nodes.iter().find(|n| n.kind == "video");
    let rules = pronunciations(doc);
    doc.nodes.iter().filter(|n| n.kind == "scene").enumerate().flat_map(|(i, scene)| {
        let id = scene_id(scene, i);
        scene.children.iter().filter(|n| n.kind == "say").enumerate()
            .map(|(j, say)| request(&id, j, say, video, &rules)).collect::<Vec<_>>()
    }).collect()
}
pub fn estimate(req: &TtsReq) -> TtsResult {
    let scale = (1.0 + parse_rate_pct(&req.rate).unwrap_or(0.0) / 100.0).max(0.01);
    let dur = req.text.chars().count() as f64 / (15.0 * scale);
    let tokens: Vec<_> = req.text.split_whitespace().collect();
    let count = tokens.iter().map(|w| w.chars().count()).sum::<usize>().max(1) as f64;
    let mut at = 0.05_f64.min(dur);
    let available = (dur - at).max(0.0);
    let words = tokens.into_iter().map(|text| {
        let start = at;
        at += available * text.chars().count() as f64 / count;
        TtsWord { text: text.into(), start, end: at }
    }).collect();
    TtsResult { mp3: None, dur, words, estimated: true }
}
fn tokens(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(a) = start.take() { out.push((a, i, text[a..i].to_owned())); }
        } else if start.is_none() { start = Some(i); }
    }
    if let Some(a) = start { out.push((a, text.len(), text[a..].to_owned())); }
    out
}
fn normal(s: &str) -> String { s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect() }
fn matches(a: &str, b: &str) -> bool {
    a == b || (!a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a)
        || (a.chars().count() >= 3 && a.chars().take(3).eq(b.chars().take(3)))))
}
// Carry written-word ownership through exact substitutions, including multiword
// replacements. Byte ownership is safe because replacements use UTF-8 boundaries.
fn expected(written: &str, rules: &[(String, String)]) -> Vec<(String, Vec<usize>)> {
    let mut text = written.to_owned();
    let mut owners = vec![Vec::new(); text.len()];
    for (i, (a, b, _)) in tokens(written).iter().enumerate() {
        for slot in &mut owners[*a..*b] { slot.push(i); }
    }
    for (from, to) in rules {
        let mut next = String::new();
        let mut mapped = Vec::new();
        let mut at = 0;
        for (a, _) in text.match_indices(from) {
            next.push_str(&text[at..a]);
            mapped.extend_from_slice(&owners[at..a]);
            let b = a + from.len();
            let mut ids: Vec<_> = owners[a..b].iter().flatten().copied().collect();
            ids.sort_unstable(); ids.dedup();
            next.push_str(to);
            mapped.extend(std::iter::repeat_n(ids, to.len()));
            at = b;
        }
        next.push_str(&text[at..]); mapped.extend_from_slice(&owners[at..]);
        text = next; owners = mapped;
    }
    // Removing delivery marks must not split a token or shift its ownership.
    let mut cleaned = String::new();
    let mut mapped = Vec::new();
    for (i, c) in text.char_indices() {
        if c != '↗' && c != '…' {
            cleaned.push(c); mapped.extend_from_slice(&owners[i..i + c.len_utf8()]);
        }
    }
    tokens(&cleaned).into_iter().map(|(a, b, token)| {
        let mut ids: Vec<_> = mapped[a..b].iter().flatten().copied().collect();
        ids.sort_unstable(); ids.dedup(); (normal(&token), ids)
    }).collect()
}
fn align(written: &str, rules: &[(String, String)], tts: &TtsResult, audio_at: f64) -> Vec<WordT> {
    let written_tokens = tokens(written);
    let expected = expected(written, rules);
    let spoken: Vec<_> = tts.words.iter().map(|w| normal(&w.text)).collect();
    let (n, m) = (expected.len(), spoken.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate() { row[0] = i; }
    for j in 0..=m { dp[0][j] = j; }
    for i in 1..=n { for j in 1..=m {
        let cost = usize::from(!matches(&expected[i - 1].0, &spoken[j - 1]));
        dp[i][j] = (dp[i - 1][j - 1] + cost).min(dp[i - 1][j] + 1).min(dp[i][j - 1] + 1);
    } }
    let mut coverage = vec![Vec::new(); n];
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && dp[i][j] == dp[i - 1][j - 1]
            + usize::from(!matches(&expected[i - 1].0, &spoken[j - 1])) {
            coverage[i - 1].push(j - 1); i -= 1; j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j] == dp[i][j - 1] + 1) {
            if n > 0 { coverage[i.saturating_sub(1)].push(j - 1); }
            j -= 1;
        } else { i -= 1; }
    }
    let mut times: Vec<Option<(f64, f64)>> = vec![None; written_tokens.len()];
    for (k, covered) in coverage.iter().enumerate() {
        for &word in covered { for &owner in &expected[k].1 {
            let w = &tts.words[word];
            times[owner] = Some(match times[owner] {
                Some((a, b)) => (a.min(w.start), b.max(w.end)), None => (w.start, w.end),
            });
        } }
    }
    // Fill entire missing runs between their nearest timed neighbours.
    let mut i = 0;
    while i < times.len() {
        if times[i].is_some() { i += 1; continue; }
        let first = i;
        while i < times.len() && times[i].is_none() { i += 1; }
        let a = first.checked_sub(1).and_then(|k| times[k]).map(|v| v.1).unwrap_or(0.0);
        let b = times.get(i).copied().flatten().map(|v| v.0).unwrap_or(tts.dur).max(a);
        for (k, slot) in times.iter_mut().enumerate().take(i).skip(first) {
            let step = (b - a) / (i - first) as f64;
            *slot = Some((a + step * (k - first) as f64, a + step * (k - first + 1) as f64));
        }
    }
    written_tokens.into_iter().zip(times).map(|((_, _, text), time)| {
        let (start, end) = time.unwrap_or((0.0, 0.0));
        WordT { text, start: audio_at + start, end: audio_at + end }
    }).collect()
}
fn duration(raw: Option<String>, default: f64, fps: f64, span: Span, diags: &mut Vec<Diag>) -> f64 {
    let Some(raw) = raw else { return default; };
    match parse_dur(&raw) {
        Ok(d) if d.secs(fps).is_finite() && d.secs(fps) >= 0.0 => d.secs(fps),
        Ok(_) => { diags.push(Diag::error("E_DUR", "Duration must be nonnegative", "use a nonnegative duration", span)); default }
        Err(e) => { diags.push(Diag::error(&e.code, e.message, e.hint, span)); default }
    }
}
fn add_anchor(scene: &mut SceneT, anchor: AnchorT, span: Span, diags: &mut Vec<Diag>) {
    if scene.anchors.iter().any(|a| a.name == anchor.name) {
        diags.push(Diag::warn("W_ANCHOR_DUP", format!("Duplicate anchor `{}`", anchor.name),
            "use distinct anchor names; the first wins scene-level lookup", span));
    }
    scene.anchors.push(anchor);
}
pub fn solve(doc: &Document, tts: &HashMap<String, TtsResult>, fps_override: Option<f64>) -> (Timeline, Vec<Diag>) {
    let video = doc.nodes.iter().find(|n| n.kind == "video");
    let fps = fps_override.or_else(|| video.and_then(|v| prop(v, "fps")).and_then(|v| v.parse().ok()))
        .filter(|v: &f64| v.is_finite() && *v > 0.0).unwrap_or(30.0);
    let mut diags = Vec::new();
    let video_span = video.map(|v| v.span).unwrap_or_default();
    let lead = duration(video.and_then(|v| prop(v, "lead")), Dur::Frames(2.0).secs(fps), fps, video_span, &mut diags);
    let tail = duration(video.and_then(|v| prop(v, "tail")), Dur::Frames(6.0).secs(fps), fps, video_span, &mut diags);
    let rules = pronunciations(doc);
    let mut timeline = Timeline { fps, total: 0.0, scenes: Vec::new() };
    let mut marked = Vec::new();
    for (scene_index, node) in doc.nodes.iter().filter(|n| n.kind == "scene").enumerate() {
        let mut scene = SceneT { id: scene_id(node, scene_index), start: timeline.total,
            dur: 0.0, says: Vec::new(), anchors: Vec::new(), span: node.span };
        let mut at = scene.start + duration(prop(node, "pre"), 0.0, fps, node.span, &mut diags);
        let (mut cap_beats, mut cap_says, mut cap_anchors) = (0usize, Vec::new(), Vec::new());
        for beat in &node.children {
            if beat.kind == "pause" {
                at += duration(beat.args.first().map(|v| v.raw()), 0.0, fps, beat.span, &mut diags);
            } else if beat.kind == "say" {
                let index = scene.says.len();
                let req = request(&scene.id, index, beat, video, &rules);
                let fallback;
                let audio = match tts.get(&req.key) { Some(a) => a, None => { fallback = estimate(&req); &fallback } };
                let pre = duration(prop(beat, "pre"), lead, fps, beat.span, &mut diags);
                let post = duration(prop(beat, "post"), tail, fps, beat.span, &mut diags);
                let min = duration(prop(beat, "min"), 0.0, fps, beat.span, &mut diags);
                let end = at + (pre + audio.dur + post).max(min);
                let audio_at = at + pre;
                let (written, marks) = markup(&arg(beat, 0));
                let words = align(&written, if beat.props.contains_key("spoken") { &[] } else { &rules }, audio, audio_at);
                let bounds = tokens(&written);
                for (name, a, b) in marks {
                    let covered: Vec<_> = bounds.iter().enumerate().filter(|(_, (x, y, _))| *x < b && *y > a).map(|(k, _)| k).collect();
                    let start = covered.first().map(|&k| words[k].start).unwrap_or(audio_at);
                    let end = covered.last().map(|&k| words[k].end).unwrap_or(start);
                    marked.push((scene_index, scene.anchors.len(), beat.span));
                    add_anchor(&mut scene, AnchorT { name, start, end, say: index }, beat.span, &mut diags);
                }
                let id = beat.args.iter().skip(1).map(|v| v.raw())
                    .find_map(|s| s.strip_prefix('#').map(str::to_owned));
                if let Some(name) = &id {
                    add_anchor(&mut scene, AnchorT { name: name.clone(), start: audio_at,
                        end: audio_at + audio.dur, say: index }, beat.span, &mut diags);
                }
                scene.says.push(SayT { key: req.key, id, start: at, end, audio_at, audio_dur: audio.dur,
                    mp3: audio.mp3.clone(), written, words, estimated: audio.estimated, span: beat.span });
                at = end;
            } else if beat.kind == "captions" && beat.props.contains_key("src") {
                // a transcript beat: prerecorded audio with known words, one say per source cue
                let j = cap_beats;
                cap_beats += 1;
                let src = prop(beat, "src").unwrap_or_default();
                let path = crate::design::doc_base().join(&src);
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_owned();
                let parsed = std::fs::read_to_string(&path).map_err(|e| format!("cannot read `{src}`: {e}"))
                    .and_then(|s| crate::capsrc::parse_by_ext(&ext, &s));
                match parsed {
                    Ok(cues) => {
                        let pre = duration(prop(beat, "pre"), 0.0, fps, beat.span, &mut diags);
                        let post = duration(prop(beat, "post"), tail, fps, beat.span, &mut diags);
                        let shift = prop(beat, "offset").map(|raw| {
                            let (neg, body) = match raw.strip_prefix('-') { Some(b) => (true, b.to_owned()), None => (false, raw.clone()) };
                            let v = duration(Some(body), 0.0, fps, beat.span, &mut diags);
                            if neg { -v } else { v }
                        }).unwrap_or(0.0);
                        let audio_at = at + pre;
                        let last = cues.iter().map(|c| c.end).fold(0.0_f64, f64::max);
                        let end = audio_at + (last + shift).max(0.0) + post;
                        if cues.iter().any(|c| !c.timed) {
                            diags.push(Diag::info("I_CAPTION_ESTIMATED", format!("`{src}` has no word timings; words are spread evenly over each cue"),
                                "use a whisper JSON with word timestamps (or a VTT with inline <00:00:01.200> tags) for exact word sync", beat.span));
                        }
                        for (k, c) in cues.iter().enumerate() {
                            let words: Vec<WordT> = c.words.iter().map(|w| WordT { text: w.text.clone(),
                                start: audio_at + shift + w.start, end: audio_at + shift + w.end }).collect();
                            let written = c.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
                            cap_says.push(SayT { key: format!("{}~cap{j}~{k}", scene.id), id: None,
                                start: audio_at + shift + c.start, end: audio_at + shift + c.end, audio_at,
                                audio_dur: if k == 0 { last.max(0.0) } else { 0.0 }, mp3: None, written, words,
                                estimated: false, span: beat.span });
                        }
                        if let Some(name) = beat.args.iter().map(|v| v.raw()).find_map(|s| s.strip_prefix('#').map(str::to_owned)) {
                            // not tied to one say (a transcript spans several cue beats)
                            cap_anchors.push(AnchorT { name, start: audio_at, end: end - post, say: usize::MAX });
                        }
                        at = end;
                    }
                    Err(e) => diags.push(Diag::error("E_CAPTION_SRC", format!("captions src=`{src}`: {e}"),
                        "src is relative to the .reel file: .srt, .vtt or whisper .json (openai-whisper, faster-whisper, whisper.cpp -oj/-ojf)", beat.span)),
                }
            }
        }
        // transcript beats go after the narration says, so say indices of `say` nodes stay stable
        for a in cap_anchors.drain(..) { add_anchor(&mut scene, a, node.span, &mut diags); }
        scene.says.append(&mut cap_says);
        let content = at - scene.start + duration(prop(node, "post"), 0.5, fps, node.span, &mut diags);
        let fixed = duration(prop(node, "dur"), content, fps, node.span, &mut diags);
        if node.props.contains_key("dur") && content > fixed + 1e-9 {
            diags.push(Diag::error("E_TIMING_OVERRUN", format!("Scene content is {content:.3}s, exceeding dur={fixed:.3}s"),
                "remove dur= so the scene follows its narration, or shorten the narration", node.span));
        }
        scene.dur = content.max(fixed).max(duration(prop(node, "min"), 0.0, fps, node.span, &mut diags));
        timeline.total += scene.dur;
        timeline.scenes.push(scene);
    }
    // music beat grid (`beat:N`, `bar:N`), only when the document uses audio reactivity
    if let Some(f) = crate::audio::load(doc, &crate::design::doc_base(), fps, &mut diags) {
        crate::audio::anchors(&mut timeline, &f);
    }
    let mut used = HashSet::new();
    for (i, node) in doc.nodes.iter().filter(|n| n.kind == "scene").enumerate() {
        let mut say_index = 0;
        let mut prev = timeline.scenes[i].start;
        for child in &node.children {
            let (say, events) = if child.kind == "say" {
                let index = say_index; say_index += 1;
                (Some(index), child.children.iter().filter(|n| n.kind == "at").collect::<Vec<_>>())
            } else { (None, if child.kind == "at" { vec![child] } else { Vec::new() }) };
            for event in events {
                let expr = arg(event, 0);
                match timeline.anchor_time(i, say, &expr, prev, event.span) {
                    Ok(time) => {
                        prev = time;
                        if time > timeline.total + 1e-6 && (expr.contains("beat:") || expr.contains("bar:")) {
                            diags.push(Diag::warn("W_BEAT_PAST_END", format!("`{expr}` is at {time:.2}s, after the video ends ({:.2}s)", timeline.total),
                                "use an earlier beat or lengthen the scene with dur=", event.span).with_at(time));
                        }
                        if let Ok(Anchor { base: AnchorBase::Name(name), .. }) = parse_anchor(&expr) {
                            if let Some(k) = lookup(&timeline.scenes[i], say, &name) { used.insert((i, k)); }
                        }
                    }
                    Err(e) => diags.push(e),
                }
            }
        }
    }
    // anchors referenced from element props (at=@x, until=@x, emit=@x) and @x:value keyframes
    fn refs(n: &Node, out: &mut Vec<String>) {
        for v in n.props.values().chain(n.args.iter()) {
            let r = v.raw();
            if let Some(i) = r.find('@') {
                let name: String = r[i + 1..].chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
                if !name.is_empty() { out.push(name); }
            }
        }
        if n.kind == "at" {
            if let Some(a) = n.args.first() {
                let r = a.raw();
                let name: String = r.trim_start_matches('@').chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
                if !name.is_empty() { out.push(name); }
            }
        }
        for c in &n.children { refs(c, out); }
    }
    for (i, node) in doc.nodes.iter().filter(|n| n.kind == "scene").enumerate() {
        let mut names = Vec::new();
        refs(node, &mut names);
        for name in names {
            if let Some(k) = timeline.scenes[i].anchors.iter().position(|a| a.name == name) { used.insert((i, k)); }
        }
    }
    for (i, k, span) in marked {
        if !used.contains(&(i, k)) {
            diags.push(Diag::warn("W_ANCHOR_UNUSED", format!("Unused anchor `{}`", timeline.scenes[i].anchors[k].name),
                "reference this anchor in an event or remove its markup", span));
        }
    }
    (timeline, diags)
}
fn lookup(scene: &SceneT, say: Option<usize>, name: &str) -> Option<usize> {
    say.and_then(|s| scene.anchors.iter().position(|a| a.say == s && a.name == name))
        .or_else(|| scene.anchors.iter().position(|a| a.name == name))
}
impl Timeline {
    /// Resolve an event anchor; child events prefer anchors in their own say.
    pub fn anchor_time(&self, scene: usize, say: Option<usize>, expr: &str, prev: f64, span: Span) -> Result<f64, Diag> {
        let scene = self.scenes.get(scene).ok_or_else(|| Diag::error("E_ANCHOR", "Unknown scene", "use an existing scene index", span))?;
        let anchor = parse_anchor(expr).map_err(|e| Diag::error("E_ANCHOR", e.message, e.hint, span))?;
        let base = match anchor.base {
            AnchorBase::Name(name) => {
                let k = lookup(scene, say, &name).ok_or_else(|| {
                    if let Some((grid, _)) = name.split_once(':') {
                        let count = scene.anchors.iter().filter(|a| a.name.starts_with(&format!("{grid}:"))).count();
                        return if count == 0 {
                            Diag::error("E_ANCHOR", format!("`{name}` needs an analysed music track"), "add a top-level `music src=track.wav` (optional bpm=128); beat anchors come from its beat grid", span)
                        } else {
                            Diag::error("E_BEAT_RANGE", format!("`{name}` is beyond the music track, which has {count} {grid}s"), format!("use {grid}:1..{grid}:{count}, or a longer track"), span)
                        };
                    }
                    let mut names: Vec<_> = scene.anchors.iter().map(|a| a.name.as_str()).collect();
                    names.sort_unstable(); names.dedup();
                    if let Some(best) = suggest(&name, &names) { names.retain(|n| *n != best); names.insert(0, best); }
                    names.truncate(5);
                    let hint = if names.is_empty() { "define an anchor using [[words|name]]".into() }
                        else { format!("did you mean {}?", names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")) };
                    Diag::error("E_ANCHOR", format!("Unknown anchor `{name}`"), hint, span)
                })?;
                if anchor.end_edge { scene.anchors[k].end } else { scene.anchors[k].start }
            }
            AnchorBase::Time(d) => scene.start + d.secs(self.fps),
            AnchorBase::SceneStart => scene.start,
            AnchorBase::SceneEnd => scene.start + scene.dur,
            AnchorBase::Prev => prev,
        };
        Ok(base + anchor.offset.map(|d| d.secs(self.fps)).unwrap_or(0.0))
    }
    /// Half-open scene intervals, clamped to the first/last scene; empty => 0.
    pub fn scene_at(&self, t: f64) -> usize {
        self.scenes.iter().rposition(|s| t >= s.start).unwrap_or(0)
    }
}
