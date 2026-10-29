//! anyrender `PaintScene` that records Blitz's paint commands into IR display-list
//! items (`agentfx_ir::Draw`), plus resource registration (fonts, images) and
//! display-list ink bounds. Unsupported commands are collected as errors (fail loudly).
use anyhow::{Context, Result};
use anyrender::{Paint, PaintRef, PaintScene, RenderContext};
use kurbo::{Affine, Rect, Shape as _, Stroke};
use peniko::{BlendMode, Color, Compose, Fill, FontData, Mix, StyleRef};
use agentfx_ir::{Asset, AssetId, Brush, Draw, DrawFilter, Extend, FontAsset, FontId, Mat, Shape, Stop, StrokeStyle};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Fonts and images referenced by recorded display lists. Font/image blobs are keyed by
/// their peniko blob id; known source files are referenced in place, everything else is
/// written to `out_dir`.
pub struct Resources {
    pub out_dir: PathBuf,
    pub fonts: Vec<FontAsset>,
    pub assets: Vec<Asset>,
    font_ids: HashMap<(u64, u32), FontId>,
    image_ids: HashMap<u64, AssetId>,
    /// blob id -> source file of an image (from `<img src>`), used instead of re-encoding.
    pub image_sources: HashMap<u64, PathBuf>,
    /// Decoded fonts/images kept in memory for an in-process render (keyed like the IR lists).
    pub font_data: Vec<FontData>,
    pub image_data: Vec<Option<Arc<peniko::ImageData>>>,
}

impl Resources {
    pub fn new(out_dir: &Path) -> Self {
        Self {
            out_dir: out_dir.to_path_buf(),
            fonts: vec![],
            assets: vec![],
            font_ids: HashMap::new(),
            image_ids: HashMap::new(),
            image_sources: HashMap::new(),
            font_data: vec![],
            image_data: vec![],
        }
    }

    fn font(&mut self, f: &FontData) -> Result<FontId> {
        let key = (f.data.id(), f.index);
        if let Some(&id) = self.font_ids.get(&key) {
            return Ok(id);
        }
        let id = self.fonts.len();
        let dir = self.out_dir.join("fonts");
        std::fs::create_dir_all(&dir)?;
        let name = format!("font{id}.bin");
        std::fs::write(dir.join(&name), f.data.data()).context("writing font")?;
        self.fonts.push(FontAsset { path: format!("fonts/{name}"), index: f.index });
        self.font_data.push(f.clone());
        self.font_ids.insert(key, id);
        Ok(id)
    }

    fn image(&mut self, img: &peniko::ImageData) -> Result<AssetId> {
        let key = img.data.id();
        if let Some(&id) = self.image_ids.get(&key) {
            return Ok(id);
        }
        anyhow::ensure!(img.format == peniko::ImageFormat::Rgba8, "image format {:?} unsupported", img.format);
        anyhow::ensure!(img.alpha_type == peniko::ImageAlphaType::Alpha, "premultiplied image input unsupported");
        let id = self.assets.len();
        let path = match self.image_sources.get(&key) {
            Some(p) => p.to_string_lossy().into_owned(),
            None => {
                let dir = self.out_dir.join("images");
                std::fs::create_dir_all(&dir)?;
                let name = format!("img{id}.png");
                let f = std::io::BufWriter::new(std::fs::File::create(dir.join(&name))?);
                let mut enc = png::Encoder::new(f, img.width, img.height);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                enc.set_compression(png::Compression::Fast);
                enc.write_header()?.write_image_data(img.data.data())?;
                format!("images/{name}")
            }
        };
        self.assets.push(Asset { path, scale: 1.0 });
        self.image_data.push(Some(Arc::new(img.clone())));
        self.image_ids.insert(key, id);
        Ok(id)
    }
}

pub struct Recorder<'r> {
    pub items: Vec<Draw>,
    pub errors: Vec<String>,
    res: &'r mut Resources,
    /// Shared item counter (read by the paint observer to attribute items to elements).
    pub counter: Option<std::rc::Rc<std::cell::Cell<usize>>>,
    /// pops owed per open layer (a filter chain is several nested groups)
    layer_pops: Vec<usize>,
}

