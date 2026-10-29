//! S1 native layout + paint: the compiler owns layout (no Chrome).
//!
//! The motion IR's layout model (flow/abs/fill, flex/grid, padding, aspect, text props) is
//! written as a *static snapshot* of the HTML backend's page (`html::emit` with `Snap`: every
//! channel a literal value, no animation, no runtime) and resolved by Blitz (Stylo + Taffy +
//! Parley) with an explicitly registered font set (Inter variable with automatic optical
//! sizing, Noto Sans Mono). This is the same engine and the same patched crates the reel
//! renderer uses for its HTML mode, so both backends share one layout semantics by
//! construction, and the Chrome reference page uses the very same CSS.
//!
//! Per layer it provides: the scene-space box (anchor translates included), content box,
//! overflow, computed text metrics, line boxes (for `split=line`), and the layer's *own* paint
//! as a reel-ir display list (box, borders, shadows, rich text glyph runs, inline SVG, raw
//! HTML), via reel's subset painter. Layout/paint channels are re-resolved by restyling single
//! elements (`restyle`) and re-laying out incrementally, which is how the reel-ir backend turns
//! layout-affecting animation into tracks and switch states.
use crate::html::{self, HtmlOpts, Snap};
use crate::ir::*;
use crate::record::{Recorder, Resources};
use anyhow::{bail, Context, Result};
use blitz_dom::{DocumentConfig, NodeId};
use blitz_html::HtmlDocument;
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};
use blitz_traits::shell::{ColorScheme, Viewport};
use agentfx_ir::Draw;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Synchronous file:// provider (raw `html` blocks may reference local images).
struct FileNet {
    failed: Mutex<Vec<String>>,
}
impl NetProvider for FileNet {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.clone();
        if url.scheme() == "data" {
            // data:[mime][;base64],payload (percent-encoded otherwise)
            let s = url.as_str();
            if let Some((head, payload)) = s.split_once(',') {
                let bytes: Vec<u8> = if head.ends_with(";base64") {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.decode(payload).unwrap_or_default()
                } else {
                    percent_decode(payload)
                };
                handler.bytes(url.to_string(), Bytes::from(bytes));
                return;
            }
        }
        if let Ok(path) = url.to_file_path() {
            if let Ok(bytes) = std::fs::read(&path) {
                handler.bytes(url.to_string(), Bytes::from(bytes));
                return;
            }
        }
        self.failed.lock().unwrap().push(url.to_string());
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Deterministic fonts: exactly the files in `fonts_dir` (Inter variable, Noto Sans Mono),
/// no system lookup; generic families map to them.
fn font_ctx(fonts_dir: &Path, sources: &mut HashMap<u64, PathBuf>, custom: &[Font]) -> parley::FontContext {
    use parley::fontique::{Blob, Collection, CollectionOptions, FontInfoOverride, GenericFamily, SourceCache};
    let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    let mut files: Vec<PathBuf> = std::fs::read_dir(fonts_dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "ttf" || e == "otf")).collect())
        .unwrap_or_default();
    files.sort();
    for f in files {
        if let Ok(data) = std::fs::read(&f) {
            let blob = Blob::new(Arc::new(data));
            sources.insert(blob.id(), f.clone());
            collection.register_fonts(blob, None);
        }
    }
    // custom font files (`font "Brand" src=...`), registered under their alias; WOFF/WOFF2 are
    // decoded here (no source file: the recorder writes the SFNT next to the IR)
    for f in custom {
        let Some(file) = &f.file else { continue };
        let path = PathBuf::from(file);
        if let Ok(data) = crate::fonts::load(&path) {
            let sfnt = path.extension().is_some_and(|e| matches!(e.to_ascii_lowercase().to_str(), Some("ttf" | "otf" | "ttc")));
            let blob = Blob::new(Arc::new(data));
            if sfnt {
                sources.insert(blob.id(), path.clone());
            }
            collection.register_fonts(blob, Some(FontInfoOverride { family_name: Some(&f.family), ..Default::default() }));
        }
    }
    let sans = collection.family_id("Inter");
    let mono = collection.family_id("Noto Sans Mono");
    for (g, id) in [
        (GenericFamily::SansSerif, sans),
        (GenericFamily::SystemUi, sans),
        (GenericFamily::UiSansSerif, sans),
        (GenericFamily::Serif, sans),
        (GenericFamily::Monospace, mono),
        (GenericFamily::UiMonospace, mono),
    ] {
        if let Some(id) = id {
            collection.set_generic_families(g, std::iter::once(id));
        }
    }
    parley::FontContext { collection, source_cache: SourceCache::default() }
}

