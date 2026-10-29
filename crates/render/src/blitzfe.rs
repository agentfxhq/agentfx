//! Native HTML frontend on Blitz (Stylo + Taffy + Parley), replacing Chrome in the
//! render path (M1):
//! - loads the scene with a deterministic font set and file:// resources,
//! - extracts the frame-0 geometry the planner needs (was `ref/geom.mjs`),
//! - paints each paint unit (an element subset) natively into an IR display list
//!   (was `ref/raster.mjs`), including repaint states (paint/layout tracks evaluated
//!   at the state's time and applied as inline styles).
//!
//! Element identity lives here: ElemId (document order, = `querySelectorAll('*')`)
//! maps to a Blitz node id; the patched painter paints only a unit's elements and
//! applies only the effects inside the unit's layer.
use crate::cssval;
use crate::html::{Geom, GeomElem, HtmlOut, Job};
use crate::model::{Compiled, ElemId, Stage};
use crate::record::{self, Recorder, Resources};
use anyhow::{Context, Result, bail};
use blitz_dom::{DocumentConfig, NodeId};
use blitz_html::HtmlDocument;
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};
use blitz_traits::shell::{ColorScheme, Viewport};
use agentfx_ir::eval::{eval_segment, eval_track};
use agentfx_ir::{Content, Draw, Sampled, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const W: u32 = 1920;
const H: u32 = 1080;
const INJECTED: &str = "<style data-reel-injected>*, *::before, *::after { animation: none !important; transition: none !important; }</style>";

/// Synchronous file:// provider; remembers which files were fetched.
struct FileNet {
    fetched: Mutex<Vec<(String, PathBuf)>>,
}
impl NetProvider for FileNet {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.clone();
        if let Ok(path) = url.to_file_path() {
            if let Ok(bytes) = std::fs::read(&path) {
                self.fetched.lock().unwrap().push((url.to_string(), path));
                handler.bytes(url.to_string(), Bytes::from(bytes));
                return;
            }
        }
        eprintln!("agentfx-render: fetch failed: {url}");
    }
}

/// Deterministic fonts: files in `<scene>/fonts`, `<scene>/../_shared`, plus a fixed
/// system set: `~/.local/share/fonts`, Liberation, Noto (glyph fallback).
/// Generic families follow Chrome on Linux (measured with CDP getPlatformFontsForNode):
/// sans-serif -> Liberation Sans (Arial), serif -> Liberation Serif, monospace ->
/// Liberation Mono, system-ui -> Nimbus Sans if present; `ui-*` and unknown families
/// fall through to the next family in the list. No fontconfig lookup.
pub(crate) fn font_ctx(scene_dir: &Path) -> parley::FontContext {
    use parley::fontique::{Blob, Collection, CollectionOptions, GenericFamily, SourceCache};
    let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    let mut dirs = vec![scene_dir.join("fonts"), scene_dir.join("../_shared")];
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(h).join(".local/share/fonts"));
    }
    dirs.push(PathBuf::from("/usr/share/fonts/truetype/liberation"));
    dirs.push(PathBuf::from("/usr/share/fonts/truetype/noto"));
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "ttf" || e == "otf")).collect();
        files.sort();
        for f in files {
            if let Ok(data) = std::fs::read(&f) {
                collection.register_fonts(Blob::new(Arc::new(data)), None);
            }
        }
    }
    let nimbus = PathBuf::from("/usr/share/fonts/opentype/urw-base35/NimbusSans-Regular.otf");
    for f in ["NimbusSans-Regular.otf", "NimbusSans-Bold.otf"] {
        if let Ok(data) = std::fs::read(nimbus.with_file_name(f)) {
            collection.register_fonts(Blob::new(Arc::new(data)), None);
        }
    }
    let pick = |c: &mut Collection, names: &[&str]| names.iter().find_map(|n| c.family_id(n));
    let sans = pick(&mut collection, &["Liberation Sans", "Inter"]);
    let serif = pick(&mut collection, &["Liberation Serif"]);
    let mono = pick(&mut collection, &["Liberation Mono"]);
    let sys = pick(&mut collection, &["Nimbus Sans", "Liberation Sans"]);
    for (g, id) in [(GenericFamily::SansSerif, sans), (GenericFamily::SystemUi, sys), (GenericFamily::Serif, serif), (GenericFamily::Monospace, mono)] {
        if let Some(id) = id {
            collection.set_generic_families(g, std::iter::once(id));
        }
    }
    parley::FontContext { collection, source_cache: SourceCache::default() }
}