impl<'r> Recorder<'r> {
    pub fn new(res: &'r mut Resources) -> Self {
        Self { items: vec![], errors: vec![], res, counter: None, layer_pops: vec![] }
    }
    fn push(&mut self, d: Draw) {
        self.items.push(d);
        if let Some(c) = &self.counter {
            c.set(self.items.len());
        }
    }
}

pub fn mat(a: Affine) -> Mat {
    a.as_coeffs()
}

fn shape(s: &impl kurbo::Shape) -> Shape {
    if let Some(r) = s.as_rect() {
        return Shape::Rect([r.x0, r.y0, r.x1, r.y1]);
    }
    if let Some(rr) = s.as_rounded_rect() {
        let r = rr.rect();
        let k = rr.radii();
        return Shape::RoundedRect { rect: [r.x0, r.y0, r.x1, r.y1], radii: [k.top_left, k.top_right, k.bottom_right, k.bottom_left] };
    }
    Shape::Path(s.to_path(0.1).to_svg())
}

fn color(c: Color) -> [f32; 4] {
    c.components
}

fn extend(e: peniko::Extend) -> Extend {
    match e {
        peniko::Extend::Pad => Extend::Pad,
        peniko::Extend::Repeat => Extend::Repeat,
        peniko::Extend::Reflect => Extend::Reflect,
    }
}

fn blend(b: BlendMode) -> std::result::Result<agentfx_ir::BlendMode, String> {
    use agentfx_ir::BlendMode as B;
    Ok(match (b.mix, b.compose) {
        (Mix::Normal, Compose::SrcOver) => B::Normal,
        (Mix::Multiply, Compose::SrcOver) => B::Multiply,
        (Mix::Screen, Compose::SrcOver) => B::Screen,
        (Mix::Overlay, Compose::SrcOver) => B::Overlay,
        (Mix::Darken, Compose::SrcOver) => B::Darken,
        (Mix::Lighten, Compose::SrcOver) => B::Lighten,
        (Mix::Normal, Compose::Plus) => B::Plus,
        (Mix::ColorDodge, Compose::SrcOver) => B::ColorDodge,
        (Mix::ColorBurn, Compose::SrcOver) => B::ColorBurn,
        (Mix::HardLight, Compose::SrcOver) => B::HardLight,
        (Mix::SoftLight, Compose::SrcOver) => B::SoftLight,
        (Mix::Difference, Compose::SrcOver) => B::Difference,
        (Mix::Exclusion, Compose::SrcOver) => B::Exclusion,
        (Mix::Hue, Compose::SrcOver) => B::Hue,
        (Mix::Saturation, Compose::SrcOver) => B::Saturation,
        (Mix::Color, Compose::SrcOver) => B::Color,
        (Mix::Luminosity, Compose::SrcOver) => B::Luminosity,
        (Mix::Normal, Compose::SrcAtop) => B::SrcAtop,
        other => return Err(format!("blend mode {other:?} unsupported")),
    })
}

impl Recorder<'_> {
    fn brush(&mut self, p: PaintRef<'_>) -> Option<Brush> {
        match p {
            Paint::Solid(c) => Some(Brush::Solid(color(c))),
            Paint::Gradient(g) => {
                if g.interpolation_cs != peniko::color::ColorSpaceTag::Srgb {
                    self.errors.push(format!("gradient interpolation space {:?} unsupported", g.interpolation_cs));
                }
                let stops: Vec<Stop> = g
                    .stops
                    .iter()
                    .map(|s| Stop { offset: s.offset, color: s.color.to_alpha_color::<peniko::color::Srgb>().components })
                    .collect();
                let ext = extend(g.extend);
                Some(match g.kind {
                    peniko::GradientKind::Linear(l) => Brush::Linear { start: [l.start.x, l.start.y], end: [l.end.x, l.end.y], stops, extend: ext },
                    peniko::GradientKind::Radial(r) => Brush::Radial {
                        start_center: [r.start_center.x, r.start_center.y],
                        start_radius: r.start_radius,
                        end_center: [r.end_center.x, r.end_center.y],
                        end_radius: r.end_radius,
                        stops,
                        extend: ext,
                    },
                    peniko::GradientKind::Sweep(s) => Brush::Sweep { center: [s.center.x, s.center.y], start_angle: s.start_angle, end_angle: s.end_angle, stops, extend: ext },
                })
            }
            Paint::Image(img) => match self.res.image(img.image) {
                Ok(asset) => Some(Brush::Image {
                    asset,
                    quality: match img.sampler.quality {
                        peniko::ImageQuality::Low => 0,
                        peniko::ImageQuality::Medium => 1,
                        peniko::ImageQuality::High => 2,
                    },
                    extend: [extend(img.sampler.x_extend), extend(img.sampler.y_extend)],
                    alpha: img.sampler.alpha,
                }),
                Err(e) => {
                    self.errors.push(format!("image: {e:#}"));
                    None
                }
            },
            _ => {
                self.errors.push("custom/resource paint unsupported".into());
                None
            }
        }
    }
}

