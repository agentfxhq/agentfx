//! Design layer L2: the design spec (`design.reel`, or a `design` block in a `.reel`).
//!
//! A design spec extends the S1 brand tokens (`theme`), it does not fork them: it is a `theme`-style
//! block whose lines are applied to the same `Theme` (colors, type, motion, layout, lint, bg) plus
//! the lines below, which components read (surface treatment, elevation, icon style, type roles,
//! spacing rhythm, scale, signature treatments, do/don't lists).
//!
//! ```text
//! design "Swiss International" base=light lens=swiss
//!   colors bg=#f4f1ea surface=#ffffff text=#111111 muted=#4a4a4a accent=#e30613
//!   type display="Inter" body="Inter" base=40 ratio=1.333 display-weight=800 label-case=upper
//!   space unit=8 scale=[8, 16, 24, 40, 64, 104]
//!   layout radius=0 safe=7%
//!   surface style=flat|elevated|outline|glass|tinted border=2
//!   elevation shadow="0 24 60 #00000040, 0 2 6 #00000026"
//!   texture bg=solid grain=0
//!   icons style=tile|solid|outline|plain|ring stroke=2
//!   scale components=1 diagram=1.35 chart=1
//!   motion enter=out-expo exit=in-cubic enter-dur=0.5s exit-dur=0.3s stagger=60ms
//!   treatment hairlines
//!   do "One accent per frame, on the number that matters"
//!   dont "Rounded cards, drop shadows"
//! ```
//!
//! Use it from a video with `design src=design.reel` (path relative to the .reel file), a built-in
//! lens with `design lens=swiss` (`design/lenses/<name>/design.reel`), or inline children.
use crate::ast::{Node, Span, Value};
use crate::diag::Diag;
use crate::ir::Shadow;
use crate::values::{parse_color, ColorRef};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Directory of the document being compiled (for `design src=`); set by the CLI / tests.
static DOC_BASE: RwLock<Option<PathBuf>> = RwLock::new(None);
pub fn set_doc_base(p: &Path) {
    if let Ok(mut g) = DOC_BASE.write() {
        *g = Some(p.to_path_buf());
    }
}
pub fn doc_base() -> PathBuf {
    DOC_BASE.read().ok().and_then(|g| g.clone()).unwrap_or_else(|| PathBuf::from("."))
}

pub const SURFACES: &[&str] = &["elevated", "flat", "outline", "glass", "tinted"];
pub const ICON_STYLES: &[&str] = &["tile", "solid", "outline", "plain", "ring"];
pub const LINE_KINDS: &[&str] = &["colors", "color", "type", "sizes", "motion", "durs", "eases", "layout", "lint", "bg", "space", "surface", "elevation", "texture", "icons", "scale", "treatment", "do", "dont", "grid", "skin", "skins"];
/// Signature treatments components understand (others are kept as notes for agents and the critic).
pub const TREATMENTS: &[&str] = &["hairlines", "grid-bg", "glow", "grain", "mesh-bg", "draw-on", "motion-blur", "numbered", "underline-accent", "dot-grid", "big-numbers", "caps-labels", "frame-rules"];

#[derive(Debug, Clone)]
pub struct Design {
    pub name: Option<String>,
    pub lens: Option<String>,
    pub source: Option<String>,
    pub display_font: Option<String>,
    pub body_font: Option<String>,
    pub w_display: f64,
    pub w_heading: f64,
    pub w_body: f64,
    pub w_label: f64,
    pub tr_display: f64,
    pub tr_heading: f64,
    pub tr_label: f64,
    pub label_upper: bool,
    pub lh_display: Option<f64>,
    pub lh_body: f64,
    pub base: Option<f64>,
    pub ratio: Option<f64>,
    pub unit: f64,
    pub space: Vec<f64>,
    pub radius_sm: Option<f64>,
    pub surface: String,
    pub border_w: f64,
    pub shadows: Option<Vec<Shadow>>,
    pub grain: f64,
    pub vignette: f64,
    pub bg_style: Option<String>,
    pub icons: String,
    pub icon_stroke: f64,
    pub scale: f64,
    pub scale_diagram: f64,
    pub scale_chart: f64,
    pub grid_cols: u32,
    pub gutter: f64,
    pub treatments: Vec<String>,
    pub dos: Vec<String>,
    pub donts: Vec<String>,
    /// explicit size keys set by a design (the type scale only fills the others)
    pub explicit_sizes: Vec<String>,
    /// unknown keys/lines, reported by lint (`E_DESIGN_TOKEN`)
    pub problems: Vec<(String, String, Span)>,
    pub loaded: bool,
    /// enter travel for rise/drop/left/right (motion tokens: 28 px measured)
    pub enter_distance: f64,
    /// default skins per component kind (`skin node=tile progress=bar`)
    pub skins: std::collections::HashMap<String, String>,
}