pub struct BlitzScene {
    pub doc: HtmlDocument,
    /// ElemId -> Blitz node id
    pub elems: Vec<NodeId>,
    pub node_elem: HashMap<NodeId, ElemId>,
    /// text-track element -> its (single) text node
    text_nodes: HashMap<ElemId, NodeId>,
    /// inline style properties currently set by reel: (elem, property)
    applied: HashSet<(ElemId, String)>,
    /// value currently set per (elem, property) (skip unchanged writes)
    applied_vals: HashMap<(ElemId, String), String>,
    applied_text: HashMap<ElemId, String>,
    /// authored inline declarations (restored when reel removes an override)
    inline: HashMap<(ElemId, String), String>,
    pub timings: BTreeMap<&'static str, f64>,
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

impl BlitzScene {
    pub fn load(scene_dir: &Path, compiled: &Compiled) -> Result<BlitzScene> {
        let t0 = std::time::Instant::now();
        let index = std::fs::canonicalize(scene_dir.join("index.html")).context("index.html")?;
        let mut html = std::fs::read_to_string(&index)?;
        // Chrome applies automatic optical sizing to web fonts only (blitz-dom patch)
        let mut web_fonts = html.contains("@font-face");
        for part in html.split("href=\"").skip(1) {
            if let Some(end) = part.find('"') {
                let href = &part[..end];
                if href.ends_with(".css") {
                    web_fonts |= std::fs::read_to_string(index.parent().unwrap().join(href)).is_ok_and(|c| c.contains("@font-face"));
                }
            }
        }
        let web_fonts = match agentfx_ir::env::var("AGENTFX_OPSZ").as_deref() { Ok("1") => true, Ok("0") => false, _ => web_fonts };
        blitz_dom::AUTO_OPSZ.store(web_fonts, std::sync::atomic::Ordering::Relaxed);
        // M4d: layout on Chrome's LayoutUnit grid (1/64 px) is the default; AGENTFX_LAYOUT_ROUND=px
        // (any value but "lu") restores Taffy's whole-px rounding (M3..M4c behaviour).
        let lu = agentfx_ir::env::var("AGENTFX_LAYOUT_ROUND").map_or(true, |v| v == "lu");
        blitz_dom::LU_ROUND.store(lu, std::sync::atomic::Ordering::Relaxed);
        // M4d: Chrome's per-line text y snapping (line box top rounded in the nearest transformed
        // ancestor's space); on with `lu` layout, AGENTFX_LINE_SNAP=0/1 overrides. Baked into the IR.
        let line_snap = agentfx_ir::env::var("AGENTFX_LINE_SNAP").map_or(lu, |v| v != "0");
        blitz_paint::LINE_SNAP.store(line_snap, std::sync::atomic::Ordering::Relaxed);
        let at = html.find("</head>").context("scene needs a </head>")?;
        html.insert_str(at, INJECTED);
        let base = url::Url::from_file_path(&index).unwrap().to_string();
        let net = Arc::new(FileNet { fetched: Mutex::new(vec![]) });
        let mut doc = HtmlDocument::from_html(
            &html,
            DocumentConfig {
                base_url: Some(base),
                viewport: Some(Viewport::new(W, H, 1.0, ColorScheme::Light)),
                net_provider: Some(net.clone()),
                font_ctx: Some(font_ctx(index.parent().unwrap())),
                ..Default::default()
            },
        );
        resolve(&mut doc);
        // elements in document order, minus the injected style
        let mut elems = Vec::new();
        let mut stack = vec![doc.root_node().id];
        while let Some(n) = stack.pop() {
            let node = doc.get_node(n).unwrap();
            if let Some(el) = node.element_data() {
                if !el.attrs().iter().any(|a| &*a.name.local == "data-reel-injected") {
                    elems.push(n);
                }
            }
            for &c in node.children.iter().rev() {
                stack.push(c);
            }
        }
        if elems.len() != compiled.tags.len() {
            bail!("Blitz DOM has {} elements, CSS compiler {}", elems.len(), compiled.tags.len());
        }
        for (i, (&n, t)) in elems.iter().zip(&compiled.tags).enumerate() {
            let name = doc.get_node(n).unwrap().element_data().unwrap().name.local.to_string();
            if !name.eq_ignore_ascii_case(t) {
                bail!("element {i}: Blitz <{name}> vs compiler <{t}>");
            }
        }
        let node_elem = elems.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        // text tracks: one text node per element
        let mut text_nodes = HashMap::new();
        for tt in &compiled.texts {
            let n = elems[tt.elem];
            let kids: Vec<NodeId> = doc.get_node(n).unwrap().children.to_vec();
            if !kids.iter().all(|&k| doc.get_node(k).unwrap().text_data().is_some()) {
                bail!("text track element #{} must contain only text", tt.elem);
            }
            let mut m = doc.mutate();
            for &k in &kids {
                m.remove_and_drop_node(k);
            }
            let tn = m.create_text_node("");
            m.append_children(n, &[tn]);
            drop(m);
            text_nodes.insert(tt.elem, tn);
        }
        let mut inline = HashMap::new();
        for (e, &n) in elems.iter().enumerate() {
            let el = doc.get_node(n).unwrap().element_data().unwrap();
            if let Some(a) = el.attrs().iter().find(|a| &*a.name.local == "style") {
                for decl in a.value.split(';') {
                    if let Some((k, v)) = decl.split_once(':') {
                        inline.insert((e, k.trim().to_ascii_lowercase()), v.trim().to_string());
                    }
                }
            }
        }
        let mut s = BlitzScene { doc, elems, node_elem, text_nodes, applied: HashSet::new(), applied_vals: HashMap::new(), applied_text: HashMap::new(), inline, timings: BTreeMap::new() };
        s.timings.insert("load_s", t0.elapsed().as_secs_f64());
        Ok(s)
    }