impl RenderContext for Recorder<'_> {}

impl PaintScene for Recorder<'_> {
    fn reset(&mut self) {
        self.items.clear();
    }

    fn push_layer(
        &mut self,
        fill: Fill,
        blend_mode: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl kurbo::Shape,
        filter: Option<Arc<anyrender::Filter>>,
        backdrop_filter: Option<Arc<anyrender::Filter>>,
    ) {
        if backdrop_filter.is_some() {
            self.errors.push("backdrop-filter unsupported".into());
        }
        let blend = match blend(blend_mode.into()) {
            Ok(b) => b,
            Err(e) => {
                self.errors.push(e);
                agentfx_ir::BlendMode::Normal
            }
        };
        // filter chains: one nested group per primitive (innermost = first primitive)
        let mut chain: Vec<DrawFilter> = Vec::new();
        if let Some(f) = filter {
            use anyrender::filters::FilterEffect as E;
            for node in f.nodes() {
                match &node.effect {
                    E::GaussianBlur(b) => chain.push(DrawFilter::Blur { std_dev: b.std_deviation as f64 }),
                    E::DropShadow(d) => chain.push(DrawFilter::DropShadow { dx: d.dx as f64, dy: d.dy as f64, std_dev: d.std_deviation as f64, color: d.color.components }),
                    other => self.errors.push(format!("filter {other:?} unsupported")),
                }
            }
        }
        let n = chain.len().max(1);
        let outer = chain.pop();
        self.push(Draw::Push { clip: Some(shape(clip)), transform: mat(transform), alpha, blend, filter: outer, even_odd: fill == Fill::EvenOdd });
        while let Some(f) = chain.pop() {
            self.push(Draw::Push { clip: None, transform: mat(transform), alpha: 1.0, blend: agentfx_ir::BlendMode::Normal, filter: Some(f), even_odd: false });
        }
        self.layer_pops.push(n);
    }

    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl kurbo::Shape) {
        self.layer_pops.push(1);
        self.push(Draw::Push { clip: Some(shape(clip)), transform: mat(transform), alpha: 1.0, blend: agentfx_ir::BlendMode::Normal, filter: None, even_odd: fill == Fill::EvenOdd });
    }

    fn pop_layer(&mut self) {
        for _ in 0..self.layer_pops.pop().unwrap_or(1) {
            self.push(Draw::Pop);
        }
    }

    fn stroke<'a>(&mut self, style: &Stroke, transform: Affine, brush: impl Into<PaintRef<'a>>, brush_transform: Option<Affine>, s: &impl kurbo::Shape) {
        let Some(brush) = self.brush(brush.into()) else { return };
        let cap = |c: kurbo::Cap| match c {
            kurbo::Cap::Butt => "butt",
            kurbo::Cap::Round => "round",
            kurbo::Cap::Square => "square",
        };
        if style.start_cap != style.end_cap {
            self.errors.push("different start/end caps unsupported".into());
        }
        let st = StrokeStyle {
            width: style.width,
            join: match style.join {
                kurbo::Join::Bevel => "bevel",
                kurbo::Join::Miter => "miter",
                kurbo::Join::Round => "round",
            }
            .into(),
            miter_limit: style.miter_limit,
            cap: cap(style.start_cap).into(),
            dash: style.dash_pattern.to_vec(),
            dash_offset: style.dash_offset,
        };
        self.push(Draw::Stroke { shape: shape(s), brush, style: st, transform: mat(transform), brush_transform: brush_transform.map(mat) });
    }

    fn fill<'a>(&mut self, style: Fill, transform: Affine, brush: impl Into<PaintRef<'a>>, brush_transform: Option<Affine>, s: &impl kurbo::Shape) {
        let Some(brush) = self.brush(brush.into()) else { return };
        self.push(Draw::Fill { shape: shape(s), brush, transform: mat(transform), brush_transform: brush_transform.map(mat), even_odd: style == Fill::EvenOdd });
    }

    fn draw_glyphs<'a, 's: 'a>(
        &'s mut self,
        font: &'a FontData,
        font_size: f32,
        hint: bool,
        normalized_coords: &'a [anyrender::NormalizedCoord],
        embolden: kurbo::Vec2,
        style: impl Into<StyleRef<'a>>,
        brush: impl Into<PaintRef<'a>>,
        _brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = anyrender::Glyph> + Clone,
    ) {
        if !matches!(style.into(), StyleRef::Fill(Fill::NonZero)) {
            self.errors.push("stroked/even-odd glyph runs unsupported".into());
        }
        let Some(brush) = self.brush(brush.into()) else { return };
        let font = match self.res.font(font) {
            Ok(f) => f,
            Err(e) => {
                self.errors.push(format!("font: {e:#}"));
                return;
            }
        };
        self.push(Draw::Glyphs {
            font,
            size: font_size,
            coords: normalized_coords.to_vec(),
            embolden: (embolden != kurbo::Vec2::ZERO).then_some([embolden.x, embolden.y]),
            hint,
            brush,
            transform: mat(transform),
            glyph_transform: glyph_transform.map(mat),
            glyphs: glyphs.map(|g| (g.id, g.x, g.y)).collect(),
            stroke: None,
        });
    }

    fn draw_box_shadow(&mut self, transform: Affine, rect: Rect, brush: Color, radius: f64, std_dev: f64) {
        self.push(Draw::BoxShadow { rect: [rect.x0, rect.y0, rect.x1, rect.y1], radius, std_dev, color: color(brush), transform: mat(transform) });
    }
}

