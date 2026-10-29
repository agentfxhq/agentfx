//! Dirty-rectangle, premultiplied-alpha composition using tiny-skia.
//!
//! Infallible entry points panic on invalid geometry or allocation failure.
use crate::model::Affine;
use anyhow::{Context, Result, ensure};
use std::borrow::Cow;
use std::sync::OnceLock;
use tiny_skia::{BlendMode, FillRule, FilterQuality, Mask, PathBuilder, Pixmap, PixmapPaint, Transform};

/// A premultiplied raster covering `rect = [x, y, width, height]` in CSS pixels.
/// Treat the public image fields as immutable after construction: mip levels are cached.
pub struct LayerImage {
    pub pixmap: Pixmap,
    pub rect: [f64; 4],
    pub opaque: bool,
    mips: OnceLock<Vec<Pixmap>>,
}
impl LayerImage {
    /// Load a straight-alpha PNG, premultiply it, and determine whether it is opaque.
    pub fn load_png(path: &std::path::Path, rect: [f64; 4]) -> Result<Self> {
        ensure!(valid_rect(rect), "layer rectangle must be finite with positive dimensions");
        let pixmap = Pixmap::load_png(path).with_context(|| format!("loading layer PNG {}", path.display()))?;
        Ok(Self::from_pixmap(pixmap, rect))
    }
    /// Construct from premultiplied pixels; panics on invalid rectangle geometry.
    pub fn from_pixmap(pixmap: Pixmap, rect: [f64; 4]) -> Self {
        assert!(valid_rect(rect), "layer rectangle must be finite with positive dimensions");
        let opaque = pixmap.data().chunks_exact(4).all(|p| p[3] == 255);
        Self { pixmap, rect, opaque, mips: OnceLock::new() }
    }
}

/// Rounded rectangle in page coordinates, mapped to frame coordinates.
#[derive(Clone, Debug)]
pub struct Clip {
    pub rect: [f64; 4],
    /// Circular radii in top-left, top-right, bottom-right, bottom-left order.
    pub radius: [f64; 4],
    pub transform: Affine,
}
/// Paint-order scene tree, with isolated CSS opacity groups.
#[derive(Clone, Debug)]
pub enum Node {
    /// Draw a layer with a page-to-frame transform and intersected clips.
    Image { image: usize, transform: Affine, opacity: f32, clips: Vec<Clip>, blend: BlendMode },
    /// Composite children in isolation, then apply clips and group opacity once.
    Group { opacity: f32, clips: Vec<Clip>, children: Vec<Node>, blend: BlendMode },
}
/// Integer frame-space damage rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IRect { pub x: i32, pub y: i32, pub w: u32, pub h: u32 }
/// Image reconstruction filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter { Bilinear, Bicubic }