    /// Map decoded `<img>` blobs to their source files (avoids re-encoding them).
    pub fn image_sources(&self, res: &mut Resources) {
        for &n in &self.elems {
            let node = self.doc.get_node(n).unwrap();
            let Some(el) = node.element_data() else { continue };
            if let (Some(img), Some(src)) = (el.raster_image_data(), el.attr(blitz_dom::local_name!("src"))) {
                if let Some(p) = self.doc.url().join(src).ok().and_then(|u| u.to_file_path().ok()) {
                    res.image_sources.insert(img.data.id(), p);
                }
            }
        }
    }

    pub fn set_text(&mut self, elem: ElemId, text: &str) {
        if self.applied_text.get(&elem).is_some_and(|t| t == text) {
            return;
        }
        let tn = self.text_nodes[&elem];
        self.doc.mutate().set_node_text(tn, text);
        self.applied_text.insert(elem, text.to_string());
    }

    /// Replace the reel-set inline style properties with `props` (elem, property, value).
    /// `strict`: fail if an override hits an authored inline `!important` declaration
    /// (inline setters cannot express priority, so it could neither be overridden nor
    /// restored faithfully). Non-strict (ink pass) leaves such properties alone.
    pub fn set_props(&mut self, props: &[(ElemId, String, String)], strict: bool) -> Result<()> {
        let mut kept = Vec::new();
        for (e, p, v) in props {
            if self.inline.get(&(*e, p.clone())).is_some_and(|o| o.to_ascii_lowercase().contains("!important")) {
                if strict {
                    bail!("element #{e}: animated {p} overrides an inline !important declaration (unsupported)");
                }
                continue;
            }
            kept.push((*e, p.clone(), v.clone()));
        }
        let props = &kept[..];
        let want: HashSet<(ElemId, String)> = props.iter().map(|(e, p, _)| (*e, p.clone())).collect();
        let stale: Vec<(ElemId, String)> = self.applied.difference(&want).cloned().collect();
        let mut m = self.doc.mutate();
        for (e, p) in &stale {
            self.applied_vals.remove(&(*e, p.clone()));
            match self.inline.get(&(*e, p.clone())) {
                Some(orig) => m.set_style_property(self.elems[*e], p, orig),
                None => m.remove_style_property(self.elems[*e], p),
            }
        }
        for (e, p, v) in props {
            let k = (*e, p.clone());
            if self.applied_vals.get(&k) == Some(v) {
                continue;
            }
            m.set_style_property(self.elems[*e], p, v);
            self.applied_vals.insert(k, v.clone());
        }
        drop(m);
        self.applied = want;
        Ok(())
    }

