//! Geometry for `Content::Scene3d` objects (built once at load): boxes (optionally rounded),
//! planes, UV spheres, cylinders/cones, tori, extruded text and SVG paths (nonzero fill,
//! triangulated by a scanline trapezoid sweep, so any self-overlapping glyph outline works),
//! and minimal Wavefront OBJ meshes. Plus feature edges (creases and borders) for wire lines.
use agentfx_ir::s3d::{self, V3};
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub tris: Vec<[u32; 3]>,
    /// wire edges (faces meeting at more than 1 degree, open borders), as position pairs
    pub edges: Vec<([f32; 3], [f32; 3])>,
    /// closed surface: back faces may be culled
    pub closed: bool,
}

impl Mesh {
    fn vert(&mut self, p: V3, n: V3, uv: [f64; 2]) -> u32 {
        self.pos.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        self.nrm.push([n[0] as f32, n[1] as f32, n[2] as f32]);
        self.uv.push([uv[0] as f32, uv[1] as f32]);
        (self.pos.len() - 1) as u32
    }
    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        // a b c d counter-clockwise seen from the front
        self.tris.push([a, b, c]);
        self.tris.push([a, c, d]);
    }
    /// Local bounding box.
    #[cfg(test)]
    pub fn bounds(&self) -> Option<(V3, V3)> {
        let mut it = self.pos.iter();
        let f = it.next()?;
        let mut lo = [f[0] as f64, f[1] as f64, f[2] as f64];
        let mut hi = lo;
        for p in it {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k] as f64);
                hi[k] = hi[k].max(p[k] as f64);
            }
        }
        Some((lo, hi))
    }

    /// Feature edges: welded positions, edges whose faces meet at more than `angle` degrees,
    /// or that have one face (borders). Deterministic order (by first occurrence).
    pub fn compute_edges(&mut self, angle: f64) {
        let q = |p: &[f32; 3]| [(p[0] * 1e4).round() as i64, (p[1] * 1e4).round() as i64, (p[2] * 1e4).round() as i64];
        let mut weld: HashMap<[i64; 3], u32> = HashMap::new();
        let mut wid = Vec::with_capacity(self.pos.len());
        let mut wpos: Vec<[f32; 3]> = Vec::new();
        for p in &self.pos {
            let k = q(p);
            let id = *weld.entry(k).or_insert_with(|| {
                wpos.push(*p);
                (wpos.len() - 1) as u32
            });
            wid.push(id);
        }
        let fnorm = |t: &[u32; 3]| -> V3 {
            let p = |i: u32| {
                let v = self.pos[i as usize];
                [v[0] as f64, v[1] as f64, v[2] as f64]
            };
            s3d::norm(s3d::cross(s3d::sub(p(t[1]), p(t[0])), s3d::sub(p(t[2]), p(t[0]))))
        };
        let mut order: Vec<(u32, u32)> = Vec::new();
        let mut faces: HashMap<(u32, u32), Vec<V3>> = HashMap::new();
        for t in &self.tris {
            let n = fnorm(t);
            if s3d::len(n) == 0.0 {
                continue;
            }
            for k in 0..3 {
                let (a, b) = (wid[t[k] as usize], wid[t[(k + 1) % 3] as usize]);
                if a == b {
                    continue;
                }
                let key = (a.min(b), a.max(b));
                let e = faces.entry(key).or_insert_with(|| {
                    order.push(key);
                    Vec::new()
                });
                e.push(n);
            }
        }
        let c = angle.to_radians().cos();
        self.edges.clear();
        for key in order {
            let ns = &faces[&key];
            let feature = ns.len() == 1 || ns.iter().skip(1).any(|n| s3d::dot(*n, ns[0]) < c);
            if feature {
                self.edges.push((wpos[key.0 as usize], wpos[key.1 as usize]));
            }
        }
    }
}

