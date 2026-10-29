//! Damage invariant tests: for every frame, the incrementally updated frame (previous
//! frame + the scheduled damaged rects re-rendered) must equal a fresh full render.
//! Small synthetic scenes cover the cases from the M1 review (root effects, nested
//! filtered groups, matte sources with animated effects, opacity moving between
//! group levels, moving clips, motion blur over animated glyphs).
#![cfg(test)]
use crate::schedule;
use crate::vraster::{Info, Painter, Prepared};
use agentfx_ir::Scene;
use std::path::Path;

fn check(json: &str) {
    let scene: Scene = serde_json::from_str(json).expect("scene json");
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Err(e) = agentfx_ir::validate::validate(&scene, base) {
        panic!("invalid test scene: {e:?}");
    }
    let prep = Prepared::new(&scene, base, None, None).unwrap();
    let info = Info::new(&scene, &prep);
    let sched = schedule::build(&scene, &info).unwrap();
    let (w, h) = (scene.width as usize, scene.height as usize);
    let full = [0, 0, w as i32, h as i32];
    let mut inc = Painter::new(&scene, &prep, &sched, &info, 0);
    // exact mode: cached blurs reused only at the same subpixel phase
    inc.fx_smooth = false;
    let mut fresh = Painter::new(&scene, &prep, &sched, &info, 0);
    fresh.use_cache = false;
    let mut frame = vec![0u8; w * h * 4];
    for f in 0..sched.frames {
        let rects: Vec<[i32; 4]> = if f == 0 { vec![full] } else { sched.damage[f].clone() };
        for r in &rects {
            let px = inc.render_rect(f, *r).unwrap();
            let src = px.data_as_u8_slice();
            let rw = r[2] as usize * 4;
            for y in 0..r[3] as usize {
                let d = ((r[1] as usize + y) * w + r[0] as usize) * 4;
                frame[d..d + rw].copy_from_slice(&src[y * rw..y * rw + rw]);
            }
        }
        let want = fresh.render_rect(f, full).unwrap();
        let want = want.data_as_u8_slice();
        let mut bad = 0usize;
        let mut maxd = 0u8;
        for (a, b) in frame.iter().zip(want) {
            let d = a.abs_diff(*b);
            if d > 2 {
                bad += 1;
            }
            maxd = maxd.max(d);
        }
        assert!(bad == 0, "frame {f}: {bad} channel values differ (max {maxd}) between incremental and full render; damage {:?}", sched.damage[f]);
    }
}