/// Ink bounds `[x, y, w, h]` (scene coordinates) of a display list. Glyph runs use the
/// fonts' outline bounds; blurs and shadows expand by 3 sigma. With `clips` false, layer
/// clips are ignored (the unclipped ink, for "is content hidden by a clip" checks).
pub fn bounds(items: &[Draw], fonts: &dyn Fn(FontId) -> Option<FontData>, clips: bool) -> [f64; 4] {
    // stack of (bounds accumulated inside the group, clip rect in scene space, filter)
    let mut stack: Vec<(Rect, Option<Rect>, Option<DrawFilter>, Affine)> = vec![(Rect::ZERO, None, None, Affine::IDENTITY)];
    let add = |stack: &mut Vec<(Rect, Option<Rect>, Option<DrawFilter>, Affine)>, r: Rect| {
        if r.width() <= 0.0 || r.height() <= 0.0 {
            return;
        }
        let top = stack.last_mut().unwrap();
        top.0 = if top.0.area() == 0.0 { r } else { top.0.union(r) };
    };
    for d in items {
        match d {
            Draw::Fill { shape, transform, .. } => add(&mut stack, aff(transform).transform_rect_bbox(shape_rect(shape))),
            Draw::Stroke { shape, transform, style, .. } => {
                let w = style.width * if style.join == "miter" { style.miter_limit.max(1.0) } else { 1.0 } / 2.0;
                add(&mut stack, aff(transform).transform_rect_bbox(shape_rect(shape).inflate(w, w)))
            }
            Draw::BoxShadow { rect, std_dev, transform, .. } => {
                let e = 3.0 * std_dev;
                add(&mut stack, aff(transform).transform_rect_bbox(Rect::new(rect[0], rect[1], rect[2], rect[3]).inflate(e, e)))
            }
            Draw::Glyphs { font, size, coords, transform, glyph_transform, glyphs, stroke, .. } => {
                let fd = fonts(*font);
                let mut gb = glyph_bounds(fd.as_ref(), *size, coords, glyphs, glyph_transform.map(|m| aff(&m)));
                if let Some(s) = stroke {
                    let w = s.width / 2.0 * if s.join == "miter" { 4.0 } else { 1.0 };
                    gb = gb.inflate(w, w);
                }
                add(&mut stack, aff(transform).transform_rect_bbox(gb));
            }
            Draw::Push { clip, transform, filter, .. } => {
                let c = clip.as_ref().map(|s| aff(transform).transform_rect_bbox(shape_rect(s)));
                stack.push((Rect::ZERO, c, filter.clone(), aff(transform)));
            }
            Draw::Pop => {
                if stack.len() < 2 {
                    continue;
                }
                let (mut b, clip, filter, t) = stack.pop().unwrap();
                if b.area() == 0.0 {
                    continue;
                }
                if let Some(f) = filter {
                    let s = t.as_coeffs()[0].hypot(t.as_coeffs()[1]).max(t.as_coeffs()[2].hypot(t.as_coeffs()[3]));
                    match f {
                        DrawFilter::Blur { std_dev } => b = b.inflate(3.0 * std_dev * s, 3.0 * std_dev * s),
                        DrawFilter::DropShadow { dx, dy, std_dev, .. } => {
                            let e = 3.0 * std_dev * s;
                            b = b.union((b + kurbo::Vec2::new(dx * s, dy * s)).inflate(e, e));
                        }
                    }
                }
                if clips {
                    if let Some(c) = clip {
                        b = b.intersect(c);
                    }
                }
                add(&mut stack, b);
            }
        }
    }
    let r = stack[0].0;
    if r.area() <= 0.0 { [0.0; 4] } else { [r.x0, r.y0, r.width(), r.height()] }
}

