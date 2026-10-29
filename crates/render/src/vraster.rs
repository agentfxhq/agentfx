//! Native renderer on vello_cpu: IR (display lists, text, paths, rasters) + schedule ->
//! pixels of the damaged rects of a frame. Node effects (blur, drop shadow, glow, matte)
//! and motion blur render the node's subtree offscreen and composite the result; the
//! main context therefore never uses vello filter layers and can run multithreaded.
use crate::effects;
use crate::pathx::{self, Measured};
use crate::record::{aff, shape_rect};
use crate::schedule::{self, ContentInfo, NodeState, Schedule, intersect, transform_bbox, union};
use crate::text::{Shaped, Shaper};
use anyhow::{Context, Result, bail};
use kurbo::{Affine, BezPath, Rect, Shape as _};
use peniko::{BlendMode, Compose, Fill, FontData, Mix};
use agentfx_ir::{Brush, Content, Draw, DrawFilter, Effect, Extend, NodeId, Sampled, Scene, Shape};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use vello_cpu::{ImageSource, PaintType, Pixmap, RenderContext, RenderSettings, Resources};

type Paint = PaintType;

pub enum PDraw {
    /// `bb`: ink bounds `[x, y, w, h]` in content space (culling against damaged rects)
    Fill { path: BezPath, paint: Paint, t: Affine, pt: Affine, rule: Fill, bb: [f64; 4] },
    Stroke { path: BezPath, paint: Paint, stroke: kurbo::Stroke, t: Affine, pt: Affine, bb: [f64; 4] },
    /// `stroke`: text stroke (style, paint, drawn over the fill)
    Glyphs { font: usize, size: f32, coords: Vec<i16>, paint: Paint, t: Affine, gt: Option<Affine>, glyphs: Vec<vello_cpu::Glyph>, bb: [f64; 4], stroke: Option<Box<(kurbo::Stroke, Paint, bool)>> },
    Shadow { rect: Rect, radius: f32, std_dev: f32, color: peniko::Color, t: Affine, bb: [f64; 4] },
    /// `len` = number of items up to the matching Pop (relative, so slices stay valid);
    /// `bounds` = scene-space ink of the group's items
    Push { clip: Option<BezPath>, t: Affine, alpha: f32, blend: BlendMode, filter: Option<DrawFilter>, rule: Fill, len: usize, bounds: [f64; 4] },
    Pop,
}

/// Skia text contrast (`SK_GAMMA_CONTRAST`, Chrome Linux 0.2); AGENTFX_TEXT_CONTRAST overrides.
fn text_contrast() -> f32 { agentfx_ir::env::var("AGENTFX_TEXT_CONTRAST").ok().and_then(|v| v.parse().ok()).unwrap_or(0.2) }
/// Skia text gamma (`SK_GAMMA_EXPONENT`, Chrome Linux 1.2); AGENTFX_TEXT_GAMMA overrides.
fn text_gamma() -> f32 { agentfx_ir::env::var("AGENTFX_TEXT_GAMMA").ok().and_then(|v| v.parse().ok()).unwrap_or(1.2) }

/// Opacity at or above this (rounds to 255/255) is drawn as fully opaque.
const OPAQUE: f64 = 1.0 - 0.5 / 255.0;

pub struct PText {
    pub shaped: Shaped,
    pub paint: Paint,
    pub solid: Option<[f32; 4]>,
    pub font: usize,
}

pub struct PPath {
    pub m: Measured,
    pub fill: Option<Paint>,
    /// brush transform of the fill (identity = none)
    pub fill_pt: Affine,
    pub stroke: Option<(kurbo::Stroke, Paint)>,
    pub bounds: [f64; 4],
    pub trim: [f64; 2],
}

pub enum PContent {
    /// rasters (Image/Sequence): per asset image; drawn asset chosen by the schedule
    Raster { rect: [f64; 4] },
    Rect { path: BezPath, color: peniko::Color },
    List(Vec<PDraw>),
    Switch(Vec<Vec<PDraw>>),
    /// text v2: a paint list with animated units / path / colors (base list converted at rest)
    TextFx(Box<(crate::textfx::Prep, Vec<PDraw>)>),
    Text(PText),
    Path(PPath),
    Cloud(Box<crate::cloud::Cloud>),
    Fluid(Box<crate::fluid::Fluid>),
}

pub struct Prepared {
    pub fonts: Vec<FontData>,
    pub images: Vec<Option<Arc<Pixmap>>>,
    pub image_scale: Vec<f64>,
    pub contents: Vec<Option<PContent>>,
}

fn color(c: [f32; 4]) -> peniko::Color {
    peniko::Color::new(c)
}

fn ext(e: Extend) -> peniko::Extend {
    match e {
        Extend::Pad => peniko::Extend::Pad,
        Extend::Repeat => peniko::Extend::Repeat,
        Extend::Reflect => peniko::Extend::Reflect,
    }
}

fn blend(b: agentfx_ir::BlendMode) -> BlendMode {
    use agentfx_ir::BlendMode as B;
    match b {
        B::Normal => BlendMode::new(Mix::Normal, Compose::SrcOver),
        B::Multiply => BlendMode::new(Mix::Multiply, Compose::SrcOver),
        B::Screen => BlendMode::new(Mix::Screen, Compose::SrcOver),
        B::Overlay => BlendMode::new(Mix::Overlay, Compose::SrcOver),
        B::Darken => BlendMode::new(Mix::Darken, Compose::SrcOver),
        B::Lighten => BlendMode::new(Mix::Lighten, Compose::SrcOver),
        B::Plus => BlendMode::new(Mix::Normal, Compose::Plus),
        B::ColorDodge => BlendMode::new(Mix::ColorDodge, Compose::SrcOver),
        B::ColorBurn => BlendMode::new(Mix::ColorBurn, Compose::SrcOver),
        B::HardLight => BlendMode::new(Mix::HardLight, Compose::SrcOver),
        B::SoftLight => BlendMode::new(Mix::SoftLight, Compose::SrcOver),
        B::Difference => BlendMode::new(Mix::Difference, Compose::SrcOver),
        B::Exclusion => BlendMode::new(Mix::Exclusion, Compose::SrcOver),
        B::Hue => BlendMode::new(Mix::Hue, Compose::SrcOver),
        B::Saturation => BlendMode::new(Mix::Saturation, Compose::SrcOver),
        B::Color => BlendMode::new(Mix::Color, Compose::SrcOver),
        B::Luminosity => BlendMode::new(Mix::Luminosity, Compose::SrcOver),
        B::SrcAtop => BlendMode::new(Mix::Normal, Compose::SrcAtop),
    }
}

pub fn load_png_premul(path: &Path) -> Result<Pixmap> {
    let f = std::io::BufReader::new(std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?);
    Pixmap::from_png(f).map_err(|e| anyhow::anyhow!("decoding {}: {e}", path.display()))
}

fn path_of(s: &Shape) -> Result<BezPath> {
    Ok(match s {
        Shape::Rect(r) => Rect::new(r[0], r[1], r[2], r[3]).to_path(0.1),
        Shape::RoundedRect { rect: r, radii } => kurbo::RoundedRect::from_rect(Rect::new(r[0], r[1], r[2], r[3]), kurbo::RoundedRectRadii::new(radii[0], radii[1], radii[2], radii[3])).to_path(0.1),
        Shape::Path(d) => BezPath::from_svg(d).map_err(|e| anyhow::anyhow!("bad path data: {e:?}"))?,
    })
}

impl Prepared {
    /// Load fonts/images and convert every node's content into drawable form.
    /// `mem_fonts` / `mem_images`: already-decoded resources from an in-process frontend.
    pub fn new(scene: &Scene, base: &Path, mem_fonts: Option<&[FontData]>, mem_images: Option<&[Option<Arc<peniko::ImageData>>]>) -> Result<Prepared> {
        crate::color::preload(scene, base)?;
        let mut fonts = Vec::new();
        for (i, f) in scene.fonts.iter().enumerate() {
            if let Some(m) = mem_fonts.and_then(|m| m.get(i)) {
                fonts.push(m.clone());
                continue;
            }
            let data = std::fs::read(base.join(&f.path)).with_context(|| format!("font {i} {}", f.path))?;
            fonts.push(FontData::new(peniko::Blob::new(Arc::new(data)), f.index));
        }
        // images: only the assets referenced
        let mut images: Vec<Option<Arc<Pixmap>>> = vec![None; scene.assets.len()];
        let image_scale: Vec<f64> = scene.assets.iter().map(|a| a.scale).collect();
        let mut used = vec![false; scene.assets.len()];
        let mark = |c: &Content, used: &mut Vec<bool>| {
            fn walk(c: &Content, used: &mut Vec<bool>) {
                match c {
                    Content::Image { asset, variants, .. } => {
                        used[*asset] = true;
                        for v in variants {
                            used[*v] = true;
                        }
                    }
                    Content::Sequence { frames, .. } => frames.iter().for_each(|f| used[f.1] = true),
                    Content::Paint { items, .. } => {
                        for d in items {
                            if let Draw::Fill { brush: Brush::Image { asset, .. }, .. } | Draw::Stroke { brush: Brush::Image { asset, .. }, .. } | Draw::Glyphs { brush: Brush::Image { asset, .. }, .. } = d {
                                used[*asset] = true;
                            }
                        }
                    }
                    Content::Path(p) => {
                        for b in p.fill.iter().chain(p.stroke.as_ref().map(|s| &s.brush)) {
                            if let Brush::Image { asset, .. } = b {
                                used[*asset] = true;
                            }
                        }
                    }
                    Content::Text(t) => {
                        if let Brush::Image { asset, .. } = &t.brush {
                            used[*asset] = true;
                        }
                    }
                    Content::Switch { states, .. } => states.iter().for_each(|s| walk(s, used)),
                    _ => {}
                }
            }
            walk(c, used)
        };
        for n in &scene.nodes {
            if let Some(c) = &n.content {
                mark(c, &mut used);
            }
        }
        let loaded: Vec<(usize, Result<Pixmap>)> = {
            use rayon::prelude::*;
            (0..scene.assets.len())
                .into_par_iter()
                .filter(|&i| used[i])
                .map(|i| {
                    if let Some(Some(img)) = mem_images.and_then(|m| m.get(i)) {
                        let px = Pixmap::from_parts(img.data.data().to_vec(), img.width as u16, img.height as u16, vello_cpu::PixelMetadata { alpha_type: peniko::ImageAlphaType::Alpha, may_have_transparency: true });
                        return (i, Ok(px));
                    }
                    (i, load_png_premul(&base.join(&scene.assets[i].path)))
                })
                .collect()
        };
        for (i, r) in loaded {
            images[i] = Some(Arc::new(r?));
        }
        let mut shaper: Option<Shaper> = None;
        let mut contents = Vec::with_capacity(scene.nodes.len());
        for (ni, node) in scene.nodes.iter().enumerate() {
            let c = match &node.content {
                None => None,
                Some(Content::Image { rect, .. }) | Some(Content::Sequence { rect, .. }) => Some(PContent::Raster { rect: *rect }),
                Some(Content::Rect { rect, color: c }) => {
                    let r = rect.rect;
                    let path = kurbo::RoundedRect::from_rect(Rect::new(r[0], r[1], r[0] + r[2], r[1] + r[3]), kurbo::RoundedRectRadii::new(rect.radius[0], rect.radius[1], rect.radius[2], rect.radius[3])).to_path(0.1);
                    Some(PContent::Rect { path, color: color([c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32]) })
                }
                Some(Content::Paint { items, text, .. }) if text.is_some() || scene.tracks.iter().any(|t| t.node == ni && (t.property.starts_with("text.") || t.property.starts_with("animators."))) => {
                    let base = convert(items, &images, &fonts).with_context(|| format!("node {ni} {:?}", node.name))?;
                    Some(PContent::TextFx(Box::new((crate::textfx::Prep::new(items, text.as_deref(), &fonts), base))))
                }
                Some(Content::Paint { items, .. }) => Some(PContent::List(convert(items, &images, &fonts).with_context(|| format!("node {ni} {:?}", node.name))?)),
                Some(Content::Switch { states, .. }) => {
                    let mut out = Vec::new();
                    for (k, s) in states.iter().enumerate() {
                        match s {
                            Content::Paint { items, .. } => out.push(convert(items, &images, &fonts).with_context(|| format!("node {ni} state {k}"))?),
                            _ => bail!("node {ni}: switch states must be paint lists"),
                        }
                    }
                    Some(PContent::Switch(out))
                }
                Some(Content::Text(t)) => {
                    if shaper.is_none() {
                        shaper = Some(Shaper::new(&fonts)?);
                    }
                    let shaped = shaper.as_mut().unwrap().shape(t).with_context(|| format!("node {ni} text"))?;
                    let solid = if let Brush::Solid(c) = t.brush { Some(c) } else { None };
                    Some(PContent::Text(PText { paint: brush(&t.brush, &images)?, solid, font: t.font, shaped }))
                }
                Some(Content::Path(p)) => {
                    let path = BezPath::from_svg(&p.d).map_err(|e| anyhow::anyhow!("node {ni}: bad path: {e:?}"))?;
                    let mut b = path.bounding_box();
                    let stroke = match &p.stroke {
                        Some(s) => {
                            let st = stroke_of(&s.style);
                            let w = s.style.width / 2.0 * if s.style.join == "miter" { s.style.miter_limit.max(1.0) } else { 1.0 };
                            b = b.inflate(w, w);
                            Some((st, brush(&s.brush, &images)?))
                        }
                        None => None,
                    };
                    let fill = match &p.fill {
                        Some(f) => Some(brush(f, &images)?),
                        None => None,
                    };
                    Some(PContent::Path(PPath { m: pathx::measure(path), fill, fill_pt: p.fill_transform.map(|m| aff(&m)).unwrap_or(Affine::IDENTITY), stroke, bounds: [b.x0, b.y0, b.width(), b.height()], trim: p.trim }))
                }
                Some(Content::Cloud(c)) => {
                    if shaper.is_none() && c.forms.iter().any(|f| matches!(f.shape, agentfx_ir::CloudShape::Text(_))) {
                        shaper = Some(Shaper::new(&fonts)?);
                    }
                    let mut sampler = |t: &agentfx_ir::CloudText, n: usize, seed: u64| cloud_text_points(shaper.as_mut().context("no shaper")?, &fonts, t, n, seed);
                    Some(PContent::Cloud(Box::new(crate::cloud::Cloud::new(c, &mut sampler).with_context(|| format!("node {ni} cloud"))?)))
                }
                Some(Content::Fluid(c)) => Some(PContent::Fluid(Box::new(crate::fluid::Fluid::new(c, scene.fps as f64, scene.frames as usize)))),
            };
            contents.push(c);
        }
        Ok(Prepared { fonts, images, image_scale, contents })
    }
}

/// Points inside / on the outline of shaped text: the glyphs rasterized into a coverage mask
/// (resolution from the expected point spacing), then sampled (`cloud::sample_mask`).
fn cloud_text_points(shaper: &mut Shaper, fonts: &[FontData], t: &agentfx_ir::CloudText, n: usize, seed: u64) -> Result<Vec<[f64; 2]>> {
    let tc = agentfx_ir::TextContent {
        text: t.text.clone(),
        font: t.font,
        size: t.size,
        weight: t.weight,
        letter_spacing: t.letter_spacing,
        line_height: t.line_height,
        pos: t.pos,
        max_width: t.max_width,
        align: t.align.clone(),
        brush: Brush::Solid([1.0; 4]),
        unit: "glyph".into(),
    };
    let sh = shaper.shape(&tc)?;
    let mut b = [0.0f64; 4];
    for g in &sh.glyphs {
        b = union(b, g.bounds);
    }
    anyhow::ensure!(b[2] > 0.0 && b[3] > 0.0, "text has no ink");
    // ~3 mask px per point spacing (ink covers roughly a third of the box), capped at 4096 px
    let sp = (b[2] * b[3] * 0.35 / n.max(1) as f64).sqrt();
    let s = (4.0 / sp.max(1e-3)).clamp(0.25, 4.0).min(4096.0 / b[2].max(b[3]));
    let (mw, mh) = (((b[2] * s).ceil() as usize + 4).min(8192), ((b[3] * s).ceil() as usize + 4).min(8192));
    let (ox, oy) = (b[0] - 2.0 / s, b[1] - 2.0 / s);
    let mut ctx = RenderContext::new(mw as u16, mh as u16);
    ctx.set_transform(Affine::scale(s) * Affine::translate((-ox, -oy)));
    ctx.set_paint(Paint::Solid(peniko::Color::WHITE));
    ctx.set_fill_rule(Fill::NonZero);
    let mut res = Resources::new();
    let _ = ctx
        .glyph_run(&mut res, &fonts[t.font])
        .font_size(sh.size)
        .hint(false)
        .normalized_coords(&sh.coords)
        .fill_glyphs(sh.glyphs.iter().map(|g| vello_cpu::Glyph { id: g.id, x: g.x, y: g.y }));
    ctx.flush();
    let mut px = Pixmap::new(mw as u16, mh as u16);
    ctx.render(&mut px, &mut res);
    let alpha: Vec<u8> = px.data().iter().map(|p| p.a).collect();
    Ok(crate::cloud::sample_mask(&alpha, mw, mh, ox, oy, s, n, t.outline, seed))
}