impl Default for Design {
    /// The S1 look (so videos without a design spec render as before, apart from the larger
    /// default diagram scale).
    fn default() -> Self {
        Design {
            name: None,
            lens: None,
            source: None,
            display_font: None,
            body_font: None,
            w_display: 800.0,
            w_heading: 720.0,
            w_body: 500.0,
            w_label: 700.0,
            tr_display: -0.03,
            tr_heading: -0.01,
            tr_label: 0.08,
            label_upper: true,
            lh_display: None,
            lh_body: 1.35,
            base: None,
            ratio: None,
            unit: 8.0,
            space: vec![],
            radius_sm: None,
            surface: "elevated".into(),
            border_w: 1.0,
            shadows: None,
            grain: 0.0,
            vignette: 0.0,
            bg_style: None,
            icons: "tile".into(),
            icon_stroke: 2.0,
            scale: 1.0,
            scale_diagram: 1.35,
            scale_chart: 1.0,
            grid_cols: 12,
            gutter: 24.0,
            treatments: vec![],
            dos: vec![],
            donts: vec![],
            explicit_sizes: vec![],
            problems: vec![],
            loaded: false,
            enter_distance: 28.0,
            skins: Default::default(),
        }
    }
}

pub fn num(raw: &str) -> Option<f64> {
    raw.trim().trim_end_matches("px").trim_end_matches('%').trim_end_matches("em").parse().ok()
}
fn is_num(raw: &str) -> bool {
    num(raw).is_some()
}

/// "0 24 60 #00000040, 0 2 6 #00000026" (x y blur [spread] color)
pub fn parse_shadows(s: &str) -> Option<Vec<Shadow>> {
    let mut out = vec![];
    for part in s.split(',') {
        let p: Vec<&str> = part.split_whitespace().collect();
        if p.is_empty() {
            continue;
        }
        let inner = p.contains(&"inset");
        let p: Vec<&str> = p.into_iter().filter(|x| *x != "inset").collect();
        let nums: Vec<f64> = p.iter().filter_map(|x| num(x)).collect();
        let col = p.iter().find(|x| num(x).is_none()).and_then(|c| match parse_color(c) {
            Ok(ColorRef::Lit(c)) => Some(c),
            _ => None,
        });
        if nums.len() < 3 || col.is_none() {
            return None;
        }
        out.push(Shadow { x: nums[0], y: nums[1], blur: nums[2], spread: nums.get(3).copied().unwrap_or(0.0), color: col, inner });
    }
    (!out.is_empty()).then_some(out)
}

