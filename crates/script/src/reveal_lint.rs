//! Lint `W_REVEAL_NO_INK`: a `reveal #el with #fluid` (mode=alpha) whose element the dye never
//! reaches shows nothing. Lint time cannot afford the fluid simulation the renderer runs, so this
//! is a geometric estimate of where dye can get to: every dye source (a `pour` jet, an `ink #el
//! into` source) spawns a region, grown by how far its flow can carry dye before the scene ends;
//! `stir`s that touch dye spread it over their disc; turbulence widens everything, buoyancy
//! extends it up (or down) to the box edge. The estimate is deliberately generous (about twice
//! the reach measured on renders), so it only fires when the target is clearly out of reach.
use crate::diag::Diag;
use crate::fluid::{resolve_len, FluidCfg, FluidEv};
use crate::ir::{Ir, Kind, Layer};
use crate::timing::Timeline;

/// Below this share of the target's area within reach, the reveal is reported.
const MIN_REACH: f64 = 0.01;
/// Curl-noise feature size of the renderer's turbulence (scene px, crates/render fluid.rs).
const TURB_SCALE: f64 = 220.0;
/// Feature size of an ink source's noise kick (scene px).
const KICK_SCALE: f64 = 70.0;

#[derive(Debug, Clone, Copy)]
enum Region {
    /// disc (cx, cy, r)
    Disc(f64, f64, f64),
    /// capsule: segment (ax, ay)-(bx, by), radius r
    Capsule(f64, f64, f64, f64, f64),
    /// rect (x, y, w, h) grown by m
    Rect([f64; 4], f64),
}