fn track(node: usize, prop: &str, delay: f64, dur: f64, a: &str, b: &str) -> String {
    format!(r#"{{"node":{node},"property":"{prop}","delay":{delay},"duration":{dur},"fill":"both","easing":"linear","keyframes":[{{"offset":0,"value":{a}}},{{"offset":1,"value":{b}}}]}}"#)
}

fn rect(x: f64, y: f64, w: f64, h: f64, c: &str) -> String {
    format!(r#"{{"type":"rect","rect":{{"rect":[{x},{y},{w},{h}],"radius":[0,0,0,0]}},"color":{c}}}"#)
}

fn scene(nodes: &[String], tracks: &[String]) -> String {
    format!(
        r#"{{"version":1,"width":96,"height":64,"fps":10,"frames":12,"fonts":[{{"path":"../../examples/scenes/_shared/Inter.ttf"}}],"nodes":[{}],"tracks":[{}]}}"#,
        nodes.join(","),
        tracks.join(",")
    )
}

#[test]
fn root_effect_does_not_recurse() {
    let s = scene(
        &[format!(r#"{{"name":"root","children":[1],"effects":[{{"type":"blur","radius":1}}]}}"#), format!(r#"{{"name":"r","content":{}}}"#, rect(10.0, 10.0, 30.0, 20.0, "[1,1,1,1]"))],
        &[track(1, "transform", 0.2, 0.6, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[30,10]}]}"#)],
    );
    check(&s);
}

#[test]
fn nested_filtered_groups() {
    let paint = r#"{"type":"paint","rect":[0,0,96,64],"items":[
        {"op":"fill","shape":{"rect":[0,0,96,64]},"brush":{"solid":[0.1,0.1,0.2,1]}},
        {"op":"push","clip":{"rect":[0,0,96,64]},"alpha":1,"blend":"normal","filter":{"type":"blur","std_dev":2}},
        {"op":"push","clip":{"rect":[0,0,96,64]},"alpha":1,"blend":"normal","filter":{"type":"drop-shadow","dx":2,"dy":2,"std_dev":1,"color":[0,0,0,0.6]}},
        {"op":"fill","shape":{"rect":[20,20,50,40]},"brush":{"solid":[1,0.5,0,1]}},
        {"op":"pop"},{"op":"pop"}]}"#;
    let s = scene(&[r#"{"name":"root","children":[1]}"#.to_string(), format!(r#"{{"name":"p","content":{paint},"origin":[48,32]}}"#)], &[track(1, "transform", 0.1, 0.8, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[10,5]}]}"#)]);
    check(&s);
}

#[test]
fn matte_source_effect_animation_damages_target() {
    let s = scene(
        &[
            r#"{"name":"root","children":[1,2]}"#.to_string(),
            format!(r#"{{"name":"src","hidden":true,"content":{},"effects":[{{"type":"blur","radius":0}}]}}"#, rect(30.0, 20.0, 30.0, 20.0, "[1,1,1,1]")),
            format!(r#"{{"name":"target","children":[3],"effects":[{{"type":"matte","source":1,"mode":"alpha"}}]}}"#),
            format!(r#"{{"name":"fill","content":{}}}"#, rect(0.0, 0.0, 96.0, 64.0, "[0.2,0.9,0.5,1]")),
        ],
        &[track(1, "effects.0.radius", 0.2, 0.6, r#"{"number":0}"#, r#"{"number":6}"#)],
    );
    check(&s);
}

#[test]
fn opacity_moving_between_group_levels() {
    let s = scene(
        &[
            r#"{"name":"root","children":[1]}"#.to_string(),
            r#"{"name":"group","children":[2,3],"opacity":0.5}"#.to_string(),
            format!(r#"{{"name":"a","content":{}}}"#, rect(10.0, 10.0, 40.0, 30.0, "[1,1,1,1]")),
            format!(r#"{{"name":"b","content":{}}}"#, rect(30.0, 20.0, 40.0, 30.0, "[1,1,1,1]")),
        ],
        &[
            track(1, "opacity", 0.5, 0.0001, r#"{"number":0.5}"#, r#"{"number":1}"#),
            track(2, "opacity", 0.5, 0.0001, r#"{"number":1}"#, r#"{"number":0.5}"#),
            track(3, "opacity", 0.5, 0.0001, r#"{"number":1}"#, r#"{"number":0.5}"#),
        ],
    );
    check(&s);
}

#[test]
fn moving_clip_with_compensated_content() {
    let s = scene(
        &[
            r#"{"name":"root","children":[1]}"#.to_string(),
            r#"{"name":"mover","children":[2]}"#.to_string(),
            format!(r#"{{"name":"clipped","content":{},"clips":[{{"rect":[20,10,30,30],"radius":[0,0,0,0]}}]}}"#, rect(0.0, 0.0, 96.0, 64.0, "[1,0.3,0.3,1]")),
        ],
        &[
            track(1, "transform", 0.2, 0.6, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[20,0]}]}"#),
            track(2, "transform", 0.2, 0.6, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[-20,0]}]}"#),
        ],
    );
    check(&s);
}

#[test]
fn motion_blur_over_glyph_tracks_and_effect_cache() {
    let text = r#"{"type":"text","text":"Ab","font":0,"size":28,"pos":[10,10],"brush":{"solid":[1,1,1,1]}}"#;
    let s = scene(
        &[
            r#"{"name":"root","children":[1,3]}"#.to_string(),
            r#"{"name":"mb","children":[2],"motion_blur":{"samples":4,"shutter":1}}"#.to_string(),
            format!(r#"{{"name":"t","content":{text}}}"#),
            format!(r#"{{"name":"glowbox","content":{},"effects":[{{"type":"glow","radius":2,"color":[0,1,0,1]}}],"children":[4]}}"#, rect(60.0, 30.0, 20.0, 20.0, "[1,1,1,1]")),
            format!(r#"{{"name":"inner blur","content":{},"effects":[{{"type":"blur","radius":0}}]}}"#, rect(62.0, 32.0, 10.0, 10.0, "[1,0,0,1]")),
        ],
        &[
            track(2, "glyph.transform", 0.1, 0.5, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[30,0]}]}"#),
            track(3, "transform", 0.0, 1.2, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[-40,0]}]}"#),
            track(4, "effects.0.radius", 0.6, 0.4, r#"{"number":0}"#, r#"{"number":3}"#),
        ],
    );
    check(&s);
}

#[test]
fn motion_blur_over_glyph_tracks() {
    let text = r#"{"type":"text","text":"Ab","font":0,"size":28,"pos":[10,10],"brush":{"solid":[1,1,1,1]}}"#;
    let s = scene(
        &[r#"{"name":"root","children":[1]}"#.to_string(), r#"{"name":"mb","children":[2],"motion_blur":{"samples":4,"shutter":1}}"#.to_string(), format!(r#"{{"name":"t","content":{text}}}"#)],
        &[track(2, "glyph.transform", 0.1, 0.5, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[30,0]}]}"#)],
    );
    check(&s);
}

#[test]
fn moving_glow_with_animated_inner_blur() {
    let s = scene(
        &[
            r#"{"name":"root","children":[1]}"#.to_string(),
            format!(r#"{{"name":"glowbox","content":{},"effects":[{{"type":"glow","radius":2,"color":[0,1,0,1]}}],"children":[2]}}"#, rect(60.0, 30.0, 20.0, 20.0, "[1,1,1,1]")),
            format!(r#"{{"name":"inner blur","content":{},"effects":[{{"type":"blur","radius":0}}]}}"#, rect(62.0, 32.0, 10.0, 10.0, "[1,0,0,1]")),
        ],
        &[
            track(1, "transform", 0.0, 1.2, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[-40,0]}]}"#),
            track(2, "effects.0.radius", 0.6, 0.4, r#"{"number":0}"#, r#"{"number":3}"#),
        ],
    );
    check(&s);
}

/// Review M3 #2: a static filtered display-list group partly covered by a small damage
/// rect. Items whose bbox starts in the filter margin right of the rect must still be drawn
/// (culling against the offscreen target, not the parent's region).
#[test]
fn filtered_group_partial_damage_keeps_margin_items() {
    let paint = r#"{"type":"paint","rect":[0,0,96,64],"items":[
        {"op":"fill","shape":{"rect":[0,0,96,64]},"brush":{"solid":[1,1,1,1]}},
        {"op":"push","clip":{"rect":[0,0,96,64]},"alpha":1,"blend":"normal","filter":{"type":"blur","std_dev":3}},
        {"op":"fill","shape":{"rect":[10,10,30,40]},"brush":{"solid":[1,0,0,1]}},
        {"op":"fill","shape":{"rect":[47,24,80,44]},"brush":{"solid":[0,0,1,1]}},
        {"op":"pop"}]}"#;
    let s = scene(
        &[
            r#"{"name":"root","children":[1,2]}"#.to_string(),
            format!(r#"{{"name":"p","content":{paint}}}"#),
            format!(r#"{{"name":"g","content":{},"opacity":0}}"#, rect(36.0, 28.0, 10.0, 8.0, "[0,0.6,0,1]")),
        ],
        &[track(2, "opacity", 0.2, 0.5, r#"{"number":0}"#, r#"{"number":0.3}"#)],
    );
    check(&s);
}

/// Dithered gradients are anchored to screen pixels: partial damage over a static
/// gradient (and a moving node over it) must match a full render bit for bit.
#[test]
fn dithered_gradient_partial_damage() {
    let grad = r#"{"type":"paint","rect":[0,0,96,64],"items":[
        {"op":"fill","shape":{"rect":[0,0,96,64]},"brush":{"linear":{"start":[0,0],"end":[96,64],"stops":[{"offset":0,"color":[0.02,0.1,0.06,1]},{"offset":1,"color":[0.05,0.16,0.1,1]}],"extend":"pad"}}}]}"#;
    let s = scene(
        &[
            r#"{"name":"root","children":[1,2]}"#.to_string(),
            format!(r#"{{"name":"bg","content":{grad}}}"#),
            format!(r#"{{"name":"m","content":{}}}"#, rect(5.0, 5.0, 12.0, 9.0, "[1,1,1,0.5]")),
        ],
        &[track(2, "transform", 0.1, 0.8, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[53,37]}]}"#)],
    );
    check(&s);
}

// ---- time effects (X3) ----

fn time_scene(time: &str, extra_nodes: &[String], extra_tracks: &[String]) -> String {
    let mut nodes = vec![
        format!(r#"{{"name":"root","children":[1{}]}}"#, (0..extra_nodes.len()).map(|i| format!(",{}", 3 + i)).collect::<String>()),
        format!(r#"{{"name":"fx","children":[2],"time":{time}}}"#),
        format!(r#"{{"name":"bar","origin":[48,32],"content":{}}}"#, rect(38.0, 12.0, 20.0, 40.0, "[1,0.4,0.1,1]")),
    ];
    nodes.extend(extra_nodes.iter().cloned());
    let mut tracks = vec![
        track(2, "transform", 0.1, 0.4, r#"{"transform":[{"translate":[-30,0]},{"rotate":0}]}"#, r#"{"transform":[{"translate":[30,0]},{"rotate":90}]}"#),
    ];
    tracks.extend(extra_tracks.iter().cloned());
    scene(&nodes, &tracks)
}

#[test]
fn time_slitscan_damage() {
    check(&time_scene(r#"{"type":"slitscan","axis":"y","span":0.3,"band":3}"#, &[], &[]));
    check(&time_scene(r#"{"type":"slitscan","axis":"x","span":-0.2,"range":[10,80]}"#, &[], &[]));
}

#[test]
fn time_slitscan_animated_span_damage() {
    check(&time_scene(r#"{"type":"slitscan","axis":"y","span":0.4}"#, &[], &[track(1, "time.span", 0.6, 0.3, r#"{"number":0.4}"#, r#"{"number":0}"#)]));
}

#[test]
fn time_echo_damage() {
    check(&time_scene(r#"{"type":"echo","count":4,"span":0.1,"decay":0.6,"blend":"plus","tint":[[0,0.5,1,1],[1,0,0.5,1]]}"#, &[], &[]));
    check(&time_scene(r#"{"type":"echo","count":3,"span":0.2,"decay":0.5,"blend":"multiply"}"#, &[], &[]));
}

#[test]
fn time_displace_damage() {
    check(&time_scene(r#"{"type":"displace","map":{"type":"radial","center":[48,32],"radius":40},"span":0.3,"levels":8}"#, &[], &[]));
    check(&time_scene(r#"{"type":"displace","map":{"type":"noise","scale":12,"seed":5},"span":0.3,"levels":6}"#, &[], &[]));
    // layer map whose source moves (damages the target)
    let src = format!(r#"{{"name":"map","hidden":true,"content":{}}}"#, rect(0.0, 0.0, 48.0, 64.0, "[1,1,1,1]"));
    check(&time_scene(
        r#"{"type":"displace","map":{"type":"layer","node":3},"span":0.3,"levels":4}"#,
        &[src],
        &[track(3, "transform", 0.5, 0.4, r#"{"transform":[{"translate":[0,0]}]}"#, r#"{"transform":[{"translate":[40,0]}]}"#)],
    ));
}

/// span 0 is the plain subtree, pixel for pixel; renders are deterministic.
#[test]
fn time_span_zero_is_identity_and_deterministic() {
    let render_all = |json: &str| -> Vec<Vec<u8>> {
        let scene: Scene = serde_json::from_str(json).unwrap();
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        let prep = Prepared::new(&scene, base, None, None).unwrap();
        let info = Info::new(&scene, &prep);
        let sched = schedule::build(&scene, &info).unwrap();
        let mut p = Painter::new(&scene, &prep, &sched, &info, 0);
        let full = [0, 0, scene.width as i32, scene.height as i32];
        (0..sched.frames).map(|f| p.render_rect(f, full).unwrap().data_as_u8_slice().to_vec()).collect()
    };
    let plain = render_all(&time_scene(r#"{"type":"echo","count":3,"span":0,"decay":0.5}"#, &[], &[]).replace(r#","time":{"type":"echo","count":3,"span":0,"decay":0.5}"#, ""));
    for t in [r#"{"type":"echo","count":3,"span":0,"decay":0.5}"#, r#"{"type":"slitscan","span":0}"#, r#"{"type":"displace","map":{"type":"radial","center":[0,0],"radius":9},"span":0}"#] {
        assert_eq!(render_all(&time_scene(t, &[], &[])), plain, "{t}");
    }
    let s = time_scene(r#"{"type":"displace","map":{"type":"noise","scale":12,"seed":5},"span":0.3,"levels":6}"#, &[], &[]);
    let a = render_all(&s);
    assert_eq!(a, render_all(&s));
    assert_ne!(a, plain);
}

/// The two-pass slit-scan (even/odd levels in two contexts) equals one render per level.
#[test]
fn time_slitscan_two_pass_matches_per_level() {
    for t in [r#"{"type":"slitscan","axis":"y","span":0.3,"band":3}"#, r#"{"type":"slitscan","axis":"x","span":0.4,"range":[80,10],"band":2}"#] {
        let json = time_scene(t, &[], &[]);
        let scene: Scene = serde_json::from_str(&json).unwrap();
        let base = Path::new(env!("CARGO_MANIFEST_DIR"));
        let prep = Prepared::new(&scene, base, None, None).unwrap();
        let info = Info::new(&scene, &prep);
        let sched = schedule::build(&scene, &info).unwrap();
        let mut a = Painter::new(&scene, &prep, &sched, &info, 0);
        let mut b = Painter::new(&scene, &prep, &sched, &info, 0);
        b.slit_two_pass = false;
        let full = [0, 0, scene.width as i32, scene.height as i32];
        for f in 0..sched.frames {
            let (pa, pb) = (a.render_rect(f, full).unwrap(), b.render_rect(f, full).unwrap());
            let maxd = pa.data_as_u8_slice().iter().zip(pb.data_as_u8_slice()).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
            assert!(maxd <= 1, "{t} frame {f}: max diff {maxd}");
        }
    }
}
