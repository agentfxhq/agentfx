//! CPU rasteriser for `Content::Scene3d` (stylised 3D): z-buffer, perspective-correct
//! interpolation, near-plane clipping, back-face culling, supersampling, per-pixel flat /
//! lambert / toon / phong shading, fog, screen-space outlines (silhouettes and creases from the
//! depth/normal buffer), feature-edge wire lines, planar ground shadows (soft), emissive bloom.
//!
//! Determinism: the supersampled buffer is split into horizontal bands processed in parallel;
//! every band walks its triangles in one global order and writes only its own rows; blurs are
//! per row / per column; no shared accumulators. So a frame is bit-identical across runs,
//! thread counts and segments.
use crate::s3d_mesh::Mesh;
use agentfx_ir::s3d::{self, Scene3d, Shape3d, State3d, View3, V3};
use rayon::prelude::*;
use std::sync::Arc;

/// A texture with a box-filtered mip chain; texels premultiplied RGBA8.
pub struct Tex {
    pub levels: Vec<(usize, usize, Vec<[u8; 4]>)>,
}

impl Tex {
    /// From premultiplied RGBA8 rows.
    pub fn new(w: usize, h: usize, px: Vec<[u8; 4]>) -> Tex {
        let mut levels = vec![(w, h, px)];
        loop {
            let (w0, h0, ref p0) = *levels.last().unwrap();
            if w0 <= 1 && h0 <= 1 {
                break;
            }
            let (w1, h1) = ((w0 / 2).max(1), (h0 / 2).max(1));
            let mut p1 = vec![[0u8; 4]; w1 * h1];
            for y in 0..h1 {
                for x in 0..w1 {
                    let mut s = [0u32; 4];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (sx, sy) = ((2 * x + dx).min(w0 - 1), (2 * y + dy).min(h0 - 1));
                        let q = p0[sy * w0 + sx];
                        for k in 0..4 {
                            s[k] += q[k] as u32;
                        }
                    }
                    p1[y * w1 + x] = [((s[0] + 2) / 4) as u8, ((s[1] + 2) / 4) as u8, ((s[2] + 2) / 4) as u8, ((s[3] + 2) / 4) as u8];
                }
            }
            levels.push((w1, h1, p1));
        }
        Tex { levels }
    }
    fn bilinear(&self, l: usize, u: f64, v: f64) -> [f32; 4] {
        let (w, h, ref p) = self.levels[l.min(self.levels.len() - 1)];
        let x = (u * w as f64 - 0.5).clamp(-0.5, w as f64 - 0.5);
        let y = (v * h as f64 - 0.5).clamp(-0.5, h as f64 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let ix = |i: f64| (i.max(0.0) as usize).min(w - 1);
        let iy = |i: f64| (i.max(0.0) as usize).min(h - 1);
        let (xa, xb, ya, yb) = (ix(x0), ix(x0 + 1.0), iy(y0), iy(y0 + 1.0));
        let q = |x: usize, y: usize| p[y * w + x];
        let (a, b, c, d) = (q(xa, ya), q(xb, ya), q(xa, yb), q(xb, yb));
        let mut o = [0f32; 4];
        for k in 0..4 {
            let top = a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx;
            let bot = c[k] as f32 * (1.0 - fx) + d[k] as f32 * fx;
            o[k] = (top * (1.0 - fy) + bot * fy) / 255.0;
        }
        o
    }
    /// Trilinear sample (premultiplied, 0..1) at uv with level of detail `lod` (log2 texels per px).
    pub fn sample(&self, u: f64, v: f64, lod: f64) -> [f32; 4] {
        let n = self.levels.len();
        let lod = lod.clamp(0.0, (n - 1) as f64);
        let l0 = lod.floor() as usize;
        let f = (lod - l0 as f64) as f32;
        let a = self.bilinear(l0, u, v);
        if f <= 1e-4 || l0 + 1 >= n {
            return a;
        }
        let b = self.bilinear(l0 + 1, u, v);
        [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, a[3] + (b[3] - a[3]) * f]
    }
    pub fn size(&self) -> (usize, usize) {
        (self.levels[0].0, self.levels[0].1)
    }
}

/// What one object contributes this frame (evaluated material).
#[derive(Clone)]
struct ObjF {
    kind: u8, // 0 flat, 1 lambert, 2 toon, 3 phong, 4 wire
    color: [f32; 4],
    alpha: f32,
    emissive: f32,
    bands: f32,
    spec: f32,
    shin: f32,
    outline: f32,
    outline_color: [f32; 4],
    crease_cos: f32,
    screen: bool,
}

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct VA {
    pv: [f32; 3],
    nv: [f32; 3],
    uv: [f32; 2],
}

struct Tri {
    /// supersampled px, fixed point (1/256)
    x: [i64; 3],
    y: [i64; 3],
    invw: [f32; 3],
    key: [f32; 3],
    a: [VA; 3],
    obj: u32,
    back: bool,
    /// screen-space gradients (per SS px) of invw, u*invw, v*invw: [dx, dy] each
    grad: [[f32; 2]; 3],
    y0: i32,
    y1: i32,
}

/// Lights in view space.
struct LightV {
    dir: Option<[f32; 3]>,
    pos: [f32; 3],
    col: [f32; 3],
    range: f32,
}

/// Everything a frame needs besides the scene's own data.
pub struct Frame<'a> {
    pub st: &'a State3d,
    /// per object texture (screens or image textures)
    pub tex: &'a [Option<Arc<Tex>>],
    /// NDC -> output px (affine [a, b, c, d, e, f]: x' = a x + c y + e, y' = b x + d y + f)
    pub ndc_to_px: [f64; 6],
    pub w: usize,
    pub h: usize,
    /// output px -> viewport-relative fraction (for non-axis-aligned layers): (affine, test)
    pub clip: Option<[f64; 6]>,
    /// output px per scene px (outline/wire widths, blur radii scale with the layer)
    pub px_scale: f64,
    pub aa: u32,
}

/// Per-painter buffers reused across frames (no page faults per frame).
#[derive(Default)]
pub struct Scratch {
    tri: Vec<u32>,
    zb: Vec<f32>,
    aux: Vec<u32>,
    col: Vec<[u8; 4]>,
    emis: Vec<[f32; 3]>,
    mask: Vec<f32>,
    own: Vec<u32>,
    seed: Vec<u32>,
    skey: Vec<f32>,
    hx: Vec<u32>,
}

/// `v[..n]` filled with `x` (grown when needed).
fn prep<T: Copy + Send + Sync>(v: &mut Vec<T>, n: usize, x: T) -> &mut [T] {
    if v.len() < n {
        v.resize(n, x);
    }
    v[..n].par_chunks_mut(1 << 16).for_each(|c| c.fill(x));
    &mut v[..n]
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    pub tris: usize,
    pub drawn: usize,
}

fn f3(v: V3) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}
fn d3(v: [f32; 3]) -> V3 {
    [v[0] as f64, v[1] as f64, v[2] as f64]
}
fn nrm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-20 { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, 0.0, 1.0] }
}
fn dotf(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Octahedral normal encoding into 2 x u16.
fn oct_enc(n: [f32; 3]) -> u32 {
    let s = n[0].abs() + n[1].abs() + n[2].abs();
    let (mut x, mut y) = (n[0] / s.max(1e-20), n[1] / s.max(1e-20));
    if n[2] < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - ox.abs()) * if oy >= 0.0 { 1.0 } else { -1.0 };
    }
    let q = |v: f32| ((v * 0.5 + 0.5) * 65535.0 + 0.5) as u16 as u32;
    q(x) | (q(y) << 16)
}
fn oct_dec(p: u32) -> [f32; 3] {
    let x = (p & 0xffff) as f32 / 65535.0 * 2.0 - 1.0;
    let y = (p >> 16) as f32 / 65535.0 * 2.0 - 1.0;
    let z = 1.0 - x.abs() - y.abs();
    let (mut x, mut y) = (x, y);
    if z < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - ox.abs()) * if oy >= 0.0 { 1.0 } else { -1.0 };
    }
    nrm3([x, y, z])
}

