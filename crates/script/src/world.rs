//! World canvas + camera director (X9).
//!
//! - `world #w 16000x9000` lays out boards on one large canvas (world px = scene px before the
//!   camera); `place #p at 7000,4000 [scale=] [rot=] [w=1920 h=1080]` is a board centered at
//!   (x, y) of its parent, laid out like a scene (column flow, centered). Places nest, so a
//!   board at `scale=0.01` inside another gives "powers of ten" zooms.
//! - `camera tour|zoom-through|follow|focus` move the scene camera (the scene's world layer:
//!   scale about the frame center + translation) along smooth zoom-pan paths (van Wijk & Nuij
//!   2003: optimal in perceived motion, exact at any zoom ratio), baked into per-frame keys at
//!   full f64 precision (the layer is tagged `cam-precise`).
//! - The renderer draws the world's static part from a mip tile cache while the camera moves
//!   (reel-ir node `world`, reel `IR.md`).
use crate::ast::{Node, Span, Value};
use crate::ir::*;
use crate::lower::*;
use crate::values::*;

pub const DIRECTOR: &[&str] = &["tour", "zoom-through", "follow", "focus", "loop"];
const PROPS_TOUR: &[&str] = &["ease", "dwell", "move", "fit", "rho", "start"];
const PROPS_THROUGH: &[&str] = &["ease", "dur", "fit", "rho", "start"];
const PROPS_FOLLOW: &[&str] = &["fit", "lead", "for", "until", "smooth", "zoom"];
const PROPS_LOOP: &[&str] = &["dur", "levels", "dir", "zoom", "wrap", "scale", "rot"];
const PROPS_FOCUS: &[&str] = &["overlay", "pad", "for", "dur", "fit", "move", "ease", "rho", "radius"];
/// Zoom faster than this (octaves per second) is hard to read.
pub const FAST_OCT: f64 = 5.0;
/// Pan faster than this (frame widths per second, measured at the frame center) is hard to read.
pub const FAST_PAN: f64 = 2.5;
/// A framed target smaller than this (px) at the world's max zoom cannot be framed.
pub const MIN_TARGET_PX: f64 = 24.0;

/// A world canvas of a scene.
#[derive(Debug, Clone)]
pub struct WorldL {
    pub scene: usize,
    pub idx: usize,
    pub max_zoom: f64,
}

/// Camera view: center (world px) and visible world width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
}

/// Van Wijk & Nuij smooth zoom-pan path between two views (`rho` trades zoom for pan;
/// sqrt(2) is their recommended value). `at(s)` for s in [0, len]; s beyond len extrapolates
/// (spring overshoot keeps moving along the same curve).
#[derive(Debug, Clone)]
pub struct ZoomPath {
    a: View,
    b: View,
    rho: f64,
    d: f64,
    r0: f64,
    pub len: f64,
}

impl ZoomPath {
    pub fn new(a: View, b: View, rho: f64) -> ZoomPath {
        let d = ((b.cx - a.cx).powi(2) + (b.cy - a.cy).powi(2)).sqrt();
        let (w0, w1) = (a.w, b.w);
        let rho2 = rho * rho;
        if d <= 1e-9 * w0.min(w1) {
            return ZoomPath { a, b, rho, d: 0.0, r0: 0.0, len: (w1 / w0).ln().abs() / rho };
        }
        let bi = |w: f64, sign: f64| (w1 * w1 - w0 * w0 + sign * rho2 * rho2 * d * d) / (2.0 * w * rho2 * d);
        // ln(-b + sqrt(b^2 + 1)) = -asinh(b), without cancellation for large b
        let r0 = -bi(w0, 1.0).asinh();
        let r1 = -bi(w1, -1.0).asinh();
        ZoomPath { a, b, rho, d, r0, len: (r1 - r0) / rho }
    }

    pub fn at(&self, s: f64) -> View {
        let (a, b) = (self.a, self.b);
        if self.d == 0.0 {
            if self.len <= 0.0 {
                return a;
            }
            let k = if b.w < a.w { -1.0 } else { 1.0 };
            return View { cx: a.cx + (b.cx - a.cx) * (s / self.len), cy: a.cy + (b.cy - a.cy) * (s / self.len), w: a.w * (k * self.rho * s).exp() };
        }
        if s == self.len {
            return b;
        }
        let rho = self.rho;
        let r0 = self.r0;
        let u = a.w / (rho * rho) * (r0.cosh() * (rho * s + r0).tanh() - r0.sinh());
        let w = a.w * r0.cosh() / (rho * s + r0).cosh();
        View { cx: a.cx + (b.cx - a.cx) / self.d * u, cy: a.cy + (b.cy - a.cy) / self.d * u, w }
    }
}