/// Persistent frame plus reusable dirty-sized image and mask buffers.
pub struct Compositor {
    frame: Pixmap,
    filter: Filter,
    mipmaps: bool,
    pixmaps: Vec<Pixmap>,
    masks: Vec<Mask>,
}
impl Compositor {
    /// Allocate a transparent frame. Panics if dimensions cannot be allocated.
    pub fn new(width: u32, height: u32, filter: Filter, mipmaps: bool) -> Self {
        assert!(width <= i32::MAX as u32 && height <= i32::MAX as u32, "frame dimensions exceed signed coordinates");
        Self { frame: Pixmap::new(width, height).expect("invalid or unallocatable frame dimensions"), filter, mipmaps, pixmaps: Vec::new(), masks: Vec::new() }
    }
    /// Replace damage with a fresh composition; all other frame bytes are preserved.
    /// Invalid scene inputs panic because this interface has no error return.
    pub fn compose(&mut self, nodes: &[Node], images: &[LayerImage], dirty: &[IRect]) {
        for &r in dirty {
            let x = i64::from(r.x).max(0);
            let y = i64::from(r.y).max(0);
            let right = (i64::from(r.x) + i64::from(r.w)).min(i64::from(self.frame.width()));
            let bottom = (i64::from(r.y) + i64::from(r.h)).min(i64::from(self.frame.height()));
            if right <= x || bottom <= y { continue; }
            let r = IRect { x: x as i32, y: y as i32, w: (right-x) as u32, h: (bottom-y) as u32 };
            let mut scratch = self.take_pixmap(r.w, r.h);
            self.render(&mut scratch, nodes, images, r);
            let stride = self.frame.width() as usize * 4;
            for row in 0..r.h as usize {
                let dst = (r.y as usize + row)*stride + r.x as usize*4;
                let src = row*r.w as usize*4;
                self.frame.data_mut()[dst..dst+r.w as usize*4].copy_from_slice(&scratch.data()[src..src+r.w as usize*4]);
            }
            self.pixmaps.push(scratch);
        }
    }
    /// Current premultiplied frame.
    #[allow(dead_code)]
    pub fn frame(&self) -> &Pixmap { &self.frame }
    /// Straight RGBA; borrows the frame when all pixels are opaque.
    pub fn frame_rgba(&self) -> Cow<'_, [u8]> {
        if self.frame.data().chunks_exact(4).all(|p| p[3] == 255) { return Cow::Borrowed(self.frame.data()); }
        let mut data = self.frame.data().to_vec();
        for p in data.chunks_exact_mut(4) {
            let a = u32::from(p[3]);
            for c in &mut p[..3] { *c = if a == 0 { 0 } else { ((u32::from(*c)*255+a/2)/a).min(255) as u8 }; }
        }
        Cow::Owned(data)
    }
    fn take_pixmap(&mut self, w: u32, h: u32) -> Pixmap {
        let mut p = if let Some(i) = self.pixmaps.iter().position(|p| p.width()==w && p.height()==h) {
            self.pixmaps.swap_remove(i)
        } else {
            // Replace an unused buffer when damage dimensions change.
            self.pixmaps.pop();
            Pixmap::new(w,h).expect("could not allocate dirty scratch pixmap")
        };
        p.data_mut().fill(0);
        p
    }
    fn take_mask(&mut self, r: IRect) -> Mask {
        let mut mask = if let Some(i) = self.masks.iter().position(|m| m.width()==r.w && m.height()==r.h) {
            self.masks.swap_remove(i)
        } else {
            self.masks.pop();
            Mask::new(r.w,r.h).expect("could not allocate clip mask")
        };
        mask.data_mut().fill(0);
        mask
    }
    fn mask(&mut self, clips: &[Clip], r: IRect) -> Option<Mask> {
        if clips.is_empty() { return None; }
        let mut mask = self.take_mask(r);
        mask.data_mut().fill(255);
        let mut coverage = self.take_mask(r);
        for clip in clips {
            let path = rounded_path(clip);
            let t = Affine::translate(-f64::from(r.x),-f64::from(r.y)).mul(&clip.transform);
            if determinant(t) == 0.0 { mask.data_mut().fill(0); continue; }
            coverage.data_mut().fill(0);
            coverage.fill_path(&path, FillRule::Winding, true, skia_transform(t));
            for (dst, src) in mask.data_mut().iter_mut().zip(coverage.data()) {
                *dst = ((u16::from(*dst) * u16::from(*src) + 127) / 255) as u8;
            }
        }
        self.masks.push(coverage);
        Some(mask)
    }
    fn render(&mut self, target: &mut Pixmap, nodes: &[Node], images: &[LayerImage], r: IRect) {
        for node in nodes {
            let (opacity, clips) = match node {
                Node::Image { opacity, clips, .. } | Node::Group { opacity, clips, .. } => (*opacity, clips),
            };
            let blend = match node {
                Node::Image { blend, .. } | Node::Group { blend, .. } => *blend,
            };
            assert!(opacity.is_finite() && opacity <= 1.0, "opacity must be finite and <= 1");
            if opacity <= 0.0 { continue; }
            match node {
                Node::Image { image, transform, .. } => {
                    let image = images.get(*image).expect("image node references missing layer");
                    assert!(valid_rect(image.rect), "invalid layer rectangle");
                    validate_transform(*transform);
                    if determinant(*transform)==0.0 || !intersects(image.rect, *transform, r) { continue; }
                    let [x,y,w,h] = image.rect;
                    let base = transform.mul(&Affine::translate(x,y)).mul(&Affine::scale(w/f64::from(image.pixmap.width()),h/f64::from(image.pixmap.height())));
                    let mut source = &image.pixmap;
                    if self.mipmaps && base.max_scale() < 0.5 {
                        for level in image.mips.get_or_init(|| mip_chain(&image.pixmap)) {
                            let scale = base.mul(&Affine::scale(f64::from(image.pixmap.width())/f64::from(level.width()),f64::from(image.pixmap.height())/f64::from(level.height()))).max_scale();
                            if scale > 1.0 { break; }
                            source = level;
                        }
                    }
                    let t = Affine::translate(-f64::from(r.x),-f64::from(r.y)).mul(transform).mul(&Affine::translate(x,y)).mul(&Affine::scale(w/f64::from(source.width()), h/f64::from(source.height())));
                    if image.opaque && opacity == 1.0 && blend == BlendMode::SourceOver && clips.is_empty() && t.a==1.0 && t.d==1.0 && t.b==0.0 && t.c==0.0 && t.e.fract()==0.0 && t.f.fract()==0.0 {
                        copy_image(target, source, t.e as i64, t.f as i64);
                        continue;
                    }
                    let mask = self.mask(clips, r);
                    let paint = PixmapPaint { opacity, blend_mode: blend, quality: match self.filter { Filter::Bilinear => FilterQuality::Bilinear, Filter::Bicubic => FilterQuality::Bicubic }, ..PixmapPaint::default() };
                    target.draw_pixmap(0,0,source.as_ref(), &paint, skia_transform(t), mask.as_ref());
                    if let Some(mask) = mask { self.masks.push(mask); }
                }
                Node::Group { children, .. } => {
                    if opacity == 1.0 && clips.is_empty() && blend == BlendMode::SourceOver { self.render(target,children,images,r); continue; }
                    let mut group = self.take_pixmap(r.w,r.h);
                    self.render(&mut group,children,images,r);
                    let mask = self.mask(clips,r);
                    target.draw_pixmap(0,0,group.as_ref(), &PixmapPaint { opacity, blend_mode: blend, ..PixmapPaint::default() }, Transform::identity(), mask.as_ref());
                    if let Some(mask) = mask { self.masks.push(mask); }
                    self.pixmaps.push(group);
                }
            }
        }
    }
}
fn valid_rect(r: [f64;4]) -> bool { r.iter().all(|v| v.is_finite()) && r[2]>0.0 && r[3]>0.0 && (r[0]+r[2]).is_finite() && (r[1]+r[3]).is_finite() }
fn validate_transform(t: Affine) { assert!([t.a,t.b,t.c,t.d,t.e,t.f].iter().all(|v| v.is_finite()), "non-finite affine transform"); }
fn determinant(t: Affine) -> f64 { t.a*t.d-t.b*t.c }
fn skia_transform(t: Affine) -> Transform {
    validate_transform(t);
    assert!([t.a,t.b,t.c,t.d,t.e,t.f].iter().all(|v| (*v as f32).is_finite()), "affine transform exceeds tiny-skia f32 range");
    Transform::from_row(t.a as f32,t.b as f32,t.c as f32,t.d as f32,t.e as f32,t.f as f32)
}
fn intersects(rect: [f64;4], t: Affine, r: IRect) -> bool {
    let [x,y,w,h]=rect;
    let corners = [t.apply(x,y),t.apply(x+w,y),t.apply(x,y+h),t.apply(x+w,y+h)];
    let min_x=corners.iter().map(|p|p.0).fold(f64::INFINITY,f64::min);
    let max_x=corners.iter().map(|p|p.0).fold(f64::NEG_INFINITY,f64::max);
    let min_y=corners.iter().map(|p|p.1).fold(f64::INFINITY,f64::min);
    let max_y=corners.iter().map(|p|p.1).fold(f64::NEG_INFINITY,f64::max);
    max_x>f64::from(r.x) && max_y>f64::from(r.y) && min_x<f64::from(r.x)+f64::from(r.w) && min_y<f64::from(r.y)+f64::from(r.h)
}
fn copy_image(dst: &mut Pixmap, src: &Pixmap, x: i64, y: i64) {
    let left=x.max(0); let top=y.max(0);
    let right=(x+i64::from(src.width())).min(i64::from(dst.width()));
    let bottom=(y+i64::from(src.height())).min(i64::from(dst.height()));
    if right<=left || bottom<=top { return; }
    let stride=dst.width() as usize*4;
    for row in top..bottom {
        let d=row as usize*stride+left as usize*4;
        let s=((row-y) as usize*src.width() as usize+(left-x) as usize)*4;
        let n=(right-left) as usize*4;
        dst.data_mut()[d..d+n].copy_from_slice(&src.data()[s..s+n]);
    }
}
fn rounded_path(clip: &Clip) -> tiny_skia::Path {
    assert!(valid_rect(clip.rect), "clip rectangle must be finite with positive dimensions");
    assert!(clip.radius.iter().all(|r|r.is_finite() && *r>=0.0), "clip radii must be finite and nonnegative");
    validate_transform(clip.transform);
    let [x,y,w,h]=clip.rect;
    let [tl,tr,br,bl]=clip.radius;
    // CSS scales all radii together when adjacent corners overlap.
    let factor=1.0_f64.min(w/(tl+tr)).min(w/(bl+br)).min(h/(tl+bl)).min(h/(tr+br));
    let [tl,tr,br,bl]=clip.radius.map(|r|r*factor);
    let mut b=PathBuilder::new();
    let k=0.5522847498307936;
    b.move_to((x+tl) as f32,y as f32);
    b.line_to((x+w-tr) as f32,y as f32);
    b.cubic_to((x+w-tr+k*tr) as f32,y as f32,(x+w) as f32,(y+tr-k*tr) as f32,(x+w) as f32,(y+tr) as f32);
    b.line_to((x+w) as f32,(y+h-br) as f32);
    b.cubic_to((x+w) as f32,(y+h-br+k*br) as f32,(x+w-br+k*br) as f32,(y+h) as f32,(x+w-br) as f32,(y+h) as f32);
    b.line_to((x+bl) as f32,(y+h) as f32);
    b.cubic_to((x+bl-k*bl) as f32,(y+h) as f32,x as f32,(y+h-bl+k*bl) as f32,x as f32,(y+h-bl) as f32);
    b.line_to(x as f32,(y+tl) as f32);
    b.cubic_to(x as f32,(y+tl-k*tl) as f32,(x+tl-k*tl) as f32,y as f32,(x+tl) as f32,y as f32);
    b.close();
    b.finish().expect("clip path exceeds tiny-skia coordinate range")
}
fn mip_chain(base: &Pixmap) -> Vec<Pixmap> {
    let mut levels: Vec<Pixmap>=Vec::new();
    loop {
        let src=levels.last().unwrap_or(base);
        if src.width()==1 && src.height()==1 { break; }
        let w=(src.width()/2).max(1); let h=(src.height()/2).max(1);
        let mut dst=Pixmap::new(w,h).expect("could not allocate mip level");
        // Area box averaging preserves the whole extent for odd-sized images too.
        let sx=f64::from(src.width())/f64::from(w); let sy=f64::from(src.height())/f64::from(h);
        for y in 0..h { for x in 0..w {
            let (x0,x1,y0,y1)=(f64::from(x)*sx,f64::from(x+1)*sx,f64::from(y)*sy,f64::from(y+1)*sy);
            let mut sum=[0.0;4];
            for iy in y0.floor() as u32..(y1.ceil() as u32).min(src.height()) { for ix in x0.floor() as u32..(x1.ceil() as u32).min(src.width()) {
                let weight=(x1.min(f64::from(ix+1))-x0.max(f64::from(ix)))*(y1.min(f64::from(iy+1))-y0.max(f64::from(iy)));
                let offset=(iy as usize*src.width() as usize+ix as usize)*4;
                for c in 0..4 { sum[c]+=f64::from(src.data()[offset+c])*weight; }
            }}
            let offset=(y as usize*w as usize+x as usize)*4;
            for c in 0..4 { dst.data_mut()[offset+c]=(sum[c]/(sx*sy)).round() as u8; }
        }}
        levels.push(dst);
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;
    fn solid(w:u32,h:u32,rgba:[u8;4]) -> LayerImage {
        let mut p=Pixmap::new(w,h).unwrap();
        for px in p.data_mut().chunks_exact_mut(4) { px.copy_from_slice(&rgba); }
        LayerImage::from_pixmap(p,[0.0,0.0,f64::from(w),f64::from(h)])
    }
    fn node(image:usize,transform:Affine,opacity:f32) -> Node { Node::Image { image,transform,opacity,clips:vec![],blend:BlendMode::SourceOver } }
    fn full(w:u32,h:u32) -> IRect { IRect{x:0,y:0,w,h} }
    fn pixel(c:&Compositor,x:usize,y:usize) -> &[u8] { let i=(y*c.frame.width() as usize+x)*4; &c.frame.data()[i..i+4] }
    #[test]
    fn exact_identity_and_translation() {
        let mut layer=solid(4,3,[0,0,0,255]);
        for (i,p) in layer.pixmap.data_mut().chunks_exact_mut(4).enumerate() { p[0]=i as u8*17; p[1]=255-i as u8*11; }
        let mut c=Compositor::new(8,8,Filter::Bicubic,true);
        for t in [Affine::IDENTITY,Affine::translate(2.0,3.0)] {
            c.compose(&[node(0,t,1.0)],std::slice::from_ref(&layer),&[full(8,8)]);
            for y in 0..3 { for x in 0..4 { assert_eq!(pixel(&c,x+t.e as usize,y+t.f as usize),&layer.pixmap.data()[(y*4+x)*4..(y*4+x+1)*4]); }}
        }
    }
    #[test]
    fn dirty_only_including_overlap_and_outside() {
        let images=[solid(8,8,[255,0,0,255]),solid(8,8,[0,255,0,255])];
        let mut c=Compositor::new(8,8,Filter::Bilinear,false);
        c.compose(&[node(0,Affine::IDENTITY,1.0)],&images,&[full(8,8)]);
        let dirty=[IRect{x:-2,y:2,w:5,h:3},IRect{x:2,y:3,w:2,h:3},IRect{x:i32::MAX,y:0,w:u32::MAX,h:1}];
        c.compose(&[node(1,Affine::IDENTITY,1.0)],&images,&dirty);
        for y in 0..8 { for x in 0..8 { let changed=(x<3 && (2..5).contains(&y)) || ((2..4).contains(&x) && (3..6).contains(&y)); assert_eq!(pixel(&c,x,y),if changed { &[0,255,0,255] } else { &[255,0,0,255] }); }}
        c.compose(&[],&images,&[IRect{x:1,y:1,w:1,h:1}]);
        assert_eq!(pixel(&c,1,1),&[0,0,0,0]);
    }
    #[test]
    fn opacity_and_group_isolation() {
        let images=[solid(4,4,[0,0,255,255]),solid(4,4,[255,0,0,255]),solid(4,4,[0,255,0,255])];
        let mut c=Compositor::new(4,4,Filter::Bilinear,false);
        c.compose(&[node(0,Affine::IDENTITY,1.0),node(1,Affine::IDENTITY,0.5)],&images,&[full(4,4)]);
        for (a,b) in pixel(&c,2,2).iter().zip([128,0,127,255]) { assert!((i32::from(*a)-b).abs()<=1); }
        let group=Node::Group { opacity:0.5,clips:vec![],children:vec![node(1,Affine::IDENTITY,1.0),node(2,Affine::IDENTITY,1.0)],blend:BlendMode::SourceOver };
        c.compose(&[node(0,Affine::IDENTITY,1.0),group],&images,&[full(4,4)]);
        let grouped=pixel(&c,2,2).to_vec();
        for (a,b) in grouped.iter().zip([0,128,127,255]) { assert!((i32::from(*a)-b).abs()<=1); }
        c.compose(&[node(0,Affine::IDENTITY,1.0),node(1,Affine::IDENTITY,0.5),node(2,Affine::IDENTITY,0.5)],&images,&[full(4,4)]);
        assert_ne!(pixel(&c,2,2),grouped);
        c.compose(&[node(1,Affine::IDENTITY,0.5)],&images,&[full(4,4)]);
        assert_eq!(pixel(&c,2,2),&[128,0,0,128]);
        assert_eq!(&c.frame_rgba()[0..4],&[255,0,0,128]);
    }
    #[test]
    fn rounded_and_enclosing_clips() {
        let images=[solid(20,20,[255,0,0,255])];
        let clip=Clip { rect:[0.0,0.0,20.0,20.0],radius:[8.0;4],transform:Affine::IDENTITY };
        let child=Node::Image {image:0,transform:Affine::IDENTITY,opacity:1.0,clips:vec![clip.clone()],blend:BlendMode::SourceOver};
        let group=Node::Group {opacity:1.0,clips:vec![Clip{rect:[0.0,0.0,10.0,20.0],radius:[0.0;4],transform:Affine::IDENTITY}],children:vec![child],blend:BlendMode::SourceOver};
        let mut c=Compositor::new(20,20,Filter::Bilinear,false);
        c.compose(&[group],&images,&[full(20,20)]);
        assert_eq!(pixel(&c,0,0),&[0;4]);
        assert_eq!(pixel(&c,5,10),&[255,0,0,255]);
        assert_eq!(pixel(&c,15,10),&[0;4]);
        assert!(c.frame.data().chunks_exact(4).any(|p|p[3]>0 && p[3]<255));
    }
    #[test]
    fn scaled_image_and_mips() {
        let mut low=solid(32,32,[0,0,0,255]);
        for y in 0..32 { for x in 0..32 { let i=(y*32+x)*4; low.pixmap.data_mut()[i]=(x*6) as u8; low.pixmap.data_mut()[i+1]=(y*6) as u8; }}
        for scale in [2,4] {
            let mut high=solid(32*scale,32*scale,[0,0,0,255]);
            for y in 0..32*scale { for x in 0..32*scale { let i=((y*32*scale+x)*4) as usize; let j=(((y/scale)*32+x/scale)*4) as usize; high.pixmap.data_mut()[i..i+4].copy_from_slice(&low.pixmap.data()[j..j+4]); }}
            let mut c=Compositor::new(32,32,Filter::Bilinear,true);
            c.compose(&[node(0,Affine::scale(1.0/f64::from(scale),1.0/f64::from(scale)),1.0)],std::slice::from_ref(&high),&[full(32,32)]);
            let error:f64=c.frame.data().iter().zip(low.pixmap.data()).map(|(a,b)|(f64::from(*a)-f64::from(*b)).abs()).sum::<f64>()/4096.0;
            assert!(error<1.0,"scale {scale}: mean error {error}");
            if scale==4 { assert!(high.mips.get().is_some()); }
        }
    }
    #[test]
    fn transformed_partial_matches_full() {
        let images=[solid(32,24,[90,40,20,128])];
        let t=Affine::translate(20.0,8.0).mul(&Affine::rotate_deg(19.0)).mul(&Affine::scale(1.3,0.8));
        let n=Node::Image {image:0,transform:t,opacity:0.8,clips:vec![Clip{rect:[0.0,0.0,32.0,24.0],radius:[5.0;4],transform:t}],blend:BlendMode::SourceOver};
        let mut a=Compositor::new(64,64,Filter::Bicubic,true);
        let mut b=Compositor::new(64,64,Filter::Bicubic,true);
        a.compose(std::slice::from_ref(&n),&images,&[full(64,64)]);
        b.compose(&[n],&images,&[IRect{x:17,y:13,w:25,h:20}]);
        for y in 13..33 { for x in 17..42 { for (a,b) in pixel(&a,x,y).iter().zip(pixel(&b,x,y)) { assert!((i16::from(*a)-i16::from(*b)).abs()<=1); } }}
    }
    #[test]
    #[ignore]
    fn benchmark() {
        let images=[solid(1920,1080,[30,40,50,255]),solid(2304,1296,[160,100,40,255])];
        let t=Affine::translate(100.0,50.0).mul(&Affine::scale(0.7,0.7));
        let nodes=[node(0,Affine::IDENTITY,1.0),Node::Image{image:1,transform:t,opacity:1.0,clips:vec![Clip{rect:[0.0,0.0,2304.0,1296.0],radius:[100.0;4],transform:t}],blend:BlendMode::SourceOver}];
        let mut c=Compositor::new(1920,1080,Filter::Bilinear,true);
        for r in [full(1920,1080),IRect{x:150,y:80,w:300,h:200}] {
            for _ in 0..3 {c.compose(&nodes,&images,&[r]);}
            let start=std::time::Instant::now();
            for _ in 0..30 {c.compose(std::hint::black_box(&nodes),&images,&[r]); std::hint::black_box(c.frame());}
            println!("{}x{} dirty: {:.3} ms/frame (30 frames, bilinear)",r.w,r.h,start.elapsed().as_secs_f64()*1000.0/30.0);
        }
    }
}
