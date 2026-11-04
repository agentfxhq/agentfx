//! Path morphing (`PathContent::morph`): shape correspondence, computed once at load, and the
//! per-frame interpolation.
//!
//! Every shape is normalised to closed or open runs of cubic beziers (lines and quads are
//! elevated exactly, arcs come as cubics from the SVG parser). For each pair of consecutive
//! shapes:
//! - subpaths are paired greedily, largest first, by centroid distance and size; a subpath
//!   without a partner grows from (or shrinks to) a point at its own centroid;
//! - when the two shapes' largest closed subpaths wind in opposite directions, every subpath of
//!   the target is reversed (this does not change how it fills, nonzero or even-odd);
//! - the segment counts of paired subpaths are equalised by halving the longest segment (arc
//!   length) until they match;
//! - a closed pair takes the rotation of the target's start point that minimises the summed
//!   squared distance of all control points (the point travel of the morph).
//!
//! The result is a deterministic pure function of the two path strings.
use kurbo::{BezPath, CubicBez, ParamCurve, ParamCurveArclen, PathEl, Point};

#[derive(Clone, Debug)]
pub struct Sub {
    pub segs: Vec<CubicBez>,
    pub closed: bool,
}

/// Two shapes with identical structure: `a.len() == b.len()` and paired subpaths have the same
/// segment count.
#[derive(Clone, Debug)]
pub struct Pair {
    pub a: Vec<Sub>,
    pub b: Vec<Sub>,
}

/// A morph chain: shape k to shape k+1 for every k.
pub struct Morph {
    pub pairs: Vec<Pair>,
    /// Union of every control point of every shape (the interpolated curves stay inside it).
    pub hull: kurbo::Rect,
}

/// Parse a path into cubic subpaths (empty subpaths dropped).
pub fn subpaths(path: &BezPath) -> Vec<Sub> {
    let mut out: Vec<Sub> = vec![];
    let mut cur: Vec<CubicBez> = vec![];
    let (mut start, mut pen) = (Point::ZERO, Point::ZERO);
    let flush = |cur: &mut Vec<CubicBez>, closed: bool, out: &mut Vec<Sub>| {
        if !cur.is_empty() {
            out.push(Sub { segs: std::mem::take(cur), closed });
        }
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                flush(&mut cur, false, &mut out);
                start = p;
                pen = p;
            }
            PathEl::LineTo(p) => {
                cur.push(line(pen, p));
                pen = p;
            }
            PathEl::QuadTo(c, p) => {
                cur.push(CubicBez::new(pen, pen + (c - pen) * (2.0 / 3.0), p + (c - p) * (2.0 / 3.0), p));
                pen = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                cur.push(CubicBez::new(pen, c1, c2, p));
                pen = p;
            }
            PathEl::ClosePath => {
                if pen.distance(start) > 1e-9 || cur.is_empty() {
                    cur.push(line(pen, start));
                }
                flush(&mut cur, true, &mut out);
                pen = start;
            }
        }
    }
    flush(&mut cur, false, &mut out);
    out
}

fn line(a: Point, b: Point) -> CubicBez {
    CubicBez::new(a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b)
}

fn pts(s: &Sub) -> impl Iterator<Item = Point> + '_ {
    s.segs.iter().flat_map(|c| [c.p0, c.p1, c.p2])
}

fn centroid(s: &Sub) -> Point {
    let n = s.segs.len().max(1) as f64 * 3.0;
    let (x, y) = pts(s).fold((0.0, 0.0), |a, p| (a.0 + p.x, a.1 + p.y));
    Point::new(x / n, y / n)
}

/// Signed area (shoelace over the control polygon; positive = clockwise in y-down space).
fn area(s: &Sub) -> f64 {
    let p: Vec<Point> = pts(s).collect();
    let n = p.len();
    (0..n).map(|i| p[i].x * p[(i + 1) % n].y - p[(i + 1) % n].x * p[i].y).sum::<f64>() / 2.0
}

fn size(s: &Sub) -> f64 {
    let r = bbox(s);
    r.width().hypot(r.height())
}

fn bbox(s: &Sub) -> kurbo::Rect {
    let mut r = kurbo::Rect::from_points(s.segs[0].p0, s.segs[0].p0);
    for p in pts(s).chain(s.segs.last().map(|c| c.p3)) {
        r = r.union_pt(p);
    }
    r
}

fn reverse(s: &Sub) -> Sub {
    Sub { segs: s.segs.iter().rev().map(|c| CubicBez::new(c.p3, c.p2, c.p1, c.p0)).collect(), closed: s.closed }
}

/// A subpath collapsed to point `p` with `n` segments (closed like `like`).
fn point_sub(p: Point, n: usize, closed: bool) -> Sub {
    Sub { segs: vec![CubicBez::new(p, p, p, p); n.max(1)], closed }
}

