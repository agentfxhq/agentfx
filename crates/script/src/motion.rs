//! Channel evaluation and scene-space transforms of the motion IR (shared by the reel-ir
//! backend and the native `lint --measure`). CSS semantics, as the HTML backend writes them:
//! `M = T(o) · translate(px%, py%) · magic · translate3d(x, y, z) · rotateX · rotateY · rotate ·
//! scale(sx, sy) · scale(s) · T(-o)`, a parent's `perspective` applies to its children about its
//! box center, and 3D is projected to the screen. Anchor translates are already part of the
//! native layout rects (`Native::rect`).
use crate::ir::*;
use std::collections::HashMap;

pub type M4 = [[f64; 4]; 4];

pub fn m4_id() -> M4 {
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
}
pub fn m4_mul(a: &M4, b: &M4) -> M4 {
    let mut r = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            r[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}
fn m4_t(x: f64, y: f64, z: f64) -> M4 {
    let mut m = m4_id();
    m[0][3] = x;
    m[1][3] = y;
    m[2][3] = z;
    m
}
fn m4_s(x: f64, y: f64) -> M4 {
    let mut m = m4_id();
    m[0][0] = x;
    m[1][1] = y;
    m
}
fn m4_rz(deg: f64) -> M4 {
    let (s, c) = deg.to_radians().sin_cos();
    let mut m = m4_id();
    m[0][0] = c;
    m[0][1] = -s;
    m[1][0] = s;
    m[1][1] = c;
    m
}
fn m4_rx(deg: f64) -> M4 {
    let (s, c) = deg.to_radians().sin_cos();
    let mut m = m4_id();
    m[1][1] = c;
    m[1][2] = -s;
    m[2][1] = s;
    m[2][2] = c;
    m
}
fn m4_ry(deg: f64) -> M4 {
    let (s, c) = deg.to_radians().sin_cos();
    let mut m = m4_id();
    m[0][0] = c;
    m[0][2] = s;
    m[2][0] = -s;
    m[2][2] = c;
    m
}
fn m4_persp(d: f64) -> M4 {
    let mut m = m4_id();
    m[3][2] = -1.0 / d.max(1.0);
    m
}

/// 2D affine `[a, b, c, d, e, f]`: x' = a x + c y + e, y' = b x + d y + f (CSS matrix order).
pub type A2 = [f64; 6];
pub const A2_ID: A2 = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
pub fn a2_mul(m: &A2, n: &A2) -> A2 {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}
pub fn a2_inv(m: &A2) -> A2 {
    let det = m[0] * m[3] - m[1] * m[2];
    let det = if det.abs() < 1e-12 { 1e-12 } else { det };
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    [a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])]
}
pub fn a2_apply(m: &A2, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}
pub fn a2_bbox(m: &A2, r: [f64; 4]) -> [f64; 4] {
    let pts = [(r[0], r[1]), (r[0] + r[2], r[1]), (r[0], r[1] + r[3]), (r[0] + r[2], r[1] + r[3])].map(|(x, y)| a2_apply(m, x, y));
    let x0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let y0 = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let x1 = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    let y1 = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
    [x0, y0, x1 - x0, y1 - y0]
}
pub fn a2_scale(m: &A2) -> f64 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt()
}

/// Projective map of a 4x4 at a point, linearized (Jacobian) into a 2D affine. Exact for
/// 2D-only chains and for pure depth translation (parallax); first-order for rotations.
pub fn linearize(m: &M4, cx: f64, cy: f64) -> A2 {
    let proj = |x: f64, y: f64| -> (f64, f64) {
        let v = [x, y, 0.0, 1.0];
        let q: Vec<f64> = (0..4).map(|i| (0..4).map(|k| m[i][k] * v[k]).sum()).collect();
        let w = if q[3].abs() < 1e-9 { 1e-9 } else { q[3] };
        (q[0] / w, q[1] / w)
    };
    let h = 1.0;
    let p0 = proj(cx, cy);
    let px = proj(cx + h, cy);
    let py = proj(cx, cy + h);
    let (a, b) = ((px.0 - p0.0) / h, (px.1 - p0.1) / h);
    let (c, d) = ((py.0 - p0.0) / h, (py.1 - p0.1) / h);
    [a, b, c, d, p0.0 - a * cx - c * cy, p0.1 - b * cx - d * cy]
}