type A2 = [f64; 6];
fn a2_mul(m: &A2, n: &A2) -> A2 {
    [m[0] * n[0] + m[2] * n[1], m[1] * n[0] + m[3] * n[1], m[0] * n[2] + m[2] * n[3], m[1] * n[2] + m[3] * n[3], m[0] * n[4] + m[2] * n[5] + m[4], m[1] * n[4] + m[3] * n[5] + m[5]]
}
fn a2_pt(m: &A2, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// Director arguments: `#a -> #b -> #c` (ids without `#`).
fn stops(n: &Node) -> Vec<String> {
    n.args.iter().skip(1).filter_map(|a| a.as_str()).filter(|s| *s != "->" && *s != "→").flat_map(|s| s.split("->").map(|x| x.trim().to_string()).collect::<Vec<_>>()).filter(|s| !s.is_empty()).map(|s| s.trim_start_matches('#').to_string()).collect()
}

/// The director verb of a `camera` node (`camera tour ...`), None for a 2.5D camera declaration.
pub fn director_verb(n: &Node) -> Option<String> {
    if n.kind != "camera" {
        return None;
    }
    match n.args.first() {
        Some(Value::Str(s)) if !s.starts_with('#') => Some(s.to_ascii_lowercase()),
        _ => None,
    }
}

pub(crate) fn parse_xy(s: &str) -> Option<(f64, f64)> {
    let s = s.trim().trim_start_matches('[').trim_end_matches(']');
    let (a, b) = s.split_once(',')?;
    Some((a.trim().trim_end_matches("px").parse().ok()?, b.trim().trim_end_matches("px").parse().ok()?))
}

impl<'a> Ctx<'a> {
    // ------------------------------------------------------------ elements
    /// `world #w 16000x9000 [max-zoom=] [tile=] [cache=]`: a canvas in the scene world (moved by the camera).
    pub fn world_el(&mut self, si: usize, n: &Node, lid: String) -> usize {
        let (w, h) = match n.args.iter().filter_map(|a| a.as_str()).find(|s| s.contains(['x', 'X']) && !s.starts_with('#')) {
            Some(s) => match parse_size(s) {
                Ok((w, h)) => (w as f64, h as f64),
                Err(e) => {
                    self.err("E_WORLD", format!("world size `{s}`: {}", e.message), "write `world #w 16000x9000`", n.span);
                    (16000.0, 9000.0)
                }
            },
            None => (16000.0, 9000.0),
        };
        let mut l = Layer::new(lid, Kind::Group);
        l.layout.pos = Pos::Abs;
        l.layout.x = Some(Len::px(0.0));
        l.layout.y = Some(Len::px(0.0));
        l.layout.w = Some(Len::px(w));
        l.layout.h = Some(Len::px(h));
        l.layout.flow = "none";
        l.tags.push("world".into());
        if let Some(t) = self.num(n, "tile") {
            l.tags.push(format!("world-tile:{}", t as u32));
        }
        if let Some(c) = self.num(n, "cache") {
            l.tags.push(format!("world-cache:{}", c as u32));
        }
        for k in n.props.keys() {
            const OK: &[&str] = &["max-zoom", "tile", "cache", "fill", "bg", "opacity", "w", "h"];
            if !OK.contains(&k.as_str()) && !["x", "y"].contains(&k.as_str()) {
                let hint = suggest(k, OK).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("world props: {}", OK.join(", ")));
                self.warn("W_WORLD_PROP", format!("world: unknown prop `{k}`"), hint, n.span);
            }
        }
        if let Some(p) = self.paint(n, "bg") {
            l.style.fill = Some(p);
        }
        let parent = self.scenes[si].world;
        let idx = self.add(Some(parent), l);
        let max_zoom = self.num(n, "max-zoom").unwrap_or(1e5);
        self.worlds.push(WorldL { scene: si, idx, max_zoom });
        idx
    }

    /// `place #p at x,y [scale=] [rot=] [w=] [h=]`: a board centered at (x, y) of its parent.
    pub fn place_el(&mut self, parent: usize, n: &Node, lid: String) -> usize {
        let at = n.args.iter().position(|a| a.as_str() == Some("at")).and_then(|i| n.args.get(i + 1)).map(|v| v.raw()).or_else(|| n.props.get("at").map(|v| v.raw()));
        let xy = at.as_deref().and_then(parse_xy);
        if xy.is_none() {
            self.err("E_WORLD", format!("place needs a position: {}", at.map(|a| format!("`at {a}` is not x,y")).unwrap_or_else(|| "no `at x,y`".into())), "write `place #board at 4000,2000 scale=0.5`", n.span);
        }
        let (x, y) = xy.unwrap_or((0.0, 0.0));
        let mut l = Layer::new(lid, Kind::Group);
        l.layout.pos = Pos::Abs;
        l.layout.x = Some(Len::px(x));
        l.layout.y = Some(Len::px(y));
        l.layout.anchor = (0.5, 0.5);
        l.layout.w = Some(Len::px(self.w));
        l.layout.h = Some(Len::px(self.h));
        l.layout.flow = "col";
        l.layout.gap = praw_n(n, "gap").unwrap_or(36.0);
        l.layout.align = n.props.get("align").map(|v| v.raw()).unwrap_or("center".into());
        l.layout.justify = n.props.get("justify").map(|v| v.raw()).unwrap_or("center".into());
        if let Some(p) = praw_n(n, "pad") {
            l.layout.pad = [p; 4];
        }
        l.tags.push("place".into());
        self.add(Some(parent), l)
    }

    // ------------------------------------------------------------ geometry
    /// Local 2D transform of layer `i` at time t (translate, rotate, scale about its origin).
    fn layer_a2(&self, i: usize, t: f64, r: [f64; 4]) -> A2 {
        let id = &self.layers[i].id;
        let v = |ch: Ch| self.tracks.get(&(id.clone(), ch)).map(|tr| tr.eval(t).n()).unwrap_or(ch.default_value().n());
        let (s, sx, sy, rot, x, y) = (v(Ch::Scale), v(Ch::Sx), v(Ch::Sy), v(Ch::Rot), v(Ch::X), v(Ch::Y));
        let o = self.layers[i].style.origin;
        let (ox, oy) = (r[0] + o.0 * r[2], r[1] + o.1 * r[3]);
        let (sn, cs) = rot.to_radians().sin_cos();
        let (ax, ay) = (s * sx, s * sy);
        // o + T + R S (p - o)
        let m = [cs * ax, sn * ax, -sn * ay, cs * ay, 0.0, 0.0];
        let (px, py) = a2_pt(&m, -ox, -oy);
        [m[0], m[1], m[2], m[3], px + ox + x, py + oy + y]
    }

    /// World rect (camera space: scene px before the camera) of layer `i` at time t.
    pub fn world_rect(&self, i: usize, t: f64, cam: usize) -> Option<[f64; 4]> {
        let g = self.geo.as_ref()?;
        let r = *g.rects.get(&self.layers[i].id)?;
        let mut m: A2 = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let mut cur = i;
        loop {
            if cur == cam {
                break;
            }
            let rc = g.rects.get(&self.layers[cur].id).copied().unwrap_or(r);
            m = a2_mul(&self.layer_a2(cur, t, rc), &m);
            let p = self.parent_of(cur);
            if p == cur {
                break;
            }
            cur = p;
        }
        let pts = [(r[0], r[1]), (r[0] + r[2], r[1]), (r[0], r[1] + r[3]), (r[0] + r[2], r[1] + r[3])].map(|(x, y)| a2_pt(&m, x, y));
        let x0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        let y0 = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
        let x1 = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
        let y1 = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
        Some([x0, y0, x1 - x0, y1 - y0])
    }

    fn cam_layer(&self, si: usize) -> usize {
        self.names.get(&format!("camera@{si}")).map(|n| n.idx).unwrap_or(self.scenes[si].world)
    }

    /// Camera pose (z, x, y) -> view.
    fn pose_view(&self, cam: usize, t: f64) -> View {
        let id = &self.layers[cam].id;
        let v = |ch: Ch| self.tracks.get(&(id.clone(), ch)).map(|tr| tr.eval(t).n()).unwrap_or(ch.default_value().n());
        let z = v(Ch::Scale).max(1e-12);
        View { cx: self.w / 2.0 - v(Ch::X) / z, cy: self.h / 2.0 - v(Ch::Y) / z, w: self.w / z }
    }

    /// The view that frames rect r (fit = fraction of the safe area).
    fn frame_view(&self, r: [f64; 4], fit: f64) -> View {
        let (sw, sh) = (self.w * (1.0 - 2.0 * self.th.safe.0), self.h * (1.0 - 2.0 * self.th.safe.1));
        let z = fit * (sw / r[2].max(1e-12)).min(sh / r[3].max(1e-12));
        View { cx: r[0] + r[2] / 2.0, cy: r[1] + r[3] / 2.0, w: self.w / z }
    }

    fn lookup(&mut self, name: &str, si: usize, span: Span, t: f64) -> Option<usize> {
        if let Some(nm) = self.names.get(&format!("{name}@{si}")) {
            return Some(nm.idx);
        }
        let mut cands: Vec<String> = self.names.iter().filter(|(_, v)| v.scene == si).map(|(k, _)| k.split('@').next().unwrap().to_string()).collect();
        cands.sort();
        cands.dedup();
        let refs: Vec<&str> = cands.iter().map(|s| s.as_str()).collect();
        let hint = suggest(name, &refs).map(|s| format!("did you mean `#{s}`?")).unwrap_or_else(|| "frame an element of this scene: give it an id like `place #board at ...`".into());
        self.diags.push(crate::diag::Diag::error("E_CAMERA_TARGET", format!("camera target `#{name}` not found in scene `{}`", self.scenes[si].id), hint, span).with_id(name.to_string()).with_at(t));
        None
    }

    fn max_zoom_of(&self, i: usize) -> f64 {
        let mut cur = i;
        loop {
            if let Some(w) = self.worlds.iter().find(|w| w.idx == cur) {
                return w.max_zoom;
            }
            let p = self.parent_of(cur);
            if p == cur {
                return 1e5;
            }
            cur = p;
        }
    }

    fn world_of(&self, i: usize) -> Option<usize> {
        let mut cur = i;
        loop {
            if self.worlds.iter().any(|w| w.idx == cur) {
                return Some(cur);
            }
            let p = self.parent_of(cur);
            if p == cur {
                return None;
            }
            cur = p;
        }
    }

    /// Framing view of target `name` at time t, with lint (not found, too small).
    fn target_view(&mut self, name: &str, e: &Ev, t: f64, fit: f64) -> Option<(usize, View, [f64; 4])> {
        let si = e.scene;
        let i = self.lookup(name, si, e.node.span, e.t)?;
        let cam = self.cam_layer(si);
        let Some(r) = self.world_rect(i, t, cam) else {
            self.diags.push(crate::diag::Diag::error("E_CAMERA_TARGET", format!("camera target `#{name}` has no layout box"), "frame a visible element (a place, text, card, ...)", e.node.span).with_id(name.to_string()).with_at(e.t));
            return None;
        };
        let mz = self.max_zoom_of(i);
        let size = r[2].max(r[3]);
        if !(size > 0.0) || size * mz < MIN_TARGET_PX {
            self.diags.push(crate::diag::Diag::error("E_CAMERA_TINY", format!("camera target `#{name}` is {:.2} px at the max zoom {mz} (needs >= {MIN_TARGET_PX} px)", size * mz), "raise the world's max-zoom= or frame a larger element", e.node.span).with_id(name.to_string()).with_at(e.t));
            return None;
        }
        let mut v = self.frame_view(r, fit);
        if self.w / v.w > mz {
            self.diags.push(crate::diag::Diag::warn("W_CAMERA_ZOOM_LIMIT", format!("framing `#{name}` needs zoom {:.0}, above max-zoom {mz}; clamped", self.w / v.w), "raise max-zoom= on the world", e.node.span).with_id(name.to_string()).with_at(e.t));
            v.w = self.w / mz;
        }
        Some((i, v, r))
    }

    // ------------------------------------------------------------ verbs
    /// `camera <verb> ...` (director verbs run after all other events: they read the final tracks).
    pub fn camera_director(&mut self, e: &Ev) {
        let n = &e.node;
        let Some(verb) = director_verb(n) else { return };
        let props: &[&str] = match verb.as_str() {
            "tour" => PROPS_TOUR,
            "zoom-through" | "zoomthrough" | "through" => PROPS_THROUGH,
            "follow" => PROPS_FOLLOW,
            "focus" => PROPS_FOCUS,
            "loop" | "zoom-loop" => PROPS_LOOP,
            _ => {
                let hint = suggest(&verb, DIRECTOR).map(|s| format!("did you mean `camera {s}`?")).unwrap_or_else(|| format!("camera verbs: {} (or `camera persp=1200` for the 2.5D camera)", DIRECTOR.join(", ")));
                self.err("E_CAMERA", format!("unknown camera verb `{verb}`"), hint, n.span);
                return;
            }
        };
        for k in n.props.keys() {
            if !props.contains(&k.as_str()) {
                let hint = suggest(k, props).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("camera {verb} props: {}", props.join(", ")));
                self.warn("W_CAMERA_PROP", format!("camera {verb}: unknown prop `{k}`"), hint, n.span);
            }
        }
        if self.geo.is_none() {
            self.need_geo = true;
            return;
        }
        if self.names.contains_key(&format!("camera@{}", e.scene)) {
            self.warn("W_CAMERA_3D", "camera director verbs with a 2.5D `camera persp=` in the same scene: the director drives x/y/zoom only", "drop `camera persp=` for world tours", n.span);
        }
        let ids = stops(n);
        if ids.is_empty() {
            self.err("E_CAMERA", format!("camera {verb} needs targets"), format!("write `camera {verb} #a -> #b`"), n.span);
            return;
        }
        let cam = self.cam_layer(e.scene);
        if !self.layers[cam].tags.iter().any(|t| t == "cam-precise") {
            self.layers[cam].tags.push("cam-precise".into());
        }
        let rho = self.num(n, "rho").unwrap_or(std::f64::consts::SQRT_2).clamp(0.3, 4.0);
        let sc = &self.scenes[e.scene];
        let (s0, s1) = (sc.start, sc.start + sc.dur);
        let at_start = (e.t - s0).abs() < 1e-3 || praw_s(n, "start").as_deref() == Some("cut");
        let t0 = e.t;
        match verb.as_str() {
            "loop" | "zoom-loop" => self.camera_loop(e, cam, &ids[0], t0, s1),
            "tour" | "focus" => {
                let fit = self.num(n, "fit").unwrap_or(if verb == "focus" { 0.7 } else { 0.85 });
                let ease = self.ease(n, "ease", Ease::Cubic(0.65, 0.0, 0.35, 1.0));
                let mv = self.dur(n, "move");
                let moving = !(verb == "focus" && praw_s(n, "move").as_deref() == Some("off"));
                // legs
                let mut views = vec![];
                for id in &ids {
                    match self.target_view(id, e, t0, fit) {
                        Some((i, v, r)) => views.push((i, v, r, id.clone())),
                        None => return,
                    }
                }
                let mut cur = self.pose_view(cam, t0);
                let mut legs: Vec<(f64, ZoomPath, f64)> = vec![]; // (start, path, dur)
                let mut t = t0;
                let mut durs = vec![];
                for (k, (_, v, _, _)) in views.iter().enumerate() {
                    if !moving {
                        break;
                    }
                    if k == 0 && at_start {
                        cur = *v;
                        durs.push(0.0);
                        continue;
                    }
                    let p = ZoomPath::new(cur, *v, rho);
                    let d = mv.unwrap_or_else(|| (0.7 + 0.32 * p.len).clamp(0.8, 3.6));
                    durs.push(d);
                    legs.push((0.0, p, d));
                    cur = *v;
                }
                let dwell_raw = praw_s(n, "dwell");
                let nst = views.len().max(1) as f64;
                let dwell = match dwell_raw.as_deref() {
                    Some("narration") | Some("auto") | None if verb == "tour" => ((s1 - 0.3 - t0 - durs.iter().sum::<f64>()) / nst).max(0.3),
                    _ => self.dur(n, "dwell").unwrap_or(1.0),
                };
                // timeline: [leg_k, dwell] per stop
                let mut segs: Vec<(f64, f64, Option<ZoomPath>, View)> = vec![]; // (t_start, dur, path or hold)
                let mut li = 0;
                let mut last = self.pose_view(cam, t0);
                for (k, (_, v, _, _)) in views.iter().enumerate() {
                    if !moving {
                        break;
                    }
                    if k == 0 && at_start {
                        segs.push((t, 0.0, None, *v));
                    } else {
                        let (_, p, d) = legs[li].clone();
                        li += 1;
                        segs.push((t, d, Some(p), *v));
                        t += d;
                    }
                    last = *v;
                    if k + 1 < views.len() {
                        t += dwell;
                    }
                }
                let _ = last;
                if moving {
                    self.bake_camera(cam, t0, t, &|tt: f64| {
                        let mut v = segs[0].3;
                        for (ts, d, p, end) in &segs {
                            if tt < *ts {
                                break;
                            }
                            v = match p {
                                Some(p) if tt < ts + d => p.at(p.len * ease.sample((tt - ts) / d)),
                                Some(p) => {
                                    let _ = p;
                                    *end
                                }
                                None => *end,
                            };
                        }
                        v
                    });
                    self.check_speed(cam, t0, t, n.span);
                }
                if verb == "focus" {
                    let (i, v, r, _) = views[0].clone();
                    let arrive = if moving { t } else { t0 };
                    self.focus_overlay(e, i, r, v, arrive);
                }
            }
            "zoom-through" | "zoomthrough" | "through" => {
                let fit = self.num(n, "fit").unwrap_or(0.85);
                let ease = self.ease(n, "ease", Ease::Cubic(0.45, 0.0, 0.25, 1.0));
                let mut views = vec![];
                for id in &ids {
                    match self.target_view(id, e, t0, fit) {
                        Some((_, v, _)) => views.push(v),
                        None => return,
                    }
                }
                let mut pts = vec![];
                if !at_start {
                    pts.push(self.pose_view(cam, t0));
                }
                pts.extend(views.iter().copied());
                if pts.len() < 2 {
                    self.err("E_CAMERA", "camera zoom-through needs two views", "write `camera zoom-through #a -> #b`", n.span);
                    return;
                }
                let paths: Vec<ZoomPath> = pts.windows(2).map(|w| ZoomPath::new(w[0], w[1], rho)).collect();
                let total: f64 = paths.iter().map(|p| p.len).sum::<f64>().max(1e-9);
                let d = self.dur(n, "dur").unwrap_or((1.2 + 0.45 * total).clamp(2.0, 14.0));
                let t1 = t0 + d;
                self.bake_camera(cam, t0, t1, &|tt: f64| {
                    let mut s = total * ease.sample((tt - t0) / d);
                    for (k, p) in paths.iter().enumerate() {
                        if s <= p.len || k + 1 == paths.len() {
                            return p.at(s.max(0.0));
                        }
                        s -= p.len;
                    }
                    pts[pts.len() - 1]
                });
                self.check_speed(cam, t0, t1, n.span);
            }
            "follow" => {
                let fit = self.num(n, "fit").unwrap_or(0.7);
                let lead = self.dur(n, "lead").unwrap_or(0.0);
                let tau = self.dur(n, "smooth").unwrap_or(0.25).max(0.0);
                let t1 = match (self.dur(n, "for"), praw_s(n, "until")) {
                    (Some(d), _) => t0 + d,
                    (None, Some(u)) => self.tl.anchor_time(e.scene, None, u.trim_start_matches('@'), t0, n.span).unwrap_or(s1),
                    _ => s1,
                }
                .min(s1);
                let id = ids[0].clone();
                let Some((i, _, r0)) = self.target_view(&id, e, t0, fit) else { return };
                let zoom_with = praw_s(n, "zoom").as_deref() == Some("follow");
                let base_w = self.frame_view(r0, fit).w;
                // per-frame target views (target at t + lead), exponentially smoothed
                let fps = self.fps;
                let mut keys: Vec<(f64, View)> = vec![];
                let mut v = self.pose_view(cam, t0);
                let k = if tau > 0.0 { 1.0 - (-1.0 / (fps * tau)).exp() } else { 1.0 };
                let f0 = (t0 * fps).ceil() as i64;
                let f1 = (t1 * fps).floor() as i64;
                for f in f0..=f1 {
                    let tt = f as f64 / fps;
                    let Some(r) = self.world_rect(i, tt + lead, cam) else { continue };
                    let mut tv = self.frame_view(r, fit);
                    if !zoom_with {
                        tv.w = base_w;
                    }
                    let lw = v.w.ln() + (tv.w.ln() - v.w.ln()) * k;
                    v = View { cx: v.cx + (tv.cx - v.cx) * k, cy: v.cy + (tv.cy - v.cy) * k, w: lw.exp() };
                    keys.push((tt, v));
                }
                if keys.len() >= 2 {
                    self.bake_keys(cam, t0, &keys);
                    self.check_speed(cam, t0, t1, n.span);
                }
            }
            _ => {}
        }
    }

    /// `camera loop #nest [levels=] [dur=] [dir=in|out] [zoom=] [wrap=on|off]`: a seamless infinite
    /// zoom into a self-similar nest. The camera scales about the nest center at constant log-speed
    /// (exactly `levels` nest levels per `dur`, per-frame keys, no ease) and the nest turns back by
    /// its per-level `rot`, so the frame at `dur` equals the frame at the start; then it wraps.
    fn camera_loop(&mut self, e: &Ev, cam: usize, id: &str, t0: f64, s1: f64) {
        let n = e.node.clone();
        let Some(ti) = self.lookup(id, e.scene, n.span, e.t) else { return };
        // nests: the target itself or every nest below it
        let mut nests = vec![];
        let mut stack = vec![ti];
        while let Some(j) = stack.pop() {
            match crate::nest::tag_params(&self.layers[j].tags) {
                Some(p) => nests.push((j, p)),
                None => stack.extend(self.kids[j].iter().copied()),
            }
        }
        let (us, ur) = (self.num(&n, "scale"), self.num(&n, "rot"));
        let first = nests.first().map(|x| x.1);
        match first {
            None if us.is_none() => {
                self.err("E_CAMERA", format!("camera loop: `#{id}` is not a nest and has no nest below it"), format!("build it with `nest #{id} levels=24 scale=0.75`, or give the loop scale= (and rot=) of your own self-similar content"), n.span);
                return;
            }
            None => self.warn("W_LOOP_SELF_SIMILAR", format!("camera loop: `#{id}` is not a nest, so self-similarity is not checked"), "build the levels with `nest` so the seam can be verified", n.span),
            Some(f) => {
                if nests.iter().any(|(_, p)| (p.0 - f.0).abs() > 1e-9 || (p.1 - f.1).abs() > 1e-9 || p.3 != f.3) {
                    self.warn("W_LOOP_SELF_SIMILAR", format!("camera loop: the nests below `#{id}` differ in scale/rot/cycle; only one can loop seamlessly"), "give every nest in the loop the same scale=, rot= and cycle length", n.span);
                }
                if us.is_some_and(|k| (k - f.0).abs() > 1e-9) || ur.is_some_and(|r| (r - f.1).abs() > 1e-9) {
                    self.warn("W_LOOP_SELF_SIMILAR", "camera loop: scale=/rot= differ from the nest's; the seam will jump", "drop scale=/rot= on the loop (it reads them from the nest)", n.span);
                }
            }
        }
        let k = us.or(first.map(|f| f.0)).unwrap_or(0.8);
        let r = ur.or(first.map(|f| f.1)).unwrap_or(0.0);
        if !(k > 0.0 && k.is_finite() && (k - 1.0).abs() > 1e-9) {
            self.err("E_CAMERA", format!("camera loop scale={k}: needs a factor > 0 and not 1"), "write `scale=0.75`", n.span);
            return;
        }
        let per = first.map(|f| f.3).unwrap_or(1);
        let levels = match self.num(&n, "levels") {
            Some(l) if l >= 1.0 && l.fract() == 0.0 => l as usize,
            Some(l) => {
                self.err("E_CAMERA", format!("camera loop levels={l}: needs a whole number >= 1"), format!("write `levels={per}`"), n.span);
                return;
            }
            None => per,
        };
        if levels % per != 0 {
            self.warn("W_LOOP_SELF_SIMILAR", format!("camera loop levels={levels} is not a multiple of the nest's cycle ({per} levels): colors jump at the seam"), format!("use levels={}", per * levels.div_ceil(per)), n.span);
        }
        let out = match praw_s(&n, "dir").as_deref() {
            None | Some("in") => false,
            Some("out") => true,
            Some(d) => {
                self.err("E_CAMERA", format!("camera loop dir={d}"), "dir=in|out", n.span);
                return;
            }
        };
        let wrap = praw_s(&n, "wrap").as_deref() != Some("off");
        let z0 = self.num(&n, "zoom").unwrap_or(1.0).max(1e-6);
        let dur = self.dur(&n, "dur").unwrap_or(s1 - t0).max(1.0 / self.fps);
        let Some(tr) = self.world_rect(ti, t0, cam) else {
            self.err("E_CAMERA_TARGET", format!("camera target `#{id}` has no layout box"), "loop a nest or a layer holding nests", n.span);
            return;
        };
        let (cx, cy) = (tr[0] + tr[2] / 2.0, tr[1] + tr[3] / 2.0);
        let lv = levels as f64;
        // progress in levels at time t (wraps exactly at whole loops)
        let prog = |t: f64| -> f64 {
            let u = (t - t0) / dur;
            let u = if (u - u.round()).abs() < 1e-9 { u.round() } else { u };
            let f = if wrap { u - u.floor() } else { u };
            if out { lv * (1.0 - f) } else { lv * f }
        };
        let fps = self.fps;
        let mut ts = vec![(t0, prog(t0))];
        let mut f = (t0 * fps).floor() as i64 + 1;
        let mut next_wrap = t0 + dur;
        while (f as f64) / fps < s1 + 1e-9 {
            let t = f as f64 / fps;
            if wrap && t >= next_wrap - 1e-9 {
                // hold the end value up to the wrap, then jump (motion blur never sweeps back)
                // (1 ms: keys closer than 0.5 ms are merged)
                if next_wrap - 1e-3 > ts.last().unwrap().0 + 1e-3 {
                    ts.push((next_wrap - 1e-3, if out { 0.0 } else { lv }));
                }
                next_wrap += dur;
            }
            ts.push((t, prog(t)));
            f += 1;
        }
        let views: Vec<(f64, View)> = ts.iter().map(|&(t, p)| (t, View { cx, cy, w: self.w / z0 * k.powf(p) })).collect();
        self.bake_keys(cam, t0, &views);
        if r != 0.0 {
            let base = self.tracks.get(&(self.layers[ti].id.clone(), Ch::Rot)).map(|t| t.base.n()).unwrap_or(0.0);
            let keys: Vec<(f64, TV)> = ts.iter().map(|&(t, p)| (t, TV::N(base - r * p))).collect();
            self.track(ti, Ch::Rot).bake(t0, &keys);
            if !self.layers[ti].tags.iter().any(|t| t == "cam-precise") {
                self.layers[ti].tags.push("cam-precise".into());
            }
        }
        // blur/glow radii are scene px before the camera: counter-scale static ones so they stay
        // constant on screen (one radius for every level is what makes a group halo self-similar)
        let mut sub = vec![];
        let mut st = vec![ti];
        while let Some(j) = st.pop() {
            sub.push(j);
            st.extend(self.kids[j].iter().copied());
        }
        let mut baked = vec![(self.layers[ti].id.clone(), Ch::Rot)];
        for j in sub {
            for ch in [Ch::Blur, Ch::Glow] {
                let key = (self.layers[j].id.clone(), ch);
                let Some(tr) = self.tracks.get(&key) else { continue };
                let b = tr.base.n();
                if b <= 0.0 || !tr.keys.is_empty() {
                    continue;
                }
                let keys: Vec<(f64, TV)> = ts.iter().map(|&(t, p)| (t, TV::N(b * k.powf(p)))).collect();
                self.track(j, ch).bake(t0, &keys);
                baked.push(key);
            }
        }
        // speed (the per-frame check would count the wrap as a jump)
        let oct = lv * k.log2().abs() / dur;
        if oct > FAST_OCT {
            self.warn("W_CAMERA_FAST", format!("camera loop zooms {oct:.1} octaves/s (readable <= {FAST_OCT})"), "fewer levels= per loop or a longer dur=", n.span);
        }
        self.loop_lint(&n, id, ti, cam, &nests, (k, r, levels, z0, dur, t0, s1), (cx, cy), &baked);
    }

    /// W_LOOP_SEAM / W_LOOP_CONTENT: what would make frame `dur` differ from frame 0.
    #[allow(clippy::type_complexity)]
    fn loop_lint(&mut self, n: &Node, id: &str, ti: usize, cam: usize, nests: &[(usize, (f64, f64, usize, usize))], lp: (f64, f64, usize, f64, f64, f64, f64), c: (f64, f64), baked: &[(String, Ch)]) {
        let (k, r, levels, z0, dur, t0, s1) = lp;
        let lv = levels as f64;
        for &(j, (_, _, nl, _)) in nests {
            let Some(b) = self.world_rect(j, t0, cam) else { continue };
            let name = self.layers[j].name.clone().unwrap_or_else(|| self.lid(j));
            let (bx, by) = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
            if (bx - c.0).hypot(by - c.1) > 0.5 {
                self.warn("W_LOOP_SELF_SIMILAR", format!("camera loop: nest `#{name}` is centered {:.0} px away from the loop center (`#{id}`)", (bx - c.0).hypot(by - c.1)), "center the nests on the looped element (leave `at` out)", n.span);
            }
            if nl <= levels {
                self.warn("W_LOOP_SEAM", format!("camera loop zooms {levels} levels but nest `#{name}` has only {nl}"), format!("raise the nest's levels= well above {levels}"), n.span);
                continue;
            }
            if k >= 1.0 {
                continue;
            }
            // outer: after one loop, level `levels-1` (level 0 grown by 1/k, turned by -r) must cover the frame
            let (hw, hh) = (b[2] / (2.0 * k), b[3] / (2.0 * k));
            let (sn, cs) = r.to_radians().sin_cos();
            let covers = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].iter().all(|&(sx, sy)| {
                let (qx, qy) = (sx * self.w / (2.0 * z0), sy * self.h / (2.0 * z0));
                let (px, py) = (cs * qx - sn * qy, sn * qx + cs * qy);
                px.abs() <= hw + 1e-6 && py.abs() <= hh + 1e-6
            });
            if !covers {
                self.warn("W_LOOP_SEAM", format!("camera loop: the outer levels of nest `#{name}` do not reach past the frame, so its edge pops at the seam"), "make the nest box larger (w=/h=, e.g. the frame diagonal) or raise zoom= on the loop", n.span);
            }
            // inner: the levels the loop pulls in from the center must start below ~2 px
            let px = b[2].max(b[3]) * z0 * k.powf(nl as f64 - lv);
            if px > 2.0 {
                let need = levels + ((2.0 / (b[2].max(b[3]) * z0)).ln() / k.ln()).ceil().max(0.0) as usize;
                self.warn("W_LOOP_SEAM", format!("camera loop: the innermost levels of nest `#{name}` are {px:.1} px at the loop start, so the center pops at the seam"), format!("raise the nest's levels= to {need}"), n.span);
            }
        }
        // other content moved by the camera breaks the seam
        let mut chain = vec![ti];
        let mut cur = ti;
        while cur != cam {
            let p = self.parent_of(cur);
            if p == cur {
                break;
            }
            chain.push(p);
            cur = p;
        }
        let mut stray = vec![];
        for &a in chain.iter().skip(1) {
            for &kid in &self.kids[a] {
                if !chain.contains(&kid) {
                    stray.push(self.layers[kid].name.clone().map(|s| format!("#{s}")).unwrap_or_else(|| self.lid(kid)));
                }
            }
        }
        if !stray.is_empty() {
            let more = if stray.len() > 3 { format!(" (+{})", stray.len() - 3) } else { String::new() };
            self.warn("W_LOOP_CONTENT", format!("camera loop: {}{more} moves with the camera but is not part of the self-similar nest, so it jumps at the seam", stray[..stray.len().min(3)].join(", ")), "mark it `hud` (stays still) or move it into the nest", n.span);
        }
        // animations inside the loop must repeat with it
        let t1 = (t0 + dur).min(s1);
        if t1 - t0 >= dur - 1e-6 {
            let mut sub = vec![];
            let mut st = vec![ti];
            while let Some(j) = st.pop() {
                sub.push(self.layers[j].id.clone());
                st.extend(self.kids[j].iter().copied());
            }
            let mut bad: Vec<String> = vec![];
            for ((lid, ch), tr) in self.tracks.iter() {
                if tr.keys.is_empty() || baked.iter().any(|(l, c)| l == lid && c == ch) || !sub.contains(lid) {
                    continue;
                }
                let (a, b) = (tr.eval(t0), tr.eval(t1));
                let differs = match (&a, &b) {
                    (TV::N(x), TV::N(y)) => (x - y).abs() > 1e-3,
                    _ => a != b,
                };
                if differs && bad.len() < 3 {
                    bad.push(format!("{lid} {ch:?}"));
                }
            }
            if !bad.is_empty() {
                self.warn("W_LOOP_CONTENT", format!("camera loop: animation inside the loop does not repeat with it ({})", bad.join(", ")), format!("make those animations periodic in {dur:.2}s (same value at the loop start and end)"), n.span);
            }
        }
    }

    /// Bake view(t) into per-frame X/Y/Scale keys on the camera layer over [t0, t1].
    fn bake_camera(&mut self, cam: usize, t0: f64, t1: f64, view: &dyn Fn(f64) -> View) {
        let fps = self.fps;
        let mut keys = vec![(t0, view(t0))];
        let mut f = (t0 * fps).floor() as i64 + 1;
        while (f as f64) / fps < t1 - 1e-9 {
            let t = f as f64 / fps;
            keys.push((t, view(t)));
            f += 1;
        }
        keys.push((t1, view(t1)));
        self.bake_keys(cam, t0, &keys);
    }

    fn bake_keys(&mut self, cam: usize, t0: f64, keys: &[(f64, View)]) {
        let (w, h) = (self.w, self.h);
        let pose = |v: &View| {
            let z = w / v.w;
            (z, -z * (v.cx - w / 2.0), -z * (v.cy - h / 2.0))
        };
        let zs: Vec<(f64, TV)> = keys.iter().map(|(t, v)| (*t, TV::N(pose(v).0))).collect();
        let xs: Vec<(f64, TV)> = keys.iter().map(|(t, v)| (*t, TV::N(pose(v).1))).collect();
        let ys: Vec<(f64, TV)> = keys.iter().map(|(t, v)| (*t, TV::N(pose(v).2))).collect();
        self.track(cam, Ch::Scale).bake(t0, &zs);
        self.track(cam, Ch::X).bake(t0, &xs);
        self.track(cam, Ch::Y).bake(t0, &ys);
    }

    /// W_CAMERA_FAST: zoom (octaves/s) or pan (frame widths/s at the center) too fast to read.
    fn check_speed(&mut self, cam: usize, t0: f64, t1: f64, span: Span) {
        let fps = self.fps;
        let (mut worst_z, mut worst_p, mut at_z, mut at_p) = (0.0f64, 0.0f64, t0, t0);
        let mut prev = self.pose_view(cam, t0);
        let mut f = (t0 * fps).ceil() as i64 + 1;
        while (f as f64) / fps <= t1 + 1e-9 {
            let t = f as f64 / fps;
            let v = self.pose_view(cam, t);
            let oct = (prev.w / v.w).log2().abs() * fps;
            // screen motion of the frame center: world displacement in screen px (mean zoom)
            let zm = self.w / (prev.w * v.w).sqrt();
            let pan = ((v.cx - prev.cx).powi(2) + (v.cy - prev.cy).powi(2)).sqrt() * zm / self.w * fps;
            if oct > worst_z {
                worst_z = oct;
                at_z = t;
            }
            if pan > worst_p {
                worst_p = pan;
                at_p = t;
            }
            prev = v;
            f += 1;
        }
        if worst_z > FAST_OCT {
            self.diags.push(crate::diag::Diag::warn("W_CAMERA_FAST", format!("camera zooms {worst_z:.1} octaves/s at {at_z:.2}s (readable <= {FAST_OCT})"), "give the move more time (move=, dur=) or a softer ease", span).with_at(at_z));
        }
        if worst_p > FAST_PAN {
            self.diags.push(crate::diag::Diag::warn("W_CAMERA_FAST", format!("camera pans {worst_p:.1} frame widths/s at {at_p:.2}s (readable <= {FAST_PAN})"), "give the move more time (move=, dur=), raise rho= (zoom out more, pan less) or a softer ease", span).with_at(at_p));
        }
    }

    /// `camera focus`: a dark overlay over the world with a hole around the target.
    fn focus_overlay(&mut self, e: &Ev, target: usize, r: [f64; 4], v: View, arrive: f64) {
        let n = e.node.clone();
        let col = self.color(&n, "overlay").unwrap_or(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.66 });
        let pad_px = self.num(&n, "pad").unwrap_or(28.0);
        let fade = self.dur(&n, "dur").unwrap_or(0.35);
        let hold = self.dur(&n, "for").unwrap_or(1.6);
        let host = self.world_of(target).unwrap_or(self.cam_layer(e.scene));
        let z = self.w / v.w;
        let pad = pad_px / z;
        let hole = [r[0] - pad, r[1] - pad, r[2] + 2.0 * pad, r[3] + 2.0 * pad];
        // cover the framed view many times over
        let ext = v.w * 6.0;
        let (ox0, oy0, ox1, oy1) = (hole[0] - ext, hole[1] - ext, hole[0] + hole[2] + ext, hole[1] + hole[3] + ext);
        // the host's own placement (worlds sit at 0,0 of the camera space)
        let off = self.geo.as_ref().and_then(|g| g.rects.get(&self.layers[host].id).copied()).unwrap_or([0.0; 4]);
        let mut g = Layer::new(format!("focus-{}-{}", self.lid(target), self.seq + 1), Kind::Group);
        g.layout.pos = Pos::Fill;
        g.tags.push("focus".into());
        let gi = self.add(Some(host), g);
        let rects = [
            [ox0, oy0, ox1 - ox0, hole[1] - oy0],
            [ox0, hole[1] + hole[3], ox1 - ox0, oy1 - hole[1] - hole[3]],
            [ox0, hole[1], hole[0] - ox0, hole[3]],
            [hole[0] + hole[2], hole[1], ox1 - hole[0] - hole[2], hole[3]],
        ];
        for (k, rr) in rects.iter().enumerate() {
            let mut l = Layer::new(format!("{}-{k}", self.lid(gi)), Kind::Rect);
            l.layout.pos = Pos::Abs;
            l.layout.x = Some(Len::px(rr[0] - off[0]));
            l.layout.y = Some(Len::px(rr[1] - off[1]));
            l.layout.w = Some(Len::px(rr[2]));
            l.layout.h = Some(Len::px(rr[3]));
            l.style.fill = Some(Paint::Solid(ColorRef::Lit(col.clone())));
            self.add(Some(gi), l);
        }
        let t_in = (arrive - 0.15).max(e.t);
        self.set_base(gi, Ch::Opacity, TV::N(0.0));
        let ease = Ease::Cubic(0.33, 1.0, 0.68, 1.0);
        self.track(gi, Ch::Opacity).to(t_in, t_in + fade, TV::N(1.0), ease.clone());
        self.track(gi, Ch::Opacity).to(t_in + fade + hold, t_in + 2.0 * fade + hold, TV::N(0.0), ease);
    }
}

