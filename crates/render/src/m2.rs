//! M2 HTML frontend helpers: evaluation of all tracks at a time as inline styles, and a
//! brute-force snapshot renderer (whole document laid out + painted natively per frame),
//! used to measure Blitz fidelity independently of the planner.
use crate::blitzfe::BlitzScene;
use crate::cssval;
use crate::model::{Compiled, ElemId, Stage};
use crate::record::Resources;
use anyhow::{Context, Result};
use agentfx_ir::eval::{eval_segment, eval_track};
use agentfx_ir::{Content, Sampled, TransformFn, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// CSS text of an evaluated composite value.
fn composite_css(prop: &str, s: &Sampled, tr: &agentfx_ir::Track, t: f64) -> Option<String> {
    Some(match (prop, s) {
        ("opacity", Sampled::Number(v)) => format!("{v}"),
        ("translate" | "rotate" | "scale", _) => {
            // individual properties: evaluate the function list directly
            let (k, u) = eval_segment(tr, t)?;
            let (a, b) = (&tr.keyframes[k].value, &tr.keyframes[k + 1].value);
            let (Value::Transform(a), Value::Transform(b)) = (a, b) else { return None };
            let l = |x: f64, y: f64| x + (y - x) * u;
            match (a.first()?, b.first()?) {
                (TransformFn::Translate(x0, y0), TransformFn::Translate(x1, y1)) => format!("{}px {}px", l(*x0, *x1), l(*y0, *y1)),
                (TransformFn::Rotate(a0), TransformFn::Rotate(a1)) => format!("{}deg", l(*a0, *a1)),
                (TransformFn::Scale(x0, y0), TransformFn::Scale(x1, y1)) => format!("{} {}", l(*x0, *x1), l(*y0, *y1)),
                _ => return None,
            }
        }
        (_, Sampled::Matrix(m)) => format!("matrix({}, {}, {}, {}, {}, {})", m.a, m.b, m.c, m.d, m.e, m.f),
        _ => return None,
    })
}

/// Inline style values of every track at time `t` (later `order` wins per element and
/// property), and typed-text contents. `composite`: include composite tracks.
pub fn state_at(compiled: &Compiled, ir: &[agentfx_ir::Track], t: f64, composite: bool) -> Result<(Vec<(ElemId, String, String)>, Vec<(ElemId, String)>)> {
    let mut best: BTreeMap<(ElemId, String), (usize, String)> = BTreeMap::new();
    let mut texts = Vec::new();
    for (i, tr) in compiled.tracks.iter().enumerate() {
        if tr.stage == Stage::Composite && !composite {
            continue;
        }
        if tr.property == "text" {
            let n = match eval_track(&ir[i], t) {
                Some(Sampled::Number(v)) => (v + 1e-9).floor().max(0.0) as usize,
                _ => 0,
            };
            let tt = compiled.texts.iter().find(|x| x.elem == tr.elem).unwrap();
            texts.push((tr.elem, tt.text.chars().take(n).collect()));
            continue;
        }
        let v = if tr.stage == Stage::Composite {
            let Some(s) = eval_track(&ir[i], t) else { continue };
            let Some(v) = composite_css(&tr.property, &s, &ir[i], t) else { continue };
            v
        } else {
            let Some((k, u)) = eval_segment(&ir[i], t) else { continue };
            let (a, b) = (&tr.keyframes[k].value, &tr.keyframes[k + 1].value);
            match (a, b) {
                (Value::Discrete(a), Value::Discrete(b)) => cssval::interpolate(a, b, u).with_context(|| format!("element #{} {}", tr.elem, tr.property))?,
                _ => match eval_track(&ir[i], t) {
                    Some(Sampled::Number(x)) => format!("{x}"),
                    _ => continue,
                },
            }
        };
        let key = (tr.elem, tr.property.clone());
        if best.get(&key).is_none_or(|x| x.0 <= tr.order) {
            best.insert(key, (tr.order, v));
        }
    }
    Ok((best.into_iter().map(|((e, p), (_, v))| (e, p, v)).collect(), texts))
}

/// `agentfx snap`: the whole document at each frame of `frames`, laid out and painted natively
/// (no planner, no caching) -> `<out_dir>/f<n>.png`.
pub fn snap(scene_dir: &Path, out_dir: &Path, frames: &[usize]) -> Result<serde_json::Value> {
    let compiled = crate::css::compile_scene(scene_dir).map_err(|e| anyhow::anyhow!("unsupported CSS:\n  - {}", e.join("\n  - ")))?;
    let mut bz = BlitzScene::load(scene_dir, &compiled)?;
    let mut res = Resources::new(out_dir);
    bz.image_sources(&mut res);
    let ir: Vec<agentfx_ir::Track> = compiled.tracks.iter().map(|t| t.to_ir(0)).collect();
    let mut times = Vec::new();
    for &f in frames {
        let t0 = std::time::Instant::now();
        let (props, texts) = state_at(&compiled, &ir, f as f64 / compiled.fps, true)?;
        bz.set_props(&props, true)?;
        for (e, s) in &texts {
            bz.set_text(*e, s);
        }
        bz.resolve();
        let t1 = t0.elapsed().as_secs_f64();
        let items = bz.paint_all(&mut res)?;
        if agentfx_ir::env::var("AGENTFX_RECTS").is_ok() {
            // element rects (data-u -> [x, y, w, h], transforms applied) for layout parity checks
            let (_, _, place) = bz.paint_probe(&mut res)?;
            let mut m = serde_json::Map::new();
            for (e, (ptf, bp)) in &place {
                let Some(u) = bz.attr(*e, "data-u") else { continue };
                let (w, h) = bz.box_size(*e);
                let r = ptf.transform_rect_bbox(kurbo::Rect::new(bp.x, bp.y, bp.x + w, bp.y + h));
                m.insert(u, serde_json::json!([r.x0, r.y0, r.width(), r.height()]));
            }
            std::fs::write(out_dir.join(format!("f{f:05}.rects.json")), serde_json::to_string(&m)?)?;
        }
        let t2 = t0.elapsed().as_secs_f64();
        let mut root = agentfx_ir::Node::new("snap");
        root.content = Some(Content::Paint { rect: [0.0, 0.0, 1920.0, 1080.0], items, elements: vec![], text: None });
        let scene = agentfx_ir::Scene { version: agentfx_ir::IR_VERSION, width: 1920, height: 1080, fps: compiled.fps as u32, frames: 1, nodes: vec![root], tracks: vec![], assets: res.assets.clone(), fonts: res.fonts.clone(), camera: None, text_aa: Default::default() };
        let prep = crate::vraster::Prepared::new(&scene, out_dir, Some(&res.font_data), Some(&res.image_data))?;
        let info = crate::vraster::Info::new(&scene, &prep);
        let sched = crate::schedule::build(&scene, &info)?;
        let mut painter = crate::vraster::Painter::new(&scene, &prep, &sched, &info, 0);
        let px = painter.render_rect(0, [0, 0, 1920, 1080])?;
        let t3 = t0.elapsed().as_secs_f64();
        crate::vraster::save_png(&px, &out_dir.join(format!("f{f:05}.png")))?;
        times.push(serde_json::json!({"frame": f, "layout_s": t1, "paint_s": t2 - t1, "raster_s": t3 - t2}));
    }
    Ok(serde_json::json!({ "frames": times }))
}

#[cfg(test)]
mod form_control_tests {
    //! M4d: `<input type=checkbox|radio>` painted like Chrome (`NativeThemeBase::PaintCheckbox`
    //! / `PaintRadio`) with the CSS `accent-color`. The golden is a Chrome 1x screenshot of
    //! `examples/form-controls` (controls under `scale(4)`).
    use std::path::Path;

    fn snap_form_controls(tag: &str) -> (Vec<u8>, usize) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out = root.join("target/test-out").join(tag);
        std::fs::create_dir_all(&out).unwrap();
        super::snap(&root.join("examples/html/form-controls"), &out, &[0]).unwrap();
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(out.join("f00000.png")).unwrap()));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgb);
        buf.truncate(info.buffer_size());
        (buf, info.width as usize)
    }

    fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let i = (y * w + x) * 3;
        [buf[i], buf[i + 1], buf[i + 2]]
    }

    fn near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
        a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= tol)
    }

    #[test]
    fn checkbox_radio_geometry_and_accent() {
        let (buf, w) = snap_form_controls("form-controls-geometry");
        let p = |x, y| px(&buf, w, x, y);
        // 4x row at (100, 200), boxes 52 px, pitch 132 (13 px box + 20 px gap, scaled).
        // b1: checked, accent #17845a: accent fill, white check mark (0.2/0.5 -> 0.4/0.7 -> 0.8/0.2 of the side).
        assert!(near(p(110, 210), [23, 132, 90], 2), "b1 accent fill {:?}", p(110, 210));
        assert!(near(p(116, 231), [255, 255, 255], 8), "b1 check mark left arm {:?}", p(116, 231));
        assert!(near(p(131, 223), [255, 255, 255], 8), "b1 check mark right arm {:?}", p(131, 223));
        // b2: unchecked: white inside, 1 px #767676 border (4 px at 4x), rounded 2 px corners.
        assert!(near(p(232 + 26, 226), [255, 255, 255], 2), "b2 inside {:?}", p(258, 226));
        assert!(near(p(232 + 1, 226), [118, 118, 118], 6), "b2 border {:?}", p(233, 226));
        assert!(near(p(232, 200), [245, 246, 248], 12), "b2 corner stays background {:?}", p(232, 200));
        // b3: `accent-color: auto` = Chrome's rgb(0, 117, 255).
        assert!(near(p(364 + 10, 210), [0, 117, 255], 2), "b3 default accent {:?}", p(374, 210));
        // b4: radio checked: 1 px accent ring, white gap, dot of radius 0.3 side.
        assert!(near(p(496 + 26, 226), [23, 132, 90], 2), "b4 dot {:?}", p(522, 226));
        assert!(near(p(496 + 8, 226), [255, 255, 255], 2), "b4 gap {:?}", p(504, 226));
        assert!(near(p(496 + 2, 226), [23, 132, 90], 40), "b4 ring {:?}", p(498, 226));
        // b5: radio unchecked: grey ring, white inside.
        assert!(near(p(628 + 26, 226), [255, 255, 255], 2), "b5 inside {:?}", p(654, 226));
        assert!(near(p(628 + 2, 226), [118, 118, 118], 40), "b5 ring {:?}", p(630, 226));
        // b6: disabled: accent ignored, #c5c5c5 fill.
        assert!(near(p(760 + 10, 210), [197, 197, 197], 2), "b6 disabled {:?}", p(770, 210));
        // 1x row at (100, 100): the 13 px box fills exactly 100..113 (Chrome's kCheckboxSize; no UA padding/border).
        assert!(near(p(101, 106), [23, 132, 90], 40), "c1 1x left edge inside {:?}", p(101, 106));
        assert!(near(p(99, 106), [245, 246, 248], 12), "c1 1x left of box {:?}", p(99, 106));
        assert!(near(p(113, 106), [245, 246, 248], 12), "c1 1x right of box {:?}", p(113, 106));
    }

    #[test]
    fn matches_chrome_golden_4x() {
        let (buf, w) = snap_form_controls("form-controls-golden");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(root.join("examples/html/form-controls/chrome-4xrow.png")).unwrap()));
        let mut r = dec.read_info().unwrap();
        let mut g = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut g).unwrap();
        let (gw, gh, bpp) = (info.width as usize, info.height as usize, info.color_type.samples());
        assert_eq!((gw, gh), (910, 72));
        // crop (90, 190)-(1000, 262) of the frame vs the golden: mean abs diff over the controls
        let (mut tot, mut n) = (0u64, 0u64);
        for y in 0..gh {
            for x in 0..gw {
                let a = px(&buf, w, 90 + x, 190 + y);
                let i = (y * gw + x) * bpp;
                for c in 0..3 {
                    tot += (a[c] as i64 - g[i + c] as i64).unsigned_abs();
                    n += 1;
                }
            }
        }
        let mean = tot as f64 / n as f64;
        assert!(mean < 1.5, "mean abs diff vs Chrome golden {mean:.2} (M4d: 0.86)");
    }
}

