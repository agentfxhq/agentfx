//! Text v2: custom font files (TTF, WOFF, WOFF2), axes and features, text stroke, text color
//! tracks, text animators (`animate`) and text on a path, down to the reel-ir they emit.
use agentfx_script::{diag::Severity, lower, parse, timing};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let p = Path::new(env!("AGENTFX_ROOT")).join("target/test-out/textv2/tests").join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn fonts_dir() -> PathBuf {
    Path::new(env!("AGENTFX_ROOT")).join("assets/fonts")
}
fn doc(head: &str, body: &str) -> (agentfx_script::ast::Document, timing::Timeline) {
    let (doc, errors) = parse::parse(&format!("video \"test\" size=640x360 fps=12 captions=off\n{head}scene a dur=2s bg=#000\n{body}"));
    assert!(errors.is_empty(), "{errors:?}");
    let (tl, _) = timing::solve(&doc, &Default::default(), None);
    (doc, tl)
}
fn build(head: &str, body: &str, base: &Path) -> lower::Compiled {
    let (d, tl) = doc(head, body);
    lower::compile(&d, &tl, None, base)
}
fn codes(c: &lower::Compiled) -> Vec<String> {
    c.diags.iter().map(|d| d.code.clone()).collect()
}
fn no_errors(c: &lower::Compiled) {
    assert!(c.diags.iter().all(|d| d.severity != Severity::Error), "{:?}", c.diags);
}
fn scene(c: &lower::Compiled, dir: &Path) -> agentfx_ir::Scene {
    let out = agentfx_script::direct::compile(&c.ir, dir, true).unwrap();
    assert!(out.diags.iter().all(|d| d.severity != Severity::Error), "{:?}", out.diags);
    serde_json::from_value(out.scene).unwrap()
}
fn node<'a>(s: &'a agentfx_ir::Scene, name: &str) -> (usize, &'a agentfx_ir::Node) {
    s.nodes.iter().enumerate().find(|(_, n)| n.name == name).unwrap_or_else(|| panic!("no node {name}"))
}
fn glyph_runs(n: &agentfx_ir::Node) -> Vec<&agentfx_ir::Draw> {
    match &n.content {
        Some(agentfx_ir::Content::Paint { items, .. }) => items.iter().filter(|d| matches!(d, agentfx_ir::Draw::Glyphs { .. })).collect(),
        _ => vec![],
    }
}

// ------------------------------------------------------------ WOFF / WOFF2 writers (test only)
// WOFF 1.0 may store a table uncompressed (compLength == origLength), WOFF2 may use null
// transforms (glyf/loca version 3) and a brotli stream of uncompressed meta-blocks: enough to
// produce valid files without a compressor.

fn sfnt_tables(f: &[u8]) -> (u32, Vec<([u8; 4], u32, Vec<u8>)>) {
    let be32 = |o: usize| u32::from_be_bytes(f[o..o + 4].try_into().unwrap());
    let n = u16::from_be_bytes([f[4], f[5]]) as usize;
    let mut t = vec![];
    for i in 0..n {
        let o = 12 + 16 * i;
        let (off, len) = (be32(o + 8) as usize, be32(o + 12) as usize);
        t.push((f[o..o + 4].try_into().unwrap(), be32(o + 4), f[off..off + len].to_vec()));
    }
    (be32(0), t)
}

fn woff1(f: &[u8]) -> Vec<u8> {
    let (flavor, tables) = sfnt_tables(f);
    let mut dir = vec![];
    let mut data = vec![];
    let mut off = 44 + 20 * tables.len();
    let total_sfnt = 12 + 16 * tables.len() + tables.iter().map(|t| (t.2.len() + 3) & !3).sum::<usize>();
    for (tag, sum, d) in &tables {
        dir.extend_from_slice(tag);
        for v in [off as u32, d.len() as u32, d.len() as u32, *sum] {
            dir.extend_from_slice(&v.to_be_bytes());
        }
        data.extend_from_slice(d);
        while data.len() % 4 != 0 {
            data.push(0);
        }
        off = 44 + 20 * tables.len() + data.len();
    }
    let mut out = b"wOFF".to_vec();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&((44 + dir.len() + data.len()) as u32).to_be_bytes());
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(total_sfnt as u32).to_be_bytes());
    out.extend_from_slice(&[0, 1, 0, 0]); // version
    out.extend_from_slice(&[0; 20]); // meta / priv
    out.extend(dir);
    out.extend(data);
    out
}

