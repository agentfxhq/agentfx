//! Lottie content (`Content::Lottie`, P5): a Bodymovin JSON file evaluated per frame by velato
//! (linebender, Apache-2.0 OR MIT; its renderer-agnostic `RenderSink`) and drawn as vectors
//! into the vello_cpu context. Frame n is a pure function of n: the Lottie frame comes from
//! the scene time only (fractional frames, so any output fps samples the curves exactly).
use anyhow::{Context, Result};
use kurbo::Affine;
use peniko::BlendMode;
use std::path::Path;
use vello_cpu::{PaintType, RenderContext};

pub struct Lottie {
    pub comp: velato::Composition,
    pub rect: [f64; 4],
    pub start: f64,
    pub offset: f64,
    pub rate: f64,
    pub looped: bool,
    pub hold: bool,
}

impl Lottie {
    pub fn load(path: &Path, rect: [f64; 4], start: f64, offset: f64, rate: f64, looped: bool, hold: bool) -> Result<Lottie> {
        let raw = std::fs::read(path).with_context(|| format!("lottie {}", path.display()))?;
        let comp = velato::Composition::from_slice(&raw).map_err(|e| anyhow::anyhow!("lottie {}: {e}", path.display()))?;
        Ok(Lottie { comp, rect, start, offset, rate, looped, hold })
    }

    /// Lottie frame at scene time `t` (None: nothing drawn).
    pub fn frame_at(&self, t: f64) -> Option<f64> {
        frame_at(self.comp.frames.clone(), self.comp.frame_rate, t, self.start, self.offset, self.rate, self.looped, self.hold)
    }

    /// Composition space -> content space: contain fit, centered in `rect`.
    pub fn fit(&self) -> Affine {
        let (cw, ch) = (self.comp.width.max(1) as f64, self.comp.height.max(1) as f64);
        let s = (self.rect[2] / cw).min(self.rect[3] / ch);
        let (ox, oy) = (self.rect[0] + (self.rect[2] - cw * s) / 2.0, self.rect[1] + (self.rect[3] - ch * s) / 2.0);
        Affine::translate((ox, oy)) * Affine::scale(s)
    }