/// Decompose an affine into reel-ir transform functions `[translate, rotate, scale, rotate]`
/// (SVD; exact for any affine incl. skew).
pub fn decompose(m: &A2) -> (f64, f64, f64, f64, f64, f64, f64) {
    let (a, b, c, d) = (m[0], m[1], m[2], m[3]);
    // A = [[a, c], [b, d]] = R(t1) S(sx, sy) R(t2)
    let e = (a + d) / 2.0;
    let f = (a - d) / 2.0;
    let g = (b + c) / 2.0;
    let h = (b - c) / 2.0;
    let q = (e * e + h * h).sqrt();
    let r = (f * f + g * g).sqrt();
    let sx = q + r;
    let sy = q - r;
    let a1 = g.atan2(f);
    let a2 = h.atan2(e);
    let t2 = (a2 - a1) / 2.0;
    let t1 = (a2 + a1) / 2.0;
    (m[4], m[5], t1.to_degrees(), sx, sy, t2.to_degrees(), 0.0)
}

/// Per-layer track index.
pub struct Chans<'a> {
    pub by: HashMap<&'a str, Vec<&'a Track>>,
}
impl<'a> Chans<'a> {
    pub fn new(ir: &'a Ir) -> Self {
        let mut by: HashMap<&str, Vec<&Track>> = HashMap::new();
        for t in &ir.tracks {
            by.entry(t.layer.as_str()).or_default().push(t);
        }
        Chans { by }
    }
    pub fn track(&self, id: &str, ch: Ch) -> Option<&'a Track> {
        self.by.get(id).and_then(|v| v.iter().find(|t| t.ch == ch).copied())
    }
    pub fn has(&self, id: &str, ch: Ch) -> bool {
        self.track(id, ch).is_some()
    }
    pub fn val(&self, id: &str, ch: Ch, t: f64) -> f64 {
        self.track(id, ch).map(|tr| tr.eval(t).n()).unwrap_or(ch.default_value().n())
    }
    pub fn tv(&self, id: &str, ch: Ch, t: f64) -> TV {
        self.track(id, ch).map(|tr| tr.eval(t)).unwrap_or(ch.default_value())
    }
}

/// Is the layer (or its subtree) in a 3D context: perspective/preserve-3d, or z/rotx/roty.
pub fn is_3d(l: &Layer, ch: &Chans) -> bool {
    l.style.perspective.is_some() || l.style.preserve3d || [Ch::Z, Ch::RotX, Ch::RotY].iter().any(|c| ch.track(&l.id, *c).is_some_and(|t| !t.is_static() || t.eval(0.0).n() != 0.0))
}

/// The layer's own CSS transform about its origin, as a 4x4 in scene coordinates.
/// `rect`: its layout box at rest; `magic`: (dx, dy, sx, sy) delta of a magic move.
pub fn local_m4(l: &Layer, rect: [f64; 4], ch: &Chans, t: f64, magic: Option<(f64, f64, f64, f64)>) -> M4 {
    let v = |c: Ch| ch.val(&l.id, c, t);
    let o = (rect[0] + l.style.origin.0 * rect[2], rect[1] + l.style.origin.1 * rect[3]);
    let mut m = m4_t(o.0, o.1, 0.0);
    // px/py: fractions of the element's own box (CSS translate(%))
    m = m4_mul(&m, &m4_t(v(Ch::Px) * rect[2], v(Ch::Py) * rect[3], 0.0));
    if let Some((dx, dy, sx, sy)) = magic {
        let k = 1.0 - v(Ch::Magic);
        m = m4_mul(&m, &m4_t(dx * k, dy * k, 0.0));
        m = m4_mul(&m, &m4_s(1.0 + (sx - 1.0) * k, 1.0 + (sy - 1.0) * k));
    }
    m = m4_mul(&m, &m4_t(v(Ch::X), v(Ch::Y), v(Ch::Z)));
    m = m4_mul(&m, &m4_rx(v(Ch::RotX)));
    m = m4_mul(&m, &m4_ry(v(Ch::RotY)));
    m = m4_mul(&m, &m4_rz(v(Ch::Rot)));
    m = m4_mul(&m, &m4_s(v(Ch::Sx), v(Ch::Sy)));
    let s = v(Ch::Scale);
    m = m4_mul(&m, &m4_s(s, s));
    m4_mul(&m, &m4_t(-o.0, -o.1, 0.0))
}