fn stroke_of(s: &agentfx_ir::StrokeStyle) -> kurbo::Stroke {
    let cap = match s.cap.as_str() {
        "round" => kurbo::Cap::Round,
        "square" => kurbo::Cap::Square,
        _ => kurbo::Cap::Butt,
    };
    let join = match s.join.as_str() {
        "round" => kurbo::Join::Round,
        "bevel" => kurbo::Join::Bevel,
        _ => kurbo::Join::Miter,
    };
    let mut st = kurbo::Stroke::new(s.width).with_caps(cap).with_join(join).with_miter_limit(s.miter_limit);
    if !s.dash.is_empty() {
        st = st.with_dashes(s.dash_offset, s.dash.iter().copied());
    }
    st
}

fn stops(s: &[agentfx_ir::Stop]) -> peniko::ColorStops {
    let v: Vec<peniko::ColorStop> = s.iter().map(|s| peniko::ColorStop { offset: s.offset, color: peniko::color::DynamicColor::from_alpha_color(color(s.color)) }).collect();
    peniko::ColorStops(v.into_iter().collect())
}

fn brush(b: &Brush, images: &[Option<Arc<Pixmap>>]) -> Result<Paint> {
    Ok(match b {
        Brush::Solid(c) => Paint::Solid(color(*c)),
        Brush::Linear { start, end, stops: s, extend } => {
            let mut g = peniko::Gradient::new_linear((start[0], start[1]), (end[0], end[1])).with_extend(ext(*extend));
            g.stops = stops(s);
            Paint::Gradient(g)
        }
        Brush::Radial { start_center, start_radius, end_center, end_radius, stops: s, extend } => {
            let mut g = peniko::Gradient::new_two_point_radial((start_center[0], start_center[1]), *start_radius, (end_center[0], end_center[1]), *end_radius).with_extend(ext(*extend));
            g.stops = stops(s);
            Paint::Gradient(g)
        }
        Brush::Sweep { center, start_angle, end_angle, stops: s, extend } => {
            let mut g = peniko::Gradient::new_sweep((center[0], center[1]), *start_angle, *end_angle).with_extend(ext(*extend));
            g.stops = stops(s);
            Paint::Gradient(g)
        }
        Brush::Image { asset, quality, extend, alpha } => {
            let px = images.get(*asset).and_then(|x| x.clone()).with_context(|| format!("image asset {asset} not loaded"))?;
            Paint::Image(peniko::ImageBrush {
                image: ImageSource::Pixmap(px),
                sampler: peniko::ImageSampler {
                    x_extend: ext(extend[0]),
                    y_extend: ext(extend[1]),
                    quality: match quality {
                        0 => peniko::ImageQuality::Low,
                        1 => peniko::ImageQuality::Medium,
                        _ => peniko::ImageQuality::High,
                    },
                    alpha: *alpha,
                },
            })
        }
    })
}

fn convert(items: &[Draw], images: &[Option<Arc<Pixmap>>], fonts: &[FontData]) -> Result<Vec<PDraw>> {
    let mut out = Vec::with_capacity(items.len());
    let mut stack: Vec<usize> = Vec::new();
    let mut pops: Vec<(usize, usize)> = Vec::new();
    let fl = |f: usize| fonts.get(f).cloned();
    for (i, d) in items.iter().enumerate() {
        let bb = if matches!(d, Draw::Push { .. } | Draw::Pop) { [0.0; 4] } else { crate::record::bounds(std::slice::from_ref(d), &fl, true) };
        out.push(match d {
            Draw::Fill { shape, brush: b, transform, brush_transform, even_odd } => PDraw::Fill {
                path: path_of(shape)?,
                paint: brush(b, images)?,
                t: aff(transform),
                pt: brush_transform.map(|m| aff(&m)).unwrap_or(Affine::IDENTITY),
                rule: if *even_odd { Fill::EvenOdd } else { Fill::NonZero },
                bb,
            },
            Draw::Stroke { shape, brush: b, style, transform, brush_transform } => PDraw::Stroke {
                path: path_of(shape)?,
                paint: brush(b, images)?,
                stroke: stroke_of(style),
                t: aff(transform),
                pt: brush_transform.map(|m| aff(&m)).unwrap_or(Affine::IDENTITY),
                bb,
            },
            Draw::Glyphs { font, size, coords, embolden, brush: b, transform, glyph_transform, glyphs, stroke, .. } => {
                if embolden.is_some() {
                    bail!("synthetic embolden unsupported");
                }
                if *font >= fonts.len() {
                    bail!("font {font} out of range");
                }
                PDraw::Glyphs {
                    font: *font,
                    size: *size,
                    coords: coords.clone(),
                    paint: brush(b, images)?,
                    t: aff(transform),
                    gt: glyph_transform.map(|m| aff(&m)),
                    glyphs: glyphs.iter().map(|&(id, x, y)| vello_cpu::Glyph { id, x, y }).collect(),
                    bb,
                    stroke: match stroke {
                        Some(st) => {
                            let join = match st.join.as_str() {
                                "miter" => kurbo::Join::Miter,
                                "bevel" => kurbo::Join::Bevel,
                                _ => kurbo::Join::Round,
                            };
                            Some(Box::new((kurbo::Stroke::new(st.width).with_join(join).with_caps(kurbo::Cap::Round), brush(&st.brush, images)?, st.over)))
                        }
                        None => None,
                    },
                }
            }
            Draw::BoxShadow { rect, radius, std_dev, color: c, transform } => {
                PDraw::Shadow { rect: Rect::new(rect[0], rect[1], rect[2], rect[3]), radius: *radius as f32, std_dev: *std_dev as f32, color: color(*c), t: aff(transform), bb }
            }
            Draw::Push { clip, transform, alpha, blend: b, filter, even_odd } => {
                stack.push(i);
                PDraw::Push {
                    clip: clip.as_ref().map(path_of).transpose()?,
                    t: aff(transform),
                    alpha: *alpha,
                    blend: blend(*b),
                    filter: filter.clone(),
                    rule: if *even_odd { Fill::EvenOdd } else { Fill::NonZero },
                    len: 0,
                    bounds: [0.0; 4],
                }
            }
            Draw::Pop => {
                let s = stack.pop().context("pop without push")?;
                pops.push((s, i));
                PDraw::Pop
            }
        });
    }
    if !stack.is_empty() {
        bail!("unbalanced push/pop");
    }
    for (s, i) in pops {
        let inner = crate::record::bounds(&items[s + 1..i], &fl, true);
        if let PDraw::Push { len, bounds, .. } = &mut out[s] {
            *len = i - s;
            *bounds = inner;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- content info

fn glyph_tracks(scene: &Scene, sched_tracks: &[usize]) -> Vec<usize> {
    sched_tracks.iter().copied().filter(|&i| scene.tracks[i].property.starts_with("glyph.")).collect()
}

/// Per-unit (opacity, transform about the unit origin) at time t.
pub fn glyph_state(scene: &Scene, tracks: &[usize], units: usize, unit: usize, origin: (f64, f64), t: f64) -> (f64, Affine) {
    let mut o = 1.0;
    let mut m: Option<Affine> = None;
    for &i in tracks {
        let tr = &scene.tracks[i];
        let delay = match &tr.stagger {
            Some(s) => {
                let k = match s.from.as_str() {
                    "end" => (units - 1 - unit) as f64,
                    "center" => (unit as f64 - (units as f64 - 1.0) / 2.0).abs(),
                    _ => unit as f64,
                };
                k * s.each
            }
            None => 0.0,
        };
        match (tr.property.as_str(), agentfx_ir::eval::eval_track(tr, t - delay)) {
            ("glyph.opacity", Some(Sampled::Number(v))) => o = v,
            ("glyph.transform", Some(Sampled::Matrix(x))) => m = Some(k(&x)),
            _ => {}
        }
    }
    let a = match m {
        Some(x) => Affine::translate(origin) * x * Affine::translate((-origin.0, -origin.1)),
        None => Affine::IDENTITY,
    };
    (o, a)
}

/// Text v2 state of a `TextFx` node at t (its `text.*` / `animators.*` tracks).
pub fn text_state(scene: &Scene, tracks: &[usize], p: &crate::textfx::Prep, t: f64) -> crate::textfx::State {
    p.state(t, &|prop, base| schedule::number_at(scene, tracks, prop, t, base), &|prop| schedule::color_at(scene, tracks, prop, t))
}

/// Simulation time a fluid shows at scene time t: its `fluid.time` track (rewinds, time
/// maps), else t + time_offset.
pub fn fluid_time(scene: &Scene, tracks: &[usize], fl: &crate::fluid::Fluid, t: f64) -> f64 {
    schedule::number_at(scene, tracks, "fluid.time", t, t + fl.c.time_offset)
}

pub struct Info<'a> {
    pub prep: &'a Prepared,
    pub tracks: Vec<Vec<usize>>,
}

impl<'a> Info<'a> {
    pub fn new(scene: &Scene, prep: &'a Prepared) -> Self {
        let mut tracks: Vec<Vec<usize>> = vec![Vec::new(); scene.nodes.len()];
        for (i, t) in scene.tracks.iter().enumerate() {
            tracks[t.node].push(i);
        }
        for v in &mut tracks {
            v.sort_by_key(|&i| scene.tracks[i].order);
        }
        Info { prep, tracks }
    }
}

impl ContentInfo for Info<'_> {
    fn bounds(&self, scene: &Scene, n: NodeId, t: f64) -> Option<[f64; 4]> {
        match self.prep.contents[n].as_ref()? {
            PContent::Text(pt) => {
                let gt = glyph_tracks(scene, &self.tracks[n]);
                let mut r = [0.0; 4];
                for g in &pt.shaped.glyphs {
                    let (o, a) = if gt.is_empty() { (1.0, Affine::IDENTITY) } else { glyph_state(scene, &gt, pt.shaped.units, g.unit, g.origin, t) };
                    if o > 0.0 {
                        r = union(r, transform_bbox(&ra(a), g.bounds));
                    }
                }
                (r[2] > 0.0).then_some(r)
            }
            PContent::Path(p) => Some(p.bounds),
            PContent::TextFx(b) => {
                let st = text_state(scene, &self.tracks[n], &b.0, t);
                if b.0.is_rest(&st) {
                    return schedule::content_rect(scene.nodes[n].content.as_ref()?);
                }
                let fl = |f: usize| self.prep.fonts.get(f).cloned();
                let r = crate::record::bounds(&b.0.frame(&st), &fl, true);
                (r[2] > 0.0).then_some(r)
            }
            PContent::Cloud(c) => c.bounds(t),
            PContent::Fluid(fl) => fl.bounds(fluid_time(scene, &self.tracks[n], fl, t)),
            _ => schedule::content_rect(scene.nodes[n].content.as_ref()?),
        }
    }
    fn sig(&self, scene: &Scene, n: NodeId, t: f64) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self.prep.contents[n].as_ref() {
            Some(PContent::Text(pt)) => {
                let gt = glyph_tracks(scene, &self.tracks[n]);
                if !gt.is_empty() {
                    for g in &pt.shaped.glyphs {
                        let (o, a) = glyph_state(scene, &gt, pt.shaped.units, g.unit, g.origin, t);
                        o.to_bits().hash(&mut h);
                        for c in a.as_coeffs_f() {
                            c.to_bits().hash(&mut h);
                        }
                    }
                }
            }
            Some(PContent::Path(p)) => {
                let a = schedule::number_at(scene, &self.tracks[n], "trim-start", t, p.trim[0]);
                let b = schedule::number_at(scene, &self.tracks[n], "trim-end", t, p.trim[1]);
                a.to_bits().hash(&mut h);
                b.to_bits().hash(&mut h);
                for prop in ["fill.color", "stroke.color"] {
                    if let Some(c) = schedule::color_at(scene, &self.tracks[n], prop, t) {
                        c.map(f64::to_bits).hash(&mut h);
                    }
                }
            }
            Some(PContent::TextFx(b)) => text_state(scene, &self.tracks[n], &b.0, t).hash_into(&mut h),
            Some(PContent::Cloud(c)) => c.sig(t).hash(&mut h),
            Some(PContent::Fluid(fl)) => fl.sig(fluid_time(scene, &self.tracks[n], fl, t)).hash(&mut h),
            _ => {}
        }
        h.finish()
    }
    fn time_dependent(&self, _scene: &Scene, n: NodeId) -> bool {
        // text glyph tracks and path trims are the node's own tracks (covered by the
        // planner's activity intervals); clouds and fluids change with t itself
        match self.prep.contents[n].as_ref() {
            Some(PContent::Cloud(_) | PContent::Fluid(_)) => true,
            // per-unit key times depend on t itself
            Some(PContent::TextFx(b)) => b.0.keyed,
            _ => false,
        }
    }
}

trait Coeffs {
    fn as_coeffs_f(&self) -> [f64; 6];
}
impl Coeffs for Affine {
    fn as_coeffs_f(&self) -> [f64; 6] {
        self.as_coeffs()
    }
}

// ---------------------------------------------------------------- drawing

fn ra(a: Affine) -> agentfx_ir::Affine {
    let c = a.as_coeffs();
    agentfx_ir::Affine { a: c[0], b: c[1], c: c[2], d: c[3], e: c[4], f: c[5] }
}

fn k(a: &agentfx_ir::Affine) -> Affine {
    Affine::new([a.a, a.b, a.c, a.d, a.e, a.f])
}

/// What one frame (or one motion-blur sample) shows.
pub struct View<'a> {
    pub t: f64,
    pub states: &'a [NodeState],
    pub content: &'a [Option<usize>],
}

pub struct Painter<'a> {
    pub scene: &'a Scene,
    pub prep: &'a Prepared,
    pub sched: &'a Schedule,
    pub info: &'a Info<'a>,
    pub res: Resources,
    pub threads: u16,
    ctxs: Vec<RenderContext>,
    pub stats: PainterStats,
    /// static-content raster cache (nodes at rest): exact for the same transform
    cacheable: Vec<bool>,
    cache: std::collections::HashMap<(NodeId, usize, [u64; 6]), (Arc<Pixmap>, i32, i32)>,
    /// sightings of a cache key: (count over distinct frames, last frame time bits)
    seen: std::collections::HashMap<(NodeId, usize, [u64; 6]), (u32, u64)>,
    /// rasterized box shadows (blurred rounded rects), keyed by geometry/colour/scale
    shadow_cache: std::collections::HashMap<[u64; 9], (Arc<Pixmap>, f64, f64)>,
    cache_bytes: usize,
    pub use_cache: bool,
    /// subpixel quantization of cached content offsets (per px; 0 = exact)
    pub quant: f64,
    pub shadow_cache_on: bool,
    /// LCD subpixel text (Chrome on Linux); AGENTFX_LCD=0 disables
    pub lcd: bool,
    /// depth of isolating layers / offscreen targets (LCD text needs an opaque backdrop)
    iso: u32,
    /// Skia mask gamma/contrast for glyph coverage (LCD and, if `a8_gamma`, grayscale)
    mask_gamma: crate::lcd::MaskGamma,
    text_gamma: f32,
    /// grayscale glyph runs through the gamma-corrected mask path (AGENTFX_A8_GAMMA=0 disables)
    pub a8_gamma: bool,
    /// Chrome parity: grayscale text under a scale through distance fields (`sdft.rs`)
    pub sdft: bool,
    /// dithered gradients (AGENTFX_DITHER=0 disables), see `dither.rs`
    pub dither: bool,
    /// screen position of the current context's pixel (0, 0) (dither anchor)
    origin: (i64, i64),
    grad_cache: crate::dither::Cache,
    /// LCD run images (multiply, plus) keyed by run content and device placement
    lcd_cache: std::collections::HashMap<u64, (Arc<Pixmap>, Arc<Pixmap>)>,
    lcd_cache_px: usize,
    /// AGENTFX_SKIP (debug): item kinds not drawn
    pub skip: String,
    /// > 0 while a matte effect draws its source (fluids draw their matte coverage)
    pub(crate) fluid_matte: u32,
    fx_cache: std::collections::HashMap<u64, (Arc<Pixmap>, i32, i32, f64, f64)>,
    /// time effects (X3): sample rasters keyed by (subtree state, screen rect, matte pass)
    time_cache: std::collections::HashMap<(u64, [i32; 4], bool), Arc<Pixmap>>,
    time_cache_bytes: usize,
    /// time ring on the GPU (`timefx_paint::ring_pixmap`, E4 phase 2)
    time_ring: Option<crate::gpu::time::Ring>,
    /// AGENTFX_TIME_FX=0 draws time-effect nodes plainly (cost comparison)
    pub time_on: bool,
    /// tile size (px) of time-displacement renders (AGENTFX_TIME_TILE)
    pub time_tile: usize,
    /// slit-scans in two passes (even / odd levels); false = one render per level run
    pub slit_two_pass: bool,
    /// Chrome/Skia parity: glyph origins on a 1/4 px x grid and whole-px baselines
    /// (Skia's subpixel glyph positioning) for axis-aligned runs.
    pub glyph_snap: bool,
    /// subtree contains a matte or motion blur (pixels depend on more than the subtree's
    /// own states): no effect caching
    dynamic_subtree: Vec<bool>,
    /// allow subpixel (bilinear) placement of cached blurred subtrees
    pub fx_smooth: bool,
    fx_seen: std::collections::HashMap<u64, u32>,
    /// Chrome parity (M4b): a layer that fades (opacity < 1) while its own transform
    /// animates is composited from a raster taken at its rest transform (grayscale text),
    /// resampled bilinearly at the frame's transform. AGENTFX_FADE_RASTER=0 disables.
    pub fade_raster: bool,
    /// the last rest raster of a fading layer (key, pixmap): reused across the damage rects
    /// of one frame; a fade rebuilds it per frame (the frame's ink is part of the key)
    fade_cache: Option<(u64, Arc<Pixmap>)>,
    /// evaluated points of the last drawn cloud (node, time bits)
    cloud_pts: Vec<crate::cloud::Pt>,
    cloud_key: Option<(NodeId, u64)>,
    /// text v2: the last rewritten list (reused across the damage rects of one frame)
    textfx_key: Option<(NodeId, u64)>,
    textfx_items: Vec<PDraw>,
    /// motion-blur sample buffers of clouds (pooled)
    cloud_bufs: Vec<Vec<u8>>,
    /// world canvas (X9): static/live split per world node, mip tile cache, stats
    world_infos: std::collections::HashMap<NodeId, Arc<world_paint::WorldInfo>>,
    tiles: world_paint::TileCache,
    pub world_stats: world_paint::WorldStats,
    /// AGENTFX_WORLD: 0 = always vectors, 1 = tiles while the camera moves (default), 2 = always tiles
    pub world_mode: u8,
    /// AGENTFX_MBLUR=v1: per-layer motion blur through the reference path (whole-scene
    /// evaluation and a render per sample); default `mblur_paint.rs`
    pub mblur_v1: bool,
    mb_plans: std::collections::HashMap<NodeId, Arc<mblur_paint::MbPlan>>,
    base_m: Option<Arc<Vec<agentfx_ir::Affine>>>,
    mb_cache: mblur_paint::MbCache,
    /// motion blur inside time effects renders the node's whole ink once per frame and
    /// crops (AGENTFX_MBLUR=exact renders each requested rect)
    pub mb_crop: bool,
    /// > 0 while a time effect renders its samples
    time_depth: u32,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct PainterStats {
    pub ctx_created: usize,
    pub offscreen: usize,
    pub offscreen_px: u64,
    pub cache_hits: usize,
    pub cache_builds: usize,
    pub cache_build_px: u64,
    pub cache_build_s: f64,
    /// time effects: subtree renders at sample times, their pixels, cache hits
    pub time_renders: usize,
    pub time_px: u64,
    pub time_hits: usize,
    /// frames of time effects built from the time ring
    pub time_ring_frames: usize,
    /// per-layer motion blur: samples, distinct sample renders, frame-cache hits
    pub mb_samples: usize,
    pub mb_renders: usize,
    pub mb_hits: usize,
}