pub fn aff(m: &Mat) -> Affine {
    Affine::new(*m)
}

pub fn shape_rect(s: &Shape) -> Rect {
    match s {
        Shape::Rect(r) => Rect::new(r[0], r[1], r[2], r[3]),
        Shape::RoundedRect { rect: r, .. } => Rect::new(r[0], r[1], r[2], r[3]),
        Shape::Path(d) => kurbo::BezPath::from_svg(d).map(|p| p.bounding_box()).unwrap_or(Rect::ZERO),
    }
}

/// Bounds of positioned glyphs (run space). Without font data: 1 em around each origin.
pub fn glyph_bounds(font: Option<&FontData>, size: f32, coords: &[i16], glyphs: &[(u32, f32, f32)], glyph_t: Option<Affine>) -> Rect {
    use skrifa::MetadataProvider;
    let mut r = Rect::ZERO;
    let mut push = |b: Rect| r = if r.area() == 0.0 { b } else { r.union(b) };
    let fref = font.and_then(|f| skrifa::FontRef::from_index(f.data.data(), f.index).ok());
    let loc: Vec<skrifa::instance::NormalizedCoord> = coords.iter().map(|c| skrifa::instance::NormalizedCoord::from_bits(*c)).collect();
    let metrics = fref.as_ref().map(|f| f.glyph_metrics(skrifa::instance::Size::new(size), skrifa::instance::LocationRef::new(&loc)));
    for &(id, x, y) in glyphs {
        let b = metrics.as_ref().and_then(|m| m.bounds(skrifa::GlyphId::new(id)));
        // glyph space is y-up; text runs are drawn y-down
        let gb = match b {
            Some(b) => Rect::new(b.x_min as f64, -b.y_max as f64, b.x_max as f64, -b.y_min as f64),
            None => Rect::new(0.0, -(size as f64), size as f64, 0.3 * size as f64),
        };
        let gb = match glyph_t {
            Some(t) => t.transform_rect_bbox(gb),
            None => gb,
        };
        push(gb + kurbo::Vec2::new(x as f64, y as f64));
    }
    r.inflate(1.0, 1.0)
}