impl Region {
    fn contains(&self, x: f64, y: f64) -> bool {
        match *self {
            Region::Disc(cx, cy, r) => (x - cx).powi(2) + (y - cy).powi(2) <= r * r,
            Region::Capsule(ax, ay, bx, by, r) => {
                let (dx, dy) = (bx - ax, by - ay);
                let l2 = dx * dx + dy * dy;
                let t = if l2 > 0.0 { (((x - ax) * dx + (y - ay) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                (x - ax - t * dx).powi(2) + (y - ay - t * dy).powi(2) <= r * r
            }
            Region::Rect(r, m) => x >= r[0] - m && x <= r[0] + r[2] + m && y >= r[1] - m && y <= r[1] + r[3] + m,
        }
    }
    /// bounding box (x0, y0, x1, y1)
    fn bbox(&self) -> [f64; 4] {
        match *self {
            Region::Disc(cx, cy, r) => [cx - r, cy - r, cx + r, cy + r],
            Region::Capsule(ax, ay, bx, by, r) => [ax.min(bx) - r, ay.min(by) - r, ax.max(bx) + r, ay.max(by) + r],
            Region::Rect(r, m) => [r[0] - m, r[1] - m, r[0] + r[2] + m, r[1] + r[3] + m],
        }
    }
    fn grow(&self, g: f64) -> Region {
        match *self {
            Region::Disc(cx, cy, r) => Region::Disc(cx, cy, r + g),
            Region::Capsule(ax, ay, bx, by, r) => Region::Capsule(ax, ay, bx, by, r + g),
            Region::Rect(r, m) => Region::Rect(r, m + g),
        }
    }
}

/// Share of rect `r` (x, y, w, h) inside any region, on a 24x24 sample grid.
fn reach_share(r: [f64; 4], regions: &[Region]) -> f64 {
    const N: usize = 24;
    let mut hit = 0;
    for j in 0..N {
        for i in 0..N {
            let x = r[0] + r[2] * (i as f64 + 0.5) / N as f64;
            let y = r[1] + r[3] * (j as f64 + 0.5) / N as f64;
            if regions.iter().any(|g| g.contains(x, y)) {
                hit += 1;
            }
        }
    }
    hit as f64 / (N * N) as f64
}

/// Where a fluid's dye can be by time `t_end` (scene px), from its emitters; `rect` gives a layer's
/// box (ink sources). Empty when the fluid has no dye source at all.
fn dye_reach(cfg: &FluidCfg, bx: [f64; 4], t_end: f64, rect: &dyn Fn(&str) -> Option<[f64; 4]>) -> Vec<Region> {
    // the flow keeps carrying dye a while after a source stops: velocity decays with drag
    // (kept per 1/30 s) and with the pressure solve spreading the momentum; cap that coast at 1.5 s
    let coast = |t1: f64| (t_end - t1).clamp(0.0, 1.5);
    let mut regs: Vec<Region> = vec![];
    for e in &cfg.emitters {
        match e {
            FluidEv::Pour { t0, t1, at, dir, speed, radius, .. } => {
                if *t0 >= t_end {
                    continue;
                }
                let (px, py) = (bx[0] + resolve_len(&at.0, bx[2]).unwrap_or(0.0), bx[1] + resolve_len(&at.1, bx[3]).unwrap_or(0.0));
                let rad = resolve_len(radius, bx[2]).unwrap_or(10.0).max(0.5);
                let active = t1.min(t_end) - t0;
                // measured: a 500 px/s jet for 1 s carries dye ~210 px in 1 s and ~450 px by 4 s;
                // 0.7 x speed x (active + coast) is about twice that
                let len = speed.abs() * 0.7 * (active + coast(*t1));
                let a = dir.to_radians();
                let (ux, uy) = (a.cos(), a.sin());
                let (ex, ey) = (px + ux * len, py + uy * len);
                // a jet spreads sideways (wobble, entrainment): cone half-width ~0.45 of its length
                regs.push(Region::Capsule(px, py, ex, ey, rad * 3.0 + 0.45 * len));
                // where the jet runs into a wall it fans out along it by the remaining length
                let (cx, cy) = (ex.clamp(bx[0], bx[0] + bx[2]), ey.clamp(bx[1], bx[1] + bx[3]));
                let over = ((ex - cx).powi(2) + (ey - cy).powi(2)).sqrt();
                if over > 0.0 {
                    regs.push(Region::Disc(cx, cy, over + rad * 3.0 + 0.45 * len));
                }
            }
            FluidEv::Source { t0, t1, id, speed, dir, noise, .. } => {
                if *t0 >= t_end {
                    continue;
                }
                let Some(r) = rect(id) else { continue };
                let active = t1.min(t_end) - t0;
                let m = KICK_SCALE * 1.5 + noise.abs() * 0.4;
                let len = speed.abs() * 0.7 * (active + coast(*t1));
                regs.push(Region::Rect(r, m));
                if len > 1.0 {
                    let a = dir.to_radians();
                    let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
                    let half = 0.5 * (r[2] * r[2] + r[3] * r[3]).sqrt();
                    regs.push(Region::Capsule(cx, cy, cx + a.cos() * len, cy + a.sin() * len, half + m + 0.45 * len));
                }
            }
            FluidEv::Stir { .. } => {}
        }
    }
    if regs.is_empty() {
        return regs;
    }
    // stirs spread whatever dye they touch over their disc (the vortex reaches ~2 radii)
    for _ in 0..2 {
        for e in &cfg.emitters {
            if let FluidEv::Stir { t0, at, radius, .. } = e {
                if *t0 >= t_end {
                    continue;
                }
                let (cx, cy) = (bx[0] + resolve_len(&at.0, bx[2]).unwrap_or(0.0), bx[1] + resolve_len(&at.1, bx[3]).unwrap_or(0.0));
                let r = resolve_len(radius, bx[2]).unwrap_or(100.0).max(0.5) * 2.0;
                let disc = Region::Disc(cx, cy, r);
                let touches = regs.iter().any(|g| {
                    let b = g.bbox();
                    let (nx, ny) = (cx.clamp(b[0], b[2]), cy.clamp(b[1], b[3]));
                    (nx - cx).powi(2) + (ny - cy).powi(2) <= r * r
                });
                if touches {
                    regs.push(disc);
                }
            }
        }
    }
    // turbulence: seeded curl noise everywhere, eddies of TURB_SCALE px
    if cfg.turbulence > 0.0 {
        let g = TURB_SCALE.min(cfg.turbulence * 2.0);
        regs = regs.iter().map(|r| r.grow(g)).collect();
    }
    // buoyancy carries dense dye up (negative: down), all the way to the wall
    if cfg.buoyancy != 0.0 {
        let extra: Vec<Region> = regs
            .iter()
            .map(|g| {
                let b = g.bbox();
                if cfg.buoyancy > 0.0 {
                    Region::Rect([b[0], bx[1], b[2] - b[0], b[3] - bx[1]], 0.0)
                } else {
                    Region::Rect([b[0], b[1], b[2] - b[0], bx[1] + bx[3] - b[1]], 0.0)
                }
            })
            .collect();
        regs.extend(extra);
    }
    // vorticity confinement curls the edges outward a little
    regs.iter().map(|r| r.grow(cfg.vorticity.max(0.0) * 2.0)).collect()
}

/// `W_REVEAL_NO_INK` for every `reveal #el with #fluid` (mode=alpha) whose element the dye
/// cannot reach before its scene ends. `rect` gives a layer's laid-out box (scene px).
pub fn reveal_rules(ir: &Ir, tl: &Timeline, rect: &dyn Fn(&str) -> Option<[f64; 4]>) -> Vec<Diag> {
    let mut fluids: Vec<&Layer> = vec![];
    let mut targets: Vec<&Layer> = vec![];
    ir.root.walk(&mut |l| {
        if matches!(l.kind, Kind::Fluid(_)) {
            fluids.push(l);
        }
        if l.style.fluid_matte.as_ref().is_some_and(|(_, m)| m == "alpha") {
            targets.push(l);
        }
    });
    let mut d = vec![];
    for tgt in targets {
        let Some((fid, _)) = &tgt.style.fluid_matte else { continue };
        let Some(fl) = fluids.iter().find(|l| &l.id == fid) else { continue };
        let Kind::Fluid(cfg) = &fl.kind else { continue };
        let name = tgt.name.clone().unwrap_or(tgt.id.clone());
        let fname = fl.name.clone().unwrap_or(fl.id.clone());
        let (Some(bx), Some(tr)) = (rect(&fl.id), rect(&tgt.id)) else { continue };
        if tr[2] <= 0.0 || tr[3] <= 0.0 {
            continue;
        }
        // the scene the dye lives in: the one the first emitter (or the video) starts in
        let t_first = cfg.emitters.iter().map(|e| match e {
            FluidEv::Pour { t0, .. } | FluidEv::Stir { t0, .. } | FluidEv::Source { t0, .. } => *t0,
        }).fold(f64::MAX, f64::min);
        let t_end = if t_first < f64::MAX { tl.scenes.get(tl.scene_at(t_first)).map(|s| s.start + s.dur).unwrap_or(tl.total) } else { tl.total };
        let hint_sim = "this is a geometric estimate of where the ink can flow (no simulation): check with `agentfx frame --at <scene end>`";
        // the part of the target inside the fluid's box (a matte is empty outside it)
        let ix0 = tr[0].max(bx[0]);
        let iy0 = tr[1].max(bx[1]);
        let ix1 = (tr[0] + tr[2]).min(bx[0] + bx[2]);
        let iy1 = (tr[1] + tr[3]).min(bx[1] + bx[3]);
        if ix1 <= ix0 || iy1 <= iy0 {
            d.push(Diag::warn("W_REVEAL_NO_INK", format!("reveal: `#{name}` lies outside fluid `#{fname}`'s box, so it is never revealed"), "move the element into the fluid's box or size the fluid to cover it (a fluid without x/y/w/h fills the frame)", Default::default()).with_id(name));
            continue;
        }
        let regs = dye_reach(cfg, bx, t_end, rect);
        if regs.is_empty() {
            d.push(Diag::warn("W_REVEAL_NO_INK", format!("reveal: fluid `#{fname}` gets no dye (no `pour` or `ink ... into #{fname}`), so `#{name}` is never revealed"), format!("add a dye source aimed at `#{name}`: `@0s pour #{fname} at=50%,90% dir=-90deg speed=600 dur=2s`"), Default::default()).with_id(name));
            continue;
        }
        let share = reach_share([ix0, iy0, ix1 - ix0, iy1 - iy0], &regs);
        if share < MIN_REACH {
            let (cx, cy) = ((tr[0] + tr[2] / 2.0 - bx[0]) / bx[2].max(1.0) * 100.0, (tr[1] + tr[3] / 2.0 - bx[1]) / bx[3].max(1.0) * 100.0);
            d.push(Diag::warn("W_REVEAL_NO_INK", format!("reveal: the ink of `#{fname}` never reaches `#{name}` by the end of its scene ({:.0}% of it within reach), so it stays hidden", share * 100.0),
                format!("aim a pour at it (`pour #{fname} at={cx:.0}%,{:.0}% dir=-90deg`), raise speed=/dur=, or add a `stir` over it; {hint_sim}", (cy + 15.0).min(100.0)), Default::default()).with_id(name).with_at(t_end));
        }
    }
    d
}

/// `reveal_rules` on the native layout (one Blitz load; only when the IR has a reveal).
pub fn reveal_rules_native(ir: &Ir, tl: &Timeline) -> Result<Vec<Diag>, String> {
    let mut any = false;
    ir.root.walk(&mut |l| any |= l.style.fluid_matte.is_some());
    if !any {
        return Ok(vec![]);
    }
    let tmp = crate::run::tmp_dir();
    let _ = std::fs::create_dir_all(&tmp);
    let nat = crate::native::Native::load(ir, crate::native::snap_at(ir, 0.0), &crate::run::home().join("assets/fonts"), &tmp, false).map_err(|e| format!("native layout failed (in {}): {e:#}", tmp.display()))?;
    Ok(reveal_rules(ir, tl, &|id| nat.rect(id)))
}