impl Drop for Painter<'_> {
    fn drop(&mut self) {
        if agentfx_ir::env::var("AGENTFX_DEBUG").is_ok() {
            eprintln!("painter: {:?} cache_bytes {} entries {} shadows {}", self.stats, self.cache_bytes, self.cache.len(), self.shadow_cache.len());
        }
    }
}

const CACHE_LIMIT: usize = 768 << 20;

#[path = "timefx_paint.rs"]
mod timefx_paint;
#[path = "world_paint.rs"]
pub mod world_paint;
#[path = "fluid_paint.rs"]
mod fluid_paint;
#[path = "mblur_paint.rs"]
mod mblur_paint;

fn has_blend(items: &[PDraw]) -> bool {
    items.iter().any(|d| matches!(d, PDraw::Push { blend, .. } if *blend != BlendMode::default()))
}

fn irect(r: [f64; 4]) -> Option<[i32; 4]> {
    if r[2] <= 0.0 || r[3] <= 0.0 {
        return None;
    }
    let x0 = r[0].floor() as i32;
    let y0 = r[1].floor() as i32;
    let x1 = (r[0] + r[2]).ceil() as i32;
    let y1 = (r[1] + r[3]).ceil() as i32;
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

impl<'a> Painter<'a> {
    pub fn new(scene: &'a Scene, prep: &'a Prepared, sched: &'a Schedule, info: &'a Info<'a>, threads: u16) -> Self {
        let cacheable = (0..scene.nodes.len())
            .map(|n| match prep.contents[n].as_ref() {
                Some(PContent::List(items)) => !has_blend(items),
                Some(PContent::TextFx(b)) => !has_blend(&b.1) && !b.0.keyed && b.0.fx.path.is_none() && b.0.fx.animators.is_empty() && info.tracks[n].is_empty(),
                Some(PContent::Switch(states)) => !states.iter().any(|s| has_blend(s)),
                Some(PContent::Text(_)) => glyph_tracks(scene, &info.tracks[n]).is_empty(),
                Some(PContent::Path(_)) => !info.tracks[n].iter().any(|&i| scene.tracks[i].property.starts_with("trim") || scene.tracks[i].property.ends_with(".color")),
                _ => false,
            })
            .collect();
        Painter {
            scene,
            prep,
            sched,
            info,
            res: Resources::new(),
            threads,
            ctxs: Vec::new(),
            stats: PainterStats::default(),
            cacheable,
            cache: Default::default(),
            seen: Default::default(),
            shadow_cache: Default::default(),
            cache_bytes: 0,
            fx_cache: Default::default(),
            time_cache: Default::default(),
            time_cache_bytes: 0,
            time_ring: None,
            time_on: agentfx_ir::env::var("AGENTFX_TIME_FX").map_or(true, |v| v != "0"),
            slit_two_pass: true,
            time_tile: agentfx_ir::env::var("AGENTFX_TIME_TILE").ok().and_then(|v| v.parse().ok()).unwrap_or(256usize).clamp(16, 4096),
            glyph_snap: agentfx_ir::env::var("AGENTFX_NO_GLYPH_SNAP").is_err(),
            fx_smooth: true,
            dynamic_subtree: {
                let mut d = vec![false; scene.nodes.len()];
                for &x in sched.preorder.iter().rev() {
                    let nd = &scene.nodes[x];
                    d[x] = nd.motion_blur.is_some() || nd.time.is_some() || nd.effects.iter().any(|e| matches!(e, Effect::Matte { .. })) || matches!(nd.content, Some(Content::Cloud(_)) | Some(Content::Fluid(_))) || nd.children.iter().any(|&c| d[c]);
                }
                d
            },
            fx_seen: Default::default(),
            fade_raster: agentfx_ir::env::var("AGENTFX_FADE_RASTER").map_or(true, |v| v != "0"),
            fade_cache: None,
            cloud_pts: Vec::new(),
            cloud_key: None,
            textfx_key: None,
            textfx_items: Vec::new(),
            cloud_bufs: Vec::new(),
            world_infos: Default::default(),
            tiles: Default::default(),
            world_stats: Default::default(),
            world_mode: Self::world_mode(),
            mblur_v1: mblur_paint::mblur_v1(),
            mb_plans: Default::default(),
            base_m: None,
            mb_cache: Default::default(),
            mb_crop: agentfx_ir::env::var("AGENTFX_MBLUR").map_or(true, |v| v != "exact"),
            time_depth: 0,
            // raster cache of static content: opt-in (AGENTFX_CACHE=1); blits of large cached
            // rasters cost more than redrawing the vectors on the recorded scenes
            use_cache: agentfx_ir::env::var("AGENTFX_CACHE").is_ok(),
            skip: agentfx_ir::env::var("AGENTFX_SKIP").unwrap_or_default(),
            fluid_matte: 0,
            shadow_cache_on: agentfx_ir::env::var("AGENTFX_NO_SHADOW_CACHE").is_err(),
            lcd: match agentfx_ir::env::var("AGENTFX_LCD").as_deref() {
                Ok("0") => false,
                Ok("1") => true,
                _ => scene.text_aa == agentfx_ir::TextAa::Lcd,
            },
            mask_gamma: crate::lcd::MaskGamma::new(text_contrast(), text_gamma()),
            text_gamma: text_gamma(),
            a8_gamma: agentfx_ir::env::var("AGENTFX_A8_GAMMA").map_or(true, |v| v != "0"),
            sdft: agentfx_ir::env::var("AGENTFX_SDFT").map_or(true, |v| v != "0"),
            // the opt-in content raster cache reuses rasters at other positions, which would
            // move a screen-anchored pattern (review M3 #6): no dither with AGENTFX_CACHE
            dither: agentfx_ir::env::var("AGENTFX_DITHER").map_or(true, |v| v != "0") && agentfx_ir::env::var("AGENTFX_CACHE").is_err(),
            origin: (0, 0),
            grad_cache: Default::default(),
            lcd_cache: Default::default(),
            lcd_cache_px: 0,
            iso: 0,
            quant: agentfx_ir::env::var("AGENTFX_QUANT").ok().and_then(|v| v.parse().ok()).unwrap_or(4.0),
        }
    }

    fn take_ctx(&mut self, w: u16, h: u16, threads: u16) -> RenderContext {
        if let Some(i) = self.ctxs.iter().position(|c| c.render_settings().num_threads == threads) {
            let mut c = self.ctxs.swap_remove(i);
            // Resize the viewport while retaining allocations and worker threads.
            // Exact dimensions preserve clipping and every caller's pixmap stride.
            c.reset_and_resize(w, h);
            return c;
        }
        // Default settings detect SIMD and read the cgroup CPU quota. Do that only
        // once, even when recursive offscreen rendering needs another context.
        static DEFAULT_SETTINGS: OnceLock<RenderSettings> = OnceLock::new();
        let settings = RenderSettings { num_threads: threads, ..*DEFAULT_SETTINGS.get_or_init(RenderSettings::default) };
        self.stats.ctx_created += 1;
        RenderContext::new_with(w, h, settings)
    }

    fn give_ctx(&mut self, c: RenderContext) {
        if self.ctxs.len() < 8 {
            self.ctxs.push(c);
        }
    }

    /// Render screen rect `r` of frame `f` into an RGBA8 (premultiplied) buffer of r's size.
    pub fn render_rect(&mut self, f: usize, r: [i32; 4]) -> Result<Pixmap> {
        let sched = self.sched;
        let view = View { t: f as f64 / self.scene.fps as f64, states: &sched.states[f], content: &sched.content[f] };
        self.render_view(&view, f, r, None, self.threads, false)
    }

    /// Draw the scene (`inner` None) or the inner contents of node `inner` (content and
    /// children, without the node's own clips/opacity/effects, which the caller applies)
    /// into a fresh context covering screen rect `r`. `matte`: draw hidden nodes too.
    fn render_view(&mut self, view: &View, f: usize, r: [i32; 4], inner: Option<NodeId>, threads: u16, matte: bool) -> Result<Pixmap> {
        let (w, h) = (r[2] as u16, r[3] as u16);
        let mut ctx = self.take_ctx(w, h, threads);
        let base = Affine::translate((-r[0] as f64, -r[1] as f64));
        let region = [r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64];
        // offscreen subtree renders start transparent: no LCD text inside
        let off = inner.is_some() || matte;
        if off {
            self.iso += 1;
        }
        let saved = std::mem::replace(&mut self.origin, (r[0] as i64, r[1] as i64));
        let res = match inner {
            None => self.draw_node(&mut ctx, view, f, 0, base, region, 1.0, matte),
            Some(n) => self.draw_inner(&mut ctx, view, f, n, base, region, 1.0, matte),
        };
        self.origin = saved;
        if off {
            self.iso -= 1;
        }
        res?;
        let _p = crate::prof::scope_px("vello.rasterize", w as u64 * h as u64);
        ctx.flush();
        let mut px = Pixmap::new(w, h);
        ctx.render(&mut px, &mut self.res);
        self.give_ctx(ctx);
        Ok(px)
    }

    fn sub_ink(&self, raw: bool, f: usize, n: NodeId) -> Option<[f64; 4]> {
        // subtree ink of the frame (schedule); motion-blur samples use the frame's too.
        // raw: before ancestor clips (the input extent an effect needs)
        let ink = if raw { &self.sched.raw_ink[f] } else { &self.sched.ink[f] };
        let mut r = [0.0; 4];
        let mut st = vec![n];
        while let Some(x) = st.pop() {
            r = union(r, ink[x]);
            st.extend(self.scene.nodes[x].children.iter().copied());
        }
        (r[2] > 0.0).then_some(r)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_node(&mut self, ctx: &mut RenderContext, view: &View, f: usize, n: NodeId, base: Affine, region: [f64; 4], alpha: f64, matte: bool) -> Result<()> {
        let node = &self.scene.nodes[n];
        if node.hidden && !matte {
            return Ok(());
        }
        let st = view.states[n];
        if st.opacity <= 0.0 {
            return Ok(());
        }
        if let Some(rest) = self.fade_rest(view, f, n, matte) {
            return self.draw_fade_raster(ctx, view, f, n, base, region, alpha, matte, rest);
        }
        self.draw_node_vector(ctx, view, f, n, base, region, alpha, matte)
    }

    /// `draw_node` after its visibility checks: the node drawn as vectors (clips, opacity /
    /// blend layer, content, children) or through the effect path.
    #[allow(clippy::too_many_arguments)]
    fn draw_node_vector(&mut self, ctx: &mut RenderContext, view: &View, f: usize, n: NodeId, base: Affine, region: [f64; 4], alpha: f64, matte: bool) -> Result<()> {
        let node = &self.scene.nodes[n];
        let st = view.states[n];
        let pchain = self.sched.parents[n].map(|p| k(&view.states[p].chain)).unwrap_or(Affine::IDENTITY);
        if !node.effects.is_empty() || node.motion_blur.is_some() || (node.time.is_some() && self.time_on) {
            return self.draw_offscreen_node(ctx, view, f, n, base, region, alpha, matte);
        }
        // groups: clips (nested), opacity/blend layer
        let mut pushed = 0;
        let mut clips_pushed = 0;
        for c in &node.clips {
            let rr = c.rect;
            let p = kurbo::RoundedRect::from_rect(Rect::new(rr[0], rr[1], rr[0] + rr[2], rr[1] + rr[3]), kurbo::RoundedRectRadii::new(c.radius[0], c.radius[1], c.radius[2], c.radius[3])).to_path(0.1);
            ctx.set_transform(base * pchain);
            ctx.set_fill_rule(Fill::NonZero);
            // clip paths, not clip layers: no isolation (cheaper; LCD text stays valid)
            ctx.push_clip_path(&p);
            clips_pushed += 1;
        }
        let op = st.opacity * alpha;
        let isolate = node.isolate || node.blend != agentfx_ir::BlendMode::Normal;
        // an opacity that rounds to 255/255 is opaque: no isolation layer (keeps LCD text,
        // as Chrome does at opacity 1; recorded fits often land at 0.99999)
        let op = if op >= OPAQUE { 1.0 } else { op };
        let (group_alpha, child_alpha) = if isolate { (op, 1.0) } else { (1.0, op) };
        if group_alpha < 1.0 || node.blend != agentfx_ir::BlendMode::Normal {
            ctx.set_transform(Affine::IDENTITY);
            ctx.push_layer(None, Some(blend(node.blend)), Some(group_alpha as f32), None, None);
            pushed += 1;
            self.iso += 1;
        }
        let chain = base * k(&st.chain);
        if node.world.is_some() && self.world_tiles_at(n, f) && self.draw_world_tiles(ctx, view, f, n, base, region, child_alpha, matte)? {
            for _ in 0..pushed {
                ctx.pop_layer();
                self.iso -= 1;
            }
            for _ in 0..clips_pushed {
                ctx.pop_clip();
            }
            return Ok(());
        }
        if node.content.is_some() && self.sched.ink[f][n][2] > 0.0 && intersect(self.sched.ink[f][n], region)[2] > 0.0 {
            self.draw_content_cached(ctx, view, n, base, chain, child_alpha, region)?;
        }
        for &c in &node.children {
            if let Some(ink) = self.sub_ink(false, f, c) {
                if intersect(ink, region)[2] <= 0.0 && node.children.len() > 0 && self.scene.nodes[c].motion_blur.is_none() {
                    continue;
                }
            } else {
                continue;
            }
            self.draw_node(ctx, view, f, c, base, region, child_alpha, matte)?;
        }
        for _ in 0..pushed {
            ctx.pop_layer();
            self.iso -= 1;
        }
        for _ in 0..clips_pushed {
            ctx.pop_clip();
        }
        Ok(())
    }

    /// Node content + children without the node's own clips/opacity/effects.
    #[allow(clippy::too_many_arguments)]
    fn draw_inner(&mut self, ctx: &mut RenderContext, view: &View, f: usize, n: NodeId, base: Affine, region: [f64; 4], alpha: f64, matte: bool) -> Result<()> {
        let node = &self.scene.nodes[n];
        let chain = base * k(&view.states[n].chain);
        if node.content.is_some() {
            self.draw_content_cached(ctx, view, n, base, chain, alpha, region)?;
        }
        for &c in &node.children {
            self.draw_node(ctx, view, f, c, base, region, 1.0, matte)?;
        }
        Ok(())
    }

    /// Static content at a transform seen before is drawn from a raster made once at
    /// exactly that transform (same fractional offset, so pixel-identical to vector).
    #[allow(clippy::too_many_arguments)]
    fn draw_content_cached(&mut self, ctx: &mut RenderContext, view: &View, n: NodeId, base: Affine, chain: Affine, alpha: f64, region: [f64; 4]) -> Result<()> {
        if !self.use_cache || !self.cacheable[n] {
            return self.draw_content(ctx, view, n, chain, alpha, region);
        }
        let screen = Affine::translate((region[0], region[1])) * chain;
        let c = screen.as_coeffs();
        let (mut ix, mut iy) = (c[4].floor(), c[5].floor());
        let (mut fx, mut fy) = (c[4] - ix, c[5] - iy);
        // moving content: the fractional offset is quantized to 1/4 px (<= 1/8 px error),
        // so a pan reuses at most 16 rasters per scale instead of re-rasterizing every frame
        let mut chain = chain;
        if self.quant > 0.0 {
            let q = self.quant;
            let (qx, qy) = ((fx * q).round() / q, (fy * q).round() / q);
            chain = Affine::translate((qx - fx, qy - fy)) * chain;
            (fx, fy) = (qx, qy);
            if fx >= 1.0 {
                fx -= 1.0;
                ix += 1.0;
            }
            if fy >= 1.0 {
                fy -= 1.0;
                iy += 1.0;
            }
        }
        let lin = [c[0], c[1], c[2], c[3], fx, fy];
        let key = (n, view.content[n].unwrap_or(usize::MAX), lin.map(f64::to_bits));
        if !self.cache.contains_key(&key) {
            // count distinct frames only (several damage rects of one frame are one sighting)
            let cnt = self.seen.entry(key).or_insert((0, u64::MAX));
            if cnt.1 != view.t.to_bits() {
                cnt.0 += 1;
                cnt.1 = view.t.to_bits();
            }
            if cnt.0 < 2 {
                return self.draw_content(ctx, view, n, chain, alpha, region);
            }
            let Some(b) = self.info.bounds(self.scene, n, view.t) else { return Ok(()) };
            let local = Affine::new(lin);
            let bb = transform_bbox(&ra(local), b);
            let Some(r) = irect(schedule::inflate(bb, 2.0)) else { return Ok(()) };
            if r[2] > 8192 || r[3] > 8192 {
                return self.draw_content(ctx, view, n, chain, alpha, region);
            }
            let tb = std::time::Instant::now();
            self.stats.cache_build_px += r[2] as u64 * r[3] as u64;
            let mut off = self.take_ctx(r[2] as u16, r[3] as u16, 0);
            self.iso += 1;
            let res = self.draw_content(&mut off, view, n, Affine::translate((-r[0] as f64, -r[1] as f64)) * local, 1.0, [0.0, 0.0, r[2] as f64, r[3] as f64]);
            self.iso -= 1;
            res?;
            off.flush();
            let mut px = Pixmap::new(r[2] as u16, r[3] as u16);
            off.render(&mut px, &mut self.res);
            self.give_ctx(off);
            let bytes = r[2] as usize * r[3] as usize * 4;
            if self.cache_bytes + bytes > CACHE_LIMIT {
                self.cache.clear();
                self.cache_bytes = 0;
            }
            self.cache_bytes += bytes;
            self.cache.insert(key, (Arc::new(px), r[0], r[1]));
            self.stats.cache_builds += 1;
            self.stats.cache_build_s += tb.elapsed().as_secs_f64();
        } else {
            self.stats.cache_hits += 1;
        }
        let (px, rx, ry) = self.cache[&key].clone();
        ctx.set_transform(base);
        draw_pixmap_alpha(ctx, px, ix + rx as f64, iy + ry as f64, alpha as f32);
        Ok(())
    }

    fn draw_content(&mut self, ctx: &mut RenderContext, view: &View, n: NodeId, chain: Affine, alpha: f64, region: [f64; 4]) -> Result<()> {
        let prep = self.prep;
        let Some(c) = prep.contents[n].as_ref() else { return Ok(()) };
        let alpha = if alpha >= OPAQUE { 1.0 } else { alpha };
        let wrap = alpha < 1.0 && !matches!(c, PContent::Rect { .. });
        if wrap {
            ctx.set_transform(Affine::IDENTITY);
            ctx.push_opacity_layer(alpha as f32);
            self.iso += 1;
        }
        let _p = crate::prof::scope(match c {
            PContent::Raster { .. } => "draw.raster",
            PContent::Rect { .. } => "draw.rect",
            PContent::List(_) | PContent::Switch(_) => "draw.paint",
            PContent::TextFx(_) => "draw.textfx",
            PContent::Text(_) => "draw.text",
            PContent::Path(_) => "draw.path",
            PContent::Cloud(_) => "cloud.splat",
            PContent::Fluid(_) => "fluid.draw",
        });
        match c {
            PContent::Raster { rect } => {
                if let Some(a) = view.content[n] {
                    let px = prep.images[a].clone().context("raster not loaded")?;
                    let s = prep.image_scale[a];
                    ctx.set_transform(chain);
                    ctx.set_paint(Paint::Image(peniko::ImageBrush {
                        image: ImageSource::Pixmap(px),
                        sampler: peniko::ImageSampler { x_extend: peniko::Extend::Pad, y_extend: peniko::Extend::Pad, quality: peniko::ImageQuality::Medium, alpha: 1.0 },
                    }));
                    ctx.set_paint_transform(Affine::translate((rect[0], rect[1])) * Affine::scale(1.0 / s));
                    ctx.fill_rect(&Rect::new(rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]));
                    ctx.reset_paint_transform();
                }
            }
            PContent::Rect { path, color } => {
                ctx.set_transform(chain);
                ctx.set_fill_rule(Fill::NonZero);
                ctx.set_paint(Paint::Solid(color.multiply_alpha(alpha as f32)));
                ctx.fill_path(path);
            }
            PContent::List(items) => self.replay(ctx, items, chain, region)?,
            PContent::TextFx(b) => {
                let st = text_state(self.scene, &self.info.tracks[n], &b.0, view.t);
                if b.0.is_rest(&st) {
                    self.replay(ctx, &b.1, chain, region)?;
                } else {
                    let key = (n, view.t.to_bits());
                    if self.textfx_key != Some(key) {
                        self.textfx_items = convert(&b.0.frame(&st), &prep.images, &prep.fonts)?;
                        self.textfx_key = Some(key);
                    }
                    let items = std::mem::take(&mut self.textfx_items);
                    let r = self.replay(ctx, &items, chain, region);
                    self.textfx_items = items;
                    r?;
                }
            }
            PContent::Switch(states) => {
                let i = view.content[n].context("switch without state")?;
                self.replay(ctx, &states[i], chain, region)?;
            }
            PContent::Text(pt) => {
                let gt = glyph_tracks(self.scene, &self.info.tracks[n]);
                let font = &prep.fonts[pt.font];
                ctx.set_fill_rule(Fill::NonZero);
                if gt.is_empty() {
                    ctx.set_transform(chain);
                    ctx.set_paint(pt.paint.clone());
                    let _ = ctx
                        .glyph_run(&mut self.res, font)
                        .font_size(pt.shaped.size)
                        .hint(false)
                        .normalized_coords(&pt.shaped.coords)
                        .fill_glyphs(pt.shaped.glyphs.iter().map(|g| vello_cpu::Glyph { id: g.id, x: g.x, y: g.y }));
                } else {
                    for g in &pt.shaped.glyphs {
                        let (o, a) = glyph_state(self.scene, &gt, pt.shaped.units, g.unit, g.origin, view.t);
                        if o <= 0.0 {
                            continue;
                        }
                        ctx.set_transform(chain * a);
                        let layer = o < 1.0 && pt.solid.is_none();
                        match pt.solid {
                            Some(c) if o < 1.0 => ctx.set_paint(Paint::Solid(color(c).multiply_alpha(o as f32))),
                            _ => ctx.set_paint(pt.paint.clone()),
                        }
                        if layer {
                            ctx.push_opacity_layer(o as f32);
                        }
                        let _ = ctx
                            .glyph_run(&mut self.res, font)
                            .font_size(pt.shaped.size)
                            .hint(false)
                            .normalized_coords(&pt.shaped.coords)
                            .fill_glyphs(std::iter::once(vello_cpu::Glyph { id: g.id, x: g.x, y: g.y }));
                        if layer {
                            ctx.pop_layer();
                        }
                    }
                }
            }
            PContent::Cloud(c) => self.draw_cloud(ctx, n, c, view.t, chain)?,
            PContent::Fluid(fl) => self.draw_fluid(ctx, view, n, fl, chain)?,
            PContent::Path(p) => {
                let a = schedule::number_at(self.scene, &self.info.tracks[n], "trim-start", view.t, p.trim[0]);
                let b = schedule::number_at(self.scene, &self.info.tracks[n], "trim-end", view.t, p.trim[1]);
                let trimmed;
                let path = if a <= 0.0 && b >= 1.0 {
                    &p.m.path
                } else {
                    trimmed = p.m.trim(a, b);
                    &trimmed
                };
                ctx.set_transform(chain);
                let fill_c = schedule::color_at(self.scene, &self.info.tracks[n], "fill.color", view.t);
                let stroke_c = schedule::color_at(self.scene, &self.info.tracks[n], "stroke.color", view.t);
                let solid = |c: [f64; 4]| Paint::Solid(color(c.map(|v| v as f32)));
                let over = fill_c.filter(|_| p.fill.is_some()).map(solid);
                if let Some(fp) = over.as_ref().or(p.fill.as_ref()) {
                    if a <= 0.0 && b >= 1.0 {
                        let dithered = match fp {
                            Paint::Gradient(g) if self.dither && p.fill_pt == Affine::IDENTITY => {
                                let screen = [0, 0, self.scene.width as i32, self.scene.height as i32];
                                crate::dither::fill(ctx, &mut self.grad_cache, g as *const _ as usize, path, g, chain, Affine::IDENTITY, 1.0, self.origin, screen, false)
                            }
                            _ => false,
                        };
                        if !dithered {
                            ctx.set_transform(chain);
                            ctx.set_fill_rule(Fill::NonZero);
                            ctx.set_paint(fp.clone());
                            let pt = p.fill_pt != Affine::IDENTITY && over.is_none();
                            if pt {
                                ctx.set_paint_transform(p.fill_pt);
                            }
                            ctx.fill_path(path);
                            if pt {
                                ctx.reset_paint_transform();
                            }
                        }
                    }
                }
                if let Some((st, sp)) = &p.stroke {
                    if !path.elements().is_empty() {
                        ctx.set_stroke(st.clone());
                        ctx.set_paint(match stroke_c {
                            Some(c) => solid(c),
                            None => sp.clone(),
                        });
                        ctx.stroke_path(path);
                    }
                }
            }
        }
        if wrap {
            ctx.pop_layer();
            self.iso -= 1;
        }
        Ok(())
    }

    /// Point cloud: evaluated at t (reused across the damage rects of one frame), splatted
    /// into a premultiplied buffer covering its ink within the context, drawn 1:1.
    fn draw_cloud(&mut self, ctx: &mut RenderContext, n: NodeId, c: &crate::cloud::Cloud, t: f64, chain: Affine) -> Result<()> {
        let Some(b) = c.bounds(t) else { return Ok(()) };
        let dev = transform_bbox(&ra(chain), b);
        let (cw, ch) = (ctx.width() as f64, ctx.height() as f64);
        let x0 = dev[0].floor().max(0.0);
        let y0 = dev[1].floor().max(0.0);
        let x1 = (dev[0] + dev[2]).ceil().min(cw);
        let y1 = (dev[1] + dev[3]).ceil().min(ch);
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        if self.cloud_key != Some((n, t.to_bits())) {
            c.eval(t, &mut self.cloud_pts);
            self.cloud_key = Some((n, t.to_bits()));
        }
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut px = Pixmap::new(w as u16, h as u16);
        let m = (Affine::translate((-x0, -y0)) * chain).as_coeffs();
        crate::cloud::splat(&self.cloud_pts, m, px.data_as_u8_slice_mut(), w, h);
        ctx.set_transform(Affine::IDENTITY);
        draw_pixmap(ctx, Arc::new(px), x0, y0);
        Ok(())
    }

    fn replay(&mut self, ctx: &mut RenderContext, items: &[PDraw], base: Affine, region: [f64; 4]) -> Result<()> {
        let skip_on = !self.skip.is_empty();
        // per push: true = layer (isolating), false = clip path
        let mut kinds: Vec<bool> = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let kind = match &items[i] {
                PDraw::Fill { paint: Paint::Image(_), .. } => "image",
                PDraw::Fill { paint: Paint::Gradient(_), .. } => "gradient",
                PDraw::Fill { .. } => "fill",
                PDraw::Shadow { .. } => "shadow",
                PDraw::Glyphs { .. } => "glyphs",
                _ => "",
            };
            if skip_on && !kind.is_empty() && self.skip.contains(kind) {
                i += 1;
                continue;
            }
            // cull against the target area (this context: [0, 0, region w, h])
            let vis = [0.0, 0.0, region[2], region[3]];
            let off = |bb: &[f64; 4]| bb[2] > 0.0 && intersect(transform_bbox(&ra(base), *bb), vis)[2] <= 0.0;
            match &items[i] {
                PDraw::Fill { bb, .. } | PDraw::Stroke { bb, .. } | PDraw::Glyphs { bb, .. } | PDraw::Shadow { bb, .. } if off(bb) => {
                    i += 1;
                    continue;
                }
                PDraw::Push { filter: None, len, bounds, .. } if bounds[2] > 0.0 && off(bounds) => {
                    i += *len + 1;
                    continue;
                }
                _ => {}
            }
            // text stroke below the fill (outline) / above it (`over`)
            let text_stroke = match &items[i] {
                PDraw::Glyphs { stroke: Some(st), font, size, coords, t, gt, glyphs, .. } => Some((st, *font, *size, coords, *t, *gt, glyphs)),
                _ => None,
            };
            if let Some((st, font, size, coords, t, gt, glyphs)) = text_stroke {
                if !st.2 {
                    self.stroke_glyphs(ctx, st, font, size, coords, base * t, gt, glyphs);
                }
            }
            match &items[i] {
                PDraw::Fill { path, paint: Paint::Gradient(g), t, pt, rule, .. }
                    if self.dither && crate::dither::fill(ctx, &mut self.grad_cache, g as *const _ as usize, path, g, base * *t, *pt, 1.0, self.origin, [0, 0, self.scene.width as i32, self.scene.height as i32], *rule == Fill::EvenOdd) => {}
                PDraw::Fill { path, paint, t, pt, rule, .. } => {
                    ctx.set_transform(base * *t);
                    ctx.set_fill_rule(*rule);
                    ctx.set_paint(paint.clone());
                    ctx.set_paint_transform(*pt);
                    ctx.fill_path(path);
                    ctx.reset_paint_transform();
                }
                PDraw::Stroke { path, paint, stroke, t, pt, .. } => {
                    ctx.set_transform(base * *t);
                    ctx.set_stroke(stroke.clone());
                    ctx.set_paint(paint.clone());
                    ctx.set_paint_transform(*pt);
                    ctx.stroke_path(path);
                    ctx.reset_paint_transform();
                }
                PDraw::Glyphs { font, size, coords, paint: Paint::Solid(c), t, gt: None, glyphs, bb, .. }
                    if ((self.lcd && self.iso == 0) || self.a8_gamma) && { let m = (base * *t).as_coeffs(); m[1] == 0.0 && m[2] == 0.0 && m[0] > 0.0 && m[3] > 0.0 } =>
                {
                    let lcd = self.lcd && self.iso == 0;
                    self.draw_glyphs_mask(ctx, *font, *size, coords, *c, base * *t, glyphs, transform_bbox(&ra(base), *bb), lcd)?;
                }
                PDraw::Glyphs { font, size, coords, paint, t, gt, glyphs, bb, .. } => {
                    if self.lcd && agentfx_ir::env::var("AGENTFX_LCD_DEBUG").is_ok() {
                        eprintln!("gray run: iso {} m {:?} solid {} gt {} bb {:?}", self.iso, (base * *t).as_coeffs(), matches!(paint, Paint::Solid(_)), gt.is_some(), transform_bbox(&ra(base), *bb));
                    }
                    ctx.set_transform(base * *t);
                    ctx.set_fill_rule(Fill::NonZero);
                    ctx.set_paint(paint.clone());
                    let font = &self.prep.fonts[*font];
                    let mut b = ctx.glyph_run(&mut self.res, font).font_size(*size).hint(false).normalized_coords(coords);
                    if let Some(g) = gt {
                        b = b.glyph_transform(*g);
                    }
                    if self.glyph_snap {
                        let _ = b.fill_glyphs(snap_glyphs(base * *t, glyphs).into_iter());
                    } else {
                        let _ = b.fill_glyphs(glyphs.iter().copied());
                    }
                }
                PDraw::Shadow { rect, radius, std_dev, color, t, .. } if *std_dev > 2.0 && self.shadow_cache_on => {
                    // large blurs are slow to evaluate per pixel: rasterize once per
                    // geometry/scale and blit at whole pixels (sub-pixel offsets of a blur
                    // of sigma > 2 px are invisible)
                    let m = (base * *t).as_coeffs();
                    if m[1] == 0.0 && m[2] == 0.0 && m[0] > 0.0 && m[3] > 0.0 {
                        let key = [rect.width(), rect.height(), *radius as f64, *std_dev as f64, m[0], m[3], color.components[0] as f64, color.components[1] as f64, color.components[2] as f64 + 7.0 * color.components[3] as f64].map(f64::to_bits);
                        let key: [u64; 9] = [key[0], key[1], key[2], key[3], key[4], key[5], key[6], key[7], key[8]];
                        if !self.shadow_cache.contains_key(&key) {
                            let e = (3.0 * *std_dev as f64 + 2.0).ceil();
                            let (w, h) = ((rect.width() * m[0] + 2.0 * e).ceil(), (rect.height() * m[3] + 2.0 * e).ceil());
                            if w > 8192.0 || h > 8192.0 {
                                ctx.set_transform(base * *t);
                                ctx.set_paint(Paint::Solid(*color));
                                ctx.reset_paint_transform();
                                ctx.fill_blurred_rounded_rect(rect, *radius, *std_dev, false);
                                i += 1;
                                continue;
                            }
                            let mut off = self.take_ctx(w as u16, h as u16, 0);
                            off.set_transform(Affine::new([m[0], 0.0, 0.0, m[3], e - rect.x0 * m[0], e - rect.y0 * m[3]]));
                            off.set_paint(Paint::Solid(*color));
                            off.fill_blurred_rounded_rect(rect, *radius, *std_dev, false);
                            off.flush();
                            let mut px = Pixmap::new(w as u16, h as u16);
                            off.render(&mut px, &mut self.res);
                            self.give_ctx(off);
                            self.shadow_cache.insert(key, (Arc::new(px), e, e));
                        }
                        let (px, ex, ey) = self.shadow_cache[&key].clone();
                        let x = (m[0] * rect.x0 + m[4] - ex).round();
                        let y = (m[3] * rect.y0 + m[5] - ey).round();
                        ctx.set_transform(Affine::IDENTITY);
                        draw_pixmap(ctx, px, x, y);
                    } else {
                        ctx.set_transform(base * *t);
                        ctx.set_paint(Paint::Solid(*color));
                        ctx.reset_paint_transform();
                        ctx.fill_blurred_rounded_rect(rect, *radius, *std_dev, false);
                    }
                }
                PDraw::Shadow { rect, radius, std_dev, color, t, .. } => {
                    ctx.set_transform(base * *t);
                    ctx.set_paint(Paint::Solid(*color));
                    ctx.reset_paint_transform();
                    ctx.fill_blurred_rounded_rect(rect, *radius, *std_dev, false);
                }
                PDraw::Push { clip, t, alpha, blend: bm, filter, rule, len, bounds } => {
                    if let Some(fl) = filter {
                        self.filtered_group(ctx, &items[i + 1..i + *len], base, *t, clip.as_ref(), *alpha, *bm, fl, *bounds, region)?;
                        i += *len + 1;
                        continue;
                    }
                    ctx.set_transform(base * *t);
                    ctx.set_fill_rule(*rule);
                    let normal = *bm == BlendMode::default();
                    if *alpha >= 1.0 && normal {
                        match clip {
                            // pure clips: clip paths (no isolation; cheaper; LCD text stays valid)
                            Some(c) => {
                                ctx.push_clip_path(c);
                                kinds.push(false);
                            }
                            None => {
                                ctx.push_layer(None, None, None, None, None);
                                kinds.push(true);
                                self.iso += 1;
                            }
                        }
                    } else {
                        ctx.push_layer(clip.as_ref(), Some(*bm), Some(*alpha), None, None);
                        kinds.push(true);
                        self.iso += 1;
                    }
                }
                PDraw::Pop => {
                    if kinds.pop().unwrap_or(true) {
                        ctx.pop_layer();
                        self.iso -= 1;
                    } else {
                        ctx.pop_clip();
                    }
                }
            }
            if let Some((st, font, size, coords, t, gt, glyphs)) = text_stroke {
                if st.2 {
                    self.stroke_glyphs(ctx, st, font, size, coords, base * t, gt, glyphs);
                }
            }
            i += 1;
        }
        Ok(())
    }

    /// Text stroke: the glyph outlines stroked (vello_cpu `stroke_glyphs`), never hinted.
    #[allow(clippy::too_many_arguments)]
    fn stroke_glyphs(&mut self, ctx: &mut RenderContext, st: &(kurbo::Stroke, Paint, bool), font: usize, size: f32, coords: &[i16], m: Affine, gt: Option<Affine>, glyphs: &[vello_cpu::Glyph]) {
        ctx.set_transform(m);
        ctx.set_stroke(st.0.clone());
        ctx.set_paint(st.1.clone());
        let font = &self.prep.fonts[font];
        let mut b = ctx.glyph_run(&mut self.res, font).font_size(size).hint(false).normalized_coords(coords);
        if let Some(g) = gt {
            b = b.glyph_transform(g);
        }
        let _ = b.stroke_glyphs(glyphs.iter().copied());
    }

    /// Glyph run through a coverage mask with Skia's mask gamma (see `lcd.rs`): LCD
    /// subpixel antialiasing (`lcd`) or grayscale. `m` maps run space to this context's
    /// pixels (axis-aligned), `dev` = the run's ink bounds in context pixels.
    #[allow(clippy::too_many_arguments)]
    fn draw_glyphs_mask(&mut self, ctx: &mut RenderContext, font: usize, size: f32, coords: &[i16], color: peniko::Color, m: Affine, glyphs: &[vello_cpu::Glyph], dev: [f64; 4], lcd: bool) -> Result<()> {
        let _p = crate::prof::scope("text.mask");
        let vis = [0.0, 0.0, ctx.width() as f64, ctx.height() as f64];
        if dev[2] <= 0.0 || intersect(dev, vis)[2] <= 0.0 {
            return Ok(());
        }
        // (review M3 #5) only the visible part of a long run needs a mask (2 px margin for
        // the LCD filter); placement stays exact because x0/y0 are whole pixels
        let dev = intersect(dev, schedule::inflate(vis, 2.0));
        let c = m.as_coeffs();
        let (x0, y0) = ((dev[0] - 1.0).floor(), (dev[1] - 1.0).floor());
        let (w, h) = (((dev[0] + dev[2] + 1.0).ceil() - x0) as usize, ((dev[1] + dev[3] + 1.0).ceil() - y0) as usize);
        // Chrome draws grayscale text under a scale through distance fields (M4, `sdft.rs`).
        // The caller only passes axis-aligned matrices with positive scales (c[1] = c[2] = 0);
        // the max |scale| is Skia's `getMaxScale` for them.
        let device_size = size * c[0].abs().max(c[3].abs()) as f32;
        if self.sdft && crate::sdft::applies(self.lcd, !lcd, c[0], c[3], device_size) {
            let k = crate::sdft::bucket(device_size) / device_size;
            let (tw, th) = ((w as f32 * k).ceil() as usize + 2 * crate::sdft::PAD, (h as f32 * k).ceil() as usize + 2 * crate::sdft::PAD);
            if tw * th <= 4_000_000 && tw < 65_000 && th < 65_000 {
                return self.draw_glyphs_sdft(ctx, font, size, coords, color, m, glyphs, x0, y0, w, h, k, tw, th);
            }
        }
        let snapped = if self.glyph_snap { snap_glyphs(m, glyphs) } else { glyphs.to_vec() };
        if w * h > 4_000_000 || 3 * w + 4 > 16000 {
            {
                // too large for a mask: plain vector glyphs (no gamma)
                ctx.set_transform(m);
                ctx.set_fill_rule(Fill::NonZero);
                ctx.set_paint(Paint::Solid(color));
                let fd = &self.prep.fonts[font];
                let _ = ctx.glyph_run(&mut self.res, fd).font_size(size).hint(false).normalized_coords(coords).fill_glyphs(snapped.iter().copied());
                return Ok(());
            }
        }
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        (lcd, font, size.to_bits(), coords, c[0].to_bits(), c[3].to_bits(), w, h).hash(&mut hs);
        color.components.map(f32::to_bits).hash(&mut hs);
        for g in &snapped {
            g.id.hash(&mut hs);
            ((c[0] * g.x as f64 + c[4] - x0) * 64.0).round().to_bits().hash(&mut hs);
            ((c[3] * g.y as f64 + c[5] - y0) * 64.0).round().to_bits().hash(&mut hs);
        }
        let key = hs.finish();
        if !lcd {
            if !self.lcd_cache.contains_key(&key) {
                let mut off = self.take_ctx(w as u16, h as u16, 0);
                off.set_transform(Affine::translate((-x0, -y0)) * m);
                off.set_fill_rule(Fill::NonZero);
                off.set_paint(Paint::Solid(peniko::Color::WHITE));
                let fd = &self.prep.fonts[font];
                let _ = off.glyph_run(&mut self.res, fd).font_size(size).hint(false).normalized_coords(coords).fill_glyphs(snapped.iter().copied());
                off.flush();
                let mut px = Pixmap::new(w as u16, h as u16);
                off.render(&mut px, &mut self.res);
                self.give_ctx(off);
                let lut = self.mask_gamma.a8(color.components, self.text_gamma);
                let col = color.components;
                let mut img = Vec::with_capacity(w * h * 4);
                for p in px.data_as_u8_slice().chunks_exact(4) {
                    let a = lut[p[3] as usize] as f32 / 255.0 * col[3].clamp(0.0, 1.0);
                    img.extend_from_slice(&[crate::pixel::round_u8(col[0].clamp(0.0, 1.0) * a * 255.0), crate::pixel::round_u8(col[1].clamp(0.0, 1.0) * a * 255.0), crate::pixel::round_u8(col[2].clamp(0.0, 1.0) * a * 255.0), crate::pixel::round_u8(a * 255.0)]);
                }
                let meta = vello_cpu::PixelMetadata { alpha_type: peniko::ImageAlphaType::AlphaPremultiplied, may_have_transparency: true };
                let img = Pixmap::from_parts(img, w as u16, h as u16, meta);
                if self.lcd_cache_px > 64_000_000 {
                    self.lcd_cache.clear();
                    self.lcd_cache_px = 0;
                }
                self.lcd_cache_px += w * h;
                let img = Arc::new(img);
                self.lcd_cache.insert(key, (img.clone(), img));
            }
            let (img, _) = self.lcd_cache[&key].clone();
            ctx.set_transform(Affine::IDENTITY);
            draw_pixmap(ctx, img, x0, y0);
            return Ok(());
        }
        if !self.lcd_cache.contains_key(&key) {
            let sw = 3 * w + 4;
            let mut off = self.take_ctx(sw as u16, h as u16, 0);
            // device x -> 3 subpixels, 2 subpixels of filter margin on the left
            off.set_transform(Affine::new([3.0, 0.0, 0.0, 1.0, 2.0 - 3.0 * x0, -y0]) * m);
            off.set_fill_rule(Fill::NonZero);
            off.set_paint(Paint::Solid(peniko::Color::WHITE));
            let fd = &self.prep.fonts[font];
            let _ = off.glyph_run(&mut self.res, fd).font_size(size).hint(false).normalized_coords(coords).fill_glyphs(snapped.iter().copied());
            off.flush();
            let mut px = Pixmap::new(sw as u16, h as u16);
            off.render(&mut px, &mut self.res);
            self.give_ctx(off);
            let sub: Vec<u8> = px.data_as_u8_slice().chunks_exact(4).map(|p| p[3]).collect();
            let cov = crate::lcd::filter(&sub, w, h);
            let (mul, add) = crate::lcd::images(&cov, color.components, self.mask_gamma.lcd(color.components));
            let meta = vello_cpu::PixelMetadata { alpha_type: peniko::ImageAlphaType::AlphaPremultiplied, may_have_transparency: true };
            let mul = Pixmap::from_parts(mul, w as u16, h as u16, meta.clone());
            let add = Pixmap::from_parts(add, w as u16, h as u16, meta);
            if self.lcd_cache_px > 64_000_000 {
                self.lcd_cache.clear();
                self.lcd_cache_px = 0;
            }
            self.lcd_cache_px += w * h;
            self.lcd_cache.insert(key, (Arc::new(mul), Arc::new(add)));
        }
        let (mul, add) = self.lcd_cache[&key].clone();
        ctx.set_transform(Affine::IDENTITY);
        ctx.set_blend_mode(BlendMode::new(Mix::Multiply, Compose::SrcOver));
        draw_pixmap(ctx, mul, x0, y0);
        ctx.set_blend_mode(BlendMode::new(Mix::Normal, Compose::Plus));
        draw_pixmap(ctx, add, x0, y0);
        ctx.set_blend_mode(BlendMode::default());
        Ok(())
    }

    /// Grayscale run as Skia's distance-field text (`sdft.rs`): the run is rasterized at
    /// `k` texels per device pixel (the SDF bucket size), turned into a signed distance
    /// field and sampled back at the device pixels with Skia's 0.65 px ramp. Glyph
    /// positions stay fractional (SDFT does not snap). The mask is cached like the others.
    #[allow(clippy::too_many_arguments)]
    fn draw_glyphs_sdft(&mut self, ctx: &mut RenderContext, font: usize, size: f32, coords: &[i16], color: peniko::Color, m: Affine, glyphs: &[vello_cpu::Glyph], x0: f64, y0: f64, w: usize, h: usize, k: f32, tw: usize, th: usize) -> Result<()> {
        let c = m.as_coeffs();
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        ("sdft", font, size.to_bits(), coords, c[0].to_bits(), c[3].to_bits(), w, h).hash(&mut hs);
        color.components.map(f32::to_bits).hash(&mut hs);
        for g in glyphs {
            g.id.hash(&mut hs);
            ((c[0] * g.x as f64 + c[4] - x0) * 64.0).round().to_bits().hash(&mut hs);
            ((c[3] * g.y as f64 + c[5] - y0) * 64.0).round().to_bits().hash(&mut hs);
        }
        let key = hs.finish();
        if !self.lcd_cache.contains_key(&key) {
            let pad = crate::sdft::PAD as f64;
            let mut off = self.take_ctx(tw as u16, th as u16, 0);
            off.set_transform(Affine::translate((pad, pad)) * Affine::scale(k as f64) * Affine::translate((-x0, -y0)) * m);
            off.set_fill_rule(Fill::NonZero);
            off.set_paint(Paint::Solid(peniko::Color::WHITE));
            let fd = &self.prep.fonts[font];
            let _ = off.glyph_run(&mut self.res, fd).font_size(size).hint(false).normalized_coords(coords).fill_glyphs(glyphs.iter().copied());
            off.flush();
            let mut px = Pixmap::new(tw as u16, th as u16);
            off.render(&mut px, &mut self.res);
            self.give_ctx(off);
            let cov: Vec<u8> = px.data_as_u8_slice().chunks_exact(4).map(|p| p[3]).collect();
            let field = crate::sdft::distance_field(&cov, tw, th);
            let col = color.components;
            let adjust = crate::sdft::adjust(col);
            let mut img = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    let u = (x as f32 + 0.5) * k + pad as f32;
                    let v = (y as f32 + 0.5) * k + pad as f32;
                    let d = crate::sdft::sample(&field, tw, th, u, v);
                    let a = crate::sdft::coverage(d, k, adjust) * col[3].clamp(0.0, 1.0);
                    img.extend_from_slice(&[crate::pixel::round_u8(col[0].clamp(0.0, 1.0) * a * 255.0), crate::pixel::round_u8(col[1].clamp(0.0, 1.0) * a * 255.0), crate::pixel::round_u8(col[2].clamp(0.0, 1.0) * a * 255.0), crate::pixel::round_u8(a * 255.0)]);
                }
            }
            let meta = vello_cpu::PixelMetadata { alpha_type: peniko::ImageAlphaType::AlphaPremultiplied, may_have_transparency: true };
            let img = Arc::new(Pixmap::from_parts(img, w as u16, h as u16, meta));
            if self.lcd_cache_px > 64_000_000 {
                self.lcd_cache.clear();
                self.lcd_cache_px = 0;
            }
            self.lcd_cache_px += w * h;
            self.lcd_cache.insert(key, (img.clone(), img));
        }
        let (img, _) = self.lcd_cache[&key].clone();
        ctx.set_transform(Affine::IDENTITY);
        draw_pixmap(ctx, img, x0, y0);
        Ok(())
    }

    /// A display-list group with a CSS filter: rendered offscreen, filtered, composited.
    #[allow(clippy::too_many_arguments)]
    fn filtered_group(&mut self, ctx: &mut RenderContext, items: &[PDraw], base: Affine, t: Affine, clip: Option<&BezPath>, alpha: f32, bm: BlendMode, fl: &DrawFilter, bounds: [f64; 4], region: [f64; 4]) -> Result<()> {
        let _p = crate::prof::scope("filter.group");
        let bt = base * t;
        // scale of the layer transform for filter parameters; base includes the region offset
        let s = bt.as_coeffs()[0].hypot(bt.as_coeffs()[1]).max(bt.as_coeffs()[2].hypot(bt.as_coeffs()[3]));
        let (sd, dx, dy, col) = match fl {
            DrawFilter::Blur { std_dev } => (*std_dev * s, 0.0, 0.0, None),
            DrawFilter::DropShadow { dx, dy, std_dev, color } => (*std_dev * s, dx * s, dy * s, Some(*color)),
        };
        let m = schedule::BLUR_EXTENT * sd + dx.abs().max(dy.abs()) + 2.0;
        // screen-space (this context's space) region of the group
        let content = transform_bbox(&ra(base), bounds);
        let local_region = [0.0, 0.0, region[2], region[3]];
        let o = intersect(schedule::inflate(content, m), schedule::inflate(local_region, m));
        let Some(o) = irect(o) else { return Ok(()) };
        let mut off = self.take_ctx(o[2] as u16, o[3] as u16, 0);
        let ob = Affine::translate((-o[0] as f64, -o[1] as f64)) * base;
        self.iso += 1;
        let saved = self.origin;
        self.origin = (saved.0 + o[0] as i64, saved.1 + o[1] as i64);
        // (review M3 #2) cull against the offscreen target, not the parent's region
        let r = self.replay(&mut off, items, ob, [0.0, 0.0, o[2] as f64, o[3] as f64]);
        self.origin = saved;
        self.iso -= 1;
        r?;
        off.flush();
        let mut px = Pixmap::new(o[2] as u16, o[3] as u16);
        off.render(&mut px, &mut self.res);
        self.give_ctx(off);
        self.stats.offscreen += 1;
        self.stats.offscreen_px += (o[2] * o[3]) as u64;
        let (w, h) = (o[2] as usize, o[3] as usize);
        let buf = px.data_as_u8_slice_mut();
        match col {
            None => effects::blur(buf, w, h, sd),
            Some(c) => {
                let mut sh = effects::tint_alpha(buf, [c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64], 1.0);
                effects::blur(&mut sh, w, h, sd);
                let mut sh = effects::shift(&sh, w, h, dx.round() as i64, dy.round() as i64);
                effects::over(&mut sh, buf);
                buf.copy_from_slice(&sh);
            }
        }
        // composite (clip in the layer's space)
        ctx.set_transform(bt);
        let pushed = match clip {
            Some(c) => {
                ctx.push_layer(Some(c), Some(bm), Some(alpha), None, None);
                true
            }
            None if alpha < 1.0 || bm != BlendMode::default() => {
                ctx.push_layer(None, Some(bm), Some(alpha), None, None);
                true
            }
            None => false,
        };
        // the filtered pixmap is in this context's pixel space
        ctx.set_transform(Affine::IDENTITY);
        draw_pixmap(ctx, Arc::new(px), o[0] as f64, o[1] as f64);
        if pushed {
            ctx.pop_layer();
        }
        Ok(())
    }

    /// Subtree of `n` rendered into screen rect `o` (motion-blur samples averaged),
    /// then the node's effects applied in order.
    #[allow(clippy::too_many_arguments)]
    fn effect_pixmap(&mut self, view: &View, f: usize, n: NodeId, o: [i32; 4], params: &[[f64; crate::look::NPARAM]], s: f64, matte: bool) -> Result<Pixmap> {
        let scene = self.scene;
        let node = &scene.nodes[n];
        let (w, h) = (o[2] as usize, o[3] as usize);
        // content (motion blur: average of sub-frame samples)
        let leaf_cloud = match self.prep.contents[n].as_ref() {
            Some(PContent::Cloud(c)) if node.children.is_empty() => Some(c),
            _ => None,
        };
        let mut px = if let (Some(mb), Some(c)) = (&node.motion_blur, leaf_cloud) {
            // a cloud alone: splat each sample straight into its buffer (no vector pass)
            let _p = crate::prof::scope_px("mblur.cloud", (w * h) as u64 * mb.samples as u64);
            let times = schedule::mb_times(scene.fps as f64, f, mb.samples, mb.shutter);
            let base_m: Vec<agentfx_ir::Affine> = scene.nodes.iter().map(|x| agentfx_ir::eval::transform_matrix(&x.transform)).collect();
            let mut chains = Vec::with_capacity(times.len());
            for ts in &times {
                let chain = if self.mblur_v1 {
                    let locals = schedule::eval_locals(scene, &self.sched.tracks, &base_m, *ts)?;
                    let states = schedule::chains(&self.sched.preorder, &self.sched.parents, &locals);
                    k(&states[n].chain)
                } else {
                    self.mb_chain(view, n, *ts)?
                };
                chains.push((*ts, (Affine::translate((-o[0] as f64, -o[1] as f64)) * chain).as_coeffs()));
            }
            use rayon::prelude::*;
            // per-sample buffers, pooled across frames (no page faults per frame)
            let len = w * h * 4;
            let mut bufs = std::mem::take(&mut self.cloud_bufs);
            bufs.resize_with(chains.len(), Vec::new);
            bufs.par_iter_mut().zip(chains.par_iter()).for_each(|(buf, (ts, m))| {
                buf.clear();
                buf.resize(len, 0);
                let mut pts = Vec::new();
                c.eval(*ts, &mut pts);
                crate::cloud::splat(&pts, *m, buf, w, h);
            });
            // average in one pass over all samples
            let ns = bufs.len() as u32;
            let mut p = Pixmap::new(w as u16, h as u16);
            p.data_as_u8_slice_mut().par_chunks_mut(1 << 14).enumerate().for_each(|(ci, out)| {
                let o = ci << 14;
                for (j, v) in out.iter_mut().enumerate() {
                    let sum: u32 = bufs.iter().map(|b| b[o + j] as u32).sum();
                    *v = ((sum + ns / 2) / ns).min(255) as u8;
                }
            });
            self.cloud_bufs = bufs;
            p
        } else if let (Some(mb), false) = (&node.motion_blur, self.mblur_v1) {
            self.mblur_pixmap(view, f, n, o, mb, matte)?
        } else if let Some(mb) = &node.motion_blur {
            let times = schedule::mb_times(scene.fps as f64, f, mb.samples, mb.shutter);
            let base_m: Vec<agentfx_ir::Affine> = scene.nodes.iter().map(|x| agentfx_ir::eval::transform_matrix(&x.transform)).collect();
            let _p = crate::prof::scope_px("mblur", (w * h) as u64 * times.len() as u64);
            let mut acc = vec![0u16; w * h * 4];
            for ts in &times {
                let states = {
                    let _p = crate::prof::scope("mblur.eval");
                    let locals = schedule::eval_locals(scene, &self.sched.tracks, &base_m, *ts)?;
                    schedule::chains(&self.sched.preorder, &self.sched.parents, &locals)
                };
                let v = View { t: *ts, states: &states, content: view.content };
                let p = self.render_view(&v, f, o, Some(n), 0, matte)?;
                let _p = crate::prof::scope("mblur.accum");
                effects::accumulate(&mut acc, p.data_as_u8_slice());
            }
            let _pa = crate::prof::scope("mblur.accum");
            let avg = effects::average(&acc, times.len() as u32);
            let mut p = Pixmap::new(w as u16, h as u16);
            p.data_as_u8_slice_mut().copy_from_slice(&avg);
            p
        } else if node.time.is_some() && self.time_on {
            self.time_pixmap(view, f, n, o, matte)?
        } else {
            let _p = crate::prof::scope("fx.subtree");
            self.render_view(view, f, o, Some(n), 0, matte)?
        };
        self.stats.offscreen += 1;
        self.stats.offscreen_px += (w * h) as u64;
        // GPU effects: consecutive ones keep the buffer on the GPU (E4 phase 2)
        let mut gch = crate::gpu::fx::Chain::default();
        for (e, p) in node.effects.iter().zip(params) {
            let _p = crate::prof::scope_px(fx_name(e), (w * h) as u64);
            let buf = px.data_as_u8_slice_mut();
            match e {
                Effect::Blur { .. } => {
                    if !gch.blur(buf, w, h, p[0] * s) {
                        effects::blur(buf, w, h, p[0] * s)
                    }
                }
                Effect::DropShadow { color, .. } => {
                    let (sg, dx, dy) = (p[0] * s, (p[1] * s).round() as i64, (p[2] * s).round() as i64);
                    if !gch.drop_shadow(buf, w, h, *color, sg, dx, dy) {
                        effects::drop_shadow(buf, w, h, *color, sg, dx, dy);
                    }
                }
                Effect::Glow { color, .. } => {
                    if !gch.glow(buf, w, h, *color, p[3], p[0] * s) {
                        effects::glow(buf, w, h, *color, p[3], p[0] * s);
                    }
                }
                e @ (Effect::Riso { .. } | Effect::Halftone { .. } | Effect::Grain { .. } | Effect::Paper { .. } | Effect::Shadow { .. } | Effect::Sweep { .. } | Effect::Rays { .. } | Effect::Color { .. } | Effect::Rgbsplit { .. } | Effect::Glitch { .. } | Effect::Vhs { .. } | Effect::Dither { .. } | Effect::Ascii { .. }) => {
                    let ch = k(&view.states[n].chain).as_coeffs();
                    let cx = crate::look::FxCtx { ox: o[0], oy: o[1], ax: ch[4].round() as i32, ay: ch[5].round() as i32, s, frame: f as u32 };
                    if !gch.look(e, p, buf, w, h, &cx) {
                        crate::look::apply(e, p, buf, w, h, &cx);
                    }
                }
                Effect::Matte { source, mode } => {
                    gch.flush(buf);
                    let src = *source;
                    let m = {
                        // the source as drawn in this view (it is hidden in the normal pass)
                        let (w2, h2) = (o[2] as u16, o[3] as u16);
                        let mut c2 = self.take_ctx(w2, h2, 0);
                        let b2 = Affine::translate((-o[0] as f64, -o[1] as f64));
                        let reg = [o[0] as f64, o[1] as f64, o[2] as f64, o[3] as f64];
                        self.fluid_matte += 1;
                        let r = self.draw_node(&mut c2, view, f, src, b2, reg, 1.0, true);
                        self.fluid_matte -= 1;
                        r?;
                        let _p = crate::prof::scope_px("vello.rasterize", w2 as u64 * h2 as u64);
                        c2.flush();
                        let mut mp = Pixmap::new(w2, h2);
                        c2.render(&mut mp, &mut self.res);
                        self.give_ctx(c2);
                        mp
                    };
                    let buf = px.data_as_u8_slice_mut();
                    effects::matte(buf, m.data_as_u8_slice(), mode.starts_with("luma"), mode.ends_with("inverted"));
                }
            }
        }
        gch.flush(px.data_as_u8_slice_mut());
        Ok(px)
    }

    /// Hash of everything inside `n`'s subtree relative to `n` (rigid moves of `n` keep it).
    fn effect_key(&self, view: &View, n: NodeId, params: &[[f64; crate::look::NPARAM]]) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        n.hash(&mut h);
        let cn = k(&view.states[n].chain);
        for c in &cn.as_coeffs()[..4] {
            c.to_bits().hash(&mut h);
        }
        for p in params {
            for v in p {
                v.to_bits().hash(&mut h);
            }
        }
        self.hash_subtree(view, n, cn.inverse(), &mut h);
        h.finish()
    }

    /// Hash the subtree of `n` relative to `n`: descendants' chains in `n`'s space (`inv` =
    /// the inverse of `n`'s chain), opacities, content states, signatures, effect parameters.
    fn hash_subtree(&self, view: &View, n: NodeId, inv: Affine, h: &mut std::collections::hash_map::DefaultHasher) {
        let mut st = vec![n];
        while let Some(x) = st.pop() {
            if x != n {
                for c in (inv * k(&view.states[x].chain)).as_coeffs() {
                    ((c * 1e9).round() as i64).hash(h);
                }
                view.states[x].opacity.to_bits().hash(h);
            }
            view.content[x].hash(h);
            self.info.sig(self.scene, x, view.t).hash(h);
            if x != n {
                let tr = &self.sched.tracks[x];
                for (i, e) in self.scene.nodes[x].effects.iter().enumerate() {
                    for v in schedule::effect_params(self.scene, tr, i, e, view.t) {
                        v.to_bits().hash(h);
                    }
                }
            }
            st.extend(self.scene.nodes[x].children.iter().copied());
        }
    }

    /// The node's own (uncommitted) local matrix, origin applied, at time `t`.
    fn local_at(&self, n: NodeId, t: f64) -> Affine {
        let node = &self.scene.nodes[n];
        let mut m = agentfx_ir::eval::transform_matrix(&node.transform);
        for &i in &self.sched.tracks[n] {
            let tr = &self.scene.tracks[i];
            if tr.property == "transform" {
                if let Some(Sampled::Matrix(x)) = agentfx_ir::eval::eval_track(tr, t) {
                    m = x;
                }
            }
        }
        let (ox, oy) = (node.origin[0], node.origin[1]);
        k(&agentfx_ir::Affine::translate(ox, oy).mul(&m).mul(&agentfx_ir::Affine::translate(-ox, -oy)))
    }

    /// Chrome parity (M4b): a node whose opacity track is below 1 at
    /// this frame *and* whose own transform is changing at this frame is composited by
    /// Chrome from a raster of the layer taken at its rest transform (the transform at the
    /// time the opacity track is at its maximum, nearest to now), resampled at the frame's
    /// transform. Returns that rest local matrix when the rule applies. Nodes at rest, nodes
    /// that only move, nodes that only fade, and nodes with effects / motion blur / mattes
    /// (or drawn in a matte pass) are not affected. "Changing" compares the committed local
    /// matrix with the previous frame's, so the rule can only switch where the schedule
    /// records a change (damage). LCD (recorded) scenes only, like SDFT: the observation is
    /// from Remotion's sequential capture of JS-driven styles; Chrome re-rasters a seeked
    /// CSS animation per frame (M4b: the title scene loses 0.0025 with the rule).
    fn fade_rest(&self, view: &View, f: usize, n: NodeId, matte: bool) -> Option<Affine> {
        if !self.fade_raster || !self.lcd || matte || f == 0 {
            return None;
        }
        let st = view.states[n];
        if st.opacity <= 0.0 || st.opacity >= OPAQUE {
            return None;
        }
        let node = &self.scene.nodes[n];
        if !node.effects.is_empty() || node.motion_blur.is_some() || self.dynamic_subtree[n] {
            return None;
        }
        let tracks = &self.sched.tracks[n];
        let has = |p: &str| tracks.iter().any(|&i| self.scene.tracks[i].property == p);
        if !has("opacity") || !has("transform") {
            return None;
        }
        let near = |a: Affine, b: Affine| a.as_coeffs().iter().zip(b.as_coeffs()).all(|(x, y)| (x - y).abs() < 1e-9);
        let cur = k(&st.local);
        if near(cur, k(&self.sched.states[f - 1][n].local)) {
            return None;
        }
        let t_rest = fade_rest_time(tracks.iter().map(|&i| &self.scene.tracks[i]), view.t)?;
        let rest = self.local_at(n, t_rest);
        let singular = |a: Affine| a.determinant().abs() < 1e-9;
        if near(rest, cur) || singular(rest) || singular(k(&st.chain)) {
            return None;
        }
        Some(rest)
    }

    /// Draw `n` from a raster of its inner content (content + children, grayscale text)
    /// taken at the rest transform `rest`, resampled bilinearly at the frame's transform,
    /// under the node's clips and opacity. See `fade_rest`.
    #[allow(clippy::too_many_arguments)]
    fn draw_fade_raster(&mut self, ctx: &mut RenderContext, view: &View, f: usize, n: NodeId, base: Affine, region: [f64; 4], alpha: f64, matte: bool, rest: Affine) -> Result<()> {
        let _p = crate::prof::scope("fade.raster");
        let scene = self.scene;
        let node = &scene.nodes[n];
        let st = view.states[n];
        let pchain = self.sched.parents[n].map(|p| k(&view.states[p].chain)).unwrap_or(Affine::IDENTITY);
        let cur_chain = k(&st.chain);
        // the node's own rest transform under the frame's ancestors (M4b: the whole chain at
        // the rest frame, ancestors at rest too, measured marginally worse)
        let rest_chain = pchain * rest;
        if rest_chain.determinant().abs() < 1e-9 {
            return self.draw_node_vector(ctx, view, f, n, base, region, alpha, matte);
        }
        // device (rest) -> device (this frame), and back
        let to_cur = cur_chain * rest_chain.inverse();
        let to_rest = rest_chain * cur_chain.inverse();
        let Some(ink) = self.sub_ink(true, f, n) else { return Ok(()) };
        // the raster covers the subtree at rest and at the frame (children are culled against
        // the frame's inks), within the viewport neighbourhood
        let rest_ink = transform_bbox(&ra(to_rest), ink);
        let vp = [-64.0, -64.0, scene.width as f64 + 128.0, scene.height as f64 + 128.0];
        let near_vp = union(vp, transform_bbox(&ra(to_rest), vp));
        let full = intersect(union(schedule::inflate(ink, 2.0), schedule::inflate(rest_ink, 2.0)), near_vp);
        let Some(r) = irect(full) else { return Ok(()) };
        if r[2] > 4096 || r[3] > 4096 {
            return self.draw_node_vector(ctx, view, f, n, base, region, alpha, matte);
        }
        let key = {
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            n.hash(&mut hs);
            for c in rest_chain.as_coeffs() {
                c.to_bits().hash(&mut hs);
            }
            r.hash(&mut hs);
            self.hash_subtree(view, n, cur_chain.inverse(), &mut hs);
            hs.finish()
        };
        let px = match &self.fade_cache {
            Some((k0, px)) if *k0 == key => {
                self.stats.cache_hits += 1;
                px.clone()
            }
            _ => {
                // the subtree's chains moved from the frame's to the rest transform
                let mut states: Vec<NodeState> = view.states.to_vec();
                let mut stack = vec![n];
                while let Some(x) = stack.pop() {
                    states[x].chain = ra(to_rest * k(&states[x].chain));
                    stack.extend(scene.nodes[x].children.iter().copied());
                }
                states[n].local = ra(rest);
                let v2 = View { t: view.t, states: &states, content: view.content };
                let px = Arc::new(self.render_view(&v2, f, r, Some(n), 0, matte)?);
                self.stats.offscreen += 1;
                self.stats.offscreen_px += r[2] as u64 * r[3] as u64;
                self.fade_cache = Some((key, px.clone()));
                px
            }
        };
        // composite: clips, opacity (one raster, so a per-image alpha equals a group layer)
        let mut clips_pushed = 0;
        for c in &node.clips {
            let rr = c.rect;
            let p = kurbo::RoundedRect::from_rect(Rect::new(rr[0], rr[1], rr[0] + rr[2], rr[1] + rr[3]), kurbo::RoundedRectRadii::new(c.radius[0], c.radius[1], c.radius[2], c.radius[3])).to_path(0.1);
            ctx.set_transform(base * pchain);
            ctx.set_fill_rule(Fill::NonZero);
            ctx.push_clip_path(&p);
            clips_pushed += 1;
        }
        let op = (st.opacity * alpha) as f32;
        let mut pushed = 0;
        if node.blend != agentfx_ir::BlendMode::Normal {
            ctx.set_transform(Affine::IDENTITY);
            ctx.push_layer(None, Some(blend(node.blend)), Some(op), None, None);
            pushed += 1;
        }
        ctx.set_transform(base * to_cur);
        draw_pixmap_q(ctx, px, r[0] as f64, r[1] as f64, if pushed > 0 { 1.0 } else { op }, peniko::ImageQuality::Medium);
        for _ in 0..pushed {
            ctx.pop_layer();
        }
        for _ in 0..clips_pushed {
            ctx.pop_clip();
        }
        Ok(())
    }

    /// Effects / motion blur: render the subtree offscreen (per sample), apply the node's
    /// effects in order, composite with the node's clips/opacity/blend.
    #[allow(clippy::too_many_arguments)]
    fn draw_offscreen_node(&mut self, ctx: &mut RenderContext, view: &View, f: usize, n: NodeId, base: Affine, region: [f64; 4], alpha: f64, matte: bool) -> Result<()> {
        let scene = self.scene;
        let node = &scene.nodes[n];
        let st = view.states[n];
        let s = st.chain.max_scale();
        let tracks = &self.sched.tracks[n];
        let params: Vec<[f64; crate::look::NPARAM]> = node.effects.iter().enumerate().map(|(i, e)| schedule::effect_params(scene, tracks, i, e, view.t)).collect();
        // sweep / rays look at the whole subtree (its ink box): render all of it
        let whole = node.effects.iter().any(|e| matches!(e, Effect::Sweep { .. } | Effect::Rays { .. } | Effect::Rgbsplit { .. } | Effect::Glitch { .. } | Effect::Vhs { .. } | Effect::Dither { .. } | Effect::Ascii { .. }));
        let margin: f64 = if whole { 1e7 } else { node.effects.iter().zip(&params).map(|(e, p)| schedule::effect_margin(scene, e, p, s)).sum::<f64>() + 2.0 };
        let Some(ink) = self.sub_ink(true, f, n) else { return Ok(()) };
        // (raw = unclipped) ink already includes the effect margins; the inner content must be rendered with
        // margin around the visible region so blurs see their neighbourhood
        // whole-subtree effects: everything of the subtree that can reach the frame (their ink
        // margins can be far larger than the frame: rays stream across it)
        // Digital looks use cell/global neighborhoods. Match the full-raster cache
        // extent even on the first draw so clipping cannot change the pattern.
        let digital = node.effects.iter().any(|e| matches!(e, Effect::Rgbsplit { .. } | Effect::Glitch { .. } | Effect::Vhs { .. } | Effect::Dither { .. } | Effect::Ascii { .. }));
        let o = if digital && ink[2] <= 4096.0 && ink[3] <= 4096.0 {
            ink
        } else if whole {
            let frame = [-64.0, -64.0, scene.width as f64 + 128.0, scene.height as f64 + 128.0];
            intersect(ink, frame)
        } else {
            intersect(ink, schedule::inflate(region, margin))
        };
        let Some(o) = irect(o) else { return Ok(()) };
        // translation-invariant effect cache: a subtree that is static inside and only
        // moves rigidly (no matte / motion blur) reuses its filtered raster, placed at the
        // new offset (bilinear for fractional moves; blurred content hides it)
        let fx_cacheable = self.use_cache && !self.dynamic_subtree[n];
        let (px, ox, oy, smooth) = 'px: {
            if fx_cacheable {
                let c = k(&st.chain).as_coeffs();
                // Only fully blurred output (blur effects, sigma >= 1.5 px) may be placed at a
                // different subpixel phase (bilinear); glow / drop shadow keep the sharp
                // subtree, so their cache key includes the fractional offset (exact reuse).
                let smooth_ok = node.effects.iter().all(|e| matches!(e, Effect::Blur { .. })) && params.iter().all(|p| p[0] * s >= 1.5) && self.fx_smooth;
                let mut key = self.effect_key(view, n, &params);
                if node.effects.iter().any(|e| e.position_dependent()) {
                    // light effects depend on where the subtree sits relative to the light
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    key.hash(&mut h);
                    c[4].to_bits().hash(&mut h);
                    c[5].to_bits().hash(&mut h);
                    key = h.finish();
                }
                if !smooth_ok {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    key.hash(&mut h);
                    (c[4] - c[4].floor()).to_bits().hash(&mut h);
                    (c[5] - c[5].floor()).to_bits().hash(&mut h);
                    key = h.finish();
                }
                if let Some((px, x0, y0, tx0, ty0)) = self.fx_cache.get(&key).cloned() {
                    self.stats.cache_hits += 1;
                    let (dx, dy) = (c[4] - tx0, c[5] - ty0);
                    let smooth = dx.fract().abs() > 1e-9 || dy.fract().abs() > 1e-9;
                    break 'px (px, x0 as f64 + dx, y0 as f64 + dy, smooth);
                }
                let cnt = self.fx_seen.entry(key).or_insert(0);
                *cnt += 1;
                if *cnt >= 2 {
                    if let Some(full) = irect(ink).filter(|r| r[2] <= 4096 && r[3] <= 4096) {
                        let px = Arc::new(self.effect_pixmap(view, f, n, full, &params, s, matte)?);
                        let bytes = full[2] as usize * full[3] as usize * 4;
                        if self.cache_bytes + bytes > CACHE_LIMIT {
                            self.cache.clear();
                            self.fx_cache.clear();
                            self.cache_bytes = 0;
                        }
                        self.cache_bytes += bytes;
                        self.fx_cache.insert(key, (px.clone(), full[0], full[1], c[4], c[5]));
                        self.stats.cache_builds += 1;
                        break 'px (px, full[0] as f64, full[1] as f64, false);
                    }
                }
            }
            (Arc::new(self.effect_pixmap(view, f, n, o, &params, s, matte)?), o[0] as f64, o[1] as f64, false)
        };
        // composite with the node's clips / opacity / blend
        let pchain = self.sched.parents[n].map(|p| k(&view.states[p].chain)).unwrap_or(Affine::IDENTITY);
        let mut pushed = 0;
        for c in &node.clips {
            let rr = c.rect;
            let p = kurbo::RoundedRect::from_rect(Rect::new(rr[0], rr[1], rr[0] + rr[2], rr[1] + rr[3]), kurbo::RoundedRectRadii::new(c.radius[0], c.radius[1], c.radius[2], c.radius[3])).to_path(0.1);
            ctx.set_transform(base * pchain);
            ctx.push_clip_layer(&p);
            pushed += 1;
        }
        let op = (st.opacity * alpha) as f32;
        if op < 1.0 || node.blend != agentfx_ir::BlendMode::Normal {
            ctx.set_transform(Affine::IDENTITY);
            ctx.push_layer(None, Some(blend(node.blend)), Some(op), None, None);
            pushed += 1;
        }
        ctx.set_transform(base);
        if smooth {
            draw_pixmap_q(ctx, px, ox, oy, 1.0, peniko::ImageQuality::Medium);
        } else {
            draw_pixmap(ctx, px, ox.round(), oy.round());
        }
        for _ in 0..pushed {
            ctx.pop_layer();
        }
        Ok(())
    }
}