/// The deterministic font context, for other native readers (`harvest`).
/// Reel Script's layout mode on the renderer's patched Blitz (process-wide switches).
///
/// The vendored Blitz defaults to the renderer's Chrome-LayoutUnit mode: boxes rounded to
/// 1/64 px and pixel-snapped at paint (`LU_ROUND`), per-line text snapping (`LINE_SNAP`) and
/// fractional line tops in Parley. Reel Script's native layout (and every test and parity
/// number measured with it) uses whole-pixel box rounding, unsnapped text and rounded line
/// tops, with shrink-to-fit widths ceiled to 1/64 px (unconditional in the patched Blitz) and
/// automatic optical sizing for every font (`AUTO_OPSZ`; Reel Script registers all its fonts).
/// `AGENTFX_PARLEY_ROUND_LINES` is only set when the caller did not set it; it is read once per
/// process, so the first call should happen before any layout (`main` calls it first thing).
pub fn layout_mode() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        use std::sync::atomic::Ordering::Relaxed;
        blitz_dom::LU_ROUND.store(false, Relaxed);
        blitz_paint::LINE_SNAP.store(false, Relaxed);
        blitz_dom::AUTO_OPSZ.store(true, Relaxed);
        if agentfx_ir::env::var_os("AGENTFX_PARLEY_ROUND_LINES").is_none() {
            std::env::set_var("AGENTFX_PARLEY_ROUND_LINES", "1");
        }
    });
}

pub fn font_ctx_pub(fonts_dir: &Path, sources: &mut HashMap<u64, PathBuf>) -> parley::FontContext {
    font_ctx(fonts_dir, sources, &[])
}

fn resolve(doc: &mut HtmlDocument) {
    for _ in 0..50 {
        doc.handle_messages();
        doc.resolve(0.0);
        if !doc.has_pending_critical_resources() {
            break;
        }
    }
    doc.handle_messages();
    doc.resolve(0.0);
}

/// One laid-out line of a text layer (scene coords, anchors included).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Line {
    pub text: String,
    pub rect: [f64; 4],
    pub baseline: f64,
}

/// Snapshot values of every compiler-resolved (paint/layout) channel at time t.
pub fn snap_at(ir: &Ir, t: f64) -> Snap {
    let mut s = Snap::default();
    for tr in &ir.tracks {
        if matches!(tr.ch, Ch::Color | Ch::Fill | Ch::Stroke | Ch::Grad0 | Ch::Grad1 | Ch::Grad2 | Ch::Grad3 | Ch::GradAngle | Ch::Wght | Ch::Tracking | Ch::W | Ch::H | Ch::LineH | Ch::Count) || tr.ch.is_axis() {
            s.vals.insert((tr.layer.clone(), tr.ch), tr.eval(t));
        }
    }
    s
}

pub struct Native {
    pub doc: HtmlDocument,
    pub w: u32,
    pub h: u32,
    /// layer id -> Blitz node
    pub node: HashMap<String, NodeId>,
    is_layer: HashSet<NodeId>,
    pub res: Resources,
    /// layer id -> (parent layer id, anchor fractions if absolutely placed with an anchor)
    parent: HashMap<String, String>,
    anchor: HashMap<String, (f64, f64)>,
    parent_flow: HashMap<String, &'static str>,
    layers: HashMap<String, Layer>,
    chans: HashMap<String, std::collections::BTreeSet<Ch>>,
    snap: Snap,
    fonts_dir: PathBuf,
    /// current inline style per layer (to skip no-op restyles)
    cur_style: HashMap<String, String>,
    /// raw-html parts (`data-part`): the authored inline style that follows the layer style
    part_authored: HashMap<String, String>,
    pub stats: std::collections::BTreeMap<&'static str, f64>,
}