/// Brotli stream of uncompressed meta-blocks (16-bit window, <= 65536 bytes per block).
fn brotli_stored(data: &[u8]) -> Vec<u8> {
    struct W {
        out: Vec<u8>,
        acc: u64,
        n: u32,
    }
    impl W {
        fn bits(&mut self, v: u64, k: u32) {
            self.acc |= v << self.n;
            self.n += k;
            while self.n >= 8 {
                self.out.push(self.acc as u8);
                self.acc >>= 8;
                self.n -= 8;
            }
        }
        fn align(&mut self) {
            if self.n > 0 {
                self.bits(0, 8 - self.n);
            }
        }
    }
    let mut w = W { out: vec![], acc: 0, n: 0 };
    w.bits(0, 1); // WBITS = 16
    for chunk in data.chunks(65536) {
        w.bits(0, 1); // ISLAST = 0
        w.bits(0, 2); // MNIBBLES = 4
        w.bits(chunk.len() as u64 - 1, 16);
        w.bits(1, 1); // ISUNCOMPRESSED
        w.align();
        w.out.extend_from_slice(chunk);
    }
    w.bits(1, 1); // ISLAST
    w.bits(1, 1); // ISLASTEMPTY
    w.align();
    w.out
}

fn base128(mut v: u32, out: &mut Vec<u8>) {
    let mut b = vec![(v & 0x7f) as u8];
    v >>= 7;
    while v > 0 {
        b.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    b.reverse();
    out.extend(b);
}

fn woff2(f: &[u8]) -> Vec<u8> {
    let (flavor, tables) = sfnt_tables(f);
    let mut dir = vec![];
    let mut stream = vec![];
    for (tag, _, d) in &tables {
        // arbitrary tag (63) + null transform: version 3 for glyf/loca, 0 for the rest
        let ver = if tag == b"glyf" || tag == b"loca" { 3u8 } else { 0 };
        dir.push(63 | (ver << 6));
        dir.extend_from_slice(tag);
        base128(d.len() as u32, &mut dir);
        stream.extend_from_slice(d);
    }
    let comp = brotli_stored(&stream);
    let total_sfnt = 12 + 16 * tables.len() + tables.iter().map(|t| (t.2.len() + 3) & !3).sum::<usize>();
    let mut out = b"wOF2".to_vec();
    out.extend_from_slice(&flavor.to_be_bytes());
    let len = 48 + dir.len() + comp.len();
    out.extend_from_slice(&((len + 3) & !3).to_be_bytes()[4..]);
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(total_sfnt as u32).to_be_bytes());
    out.extend_from_slice(&(comp.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0, 1, 0, 0]);
    out.extend_from_slice(&[0; 20]);
    out.extend(dir);
    out.extend(comp);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

// ------------------------------------------------------------ fonts

#[test]
fn custom_font_files_load_in_every_container_and_are_used_by_text() {
    let dir = scratch("fonts");
    let ttf = std::fs::read(fonts_dir().join("SpaceGrotesk.ttf")).unwrap();
    std::fs::write(dir.join("Brand.ttf"), &ttf).unwrap();
    std::fs::write(dir.join("Brand.woff"), woff1(&ttf)).unwrap();
    std::fs::write(dir.join("Brand.woff2"), woff2(&ttf)).unwrap();
    for ext in ["ttf", "woff", "woff2"] {
        let decoded = agentfx_script::fonts::load(&dir.join(format!("Brand.{ext}"))).unwrap_or_else(|e| panic!("{ext}: {e:#}"));
        assert!(agentfx_script::fonts::has_char(&decoded, 'Q'), "{ext}");
        let c = build(&format!("font \"Brand\" src=\"Brand.{ext}\"\n"), "  text #t \"Quartz\" font=Brand size=48 x=10 y=10\n  text #i \"Quartz\" size=48 x=10 y=100\n", &dir);
        no_errors(&c);
        let s = scene(&c, &dir.join(format!("out-{ext}")));
        let font_of = |name: &str| match glyph_runs(node(&s, name).1)[0] {
            agentfx_ir::Draw::Glyphs { font, .. } => *font,
            _ => unreachable!(),
        };
        let (fb, fi) = (font_of("t"), font_of("i"));
        assert_ne!(fb, fi, "{ext}: the custom font is a separate face");
        let path = &s.fonts[fb].path;
        if ext == "ttf" {
            assert!(path.ends_with("Brand.ttf"), "TTF referenced in place: {path}");
        } else {
            // decoded SFNT written next to the IR
            let p = dir.join(format!("out-{ext}")).join(path);
            let bytes = std::fs::read(&p).unwrap_or_else(|_| panic!("{ext}: {}", p.display()));
            assert_eq!(bytes.get(0..4), Some(&[0u8, 1, 0, 0][..]), "{ext}");
        }
    }
}

#[test]
fn bad_font_declarations_are_errors_and_unknown_families_warn() {
    let dir = scratch("badfont");
    std::fs::write(dir.join("x.woff2"), b"wOF2 not really").unwrap();
    std::fs::write(dir.join("y.ttf"), b"hello").unwrap();
    for head in ["font \"B\" src=\"missing.ttf\"\n", "font \"B\" src=\"x.woff2\"\n", "font \"B\" src=\"y.ttf\"\n", "font \"B\" src=\"a.png\"\n", "font src=\"y.ttf\"\n", "font \"B\"\n"] {
        let c = build(head, "  text \"x\"\n", &dir);
        assert!(codes(&c).contains(&"E_FONT".to_string()), "{head}: {:?}", c.diags);
    }
    let (d, tl) = doc("", "  text #t \"x\" font=Fraunce\n  text #u \"y\" font=Fraunces\n");
    let c = lower::compile(&d, &tl, None, &dir);
    let st = agentfx_script::lint::static_rules(&d, &tl, &c.ir);
    let fam: Vec<&agentfx_script::diag::Diag> = st.iter().filter(|x| x.code == "W_FONT_FAMILY").collect();
    assert_eq!(fam.len(), 1, "{st:?}");
    assert!(fam[0].hint.contains("Fraunces"), "{}", fam[0].hint);
}

#[test]
fn axes_and_features_reach_the_shaper() {
    let dir = scratch("axes");
    let head = format!("font \"Brand\" src=\"{}\"\n", fonts_dir().join("Fraunces.ttf").display());
    let c = build(&head, "  text #soft \"Wonky\" font=Brand size=60 axes=\"SOFT 100, WONK 1\" x=10 y=10\n  text #hard \"Wonky\" font=Brand size=60 axes=\"SOFT 0, WONK 0\" x=10 y=120\n  text #tn \"1111\" size=60 features=\"tnum\" x=10 y=200\n  text #pn \"1111\" size=60 features=\"tnum off\" x=10 y=280\n", &dir);
    no_errors(&c);
    let s = scene(&c, &dir);
    let coords = |name: &str| match glyph_runs(node(&s, name).1)[0] {
        agentfx_ir::Draw::Glyphs { coords, .. } => coords.clone(),
        _ => unreachable!(),
    };
    assert_ne!(coords("soft"), coords("hard"), "SOFT/WONK change the variation coordinates");
    let width = |name: &str| match glyph_runs(node(&s, name).1)[0] {
        agentfx_ir::Draw::Glyphs { glyphs, .. } => glyphs.last().unwrap().1 - glyphs[0].1,
        _ => unreachable!(),
    };
    assert!((width("tn") - width("pn")).abs() > 1.0, "tnum changes the advances of 1: {} vs {}", width("tn"), width("pn"));
    // bad syntax is an error with the offending entry
    let c = build("", "  text \"x\" axes=\"wdth\" features=\"toolong on\"\n", &dir);
    assert_eq!(codes(&c).iter().filter(|x| *x == "E_VALUE").count(), 2, "{:?}", c.diags);
}

#[test]
fn custom_axis_channels_animate_through_relayout() {
    let dir = scratch("axisanim");
    let head = format!("font \"Brand\" src=\"{}\"\n", fonts_dir().join("Fraunces.ttf").display());
    let c = build(&head, "  text #t \"Soft\" font=Brand size=60 x=10 y=10\n    anim SOFT 0s:0 1s:100\n    anim font-weight 0s:300 1s:900\n", &dir);
    no_errors(&c);
    let s = scene(&c, &dir);
    let (_, n) = node(&s, "t");
    match &n.content {
        Some(agentfx_ir::Content::Switch { states, .. }) => assert!(states.len() > 5, "{} states", states.len()),
        other => panic!("expected repaint states, got {other:?}"),
    }
}

// ------------------------------------------------------------ stroke and color

#[test]
fn text_stroke_is_a_glyph_stroke_and_color_is_a_track() {
    let dir = scratch("stroke");
    let c = build("", "  text #o \"Outline\" size=60 color=#fff stroke=#f00 sw=5 x=10 y=10\n    anim color 0s:#ffffff 1s:#00ff00\n  text #v \"Over\" size=60 stroke=#00f sw=2 stroke-over x=10 y=120\n", &dir);
    no_errors(&c);
    let s = scene(&c, &dir);
    let (oi, o) = node(&s, "o");
    for g in glyph_runs(o) {
        let agentfx_ir::Draw::Glyphs { stroke: Some(st), .. } = g else { panic!("no stroke") };
        assert_eq!((st.width, st.over), (10.0, false), "sw = visible outline: twice as wide below the fill");
        assert_eq!(st.brush, agentfx_ir::Brush::Solid([1.0, 0.0, 0.0, 1.0]));
    }
    // the color channel is one text.fill track, not repaint states
    assert!(matches!(o.content, Some(agentfx_ir::Content::Paint { .. })), "{:?}", o.content);
    let tr: Vec<&agentfx_ir::Track> = s.tracks.iter().filter(|t| t.node == oi && t.property == "text.fill").collect();
    assert_eq!(tr.len(), 1);
    assert_eq!(tr[0].keyframes.last().unwrap().value, agentfx_ir::Value::Color([0.0, 1.0, 0.0, 1.0]));
    let (_, v) = node(&s, "v");
    let agentfx_ir::Draw::Glyphs { stroke: Some(st), .. } = glyph_runs(v)[0] else { panic!() };
    assert_eq!((st.width, st.over), (2.0, true));
    // accent runs keep their own colors: repaint states as before
    let c = build("", "  text #r \"plain {accent}\" size=40 x=10 y=10\n    anim color 0s:#fff 1s:#0f0\n", &dir);
    let s = scene(&c, &dir.join("r"));
    assert!(matches!(node(&s, "r").1.content, Some(agentfx_ir::Content::Switch { .. })));
}

// ------------------------------------------------------------ animators

#[test]
fn animate_builds_range_selectors_with_tracks() {
    let dir = scratch("animate");
    let c = build("", "  text #t \"Hello animated world\" size=40 x=10 y=10\n  @0.5s animate #t by=word shape=ramp range=0%..30% offset=-30%..100% dur=1s ease=linear opacity=0 y=40 blur=6 rot=10 scale=0.5 color=#f00 order=random seed=3\n", &dir);
    no_errors(&c);
    let s = scene(&c, &dir);
    let (ti, t) = node(&s, "t");
    let Some(agentfx_ir::Content::Paint { text: Some(fx), .. }) = &t.content else { panic!("{:?}", t.content) };
    let a = &fx.animators[0];
    assert_eq!(a.by, "word");
    assert_eq!((a.opacity, a.translate, a.scale, a.rotate, a.blur), (Some(0.0), Some([0.0, 40.0]), Some([0.5, 0.5]), Some(10.0), Some(6.0)));
    assert_eq!(a.color, Some([1.0, 0.0, 0.0, 1.0]));
    match &a.selector {
        agentfx_ir::TextSelector::Range { start, end, offset, shape, order, seed, .. } => {
            assert_eq!((*start, *end, *offset), (0.0, 0.3, -0.3));
            assert_eq!((shape.as_str(), order.as_str(), *seed), ("ramp-up", "random", 3));
        }
        other => panic!("{other:?}"),
    }
    let tr: Vec<&agentfx_ir::Track> = s.tracks.iter().filter(|x| x.node == ti && x.property == "animators.0.offset").collect();
    assert_eq!(tr.len(), 1);
    assert!((tr[0].delay - 0.5).abs() < 1e-6 && (tr[0].duration - 1.0).abs() < 1e-6);
    assert_eq!(tr[0].keyframes.last().unwrap().value, agentfx_ir::Value::Number(1.0));
    // validates and renders as a reel scene
    agentfx_ir::validate::validate(&s, &dir).unwrap();
}

#[test]
fn animate_times_are_per_unit_keys_and_bad_input_is_diagnosed() {
    let dir = scratch("animkeys");
    let c = build("", "  text #t \"one two three\" size=40 x=10 y=10\n  @0.2s animate #t by=word times=[0s, 0.3s, 0.9s] dur=0.25s y=30 opacity=0\n", &dir);
    no_errors(&c);
    let s = scene(&c, &dir);
    let Some(agentfx_ir::Content::Paint { text: Some(fx), .. }) = &node(&s, "t").1.content else { panic!() };
    let agentfx_ir::TextSelector::Keys { units, .. } = &fx.animators[0].selector else { panic!() };
    assert_eq!(units.len(), 3);
    assert_eq!(units[2], vec![[1.1, 1.0], [1.35, 0.0]]);
    for (body, code) in [
        ("  rect #r w=10 h=10\n  @0s animate #r by=char opacity=0\n", "E_TARGET"),
        ("  text #t \"a b\" split=word\n  @0s animate #t by=char opacity=0\n", "E_TARGET"),
        ("  text #t \"a\"\n  @0s animate #t by=glyph opacity=0\n", "E_VALUE"),
        ("  text #t \"a\"\n  @0s animate #t shape=smoth opacity=0\n", "E_VALUE"),
        ("  text #t \"a\"\n  @0s animate #t offset=abc opacity=0\n", "E_VALUE"),
        ("  text #t \"a\"\n  @0s animate #t opacty=0\n", "W_ANIMATE_PROP"),
    ] {
        let c = build("", body, &dir);
        assert!(codes(&c).iter().any(|x| x == code), "{body}: {:?}", c.diags);
    }
    let c = build("", "  text #t \"a\"\n  @0s animate #t shape=smoth opacity=0\n", &dir);
    assert!(c.diags.iter().any(|d| d.hint.contains("smooth")));
}

#[test]
fn text_on_a_path_uses_the_path_geometry_and_offset_track() {
    let dir = scratch("onpath");
    let c = build("", "  path #p d=\"M0 50 C100 0 200 0 300 50\" w=300 h=100 x=20 y=100 stroke=#888 sw=2\n  text #t \"riding the curve\" size=30 on=#p offset=10\n    anim offset 0s:10 1s:200\n", &dir);
    no_errors(&c);
    let s = scene(&c, &dir);
    let (ti, t) = node(&s, "t");
    let Some(agentfx_ir::Content::Paint { text: Some(fx), .. }) = &t.content else { panic!("{:?}", t.content) };
    let p = fx.path.as_ref().unwrap();
    assert!(p.d.starts_with("M20.00 150.00"), "{}", p.d);
    assert_eq!(p.offset, 10.0);
    assert!(s.tracks.iter().any(|x| x.node == ti && x.property == "text.path-offset"));
}

#[test]
fn feature_example_compiles_clean() {
    let f = Path::new(env!("AGENTFX_ROOT")).join("examples/features/text-v2.reel");
    let (d, errors) = parse::parse(&std::fs::read_to_string(&f).unwrap());
    assert!(errors.is_empty());
    let (tl, _) = timing::solve(&d, &Default::default(), None);
    let c = lower::compile(&d, &tl, None, f.parent().unwrap());
    no_errors(&c);
    let s = scene(&c, &scratch("example"));
    let fx = s.nodes.iter().filter(|n| matches!(&n.content, Some(agentfx_ir::Content::Paint { text: Some(_), .. }))).count();
    assert_eq!(fx, 4, "wipe, words, keys, ride");
}
