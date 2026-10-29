// Motion-energy map on a hand-built IR (no layout engine: rects are given).
use agentfx_script::energy::{compute, union_area, EnergyMap};
use agentfx_script::ir::*;
use agentfx_script::values::{Ease, Rgba};
use std::collections::HashMap;

fn ir_with(children: Vec<Layer>, tracks: Vec<Track>, dur: f64) -> Ir {
    let mut root = Layer::new("root", Kind::Group);
    root.children = children;
    Ir {
        version: "test", format: "16:9".into(), w: 1920, h: 1080, fps: 30.0, duration: dur,
        bg: Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }, fonts: vec![], root, tracks, hooks: vec![], audio: vec![], quant: Quant::default(),
    }
}

fn run(ir: &Ir, rects: &[(&str, [f64; 4])]) -> EnergyMap {
    let m: HashMap<String, [f64; 4]> = rects.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    compute(ir, &|id| m.get(id).copied())
}

fn slide(layer: &str, ch: Ch, from: f64, to: f64, t0: f64, t1: f64) -> Track {
    let mut tr = Track::new(layer, ch, TV::N(from));
    tr.to(t0, t1, TV::N(to), Ease::Linear);
    tr
}

#[test]
fn union_counts_overlap_once() {
    let a = union_area(&[[0.0, 0.0, 100.0, 100.0], [50.0, 50.0, 100.0, 100.0]], 1000.0, 1000.0);
    assert!((a - 17500.0).abs() < 1e-6, "{a}");
    // clipped to the frame
    assert!((union_area(&[[-50.0, -50.0, 100.0, 100.0]], 1000.0, 1000.0) - 2500.0).abs() < 1e-6);
    assert_eq!(union_area(&[], 10.0, 10.0), 0.0);
}

#[test]
fn static_scene_has_zero_energy() {
    let ir = ir_with(vec![Layer::new("a", Kind::Rect)], vec![], 2.0);
    let e = run(&ir, &[("root", [0.0, 0.0, 1920.0, 1080.0]), ("a", [100.0, 100.0, 400.0, 300.0])]);
    assert_eq!(e.len(), 60);
    assert!(e.frames.iter().all(|x| *x == 0.0));
    assert_eq!(e.first_motion(1e-4), None);
    assert_eq!(e.still_runs(1e-4, 1.0), vec![(0.0, 2.0)]);
}

#[test]
fn moving_box_energy_is_swept_area_and_runs_are_found() {
    // 400x300 box slides 30 px/frame in x between 1.0s and 2.0s
    let tr = slide("a", Ch::X, 0.0, 900.0, 1.0, 2.0);
    let ir = ir_with(vec![Layer::new("a", Kind::Rect)], vec![tr], 3.0);
    let e = run(&ir, &[("root", [0.0, 0.0, 1920.0, 1080.0]), ("a", [100.0, 100.0, 400.0, 300.0])]);
    assert_eq!(e.len(), 90);
    assert_eq!(e.frames[0], 0.0);
    assert_eq!(e.frames[20], 0.0, "before the move");
    let mid = e.frames[45];
    let expect = ((400.0 + 30.0) * 300.0 / (1920.0 * 1080.0)) as f32;
    assert!((mid - expect).abs() < 0.002, "{mid} vs {expect}");
    assert_eq!(e.frames[75], 0.0, "after the move");
    let first = e.first_motion(1e-4).unwrap();
    assert!((first - 1.0).abs() < 0.1, "{first}");
    let runs = e.moving_runs(1e-4, 0.1);
    assert_eq!(runs.len(), 1);
    assert!((runs[0].0 - 1.0).abs() < 0.1 && (runs[0].1 - 2.0).abs() < 0.1, "{runs:?}");
    let still = e.still_runs(1e-4, 0.5);
    assert_eq!(still.len(), 2, "{still:?}");
    assert!(e.max_in(0.0, 0.9) == 0.0 && e.max_in(1.2, 1.8) > 0.05);
}

#[test]
fn camera_move_on_a_parent_damages_every_child() {
    let mut g = Layer::new("g", Kind::Group);
    g.children = vec![Layer::new("a", Kind::Rect), Layer::new("b", Kind::Rect)];
    let tr = slide("g", Ch::Scale, 1.0, 1.5, 0.0, 1.0);
    let ir = ir_with(vec![g], vec![tr], 1.0);
    let e = run(&ir, &[("root", [0.0, 0.0, 1920.0, 1080.0]), ("g", [0.0, 0.0, 1920.0, 1080.0]), ("a", [0.0, 0.0, 960.0, 1080.0]), ("b", [960.0, 0.0, 960.0, 1080.0])]);
    assert!(e.frames[15] > 0.9, "{}", e.frames[15]);
}

#[test]
fn invisible_layers_and_tiny_motion() {
    // a fully transparent big box moving is not motion; a 20x20 dot is, but tiny
    let mut big = Layer::new("big", Kind::Rect);
    big.style.origin = (0.5, 0.5);
    let dot = Layer::new("dot", Kind::Ellipse);
    let mut op = Track::new("big", Ch::Opacity, TV::N(0.0));
    op.keys.push(Key { t: 0.0, v: TV::N(0.0), ease: Ease::Linear });
    op.keys.push(Key { t: 1.0, v: TV::N(0.0), ease: Ease::Linear });
    let tracks = vec![op, slide("big", Ch::X, 0.0, 500.0, 0.0, 1.0), slide("dot", Ch::X, 0.0, 300.0, 0.0, 1.0)];
    let ir = ir_with(vec![big, dot], tracks, 1.0);
    let e = run(&ir, &[("root", [0.0, 0.0, 1920.0, 1080.0]), ("big", [0.0, 0.0, 1920.0, 1080.0]), ("dot", [100.0, 100.0, 20.0, 20.0])]);
    let m = e.frames.iter().copied().fold(0.0, f32::max);
    assert!(m > 0.0 && m < 0.01, "{m}");
}

#[test]
fn strip_and_json_shape() {
    let e = EnergyMap::new(10.0, 100, 100, vec![0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0]);
    let s = e.strip(4);
    assert_eq!(s.chars().count(), 4);
    assert_eq!(s.chars().next().unwrap(), ' ');
    assert_eq!(s.chars().nth(1).unwrap() != ' ', true);
    assert_eq!(e.to_json()["frames"].as_array().unwrap().len(), 8);
    assert_eq!(e.moving_runs(1e-4, 0.0).len(), 1);
}