    pub fn resolve(&mut self) {
        let t = std::time::Instant::now();
        resolve(&mut self.doc);
        *self.timings.entry("layout_s").or_default() += t.elapsed().as_secs_f64();
    }

    /// Paint the elements `own` (the rest only contributes clips/opacity/filters, and only
    /// inside `layer_root`'s subtree when given) into an IR display list.
    pub fn paint(&mut self, own: &HashSet<NodeId>, layer_root: Option<ElemId>, canvas: bool, res: &mut Resources) -> Result<Vec<Draw>> {
        let t = std::time::Instant::now();
        let inside: Option<HashSet<NodeId>> = layer_root.map(|r| {
            let mut set = HashSet::new();
            let mut stack = vec![self.elems[r]];
            while let Some(n) = stack.pop() {
                set.insert(n);
                if let Some(node) = self.doc.get_node(n) {
                    stack.extend(node.children.iter().copied());
                    // anonymous blocks / pseudo boxes live in layout_children
                    if let Some(lc) = node.layout_children.borrow().as_ref() {
                        stack.extend(lc.iter().copied());
                    }
                }
            }
            set
        });
        let own_f = |n: NodeId| own.contains(&n);
        let fx_f = |n: NodeId| inside.as_ref().is_none_or(|s| s.contains(&n));
        let filter = blitz_paint::PaintFilter { own: &own_f, effects: Some(&fx_f), canvas_background: canvas, observe: None };
        let mut rec = Recorder::new(res);
        blitz_paint::paint_scene_filtered(&mut rec, &mut self.doc, 1.0, W, H, 0, 0, Some(&filter));
        if !rec.errors.is_empty() {
            let mut e = rec.errors.clone();
            e.dedup();
            bail!("unsupported paint:\n  - {}", e.join("\n  - "));
        }
        let items = std::mem::take(&mut rec.items);
        *self.timings.entry("paint_s").or_default() += t.elapsed().as_secs_f64();
        Ok(items)
    }

    fn own_set(&self, elems: &[ElemId]) -> HashSet<NodeId> {
        // an element's own paint includes its anonymous block children (inline text wrappers)
        let mut set = HashSet::new();
        for &e in elems {
            let n = self.elems[e];
            set.insert(n);
            if let Some(lc) = self.doc.get_node(n).unwrap().layout_children.borrow().as_ref() {
                for &c in lc.iter() {
                    if !self.node_elem.contains_key(&c) && self.doc.get_node(c).is_some_and(|x| x.element_data().is_none_or(|_| matches!(x.data, blitz_dom::node::NodeData::AnonymousBlock(_)))) {
                        set.insert(c);
                    }
                }
            }
        }
        set
    }

    /// Frame-0 geometry (base layout, animations off, text tracks at t = 0), the input of
    /// the planner in html.rs. Ink = bounds of each element's own native paint (opacity
    /// forced to 1, clips ignored), padded by 1 px.
    pub fn geom(&mut self, compiled: &Compiled, res: &mut Resources) -> Result<Geom> {
        for tt in &compiled.texts {
            let tr = compiled.tracks.iter().find(|t| t.elem == tt.elem && t.property == "text").unwrap();
            let n = match eval_track(&tr.to_ir(0), 0.0) {
                Some(Sampled::Number(v)) => (v + 1e-9).floor().max(0.0) as usize,
                _ => 0,
            };
            let s: String = tt.text.chars().take(n).collect();
            self.set_text(tt.elem, &s);
        }
        self.resolve();
        let mut out = Vec::with_capacity(self.elems.len());
        for e in 0..self.elems.len() {
            out.push(self.geom_elem(e));
        }
        // ink pass: every element at opacity 1
        let all: Vec<(ElemId, String, String)> = (0..self.elems.len()).map(|e| (e, "opacity".to_string(), "1".to_string())).collect();
        self.set_props(&all, false)?;
        self.resolve();
        let fonts = res.font_data.clone();
        for (e, ge) in out.iter_mut().enumerate() {
            if ge.display == "none" {
                continue;
            }
            let own = self.own_set(&[e]);
            let items = self.paint(&own, None, e == 0, res)?;
            let fonts = &res.font_data;
            let fl = |f: usize| fonts.get(f).cloned();
            let b = record::bounds(&items, &fl, false);
            if b[2] > 0.0 && b[3] > 0.0 {
                ge.ink = Some([b[0] - 1.0, b[1] - 1.0, b[2] + 2.0, b[3] + 2.0]);
            }
        }
        let _ = fonts;
        self.set_props(&[], false)?;
        self.resolve();
        Ok(Geom { elements: out })
    }

