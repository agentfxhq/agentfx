//! Stateless evaluation of compiled property tracks.
use crate::*;
use crate::timing::ease;

/// Product of the transform functions in order (CSS semantics, no origin).
pub fn transform_matrix(fns: &[TransformFn]) -> Affine {
    fns.iter().fold(Affine::IDENTITY, |m, f| m.mul(&match *f {
        TransformFn::Translate(x, y) => Affine::translate(x, y),
        TransformFn::Scale(x, y) => Affine::scale(x, y),
        TransformFn::Rotate(a) => Affine::rotate_deg(a),
    }))
}

fn identity(f: &TransformFn) -> TransformFn {
    match f {
        TransformFn::Translate(..) => TransformFn::Translate(0.0, 0.0),
        TransformFn::Scale(..) => TransformFn::Scale(1.0, 1.0),
        TransformFn::Rotate(..) => TransformFn::Rotate(0.0),
    }
}

/// Active keyframe segment and eased progress at time `t` (None = no effect).
/// The value is keyframes[i] interpolated towards keyframes[i + 1] by `u`.
pub fn eval_segment(tr: &Track, t: f64) -> Option<(usize, f64)> {
    let end = tr.delay + tr.duration;
    let t = if (t - end).abs() < 1e-6 { end }
        else if (t - tr.delay).abs() < 1e-6 { tr.delay } else { t };
    let (p, before) = if t < tr.delay {
        if !matches!(tr.fill, FillMode::Backwards | FillMode::Both) { return None; }
        (0.0, true)
    } else if t >= end {
        if !matches!(tr.fill, FillMode::Forwards | FillMode::Both) { return None; }
        (1.0, false)
    } else { ((t - tr.delay) / tr.duration, false) };
    let i = tr.keyframes.partition_point(|k| k.offset <= p).saturating_sub(1).min(tr.keyframes.len() - 2);
    let a = &tr.keyframes[i];
    let b = &tr.keyframes[i + 1];
    Some((i, ease(a.easing.as_ref().unwrap_or(&tr.easing), (p - a.offset) / (b.offset - a.offset), before)))
}