/// Perspective a layer applies to its children (about its box center), if any.
pub fn persp_m4(l: &Layer, rect: [f64; 4]) -> Option<M4> {
    let d = l.style.perspective?;
    let c = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    Some(m4_mul(&m4_mul(&m4_t(c.0, c.1, 0.0), &m4_persp(d)), &m4_t(-c.0, -c.1, 0.0)))
}

/// World (screen) affine of every layer at time t: 4x4 chain projected and linearized at the
/// layer's box center. `rects`: layout boxes (incl. layout-state offsets at t).
pub fn world(ir: &Ir, ch: &Chans, rects: &dyn Fn(&str) -> Option<[f64; 4]>, magic: &HashMap<String, (f64, f64, f64, f64)>, t: f64) -> HashMap<String, (A2, f64)> {
    let mut out = HashMap::new();
    fn rec(l: &Layer, parent: &M4, pop: f64, ch: &Chans, rects: &dyn Fn(&str) -> Option<[f64; 4]>, magic: &HashMap<String, (f64, f64, f64, f64)>, t: f64, out: &mut HashMap<String, (A2, f64)>) {
        let r = rects(&l.id).unwrap_or([0.0; 4]);
        let m = m4_mul(parent, &local_m4(l, r, ch, t, magic.get(&l.id).copied()));
        let op = pop * ch.val(&l.id, Ch::Opacity, t);
        out.insert(l.id.clone(), (linearize(&m, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0), op));
        let pm = match persp_m4(l, r) {
            Some(p) => m4_mul(&m, &p),
            None => m,
        };
        for c in &l.children {
            rec(c, &pm, op, ch, rects, magic, t, out);
        }
    }
    rec(&ir.root, &m4_id(), 1.0, ch, rects, magic, t, &mut out);
    out
}

/// Full (unlinearized) 4x4 chain of layer `target` at time t: (the parent's matrix including
/// the perspective it applies to its children, the target's own local matrix).
pub fn chain_m4(ir: &Ir, ch: &Chans, rects: &dyn Fn(&str) -> Option<[f64; 4]>, magic: &HashMap<String, (f64, f64, f64, f64)>, t: f64, target: &str) -> Option<(M4, M4)> {
    #[allow(clippy::too_many_arguments)]
    fn rec(l: &Layer, parent: &M4, ch: &Chans, rects: &dyn Fn(&str) -> Option<[f64; 4]>, magic: &HashMap<String, (f64, f64, f64, f64)>, t: f64, target: &str) -> Option<(M4, M4)> {
        let r = rects(&l.id).unwrap_or([0.0; 4]);
        let lm = local_m4(l, r, ch, t, magic.get(&l.id).copied());
        if l.id == target {
            return Some((*parent, lm));
        }
        let m = m4_mul(parent, &lm);
        let pm = match persp_m4(l, r) {
            Some(p) => m4_mul(&m, &p),
            None => m,
        };
        l.children.iter().find_map(|c| rec(c, &pm, ch, rects, magic, t, target))
    }
    rec(&ir.root, &m4_id(), ch, rects, magic, t, target)
}

/// Perspective `d` about screen point (cx, cy), as a 4x4.
pub fn persp_at(d: f64, cx: f64, cy: f64) -> M4 {
    m4_mul(&m4_mul(&m4_t(cx, cy, 0.0), &m4_persp(d)), &m4_t(-cx, -cy, 0.0))
}