/// Clip a polygon (view space) against z <= -near. Attributes interpolate linearly.
fn clip_near(poly: &[VA], near: f32) -> Vec<VA> {
    let inside = |v: &VA| -v.pv[2] >= near;
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (a, b) = (&poly[i], &poly[(i + 1) % poly.len()]);
        let (ia, ib) = (inside(a), inside(b));
        if ia {
            out.push(*a);
        }
        if ia != ib {
            let da = -a.pv[2] - near;
            let db = -b.pv[2] - near;
            let t = da / (da - db);
            let l = |x: f32, y: f32| x + (y - x) * t;
            out.push(VA {
                pv: [l(a.pv[0], b.pv[0]), l(a.pv[1], b.pv[1]), -near],
                nv: [l(a.nv[0], b.nv[0]), l(a.nv[1], b.nv[1]), l(a.nv[2], b.nv[2])],
                uv: [l(a.uv[0], b.uv[0]), l(a.uv[1], b.uv[1])],
            });
        }
    }
    out
}

struct Proj {
    view: View3,
    m: [f64; 6],
    ss: f64,
}
impl Proj {
    /// view-space point -> (SS px x, y, invw, key)
    fn px(&self, pv: [f32; 3]) -> Option<(f64, f64, f64, f64)> {
        let (x, y, d) = self.view.ndc(d3(pv)).or_else(|| {
            // on the near plane after clipping (rounding): clamp
            let mut q = d3(pv);
            q[2] = q[2].min(-self.view.near);
            self.view.ndc(q)
        })?;
        let m = &self.m;
        let (sx, sy) = ((m[0] * x + m[2] * y + m[4]) * self.ss, (m[1] * x + m[3] * y + m[5]) * self.ss);
        let (invw, key) = if self.view.ortho { (1.0, self.view.far + 1.0 - d) } else { (1.0 / d, 1.0 / d) };
        Some((sx, sy, invw, key))
    }
}

/// Setup of one (clipped) triangle; None when degenerate or off-buffer.
fn setup(p: &Proj, v: [VA; 3], obj: u32, back: bool, w: usize, h: usize) -> Option<Tri> {
    let mut x = [0i64; 3];
    let mut y = [0i64; 3];
    let mut invw = [0f32; 3];
    let mut key = [0f32; 3];
    let mut fx = [0f64; 3];
    let mut fy = [0f64; 3];
    for k in 0..3 {
        let (sx, sy, iw, ky) = p.px(v[k].pv)?;
        if !sx.is_finite() || !sy.is_finite() || sx.abs() > 1e7 || sy.abs() > 1e7 {
            return None;
        }
        fx[k] = sx;
        fy[k] = sy;
        x[k] = (sx * 256.0).round() as i64;
        y[k] = (sy * 256.0).round() as i64;
        invw[k] = iw as f32;
        key[k] = ky as f32;
    }
    let area = (x[1] - x[0]) * (y[2] - y[0]) - (y[1] - y[0]) * (x[2] - x[0]);
    if area == 0 {
        return None;
    }
    let mut v = v;
    if area < 0 {
        x.swap(1, 2);
        y.swap(1, 2);
        invw.swap(1, 2);
        key.swap(1, 2);
        fx.swap(1, 2);
        fy.swap(1, 2);
        v.swap(1, 2);
    }
    let (minx, maxx) = (x.iter().min().unwrap() >> 8, (x.iter().max().unwrap() >> 8) + 1);
    let (miny, maxy) = (y.iter().min().unwrap() >> 8, (y.iter().max().unwrap() >> 8) + 1);
    if maxx < 0 || maxy < 0 || minx >= w as i64 || miny >= h as i64 {
        return None;
    }
    // gradients of invw, u*invw, v*invw over SS px (plane through the three vertices)
    let det = (fx[1] - fx[0]) * (fy[2] - fy[0]) - (fx[2] - fx[0]) * (fy[1] - fy[0]);
    let grad = |f: [f64; 3]| -> [f32; 2] {
        if det.abs() < 1e-12 {
            return [0.0, 0.0];
        }
        let gx = ((f[1] - f[0]) * (fy[2] - fy[0]) - (f[2] - f[0]) * (fy[1] - fy[0])) / det;
        let gy = ((f[2] - f[0]) * (fx[1] - fx[0]) - (f[1] - f[0]) * (fx[2] - fx[0])) / det;
        [gx as f32, gy as f32]
    };
    let iw = [invw[0] as f64, invw[1] as f64, invw[2] as f64];
    let g0 = grad(iw);
    let g1 = grad([v[0].uv[0] as f64 * iw[0], v[1].uv[0] as f64 * iw[1], v[2].uv[0] as f64 * iw[2]]);
    let g2 = grad([v[0].uv[1] as f64 * iw[0], v[1].uv[1] as f64 * iw[1], v[2].uv[1] as f64 * iw[2]]);
    Some(Tri { x, y, invw, key, a: v, obj, back, grad: [g0, g1, g2], y0: miny.max(0) as i32, y1: maxy.min(h as i64) as i32 })
}

/// Rasterize triangle `t` into rows [ya, yb) of a band: calls `f(x, y, b0, b1, b2)` per covered
/// sample with screen-space barycentrics.
#[inline]
fn raster(t: &Tri, w: usize, ya: i32, yb: i32, mut f: impl FnMut(usize, usize, f64, f64, f64)) {
    let (x, y) = (&t.x, &t.y);
    let y0 = t.y0.max(ya);
    let y1 = t.y1.min(yb);
    if y0 >= y1 {
        return;
    }
    let x0 = ((x.iter().min().unwrap() >> 8).max(0)) as i64;
    let x1 = ((x.iter().max().unwrap() >> 8) + 1).min(w as i64);
    if x0 >= x1 {
        return;
    }
    // edge k: from vertex (k+1) to (k+2), weight of vertex k
    let mut ea = [0i64; 3];
    let mut eb = [0i64; 3];
    let mut ec = [0i64; 3];
    for k in 0..3 {
        let (i, j) = ((k + 1) % 3, (k + 2) % 3);
        let (dx, dy) = (x[j] - x[i], y[j] - y[i]);
        // E(p) = dx * (py - yi) - dy * (px - xi) = -dy*px + dx*py + (dy*xi - dx*yi)
        ea[k] = -dy;
        eb[k] = dx;
        ec[k] = dy * x[i] - dx * y[i];
        // tie rule: include E == 0 only for "top-left" edges (complementary for the shared
        // edge of the neighbour, which runs the other way)
        let incl = dy > 0 || (dy == 0 && dx < 0);
        if !incl {
            ec[k] -= 1;
        }
    }
    let area = ((x[1] - x[0]) * (y[2] - y[0]) - (y[1] - y[0]) * (x[2] - x[0])) as f64;
    let inv = 1.0 / area;
    for py in y0..y1 {
        let cy = py as i64 * 256 + 128;
        let cx0 = x0 * 256 + 128;
        let mut e = [ea[0] * cx0 + eb[0] * cy + ec[0], ea[1] * cx0 + eb[1] * cy + ec[1], ea[2] * cx0 + eb[2] * cy + ec[2]];
        for px in x0..x1 {
            if e[0] >= 0 && e[1] >= 0 && e[2] >= 0 {
                // weights from the biased edge values (the tie bias is 1/65536 px^2)
                let (b0, b1) = (e[0] as f64, e[1] as f64);
                let b2 = area - b0 - b1;
                f(px as usize, py as usize, b0 * inv, b1 * inv, b2 * inv);
            }
            e[0] += ea[0] * 256;
            e[1] += ea[1] * 256;
            e[2] += ea[2] * 256;
        }
    }
}

