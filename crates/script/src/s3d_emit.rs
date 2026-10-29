//! reel-ir `scene3d` content of a `scene3d` layer (child module of `reelir`): the object layers
//! below it become objects (preorder, parents first) and lights, their channel tracks become
//! `s3d.*` tracks on the scene's node. See `s3d.rs` for the surface.
use super::*;
use crate::s3d::S3dCfg;
use agentfx_ir::s3d::{Bloom3d, Obj3d};

/// Channels of an object -> reel-ir field.
const OBJ_CH: &[(Ch, &str)] = &[
    (Ch::X, "x"),
    (Ch::Y, "y"),
    (Ch::Z, "z"),
    (Ch::RotX, "rx"),
    (Ch::RotY, "ry"),
    (Ch::Rot, "rz"),
    (Ch::Scale, "s"),
    (Ch::Sx, "sx"),
    (Ch::Sy, "sy"),
    (Ch::Sz, "sz"),
    (Ch::Opacity, "opacity"),
    (Ch::Emissive, "emissive"),
];
const CAM_CH: &[(Ch, &str)] = &[
    (Ch::CamX, "s3d.cam.x"),
    (Ch::CamY, "s3d.cam.y"),
    (Ch::CamZ, "s3d.cam.z"),
    (Ch::LookX, "s3d.cam.tx"),
    (Ch::LookY, "s3d.cam.ty"),
    (Ch::LookZ, "s3d.cam.tz"),
    (Ch::Fov, "s3d.cam.fov"),
    (Ch::Roll, "s3d.cam.roll"),
    (Ch::Ambient, "s3d.ambient"),
];

/// Channels a scene3d layer uses itself (not reported as unsupported).
pub fn scene_ch(c: Ch) -> bool {
    CAM_CH.iter().any(|(x, _)| *x == c)
}