impl Native {
    pub fn load(ir: &Ir, snap: Snap, fonts_dir: &Path, out_dir: &Path, persist: bool) -> Result<Native> {
        let t0 = std::time::Instant::now();
        layout_mode();
        let opts = HtmlOpts { embed: false, fonts_dir: fonts_dir.to_path_buf(), snap: Some(snap.clone()) };
        let page = html::emit(ir, &opts);
        let mut res = Resources::new(out_dir);
        res.persist = persist;
        let mut sources = HashMap::new();
        let fctx = font_ctx(fonts_dir, &mut sources, &ir.fonts);
        res.font_sources = sources;
        let base = url::Url::from_directory_path(out_dir.canonicalize().unwrap_or(out_dir.to_path_buf())).map(|u| u.to_string()).unwrap_or("file:///".into());
        let net = Arc::new(FileNet { failed: Mutex::new(vec![]) });
        let mut doc = HtmlDocument::from_html(
            &page,
            DocumentConfig {
                base_url: Some(base),
                viewport: Some(Viewport::new(ir.w, ir.h, 1.0, ColorScheme::Dark)),
                net_provider: Some(net.clone()),
                font_ctx: Some(fctx),
                ..Default::default()
            },
        );
        resolve(&mut doc);
        let mut node = HashMap::new();
        let mut is_layer = HashSet::new();
        let mut stack = vec![doc.root_node().id];
        while let Some(n) = stack.pop() {
            let nd = doc.get_node(n).unwrap();
            if let Some(el) = nd.element_data() {
                // decoded <img> blobs -> their source files (referenced in place, never re-encoded)
                if let (Some(img), Some(src)) = (el.raster_image_data(), el.attr(blitz_dom::local_name!("src"))) {
                    if let Some(p) = url::Url::parse(src).ok().and_then(|u| u.to_file_path().ok()) {
                        res.image_sources.insert(img.data.id(), p);
                    }
                }
                // CSS background images (mesh rasters, image fills): same, by their url
                for bg in el.background_images.iter().flatten() {
                    if let blitz_dom::node::ImageData::Raster(img) = &bg.image {
                        // PNG only: the renderer decodes PNG; other formats are re-encoded
                        if let Some(p) = bg.url.to_file_path().ok().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))) {
                            res.image_sources.insert(img.data.id(), p);
                        }
                    }
                }
                let cls = el.attr(blitz_dom::local_name!("class")).unwrap_or("");
                if cls.split(' ').any(|c| c == "L") {
                    if let Some(id) = el.attr(blitz_dom::local_name!("id")) {
                        node.insert(id.to_string(), n);
                        is_layer.insert(n);
                    }
                }
            }
            stack.extend(nd.children.iter().copied());
        }
        let mut parent = HashMap::new();
        let mut anchor = HashMap::new();
        let mut parent_flow = HashMap::new();
        let mut layers = HashMap::new();
        fn rec(l: &Layer, pf: &'static str, par: Option<&str>, parent: &mut HashMap<String, String>, anchor: &mut HashMap<String, (f64, f64)>, parent_flow: &mut HashMap<String, &'static str>, layers: &mut HashMap<String, Layer>) {
            if let Some(p) = par {
                parent.insert(l.id.clone(), p.to_string());
            }
            if l.layout.pos == Pos::Abs && (l.layout.anchor.0 != 0.0 || l.layout.anchor.1 != 0.0) {
                anchor.insert(l.id.clone(), l.layout.anchor);
            }
            parent_flow.insert(l.id.clone(), pf);
            let mut shallow = l.clone();
            shallow.children.clear();
            layers.insert(l.id.clone(), shallow);
            for c in &l.children {
                rec(c, l.layout.flow, Some(&l.id), parent, anchor, parent_flow, layers);
            }
        }
        rec(&ir.root, "none", None, &mut parent, &mut anchor, &mut parent_flow, &mut layers);
        for id in layers.keys() {
            if !node.contains_key(id) {
                bail!("native layout: layer `{id}` missing from the snapshot page");
            }
        }
        let mut chans: HashMap<String, std::collections::BTreeSet<Ch>> = HashMap::new();
        for t in &ir.tracks {
            chans.entry(t.layer.clone()).or_default().insert(t.ch);
        }
        let mut s = Native { doc, w: ir.w, h: ir.h, node, is_layer, res, parent, anchor, parent_flow, layers, chans, snap, fonts_dir: fonts_dir.to_path_buf(), cur_style: HashMap::new(), part_authored: HashMap::new(), stats: Default::default() };
        // parts keep their authored style: it is what follows the injected layer style
        let ids: Vec<String> = s.layers.iter().filter(|(_, l)| l.tags.iter().any(|t| t == "part")).map(|(k, _)| k.clone()).collect();
        for id in ids {
            let l = &s.layers[&id];
            let injected = html::layer_style(l, "none", &opts, &s.chans.get(&id).cloned().unwrap_or_default());
            let injected = injected.trim_start_matches("position:relative;").to_string();
            let attr = s.doc.get_node(s.node[&id]).and_then(|n| n.element_data()).and_then(|e| e.attr(blitz_dom::local_name!("style")).map(|v| v.to_string())).unwrap_or_default();
            let authored = attr.strip_prefix(injected.as_str()).unwrap_or("").to_string();
            s.part_authored.insert(id, authored);
        }
        s.stats.insert("load_s", t0.elapsed().as_secs_f64());
        let failed = net.failed.lock().unwrap().clone();
        if !failed.is_empty() {
            eprintln!("native: could not load {}", failed.join(", "));
        }
        Ok(s)
    }

    pub fn has(&self, id: &str) -> bool {
        self.node.contains_key(id)
    }

    fn layout_rect(&self, n: NodeId) -> [f64; 4] {
        let nd = self.doc.get_node(n).unwrap();
        if matches!(nd.taffy_display(), taffy::Display::None) {
            return [0.0; 4];
        }
        let p = nd.absolute_position(0.0, 0.0);
        let l = nd.final_layout();
        [p.x as f64, p.y as f64, l.size.width as f64, l.size.height as f64]
    }

    /// Sum of anchor translates (`translate(-ax%, -ay%)`) of the layer and its ancestors.
    pub fn anchor_offset(&self, id: &str) -> (f64, f64) {
        let mut off = (0.0, 0.0);
        let mut cur = Some(id.to_string());
        while let Some(c) = cur {
            if let (Some(a), Some(&n)) = (self.anchor.get(&c), self.node.get(&c)) {
                let r = self.layout_rect(n);
                off.0 -= a.0 * r[2];
                off.1 -= a.1 * r[3];
            }
            cur = self.parent.get(&c).cloned();
        }
        off
    }

    /// Border box in scene coordinates at rest (all composite channels at identity).
    pub fn rect(&self, id: &str) -> Option<[f64; 4]> {
        let n = *self.node.get(id)?;
        let r = self.layout_rect(n);
        let o = self.anchor_offset(id);
        Some([r[0] + o.0, r[1] + o.1, r[2], r[3]])
    }

    /// Content box (inside border + padding), scene coords.
    pub fn content_rect(&self, id: &str) -> Option<[f64; 4]> {
        let r = self.rect(id)?;
        let l = self.doc.get_node(self.node[id]).unwrap().final_layout();
        let (b, p) = (l.border, l.padding);
        Some([
            r[0] + (b.left + p.left) as f64,
            r[1] + (b.top + p.top) as f64,
            (r[2] - (b.left + b.right + p.left + p.right) as f64).max(0.0),
            (r[3] - (b.top + b.bottom + p.top + p.bottom) as f64).max(0.0),
        ])
    }

    /// Overflow (scrollWidth - clientWidth, scrollHeight - clientHeight) like the Chrome measure.
    pub fn overflow(&self, id: &str) -> (f64, f64) {
        let Some(&n) = self.node.get(id) else { return (0.0, 0.0) };
        let l = self.doc.get_node(n).unwrap().final_layout();
        (l.scroll_width() as f64, l.scroll_height() as f64)
    }

    /// Computed font size, color (straight RGBA 0..1) and whether white-space is nowrap/pre.
    pub fn text_style(&self, id: &str) -> Option<(f64, [f64; 4], bool)> {
        let n = *self.node.get(id)?;
        let st = self.doc.get_node(n)?.primary_styles()?;
        let fs = st.get_font().font_size.computed_size().px() as f64;
        let c = st.get_inherited_text().color.clone();
        let abs = c.to_color_space(style::color::ColorSpace::Srgb);
        let comps = abs.raw_components();
        let col = [comps[0] as f64, comps[1] as f64, comps[2] as f64, abs.alpha as f64];
        use style::computed_values::white_space_collapse::T as Wsc;
        let nowrap = !matches!(st.get_inherited_text().text_wrap_mode, style::computed_values::text_wrap_mode::T::Wrap) || matches!(st.get_inherited_text().white_space_collapse, Wsc::Preserve);
        Some((fs, col, nowrap))
    }

    /// Nodes painted as the layer's own content: the element and every descendant that is not
    /// itself a layer (or inside one), anonymous block wrappers included.
    fn own_set(&self, n: NodeId, include_root: bool) -> HashSet<NodeId> {
        let mut set = HashSet::new();
        let mut stack = vec![n];
        while let Some(c) = stack.pop() {
            if c != n && self.is_layer.contains(&c) {
                continue;
            }
            if c != n || include_root {
                set.insert(c);
            }
            if let Some(nd) = self.doc.get_node(c) {
                stack.extend(nd.children.iter().copied());
                if let Some(lc) = nd.layout_children.borrow().as_ref() {
                    stack.extend(lc.iter().copied());
                }
            }
        }
        set
    }

    /// The layer's own paint (no child layers) as reel-ir draw items, scene coordinates.
    /// `skip`: descendant tags that are drawn natively elsewhere (`img` of image layers).
    pub fn paint(&mut self, id: &str, skip_img: bool) -> Result<Vec<Draw>> {
        let t = std::time::Instant::now();
        let n = *self.node.get(id).context("paint: unknown layer")?;
        let mut own = self.own_set(n, true);
        if skip_img {
            own.retain(|&c| self.doc.get_node(c).and_then(|x| x.element_data()).is_none_or(|e| &*e.name.local != "img"));
        }
        let own_f = |c: NodeId| own.contains(&c);
        let filter = blitz_paint::PaintFilter { own: &own_f, effects: Some(&own_f), canvas_background: false, observe: None };
        let mut rec = Recorder::new(&mut self.res);
        blitz_paint::paint_scene_filtered(&mut rec, &mut self.doc, 1.0, self.w, self.h, 0, 0, Some(&filter));
        if !rec.errors.is_empty() {
            let mut e = rec.errors.clone();
            e.dedup();
            bail!("layer `{id}`: unsupported paint: {}", e.join("; "));
        }
        let items = std::mem::take(&mut rec.items);
        *self.stats.entry("paint_s").or_default() += t.elapsed().as_secs_f64();
        Ok(items)
    }

    /// Re-resolve layout/paint channels: rewrite the style of every layer whose snapshot
    /// values differ from the current ones, then lay out again (incremental).
    pub fn set_snap(&mut self, snap: &Snap, layers: &[String]) {
        let t = std::time::Instant::now();
        self.snap = snap.clone();
        let opts = HtmlOpts { embed: false, fonts_dir: self.fonts_dir.clone(), snap: Some(snap.clone()) };
        let mut changed = false;
        for id in layers {
            let (Some(l), Some(&n)) = (self.layers.get(id), self.node.get(id)) else { continue };
            let mut style = html::layer_style(l, self.parent_flow.get(id).copied().unwrap_or("none"), &opts, &self.chans.get(id).cloned().unwrap_or_default());
            if let Some(auth) = self.part_authored.get(id) {
                // same shape as html::inject_part: layer style first, authored declarations after
                style = format!("{}{auth}", style.trim_start_matches("position:relative;"));
            }
            let count = snap.vals.get(&(id.clone(), Ch::Count)).map(|v| v.n().round() as i64);
            // the change key includes the counter value (it is text, not style)
            let key = format!("{style}|{count:?}");
            if self.cur_style.get(id).is_some_and(|s| *s == key) {
                continue;
            }
            let mut m = self.doc.mutate();
            m.set_attribute(n, blitz_dom::QualName::new(None, blitz_dom::ns!(), blitz_dom::local_name!("style")), &style);
            if let Kind::Counter { prefix, suffix } = &l.kind {
                let v = snap.vals.get(&(id.clone(), Ch::Count)).map(|v| v.n().round() as i64).unwrap_or(0);
                let kids: Vec<NodeId> = m.doc.get_node(n).unwrap().children.to_vec();
                for k in kids {
                    m.remove_and_drop_node(k);
                }
                let tn = m.create_text_node(&format!("{prefix}{}{suffix}", crate::ir::counter_text(l, v)));
                m.append_children(n, &[tn]);
            }
            drop(m);
            style.clear();
            self.cur_style.insert(id.clone(), key);
            changed = true;
        }
        if changed {
            resolve(&mut self.doc);
        }
        *self.stats.entry("relayout_s").or_default() += t.elapsed().as_secs_f64();
    }

    pub fn snap(&self) -> &Snap {
        &self.snap
    }

    /// Line boxes of a text layer (its inline formatting context), scene coords.
    pub fn lines(&self, id: &str) -> Vec<Line> {
        let Some(&n) = self.node.get(id) else { return vec![] };
        let nd = self.doc.get_node(n).unwrap();
        // the inline root is the element itself or its anonymous block child
        let mut cands = vec![n];
        if let Some(lc) = nd.layout_children.borrow().as_ref() {
            cands.extend(lc.iter().copied());
        }
        let off = self.anchor_offset(id);
        for c in cands {
            let cn = self.doc.get_node(c).unwrap();
            let Some(el) = cn.element_data() else { continue };
            let Some(tl) = el.inline_layout_data.as_ref() else { continue };
            let p = cn.absolute_position(0.0, 0.0);
            let l = cn.final_layout();
            let ox = p.x as f64 + (l.border.left + l.padding.left) as f64 + off.0;
            let oy = p.y as f64 + (l.border.top + l.padding.top) as f64 + off.1;
            let mut out = vec![];
            for line in tl.layout.lines() {
                let m = line.metrics();
                let range = line.text_range();
                let text = tl.text.get(range.clone()).unwrap_or("").to_string();
                let mut x0 = f64::MAX;
                let mut x1 = f64::MIN;
                for item in line.items() {
                    if let parley::PositionedLayoutItem::GlyphRun(gr) = item {
                        x0 = x0.min(gr.offset() as f64);
                        x1 = x1.max((gr.offset() + gr.advance()) as f64);
                    }
                }
                if x0 > x1 {
                    x0 = 0.0;
                    x1 = 0.0;
                }
                out.push(Line { text, rect: [ox + x0, oy + m.block_min_coord as f64, x1 - x0, (m.block_max_coord - m.block_min_coord) as f64], baseline: oy + m.baseline as f64 });
            }
            return out;
        }
        vec![]
    }
}