#[cfg(test)]
mod line_snap_tests {
    //! M4d: Chrome rounds each line box top to a whole CSS px in the nearest transformed
    //! ancestor's space before Skia rounds the baseline in device px (x stays fractional).
    //! `examples/text-snap`: three 3-line paragraphs under `scale(1.25)` at local tops 20,
    //! 20.5, 20.25. Expected ink rows are Chrome's.
    use std::path::Path;

    fn ink_rows(buf: &[u8], w: usize, x0: usize, x1: usize) -> Vec<(usize, usize)> {
        let prof: Vec<u32> = (0..200).map(|y| (x0..x1).map(|x| { let i = (y * w + x) * 3; (200i32 - buf[i + 1] as i32).max(0) as u32 }).sum()).collect();
        let (mut out, mut i) = (Vec::new(), 0);
        while i < prof.len() {
            if prof[i] > 0 {
                let mut j = i;
                while j < prof.len() && prof[j] > 0 { j += 1; }
                if j - i > 4 { out.push((i, j)); }
                i = j;
            } else { i += 1; }
        }
        out
    }

    #[test]
    fn line_tops_snap_in_transformed_ancestor_space() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out = root.join("target/test-out/text-snap");
        std::fs::create_dir_all(&out).unwrap();
        // defaults (no AGENTFX_LAYOUT_ROUND / AGENTFX_LINE_SNAP in the env): lu layout + line snap
        super::snap(&root.join("examples/html/text-snap"), &out, &[0]).unwrap();
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(out.join("f00000.png")).unwrap()));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        let w = info.width as usize;
        let tops = |x0, x1| ink_rows(&buf, w, x0, x1).iter().map(|b| b.0).collect::<Vec<_>>();
        // Chrome: f=0 [29, 52, 73], f=0.5 [31, 52, 74], f=0.25 [29, 52, 73]
        assert_eq!(tops(120, 700), vec![29, 52, 73], "top 20");
        assert_eq!(tops(745, 1320), vec![31, 52, 74], "top 20.5: lines 1 and 3 round up, line 2 (37.5 + 0.5) does not move");
        assert_eq!(tops(1370, 1920), vec![29, 52, 73], "top 20.25: rounds down in local px (device-only rounding would move line 1)");
    }
}