/// Profiler stage name of a node effect (`AGENTFX_PROFILE`).
fn fx_name(e: &Effect) -> &'static str {
    match e {
        Effect::Blur { .. } => "fx.blur",
        Effect::DropShadow { .. } => "fx.drop_shadow",
        Effect::Glow { .. } => "fx.glow",
        Effect::Riso { .. } => "fx.riso",
        Effect::Halftone { .. } => "fx.halftone",
        Effect::Grain { .. } => "fx.grain",
        Effect::Paper { .. } => "fx.paper",
        Effect::Shadow { .. } => "fx.shadow",
        Effect::Sweep { .. } => "fx.sweep",
        Effect::Rays { .. } => "fx.rays",
        Effect::Matte { .. } => "fx.matte",
        Effect::Color { .. } => "fx.color",
        Effect::Rgbsplit { .. } => "fx.rgbsplit",
        Effect::Glitch { .. } => "fx.glitch",
        Effect::Vhs { .. } => "fx.vhs",
        Effect::Dither { .. } => "fx.dither",
        Effect::Ascii { .. } => "fx.ascii",
    }
}

/// Rest time of a fading layer (`Painter::fade_rest`): the time of the opacity keyframe
/// with the highest value, the one nearest to `t` among equals. `None` without opacity
/// keyframes.
pub fn fade_rest_time<'a>(tracks: impl Iterator<Item = &'a agentfx_ir::Track>, t: f64) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None;
    for tr in tracks.filter(|tr| tr.property == "opacity") {
        for kf in &tr.keyframes {
            let agentfx_ir::Value::Number(v) = &kf.value else { continue };
            let v = *v;
            let tk = tr.delay + kf.offset * tr.duration;
            let better = match best {
                None => true,
                Some((bv, bt)) => v > bv + 1e-9 || (v >= bv - 1e-9 && (tk - t).abs() < (bt - t).abs()),
            };
            if better {
                best = Some((v, tk));
            }
        }
    }
    best.map(|(_, tk)| tk)
}