/// Box, centered; `r` > 0 rounds edges and corners (smooth normals on the rounding).
pub fn cuboid(size: V3, r: f64) -> Mesh {
    let h = [size[0] / 2.0, size[1] / 2.0, size[2] / 2.0];
    let mut m = Mesh { closed: true, ..Default::default() };
    // faces: (normal axis, sign, u axis, v axis) with u x v = normal (counter-clockwise front)
    let faces: [(usize, f64, usize, f64, usize, f64); 6] = [
        (2, 1.0, 0, 1.0, 1, 1.0),   // +z: u = +x, v = +y
        (2, -1.0, 0, -1.0, 1, 1.0), // -z: u = -x
        (0, 1.0, 2, -1.0, 1, 1.0),  // +x: u = -z
        (0, -1.0, 2, 1.0, 1, 1.0),  // -x: u = +z
        (1, 1.0, 0, 1.0, 2, -1.0),  // +y: u = +x, v = -z
        (1, -1.0, 0, 1.0, 2, 1.0),  // -y: u = +x, v = +z
    ];
    let r = r.clamp(0.0, h[0].min(h[1]).min(h[2]));
    let k = if r > 0.0 { 5usize } else { 0 };
    // coordinates along an axis of half extent hh: rounded band in tan-spaced steps
    let coords = |hh: f64| -> Vec<f64> {
        let inner = hh - r;
        let mut v = Vec::new();
        for i in (0..=k).rev() {
            v.push(-inner - r * (i as f64 / k.max(1) as f64 * std::f64::consts::FRAC_PI_4).tan());
        }
        for i in 0..=k {
            v.push(inner + r * (i as f64 / k.max(1) as f64 * std::f64::consts::FRAC_PI_4).tan());
        }
        if k == 0 {
            v = vec![-hh, hh];
        }
        v.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
        v
    };
    for &(na, ns, ua, us, va, vs) in &faces {
        let cu = coords(h[ua]);
        let cv = coords(h[va]);
        let base = m.pos.len() as u32;
        for &vv in cv.iter() {
            for &uu in cu.iter() {
                let mut p = [0.0; 3];
                p[na] = ns * h[na];
                p[ua] = us * uu;
                p[va] = vs * vv;
                let (q, n) = if r > 0.0 {
                    let inner = [p[0].clamp(-(h[0] - r), h[0] - r), p[1].clamp(-(h[1] - r), h[1] - r), p[2].clamp(-(h[2] - r), h[2] - r)];
                    let n = s3d::norm(s3d::sub(p, inner));
                    (s3d::add(inner, s3d::mul(n, r)), n)
                } else {
                    let mut n = [0.0; 3];
                    n[na] = ns;
                    (p, n)
                };
                let uvx = (uu + h[ua]) / (2.0 * h[ua]);
                let uvy = 1.0 - (vv + h[va]) / (2.0 * h[va]);
                m.vert(q, n, [uvx, uvy]);
            }
        }
        let nu = cu.len() as u32;
        for j in 0..cv.len() as u32 - 1 {
            for i in 0..nu - 1 {
                let a = base + j * nu + i;
                m.quad(a, a + 1, a + 1 + nu, a + nu);
            }
        }
    }
    m.compute_edges(1.0);
    m
}

/// Rectangle in xy facing +z (drawn from both sides).
pub fn plane(w: f64, h: f64) -> Mesh {
    let mut m = Mesh::default();
    let n = [0.0, 0.0, 1.0];
    let a = m.vert([-w / 2.0, -h / 2.0, 0.0], n, [0.0, 1.0]);
    let b = m.vert([w / 2.0, -h / 2.0, 0.0], n, [1.0, 1.0]);
    let c = m.vert([w / 2.0, h / 2.0, 0.0], n, [1.0, 0.0]);
    let d = m.vert([-w / 2.0, h / 2.0, 0.0], n, [0.0, 0.0]);
    m.quad(a, b, c, d);
    m.compute_edges(1.0);
    m
}