impl Design {
    /// Apply one design-only line (the theme handles colors/type sizes/motion/layout/lint/bg).
    /// Returns true when the line kind belongs to the design layer.
    pub fn apply_line(&mut self, n: &Node) -> bool {
        let k = n.kind.as_str();
        let bad = |me: &mut Design, key: &str, hint: &str| me.problems.push((format!("`{k} {key}`"), hint.to_string(), n.span));
        match k {
            "do" | "dont" => {
                let s: Vec<String> = n.args.iter().map(|v| v.raw()).collect();
                let s = s.join(" ");
                if k == "do" { self.dos.push(s) } else { self.donts.push(s) }
            }
            "treatment" => {
                for a in &n.args {
                    for t in a.raw().split([',', ' ']).filter(|s| !s.is_empty()) {
                        self.treatments.push(t.to_string());
                    }
                }
            }
            "skin" | "skins" => {
                for (key, v) in &n.props {
                    self.skins.insert(key.clone(), v.raw());
                }
            }
            "space" | "surface" | "elevation" | "texture" | "icons" | "scale" | "grid" => {
                for (key, v) in &n.props {
                    let raw = v.raw();
                    match (k, key.as_str()) {
                        ("space", "unit") => self.unit = num(&raw).unwrap_or(self.unit),
                        ("space", "scale") => {
                            self.space = match v {
                                Value::List(l) => l.iter().filter_map(|x| num(&x.raw())).collect(),
                                _ => raw.split([',', ' ']).filter_map(num).collect(),
                            }
                        }
                        ("surface", "style") => {
                            if SURFACES.contains(&raw.as_str()) {
                                self.surface = raw
                            } else {
                                bad(self, key, "surface style=elevated|flat|outline|glass|tinted")
                            }
                        }
                        ("surface", "border") => self.border_w = num(&raw).unwrap_or(self.border_w),
                        ("elevation", "shadow") => match raw.as_str() {
                            "none" => self.shadows = Some(vec![]),
                            s => match parse_shadows(s) {
                                Some(v) => self.shadows = Some(v),
                                None => bad(self, key, "shadow=\"x y blur #rrggbbaa, ...\" or none"),
                            },
                        },
                        ("texture", "grain") => self.grain = num(&raw).unwrap_or(0.0),
                        ("texture", "vignette") => self.vignette = num(&raw).unwrap_or(0.0),
                        ("texture", "bg") => self.bg_style = Some(raw),
                        ("icons", "style") => {
                            if ICON_STYLES.contains(&raw.as_str()) {
                                self.icons = raw
                            } else {
                                bad(self, key, "icons style=tile|solid|outline|plain|ring")
                            }
                        }
                        ("icons", "stroke") => self.icon_stroke = num(&raw).unwrap_or(2.0),
                        ("scale", "components") => self.scale = num(&raw).unwrap_or(1.0),
                        ("scale", "diagram") => self.scale_diagram = num(&raw).unwrap_or(1.35),
                        ("scale", "chart") => self.scale_chart = num(&raw).unwrap_or(1.0),
                        ("grid", "cols") => self.grid_cols = num(&raw).unwrap_or(12.0) as u32,
                        ("grid", "gutter") => self.gutter = num(&raw).unwrap_or(24.0),
                        _ => bad(self, key, "see design/AUTHORING.md for the keys of each line"),
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// `type` keys that are not plain sizes (fonts, weights, tracking, ratio). Returns true if handled.
    pub fn type_key(&mut self, key: &str, raw: &str) -> bool {
        let f = num(raw);
        match key {
            "display" | "body" if !is_num(raw) => {
                if key == "display" { self.display_font = Some(raw.to_string()) } else { self.body_font = Some(raw.to_string()) }
            }
            "base" => self.base = f,
            "ratio" => self.ratio = f,
            "display-weight" => self.w_display = f.unwrap_or(self.w_display),
            "heading-weight" => self.w_heading = f.unwrap_or(self.w_heading),
            "body-weight" => self.w_body = f.unwrap_or(self.w_body),
            "label-weight" => self.w_label = f.unwrap_or(self.w_label),
            "display-tracking" => self.tr_display = f.unwrap_or(self.tr_display),
            "heading-tracking" => self.tr_heading = f.unwrap_or(self.tr_heading),
            "label-tracking" => self.tr_label = f.unwrap_or(self.tr_label),
            "label-case" => self.label_upper = raw != "none" && raw != "lower" && raw != "normal",
            "lh-display" => self.lh_display = f,
            "lh-body" => self.lh_body = f.unwrap_or(self.lh_body),
            _ => return false,
        }
        true
    }

    pub fn has(&self, treatment: &str) -> bool {
        self.treatments.iter().any(|t| t == treatment)
    }
}

/// Resolve a `design` node's source file: `src=` (relative to the document) or `lens=` (built-in).
pub fn source_of(n: &Node) -> Option<Result<PathBuf, String>> {
    if let Some(src) = n.props.get("src").map(|v| v.raw()) {
        let p = PathBuf::from(&src);
        let p = if p.is_absolute() { p } else { doc_base().join(p) };
        return Some(if p.exists() { Ok(p) } else { Err(format!("design file `{src}` not found (paths are relative to the .reel file)")) });
    }
    if let Some(l) = n.props.get("lens").map(|v| v.raw()) {
        let p = lens_dir().join(&l).join("design.reel");
        return Some(if p.exists() { Ok(p) } else { Err(format!("unknown lens `{l}`; lenses: {}", lenses().join(", "))) });
    }
    None
}

pub fn lens_dir() -> PathBuf {
    crate::run::home().join("design/lenses")
}
pub fn lenses() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(lens_dir()).map(|rd| rd.flatten().filter(|e| e.path().join("design.reel").exists()).map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Parse a design file into its design/theme nodes (diagnostics are re-anchored to the referencing node).
pub fn load_file(p: &Path, at: Span, diags: &mut Vec<Diag>) -> Vec<Node> {
    let src = match std::fs::read_to_string(p) {
        Ok(s) => s,
        Err(e) => {
            diags.push(Diag::error("E_DESIGN", format!("cannot read {}: {e}", p.display()), "check the path", at));
            return vec![];
        }
    };
    let (doc, errs) = crate::parse::parse(&src);
    for e in errs {
        diags.push(Diag::error(&e.code, format!("{} (in {})", e.message, p.display()), e.hint, at));
    }
    doc.nodes.into_iter().filter(|n| n.kind == "design" || n.kind == "theme").collect()
}