/// Skia-style subpixel glyph positioning: device x snapped to floor((x + 1/8) * 4) / 4,
/// device y rounded, for runs whose transform is a positive axis-aligned scale.
fn snap_glyphs(t: Affine, glyphs: &[vello_cpu::Glyph]) -> Vec<vello_cpu::Glyph> {
    let c = t.as_coeffs();
    if c[1] != 0.0 || c[2] != 0.0 || c[0] <= 0.0 || c[3] <= 0.0 {
        return glyphs.to_vec();
    }
    glyphs
        .iter()
        .map(|g| {
            let dx = c[0] * g.x as f64 + c[4];
            let dy = c[3] * g.y as f64 + c[5];
            let qx = ((dx + 0.125) * 4.0).floor() / 4.0;
            let qy = (dy + 0.5).floor();
            vello_cpu::Glyph { id: g.id, x: ((qx - c[4]) / c[0]) as f32, y: ((qy - c[5]) / c[3]) as f32 }
        })
        .collect()
}

/// Draw a premultiplied pixmap 1:1 at (x, y) in the current transform's space.
fn draw_pixmap(ctx: &mut RenderContext, px: Arc<Pixmap>, x: f64, y: f64) {
    draw_pixmap_alpha(ctx, px, x, y, 1.0)
}