/// Render the scene into `w` x `h` premultiplied RGBA8 (output px).
pub fn render(sc: &Scene3d, meshes: &[Option<Arc<Mesh>>], fr: &Frame, stats: &mut Stats, scr: &mut Scratch) -> Vec<u8> {
    let t0 = std::time::Instant::now();
    let mut tm: Vec<(&str, f64)> = Vec::new();
    let ss = fr.aa.clamp(1, 4) as usize;
    let (w, h) = (fr.w * ss, fr.h * ss);
    let st = fr.st;
    let view = View3::new(&st.cam, sc.aspect());
    let proj = Proj { view, m: fr.ndc_to_px, ss: ss as f64 };
    let world = st.world(sc);
    let opac = st.opacities(sc);
    // ---- materials
    let objs: Vec<ObjF> = sc
        .objects
        .iter()
        .zip(&st.objs)
        .zip(&opac)
        .map(|((o, os), &op)| {
            let m = &o.material;
            let kind = match m.kind.as_str() {
                "flat" => 0,
                "lambert" => 1,
                "phong" => 3,
                "wire" => 4,
                _ => 2,
            };
            let c = os.color;
            ObjF {
                kind,
                color: [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32],
                alpha: (c[3] * op).clamp(0.0, 1.0) as f32,
                emissive: os.emissive.max(0.0) as f32,
                bands: m.bands.clamp(2, 16) as f32,
                spec: m.specular as f32,
                shin: m.shininess.max(1.0) as f32,
                outline: (m.outline * fr.px_scale * ss as f64) as f32,
                outline_color: [m.outline_color[0] as f32, m.outline_color[1] as f32, m.outline_color[2] as f32, m.outline_color[3] as f32],
                crease_cos: if m.crease > 0.0 { m.crease.to_radians().cos() as f32 } else { -2.0 },
                screen: matches!(o.shape, Shape3d::Screen { .. }),
            }
        })
        .collect();
    // ---- lights (view space)
    let lights: Vec<LightV> = st
        .lights
        .iter()
        .map(|l| {
            let col = [(l.color[0] * l.intensity) as f32, (l.color[1] * l.intensity) as f32, (l.color[2] * l.intensity) as f32];
            if l.kind == "point" {
                LightV { dir: None, pos: f3(view.view.point(l.pos)), col, range: l.range as f32 }
            } else {
                LightV { dir: Some(nrm3(f3(view.view.vector(l.pos)))), pos: [0.0; 3], col, range: 0.0 }
            }
        })
        .collect();
    let up = nrm3(f3(view.view.vector([0.0, 1.0, 0.0])));
    let amb = [(st.ambient * sc.ambient_color[0]) as f32, (st.ambient * sc.ambient_color[1]) as f32, (st.ambient * sc.ambient_color[2]) as f32];
    let fog = st.fog.as_ref().map(|f| (f.mode == "exp", f.near as f32, f.far as f32, f.density as f32, [f.color[0] as f32, f.color[1] as f32, f.color[2] as f32]));
    // ---- triangles
    let mut opaque: Vec<Tri> = Vec::new();
    let mut transp: Vec<(f32, Tri)> = Vec::new();
    // view-space line segments for wire: (a, b, key a, key b, color, width SS px)
    let mut lines: Vec<([f64; 3], [f64; 3], [f32; 4], f32)> = Vec::new();
    let vmat = view.view;
    for (oi, o) in sc.objects.iter().enumerate() {
        let of = &objs[oi];
        let Some(mesh) = meshes.get(oi).and_then(|m| m.as_ref()) else { continue };
        if of.alpha <= 0.0 {
            continue;
        }
        let mv = vmat.mul(&world[oi]);
        let nm = mv.normal_matrix();
        let mirrored = world[oi].det3() < 0.0;
        let wire_w = if of.kind == 4 && o.material.wire <= 0.0 { 1.5 } else { o.material.wire };
        if wire_w > 0.0 {
            let c = o.material.wire_color;
            let col = [c[0] as f32, c[1] as f32, c[2] as f32, (c[3] * of.alpha as f64) as f32];
            for (a, b) in &mesh.edges {
                lines.push((mv.point(d3(*a)), mv.point(d3(*b)), col, (wire_w * fr.px_scale * ss as f64) as f32));
            }
        }
        if of.kind == 4 {
            continue;
        }
        let two_sided = !mesh.closed || o.material.double_sided || matches!(o.shape, Shape3d::Plane { .. } | Shape3d::Screen { .. });
        let va: Vec<VA> = mesh
            .pos
            .iter()
            .zip(&mesh.nrm)
            .zip(&mesh.uv)
            .map(|((p, n), uv)| VA { pv: f3(mv.point(d3(*p))), nv: nrm3(f3(s3d::m3(&nm, d3(*n)))), uv: *uv })
            .collect();
        let transparent = of.alpha < 0.999;
        for t in &mesh.tris {
            stats.tris += 1;
            let v = [va[t[0] as usize], va[t[1] as usize], va[t[2] as usize]];
            // facing: geometric normal vs the view ray
            let e1 = [v[1].pv[0] - v[0].pv[0], v[1].pv[1] - v[0].pv[1], v[1].pv[2] - v[0].pv[2]];
            let e2 = [v[2].pv[0] - v[0].pv[0], v[2].pv[1] - v[0].pv[1], v[2].pv[2] - v[0].pv[2]];
            let mut gn = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            if mirrored {
                gn = [-gn[0], -gn[1], -gn[2]];
            }
            let ray = if view.ortho { [0.0, 0.0, -1.0] } else { v[0].pv };
            let back = dotf(gn, ray) > 0.0;
            if back && !two_sided {
                continue;
            }
            // all behind the near plane, or all beyond far
            let ds = [-v[0].pv[2], -v[1].pv[2], -v[2].pv[2]];
            if ds.iter().all(|&d| d < view.near as f32) || ds.iter().all(|&d| d > view.far as f32) {
                continue;
            }
            let poly = if ds.iter().all(|&d| d >= view.near as f32) { v.to_vec() } else { clip_near(&v, view.near as f32) };
            for k in 1..poly.len().saturating_sub(1) {
                if let Some(tr) = setup(&proj, [poly[0], poly[k], poly[k + 1]], oi as u32, back, w, h) {
                    if transparent {
                        let dz = -(poly[0].pv[2] + poly[k].pv[2] + poly[k + 1].pv[2]) / 3.0;
                        transp.push((dz, tr));
                    } else {
                        opaque.push(tr);
                    }
                }
            }
        }
    }
    stats.drawn = opaque.len() + transp.len();
    tm.push(("setup", t0.elapsed().as_secs_f64()));
    // ---- buffers (supersampled)
    let n = w * h;
    let bh = 16 * ss;
    let nb = h.div_ceil(bh);
    let bin = |tris: &[Tri]| -> Vec<Vec<u32>> {
        let mut b: Vec<Vec<u32>> = vec![Vec::new(); nb];
        for (i, t) in tris.iter().enumerate() {
            if t.y1 <= t.y0 {
                continue;
            }
            for (k, bv) in b.iter_mut().enumerate().take(((t.y1 - 1) as usize / bh + 1).min(nb)).skip(t.y0 as usize / bh) {
                let _ = k;
                bv.push(i as u32);
            }
        }
        b
    };
    let bins = bin(&opaque);
    let Scratch { tri: s_tri, zb: s_zb, aux: s_aux, col: s_col, emis: s_emis, mask: s_mask, own: s_own, seed: s_seed, skey: s_skey, hx: s_hx } = scr;
    let tri_buf = prep(s_tri, n, NONE);
    let zb = prep(s_zb, n, 0f32);
    // barycentrics, then packed normals (only read where a triangle is)
    if s_aux.len() < n {
        s_aux.resize(n, 0);
    }
    let aux = &mut s_aux[..n];
    let col = prep(s_col, n, [0u8; 4]);
    let (ow, oh) = (fr.w, fr.h);
    let emis = prep(s_emis, ow * oh, [0f32; 3]);
    let inv_ss2 = 1.0 / (ss * ss) as f32;
    let want_nrm = objs.iter().any(|o| o.outline > 0.0);
    let sh = Shader { objs: &objs, lights: &lights, up, amb, fog, tex: fr.tex };
    {
        let opaque = &opaque;
        tri_buf
            .par_chunks_mut(bh * w)
            .zip(zb.par_chunks_mut(bh * w))
            .zip(aux.par_chunks_mut(bh * w))
            .zip(col.par_chunks_mut(bh * w))
            .zip(emis.par_chunks_mut(16 * ow))
            .enumerate()
            .for_each(|(bi, ((((tb, zz), ax), cc), em))| {
                let ya = (bi * bh) as i32;
                let yb = ((bi + 1) * bh).min(h) as i32;
                for &ti in &bins[bi] {
                    let t = &opaque[ti as usize];
                    raster(t, w, ya, yb, |x, y, b0, b1, b2| {
                        let key = (b0 * t.key[0] as f64 + b1 * t.key[1] as f64 + b2 * t.key[2] as f64) as f32;
                        let i = (y - ya as usize) * w + x;
                        if key > zz[i] {
                            zz[i] = key;
                            tb[i] = ti;
                            let q = [b0 * t.invw[0] as f64, b1 * t.invw[1] as f64, b2 * t.invw[2] as f64];
                            let s = q[0] + q[1] + q[2];
                            let p1 = ((q[1] / s).clamp(0.0, 1.0) * 65535.0).round() as u32;
                            let p2 = ((q[2] / s).clamp(0.0, 1.0) * 65535.0).round() as u32;
                            ax[i] = p1 | (p2 << 16);
                        }
                    });
                }
                // shade once per (output pixel, triangle) like MSAA: the samples of a pixel
                // that one triangle covers share the colour shaded at the first of them
                let noshade = agentfx_ir::env::var("AGENTFX_S3D_NOSHADE").is_ok();
                let rows = (yb - ya) as usize;
                let mut seen: Vec<(u32, [u8; 4], [f32; 3], u32)> = Vec::with_capacity(16);
                for oy in 0..rows / ss {
                    for ox in 0..ow {
                        seen.clear();
                        for sy in 0..ss {
                            for sx in 0..ss {
                                let (yy, x) = (oy * ss + sy, ox * ss + sx);
                                let i = yy * w + x;
                                let ti = tb[i];
                                if ti == NONE || noshade {
                                    continue;
                                }
                                let (c8, e, n8) = match seen.iter().find(|s| s.0 == ti) {
                                    Some(s) => (s.1, s.2, s.3),
                                    None => {
                                        let t = &opaque[ti as usize];
                                        let p1 = (ax[i] & 0xffff) as f32 / 65535.0;
                                        let p2 = (ax[i] >> 16) as f32 / 65535.0;
                                        let (c, e, nrm) = sh.shade(t, 1.0 - p1 - p2, p1, p2, x as f64 + 0.5, (yy + ya as usize) as f64 + 0.5);
                                        let v = (ti, to8(c), e, if want_nrm { oct_enc(nrm) } else { 0 });
                                        seen.push(v);
                                        (v.1, v.2, v.3)
                                    }
                                };
                                cc[i] = c8;
                                if want_nrm {
                                    ax[i] = n8;
                                }
                                if e[0] + e[1] + e[2] > 0.0 {
                                    let k = oy * ow + ox;
                                    em[k][0] += e[0] * inv_ss2;
                                    em[k][1] += e[1] * inv_ss2;
                                    em[k][2] += e[2] * inv_ss2;
                                }
                            }
                        }
                    }
                }
            });
    }
    tm.push(("opaque", t0.elapsed().as_secs_f64()));
    // ---- ground shadows
    if let (Some(shc), Some(light)) = (&sc.shadow, st.lights.first()) {
        let receivers: Vec<usize> = sc.objects.iter().enumerate().filter(|(i, o)| o.receive && matches!(o.shape, Shape3d::Plane { .. }) && objs[*i].alpha > 0.0).map(|(i, _)| i).collect();
        if !receivers.is_empty() && shc.opacity > 0.0 {
            let mut stris: Vec<(Tri, u32)> = Vec::new();
            for &r in &receivers {
                let p0 = world[r].point([0.0, 0.0, 0.0]);
                let nn = s3d::norm(s3d::m3(&world[r].normal_matrix(), [0.0, 0.0, 1.0]));
                let n = if s3d::dot(nn, s3d::sub(light.pos, p0)) < 0.0 && light.kind == "point" { s3d::mul(nn, -1.0) } else { nn };
                for (oi, o) in sc.objects.iter().enumerate() {
                    if oi == r || !o.cast || objs[oi].alpha <= 0.0 || objs[oi].kind == 4 || receivers.contains(&oi) {
                        continue;
                    }
                    let Some(mesh) = meshes.get(oi).and_then(|m| m.as_ref()) else { continue };
                    let wm = world[oi];
                    let proj_pt = |p: V3| -> Option<V3> {
                        let h0 = s3d::dot(s3d::sub(p, p0), n);
                        if light.kind == "point" {
                            let lh = s3d::dot(s3d::sub(light.pos, p0), n);
                            if lh <= 1e-9 {
                                return None;
                            }
                            let hh = h0.clamp(0.0, lh * 0.999);
                            let t = lh / (lh - hh);
                            Some(s3d::add(light.pos, s3d::mul(s3d::sub(s3d::sub(p, s3d::mul(n, h0 - hh)), light.pos), t)))
                        } else {
                            let d = s3d::norm(s3d::mul(light.pos, -1.0));
                            let dn = s3d::dot(d, n);
                            if dn >= -1e-6 {
                                return None;
                            }
                            let hh = h0.max(0.0);
                            let q = s3d::sub(p, s3d::mul(n, h0 - hh));
                            Some(s3d::add(q, s3d::mul(d, -hh / dn)))
                        }
                    };
                    let vp: Vec<Option<VA>> = mesh
                        .pos
                        .iter()
                        .map(|p| proj_pt(wm.point(d3(*p))).map(|q| VA { pv: f3(vmat.point(s3d::add(q, s3d::mul(n, 1e-4)))), nv: [0.0, 0.0, 1.0], uv: [0.0, 0.0] }))
                        .collect();
                    for t in &mesh.tris {
                        let (Some(a), Some(b), Some(c)) = (vp[t[0] as usize], vp[t[1] as usize], vp[t[2] as usize]) else { continue };
                        let v = [a, b, c];
                        let poly = if v.iter().all(|x| -x.pv[2] >= view.near as f32) { v.to_vec() } else { clip_near(&v, view.near as f32) };
                        for k in 1..poly.len().saturating_sub(1) {
                            if let Some(tr) = setup(&proj, [poly[0], poly[k], poly[k + 1]], r as u32, false, w, h) {
                                stris.push((tr, r as u32));
                            }
                        }
                    }
                }
            }
            if !stris.is_empty() {
                let tris: Vec<Tri> = stris.iter().map(|x| clone_tri(&x.0)).collect();
                let sbins = bin(&tris);
                let mask = prep(s_mask, n, 0f32);
                {
                    let (tri_buf, opaque) = (&tri_buf, &opaque);
                    mask.par_chunks_mut(bh * w).enumerate().for_each(|(bi, mk)| {
                        let ya = (bi * bh) as i32;
                        let yb = ((bi + 1) * bh).min(h) as i32;
                        for &ti in &sbins[bi] {
                            let (t, r) = (&tris[ti as usize], stris[ti as usize].1);
                            raster(t, w, ya, yb, |x, y, _, _, _| {
                                let g = y * w + x;
                                let o = tri_buf[g];
                                if o != NONE && opaque[o as usize].obj == r {
                                    mk[(y - ya as usize) * w + x] = 1.0;
                                }
                            });
                        }
                    });
                }
                // blur at output resolution (the mask is soft anyway)
                let mut mo = vec![0f32; ow * oh];
                mo.par_chunks_mut(ow).enumerate().for_each(|(oy, row)| {
                    for (ox, v) in row.iter_mut().enumerate() {
                        let mut acc = 0f32;
                        for sy in 0..ss {
                            let b = (oy * ss + sy) * w + ox * ss;
                            for sx in 0..ss {
                                acc += mask[b + sx];
                            }
                        }
                        *v = acc * inv_ss2;
                    }
                });
                blur_f32(&mut mo, ow, oh, 1, shc.blur * fr.px_scale);
                let sc_col = [shc.color[0] as f32, shc.color[1] as f32, shc.color[2] as f32];
                let op = (shc.opacity * shc.color[3]) as f32;
                let (tri_buf, opaque) = (&tri_buf, &opaque);
                col.par_chunks_mut(w).enumerate().for_each(|(y, cr)| {
                    let mr = &mo[(y / ss) * ow..(y / ss + 1) * ow];
                    for x in 0..w {
                        let m = mr[x / ss];
                        if m <= 0.0 {
                            continue;
                        }
                        let o = tri_buf[y * w + x];
                        if o == NONE || !receivers.contains(&(opaque[o as usize].obj as usize)) {
                            continue;
                        }
                        let k = op * m.min(1.0);
                        let c = &mut cr[x];
                        let a = c[3] as f32;
                        for ch in 0..3 {
                            c[ch] = (c[ch] as f32 * (1.0 - k) + sc_col[ch] * a * k).round().clamp(0.0, 255.0) as u8;
                        }
                    }
                });
            }
        }
    }
    tm.push(("shadow", t0.elapsed().as_secs_f64()));
    // ---- outlines
    if objs.iter().any(|o| o.outline > 0.0) {
        // outlines fade into the fog like the surfaces they belong to
        let (ortho, cfar) = (view.ortho, view.far as f32);
        let fogf = move |key: f32| -> f32 {
            let Some((exp, near, far, dens, _)) = fog else { return 0.0 };
            let d = if ortho { cfar + 1.0 - key } else { 1.0 / key.max(1e-20) };
            if exp { 1.0 - (-dens * d).exp() } else { ((d - near) / (far - near).max(1e-6)).clamp(0.0, 1.0) }
        };
        outline(col, tri_buf, zb, aux, &opaque, &objs, w, h, (s_own, s_seed, s_skey, s_hx), &fogf);
    }
    tm.push(("outline", t0.elapsed().as_secs_f64()));
    // ---- transparent surfaces, back to front
    if !transp.is_empty() {
        transp.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let tris: Vec<Tri> = transp.into_iter().map(|x| x.1).collect();
        let tb = bin(&tris);
        let zb = &zb;
        col.par_chunks_mut(bh * w).zip(emis.par_chunks_mut(16 * ow)).enumerate().for_each(|(bi, (cc, em))| {
            let ya = (bi * bh) as i32;
            let yb = ((bi + 1) * bh).min(h) as i32;
            for &ti in &tb[bi] {
                let t = &tris[ti as usize];
                raster(t, w, ya, yb, |x, y, b0, b1, b2| {
                    let key = (b0 * t.key[0] as f64 + b1 * t.key[1] as f64 + b2 * t.key[2] as f64) as f32;
                    let g = y * w + x;
                    if key <= zb[g] {
                        return;
                    }
                    let q = [b0 * t.invw[0] as f64, b1 * t.invw[1] as f64, b2 * t.invw[2] as f64];
                    let s = q[0] + q[1] + q[2];
                    let (c, e, _) = sh.shade(t, (q[0] / s) as f32, (q[1] / s) as f32, (q[2] / s) as f32, x as f64 + 0.5, y as f64 + 0.5);
                    let i = (y - ya as usize) * w + x;
                    let d = &mut cc[i];
                    let a = c[3];
                    for ch in 0..4 {
                        d[ch] = ((c[ch] + d[ch] as f32 / 255.0 * (1.0 - a)) * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                    let (oy, ox) = ((y - ya as usize) / ss, x / ss);
                    if e[0] + e[1] + e[2] > 0.0 && ox < ow && oy * ow < em.len() {
                        let k = oy * ow + ox;
                        for ch in 0..3 {
                            em[k][ch] += e[ch] * inv_ss2;
                        }
                    }
                });
            }
        });
    }
    tm.push(("transp", t0.elapsed().as_secs_f64()));
    // ---- wire lines
    if !lines.is_empty() {
        let near = view.near;
        let mut segs: Vec<([f64; 4], [f64; 4], [f32; 4], f32)> = Vec::new();
        for (a, b, c, lw) in &lines {
            let (mut a, mut b) = (*a, *b);
            let (da, db) = (-a[2], -b[2]);
            if da < near && db < near {
                continue;
            }
            if da < near || db < near {
                let t = (da - near) / (da - db);
                let p = s3d::add(a, s3d::mul(s3d::sub(b, a), t));
                if da < near {
                    a = p;
                } else {
                    b = p;
                }
            }
            let (Some(pa), Some(pb)) = (proj.px(f3(a)), proj.px(f3(b))) else { continue };
            segs.push(([pa.0, pa.1, pa.3, 0.0], [pb.0, pb.1, pb.3, 0.0], *c, *lw));
        }
        let zb = &zb;
        col.par_chunks_mut(bh * w).enumerate().for_each(|(bi, cc)| {
            let ya = bi * bh;
            let yb = ((bi + 1) * bh).min(h);
            for (a, b, c, lw) in &segs {
                let hw = (*lw as f64 / 2.0).max(0.5);
                let (y0, y1) = ((a[1].min(b[1]) - hw - 1.0).floor().max(ya as f64) as usize, ((a[1].max(b[1]) + hw + 1.0).ceil().max(0.0) as usize).min(yb));
                let (x0, x1) = ((a[0].min(b[0]) - hw - 1.0).floor().max(0.0) as usize, ((a[0].max(b[0]) + hw + 1.0).ceil().max(0.0) as usize).min(w));
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = (dx * dx + dy * dy).max(1e-12);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                        let t = (((px - a[0]) * dx + (py - a[1]) * dy) / l2).clamp(0.0, 1.0);
                        let (qx, qy) = (a[0] + dx * t, a[1] + dy * t);
                        let d = (px - qx).hypot(py - qy);
                        let cov = (hw + 0.5 - d).clamp(0.0, 1.0) as f32;
                        if cov <= 0.0 {
                            continue;
                        }
                        let key = a[2] + (b[2] - a[2]) * t;
                        let z = zb[y * w + x];
                        if z > 0.0 && key * 1.003 < z as f64 {
                            continue;
                        }
                        let k = cov * c[3];
                        let dpx = &mut cc[(y - ya) * w + x];
                        for ch in 0..3 {
                            dpx[ch] = ((c[ch] * k + dpx[ch] as f32 / 255.0 * (1.0 - k)) * 255.0).round().clamp(0.0, 255.0) as u8;
                        }
                        dpx[3] = ((k + dpx[3] as f32 / 255.0 * (1.0 - k)) * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        });
    }
    tm.push(("wire", t0.elapsed().as_secs_f64()));
    // ---- downsample, bloom, background, clip
    let mut out = vec![0u8; ow * oh * 4];
    out.par_chunks_mut(ow * 4).enumerate().for_each(|(oy, row)| {
        for ox in 0..ow {
            let mut s = [0u32; 4];
            for sy in 0..ss {
                let base = (oy * ss + sy) * w + ox * ss;
                for sx in 0..ss {
                    let c = col[base + sx];
                    for k in 0..4 {
                        s[k] += c[k] as u32;
                    }
                }
            }
            let d = (ss * ss) as u32;
            for k in 0..4 {
                row[ox * 4 + k] = ((s[k] + d / 2) / d) as u8;
            }
        }
    });
    tm.push(("down", t0.elapsed().as_secs_f64()));
    if let Some(b) = &sc.bloom {
        if b.strength > 0.0 && emis.iter().any(|e| e[0] + e[1] + e[2] > 0.0) {
            // blurred at half resolution (it is wide and soft), read back bilinearly
            let (hw2, hh2) = (ow.div_ceil(2), oh.div_ceil(2));
            let mut half = vec![0f32; hw2 * hh2 * 3];
            half.par_chunks_mut(hw2 * 3).enumerate().for_each(|(y, row)| {
                for x in 0..hw2 {
                    for ch in 0..3 {
                        let mut acc = 0f32;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let (sx, sy) = ((2 * x + dx).min(ow - 1), (2 * y + dy).min(oh - 1));
                            acc += emis[sy * ow + sx][ch];
                        }
                        row[3 * x + ch] = acc * 0.25;
                    }
                }
            });
            blur_f32(&mut half, hw2, hh2, 3, b.radius * fr.px_scale / 2.0);
            let k = b.strength as f32;
            let half = &half;
            out.par_chunks_mut(ow * 4).enumerate().for_each(|(y, row)| {
                let fy = ((y as f32 + 0.5) / 2.0 - 0.5).max(0.0);
                let (y0, ty) = (fy.floor() as usize, fy.fract());
                let y1 = (y0 + 1).min(hh2 - 1);
                let y0 = y0.min(hh2 - 1);
                for x in 0..ow {
                    let fx = ((x as f32 + 0.5) / 2.0 - 0.5).max(0.0);
                    let (x0, tx) = (fx.floor() as usize, fx.fract());
                    let x1 = (x0 + 1).min(hw2 - 1);
                    let x0 = x0.min(hw2 - 1);
                    let at = |xx: usize, yy: usize, c: usize| half[(yy * hw2 + xx) * 3 + c];
                    let mut g = [0f32; 3];
                    for (c, gv) in g.iter_mut().enumerate() {
                        let top = at(x0, y0, c) * (1.0 - tx) + at(x1, y0, c) * tx;
                        let bot = at(x0, y1, c) * (1.0 - tx) + at(x1, y1, c) * tx;
                        *gv = (top * (1.0 - ty) + bot * ty) * k;
                    }
                    let gm = g[0].max(g[1]).max(g[2]);
                    if gm <= 0.0 {
                        continue;
                    }
                    let p = &mut row[4 * x..4 * x + 4];
                    for ch in 0..3 {
                        p[ch] = (p[ch] as f32 + g[ch] * 255.0).round().min(255.0) as u8;
                    }
                    let a = (p[3] as f32 / 255.0).max(gm.min(1.0));
                    p[3] = (a * 255.0).round() as u8;
                    for ch in 0..3 {
                        p[ch] = p[ch].min(p[3]);
                    }
                }
            });
        }
    }
    if let Some(bg) = sc.bg {
        let bgp = [bg[0] * bg[3], bg[1] * bg[3], bg[2] * bg[3], bg[3]];
        out.par_chunks_mut(4).for_each(|p| {
            let a = p[3] as f64 / 255.0;
            for k in 0..4 {
                p[k] = (p[k] as f64 + bgp[k] * 255.0 * (1.0 - a)).round().min(255.0) as u8;
            }
        });
    }
    tm.push(("final", t0.elapsed().as_secs_f64()));
    if agentfx_ir::env::var("AGENTFX_S3D_DEBUG").is_ok() {
        eprintln!("s3d {}x{} aa{} tris {} drawn {}: {}", fr.w, fr.h, ss, stats.tris, stats.drawn, tm.iter().map(|(k, v)| format!("{k} {:.1}", v * 1e3)).collect::<Vec<_>>().join(" "));
    }
    if let Some(m) = fr.clip {
        out.par_chunks_mut(ow * 4).enumerate().for_each(|(y, row)| {
            for x in 0..ow {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let u = m[0] * px + m[2] * py + m[4];
                let v = m[1] * px + m[3] * py + m[5];
                if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                    row[4 * x..4 * x + 4].fill(0);
                }
            }
        });
    }
    out
}

fn clone_tri(t: &Tri) -> Tri {
    Tri { x: t.x, y: t.y, invw: t.invw, key: t.key, a: t.a, obj: t.obj, back: t.back, grad: t.grad, y0: t.y0, y1: t.y1 }
}

#[inline(always)]
fn to8(c: [f32; 4]) -> [u8; 4] {
    // `as u8` saturates (NaN -> 0); +0.5 rounds the non-negative values
    [(c[0] * 255.0 + 0.5) as u8, (c[1] * 255.0 + 0.5) as u8, (c[2] * 255.0 + 0.5) as u8, (c[3] * 255.0 + 0.5) as u8]
}

struct Shader<'a> {
    objs: &'a [ObjF],
    lights: &'a [LightV],
    up: [f32; 3],
    amb: [f32; 3],
    fog: Option<(bool, f32, f32, f32, [f32; 3])>,
    tex: &'a [Option<Arc<Tex>>],
}

impl Shader<'_> {
    /// Premultiplied color, emissive light (premultiplied, for bloom) and the view normal at
    /// perspective-correct barycentrics (p0, p1, p2) of triangle `t`, SS pixel center (sx, sy).
    fn shade(&self, t: &Tri, p0: f32, p1: f32, p2: f32, sx: f64, sy: f64) -> ([f32; 4], [f32; 3], [f32; 3]) {
        let o = &self.objs[t.obj as usize];
        let a = &t.a;
        #[inline(always)]
        fn l3(x: [f32; 3], y: [f32; 3], z: [f32; 3], p0: f32, p1: f32, p2: f32) -> [f32; 3] {
            [x[0] * p0 + y[0] * p1 + z[0] * p2, x[1] * p0 + y[1] * p1 + z[1] * p2, x[2] * p0 + y[2] * p1 + z[2] * p2]
        }
        let pv = l3(a[0].pv, a[1].pv, a[2].pv, p0, p1, p2);
        let mut n = nrm3(l3(a[0].nv, a[1].nv, a[2].nv, p0, p1, p2));
        if t.back {
            n = [-n[0], -n[1], -n[2]];
        }
        let u = a[0].uv[0] * p0 + a[1].uv[0] * p1 + a[2].uv[0] * p2;
        let v = a[0].uv[1] * p0 + a[1].uv[1] * p1 + a[2].uv[1] * p2;
        let mut base = [o.color[0], o.color[1], o.color[2]];
        let mut alpha = o.alpha;
        let tex = self.tex.get(t.obj as usize).and_then(|x| x.as_ref());
        if let (Some(tx), false) = (tex, o.screen && t.back) {
            // level of detail from the screen-space derivatives of the perspective-correct uv
            let (tw, th) = tx.size();
            let g = &t.grad;
            let inv = self.invw_at(t, sx, sy);
            let (u64_, v64) = (u as f64, v as f64);
            let dudx = (g[1][0] as f64 - u64_ * g[0][0] as f64) / inv;
            let dudy = (g[1][1] as f64 - u64_ * g[0][1] as f64) / inv;
            let dvdx = (g[2][0] as f64 - v64 * g[0][0] as f64) / inv;
            let dvdy = (g[2][1] as f64 - v64 * g[0][1] as f64) / inv;
            let rx = (dudx * tw as f64).hypot(dvdx * th as f64);
            let ry = (dudy * tw as f64).hypot(dvdy * th as f64);
            let lod = rx.max(ry).max(1e-9).log2();
            let s = tx.sample(u64_, v64, lod);
            if o.screen {
                // the UI over the screen's own colour
                for k in 0..3 {
                    base[k] = s[k] + base[k] * (1.0 - s[3]);
                }
            } else {
                for k in 0..3 {
                    base[k] = if s[3] > 0.0 { base[k] * s[k] / s[3] } else { base[k] };
                }
                alpha *= s[3];
            }
        }
        let mut c = match o.kind {
            0 => base,
            _ => {
                let hemi = 0.75 + 0.25 * dotf(n, self.up);
                let mut l = [self.amb[0] * hemi, self.amb[1] * hemi, self.amb[2] * hemi];
                let mut spec = [0f32; 3];
                let want_spec = o.spec > 0.0 && (o.kind == 3 || o.kind == 2);
                let vdir = if want_spec { nrm3([-pv[0], -pv[1], -pv[2]]) } else { [0.0; 3] };
                for li in self.lights {
                    let (ld, att) = match li.dir {
                        Some(d) => (d, 1.0),
                        None => {
                            let dv = [li.pos[0] - pv[0], li.pos[1] - pv[1], li.pos[2] - pv[2]];
                            let dist = dotf(dv, dv).sqrt();
                            let att = if li.range > 0.0 { 1.0 / (1.0 + (dist / li.range) * (dist / li.range)) } else { 1.0 };
                            (nrm3(dv), att)
                        }
                    };
                    let mut d = dotf(n, ld).max(0.0);
                    if o.kind == 2 {
                        // toon: the lit side in `bands` steps (darkest lit step 1/bands), the
                        // side facing away gets ambient only
                        d = if d <= 1e-4 { 0.0 } else { ((d * o.bands).ceil() / o.bands).min(1.0) };
                    }
                    for k in 0..3 {
                        l[k] += li.col[k] * d * att;
                    }
                    if want_spec {
                        let hv = nrm3([ld[0] + vdir[0], ld[1] + vdir[1], ld[2] + vdir[2]]);
                        let mut s = dotf(n, hv).max(0.0).powf(o.shin);
                        if o.kind == 2 {
                            s = if s > 0.5 { 1.0 } else { 0.0 };
                        }
                        for k in 0..3 {
                            spec[k] += li.col[k] * s * o.spec * att;
                        }
                    }
                }
                // soft shoulder: light sums above 0.8 roll off toward 1, so a fully lit face shows
                // its colour instead of clipping to white (specular still can)
                let sh = |v: f32| if v <= 0.8 { v } else { 0.8 + 0.2 * (1.0 - (-(v - 0.8) / 0.2).exp()) };
                [base[0] * sh(l[0]) + spec[0], base[1] * sh(l[1]) + spec[1], base[2] * sh(l[2]) + spec[2]]
            }
        };
        let e = [base[0] * o.emissive, base[1] * o.emissive, base[2] * o.emissive];
        for k in 0..3 {
            c[k] += e[k];
        }
        if let Some((exp, near, far, dens, fc)) = self.fog {
            let d = -pv[2];
            let f = if exp { 1.0 - (-dens * d).exp() } else { ((d - near) / (far - near).max(1e-6)).clamp(0.0, 1.0) };
            for k in 0..3 {
                c[k] = c[k] * (1.0 - f) + fc[k] * f;
            }
        }
        let a = alpha.clamp(0.0, 1.0);
        ([c[0].clamp(0.0, 1.0) * a, c[1].clamp(0.0, 1.0) * a, c[2].clamp(0.0, 1.0) * a, a], [e[0] * a, e[1] * a, e[2] * a], n)
    }
    /// Interpolated 1/w at a SS pixel center (screen-linear).
    fn invw_at(&self, t: &Tri, sx: f64, sy: f64) -> f64 {
        let (x0, y0) = (t.x[0] as f64 / 256.0, t.y[0] as f64 / 256.0);
        (t.invw[0] as f64 + t.grad[0][0] as f64 * (sx - x0) + t.grad[0][1] as f64 * (sy - y0)).max(1e-12)
    }
}