impl<'a> Em<'a> {
    fn ch_track(&self, id: &str, ch: Ch) -> Option<&'a Track> {
        self.ch.track(id, ch)
    }
    /// The value before any key (or the base).
    fn ch_base(&self, id: &str, ch: Ch, dflt: f64) -> f64 {
        self.ch_track(id, ch).map(|t| t.eval(-1e9).n()).unwrap_or(dflt)
    }
    fn ch_emit(&mut self, node: usize, id: &str, ch: Ch, prop: &str) -> bool {
        let Some(t) = self.ch_track(id, ch) else { return false };
        if t.keys.len() < 2 || t.is_static() {
            return false;
        }
        let keys: Vec<(f64, Value, Ease)> = t
            .keys
            .iter()
            .map(|k| {
                let v = match &k.v {
                    TV::C(c) => json!({"color": [c.r, c.g, c.b, c.a]}),
                    TV::N(x) => json!({"number": r4(*x)}),
                };
                (k.t, v, k.ease.clone())
            })
            .collect();
        match self.track_json(node, prop, &keys, prop) {
            Some(j) => {
                self.tracks.push(j);
                true
            }
            None => false,
        }
    }

    /// reel-ir content for scene3d layer `l` (node `idx`, laid-out box `r`).
    pub(super) fn s3d_content(&mut self, l: &'a Layer, cfg: &S3dCfg, r: [f64; 4], idx: usize) -> Value {
        let mut sc = cfg.scene.clone();
        sc.rect = [r4(r[0]), r4(r[1]), r4(r[2]), r4(r[3])];
        let id = l.id.as_str();
        let c = &mut sc.camera;
        c.pos = [self.ch_base(id, Ch::CamX, c.pos[0]), self.ch_base(id, Ch::CamY, c.pos[1]), self.ch_base(id, Ch::CamZ, c.pos[2])];
        c.target = [self.ch_base(id, Ch::LookX, c.target[0]), self.ch_base(id, Ch::LookY, c.target[1]), self.ch_base(id, Ch::LookZ, c.target[2])];
        c.fov = self.ch_base(id, Ch::Fov, c.fov).clamp(1.0, 170.0);
        c.roll = self.ch_base(id, Ch::Roll, c.roll);
        sc.ambient = self.ch_base(id, Ch::Ambient, sc.ambient).max(0.0);
        for (ch, prop) in CAM_CH {
            self.ch_emit(idx, id, *ch, prop);
        }
        // objects, preorder
        let mut stack: Vec<(&'a Layer, Option<usize>)> = l.children.iter().rev().map(|c| (c, None)).collect();
        let mut fonts_used = false;
        while let Some((c, parent)) = stack.pop() {
            let Kind::Obj3d(oc) = &c.kind else { continue };
            let cid = c.id.as_str();
            if let Some(light) = &oc.light {
                let li = sc.lights.len();
                let mut lt = light.clone();
                lt.pos = [self.ch_base(cid, Ch::X, lt.pos[0]), self.ch_base(cid, Ch::Y, lt.pos[1]), self.ch_base(cid, Ch::Z, lt.pos[2])];
                lt.intensity = self.ch_base(cid, Ch::Intensity, lt.intensity).max(0.0);
                sc.lights.push(lt);
                for (ch, f) in [(Ch::X, "x"), (Ch::Y, "y"), (Ch::Z, "z"), (Ch::Intensity, "intensity")] {
                    self.ch_emit(idx, cid, ch, &format!("s3d.light.{li}.{f}"));
                }
                continue;
            }
            let oi = sc.objects.len();
            let mut o: Obj3d = oc.obj.clone();
            o.parent = parent;
            let b = |me: &Self, ch: Ch, d: f64| me.ch_base(cid, ch, d);
            o.pos = [b(self, Ch::X, o.pos[0]), b(self, Ch::Y, o.pos[1]), b(self, Ch::Z, o.pos[2])];
            o.rot = [b(self, Ch::RotX, o.rot[0]), b(self, Ch::RotY, o.rot[1]), b(self, Ch::Rot, o.rot[2])];
            o.scale = [b(self, Ch::Sx, o.scale[0]), b(self, Ch::Sy, o.scale[1]), b(self, Ch::Sz, o.scale[2])];
            o.material.opacity = b(self, Ch::Opacity, o.material.opacity).clamp(0.0, 1.0);
            o.material.emissive = b(self, Ch::Emissive, o.material.emissive).max(0.0);
            if let Some(TV::C(col)) = self.ch_track(cid, Ch::Color).map(|t| t.eval(-1e9)) {
                o.material.color = [col.r, col.g, col.b, col.a];
            }
            let mut emitted_s = false;
            for (ch, f) in OBJ_CH {
                let e = self.ch_emit(idx, cid, *ch, &format!("s3d.obj.{oi}.{f}"));
                if *ch == Ch::Scale {
                    emitted_s = e;
                }
            }
            if !emitted_s {
                let s = b(self, Ch::Scale, 1.0);
                o.scale = [o.scale[0] * s, o.scale[1] * s, o.scale[2] * s];
            }
            self.ch_emit(idx, cid, Ch::Color, &format!("s3d.obj.{oi}.color"));
            if let (Some(f), agentfx_ir::s3d::Shape3d::Extrude { font, .. }) = (&oc.font, &mut o.shape) {
                let fi = match self.cloud_fonts.iter().position(|x| x == f) {
                    Some(i) => i,
                    None => {
                        self.cloud_fonts.push(f.clone());
                        self.cloud_fonts.len() - 1
                    }
                };
                *font = fi;
                fonts_used = true;
            }
            if let Some(tex) = &oc.texture {
                o.material.asset = Some(self.asset(tex, 1.0));
            }
            if let (Some(src), agentfx_ir::s3d::Shape3d::Screen { rect, .. }) = (&oc.src, &mut o.shape) {
                match self.rect.get(src).copied() {
                    Some(sr) if !src.is_empty() && sr[2] > 0.0 && sr[3] > 0.0 => {
                        *rect = [r4(sr[0]), r4(sr[1]), r4(sr[2]), r4(sr[3])];
                        self.s3d_screens.push((idx, oi, src.clone()));
                    }
                    _ => {
                        self.note("3D screen without a laid-out source layer (shown blank)");
                        o.shape = agentfx_ir::s3d::Shape3d::Plane { size: match o.shape {
                            agentfx_ir::s3d::Shape3d::Screen { size, .. } => size,
                            _ => [1.0, 1.0],
                        } };
                    }
                }
            }
            sc.objects.push(o);
            for k in c.children.iter().rev() {
                stack.push((k, Some(oi)));
            }
        }
        if fonts_used {
            self.s3d_font_nodes.push(idx);
        }
        let emissive = sc.objects.iter().any(|o| o.material.emissive > 0.0) || self.tracks.iter().any(|t| t["node"] == json!(idx) && t["property"].as_str().is_some_and(|p| p.ends_with(".emissive")));
        if sc.bloom.is_none() && !cfg.bloom_off && emissive {
            sc.bloom = Some(Bloom3d { radius: 14.0, strength: 0.9 });
        }
        if !sc.objects.iter().any(|o| o.receive) {
            sc.shadow = None;
        }
        let mut v = serde_json::to_value(&sc).unwrap_or(Value::Null);
        v["type"] = json!("scene3d");
        v
    }
}
