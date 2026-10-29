//! Path helpers: arc-length tables and trimming (draw-on) for `Content::Path`.
use kurbo::{BezPath, ParamCurve, ParamCurveArclen, PathSeg};

const ACC: f64 = 1e-3;

pub struct Measured {
    pub path: BezPath,
    segs: Vec<(PathSeg, f64, bool)>, // segment, arc length, starts a new subpath
    pub total: f64,
}

pub fn measure(path: BezPath) -> Measured {
    let mut segs = Vec::new();
    let mut new_sub = true;
    let mut total = 0.0;
    let mut last_move = None;
    for el in path.elements() {
        if let kurbo::PathEl::MoveTo(p) = el {
            new_sub = true;
            last_move = Some(*p);
        }
        let _ = last_move;
    }
    // walk segments, flagging subpath starts by comparing with previous end
    let mut prev_end: Option<kurbo::Point> = None;
    for seg in path.segments() {
        let start = seg.start();
        let starts = prev_end.is_none_or(|e| e.distance(start) > 1e-9);
        let len = seg.arclen(ACC);
        total += len;
        segs.push((seg, len, starts || new_sub));
        new_sub = false;
        prev_end = Some(seg.end());
    }
    Measured { path, segs, total }
}

impl Measured {
    /// The part of the path between fractions `a` and `b` of the total length.
    pub fn trim(&self, a: f64, b: f64) -> BezPath {
        let (a, b) = (a.clamp(0.0, 1.0) * self.total, b.clamp(0.0, 1.0) * self.total);
        let mut out = BezPath::new();
        if b <= a {
            return out;
        }
        let mut pos = 0.0;
        let mut pen: Option<kurbo::Point> = None;
        for (seg, len, starts) in &self.segs {
            let (s0, s1) = (pos, pos + len);
            pos = s1;
            if s1 <= a || s0 >= b || *len <= 0.0 {
                if *starts {
                    pen = None;
                }
                continue;
            }
            let t0 = if a > s0 { seg.inv_arclen(a - s0, ACC) } else { 0.0 };
            let t1 = if b < s1 { seg.inv_arclen(b - s0, ACC) } else { 1.0 };
            let part = seg.subsegment(t0..t1);
            let st = part.start();
            if *starts || pen.is_none_or(|p| p.distance(st) > 1e-6) {
                out.move_to(st);
            }
            match part {
                PathSeg::Line(l) => out.line_to(l.p1),
                PathSeg::Quad(q) => out.quad_to(q.p1, q.p2),
                PathSeg::Cubic(c) => out.curve_to(c.p1, c.p2, c.p3),
            }
            pen = Some(part.end());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trims_by_length() {
        let p = BezPath::from_svg("M0 0 L100 0 L100 100").unwrap();
        let m = measure(p);
        assert!((m.total - 200.0).abs() < 1e-6);
        let half = m.trim(0.0, 0.5);
        assert_eq!(half.segments().count(), 1);
        let q = m.trim(0.25, 0.75);
        let end = q.segments().last().unwrap().end();
        assert!((end.x - 100.0).abs() < 1e-6 && (end.y - 50.0).abs() < 1e-3, "{end:?}");
    }
}
