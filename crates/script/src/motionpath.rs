//! Motion paths (`move #x along="M..." orient`) and path morph targets.
//!
//! A motion path is baked into x / y (and, with `orient`, rot) keys at compile time. The ease
//! applies to the distance travelled: progress p = ease(u) maps to the point at arc length
//! p * L. Keys are placed by adaptive subdivision of the time span until linear interpolation
//! between neighbouring keys stays within 0.05 px of the true point and 0.1 degrees of the
//! true tangent angle, so the baked track is exact at any frame rate or motion-blur sample.
use crate::ir::MorphShape;
use crate::values::Ease;
use kurbo::{BezPath, CubicBez, ParamCurve, ParamCurveArclen, ParamCurveDeriv, PathSeg, Point, Vec2};

const ACC: f64 = 1e-4;
/// Position tolerance of the baked keys, px.
pub const POS_TOL: f64 = 0.05;
/// Tangent tolerance of the baked keys, degrees.
pub const ANGLE_TOL: f64 = 0.1;
const MAX_DEPTH: u32 = 14;
/// Shortest key spacing, seconds (the compiler merges keys closer than 0.5 ms).
const MIN_DT: f64 = 2e-3;

/// Parse one `morph=` item / `morph to=` value: `#name` (another path element) or path data.
pub fn morph_shape(raw: &str) -> Result<MorphShape, (String, String)> {
    let raw = raw.trim();
    if let Some(name) = raw.strip_prefix('#') {
        if name.is_empty() {
            return Err(("empty id".into(), "write `#star` (a path element) or path data like \"M0 0 L100 0 Z\"".into()));
        }
        return Ok(MorphShape::Layer(name.to_string()));
    }
    parse(raw)?;
    Ok(MorphShape::D(raw.to_string()))
}

/// Parse SVG path data that must contain at least one drawable segment.
pub fn parse(d: &str) -> Result<BezPath, (String, String)> {
    let hint = || "write SVG path data like \"M0 0 C50 0 50 100 100 100\" (M L H V C S Q T A Z)".to_string();
    let p = BezPath::from_svg(d).map_err(|e| (format!("bad path data `{}`: {e:?}", short(d)), hint()))?;
    if p.segments().next().is_none() {
        return Err((format!("path `{}` has no segments", short(d)), hint()));
    }
    Ok(p)
}

fn short(d: &str) -> String {
    if d.chars().count() > 40 { format!("{}...", d.chars().take(40).collect::<String>()) } else { d.to_string() }
}

/// A path measured by arc length.
pub struct Walk {
    segs: Vec<(CubicBez, f64)>,
    pub total: f64,
}

impl Walk {
    pub fn new(p: &BezPath) -> Walk {
        let segs: Vec<(CubicBez, f64)> = p
            .segments()
            .map(|s| {
                let c = match s {
                    PathSeg::Line(l) => CubicBez::new(l.p0, l.p0.lerp(l.p1, 1.0 / 3.0), l.p0.lerp(l.p1, 2.0 / 3.0), l.p1),
                    PathSeg::Quad(q) => q.raise(),
                    PathSeg::Cubic(c) => c,
                };
                (c, c.arclen(ACC))
            })
            .collect();
        let total = segs.iter().map(|s| s.1).sum();
        Walk { segs, total }
    }
    pub fn start(&self) -> Point {
        self.segs.first().map(|s| s.0.p0).unwrap_or(Point::ZERO)
    }
    /// Point and unit tangent at arc length `s` (clamped to the path).
    pub fn at(&self, s: f64) -> (Point, Vec2) {
        let mut s = s.clamp(0.0, self.total);
        let n = self.segs.len();
        for (i, (c, len)) in self.segs.iter().enumerate() {
            if s <= *len || i + 1 == n {
                let t = if *len > 0.0 { c.inv_arclen(s.min(*len), ACC) } else { 0.0 };
                return (c.eval(t), tangent(c, t));
            }
            s -= len;
        }
        (Point::ZERO, Vec2::new(1.0, 0.0))
    }
}

/// Unit tangent; at a degenerate end (control point on the end point) it looks a little inward.
fn tangent(c: &CubicBez, t: f64) -> Vec2 {
    let d = c.deriv();
    for probe in [t, t + 1e-4, t - 1e-4, 0.5] {
        let v = d.eval(probe.clamp(0.0, 1.0)).to_vec2();
        if v.hypot() > 1e-9 {
            return v / v.hypot();
        }
    }
    let v = c.p3 - c.p0;
    if v.hypot() > 1e-12 { v / v.hypot() } else { Vec2::new(1.0, 0.0) }
}