pub fn sphere(r: f64, seg: u32) -> Mesh {
    let mut m = Mesh { closed: true, ..Default::default() };
    let lon = seg.max(3) as usize;
    let lat = (seg / 2).max(2) as usize;
    for j in 0..=lat {
        let th = std::f64::consts::PI * j as f64 / lat as f64;
        for i in 0..=lon {
            let ph = 2.0 * std::f64::consts::PI * i as f64 / lon as f64;
            let n = [th.sin() * ph.sin(), th.cos(), th.sin() * ph.cos()];
            m.vert(s3d::mul(n, r), n, [i as f64 / lon as f64, j as f64 / lat as f64]);
        }
    }
    let row = lon as u32 + 1;
    for j in 0..lat as u32 {
        for i in 0..lon as u32 {
            let a = j * row + i;
            // a (top-left), a+row (bottom-left): counter-clockwise from outside
            m.quad(a, a + row, a + row + 1, a + 1);
        }
    }
    m.compute_edges(1.0);
    m
}

/// Cylinder along y (cone when the top radius is 0), closed with flat caps.
pub fn cylinder(r0: f64, r1: f64, h: f64, seg: u32) -> Mesh {
    let mut m = Mesh { closed: true, ..Default::default() };
    let n = seg.max(3) as usize;
    let (y0, y1) = (-h / 2.0, h / 2.0);
    // side normal tilts with the slope
    let slope = (r0 - r1) / h;
    let base = m.pos.len() as u32;
    for i in 0..=n {
        let ph = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
        let (s, c) = ph.sin_cos();
        let nn = s3d::norm([s, slope, c]);
        m.vert([r0 * s, y0, r0 * c], nn, [i as f64 / n as f64, 1.0]);
        m.vert([r1 * s, y1, r1 * c], nn, [i as f64 / n as f64, 0.0]);
    }
    for i in 0..n as u32 {
        let a = base + 2 * i;
        m.quad(a, a + 2, a + 3, a + 1);
    }
    for (y, r, up) in [(y1, r1, 1.0), (y0, r0, -1.0)] {
        if r <= 0.0 {
            continue;
        }
        let c = m.vert([0.0, y, 0.0], [0.0, up, 0.0], [0.5, 0.5]);
        let first = m.pos.len() as u32;
        for i in 0..=n {
            let ph = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            let (s, co) = ph.sin_cos();
            m.vert([r * s, y, r * co], [0.0, up, 0.0], [0.5 + 0.5 * s, 0.5 - 0.5 * co]);
        }
        for i in 0..n as u32 {
            if up > 0.0 {
                m.tris.push([c, first + i, first + i + 1]);
            } else {
                m.tris.push([c, first + i + 1, first + i]);
            }
        }
    }
    m.compute_edges(1.0);
    m
}

/// Torus in the xz plane (ring radius `r`, tube radius `t`).
pub fn torus(r: f64, t: f64, seg: u32) -> Mesh {
    let mut m = Mesh { closed: true, ..Default::default() };
    let nu = seg.max(3) as usize;
    let nv = (seg / 2).max(3) as usize;
    for j in 0..=nv {
        let v = 2.0 * std::f64::consts::PI * j as f64 / nv as f64;
        for i in 0..=nu {
            let u = 2.0 * std::f64::consts::PI * i as f64 / nu as f64;
            let (su, cu) = u.sin_cos();
            let (sv, cv) = v.sin_cos();
            let n = [cv * su, sv, cv * cu];
            let p = [(r + t * cv) * su, t * sv, (r + t * cv) * cu];
            m.vert(p, n, [i as f64 / nu as f64, j as f64 / nv as f64]);
        }
    }
    let row = nu as u32 + 1;
    for j in 0..nv as u32 {
        for i in 0..nu as u32 {
            let a = j * row + i;
            m.quad(a, a + 1, a + 1 + row, a + row);
        }
    }
    m.compute_edges(1.0);
    m
}

// ---------------------------------------------------------------- extrusion

