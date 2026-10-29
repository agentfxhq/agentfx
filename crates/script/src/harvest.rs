//! `agentfx design harvest`: read style-frame HTML through Blitz/Stylo computed styles (exact values, no
//! browser, no screenshots). This module extracts one record per laid-out element; the aggregation
//! into a design spec (color roles, type scale, spacing rhythm, shadows, gradients) is
//! `tools/harvest.py`, which `agentfx design harvest` runs on this JSON.
use anyhow::Result;
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use serde_json::{json, Value};
use std::path::Path;
use style::color::AbsoluteColor;
use style::values::generics::image::{GenericGradient, GenericGradientItem, GenericImage};

fn rgba(c: &AbsoluteColor) -> Value {
    let s = c.to_color_space(style::color::ColorSpace::Srgb);
    let k = s.raw_components();
    json!([r3(k[0] as f64), r3(k[1] as f64), r3(k[2] as f64), r3(s.alpha as f64)])
}
fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

pub fn extract(file: &Path, w: u32, h: u32, fonts_dir: &Path) -> Result<Value> {
    crate::native::layout_mode();
    let html = std::fs::read_to_string(file)?;
    let mut sources = std::collections::HashMap::new();
    let fctx = crate::native::font_ctx_pub(fonts_dir, &mut sources);
    let base = url::Url::from_file_path(file.canonicalize()?).map(|u| u.to_string()).unwrap_or("file:///".into());
    let mut doc = HtmlDocument::from_html(
        &html,
        DocumentConfig { base_url: Some(base), viewport: Some(Viewport::new(w, h, 1.0, ColorScheme::Light)), font_ctx: Some(fctx), ..Default::default() },
    );
    for _ in 0..20 {
        doc.handle_messages();
        doc.resolve(0.0);
    }
    let mut out = vec![];
    // walk with absolute offsets (taffy locations are relative to the parent)
    let root = doc.root_node().id;
    let mut stack = vec![(root, 0.0f64, 0.0f64, 0usize)];
    while let Some((id, px, py, depth)) = stack.pop() {
        let Some(nd) = doc.get_node(id) else { continue };
        if nd.is_text_node() {
            continue;
        }
        if nd.element_data().is_none() {
            for c in nd.children.iter().rev() {
                stack.push((*c, px, py, depth + 1));
            }
            continue;
        }
        let l = nd.final_layout();
        let (x, y) = (px + l.location.x as f64, py + l.location.y as f64);
        let (bw, bh) = (l.size.width as f64, l.size.height as f64);
        if let (Some(el), Some(st)) = (nd.element_data(), nd.primary_styles()) {
            let tag = el.name.local.to_string();
            let disp_none = matches!(st.get_box().clone_display(), style::values::specified::box_::Display::None);
            if !disp_none && !matches!(tag.as_str(), "style" | "script" | "head" | "title" | "meta" | "link") {
                let cc = st.clone_color();
                let font = st.get_font();
                let fs = font.font_size.computed_size().px() as f64;
                let fam = font.font_family.families.list.iter().find_map(|f| match f {
                    style::values::computed::font::SingleFontFamily::FamilyName(n) => Some(n.name.to_string()),
                    _ => None,
                }).unwrap_or_default();
                let lh = match font.line_height {
                    style::values::computed::font::LineHeight::Normal => Value::Null,
                    style::values::computed::font::LineHeight::Number(n) => json!(r3(n.0 as f64)),
                    style::values::computed::font::LineHeight::Length(v) => json!(r3(v.0.px() as f64 / fs.max(1.0))),
                };
                let it = st.get_inherited_text();
                let ls = it.letter_spacing.0.resolve(style::values::computed::Length::new(fs as f32)).px() as f64;
                let upper = st.clone_text_transform().contains(style::values::computed::text::TextTransform::UPPERCASE);
                // direct text
                let mut chars = 0usize;
                let mut sample = String::new();
                for c in nd.children.iter() {
                    if let Some(t) = doc.get_node(*c).and_then(|n| n.text_data()) {
                        let s = t.content.trim();
                        chars += s.chars().filter(|c| !c.is_whitespace()).count();
                        if sample.len() < 40 {
                            sample.push_str(s);
                        }
                    }
                }
                let bg = st.get_background();
                let bgc = bg.background_color.resolve_to_absolute(&cc);
                let mut grads = vec![];
                for img in bg.background_image.0.iter() {
                    if let GenericImage::Gradient(g) = img {
                        let (kind, items): (&str, Vec<(Value, Option<f64>)>) = match &**g {
                            GenericGradient::Linear { items, .. } => ("linear", items.iter().filter_map(|i| match i {
                                GenericGradientItem::SimpleColorStop(c) => Some((rgba(&c.resolve_to_absolute(&cc)), None)),
                                GenericGradientItem::ComplexColorStop { color, position } => Some((rgba(&color.resolve_to_absolute(&cc)), position.to_percentage().map(|p| p.0 as f64))),
                                _ => None,
                            }).collect()),
                            GenericGradient::Radial { items, .. } => ("radial", items.iter().filter_map(|i| match i {
                                GenericGradientItem::SimpleColorStop(c) => Some((rgba(&c.resolve_to_absolute(&cc)), None)),
                                GenericGradientItem::ComplexColorStop { color, position } => Some((rgba(&color.resolve_to_absolute(&cc)), position.to_percentage().map(|p| p.0 as f64))),
                                _ => None,
                            }).collect()),
                            GenericGradient::Conic { .. } => ("conic", vec![]),
                        };
                        let angle = match &**g {
                            GenericGradient::Linear { direction: style::values::computed::LineDirection::Angle(a), .. } => Some(a.degrees() as f64),
                            _ => None,
                        };
                        grads.push(json!({"kind": kind, "angle": angle, "stops": items.iter().map(|(c, p)| json!({"c": c, "p": p})).collect::<Vec<_>>()}));
                    }
                }
                let b = st.get_border();
                let rad = |r: &style::values::computed::BorderCornerRadius| r.0.width.0.resolve(style::values::computed::Length::new(bw as f32)).px() as f64;
                let eff = st.get_effects();
                let shadows: Vec<Value> = eff.box_shadow.0.iter().map(|s| json!({"x": s.base.horizontal.px(), "y": s.base.vertical.px(), "blur": s.base.blur.px(), "spread": s.spread.px(), "c": rgba(&s.base.color.resolve_to_absolute(&cc)), "inset": s.inset})).collect();
                let gapv = |g: &style::values::computed::length::NonNegativeLengthPercentageOrNormal| -> f64 {
                    match g {
                        style::values::generics::length::GenericLengthPercentageOrNormal::LengthPercentage(lp) => lp.0.resolve(style::values::computed::Length::new(bw as f32)).px() as f64,
                        _ => 0.0,
                    }
                };
                let pos = st.get_position();
                out.push(json!({
                    "tag": tag, "depth": depth, "x": r3(x), "y": r3(y), "w": r3(bw), "h": r3(bh),
                    "color": rgba(&cc), "bg": rgba(&bgc), "grads": grads, "opacity": eff.opacity as f64,
                    "family": fam, "size": r3(fs), "weight": font.font_weight.value() as f64, "lh": lh, "ls_em": r3(ls / fs.max(1.0)), "upper": upper,
                    "chars": chars, "text": sample,
                    "pad": [l.padding.top, l.padding.right, l.padding.bottom, l.padding.left],
                    "margin": [l.margin.top, l.margin.right, l.margin.bottom, l.margin.left],
                    "gap": [gapv(&pos.row_gap), gapv(&pos.column_gap)],
                    "border": [l.border.top, l.border.right, l.border.bottom, l.border.left],
                    "border_color": rgba(&b.border_top_color.resolve_to_absolute(&cc)),
                    "radius": [rad(&b.border_top_left_radius), rad(&b.border_top_right_radius), rad(&b.border_bottom_right_radius), rad(&b.border_bottom_left_radius)],
                    "shadows": shadows,
                }));
            }
        }
        for c in nd.children.iter().rev() {
            stack.push((*c, x, y, depth + 1));
        }
    }
    Ok(json!({"file": file.to_string_lossy(), "w": w, "h": h, "elements": out}))
}