fn draw_pixmap_alpha(ctx: &mut RenderContext, px: Arc<Pixmap>, x: f64, y: f64, alpha: f32) {
    draw_pixmap_q(ctx, px, x, y, alpha, peniko::ImageQuality::Low)
}

fn draw_pixmap_q(ctx: &mut RenderContext, px: Arc<Pixmap>, x: f64, y: f64, alpha: f32, quality: peniko::ImageQuality) {
    let (w, h) = (px.width() as f64, px.height() as f64);
    ctx.set_paint(Paint::Image(peniko::ImageBrush {
        image: ImageSource::Pixmap(px),
        sampler: peniko::ImageSampler { x_extend: peniko::Extend::Pad, y_extend: peniko::Extend::Pad, quality, alpha },
    }));
    ctx.set_paint_transform(Affine::translate((x, y)));
    ctx.set_fill_rule(Fill::NonZero);
    ctx.fill_rect(&Rect::new(x, y, x + w, y + h));
    ctx.reset_paint_transform();
}

#[allow(dead_code)]
fn _unused(_: &Shape) -> Rect {
    shape_rect(&Shape::Rect([0.0; 4]))
}

/// Write a (premultiplied) pixmap as PNG.
pub fn save_png(px: &Pixmap, path: &Path) -> Result<()> {
    let png = px.clone().into_png().map_err(|e| anyhow::anyhow!("png: {e}"))?;
    std::fs::write(path, png)?;
    Ok(())
}

