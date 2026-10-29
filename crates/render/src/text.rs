//! Native text for direct mode: shape `Content::Text` with Parley (HarfRust shaping,
//! fontique fonts) into positioned glyphs with unit indices (glyph / word / line) for
//! per-unit tracks. Like browsers, `opsz` follows the font size (optical sizing auto).
use anyhow::{Context, Result, bail};
use peniko::FontData;
use agentfx_ir::TextContent;
use std::borrow::Cow;

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub id: u32,
    /// pen position (baseline), scene coordinates
    pub x: f32,
    pub y: f32,
    /// unit index (glyph/word/line per TextContent::unit)
    pub unit: usize,
    /// per-unit transform origin (center of the unit's advance box, 0.35 em above baseline)
    pub origin: (f64, f64),
    /// glyph ink bounds, scene coordinates [x, y, w, h]
    pub bounds: [f64; 4],
}

#[derive(Clone, Debug)]
pub struct Shaped {
    #[allow(dead_code)]
    pub font: FontData,
    pub size: f32,
    pub coords: Vec<i16>,
    pub glyphs: Vec<ShapedGlyph>,
    pub units: usize,
}

pub struct Shaper {
    fcx: parley::FontContext,
    lcx: parley::LayoutContext<()>,
    families: Vec<String>,
}

impl Shaper {
    /// `fonts`: the scene's font files (decoded), in IR order.
    pub fn new(fonts: &[FontData]) -> Result<Shaper> {
        use parley::fontique::{Collection, CollectionOptions, SourceCache};
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
        let mut families = Vec::new();
        for (i, f) in fonts.iter().enumerate() {
            let reg = collection.register_fonts(f.data.clone(), None);
            let fam = reg.first().with_context(|| format!("font {i}: no faces"))?.0;
            families.push(collection.family_name(fam).unwrap_or_default().to_string());
        }
        Ok(Shaper { fcx: parley::FontContext { collection, source_cache: SourceCache::default() }, lcx: parley::LayoutContext::new(), families })
    }

    pub fn shape(&mut self, t: &TextContent) -> Result<Shaped> {
        use parley::{Alignment, AlignmentOptions, FontFamily, FontVariation, FontVariations, FontWeight, LineHeight, PositionedLayoutItem, StyleProperty};
        let family = self.families.get(t.font).with_context(|| format!("font {} out of range", t.font))?.clone();
        let mut b = self.lcx.ranged_builder(&mut self.fcx, &t.text, 1.0, false);
        b.push_default(StyleProperty::FontFamily(FontFamily::Source(Cow::Owned(family.clone()))));
        b.push_default(StyleProperty::FontSize(t.size as f32));
        b.push_default(StyleProperty::FontWeight(FontWeight::new(t.weight as f32)));
        b.push_default(StyleProperty::LetterSpacing(t.letter_spacing as f32));
        b.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(t.line_height as f32)));
        let vars = vec![FontVariation { tag: parley::setting::Tag::from_bytes(*b"opsz"), value: t.size as f32 }];
        b.push_default(StyleProperty::FontVariations(FontVariations::List(Cow::Owned(vars))));
        let mut layout = b.build(&t.text);
        layout.break_all_lines(t.max_width.map(|w| w as f32));
        let align = match t.align.as_str() {
            "center" => Alignment::Center,
            "right" => Alignment::Right,
            _ => Alignment::Left,
        };
        layout.align(align, AlignmentOptions::default());
        // Parley aligns lines within max_width (or the widest line) already
        let x_off = 0.0;
        // word index per byte
        let mut word_of = vec![0usize; t.text.len() + 1];
        let mut w = 0usize;
        let mut in_space = true;
        for (i, ch) in t.text.char_indices() {
            if ch.is_whitespace() {
                in_space = true;
            } else if in_space {
                if i > 0 {
                    w += 1;
                }
                in_space = false;
            }
            for k in i..i + ch.len_utf8() {
                word_of[k] = w;
            }
        }
        let mut out: Vec<ShapedGlyph> = Vec::new();
        let mut font: Option<FontData> = None;
        let mut size = t.size as f32;
        let mut coords = Vec::new();
        for (li, line) in layout.lines().enumerate() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
                let run = gr.run();
                let f = run.font().clone();
                match &font {
                    None => {
                        font = Some(f.clone());
                        size = run.font_size();
                        coords = bytemuck_coords(run.normalized_coords());
                    }
                    Some(prev) if prev.data.id() != f.data.id() || prev.index != f.index => {
                        bail!("text {:?} needs more than one font (fallback); reel supports one font per text node", t.text)
                    }
                    _ => {}
                }
                // cluster byte index per glyph (visual order)
                let mut cl = Vec::new();
                for c in run.visual_clusters() {
                    let start = c.text_range().start;
                    let adv = c.advance();
                    let n = c.glyphs().count().max(0);
                    for _ in 0..n {
                        cl.push((start, adv));
                    }
                }
                for (gi, g) in gr.positioned_glyphs().enumerate() {
                    let (byte, adv) = cl.get(gi).copied().unwrap_or((0, 0.0));
                    let unit = match t.unit.as_str() {
                        "word" => word_of[byte],
                        "line" => li,
                        _ => t.text[..byte].chars().count(),
                    };
                    let x = (g.x as f64 + t.pos[0] + x_off) as f32;
                    let y = (g.y as f64 + t.pos[1]) as f32;
                    out.push(ShapedGlyph { id: g.id as u32, x, y, unit, origin: (x as f64 + adv as f64 / 2.0, y as f64 - 0.35 * t.size), bounds: [0.0; 4] });
                }
            }
        }
        let font = font.context("text produced no glyphs")?;
        // ink bounds per glyph
        for g in out.iter_mut() {
            let b = crate::record::glyph_bounds(Some(&font), size, &coords, &[(g.id, g.x, g.y)], None);
            g.bounds = [b.x0, b.y0, b.width(), b.height()];
        }
        // word / line units: origin = center of the unit's glyph extent
        if t.unit != "glyph" {
            let n = out.iter().map(|g| g.unit + 1).max().unwrap_or(0);
            let mut ext = vec![(f64::INFINITY, f64::NEG_INFINITY, 0.0f64); n];
            for g in &out {
                let e = &mut ext[g.unit];
                e.0 = e.0.min(g.bounds[0]);
                e.1 = e.1.max(g.bounds[0] + g.bounds[2]);
                e.2 = g.y as f64;
            }
            for g in out.iter_mut() {
                let e = ext[g.unit];
                g.origin = ((e.0 + e.1) / 2.0, e.2 - 0.35 * t.size);
            }
        }
        let units = out.iter().map(|g| g.unit + 1).max().unwrap_or(0);
        Ok(Shaped { font, size, coords, glyphs: out, units })
    }
}

fn bytemuck_coords<T: bytemuck::Pod>(c: &[T]) -> Vec<i16> {
    bytemuck::cast_slice::<T, i16>(c).to_vec()
}