fn praw_n(n: &Node, k: &str) -> Option<f64> {
    n.props.get(k).and_then(|v| v.raw().trim_end_matches("px").parse().ok())
}
fn praw_s(n: &Node, k: &str) -> Option<String> {
    n.props.get(k).map(|v| v.raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_path_hits_both_ends_at_extreme_ratios() {
        let a = View { cx: 8000.0, cy: 4500.0, w: 16000.0 };
        let b = View { cx: 7123.456, cy: 3999.25, w: 0.0016 };
        let p = ZoomPath::new(a, b, std::f64::consts::SQRT_2);
        assert!(p.len.is_finite() && p.len > 0.0);
        let v0 = p.at(0.0);
        assert!((v0.cx - a.cx).abs() < 1e-6 && (v0.cy - a.cy).abs() < 1e-6 && (v0.w / a.w - 1.0).abs() < 1e-9);
        let v1 = p.at(p.len * (1.0 - 1e-12));
        assert!((v1.cx - b.cx).abs() < 1e-5 && (v1.cy - b.cy).abs() < 1e-5 && (v1.w / b.w - 1.0).abs() < 1e-6, "{v1:?}");
        assert_eq!(p.at(p.len), b);
        // monotone zoom-in for a nested target (no zoom-out bump)
        let mut last = f64::MAX;
        for k in 0..=200 {
            let w = p.at(p.len * k as f64 / 200.0).w;
            assert!(w <= last * (1.0 + 1e-12));
            last = w;
        }
    }

    #[test]
    fn zoom_path_pure_zoom_and_far_pan() {
        let a = View { cx: 0.0, cy: 0.0, w: 1000.0 };
        let p = ZoomPath::new(a, View { cx: 0.0, cy: 0.0, w: 1.0 }, 1.4);
        assert!((p.len - 1000f64.ln() / 1.4).abs() < 1e-9);
        assert!((p.at(p.len / 2.0).w - 1000f64.sqrt()).abs() < 1e-6);
        // far pan between equal widths zooms out in the middle
        let q = ZoomPath::new(a, View { cx: 50000.0, cy: 0.0, w: 1000.0 }, 1.4);
        assert!(q.at(q.len / 2.0).w > 5000.0);
    }

    #[test]
    fn stops_parse_arrows() {
        let n = Node { kind: "camera".into(), args: vec![Value::Str("tour".into()), Value::Str("#a".into()), Value::Str("->".into()), Value::Str("#b->#c".into())], ..Default::default() };
        assert_eq!(stops(&n), vec!["a", "b", "c"]);
        assert_eq!(director_verb(&n).as_deref(), Some("tour"));
        let decl = Node { kind: "camera".into(), ..Default::default() };
        assert_eq!(director_verb(&decl), None);
    }
}