/// Evaluate at time `t` in seconds. `None` means the animation has no effect.
/// Tracks must satisfy the invariants established by the scene compiler.
pub fn eval_track(tr: &Track, t: f64) -> Option<Sampled> {
    let (i, u) = eval_segment(tr, t)?;
    let a = &tr.keyframes[i];
    let b = &tr.keyframes[i + 1];
    let lerp = |a: f64, b: f64| a + (b - a) * u;
    Some(match (&a.value, &b.value) {
        (Value::Number(a), Value::Number(b)) => {
            let v = lerp(*a, *b);
            Sampled::Number(if tr.property == "opacity" || tr.property == "glyph.opacity" { v.clamp(0.0, 1.0) } else { v })
        }
        (Value::Transform(a), Value::Transform(b)) => {
            let fns: Vec<_> = (0..a.len().max(b.len())).map(|i| {
                let from = a.get(i).copied().unwrap_or_else(|| identity(&b[i]));
                let to = b.get(i).copied().unwrap_or_else(|| identity(&a[i]));
                match (from, to) {
                    (TransformFn::Translate(x,y), TransformFn::Translate(xx,yy)) => TransformFn::Translate(lerp(x,xx),lerp(y,yy)),
                    (TransformFn::Scale(x,y), TransformFn::Scale(xx,yy)) => TransformFn::Scale(lerp(x,xx),lerp(y,yy)),
                    (TransformFn::Rotate(x), TransformFn::Rotate(y)) => TransformFn::Rotate(lerp(x,y)),
                    _ => panic!("incompatible transform functions in compiled track"),
                }
            }).collect();
            Sampled::Matrix(transform_matrix(&fns))
        }
        (Value::Color(a), Value::Color(b)) => Sampled::Color(std::array::from_fn(|k| lerp(a[k], b[k]).clamp(0.0, 1.0))),
        (Value::Discrete(a), Value::Discrete(b)) => Sampled::Sig(if a == b { a.clone() } else { format!("{}|{}|{:.9}", tr.label, i, u) }),
        _ => panic!("incompatible values in compiled track"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn css_order() {
        let m = transform_matrix(&[TransformFn::Translate(10.0, 0.0), TransformFn::Scale(2.0, 3.0)]);
        assert_eq!(m.apply(1.0, 1.0), (12.0, 3.0));
    }
    fn track(a: Value,b: Value) -> Track {
        Track {node:0,property:"opacity".into(),order:0,label:"test".into(),delay:1.0,duration:2.0,fill:FillMode::Both,easing:Easing::Linear,keyframes:vec![Keyframe {offset:0.0,value:a,easing:None},Keyframe {offset:1.0,value:b,easing:None}],stagger:None}
    }
    #[test]
    fn step_before_flag_and_active_start() {
        let mut tr = track(Value::Number(0.0),Value::Number(1.0));
        tr.easing = Easing::Steps(2,StepPos::JumpStart);
        assert_eq!(eval_track(&tr,0.0),Some(Sampled::Number(0.0)));
        assert_eq!(eval_track(&tr,1.0),Some(Sampled::Number(0.5)));
        assert_eq!(eval_track(&tr,3.0),Some(Sampled::Number(1.0)));
    }
    #[test]
    fn easing_is_per_segment_and_key_easing_overrides() {
        let mut tr = track(Value::Number(0.0),Value::Number(1.0));
        tr.easing = Easing::Steps(2,StepPos::JumpEnd);
        tr.keyframes.insert(1,Keyframe {offset:0.5,value:Value::Number(0.5),easing:Some(Easing::Linear)});
        assert_eq!(eval_track(&tr,1.25),Some(Sampled::Number(0.0)));
        assert_eq!(eval_track(&tr,1.75),Some(Sampled::Number(0.25)));
        assert_eq!(eval_track(&tr,2.0),Some(Sampled::Number(0.5)));
        assert_eq!(eval_track(&tr,2.25),Some(Sampled::Number(0.625)));
    }
    #[test]
    fn overshoot_opacity_only_and_opaque_signatures() {
        let mut tr = track(Value::Number(-2.0),Value::Number(2.0));
        assert_eq!(eval_track(&tr,1.0),Some(Sampled::Number(0.0)));
        assert_eq!(eval_track(&tr,3.0),Some(Sampled::Number(1.0)));
        tr.property = "text".into();
        assert_eq!(eval_track(&tr,1.0),Some(Sampled::Number(-2.0)));
        let tr = track(Value::Discrete("red".into()),Value::Discrete("blue".into()));
        assert_eq!(eval_track(&tr,2.0),Some(Sampled::Sig("test|0|0.500000000".into())));
        let tr = track(Value::Discrete("red".into()),Value::Discrete("red".into()));
        assert_eq!(eval_track(&tr,2.0),Some(Sampled::Sig("red".into())));
    }
    #[test]
    fn none_transforms_and_rotation_without_shortest_path() {
        let tr = track(Value::Transform(vec![]),Value::Transform(vec![TransformFn::Translate(10.0,20.0),TransformFn::Scale(3.0,5.0),TransformFn::Rotate(540.0)]));
        let Some(Sampled::Matrix(m)) = eval_track(&tr,2.0) else { panic!() };
        let expected = transform_matrix(&[TransformFn::Translate(5.0,10.0),TransformFn::Scale(2.0,3.0),TransformFn::Rotate(270.0)]);
        assert_eq!(m,expected);
        let tr = track(tr.keyframes[1].value.clone(),Value::Transform(vec![]));
        assert_eq!(eval_track(&tr,2.0),Some(Sampled::Matrix(expected)));
    }

}