/// Screen-space outlines: seeds where neighbouring samples belong to different objects (the
/// nearer one owns the edge) or where an object's normal bends past its crease angle; every
/// sample within the owner's half width of a seed is painted in the owner's outline colour
/// (exact Euclidean distance inside a window, separable: nearest seed per row, then columns).
#[allow(clippy::too_many_arguments)]
fn outline(col: &mut [[u8; 4]], tri: &[u32], zb: &[f32], nrm: &[u32], tris: &[Tri], objs: &[ObjF], w: usize, h: usize, bufs: (&mut Vec<u32>, &mut Vec<u32>, &mut Vec<f32>, &mut Vec<u32>), fogf: &(dyn Fn(f32) -> f32 + Sync)) {
    let tq = std::time::Instant::now();
    let dbg = agentfx_ir::env::var("AGENTFX_S3D_DEBUG").is_ok();
    let (b_own, b_seed, b_skey, b_hx) = bufs;
    if b_own.len() < w * h {
        b_own.resize(w * h, NONE);
    }
    let own = &mut b_own[..w * h];
    own.par_chunks_mut(1 << 16).zip(tri.par_chunks(1 << 16)).for_each(|(o, t)| {
        for (a, &b) in o.iter_mut().zip(t) {
            *a = if b == NONE { NONE } else { tris[b as usize].obj };
        }
    });
    let own = &*own;
    let owner = |i: usize| -> u32 { own[i] };
    let rmax = (objs.iter().map(|o| o.outline / 2.0).fold(0.0f32, f32::max).ceil() as i64 + 1).min(250);
    // owners are packed into 23 bits next to the offset
    let objs_ok = objs.len() < (1 << 23);
    if !objs_ok {
        return;
    }
    // seed owner per sample (NONE = no seed)
    let seed = prep(b_seed, w * h, NONE);
    let skey = prep(b_skey, w * h, 0f32);
    seed.par_chunks_mut(w).zip(skey.par_chunks_mut(w)).enumerate().for_each(|(y, (row, krow))| {
        for x in 0..w {
            let i = y * w + x;
            let a = owner(i);
            let mut best: Option<(f32, u32)> = None;
            let ti = tri[i];
            let mut consider = |j: usize| {
                if tri[j] == ti {
                    return;
                }
                let b = owner(j);
                let (ka, kb) = (zb[i], zb[j]);
                let edge_owner = if a != b {
                    // the nearer object draws the edge; if it has no outline, the one behind
                    // (its silhouette is still an edge, e.g. a box standing on a plain floor)
                    let (f, bk) = if ka >= kb { (a, b) } else { (b, a) };
                    let has = |o: u32| o != NONE && objs[o as usize].outline > 0.0;
                    Some((ka.max(kb), if has(f) { f } else { bk }))
                } else if a != NONE {
                    let o = &objs[a as usize];
                    let na = oct_dec(nrm[i]);
                    let nb = oct_dec(nrm[j]);
                    let rel = (ka - kb).abs() / ka.max(kb).max(1e-20);
                    if dotf(na, nb) < o.crease_cos || rel > 0.08 { Some((ka.max(kb), a)) } else { None }
                } else {
                    None
                };
                if let Some((k, f)) = edge_owner {
                    if f != NONE && objs[f as usize].outline > 0.0 && best.is_none_or(|(bk, bf)| k > bk || (k == bk && f < bf)) {
                        best = Some((k, f));
                    }
                }
            };
            if x > 0 {
                consider(i - 1);
            }
            if x + 1 < w {
                consider(i + 1);
            }
            if y > 0 {
                consider(i - w);
            }
            if y + 1 < h {
                consider(i + w);
            }
            if let Some((k, f)) = best {
                row[x] = f;
                krow[x] = k;
            }
        }
    });
    if dbg {
        eprintln!("  outline seeds {:.1}", tq.elapsed().as_secs_f64() * 1e3);
    }
    // nearest seed per row within rmax: (dx, owner)
    // nearest seed per row: dx + 256 (low 9 bits, signed offset to the seed) | owner << 9
    let hx = prep(b_hx, w * h, NONE);
    hx.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &seed[y * w..(y + 1) * w];
        let mut last: Option<usize> = None;
        for x in 0..w {
            if s[x] != NONE {
                last = Some(x);
            }
            if let Some(l) = last {
                let d = (x - l) as i64;
                if d <= rmax {
                    row[x] = (256 - d) as u32 | (s[l] << 9);
                }
            }
        }
        let mut next: Option<usize> = None;
        for x in (0..w).rev() {
            if s[x] != NONE {
                next = Some(x);
            }
            if let Some(nx) = next {
                let d = (nx - x) as i64;
                if d <= rmax && (row[x] == NONE || d < ((row[x] & 0x1ff) as i64 - 256).abs()) {
                    row[x] = (256 + d) as u32 | (s[nx] << 9);
                }
            }
        }
    });
    if dbg {
        eprintln!("  outline rows {:.1}", tq.elapsed().as_secs_f64() * 1e3);
    }
    // per sample: min over dy of dx^2 + dy^2
    col.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut best = (f32::MAX, NONE);
            let mut bkey = 0f32;
            let ylo = (y as i64 - rmax).max(0) as usize;
            let yhi = ((y as i64 + rmax) as usize).min(h - 1);
            for yy in ylo..=yhi {
                let v = hx[yy * w + x];
                if v == NONE {
                    continue;
                }
                let (sdx, o) = ((v & 0x1ff) as i64 - 256, v >> 9);
                let dy = yy as f32 - y as f32;
                let d2 = (sdx as f32) * (sdx as f32) + dy * dy;
                if d2 < best.0 || (d2 == best.0 && o < best.1) {
                    best = (d2, o);
                    bkey = skey[yy * w + (x as i64 + sdx) as usize];
                }
            }
            if best.1 == NONE {
                continue;
            }
            let ob = &objs[best.1 as usize];
            let r = ob.outline / 2.0 - 0.5;
            let a = (r + 0.5 - best.0.sqrt()).clamp(0.0, 1.0) * ob.outline_color[3] * (1.0 - fogf(bkey));
            if a <= 0.0 {
                continue;
            }
            let p = &mut row[x];
            for k in 0..3 {
                p[k] = ((ob.outline_color[k] * a + p[k] as f32 / 255.0 * (1.0 - a)) * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            p[3] = ((a + p[3] as f32 / 255.0 * (1.0 - a)) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    });
}