/// Filled area of closed contours (nonzero winding) as triangles, by a scanline sweep:
/// between consecutive vertex y levels the active edges are sorted, spans with nonzero winding
/// become trapezoids, and each trapezoid is zipped into triangles using every crossing on its
/// top and bottom lines (no T-junctions, so the cap is watertight).
pub fn fill_tris(contours: &[Vec<[f64; 2]>]) -> Vec<[[f64; 2]; 3]> {
    struct E {
        a: [f64; 2],
        b: [f64; 2],
        w: i32,
    }
    let mut edges: Vec<E> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    for c in contours {
        let n = c.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (p, q) = (c[i], c[(i + 1) % n]);
            ys.push(p[1]);
            if p[1] == q[1] {
                continue;
            }
            let (a, b, w) = if p[1] < q[1] { (p, q, 1) } else { (q, p, -1) };
            edges.push(E { a, b, w });
        }
    }
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ys.dedup();
    let x_at = |e: &E, y: f64| -> f64 {
        if y <= e.a[1] {
            e.a[0]
        } else if y >= e.b[1] {
            e.b[0]
        } else {
            e.a[0] + (e.b[0] - e.a[0]) * (y - e.a[1]) / (e.b[1] - e.a[1])
        }
    };
    // all x positions on each level (edge crossings and vertices), sorted
    let mut level: Vec<Vec<f64>> = vec![Vec::new(); ys.len()];
    for e in &edges {
        let i0 = ys.partition_point(|&y| y < e.a[1]);
        let i1 = ys.partition_point(|&y| y <= e.b[1]);
        for (i, lv) in level.iter_mut().enumerate().take(i1).skip(i0) {
            lv.push(x_at(e, ys[i]));
        }
    }
    for lv in &mut level {
        lv.sort_by(|a, b| a.partial_cmp(b).unwrap());
        lv.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    }
    let mut out = Vec::new();
    for k in 0..ys.len().saturating_sub(1) {
        let (y0, y1) = (ys[k], ys[k + 1]);
        let ym = 0.5 * (y0 + y1);
        let mut act: Vec<(f64, f64, f64, i32)> = edges.iter().filter(|e| e.a[1] <= y0 && e.b[1] >= y1).map(|e| (x_at(e, ym), x_at(e, y0), x_at(e, y1), e.w)).collect();
        act.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut wind = 0;
        let mut i = 0;
        while i < act.len() {
            let before = wind;
            wind += act[i].3;
            if before == 0 && wind != 0 {
                // span starts at i: find where winding returns to 0
                let mut j = i + 1;
                let mut w2 = wind;
                while j < act.len() {
                    w2 += act[j].3;
                    if w2 == 0 {
                        break;
                    }
                    j += 1;
                }
                if j >= act.len() {
                    break;
                }
                let (l, r) = (act[i], act[j]);
                let top: Vec<[f64; 2]> = std::iter::once(l.1).chain(level[k].iter().copied().filter(|&x| x > l.1 + 1e-12 && x < r.1 - 1e-12)).chain(std::iter::once(r.1)).map(|x| [x, y0]).collect();
                let bot: Vec<[f64; 2]> = std::iter::once(l.2).chain(level[k + 1].iter().copied().filter(|&x| x > l.2 + 1e-12 && x < r.2 - 1e-12)).chain(std::iter::once(r.2)).map(|x| [x, y1]).collect();
                zip(&top, &bot, &mut out);
                wind = 0;
                i = j + 1;
                continue;
            }
            i += 1;
        }
    }
    out
}