#[cfg(test)]
mod fade_tests {
    use super::fade_rest_time;
    use agentfx_ir::{Easing, FillMode, Keyframe, Track, Value};

    fn opacity(delay: f64, duration: f64, kfs: &[(f64, f64)]) -> Track {
        Track {
            node: 0,
            property: "opacity".into(),
            order: 0,
            label: String::new(),
            delay,
            duration,
            fill: FillMode::Both,
            easing: Easing::Linear,
            keyframes: kfs.iter().map(|&(o, v)| Keyframe { offset: o, value: Value::Number(v), easing: None }).collect(),
            stagger: None,
        }
    }

    #[test]
    fn rest_is_the_nearest_opacity_maximum() {
        // fade in over 0..1 s, at rest 1..3 s, fade out 3..4 s
        let tr = opacity(0.0, 4.0, &[(0.0, 0.0), (0.25, 1.0), (0.75, 1.0), (1.0, 0.0)]);
        let tracks = [tr];
        assert_eq!(fade_rest_time(tracks.iter(), 0.5), Some(1.0));
        assert_eq!(fade_rest_time(tracks.iter(), 3.5), Some(3.0));
        // a layer that never reaches 1: its maximum is the rest
        let tr = opacity(0.0, 2.0, &[(0.0, 0.9), (0.5, 0.9), (1.0, 0.1)]);
        let tracks = [tr];
        assert_eq!(fade_rest_time(tracks.iter(), 1.5), Some(1.0));
        assert_eq!(fade_rest_time(std::iter::empty(), 0.0), None);
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn resized_contexts_match_fresh_rgba() {
        let mut node = agentfx_ir::Node::new("translucent rounded rect");
        node.content = Some(Content::Rect {
            rect: agentfx_ir::RRect { rect: [3.25, 4.5, 105.5, 80.25], radius: [8.0; 4] },
            color: [0.2, 0.6, 0.8, 0.5],
        });
        node.opacity = 0.75;
        let scene = Scene {
            version: agentfx_ir::IR_VERSION, width: 160, height: 128, fps: 30, frames: 1,
            nodes: vec![node], tracks: vec![], assets: vec![], fonts: vec![],
            camera: None, text_aa: Default::default(),
        };
        let prep = Prepared::new(&scene, Path::new("."), None, None).unwrap();
        let info = Info::new(&scene, &prep);
        let sched = schedule::build(&scene, &info).unwrap();
        for threads in [0, 2] {
            let mut pooled = Painter::new(&scene, &prep, &sched, &info, threads);
            // Grow and shrink across tile boundaries, move the origin, and clear old ink.
            for r in [[0, 0, 160, 128], [5, 7, 17, 9], [0, 0, 65, 67],
                      [120, 100, 33, 19], [0, 0, 129, 97], [3, 4, 1, 1]] {
                let actual = pooled.render_rect(0, r).unwrap();
                let mut fresh = Painter::new(&scene, &prep, &sched, &info, threads);
                let expected = fresh.render_rect(0, r).unwrap();
                assert_eq!((actual.width(), actual.height()), (r[2] as u16, r[3] as u16));
                assert_eq!(actual.data_as_u8_slice(), expected.data_as_u8_slice(), "threads={threads}, rect={r:?}");
            }
            assert_eq!(pooled.stats.ctx_created, 1);
            // An in-use context must never be reused by a nested offscreen render.
            let outer = pooled.take_ctx(20, 20, threads);
            let inner = pooled.take_ctx(10, 10, 0);
            assert_eq!(pooled.stats.ctx_created, 2);
            pooled.give_ctx(inner);
            pooled.give_ctx(outer);
            let ctx = pooled.take_ctx(31, 37, threads);
            assert_eq!(ctx.render_settings().num_threads, threads);
            assert_eq!(pooled.stats.ctx_created, 2);
            pooled.give_ctx(ctx);
        }
    }
}

#[cfg(test)]
mod cloud_tests {
    use super::*;

    /// A small cloud scene: field -> text "re" (Inter) -> spiral, `samples` motion blur.
    fn scene(samples: u32, wrap: bool) -> Scene {
        let font = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/scenes/_shared/Inter.ttf");
        let cloud = serde_json::json!({"type": "cloud", "count": 600, "seed": 3, "radius": 1.6, "radius_jitter": 0.2,
            "palette": [[0.95, 0.93, 0.9, 1], [0.91, 0.27, 0.18, 1]], "weights": [0.7, 0.3],
            "forms": [
                {"shape": {"type": "field", "rect": [0, 0, 160, 96], "flow": [20, 0]}, "drift": {"amp": 4, "scale": 60}},
                {"at": 0.2, "dur": 0.5, "stagger": 0.3, "stagger_by": "x", "easing": {"spring": {"mass": 1, "stiffness": 150, "damping": 15, "velocity": 0}},
                 "shape": {"type": "text", "text": "re", "font": 0, "size": 80, "weight": 800, "pos": [0, 0], "max_width": 160, "align": "center"},
                 "color": {"by": "y", "split": [0.7, 0.3]}},
                {"at": 1.2, "dur": 0.4, "stagger": 0.2, "stagger_by": "distance", "shape": {"type": "spiral", "center": [80, 48], "radius": 44, "squash": 0.7, "spin": 40}}
            ]});
        let mut nodes = vec![agentfx_ir::Node::new("root")];
        let mut bg = agentfx_ir::Node::new("bg");
        bg.content = Some(Content::Rect { rect: agentfx_ir::RRect { rect: [0.0, 0.0, 160.0, 96.0], radius: [0.0; 4] }, color: [0.05, 0.04, 0.04, 1.0] });
        let mut c = agentfx_ir::Node::new("cloud");
        c.content = Some(serde_json::from_value(cloud).unwrap());
        let mb = (samples > 1).then_some(agentfx_ir::MotionBlur { samples, shutter: 0.5 });
        nodes.push(bg);
        if wrap {
            // generic motion-blur path: blur on a parent group
            let mut g = agentfx_ir::Node::new("group");
            g.motion_blur = mb;
            g.children = vec![3];
            nodes.push(g);
            nodes.push(c);
        } else {
            c.motion_blur = mb;
            nodes.push(c);
        }
        nodes[0].children = vec![1, 2];
        Scene { version: agentfx_ir::IR_VERSION, width: 160, height: 96, fps: 30, frames: 60, nodes, tracks: vec![], assets: vec![], fonts: vec![agentfx_ir::FontAsset { path: font.into(), index: 0 }], camera: None, text_aa: Default::default() }
    }

