//! Painter side of `Content::Scene3d` (child module of `vraster`): geometry built at load,
//! the animated state per frame, screen textures (other nodes rendered offscreen), and a
//! per-node raster cache so a scene that does not change is never rasterised twice.
use super::*;
use crate::s3d::{self, Tex};
use crate::s3d_mesh::{self, Mesh};
use agentfx_ir::s3d::{Scene3d, Shape3d, State3d};
use std::collections::HashSet;

pub struct Prep3d {
    pub c: Scene3d,
    pub meshes: Vec<Option<Arc<Mesh>>>,
    /// image textures (material `asset`) per object
    pub img_tex: Vec<Option<Arc<Tex>>>,
    /// animated properties (others take their base values without a track lookup)
    pub props: HashSet<String>,
}

struct GlyphPen {
    path: BezPath,
    ox: f64,
    oy: f64,
}
impl skrifa::outline::OutlinePen for GlyphPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to((self.ox + x as f64, self.oy - y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to((self.ox + x as f64, self.oy - y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.path.quad_to((self.ox + cx0 as f64, self.oy - cy0 as f64), (self.ox + x as f64, self.oy - y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.path.curve_to((self.ox + cx0 as f64, self.oy - cy0 as f64), (self.ox + cx1 as f64, self.oy - cy1 as f64), (self.ox + x as f64, self.oy - y as f64));
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

/// Font size the text outlines are taken at before scaling to world units.
const TEXT_PX: f64 = 100.0;

/// Outline of shaped text as one path (scene-like coordinates, y down).
fn text_path(shaper: &mut Shaper, fonts: &[FontData], text: &str, font: usize, weight: f64, letter_spacing: f64) -> Result<BezPath> {
    use skrifa::MetadataProvider;
    let tc = agentfx_ir::TextContent {
        text: text.to_string(),
        font,
        size: TEXT_PX,
        weight,
        letter_spacing: letter_spacing * TEXT_PX,
        line_height: 1.15,
        pos: [0.0, 0.0],
        max_width: None,
        align: "left".into(),
        brush: Brush::Solid([1.0; 4]),
        unit: "glyph".into(),
    };
    let sh = shaper.shape(&tc)?;
    let fd = fonts.get(font).context("extrude font out of range")?;
    let fref = skrifa::FontRef::from_index(fd.data.data(), fd.index).map_err(|e| anyhow::anyhow!("font {font}: {e:?}"))?;
    let outlines = fref.outline_glyphs();
    let loc: Vec<skrifa::instance::NormalizedCoord> = sh.coords.iter().map(|c| skrifa::instance::NormalizedCoord::from_bits(*c)).collect();
    let mut pen = GlyphPen { path: BezPath::new(), ox: 0.0, oy: 0.0 };
    for g in &sh.glyphs {
        let Some(og) = outlines.get(skrifa::GlyphId::new(g.id)) else { continue };
        pen.ox = g.x as f64;
        pen.oy = g.y as f64;
        let settings = skrifa::outline::DrawSettings::unhinted(skrifa::instance::Size::new(sh.size), skrifa::instance::LocationRef::new(&loc));
        og.draw(settings, &mut pen).map_err(|e| anyhow::anyhow!("glyph {}: {e:?}", g.id))?;
    }
    Ok(pen.path)
}

/// Geometry and textures of a 3D scene (load time).
pub fn prep3d(c: &Scene3d, fonts: &[FontData], shaper: &mut Option<Shaper>, images: &[Option<Arc<Pixmap>>], base: &Path, tracks: &[&agentfx_ir::Track]) -> Result<Prep3d> {
    let mut meshes = Vec::with_capacity(c.objects.len());
    let mut img_tex = Vec::with_capacity(c.objects.len());
    for (i, o) in c.objects.iter().enumerate() {
        let m = match &o.shape {
            Shape3d::Group => None,
            Shape3d::Box { size, radius } => Some(s3d_mesh::cuboid(*size, *radius)),
            Shape3d::Plane { size } | Shape3d::Screen { size, .. } => Some(s3d_mesh::plane(size[0], size[1])),
            Shape3d::Sphere { radius, segments } => Some(s3d_mesh::sphere(*radius, *segments)),
            Shape3d::Cylinder { radius, radius_top, height, segments } => Some(s3d_mesh::cylinder(*radius, radius_top.unwrap_or(*radius), *height, *segments)),
            Shape3d::Torus { radius, tube, segments } => Some(s3d_mesh::torus(*radius, *tube, *segments)),
            Shape3d::Extrude { text, font, size, weight, letter_spacing, d, depth } => {
                let (path, k) = if let Some(t) = text {
                    if shaper.is_none() {
                        *shaper = Some(Shaper::new(fonts)?);
                    }
                    (text_path(shaper.as_mut().unwrap(), fonts, t, *font, *weight, *letter_spacing).with_context(|| format!("object {i} {:?} text", o.id))?, size / TEXT_PX)
                } else {
                    let p = BezPath::from_svg(d.as_deref().unwrap_or("")).map_err(|e| anyhow::anyhow!("object {i} {:?}: bad path: {e:?}", o.id))?;
                    let b = p.bounding_box();
                    (p, size / b.height().max(b.width() * 1e-3).max(1e-9))
                };
                let pb = path.bounding_box();
                let tol = (pb.width().max(pb.height()) * 8e-4).max(1e-6);
                let cs = s3d_mesh::contours(&path, tol);
                anyhow::ensure!(!cs.is_empty(), "object {i} {:?}: extrude has no outline", o.id);
                Some(s3d_mesh::extrude(&s3d_mesh::center_flip(&cs, k), *depth))
            }
            Shape3d::Mesh { path, size } => {
                let p = base.join(path);
                let src = std::fs::read_to_string(&p).with_context(|| format!("object {i}: reading {}", p.display()))?;
                Some(s3d_mesh::obj(&src, *size).with_context(|| format!("object {i}: {}", p.display()))?)
            }
        };
        meshes.push(m.map(Arc::new));
        let tex = match o.material.asset {
            Some(a) => {
                let px = images.get(a).and_then(|x| x.clone()).with_context(|| format!("object {i}: texture asset {a} not loaded"))?;
                let data: Vec<[u8; 4]> = px.data_as_u8_slice().chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
                Some(Arc::new(Tex::new(px.width() as usize, px.height() as usize, data)))
            }
            None => None,
        };
        img_tex.push(tex);
    }
    let props = tracks.iter().map(|t| t.property.clone()).filter(|p| p.starts_with("s3d.")).collect();
    Ok(Prep3d { c: c.clone(), meshes, img_tex, props })
}

/// The scene's animated values at t (node `n`'s tracks).
pub fn state3d(scene: &Scene, tracks: &[usize], p: &Prep3d, t: f64) -> State3d {
    let props = &p.props;
    p.c.state(
        &mut |prop, base| if props.contains(prop) { schedule::number_at(scene, tracks, prop, t, base) } else { base },
        &mut |prop, base| if props.contains(prop) { schedule::color_at(scene, tracks, prop, t).unwrap_or(base) } else { base },
    )
}

/// Signature of the 3D state (planner).
pub fn sig3d(scene: &Scene, tracks: &[usize], p: &Prep3d, t: f64, h: &mut impl Hasher) {
    state3d(scene, tracks, p, t).hash_into(h);
}

const S3D_CACHE: usize = 3;

impl<'a> Painter<'a> {
    pub(super) fn draw_scene3d(&mut self, ctx: &mut RenderContext, view: &View, f: usize, n: NodeId, p: &Prep3d, chain: Affine) -> Result<()> {
        let scene = self.scene;
        let st = state3d(scene, &self.info.tracks[n], p, view.t);
        let sc_chain = Affine::translate((self.origin.0 as f64, self.origin.1 as f64)) * chain;
        let r = p.c.rect;
        let dev = transform_bbox(&ra(sc_chain), r);
        let ctx_reg = [self.origin.0 as f64, self.origin.1 as f64, ctx.width() as f64, ctx.height() as f64];
        let lim = union([0.0, 0.0, scene.width as f64, scene.height as f64], ctx_reg);
        let bb = intersect(dev, lim);
        let Some(bi) = irect(bb) else { return Ok(()) };
        // screen textures (other nodes' subtrees at this frame)
        let mut tex = p.img_tex.clone();
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        let screens: Vec<(usize, NodeId)> = p.c.screens().collect();
        if !screens.is_empty() {
            let world = st.world(&p.c);
            let v3 = agentfx_ir::s3d::View3::new(&st.cam, p.c.aspect());
            for (oi, src) in screens {
                let Shape3d::Screen { size, rect, res, .. } = &p.c.objects[oi].shape else { continue };
                // texels per scene px: from the screen's size on screen (powers of two, stable while it moves a little)
                let s = if *res > 0.0 {
                    *res
                } else {
                    let corners = [[-size[0] / 2.0, -size[1] / 2.0, 0.0], [size[0] / 2.0, -size[1] / 2.0, 0.0], [size[0] / 2.0, size[1] / 2.0, 0.0], [-size[0] / 2.0, size[1] / 2.0, 0.0]];
                    let pts: Vec<(f64, f64)> = corners
                        .iter()
                        .filter_map(|c| v3.project(world[oi].point(*c)))
                        .map(|(x, y, _)| {
                            let (cx, cy) = agentfx_ir::s3d::ndc_to_rect(r, x, y);
                            sc_chain * kurbo::Point::new(cx, cy)
                        })
                        .map(|q| (q.x, q.y))
                        .collect();
                    let mut w_on = 0.0f64;
                    for k in 0..pts.len() {
                        let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
                        w_on = w_on.max((a.0 - b.0).hypot(a.1 - b.1));
                    }
                    let k = (w_on / rect[2].max(rect[3]).max(1.0)).clamp(0.25, 2.0);
                    2f64.powf(k.log2().ceil())
                };
                let (tw, th) = (((rect[2] * s).round() as usize).clamp(1, 4096), ((rect[3] * s).round() as usize).clamp(1, 4096));
                let src_chain = k(&view.states[src].chain);
                let mut kh = std::collections::hash_map::DefaultHasher::new();
                (src, tw, th).hash(&mut kh);
                for c in src_chain.as_coeffs() {
                    c.to_bits().hash(&mut kh);
                }
                self.hash_subtree(view, src, src_chain.inverse(), &mut kh);
                let key = kh.finish();
                key.hash(&mut hs);
                let t = match self.s3d_tex.iter().find(|(k2, _)| *k2 == key) {
                    Some((_, t)) => t.clone(),
                    None => {
                        let mut c2 = self.take_ctx(tw as u16, th as u16, 0);
                        let sx = tw as f64 / rect[2];
                        let sy = th as f64 / rect[3];
                        let b2 = Affine::scale_non_uniform(sx, sy) * Affine::translate((-rect[0], -rect[1]));
                        self.iso += 1;
                        let saved = std::mem::replace(&mut self.origin, (0, 0));
                        let res = self.draw_node(&mut c2, view, f, src, b2, *rect, 1.0, true);
                        self.origin = saved;
                        self.iso -= 1;
                        res?;
                        c2.flush();
                        let mut px = Pixmap::new(tw as u16, th as u16);
                        c2.render(&mut px, &mut self.res);
                        self.give_ctx(c2);
                        let data: Vec<[u8; 4]> = px.data_as_u8_slice().chunks_exact(4).map(|q| [q[0], q[1], q[2], q[3]]).collect();
                        let t = Arc::new(Tex::new(tw, th, data));
                        self.s3d_tex.insert(0, (key, t.clone()));
                        self.s3d_tex.truncate(8);
                        t
                    }
                };
                tex[oi] = Some(t);
            }
        }
        // raster cache: same state, same screens, same placement -> same pixels
        st.hash_into(&mut hs);
        for c in sc_chain.as_coeffs() {
            c.to_bits().hash(&mut hs);
        }
        bi.hash(&mut hs);
        let key = hs.finish();
        let hit = self.s3d_cache.get(&n).and_then(|v| v.iter().find(|e| e.0 == key)).map(|e| e.1.clone());
        let px = match hit {
            Some(px) => px,
            None => {
                let _p = crate::prof::scope_px("s3d.render", bi[2] as u64 * bi[3] as u64);
                // NDC -> content -> screen -> buffer px
                let m = sc_chain.as_coeffs();
                let (cx, cy, hw, hh) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0, r[2] / 2.0, r[3] / 2.0);
                // content(x, y) = (cx + hw * x, cy - hh * y)
                let ndc = [m[0] * hw, m[1] * hw, -m[2] * hh, -m[3] * hh, m[0] * cx + m[2] * cy + m[4] - bi[0] as f64, m[1] * cx + m[3] * cy + m[5] - bi[1] as f64];
                let axis = m[1].abs() < 1e-9 && m[2].abs() < 1e-9;
                let clip = if axis {
                    None
                } else {
                    // buffer px -> viewport fraction
                    let inv = (Affine::translate((bi[0] as f64, bi[1] as f64)) * Affine::IDENTITY).as_coeffs();
                    let to_content = sc_chain.inverse() * Affine::new(inv);
                    let frac = Affine::new([1.0 / r[2], 0.0, 0.0, 1.0 / r[3], -r[0] / r[2], -r[1] / r[3]]) * to_content;
                    Some(frac.as_coeffs())
                };
                let aa = match agentfx_ir::env::var("AGENTFX_S3D_AA").ok().and_then(|v| v.parse::<u32>().ok()) {
                    Some(a) => a.clamp(1, 4),
                    None => p.c.aa,
                };
                let fr = s3d::Frame { st: &st, tex: &tex, ndc_to_px: ndc, w: bi[2] as usize, h: bi[3] as usize, clip, px_scale: sc_chain.as_coeffs()[0].hypot(sc_chain.as_coeffs()[1]).max(1e-6), aa };
                let mut stats = s3d::Stats::default();
                let buf = s3d::render(&p.c, &p.meshes, &fr, &mut stats, &mut self.s3d_scratch);
                let mut px = Pixmap::new(bi[2] as u16, bi[3] as u16);
                px.data_as_u8_slice_mut().copy_from_slice(&buf);
                let px = Arc::new(px);
                let v = self.s3d_cache.entry(n).or_default();
                v.insert(0, (key, px.clone()));
                v.truncate(S3D_CACHE);
                px
            }
        };
        ctx.set_transform(Affine::IDENTITY);
        draw_pixmap(ctx, px, (bi[0] as i64 - self.origin.0) as f64, (bi[1] as i64 - self.origin.1) as f64);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 160;
    const H: u32 = 96;

    /// 160x96 scene: an optional backdrop rect, a scene3d node (and an optional hidden screen source).
    fn scene(objects: serde_json::Value, extra: serde_json::Value, tracks: serde_json::Value, source: bool) -> Scene {
        let mut nodes = vec![
            serde_json::json!({"name": "root", "children": if source { vec![1, 2] } else { vec![1] }}),
            serde_json::json!({"name": "s3d", "content": {"type": "scene3d", "rect": [0, 0, W, H],
                "camera": {"pos": [0, 1.5, 6], "target": [0, 0, 0], "fov": 40},
                "lights": [{"kind": "directional", "pos": [-3, 5, 4], "intensity": 0.8}],
                "objects": objects}}),
        ];
        if let serde_json::Value::Object(m) = extra {
            for (k, v) in m {
                nodes[1]["content"][k] = v;
            }
        }
        if source {
            nodes.push(serde_json::json!({"name": "ui", "hidden": true, "content": {"type": "rect", "rect": {"rect": [0, 0, 64, 36]}, "color": [1, 0, 0, 1]}}));
        }
        serde_json::from_value(serde_json::json!({"version": 1, "width": W, "height": H, "fps": 30, "frames": 30, "nodes": nodes, "tracks": tracks})).unwrap()
    }

    fn frame(scene: &Scene, f: usize) -> Vec<u8> {
        let prep = Prepared::new(scene, Path::new("."), None, None).unwrap();
        let info = Info::new(scene, &prep);
        let sched = schedule::build(scene, &info).unwrap();
        let mut p = Painter::new(scene, &prep, &sched, &info, 0);
        p.render_rect(f, [0, 0, W as i32, H as i32]).unwrap().data_as_u8_slice().to_vec()
    }

    fn px(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let o = (y * W as usize + x) * 4;
        [buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]
    }

    fn rich() -> Scene {
        scene(
            serde_json::json!([
                {"shape": {"type": "plane", "size": [20, 20]}, "rot": [-90, 0, 0], "pos": [0, -1, 0], "receive": true, "material": {"kind": "lambert", "color": [0.2, 0.25, 0.2, 1]}},
                {"shape": {"type": "box", "size": [1.4, 1.4, 1.4], "radius": 0.1}, "rot": [0, 30, 0], "pos": [-1.2, -0.3, 0], "material": {"kind": "toon", "color": [0.8, 0.6, 0.3, 1], "outline": 2}},
                {"shape": {"type": "sphere", "radius": 0.6}, "pos": [1.2, -0.4, 0.5], "material": {"kind": "phong", "color": [0.2, 0.4, 1, 1], "specular": 0.5, "emissive": 0.3}},
                {"shape": {"type": "torus", "radius": 0.6, "tube": 0.15}, "pos": [0, 1.2, -1], "rot": [70, 0, 0], "material": {"kind": "wire", "wire_color": [1, 1, 1, 1]}},
                {"shape": {"type": "box", "size": [1, 1, 1]}, "pos": [0, 0, -2], "material": {"kind": "flat", "color": [0, 1, 0, 1], "opacity": 0.5}}
            ]),
            serde_json::json!({"shadow": {"opacity": 0.5, "blur": 2}, "bloom": {"radius": 3}, "fog": {"color": [0, 0, 0, 1], "near": 6, "far": 20}}),
            serde_json::json!([]),
            false,
        )
    }

    #[test]
    fn frames_are_deterministic_across_runs_and_thread_counts() {
        let s = rich();
        let a = frame(&s, 0);
        let b = frame(&s, 0);
        assert_eq!(a, b, "two runs differ");
        let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap().install(|| frame(&s, 0));
        let four = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap().install(|| frame(&s, 0));
        assert_eq!(a, one, "1 thread differs");
        assert_eq!(a, four, "4 threads differ");
        // something was drawn, and the floor covers the bottom rows
        let covered = a.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(covered > (W * H / 3) as usize, "covered {covered}");
        assert_eq!(px(&a, 80, 94)[3], 255, "floor under the camera");
    }

    #[test]
    fn toon_box_takes_its_colour_and_the_sky_stays_transparent() {
        let s = scene(
            serde_json::json!([{"shape": {"type": "box", "size": [2, 2, 2]}, "material": {"kind": "flat", "color": [1, 0.5, 0, 1]}}]),
            serde_json::json!({}),
            serde_json::json!([]),
            false,
        );
        let f = frame(&s, 0);
        // flat material: exactly its colour in the middle
        assert_eq!(px(&f, 80, 48), [255, 128, 0, 255]);
        // corners: nothing there
        assert_eq!(px(&f, 2, 2), [0, 0, 0, 0]);
        // lit material: the face toward the light (top) is brighter than the side away from it
        let s = scene(
            serde_json::json!([{"shape": {"type": "box", "size": [2, 2, 2]}, "rot": [0, 45, 0], "material": {"kind": "lambert", "color": [1, 1, 1, 1]}}]),
            serde_json::json!({"camera": {"pos": [0, 4, 6], "target": [0, 0, 0], "fov": 40}}),
            serde_json::json!([]),
            false,
        );
        let f = frame(&s, 0);
        let (left, right) = (px(&f, 66, 56), px(&f, 94, 56));
        assert!(left[3] == 255 && right[3] == 255);
        assert!(left[0] > right[0] + 20, "light from the left: {left:?} vs {right:?}");
    }

    #[test]
    fn nearer_surface_wins_whatever_the_object_order() {
        let red = serde_json::json!({"shape": {"type": "box", "size": [1, 1, 1]}, "pos": [0, 0, 1], "material": {"kind": "flat", "color": [1, 0, 0, 1]}});
        let blue = serde_json::json!({"shape": {"type": "box", "size": [3, 3, 1]}, "pos": [0, 0, -1], "material": {"kind": "flat", "color": [0, 0, 1, 1]}});
        for objs in [serde_json::json!([red.clone(), blue.clone()]), serde_json::json!([blue, red])] {
            let f = frame(&scene(objs, serde_json::json!({}), serde_json::json!([]), false), 0);
            assert_eq!(px(&f, 80, 46), [255, 0, 0, 255], "the near red box covers the far blue one");
        }
    }

    #[test]
    fn outline_darkens_the_silhouette_only() {
        let obj = |w: f64| serde_json::json!([{"shape": {"type": "box", "size": [2, 2, 2]}, "material": {"kind": "flat", "color": [1, 1, 1, 1], "outline": w, "outline_color": [0, 0, 0, 1]}}]);
        let plain = frame(&scene(obj(0.0), serde_json::json!({}), serde_json::json!([]), false), 0);
        let lined = frame(&scene(obj(3.0), serde_json::json!({}), serde_json::json!([]), false), 0);
        // the interior of the front face is untouched
        assert_eq!(px(&plain, 80, 50), px(&lined, 80, 50));
        // along the left silhouette some pixels turn dark
        let dark = (0..W as usize).filter(|&x| {
            let (a, b) = (px(&plain, x, 50), px(&lined, x, 50));
            b[3] > 0 && (b[0] as i32) < (a[0] as i32) - 100
        }).count();
        assert!(dark >= 2, "outline pixels on row 50: {dark}");
    }

    #[test]
    fn screen_shows_its_source_node() {
        let s = scene(
            serde_json::json!([{"shape": {"type": "screen", "size": [3.2, 1.8], "node": 2, "rect": [0, 0, 64, 36]}, "material": {"kind": "flat", "color": [0, 0, 0, 1]}}]),
            serde_json::json!({}),
            serde_json::json!([]),
            true,
        );
        let f = frame(&s, 0);
        let c = px(&f, 80, 50);
        assert!(c[0] > 200 && c[1] < 40 && c[2] < 40, "screen centre shows the red source: {c:?}");
        // the hidden source is not drawn in the 2D pass (its own box at the top-left stays empty)
        assert_eq!(px(&f, 3, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn static_scene_rests_and_a_camera_track_damages() {
        let objs = serde_json::json!([{"shape": {"type": "sphere", "radius": 1}}]);
        let s = scene(objs.clone(), serde_json::json!({}), serde_json::json!([]), false);
        let prep = Prepared::new(&s, Path::new("."), None, None).unwrap();
        let info = Info::new(&s, &prep);
        let sched = schedule::build(&s, &info).unwrap();
        assert!((1..30).all(|f| sched.damage[f].is_empty()), "a static 3D scene repeats");
        let tr = serde_json::json!([{"node": 1, "property": "s3d.cam.x", "delay": 0.2, "duration": 0.3, "fill": "both", "easing": "linear",
            "keyframes": [{"offset": 0, "value": {"number": 0}}, {"offset": 1, "value": {"number": 2}}]}]);
        let s2 = scene(objs, serde_json::json!({}), tr, false);
        let prep = Prepared::new(&s2, Path::new("."), None, None).unwrap();
        let info = Info::new(&s2, &prep);
        let sched = schedule::build(&s2, &info).unwrap();
        assert!(sched.damage[3].is_empty() && sched.damage[20].is_empty());
        assert!(!sched.damage[10].is_empty(), "the camera move damages");
        // and the frame differs from the rest frame
        assert_ne!(frame(&s2, 0), frame(&s2, 20));
    }

    #[test]
    fn camera_inside_the_near_plane_range_clips_instead_of_failing() {
        let s = scene(
            serde_json::json!([{"shape": {"type": "box", "size": [4, 4, 4]}, "pos": [0, 0, 4.5], "material": {"kind": "lambert", "color": [1, 1, 1, 1], "double_sided": true}}]),
            serde_json::json!({}),
            serde_json::json!([]),
            false,
        );
        let f = frame(&s, 0);
        assert!(f.chunks_exact(4).any(|p| p[3] > 0), "the box around the camera is still drawn (clipped)");
    }
}