fn zip(top: &[[f64; 2]], bot: &[[f64; 2]], out: &mut Vec<[[f64; 2]; 3]>) {
    let (mut i, mut j) = (0usize, 0usize);
    let push = |a: [f64; 2], b: [f64; 2], c: [f64; 2], out: &mut Vec<[[f64; 2]; 3]>| {
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area.abs() > 1e-14 {
            out.push([a, b, c]);
        }
    };
    while i + 1 < top.len() || j + 1 < bot.len() {
        let adv_top = if i + 1 >= top.len() {
            false
        } else if j + 1 >= bot.len() {
            true
        } else {
            top[i + 1][0] <= bot[j + 1][0]
        };
        if adv_top {
            push(top[i], top[i + 1], bot[j], out);
            i += 1;
        } else {
            push(top[i], bot[j + 1], bot[j], out);
            j += 1;
        }
    }
}

/// Nonzero winding number of point p.
fn winding(contours: &[Vec<[f64; 2]>], p: [f64; 2]) -> i32 {
    let mut w = 0;
    for c in contours {
        let n = c.len();
        for i in 0..n {
            let (a, b) = (c[i], c[(i + 1) % n]);
            if a[1] <= p[1] {
                if b[1] > p[1] && (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]) > 0.0 {
                    w += 1;
                }
            } else if b[1] <= p[1] && (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]) < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

/// Extrude closed contours (2D, y up, already in world units and centered) by `depth` along z
/// (front cap at +depth/2). Side normals are smoothed across bends below 30 degrees.
pub fn extrude(contours: &[Vec<[f64; 2]>], depth: f64) -> Mesh {
    let mut m = Mesh { closed: true, ..Default::default() };
    let (zf, zb) = (depth / 2.0, -depth / 2.0);
    // caps
    let tris = fill_tris(contours);
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for c in contours {
        for p in c {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
    }
    let ext = [(hi[0] - lo[0]).max(1e-9), (hi[1] - lo[1]).max(1e-9)];
    let uv = |p: [f64; 2]| [(p[0] - lo[0]) / ext[0], 1.0 - (p[1] - lo[1]) / ext[1]];
    for t in &tris {
        let ccw = (t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0]) > 0.0;
        let t = if ccw { *t } else { [t[0], t[2], t[1]] };
        let a = m.vert([t[0][0], t[0][1], zf], [0.0, 0.0, 1.0], uv(t[0]));
        let b = m.vert([t[1][0], t[1][1], zf], [0.0, 0.0, 1.0], uv(t[1]));
        let c = m.vert([t[2][0], t[2][1], zf], [0.0, 0.0, 1.0], uv(t[2]));
        m.tris.push([a, b, c]);
        if depth > 0.0 {
            let a = m.vert([t[0][0], t[0][1], zb], [0.0, 0.0, -1.0], uv(t[0]));
            let b = m.vert([t[1][0], t[1][1], zb], [0.0, 0.0, -1.0], uv(t[1]));
            let c = m.vert([t[2][0], t[2][1], zb], [0.0, 0.0, -1.0], uv(t[2]));
            m.tris.push([a, c, b]);
        }
    }
    if depth <= 0.0 {
        m.closed = false;
        m.compute_edges(1.0);
        return m;
    }
    // sides: per contour edge, outward = the side that is not filled
    let cos30 = 30f64.to_radians().cos();
    for c in contours {
        let n = c.len();
        if n < 3 {
            continue;
        }
        // outward normal per edge
        let mut en: Vec<Option<[f64; 2]>> = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b) = (c[i], c[(i + 1) % n]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = d[0].hypot(d[1]);
            if l < 1e-12 {
                en.push(None);
                continue;
            }
            let left = [-d[1] / l, d[0] / l];
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let eps = (l * 1e-3).clamp(1e-7, 1e-3);
            let wl = winding(contours, [mid[0] + left[0] * eps, mid[1] + left[1] * eps]);
            let wr = winding(contours, [mid[0] - left[0] * eps, mid[1] - left[1] * eps]);
            en.push(match (wl != 0, wr != 0) {
                (true, false) => Some([-left[0], -left[1]]),
                (false, true) => Some(left),
                _ => None, // interior or degenerate edge: no wall
            });
        }
        let prev_valid = |i: usize| -> Option<[f64; 2]> {
            for k in 1..n {
                if let Some(v) = en[(i + n - k) % n] {
                    return Some(v);
                }
            }
            None
        };
        let next_valid = |i: usize| -> Option<[f64; 2]> {
            for k in 1..n {
                if let Some(v) = en[(i + k) % n] {
                    return Some(v);
                }
            }
            None
        };
        let mut acc = 0.0;
        let total: f64 = (0..n).map(|i| {
            let (a, b) = (c[i], c[(i + 1) % n]);
            (b[0] - a[0]).hypot(b[1] - a[1])
        }).sum::<f64>().max(1e-9);
        for i in 0..n {
            let Some(ne) = en[i] else { continue };
            let (a, b) = (c[i], c[(i + 1) % n]);
            let l = (b[0] - a[0]).hypot(b[1] - a[1]);
            let smooth = |o: Option<[f64; 2]>| -> [f64; 2] {
                match o {
                    Some(o) if o[0] * ne[0] + o[1] * ne[1] > cos30 => {
                        let s = [o[0] + ne[0], o[1] + ne[1]];
                        let k = s[0].hypot(s[1]).max(1e-12);
                        [s[0] / k, s[1] / k]
                    }
                    _ => ne,
                }
            };
            let na = smooth(prev_valid(i));
            let nb = smooth(next_valid(i));
            let (u0, u1) = (acc / total, (acc + l) / total);
            acc += l;
            let v0 = m.vert([a[0], a[1], zf], [na[0], na[1], 0.0], [u0, 0.0]);
            let v1 = m.vert([b[0], b[1], zf], [nb[0], nb[1], 0.0], [u1, 0.0]);
            let v2 = m.vert([b[0], b[1], zb], [nb[0], nb[1], 0.0], [u1, 1.0]);
            let v3 = m.vert([a[0], a[1], zb], [na[0], na[1], 0.0], [u0, 1.0]);
            // counter-clockwise seen from outside: check against the outward normal
            let e = [b[0] - a[0], b[1] - a[1], 0.0];
            let down = [0.0, 0.0, -depth];
            let face = s3d::cross(e, down);
            // quad (v0, v1, v2, v3) faces along e x down
            if s3d::dot(face, [ne[0], ne[1], 0.0]) > 0.0 {
                m.quad(v0, v1, v2, v3);
            } else {
                m.quad(v0, v3, v2, v1);
            }
        }
    }
    m.compute_edges(1.0);
    m
}