    /// Paint the whole document without culling, attributing every display-list item to
    /// the element that issued it. Returns items, per-item owner (ElemId; usize::MAX = the
    /// canvas) and per-element placement (parent transform, box position) of painted elements.
    pub fn paint_probe(&mut self, res: &mut Resources) -> Result<(Vec<Draw>, Vec<usize>, HashMap<ElemId, (kurbo::Affine, kurbo::Vec2)>)> {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        let t = std::time::Instant::now();
        let counter = Rc::new(Cell::new(0usize));
        let events: RefCell<Vec<(NodeId, bool, usize)>> = RefCell::new(Vec::new());
        let place: RefCell<HashMap<ElemId, (kurbo::Affine, kurbo::Vec2)>> = RefCell::new(HashMap::new());
        let node_elem = &self.node_elem;
        let c2 = counter.clone();
        let obs = |n: NodeId, begin: bool, ptf: kurbo::Affine, bp: kurbo::Vec2| {
            if let Some(&e) = node_elem.get(&n) {
                events.borrow_mut().push((n, begin, c2.get()));
                if begin {
                    place.borrow_mut().insert(e, (ptf, bp));
                }
            }
        };
        let all = |_: NodeId| true;
        let filter = blitz_paint::PaintFilter { own: &all, effects: None, canvas_background: true, observe: Some(&obs) };
        let mut rec = Recorder::new(res);
        rec.counter = Some(counter.clone());
        blitz_paint::paint_scene_filtered(&mut rec, &mut self.doc, 1.0, W, H, 0, 0, Some(&filter));
        if !rec.errors.is_empty() {
            let mut e = rec.errors.clone();
            e.dedup();
            bail!("unsupported paint:\n  - {}", e.join("\n  - "));
        }
        let items = std::mem::take(&mut rec.items);
        drop(rec);
        let events = events.into_inner();
        let mut owner = vec![usize::MAX; items.len()];
        let mut stack: Vec<ElemId> = Vec::new();
        let mut ev = events.iter().peekable();
        for (i, o) in owner.iter_mut().enumerate() {
            while let Some(&&(n, begin, at)) = ev.peek() {
                if at > i {
                    break;
                }
                let e = self.node_elem[&n];
                if begin {
                    stack.push(e);
                } else {
                    stack.pop();
                }
                ev.next();
            }
            *o = stack.last().copied().unwrap_or(usize::MAX);
        }
        *self.timings.entry("paint_s").or_default() += t.elapsed().as_secs_f64();
        Ok((items, owner, place.into_inner()))
    }

    /// Static (unanimated) composite values of element `e` in the current state:
    /// translate (px), rotate (deg), scale, opacity, and whether `transform` is non-none.
    pub fn composite_base(&self, e: ElemId) -> Option<([f64; 2], f64, [f64; 2], f64, bool)> {
        use style::values::generics::transform::{Rotate, Scale, Translate};
        let node = self.doc.get_node(self.elems[e])?;
        let st = node.primary_styles()?;
        let bx = st.get_box();
        let l = node.final_layout();
        let (w, h) = (l.size.width, l.size.height);
        let tr = match &bx.translate {
            Translate::None => [0.0, 0.0],
            Translate::Translate(x, y, _) => [
                x.resolve(style::values::computed::CSSPixelLength::new(w)).px() as f64,
                y.resolve(style::values::computed::CSSPixelLength::new(h)).px() as f64,
            ],
        };
        let rot = match &bx.rotate {
            Rotate::None => 0.0,
            Rotate::Rotate(a) => a.degrees() as f64,
            Rotate::Rotate3D(_, _, z, a) if *z > 0.0 => a.degrees() as f64,
            Rotate::Rotate3D(..) => return None,
        };
        let sc = match &bx.scale {
            Scale::None => [1.0, 1.0],
            Scale::Scale(x, y, _) => [*x as f64, *y as f64],
        };
        Some((tr, rot, sc, st.get_effects().opacity as f64, !bx.transform.0.is_empty()))
    }