/// One baked sample: time, point, tangent angle (degrees, unwrapped).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t: f64,
    pub p: Point,
    pub angle: f64,
}

/// Samples of a motion along `walk` from t0 to t1 with `ease` on the distance. Angles are
/// unwrapped (continuous through ±180).
pub fn bake(walk: &Walk, t0: f64, t1: f64, ease: &Ease) -> Vec<Sample> {
    let d = (t1 - t0).max(1e-3);
    let eval = |u: f64| {
        let (p, tg) = walk.at(ease.sample(u) * walk.total);
        (p, tg.y.atan2(tg.x).to_degrees())
    };
    let mut us = vec![0.0];
    let a = eval(0.0);
    let b = eval(1.0);
    subdivide(&eval, (0.0, a), (1.0, b), 0, MIN_DT / d, &mut us);
    let mut out: Vec<Sample> = Vec::with_capacity(us.len());
    for u in us {
        let (p, raw) = eval(u);
        let angle = match out.last() {
            Some(prev) => prev.angle + wrap(raw - prev.angle),
            None => raw,
        };
        out.push(Sample { t: t0 + u * d, p, angle });
    }
    out
}

fn wrap(a: f64) -> f64 {
    let mut a = a % 360.0;
    if a > 180.0 {
        a -= 360.0;
    } else if a < -180.0 {
        a += 360.0;
    }
    a
}

/// Push the parameters of (lo, hi], refined where linear interpolation is off.
fn subdivide(eval: &dyn Fn(f64) -> (Point, f64), lo: (f64, (Point, f64)), hi: (f64, (Point, f64)), depth: u32, min_span: f64, out: &mut Vec<f64>) {
    let span = hi.0 - lo.0;
    // test a few interior points, not only the midpoint (an S curve can cross its chord there)
    let mut bad = false;
    if depth < MAX_DEPTH && span / 2.0 >= min_span {
        for f in [0.25, 0.5, 0.75] {
            let (p, ang) = eval(lo.0 + span * f);
            let lin = lo.1 .0.lerp(hi.1 .0, f);
            let lin_a = lo.1 .1 + wrap(hi.1 .1 - lo.1 .1) * f;
            if p.distance(lin) > POS_TOL || wrap(ang - lin_a).abs() > ANGLE_TOL {
                bad = true;
                break;
            }
        }
        // never skip a whole corner: a span covering a long distance gets split at least to 1/64
        bad |= depth < 6;
    }
    if bad {
        let m = lo.0 + span / 2.0;
        let mid = (m, eval(m));
        subdivide(eval, lo, mid, depth + 1, min_span, out);
        subdivide(eval, mid, hi, depth + 1, min_span, out);
    } else {
        out.push(hi.0);
    }
}

// ------------------------------------------------------------ verbs
use crate::ir::{Ch, Kind, TV};
use crate::lower::{flag, id_arg, Ctx, Ev};