    fn frame(scene: &Scene, f: usize) -> Vec<u8> {
        let prep = Prepared::new(scene, Path::new("."), None, None).unwrap();
        let info = Info::new(scene, &prep);
        let sched = schedule::build(scene, &info).unwrap();
        let mut p = Painter::new(scene, &prep, &sched, &info, 0);
        p.render_rect(f, [0, 0, 160, 96]).unwrap().data_as_u8_slice().to_vec()
    }

    /// 160x96 scene from JSON nodes/tracks (parity tests).
    fn json_scene(nodes: serde_json::Value, tracks: serde_json::Value) -> Scene {
        serde_json::from_value(serde_json::json!({"version": 1, "width": 160, "height": 96, "fps": 30, "frames": 30, "nodes": nodes, "tracks": tracks})).unwrap()
    }
    fn px(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let o = (y * 160 + x) * 4;
        [buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]
    }

    /// Glyph run of `text` in Inter at `size`, baseline at (x, y): (ids + positions, font path).
    fn inter_run(text: &str, size: f32, x: f32, y: f32) -> (serde_json::Value, String) {
        use skrifa::MetadataProvider;
        let font = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/scenes/_shared/Inter.ttf").to_string();
        let data = std::fs::read(&font).unwrap();
        let f = skrifa::FontRef::new(&data).unwrap();
        let gm = f.glyph_metrics(skrifa::instance::Size::new(size), skrifa::instance::LocationRef::default());
        let mut pen = x;
        let mut v = vec![];
        for c in text.chars() {
            let g = f.charmap().map(c).unwrap();
            v.push(serde_json::json!([g.to_u32(), pen, y]));
            pen += gm.advance_width(g).unwrap();
        }
        (serde_json::json!(v), font)
    }

    fn text_scene(text: &str, fx: serde_json::Value, stroke: Option<serde_json::Value>, tracks: serde_json::Value) -> Scene {
        let (glyphs, font) = inter_run(text, 40.0, 10.0, 60.0);
        let mut g = serde_json::json!({"op": "glyphs", "font": 0, "size": 40.0, "brush": {"solid": [1, 1, 1, 1]}, "glyphs": glyphs});
        if let Some(st) = stroke {
            g["stroke"] = st;
        }
        let mut content = serde_json::json!({"type": "paint", "rect": [0, 0, 160, 96], "items": [g]});
        if !fx.is_null() {
            content["text"] = fx;
        }
        let mut sc = json_scene(serde_json::json!([{"name": "root", "children": [1, 2]}, {"name": "bg", "content": {"type": "rect", "rect": {"rect": [0, 0, 160, 96]}, "color": [0, 0, 0, 1]}}, {"name": "t", "content": content}]), tracks);
        sc.fonts = vec![agentfx_ir::FontAsset { path: font, index: 0 }];
        if let Err(e) = agentfx_ir::validate::validate(&sc, Path::new(".")) {
            panic!("{e:?}");
        }
        sc
    }

    /// Sum of channel `ch` over a column range (ink measure).
    fn ink_cols(buf: &[u8], x0: usize, x1: usize, ch: usize) -> u64 {
        let mut s = 0u64;
        for y in 0..96 {
            for x in x0..x1 {
                s += buf[(y * 160 + x) * 4 + ch] as u64;
            }
        }
        s
    }

    #[test]
    fn text_stroke_outlines_glyphs_under_the_fill() {
        let plain = frame(&text_scene("II", serde_json::Value::Null, None, serde_json::json!([])), 0);
        let st = frame(&text_scene("II", serde_json::Value::Null, Some(serde_json::json!({"width": 6, "brush": {"solid": [1, 0, 0, 1]}})), serde_json::json!([])), 0);
        // red appears around the glyphs, white fill stays on top (green channel unchanged inside)
        assert!(ink_cols(&st, 0, 160, 0) > ink_cols(&plain, 0, 160, 0) + 2000, "stroke adds red ink");
        // under the fill: the white (green channel) keeps at least the plain glyph ink
        assert!(ink_cols(&st, 0, 160, 1) + 255 >= ink_cols(&plain, 0, 160, 1));
        let over = frame(&text_scene("II", serde_json::Value::Null, Some(serde_json::json!({"width": 6, "brush": {"solid": [1, 0, 0, 1]}, "over": true})), serde_json::json!([])), 0);
        assert!(ink_cols(&over, 0, 160, 1) < ink_cols(&st, 0, 160, 1), "a stroke over the fill covers part of it");
    }

    #[test]
    fn range_selector_hides_units_and_offset_track_moves_the_range() {
        // "IIII": 4 chars; opacity 0 on [0, 0.5] + offset (track 0 -> 0.5 over frames 0..30)
        let fx = serde_json::json!({"animators": [{"by": "char", "selector": {"type": "range", "start": 0, "end": 0.5}, "opacity": 0}]});
        let tr = serde_json::json!([{"node": 2, "property": "animators.0.offset", "delay": 0, "duration": 1, "fill": "both", "easing": "linear",
            "keyframes": [{"offset": 0, "value": {"number": 0}}, {"offset": 1, "value": {"number": 0.5}}]}]);
        let sc = text_scene("IIII", fx, None, tr);
        let rest = frame(&text_scene("IIII", serde_json::Value::Null, None, serde_json::json!([])), 0);
        let a = frame(&sc, 0);
        let (glyphs, _) = inter_run("IIII", 40.0, 10.0, 60.0);
        let xs: Vec<usize> = glyphs.as_array().unwrap().iter().map(|g| g[1].as_f64().unwrap() as usize).collect();
        let mid = xs[2];
        assert_eq!(ink_cols(&a, 0, mid, 1), 0, "first half hidden");
        assert_eq!(ink_cols(&a, mid, 160, 1), ink_cols(&rest, mid, 160, 1), "second half untouched");
        let b = frame(&sc, 29);
        assert!(ink_cols(&b, 0, xs[1], 1) > 0 && ink_cols(&b, xs[2], 160, 1) < ink_cols(&rest, xs[2], 160, 1) / 2);
        // the planner sees the change (damage) and the rest frames repeat
        let prep = Prepared::new(&sc, Path::new("."), None, None).unwrap();
        let info = Info::new(&sc, &prep);
        assert_ne!(info.sig(&sc, 2, 0.0), info.sig(&sc, 2, 0.5));
        assert_eq!(info.sig(&sc, 2, 1.5), info.sig(&sc, 2, 2.0));
    }

    #[test]
    fn key_selector_times_units_and_color_track_recolors() {
        // word units with their own key times: word 0 moves at 0.1 s, word 1 at 0.6 s
        let fx = serde_json::json!({"animators": [{"by": "word", "selector": {"type": "keys", "units": [[[0.1, 1], [0.3, 0]], [[0.6, 1], [0.8, 0]]]}, "translate": [0, 40]}]});
        let sc = text_scene("I I", fx, None, serde_json::json!([]));
        let prep = Prepared::new(&sc, Path::new("."), None, None).unwrap();
        let PContent::TextFx(b) = prep.contents[2].as_ref().unwrap() else { panic!() };
        assert_eq!(b.0.units(), [2, 2, 1]);
        let info = Info::new(&sc, &prep);
        assert!(info.time_dependent(&sc, 2));
        let b0 = info.bounds(&sc, 2, 0.0).unwrap();
        let b1 = info.bounds(&sc, 2, 0.45).unwrap();
        // before its key both words sit 40 px down; between, only word 1 does; after: rest
        assert!(b0[1] > b1[1] + 30.0 && b1[3] > b0[3] + 20.0, "{b0:?} {b1:?}");
        assert_eq!(info.bounds(&sc, 2, 1.0), Some([0.0, 0.0, 160.0, 96.0]));
        // text.fill: white -> red over the clip
        let tr = serde_json::json!([{"node": 2, "property": "text.fill", "delay": 0, "duration": 1, "fill": "both", "easing": "linear",
            "keyframes": [{"offset": 0, "value": {"color": [1, 1, 1, 1]}}, {"offset": 1, "value": {"color": [1, 0, 0, 1]}}]}]);
        let sc = text_scene("II", serde_json::Value::Null, None, tr);
        let (a, z) = (frame(&sc, 0), frame(&sc, 29));
        assert!(ink_cols(&a, 0, 160, 1) > 1000 && ink_cols(&z, 0, 160, 1) < ink_cols(&a, 0, 160, 1) / 10);
        assert!(ink_cols(&z, 0, 160, 0) > 1000);
    }

    #[test]
    fn text_on_a_path_follows_it_and_hides_off_the_end() {
        // a vertical path: the glyphs stack downwards from (80, 0)
        let fx = serde_json::json!({"path": {"d": "M80 0 L80 96", "offset": 0}});
        let tr = serde_json::json!([{"node": 2, "property": "text.path-offset", "delay": 0, "duration": 1, "fill": "both", "easing": "linear",
            "keyframes": [{"offset": 0, "value": {"number": 0}}, {"offset": 1, "value": {"number": 400}}]}]);
        let sc = text_scene("III", fx, None, tr);
        let prep = Prepared::new(&sc, Path::new("."), None, None).unwrap();
        let info = Info::new(&sc, &prep);
        let b = info.bounds(&sc, 2, 0.0).unwrap();
        assert!(b[2] < 50.0 && b[3] > 20.0, "vertical run: {b:?}");
        assert!(b[0] > 30.0 && b[0] < 80.0, "left of the path (baseline on it): {b:?}");
        assert_eq!(info.bounds(&sc, 2, 1.0), None, "pushed off the end: hidden");
    }

    #[test]
    fn path_color_tracks_recolor_fill_and_stroke() {
        let s = json_scene(serde_json::json!([
            {"children": [1]},
            {"content": {"type": "path", "d": "M20 20 L140 20 L140 76 L20 76 Z", "fill": {"solid": [1, 0, 0, 1]},
                "stroke": {"width": 8, "brush": {"solid": [0, 0, 1, 1]}}}}
        ]), serde_json::json!([
            {"node": 1, "property": "fill.color", "delay": 0, "duration": 1, "fill": "both", "easing": "linear",
             "keyframes": [{"offset": 0, "value": {"color": [1, 0, 0, 1]}}, {"offset": 1, "value": {"color": [0, 1, 0, 1]}}]},
            {"node": 1, "property": "stroke.color", "delay": 0, "duration": 1, "fill": "both", "easing": "linear",
             "keyframes": [{"offset": 0, "value": {"color": [0, 0, 1, 1]}}, {"offset": 1, "value": {"color": [1, 1, 0, 1]}}]}
        ]));
        assert!(agentfx_ir::validate::validate(&s, Path::new(".")).is_ok(), "{:?}", agentfx_ir::validate::validate(&s, Path::new(".")));
        let f0 = frame(&s, 0);
        assert_eq!(px(&f0, 80, 48), [255, 0, 0, 255]);
        assert_eq!(px(&f0, 20, 48), [0, 0, 255, 255]);
        let f15 = frame(&s, 15);
        assert_eq!(px(&f15, 80, 48), [128, 128, 0, 255]);
        let f29 = frame(&s, 29);
        assert_eq!(px(&f29, 80, 48)[1], 247);
        assert_eq!(px(&f29, 20, 48)[0], 247);
        // the schedule sees the change (no frame skipped as a rest frame)
        let prep = Prepared::new(&s, Path::new("."), None, None).unwrap();
        let info = Info::new(&s, &prep);
        assert_ne!(info.sig(&s, 1, 0.0), info.sig(&s, 1, 0.5));
    }

    #[test]
    fn color_effect_animates_and_blend_modes_draw() {
        // a grey rect under a color effect whose brightness track runs 1 -> 0 (black)
        let s = json_scene(serde_json::json!([
            {"children": [1]},
            {"children": [2], "effects": [{"type": "color", "brightness": 1, "saturate": 0}]},
            {"content": {"type": "rect", "rect": {"rect": [0, 0, 160, 96]}, "color": [1, 0, 0, 1]}}
        ]), serde_json::json!([
            {"node": 1, "property": "effects.0.brightness", "delay": 0, "duration": 1, "fill": "both", "easing": "linear",
             "keyframes": [{"offset": 0, "value": {"number": 1}}, {"offset": 1, "value": {"number": 0}}]}
        ]));
        assert!(agentfx_ir::validate::validate(&s, Path::new(".")).is_ok());
        // saturate(0) of red = rgb(54, 54, 54) (CSS luma weights)
        assert_eq!(px(&frame(&s, 0), 80, 48), [54, 54, 54, 255]);
        assert_eq!(px(&frame(&s, 29), 80, 48)[0], 2);
        // difference: white over red -> cyan; src-atop clips a group to the backdrop's alpha
        for (mode, want) in [("difference", [0, 255, 255, 255]), ("exclusion", [0, 255, 255, 255]), ("luminosity", [255, 255, 255, 255]), ("plus", [255, 255, 255, 255])] {
            let s = json_scene(serde_json::json!([
                {"children": [1, 2]},
                {"content": {"type": "rect", "rect": {"rect": [0, 0, 160, 96]}, "color": [1, 0, 0, 1]}},
                {"blend": mode, "content": {"type": "rect", "rect": {"rect": [40, 0, 120, 96]}, "color": [1, 1, 1, 1]}}
            ]), serde_json::json!([]));
            let f = frame(&s, 0);
            assert_eq!(px(&f, 80, 48), want, "{mode}");
            assert_eq!(px(&f, 10, 48), [255, 0, 0, 255], "{mode}");
        }
        let s = json_scene(serde_json::json!([
            {"children": [1]},
            {"content": {"type": "paint", "rect": [0, 0, 160, 96], "items": [
                {"op": "push"},
                {"op": "fill", "shape": {"rect": [0, 0, 80, 96]}, "brush": {"solid": [1, 1, 1, 1]}},
                {"op": "push", "blend": "src-atop"},
                {"op": "fill", "shape": {"rect": [0, 0, 160, 96]}, "brush": {"solid": [0, 0, 1, 1]}},
                {"op": "pop"}, {"op": "pop"}]}}
        ]), serde_json::json!([]));
        let f = frame(&s, 0);
        assert_eq!(px(&f, 40, 48), [0, 0, 255, 255]);
        assert_eq!(px(&f, 120, 48)[3], 0);
    }

    #[test]
    fn golden_cloud_frames() {
        // frames: field, mid-morph into text, text at rest, spiral (spinning), with motion blur
        let s = scene(6, false);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/cloud");
        for f in [0usize, 12, 30, 55] {
            let got = frame(&s, f);
            let path = dir.join(format!("cloud-f{f}.png"));
            if agentfx_ir::env::var("AGENTFX_BLESS").is_ok() {
                std::fs::create_dir_all(&dir).unwrap();
                let mut px = Pixmap::new(160, 96);
                px.data_as_u8_slice_mut().copy_from_slice(&got);
                save_png(&px, &path).unwrap();
                continue;
            }
            let want = load_png_premul(&path).unwrap_or_else(|e| panic!("{e:#} (AGENTFX_BLESS=1 writes the goldens)"));
            let diff = want.data_as_u8_slice().iter().zip(&got).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
            assert!(diff <= 1, "frame {f}: max diff {diff} vs {}", path.display());
        }
    }

    #[test]
    fn rest_frames_have_no_damage_and_mblur_fast_path_matches() {
        let s = scene(1, false);
        let prep = Prepared::new(&s, Path::new("."), None, None).unwrap();
        let info = Info::new(&s, &prep);
        let sched = schedule::build(&s, &info).unwrap();
        // the field drifts: every early frame is damaged; the text at rest (0.2 + 0.3 + 0.5 s
        // ... 1.2 s) repeats
        assert!(!sched.damage[3].is_empty());
        for f in 31..36 {
            assert!(sched.damage[f].is_empty(), "frame {f} {:?}", sched.damage[f]);
        }
        assert!(!sched.damage[40].is_empty());
        // motion blur: splat fast path (cloud node) == generic subtree path (parent group)
        let (a, b) = (scene(6, false), scene(6, true));
        for f in [12usize, 40, 55] {
            let (x, y) = (frame(&a, f), frame(&b, f));
            let diff = x.iter().zip(&y).map(|(p, q)| (*p as i32 - *q as i32).abs()).max().unwrap();
            assert!(diff <= 1, "frame {f}: {diff}");
        }
    }
}

#[cfg(test)]
mod digital_tests {
    use super::*;
    #[test]
    fn digital_cache_matches_direct_and_planner_tracks_time() {
        for name in ["rgbsplit", "glitch", "vhs", "dither", "ascii"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../examples/ir/looks/{name}.ir.json"));
            let scene = Scene::load(&path).unwrap();
            let prep = Prepared::new(&scene, path.parent().unwrap(), None, None).unwrap();
            let info = Info::new(&scene, &prep);
            let sched = schedule::build(&scene, &info).unwrap();
            let live = matches!(name, "glitch" | "vhs");
            assert_eq!(!sched.damage[1].is_empty(), live, "{name}");
            let mut cached = Painter::new(&scene, &prep, &sched, &info, 0);
            cached.use_cache = true;
            let mut direct = Painter::new(&scene, &prep, &sched, &info, 0);
            direct.use_cache = false;
            for frame in [0, 0, 0, 24, 47] {
                let rect = [0, 0, scene.width as i32, scene.height as i32];
                let a = cached.render_rect(frame, rect).unwrap().data_as_u8_slice().to_vec();
                let b = direct.render_rect(frame, rect).unwrap().data_as_u8_slice().to_vec();
                assert_eq!(a, b, "{name} frame {frame}");
            }
        }
    }
}