    /// Attribute value of element `e`.
    pub fn attr(&self, e: ElemId, name: &str) -> Option<String> {
        let el = self.doc.get_node(self.elems[e])?.element_data()?;
        el.attrs().iter().find(|a| &*a.name.local == name).map(|a| a.value.to_string())
    }

    /// Border-box size of element `e` (layout px).
    pub fn box_size(&self, e: ElemId) -> (f64, f64) {
        let l = self.doc.get_node(self.elems[e]).unwrap().final_layout();
        (l.size.width as f64, l.size.height as f64)
    }

    /// Unfiltered paint of the whole document (as a browser would draw it).
    pub fn paint_all(&mut self, res: &mut Resources) -> Result<Vec<Draw>> {
        let mut rec = Recorder::new(res);
        blitz_paint::paint_scene(&mut rec, &mut self.doc, 1.0, W, H, 0, 0);
        if !rec.errors.is_empty() {
            let mut e = rec.errors.clone();
            e.dedup();
            bail!("unsupported paint:\n  - {}", e.join("\n  - "));
        }
        Ok(std::mem::take(&mut rec.items))
    }

    pub fn geom_elem(&self, e: ElemId) -> GeomElem {
        use style::values::computed::Overflow;
        let node = self.doc.get_node(self.elems[e]).unwrap();
        let tag = node.element_data().unwrap().name.local.to_string();
        let styles = node.primary_styles();
        let hidden = matches!(node.taffy_display(), taffy::Display::None) || styles.is_none();
        let l = node.final_layout();
        let p = node.absolute_position(0.0, 0.0);
        let (w, h) = (l.size.width as f64, l.size.height as f64);
        let rect = if hidden { [0.0; 4] } else { [p.x as f64, p.y as f64, w, h] };
        let Some(st) = styles else {
            return GeomElem {
                tag,
                rect,
                ink: None,
                ink_error: None,
                position: "static".into(),
                z_index: "auto".into(),
                display: "none".into(),
                overflow_clip: false,
                radius: [0.0; 4],
                transform_origin: [0.0; 2],
                transform: "none".into(),
                opacity: 1.0,
                filter: "none".into(),
                mix_blend_mode: "normal".into(),
                visibility: "visible".into(),
                pseudo: vec![],
            };
        };
        use style::computed_values::position::T as Pos;
        let position = match st.clone_position() {
            Pos::Static => "static",
            Pos::Relative => "relative",
            Pos::Absolute => "absolute",
            Pos::Fixed => "fixed",
            Pos::Sticky => "sticky",
        };
        let z = st.clone_z_index();
        let z_index = if z.is_auto() { "auto".to_string() } else { z.integer_or(0).to_string() };
        let bx = st.get_box();
        let overflow_clip = !matches!(bx.overflow_x, Overflow::Visible) || !matches!(bx.overflow_y, Overflow::Visible);
        let b = st.get_border();
        let rw = style::values::computed::CSSPixelLength::new(w as f32);
        let r = |c: &style::values::computed::BorderCornerRadius| c.0.width.0.resolve(rw).px() as f64;
        let radius = [r(&b.border_top_left_radius), r(&b.border_top_right_radius), r(&b.border_bottom_right_radius), r(&b.border_bottom_left_radius)];
        let to = &bx.transform_origin;
        let transform_origin = [
            to.horizontal.resolve(style::values::computed::CSSPixelLength::new(w as f32)).px() as f64,
            to.vertical.resolve(style::values::computed::CSSPixelLength::new(h as f32)).px() as f64,
        ];
        let has_transform = !bx.transform.0.is_empty() || !matches!(bx.translate, style::values::generics::transform::Translate::None)
            || !matches!(bx.rotate, style::values::generics::transform::Rotate::None)
            || !matches!(bx.scale, style::values::generics::transform::Scale::None);
        let fx = st.get_effects();
        let vis = st.get_inherited_box().visibility;
        let pseudo: Vec<String> = [(node.before(), "::before"), (node.after(), "::after")].iter().filter(|(n, _)| n.is_some()).map(|(_, s)| s.to_string()).collect();
        GeomElem {
            tag,
            rect,
            ink: None,
            ink_error: None,
            position: position.into(),
            z_index,
            display: if hidden { "none".into() } else { "block".into() },
            overflow_clip,
            radius,
            transform_origin,
            transform: if has_transform { "matrix".into() } else { "none".into() },
            opacity: fx.opacity as f64,
            filter: if fx.filter.0.is_empty() { "none".into() } else { "filter".into() },
            mix_blend_mode: if fx.mix_blend_mode == style::computed_values::mix_blend_mode::T::Normal { "normal".into() } else { "other".into() },
            visibility: if vis == style::computed_values::visibility::T::Visible { "visible".into() } else { "hidden".into() },
            pseudo,
        }
    }