    pub fn draw(&self, ctx: &mut RenderContext, chain: Affine, frame: f64) {
        let mut sink = Sink { ctx, depth: 0 };
        velato::Renderer::new().append(&self.comp, frame, chain * self.fit(), 1.0, &mut sink);
        // a malformed file must not leave layers open on the shared context
        while sink.depth > 0 {
            sink.ctx.pop_layer();
            sink.depth -= 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn frame_at(frames: std::ops::Range<f64>, fps: f64, t: f64, start: f64, offset: f64, rate: f64, looped: bool, hold: bool) -> Option<f64> {
    let src = offset + (t - start) * rate;
    let len = frames.end - frames.start;
    if src < 0.0 || len <= 0.0 || fps <= 0.0 {
        return None;
    }
    let f = src * fps;
    // the last frame is `op - 1` (op is exclusive)
    let last = (len - 1.0).max(0.0);
    if f <= last {
        Some(frames.start + f)
    } else if looped {
        Some(frames.start + f.rem_euclid(len))
    } else if hold {
        Some(frames.start + last)
    } else {
        None
    }
}

struct Sink<'a> {
    ctx: &'a mut RenderContext,
    depth: u32,
}

fn paint(b: &peniko::Brush) -> Option<PaintType> {
    match b {
        peniko::Brush::Solid(c) => Some(PaintType::Solid(*c)),
        peniko::Brush::Gradient(g) => Some(PaintType::Gradient(g.clone())),
        // velato does not load embedded images (draw_image covers its image layers)
        peniko::Brush::Image(_) => None,
    }
}

impl velato::RenderSink for Sink<'_> {
    fn push_layer(&mut self, blend: impl Into<BlendMode>, alpha: f32, transform: Affine, shape: &impl kurbo::Shape) {
        self.ctx.set_transform(transform);
        let clip = shape.to_path(0.1);
        self.ctx.push_layer(Some(&clip), Some(blend.into()), Some(alpha), None, None);
        self.depth += 1;
    }

    fn push_clip_layer(&mut self, transform: Affine, shape: &impl kurbo::Shape) {
        self.ctx.set_transform(transform);
        self.ctx.push_clip_layer(&shape.to_path(0.1));
        self.depth += 1;
    }

    fn pop_layer(&mut self) {
        if self.depth > 0 {
            self.ctx.pop_layer();
            self.depth -= 1;
        }
    }

    fn draw(&mut self, stroke: Option<&kurbo::Stroke>, transform: Affine, brush: &peniko::Brush, shape: &impl kurbo::Shape) {
        let Some(p) = paint(brush) else { return };
        let path = shape.to_path(0.1);
        self.ctx.set_transform(transform);
        self.ctx.set_paint(p);
        self.ctx.reset_paint_transform();
        match stroke {
            Some(s) => {
                self.ctx.set_stroke(s.clone());
                self.ctx.stroke_path(&path);
            }
            None => {
                self.ctx.set_fill_rule(peniko::Fill::NonZero);
                self.ctx.fill_path(&path);
            }
        }
    }

    fn draw_image(&mut self, _image: &velato::model::ImageAsset, _transform: Affine, _alpha: f64) {
        // raster image layers are not drawn (vector shapes only; IMPORTS.md)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_time_maps_to_lottie_frames() {
        // 0..60 at 30 fps, scene starts at 2 s
        let f = |t, looped, hold| frame_at(0.0..60.0, 30.0, t, 2.0, 0.0, 1.0, looped, hold);
        assert_eq!(f(1.0, false, false), None);
        assert_eq!(f(2.0, false, false), Some(0.0));
        assert!((f(3.0, false, false).unwrap() - 30.0).abs() < 1e-9);
        assert_eq!(f(5.0, false, false), None);
        assert_eq!(f(5.0, false, true), Some(59.0));
        assert!((f(5.0, true, false).unwrap() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn draws_a_minimal_animation() {
        // one shape layer: a rectangle whose position animates
        let json = r#"{"v":"5.7.0","fr":30,"ip":0,"op":30,"w":100,"h":100,"layers":[{"ty":4,"ind":1,"ip":0,"op":30,"st":0,
          "ks":{"o":{"a":0,"k":100},"r":{"a":0,"k":0},"p":{"a":1,"k":[{"t":0,"s":[20,50,0],"o":{"x":[0.33],"y":[0]},"i":{"x":[0.67],"y":[1]}},{"t":29,"s":[80,50,0]}]},"a":{"a":0,"k":[0,0,0]},"s":{"a":0,"k":[100,100,100]}},
          "shapes":[{"ty":"gr","it":[{"ty":"rc","p":{"a":0,"k":[0,0]},"s":{"a":0,"k":[20,20]},"r":{"a":0,"k":0}},{"ty":"fl","c":{"a":0,"k":[1,0,0,1]},"o":{"a":0,"k":100}},{"ty":"tr","p":{"a":0,"k":[0,0]},"a":{"a":0,"k":[0,0]},"s":{"a":0,"k":[100,100]},"r":{"a":0,"k":0},"o":{"a":0,"k":100}}]}]}]}"#;
        let comp = velato::Composition::from_slice(json.as_bytes()).unwrap();
        let l = Lottie { comp, rect: [0.0, 0.0, 100.0, 100.0], start: 0.0, offset: 0.0, rate: 1.0, looped: false, hold: false };
        let red_x = |frame: f64| {
            let mut ctx = RenderContext::new(100, 100);
            l.draw(&mut ctx, Affine::IDENTITY, frame);
            ctx.flush();
            let mut px = vello_cpu::Pixmap::new(100, 100);
            ctx.render(&mut px, &mut vello_cpu::Resources::new());
            let d = px.data();
            let xs: Vec<usize> = (0..100).filter(|&x| d[50 * 100 + x].r > 128).collect();
            xs.iter().sum::<usize>() as f64 / xs.len().max(1) as f64
        };
        assert!((red_x(0.0) - 20.0).abs() < 3.0, "{}", red_x(0.0));
        assert!((red_x(29.0) - 80.0).abs() < 3.0, "{}", red_x(29.0));
    }
}