/// Halve the longest segment (arc length) until the subpath has `n` segments.
fn split_to(s: &mut Sub, n: usize) {
    let mut lens: Vec<f64> = s.segs.iter().map(|c| c.arclen(1e-3)).collect();
    while s.segs.len() < n {
        // first index of the maximum: deterministic
        let mut k = 0;
        for i in 1..lens.len() {
            if lens[i] > lens[k] + 1e-9 {
                k = i;
            }
        }
        let c = s.segs[k];
        let t = if lens[k] > 1e-9 { c.inv_arclen(lens[k] / 2.0, 1e-4) } else { 0.5 };
        let (l, r) = (c.subsegment(0.0..t), c.subsegment(t..1.0));
        s.segs[k] = l;
        s.segs.insert(k + 1, r);
        let half = lens[k] / 2.0;
        lens[k] = half;
        lens.insert(k + 1, half);
    }
}

fn rotate(s: &Sub, r: usize) -> Sub {
    let n = s.segs.len();
    Sub { segs: (0..n).map(|i| s.segs[(i + r) % n]).collect(), closed: s.closed }
}

fn cost(a: &Sub, b: &Sub, r: usize) -> f64 {
    let n = a.segs.len();
    (0..n)
        .map(|i| {
            let (x, y) = (a.segs[i], b.segs[(i + r) % n]);
            (x.p0 - y.p0).hypot2() + (x.p1 - y.p1).hypot2() + (x.p2 - y.p2).hypot2()
        })
        .sum()
}

/// Make two subpaths structurally identical.
fn match_sub(a: &Sub, b: &Sub) -> (Sub, Sub) {
    let (mut a, mut b) = (a.clone(), b.clone());
    let n = a.segs.len().max(b.segs.len());
    split_to(&mut a, n);
    split_to(&mut b, n);
    if a.closed && b.closed {
        let best = (0..n).min_by(|&i, &j| cost(&a, &b, i).total_cmp(&cost(&a, &b, j))).unwrap_or(0);
        b = rotate(&b, best);
    }
    (a, b)
}

/// Correspondence between two shapes.
pub fn pair(a: &[Sub], b: &[Sub]) -> Pair {
    let mut b: Vec<Sub> = b.to_vec();
    // global direction: keep fills identical, align the dominant windings
    let dom = |v: &[Sub]| v.iter().filter(|s| s.closed).max_by(|x, y| area(x).abs().total_cmp(&area(y).abs())).map(area);
    if let (Some(x), Some(y)) = (dom(a), dom(&b)) {
        if x * y < 0.0 {
            b = b.iter().map(|s| if s.closed { reverse(s) } else { s.clone() }).collect();
        }
    }
    // pair subpaths, largest first
    let mut order: Vec<usize> = (0..a.len()).collect();
    order.sort_by(|&i, &j| size(&a[j]).total_cmp(&size(&a[i])).then(i.cmp(&j)));
    let mut used = vec![false; b.len()];
    let mut partner = vec![None; a.len()];
    for &i in &order {
        let (ca, sa) = (centroid(&a[i]), size(&a[i]));
        let best = (0..b.len())
            .filter(|&j| !used[j])
            .min_by(|&j, &k| {
                let f = |j: usize| centroid(&b[j]).distance(ca) + (size(&b[j]) - sa).abs();
                f(j).total_cmp(&f(k)).then(j.cmp(&k))
            });
        if let Some(j) = best {
            used[j] = true;
            partner[i] = Some(j);
        }
    }
    let (mut pa, mut pb) = (vec![], vec![]);
    for i in 0..a.len() {
        match partner[i] {
            Some(j) => {
                let (x, y) = match_sub(&a[i], &b[j]);
                pa.push(x);
                pb.push(y);
            }
            None => {
                pb.push(point_sub(centroid(&a[i]), a[i].segs.len(), a[i].closed));
                pa.push(a[i].clone());
            }
        }
    }
    for j in (0..b.len()).filter(|&j| !used[j]) {
        pa.push(point_sub(centroid(&b[j]), b[j].segs.len(), b[j].closed));
        pb.push(b[j].clone());
    }
    Pair { a: pa, b: pb }
}

impl Morph {
    pub fn new(shapes: &[BezPath]) -> Morph {
        let subs: Vec<Vec<Sub>> = shapes.iter().map(subpaths).collect();
        let mut hull: Option<kurbo::Rect> = None;
        for s in subs.iter().flatten() {
            let r = bbox(s);
            hull = Some(hull.map_or(r, |h| h.union(r)));
        }
        let pairs = subs.windows(2).map(|w| pair(&w[0], &w[1])).collect();
        Morph { pairs, hull: hull.unwrap_or(kurbo::Rect::ZERO) }
    }