    /// Replace the Chrome raster jobs of `out` with native display lists: every Image /
    /// Sequence unit content becomes `Paint` / `Switch` (one state per distinct repaint
    /// signature). Layer roots are painted at neutral transform/opacity.
    pub fn paint_units(&mut self, compiled: &Compiled, geom: &Geom, out: &mut HtmlOut, res: &mut Resources) -> Result<serde_json::Value> {
        let t0 = std::time::Instant::now();
        let parent = &compiled.parent;
        let is_anc = |a: ElemId, mut e: ElemId| -> bool {
            while let Some(p) = parent[e] {
                if p == a {
                    return true;
                }
                e = p;
            }
            false
        };
        // which layer (root elem) contains a unit: the nearest neutral root that is an
        // ancestor-or-self of the unit's elements
        let layer_of = |elems: &[ElemId]| -> Option<ElemId> {
            let e = *elems.first()?;
            out.neutral.iter().copied().filter(|&r| r == e || is_anc(r, e)).max_by_key(|&r| depth(parent, r))
        };
        let ir_tracks: Vec<agentfx_ir::Track> = compiled.tracks.iter().map(|t| t.to_ir(0)).collect();
        // state of a repaint layer at time t: inline style values of its subtree's tracks
        let state_at = |root: ElemId, t: f64| -> Result<(Vec<(ElemId, String, String)>, Vec<(ElemId, String)>)> {
            let mut best: BTreeMap<(ElemId, String), (usize, String)> = BTreeMap::new();
            let mut texts = Vec::new();
            for (i, tr) in compiled.tracks.iter().enumerate() {
                if !(tr.elem == root || is_anc(root, tr.elem)) || (tr.elem == root && tr.stage == Stage::Composite) {
                    continue;
                }
                if tr.property == "text" {
                    let n = match eval_track(&ir_tracks[i], t) {
                        Some(Sampled::Number(v)) => (v + 1e-9).floor().max(0.0) as usize,
                        _ => 0,
                    };
                    let tt = compiled.texts.iter().find(|x| x.elem == tr.elem).unwrap();
                    texts.push((tr.elem, tt.text.chars().take(n).collect()));
                    continue;
                }
                let Some((k, u)) = eval_segment(&ir_tracks[i], t) else { continue };
                let (a, b) = (&tr.keyframes[k].value, &tr.keyframes[k + 1].value);
                let v = match (a, b) {
                    (Value::Discrete(a), Value::Discrete(b)) => cssval::interpolate(a, b, u).with_context(|| format!("element #{} {}", tr.elem, tr.property))?,
                    _ => match eval_track(&ir_tracks[i], t) {
                        Some(Sampled::Number(x)) => format!("{x}"),
                        Some(Sampled::Matrix(m)) => format!("matrix({}, {}, {}, {}, {}, {})", m.a, m.b, m.c, m.d, m.e, m.f),
                        _ => continue,
                    },
                };
                let key = (tr.elem, tr.property.clone());
                if best.get(&key).is_none_or(|x| x.0 <= tr.order) {
                    best.insert(key, (tr.order, v));
                }
            }
            Ok((best.into_iter().map(|((e, p), (_, v))| (e, p, v)).collect(), texts))
        };
        let neutral: Vec<(ElemId, String, String)> = out
            .neutral
            .iter()
            .flat_map(|&r| [(r, "opacity".to_string(), "1".to_string()), (r, "transform".to_string(), "none".to_string())])
            .collect();
        let jobs: Vec<Job> = out.jobs.clone();
        let mut painted: HashMap<usize, Content> = HashMap::new();
        let mut n_states = 0usize;
        // order jobs by time so consecutive states differ little
        let mut order: Vec<usize> = (0..jobs.len()).collect();
        order.sort_by(|&a, &b| jobs[a].time.total_cmp(&jobs[b].time));
        // only paint jobs that are referenced as main content (variants are not needed: vector)
        let mut needed: HashSet<usize> = HashSet::new();
        for node in &out.scene.nodes {
            match &node.content {
                Some(Content::Image { asset, .. }) => {
                    needed.insert(*asset);
                }
                Some(Content::Sequence { frames, .. }) => needed.extend(frames.iter().map(|f| f.1)),
                _ => {}
            }
        }
        for &j in &order {
            if !needed.contains(&j) {
                continue;
            }
            let job = &jobs[j];
            let mut props = neutral.clone();
            let mut texts = Vec::new();
            if let Some(root) = job.bounds_check {
                let (p, t) = state_at(root, job.time)?;
                // later entries win (state over neutral) for the same (elem, prop)
                props.retain(|(e, pp, _)| !p.iter().any(|(e2, p2, _)| e2 == e && p2 == pp));
                props.extend(p);
                texts = t;
            }
            self.set_props(&props, true)?;
            for (e, s) in &texts {
                self.set_text(*e, s);
            }
            self.resolve();
            if let (Some(root), Some(base)) = (job.bounds_check, job.base_rect) {
                let g = self.geom_elem(root);
                if g.rect.iter().zip(&base).any(|(a, b)| (a - b).abs() > 0.5) {
                    bail!("repaint layer #{root} at t={:.3}: its box moved/resized {:?} -> {:?} (layout escapes the repaint root)", job.time, base, g.rect);
                }
            }
            let own = self.own_set(&job.visible);
            let layer = layer_of(&job.visible);
            let items = self.paint(&own, layer, job.opaque_root, res)?;
            let fonts = &res.font_data;
            let fl = |f: usize| fonts.get(f).cloned();
            let ink = record::bounds(&items, &fl, true);
            if job.bounds_check.is_some() && ink[2] > 0.0 {
                let r = job.rect;
                if ink[0] < r[0] - 1.5 || ink[1] < r[1] - 1.5 || ink[0] + ink[2] > r[0] + r[2] + 1.5 || ink[1] + ink[3] > r[1] + r[3] + 1.5 {
                    bail!("repaint unit {} at t={:.3}: ink {:?} escapes its rect {:?}", job.unit, job.time, ink, r);
                }
            }
            painted.insert(j, Content::Paint { rect: job.rect, items, elements: job.visible.clone(), text: None });
            n_states += 1;
        }
        self.set_props(&[], false)?;
        let _ = geom;
        // rewrite contents
        for node in out.scene.nodes.iter_mut() {
            let c = match node.content.take() {
                Some(Content::Image { asset, .. }) => Some(painted.remove(&asset).context("unpainted job")?),
                Some(Content::Sequence { rect, frames }) => {
                    let mut states = Vec::new();
                    let mut idx: HashMap<usize, usize> = HashMap::new();
                    let mut fr = Vec::new();
                    for (f, a) in frames {
                        let i = match idx.get(&a) {
                            Some(&i) => i,
                            None => {
                                states.push(painted.get(&a).cloned().context("unpainted job")?);
                                idx.insert(a, states.len() - 1);
                                states.len() - 1
                            }
                        };
                        fr.push((f, i));
                    }
                    Some(Content::Switch { rect, frames: fr, states })
                }
                other => other,
            };
            node.content = c;
        }
        out.scene.assets = res.assets.clone();
        out.scene.fonts = res.fonts.clone();
        out.scene.version = agentfx_ir::IR_VERSION;
        Ok(serde_json::json!({ "painted_states": n_states, "paint_units_s": t0.elapsed().as_secs_f64() }))
    }
}

fn depth(parent: &[Option<ElemId>], mut e: ElemId) -> usize {
    let mut d = 0;
    while let Some(p) = parent[e] {
        d += 1;
        e = p;
    }
    d
}