/// Gaussian-like blur (three box passes per axis) of an interleaved f32 image with `ch`
/// channels; deterministic (rows, then columns, each independent).
pub fn blur_f32(buf: &mut [f32], w: usize, h: usize, ch: usize, sigma: f64) {
    if sigma < 0.3 || w == 0 || h == 0 {
        return;
    }
    // box widths for 3 passes approximating a gaussian (Kovesi)
    let n = 3.0;
    let wi = ((12.0 * sigma * sigma / n + 1.0).sqrt()).floor() as i64;
    let wl = if wi % 2 == 0 { wi - 1 } else { wi };
    let wu = wl + 2;
    let m = ((12.0 * sigma * sigma - n * (wl * wl) as f64 - 4.0 * n * wl as f64 - 3.0 * n) / (-4.0 * wl as f64 - 4.0)).round() as i64;
    let radii: Vec<usize> = (0..3).map(|i| (((if i < m { wl } else { wu }) - 1) / 2).max(0) as usize).collect();
    // one box pass over a contiguous line of `len` pixels (zero outside)
    fn pass(line: &mut [f32], len: usize, ch: usize, r: usize, tmp: &mut Vec<f32>) {
        if r == 0 {
            return;
        }
        tmp.clear();
        tmp.extend_from_slice(&line[..len * ch]);
        let k = 1.0 / (2 * r + 1) as f32;
        for c in 0..ch {
            let mut acc = 0f32;
            for i in 0..r.min(len) {
                acc += tmp[i * ch + c];
            }
            for i in 0..len {
                if i + r < len {
                    acc += tmp[(i + r) * ch + c];
                }
                line[i * ch + c] = acc * k;
                if i >= r {
                    acc -= tmp[(i - r) * ch + c];
                }
            }
        }
    }
    buf.par_chunks_mut(w * ch).for_each(|row| {
        let mut tmp = Vec::new();
        for &r in &radii {
            pass(row, w, ch, r, &mut tmp);
        }
    });
    // columns: blocks of 32 columns, gathered (contiguous row segments), filtered with one
    // running sum per column, scattered back
    const CW: usize = 32;
    let nblk = w.div_ceil(CW);
    let blocks: Vec<Vec<f32>> = (0..nblk)
        .into_par_iter()
        .map(|b| {
            let x0 = b * CW;
            let cw = (w - x0).min(CW) * ch;
            let mut blk = vec![0f32; h * cw];
            for y in 0..h {
                blk[y * cw..(y + 1) * cw].copy_from_slice(&buf[(y * w + x0) * ch..(y * w + x0) * ch + cw]);
            }
            let mut src = vec![0f32; h * cw];
            let mut acc = vec![0f32; cw];
            for &r in &radii {
                if r == 0 {
                    continue;
                }
                src.copy_from_slice(&blk);
                let k = 1.0 / (2 * r + 1) as f32;
                acc.iter_mut().for_each(|a| *a = 0.0);
                for y in 0..r.min(h) {
                    for (a, v) in acc.iter_mut().zip(&src[y * cw..(y + 1) * cw]) {
                        *a += *v;
                    }
                }
                for y in 0..h {
                    if y + r < h {
                        for (a, v) in acc.iter_mut().zip(&src[(y + r) * cw..(y + r + 1) * cw]) {
                            *a += *v;
                        }
                    }
                    for (o, a) in blk[y * cw..(y + 1) * cw].iter_mut().zip(&acc) {
                        *o = *a * k;
                    }
                    if y >= r {
                        for (a, v) in acc.iter_mut().zip(&src[(y - r) * cw..(y - r + 1) * cw]) {
                            *a -= *v;
                        }
                    }
                }
            }
            blk
        })
        .collect();
    for (b, blk) in blocks.iter().enumerate() {
        let x0 = b * CW;
        let cw = (w - x0).min(CW) * ch;
        for y in 0..h {
            buf[(y * w + x0) * ch..(y * w + x0) * ch + cw].copy_from_slice(&blk[y * cw..(y + 1) * cw]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oct_roundtrip() {
        for n in [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.3, -0.5, 0.81], [-0.6, 0.6, -0.52]] {
            let n = nrm3(n);
            let m = oct_dec(oct_enc(n));
            assert!(dotf(n, m) > 0.99999, "{n:?} {m:?}");
        }
    }
    #[test]
    fn shared_edges_cover_every_sample_once() {
        // two triangles sharing a diagonal cover a square exactly once (no cracks, no double hits)
        let mk = |p: [[f64; 2]; 3]| {
            let va = |q: [f64; 2]| VA { pv: [q[0] as f32, q[1] as f32, -1.0], nv: [0.0, 0.0, 1.0], uv: [0.0, 0.0] };
            let x = [(p[0][0] * 256.0) as i64, (p[1][0] * 256.0) as i64, (p[2][0] * 256.0) as i64];
            let y = [(p[0][1] * 256.0) as i64, (p[1][1] * 256.0) as i64, (p[2][1] * 256.0) as i64];
            let mut t = Tri { x, y, invw: [1.0; 3], key: [1.0; 3], a: [va(p[0]), va(p[1]), va(p[2])], obj: 0, back: false, grad: [[0.0; 2]; 3], y0: 0, y1: 16 };
            let area = (x[1] - x[0]) * (y[2] - y[0]) - (y[1] - y[0]) * (x[2] - x[0]);
            if area < 0 {
                t.x.swap(1, 2);
                t.y.swap(1, 2);
            }
            t
        };
        let mut hits = vec![0u8; 16 * 16];
        // diagonal through sample centers on purpose (ties)
        for t in [mk([[2.5, 2.5], [12.5, 2.5], [12.5, 12.5]]), mk([[2.5, 2.5], [12.5, 12.5], [2.5, 12.5]])] {
            raster(&t, 16, 0, 16, |x, y, _, _, _| hits[y * 16 + x] += 1);
        }
        assert!(hits.iter().all(|&h| h <= 1));
        let covered = hits.iter().filter(|&&h| h == 1).count();
        assert_eq!(covered, 100, "a 10x10 square of sample centers");
    }
    #[test]
    fn near_clip_splits_a_crossing_triangle() {
        let va = |z: f32| VA { pv: [0.0, z, z], nv: [0.0, 0.0, 1.0], uv: [0.0, 0.0] };
        let p = clip_near(&[va(-2.0), va(-2.0), va(1.0)], 0.5);
        assert_eq!(p.len(), 4);
        assert!(p.iter().all(|v| -v.pv[2] >= 0.5 - 1e-6));
        assert!(clip_near(&[va(1.0), va(2.0), va(0.1)], 0.5).is_empty());
    }
    #[test]
    fn blur_keeps_mass() {
        let mut b = vec![0f32; 64 * 64];
        b[32 * 64 + 32] = 1.0;
        blur_f32(&mut b, 64, 64, 1, 3.0);
        let s: f32 = b.iter().sum();
        assert!((s - 1.0).abs() < 1e-3, "{s}");
        assert!(b[32 * 64 + 32] < 0.1);
    }
}