/// Flatten a kurbo path into closed contours (tolerance in path units).
pub fn contours(path: &kurbo::BezPath, tol: f64) -> Vec<Vec<[f64; 2]>> {
    let mut out: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut cur: Vec<[f64; 2]> = Vec::new();
    kurbo::flatten(path.iter(), tol, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            if cur.len() >= 3 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            cur.push([p.x, p.y]);
        }
        kurbo::PathEl::LineTo(p) => {
            if cur.last().is_none_or(|l| (l[0] - p.x).abs() > 1e-12 || (l[1] - p.y).abs() > 1e-12) {
                cur.push([p.x, p.y]);
            }
        }
        kurbo::PathEl::ClosePath => {
            if cur.len() >= 3 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
        _ => {}
    });
    if cur.len() >= 3 {
        out.push(cur);
    }
    for c in &mut out {
        while c.len() > 3 && {
            let (f, l) = (c[0], c[c.len() - 1]);
            (f[0] - l[0]).abs() < 1e-12 && (f[1] - l[1]).abs() < 1e-12
        } {
            c.pop();
        }
    }
    out
}

/// Contours (y down, scene-like units) -> world contours (y up), centered on their ink box and
/// scaled by `k`.
pub fn center_flip(cs: &[Vec<[f64; 2]>], k: f64) -> Vec<Vec<[f64; 2]>> {
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for c in cs {
        for p in c {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
    }
    let cx = (lo[0] + hi[0]) / 2.0;
    let cy = (lo[1] + hi[1]) / 2.0;
    cs.iter().map(|c| c.iter().map(|p| [(p[0] - cx) * k, -(p[1] - cy) * k]).collect()).collect()
}

/// Minimal Wavefront OBJ: `v`, `vn`, `vt`, `f` (polygons fanned; negative indices allowed).
/// Faces without normals get flat face normals. Normalized: bounding box centered, largest
/// extent = `size`.
pub fn obj(src: &str, size: f64) -> anyhow::Result<Mesh> {
    let mut vs: Vec<V3> = Vec::new();
    let mut vns: Vec<V3> = Vec::new();
    let mut vts: Vec<[f64; 2]> = Vec::new();
    let mut faces: Vec<Vec<(usize, Option<usize>, Option<usize>)>> = Vec::new();
    for (ln, line) in src.lines().enumerate() {
        let mut it = line.split_whitespace();
        let Some(tag) = it.next() else { continue };
        let nums = |it: std::str::SplitWhitespace| -> Vec<f64> { it.filter_map(|s| s.parse::<f64>().ok()).collect() };
        match tag {
            "v" => {
                let n = nums(it);
                anyhow::ensure!(n.len() >= 3, "obj line {}: vertex needs x y z", ln + 1);
                vs.push([n[0], n[1], n[2]]);
            }
            "vn" => {
                let n = nums(it);
                anyhow::ensure!(n.len() >= 3, "obj line {}: normal needs x y z", ln + 1);
                vns.push(s3d::norm([n[0], n[1], n[2]]));
            }
            "vt" => {
                let n = nums(it);
                vts.push([n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0)]);
            }
            "f" => {
                let mut f = Vec::new();
                for tok in it {
                    let mut p = tok.split('/');
                    let idx = |s: Option<&str>, len: usize| -> Option<usize> {
                        let v: i64 = s.filter(|s| !s.is_empty())?.parse().ok()?;
                        let i = if v < 0 { len as i64 + v } else { v - 1 };
                        (i >= 0 && (i as usize) < len).then_some(i as usize)
                    };
                    let vi = idx(p.next(), vs.len()).ok_or_else(|| anyhow::anyhow!("obj line {}: bad vertex index {tok}", ln + 1))?;
                    let ti = idx(p.next(), vts.len());
                    let ni = idx(p.next(), vns.len());
                    f.push((vi, ti, ni));
                }
                if f.len() >= 3 {
                    faces.push(f);
                }
            }
            _ => {}
        }
    }
    anyhow::ensure!(!faces.is_empty(), "obj: no faces");
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for v in &vs {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    let ext = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]).max(1e-12);
    let c = s3d::mul(s3d::add(lo, hi), 0.5);
    let k = size / ext;
    let tf = |v: V3| s3d::mul(s3d::sub(v, c), k);
    let mut m = Mesh { closed: true, ..Default::default() };
    for f in &faces {
        let fnrm = s3d::norm(s3d::cross(s3d::sub(vs[f[1].0], vs[f[0].0]), s3d::sub(vs[f[2].0], vs[f[0].0])));
        let ids: Vec<u32> = f.iter().map(|&(vi, ti, ni)| m.vert(tf(vs[vi]), ni.map(|n| vns[n]).unwrap_or(fnrm), ti.map(|t| [vts[t][0], 1.0 - vts[t][1]]).unwrap_or([0.0, 0.0]))).collect();
        for i in 1..ids.len() - 1 {
            m.tris.push([ids[0], ids[i], ids[i + 1]]);
        }
    }
    m.compute_edges(1.0);
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn area(ts: &[[[f64; 2]; 3]]) -> f64 {
        ts.iter().map(|t| ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0])).abs() / 2.0).sum()
    }
    #[test]
    fn fill_handles_holes_and_overlaps() {
        let sq = |x0: f64, y0: f64, s: f64, ccw: bool| {
            let mut v = vec![[x0, y0], [x0 + s, y0], [x0 + s, y0 + s], [x0, y0 + s]];
            if !ccw {
                v.reverse();
            }
            v
        };
        // square 10x10 with a reversed 4x4 hole: 100 - 16
        let t = fill_tris(&[sq(0.0, 0.0, 10.0, true), sq(3.0, 3.0, 4.0, false)]);
        assert!((area(&t) - 84.0).abs() < 1e-9, "{}", area(&t));
        // two overlapping same-direction squares: nonzero union = 100 + 100 - 25
        let t = fill_tris(&[sq(0.0, 0.0, 10.0, true), sq(5.0, 5.0, 10.0, true)]);
        assert!((area(&t) - 175.0).abs() < 1e-9, "{}", area(&t));
        // a triangle
        let t = fill_tris(&[vec![[0.0, 0.0], [4.0, 0.0], [0.0, 3.0]]]);
        assert!((area(&t) - 6.0).abs() < 1e-9);
    }
    #[test]
    fn meshes_are_closed_and_outward() {
        // every closed generator: the signed volume (divergence theorem) is positive
        let vol = |m: &Mesh| -> f64 {
            m.tris.iter().map(|t| {
                let p = |i: u32| {
                    let v = m.pos[i as usize];
                    [v[0] as f64, v[1] as f64, v[2] as f64]
                };
                s3d::dot(p(t[0]), s3d::cross(p(t[1]), p(t[2]))) / 6.0
            }).sum()
        };
        let b = cuboid([2.0, 1.0, 3.0], 0.0);
        assert!((vol(&b) - 6.0).abs() < 1e-6, "{}", vol(&b));
        assert_eq!(b.edges.len(), 12);
        let rb = cuboid([2.0, 2.0, 2.0], 0.3);
        assert!(vol(&rb) > 7.5 && vol(&rb) < 8.0, "{}", vol(&rb));
        let s = sphere(1.0, 48);
        assert!((vol(&s) - 4.18879).abs() < 0.05, "{}", vol(&s));
        let c = cylinder(1.0, 1.0, 2.0, 64);
        assert!((vol(&c) - 2.0 * std::f64::consts::PI).abs() < 0.05, "{}", vol(&c));
        let cone = cylinder(1.0, 0.0, 3.0, 64);
        assert!((vol(&cone) - std::f64::consts::PI).abs() < 0.05, "{}", vol(&cone));
        let t = torus(2.0, 0.5, 64);
        let want = 2.0 * std::f64::consts::PI * std::f64::consts::PI * 2.0 * 0.25;
        assert!((vol(&t) - want).abs() < 0.2, "{} vs {want}", vol(&t));
        // extruded square ring (hole): (16 - 4) * depth
        let ring = vec![vec![[-2.0, -2.0], [2.0, -2.0], [2.0, 2.0], [-2.0, 2.0]], vec![[-1.0, -1.0], [-1.0, 1.0], [1.0, 1.0], [1.0, -1.0]]];
        let e = extrude(&ring, 0.5);
        assert!((vol(&e) - 6.0).abs() < 1e-6, "{}", vol(&e));
        // the same with the hole wound the same way as the outer contour (nonzero: no hole)
        let solid = vec![ring[0].clone(), vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]];
        assert!((vol(&extrude(&solid, 0.5)) - 8.0).abs() < 1e-6);
    }
    #[test]
    fn obj_parses_and_normalizes() {
        let src = "v 0 0 0\nv 2 0 0\nv 2 2 0\nv 0 2 0\nf 1 2 3 4\n";
        let m = obj(src, 1.0).unwrap();
        assert_eq!(m.tris.len(), 2);
        let (lo, hi) = m.bounds().unwrap();
        assert!((hi[0] - lo[0] - 1.0).abs() < 1e-6 && (lo[0] + 0.5).abs() < 1e-6);
        assert!(obj("v 0 0 0\nf 1 2 3\n", 1.0).is_err());
    }
}