impl Ctx<'_> {
    /// `morph #p to="M..."|#other [dur=] [ease=]`: append a morph target (or reuse the same one)
    /// and animate the `morph` channel to it.
    pub(crate) fn morph_verb(&mut self, i: usize, e: &Ev) {
        let n = &e.node;
        if !matches!(self.layers[i].kind, Kind::Path { .. }) {
            self.err("E_MORPH", "morph needs a path", "morph works on `path`, `line`, `poly` and `star`; draw the shape as `path #p d=\"...\"`", n.span);
            return;
        }
        let raw = n.props.get("to").map(|v| v.raw()).or_else(|| n.args.iter().filter_map(|a| a.as_str()).find(|s| !s.starts_with('#')).map(str::to_string));
        let Some(raw) = raw else {
            self.err("E_MORPH", "morph needs a target shape", "write `morph #p to=#star dur=0.8s` or `morph #p to=\"M0 0 L100 0 L50 80 Z\"`", n.span);
            return;
        };
        let shape = match morph_shape(&raw) {
            Ok(s) => s,
            Err((msg, hint)) => {
                self.err("E_MORPH", format!("morph to=: {msg}"), hint, n.span);
                return;
            }
        };
        if let MorphShape::Layer(name) = &shape {
            match self.names.get(&format!("{name}@{}", e.scene)).map(|nm| nm.idx) {
                Some(j) if matches!(self.layers[j].kind, Kind::Path { .. }) => {}
                Some(_) => {
                    self.err("E_MORPH", format!("morph to=#{name}: not a path"), "the target must be a `path`, `line`, `poly` or `star` element", n.span);
                    return;
                }
                None => {
                    self.err("E_ID", format!("morph to=#{name}: no element `#{name}` in this scene"), "declare the target shape as a path in the same scene (hide it with `opacity=0`)", n.span);
                    return;
                }
            }
        }
        let k = match self.layers[i].style.morph.iter().position(|m| *m == shape) {
            Some(p) => p + 1,
            None => {
                self.layers[i].style.morph.push(shape);
                self.layers[i].style.morph.len()
            }
        };
        let d = self.dur(n, "dur").unwrap_or(self.th.durs.get("move-dur").copied().unwrap_or(self.th.dur("move")));
        let mv = self.th.ease("move");
        let ease = self.ease(n, "ease", mv);
        self.track(i, Ch::Morph).to(e.t, e.t + d, TV::N(k as f64), ease);
    }

    /// `move #x along="M..."|#path [orient[=deg]] dur= ease=`: bake the motion path into x / y
    /// (and rot) keys. Path data is relative (its start is where the element is when the move
    /// starts); `#path` is another path element's rest geometry in scene space, followed by the
    /// element's center.
    pub(crate) fn move_along(&mut self, i: usize, e: &Ev, d: f64, ease: Ease) {
        let n = &e.node;
        let raw = n.props.get("along").or_else(|| n.props.get("path")).map(|v| v.raw()).unwrap_or_default();
        let (t, si) = (e.t, e.scene);
        let (path, ox, oy) = if let Some(name) = raw.trim().strip_prefix('#') {
            let me = id_arg(n).or_else(|| e.target.clone()).unwrap_or_default();
            let Some(j) = self.names.get(&format!("{name}@{si}")).map(|nm| nm.idx) else {
                self.err("E_ID", format!("move along=#{name}: no element `#{name}` in this scene"), "declare the guide path in the same scene (hide it with `opacity=0`)", n.span);
                return;
            };
            let Kind::Path { d: pd, vb, .. } = self.layers[j].kind.clone() else {
                self.err("E_PATH", format!("move along=#{name}: not a path"), "the guide must be a `path`, `line`, `poly` or `star` element", n.span);
                return;
            };
            let Some(g) = &self.geo else {
                self.need_geo = true;
                return;
            };
            let (Some(a), Some(b)) = (g.rects.get(&format!("{me}@{si}")).copied(), g.rects.get(&format!("{name}@{si}")).copied()) else {
                self.err("E_ID", format!("move along=#{name}: `#{me}` or `#{name}` is not laid out"), "use visible elements of the same scene", n.span);
                return;
            };
            let sx = if vb.0 > 0.0 { b[2] / vb.0 } else { 1.0 };
            let sy = if vb.1 > 0.0 { b[3] / vb.1 } else { 1.0 };
            match parse(&crate::reelir::map_path(&pd, b[0], b[1], sx, sy)) {
                Ok(p) => (p, -(a[0] + a[2] / 2.0), -(a[1] + a[3] / 2.0)),
                Err((msg, hint)) => {
                    self.err("E_PATH", format!("move along=#{name}: {msg}"), hint, n.span);
                    return;
                }
            }
        } else {
            match parse(&raw) {
                Ok(p) => {
                    let w0 = Walk::new(&p).start();
                    let (x0, y0) = (self.track(i, Ch::X).eval(t).n(), self.track(i, Ch::Y).eval(t).n());
                    (p, x0 - w0.x, y0 - w0.y)
                }
                Err((msg, hint)) => {
                    self.err("E_PATH", format!("move along=: {msg}"), hint, n.span);
                    return;
                }
            }
        };
        let walk = Walk::new(&path);
        if walk.total <= 1e-9 {
            self.warn("W_MOTION_PATH", "move along=: the path has zero length", "give the path some extent, or use `move #x x= y=`", n.span);
        }
        let orient = if !flag(n, "orient") {
            None
        } else {
            match n.props.get("orient").map(|v| v.raw()) {
                None => Some(0.0),
                Some(r) if r == "true" || r.is_empty() => Some(0.0),
                Some(_) => Some(self.num(n, "orient").unwrap_or(0.0)),
            }
        };
        let samples = bake(&walk, t, t + d, &ease);
        // `Track::bake` interpolates from the last earlier key to the first sample: pin the
        // current value just before the move so nothing changes until it starts
        let pinned = |c: &mut Ctx, ch: Ch, keys: Vec<(f64, TV)>| {
            let mut ks = vec![];
            if t > 2e-3 {
                ks.push((t - 1e-3, c.track(i, ch).eval(t - 1e-3)));
            }
            ks.extend(keys);
            let t0 = ks[0].0;
            c.track(i, ch).bake(t0, &ks);
        };
        pinned(self, Ch::X, samples.iter().map(|s| (s.t, TV::N(s.p.x + ox))).collect());
        pinned(self, Ch::Y, samples.iter().map(|s| (s.t, TV::N(s.p.y + oy))).collect());
        if let Some(off) = orient {
            // keep the current turn count (no spin back into -180..180)
            let cur = self.track(i, Ch::Rot).eval(t).n();
            let a0 = samples.first().map(|s| s.angle + off).unwrap_or(0.0);
            let shift = ((cur - a0) / 360.0).round() * 360.0;
            pinned(self, Ch::Rot, samples.iter().map(|s| (s.t, TV::N(s.angle + off + shift))).collect());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_length_is_uniform() {
        // a line made of a short and a long segment: half the length is at x=50
        let w = Walk::new(&BezPath::from_svg("M0 0 L10 0 L100 0").unwrap());
        assert!((w.total - 100.0).abs() < 1e-6);
        let (p, tg) = w.at(50.0);
        assert!((p.x - 50.0).abs() < 1e-3 && p.y.abs() < 1e-9, "{p:?}");
        assert!((tg.x - 1.0).abs() < 1e-9);
        // a curve: the point at s really lies at arc length s
        let c = BezPath::from_svg("M0 0 C0 100 100 100 100 0").unwrap();
        let w = Walk::new(&c);
        let (p, _) = w.at(w.total * 0.3);
        let seg = c.segments().next().unwrap();
        let t = seg.inv_arclen(w.total * 0.3, 1e-6);
        assert!(p.distance(seg.eval(t)) < 1e-3);
    }

    #[test]
    fn bake_is_within_tolerance_everywhere() {
        let path = BezPath::from_svg("M0 0 C200 -300 400 300 600 0 S900 -200 1000 100").unwrap();
        let w = Walk::new(&path);
        let ease = Ease::Cubic(0.65, 0.0, 0.35, 1.0);
        let keys = bake(&w, 1.0, 3.0, &ease);
        assert_eq!(keys.first().unwrap().t, 1.0);
        assert!((keys.last().unwrap().t - 3.0).abs() < 1e-9);
        assert!(keys.len() < 2000, "{}", keys.len());
        // between keys, linear interpolation vs the truth, at many times
        for i in 0..=2000 {
            let t = 1.0 + 2.0 * i as f64 / 2000.0;
            let k = keys.windows(2).find(|k| t >= k[0].t && t <= k[1].t).unwrap();
            let f = (t - k[0].t) / (k[1].t - k[0].t);
            let lin = k[0].p.lerp(k[1].p, f);
            let (truth, _) = w.at(ease.sample((t - 1.0) / 2.0) * w.total);
            assert!(lin.distance(truth) < 0.2, "t {t}: {}", lin.distance(truth));
        }
    }

    #[test]
    fn angles_unwrap_on_a_circle() {
        // two full turns of a circle (arcs): the angle grows by about 720 degrees without jumps
        let path = BezPath::from_svg("M100 0 A100 100 0 1 1 -100 0 A100 100 0 1 1 100 0 A100 100 0 1 1 -100 0 A100 100 0 1 1 100 0").unwrap();
        let keys = bake(&Walk::new(&path), 0.0, 4.0, &Ease::Linear);
        for k in keys.windows(2) {
            assert!((k[1].angle - k[0].angle).abs() < 30.0);
        }
        let turn = keys.last().unwrap().angle - keys[0].angle;
        assert!((turn.abs() - 720.0).abs() < 1.0, "{turn}");
    }

    #[test]
    fn morph_items() {
        assert_eq!(morph_shape("#star").unwrap(), MorphShape::Layer("star".into()));
        assert_eq!(morph_shape("M0 0 L10 0").unwrap(), MorphShape::D("M0 0 L10 0".into()));
        assert!(morph_shape("M0 0").is_err());
        assert!(morph_shape("Mx").is_err());
    }
}