    /// The shape at morph position `v` (0 = first shape, k = shape k; clamped to the chain).
    pub fn at(&self, v: f64) -> BezPath {
        let last = self.pairs.len() as f64;
        let v = if v.is_finite() { v.clamp(0.0, last) } else { 0.0 };
        let k = (v.floor() as usize).min(self.pairs.len().saturating_sub(1));
        let f = v - k as f64;
        let p = &self.pairs[k];
        let mut out = BezPath::new();
        for (x, y) in p.a.iter().zip(&p.b) {
            let l = |a: Point, b: Point| a.lerp(b, f);
            let first = l(x.segs[0].p0, y.segs[0].p0);
            out.move_to(first);
            for (c, d) in x.segs.iter().zip(&y.segs) {
                out.curve_to(l(c.p1, d.p1), l(c.p2, d.p2), l(c.p3, d.p3));
            }
            // an open/closed pair is closed exactly at the closed end (a real join, no cap stub)
            let closed = if f >= 1.0 - 1e-9 {
                y.closed
            } else if f <= 1e-9 {
                x.closed
            } else {
                x.closed && y.closed
            };
            if closed {
                out.close_path();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use kurbo::Shape;

    fn sh(d: &str) -> Vec<Sub> {
        subpaths(&BezPath::from_svg(d).unwrap())
    }

    #[test]
    fn structures_match_and_ends_are_exact() {
        // square (4 lines) to a circle-ish (arcs) to a triangle
        let shapes = ["M0 0 H100 V100 H0 Z", "M50 0 A50 50 0 1 1 50 100 A50 50 0 1 1 50 0 Z", "M50 0 L100 100 L0 100 Z"].map(|d| BezPath::from_svg(d).unwrap());
        let m = Morph::new(&shapes);
        assert_eq!(m.pairs.len(), 2);
        for p in &m.pairs {
            assert_eq!(p.a.len(), p.b.len());
            for (x, y) in p.a.iter().zip(&p.b) {
                assert_eq!(x.segs.len(), y.segs.len());
            }
        }
        // the ends reproduce the shapes (as point sets of the control polygon start points)
        let bb = |p: &BezPath| p.bounding_box();
        for (v, s) in [(0.0, &shapes[0]), (1.0, &shapes[1]), (2.0, &shapes[2])] {
            let (r, e) = (bb(&m.at(v)), bb(s));
            assert!((r.x0 - e.x0).abs() < 1e-6 && (r.y1 - e.y1).abs() < 1e-6 && (r.x1 - e.x1).abs() < 1e-6, "{v}: {r:?} vs {e:?}");
        }
        // clamped outside the chain
        assert_eq!(m.at(-1.0), m.at(0.0));
        assert_eq!(m.at(9.0), m.at(2.0));
    }

    #[test]
    fn rotation_minimises_travel() {
        // the same square, starting at a different corner: the morph must not move at all
        let a = sh("M0 0 H100 V100 H0 Z");
        let b = sh("M100 100 H0 V0 H100 Z");
        let p = pair(&a, &b);
        let travel: f64 = p.a[0].segs.iter().zip(&p.b[0].segs).map(|(x, y)| x.p0.distance(y.p0)).sum();
        assert!(travel < 1e-6, "travel {travel}");
    }

    #[test]
    fn opposite_winding_is_aligned_without_changing_the_fill() {
        // same square, drawn counter-clockwise: reversed, then rotated, so no travel
        let a = sh("M0 0 H100 V100 H0 Z");
        let b = sh("M0 0 V100 H100 V0 Z");
        let p = pair(&a, &b);
        assert!(area(&p.a[0]) * area(&p.b[0]) > 0.0);
        let travel: f64 = p.a[0].segs.iter().zip(&p.b[0].segs).map(|(x, y)| x.p0.distance(y.p0)).sum();
        assert!(travel < 1e-6, "travel {travel}");
    }

    #[test]
    fn extra_subpaths_grow_from_a_point() {
        // one ring to a ring with a hole: the hole grows from its centroid
        let a = sh("M0 0 H100 V100 H0 Z");
        let b = sh("M0 0 H100 V100 H0 Z M40 40 V60 H60 V40 Z");
        let p = pair(&a, &b);
        assert_eq!(p.a.len(), 2);
        let grow = &p.a[1];
        assert!(grow.segs.iter().all(|c| c.p0.distance(Point::new(50.0, 50.0)) < 1e-6));
        // and back: the hole shrinks into the point
        let q = pair(&b, &a);
        assert!(q.b[1].segs.iter().all(|c| c.p0.distance(Point::new(50.0, 50.0)) < 1e-6));
    }

    #[test]
    fn splitting_halves_longest_segments() {
        let mut s = sh("M0 0 L300 0 L300 10")[0].clone();
        split_to(&mut s, 4);
        assert_eq!(s.segs.len(), 4);
        // the 300 px line got split twice (into 75 px pieces would need 3 splits; 2 splits give 150, 75, 75)
        let lens: Vec<f64> = s.segs.iter().map(|c| c.arclen(1e-6)).collect();
        assert!((lens.iter().sum::<f64>() - 310.0).abs() < 1e-3);
        assert!(lens.iter().all(|l| *l <= 150.0 + 1e-6), "{lens:?}");
    }

    #[test]
    fn deterministic() {
        let shapes = ["M10 10 C40 0 60 80 90 10", "M0 50 Q50 -20 100 50 T200 50"].map(|d| BezPath::from_svg(d).unwrap());
        let a = Morph::new(&shapes).at(0.37);
        let b = Morph::new(&shapes).at(0.37);
        assert_eq!(a, b);
    }
}
