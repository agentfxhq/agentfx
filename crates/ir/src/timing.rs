//! CSS easing functions, including the discontinuity's before side.
use crate::{CurveKind, EaseDir, Easing, StepPos};

/// Evaluate CSS easing at `x`; `before_flag` selects the preceding side of a step.
pub fn ease(e: &Easing, x: f64, before_flag: bool) -> f64 {
    match *e {
        Easing::Linear => x,
        Easing::Steps(n, pos) => {
            assert!(n > 0 && !(n == 1 && pos == StepPos::JumpNone));
            let scaled = x * n as f64;
            let mut step = scaled.floor();
            if matches!(pos, StepPos::JumpStart | StepPos::JumpBoth) { step += 1.0; }
            if before_flag && scaled == scaled.floor() { step -= 1.0; }
            let jumps = n as f64 + match pos { StepPos::JumpBoth => 1.0, StepPos::JumpNone => -1.0, _ => 0.0 };
            if x >= 0.0 { step = step.max(0.0); }
            if x <= 1.0 { step = step.min(jumps); }
            step / jumps
        }
        Easing::Curve { kind, dir, a, b } => curve(kind, dir, a, b, x),
        Easing::Spring { mass, stiffness, damping, velocity } => {
            assert!(mass.is_finite() && mass > 0.0 && stiffness.is_finite() && stiffness > 0.0
                && damping.is_finite() && damping > 0.0 && velocity.is_finite(),
                "spring requires finite mass/stiffness > 0, damping > 0 (zero damping never settles), and velocity");
            if x <= 0.0 { return 0.0; }
            if x >= 1.0 { return 1.0; }
            let spring = Spring::new(mass, stiffness, damping, velocity);
            1.0 - spring.error(x * settling_time_cached(&spring, [mass, stiffness, damping, velocity]))
        },
        Easing::CubicBezier(x1, y1, x2, y2) => {
            let start = if x1 > 0.0 { y1 / x1 } else if y1 == 0.0 && x2 > 0.0 { y2 / x2 } else { 0.0 };
            let end = if x2 < 1.0 { (y2 - 1.0) / (x2 - 1.0) } else if y2 == 1.0 && x1 < 1.0 { (y1 - 1.0) / (x1 - 1.0) } else { 0.0 };
            if x < 0.0 { return start * x; }
            if x > 1.0 { return 1.0 + end * (x - 1.0); }
            let cx = 3.0 * x1;
            let bx = 3.0 * (x2 - x1) - cx;
            let ax = 1.0 - cx - bx;
            let cy = 3.0 * y1;
            let by = 3.0 * (y2 - y1) - cy;
            let ay = 1.0 - cy - by;
            let sx = |t: f64| ((ax * t + bx) * t + cx) * t;
            let sy = |t: f64| ((ay * t + by) * t + cy) * t;
            // Blink's gfx curve uses eleven samples to seed Newton's method.
            let mut lo = 0.0;
            let mut hi = 1.0;
            let mut t = x;
            for i in 1..=10 {
                hi = i as f64 / 10.0;
                if x <= sx(hi) {
                    lo = (i - 1) as f64 / 10.0;
                    t = lo + (hi - lo) * (x - sx(lo)) / (sx(hi) - sx(lo));
                    break;
                }
            }
            let initial = t;
            for _ in 0..4 {
                let error = sx(t) - x;
                if error.abs() < 1e-7 { return sy(t); }
                let derivative = (3.0 * ax * t + 2.0 * bx) * t + cx;
                if derivative.abs() < 1e-7 { break; }
                t -= error / derivative;
            }
            t = initial;
            while lo < hi {
                let value = sx(t);
                if (value - x).abs() < 1e-7 { break; }
                if x > value { lo = t; } else { hi = t; }
                let next = (hi + lo) * 0.5;
                if next == t { break; }
                t = next;
            }
            sy(t)
        }
    }
}

/// Closed-form easing family (`Easing::Curve`) at `x`, clamped to [0, 1]. The single
/// definition: the compiler samples it (HTML `linear()`, baked tracks) through this too.
pub fn curve(kind: CurveKind, dir: EaseDir, a: f64, b: f64, x: f64) -> f64 {
    let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
    if x == 0.0 || x == 1.0 {
        return x;
    }
    let out_defined = matches!(kind, CurveKind::Elastic | CurveKind::Bounce | CurveKind::Bounces);
    let f_in = |x: f64| if out_defined { 1.0 - curve_out(kind, a, b, 1.0 - x) } else { curve_in(kind, a, x) };
    match dir {
        EaseDir::In => f_in(x),
        EaseDir::Out => 1.0 - f_in(1.0 - x),
        EaseDir::InOut if x < 0.5 => f_in(2.0 * x) * 0.5,
        EaseDir::InOut => 1.0 - f_in(2.0 - 2.0 * x) * 0.5,
    }
}

fn curve_in(kind: CurveKind, a: f64, x: f64) -> f64 {
    use std::f64::consts::FRAC_PI_2;
    match kind {
        CurveKind::Power => x.powf(a),
        CurveKind::Sine => 1.0 - (x * FRAC_PI_2).cos(),
        CurveKind::Expo => if x <= 0.0 { 0.0 } else { (10.0 * (x - 1.0)).exp2() },
        CurveKind::Circ => 1.0 - (1.0 - x * x).max(0.0).sqrt(),
        CurveKind::Back => x * x * ((a + 1.0) * x - a),
        _ => unreachable!("out-defined curve"),
    }
}

fn curve_out(kind: CurveKind, a: f64, b: f64, x: f64) -> f64 {
    use std::f64::consts::TAU;
    if x <= 0.0 { return 0.0; }
    if x >= 1.0 { return 1.0; }
    match kind {
        CurveKind::Elastic => {
            // GSAP's elastic.out(amplitude, period)
            let amp = a.max(1.0);
            let shift = b / TAU * (1.0 / amp).asin();
            amp * (-10.0 * x).exp2() * ((x - shift) * TAU / b).sin() + 1.0
        }
        CurveKind::Bounce => {
            const N: f64 = 7.5625;
            const D: f64 = 2.75;
            if x < 1.0 / D {
                N * x * x
            } else if x < 2.0 / D {
                let x = x - 1.5 / D;
                N * x * x + 0.75
            } else if x < 2.5 / D {
                let x = x - 2.25 / D;
                N * x * x + 0.9375
            } else {
                let x = x - 2.625 / D;
                N * x * x + 0.984375
            }
        }
        CurveKind::Bounces => {
            let n = a.max(1.0).round();
            let fall = 1.0 / (1.0 + n.sqrt());
            if x <= fall {
                (x / fall).powi(2)
            } else {
                let phase = (x - fall) / (1.0 - fall) * n;
                let u = phase.fract();
                1.0 - 0.5_f64.powf(phase.floor() + 1.0) * 4.0 * u * (1.0 - u)
            }
        }
        _ => unreachable!("in-defined curve"),
    }
}

// Residual y = 1 - position, so y(0) = 1 and y'(0) = -velocity.
/// `Spring::settling_time` is a 64-step bisection (~3 us); scenes evaluate the same few
/// springs millions of times. Memoized per thread by the exact parameter bits (same value).
fn settling_time_cached(spring: &Spring, key: [f64; 4]) -> f64 {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static CACHE: RefCell<HashMap<[u64; 4], f64>> = RefCell::new(HashMap::new());
    }
    let k = key.map(f64::to_bits);
    CACHE.with(|c| {
        if let Some(&v) = c.borrow().get(&k) {
            return v;
        }
        let v = spring.settling_time();
        let mut c = c.borrow_mut();
        if c.len() >= 4096 {
            c.clear();
        }
        c.insert(k, v);
        v
    })
}

enum Spring {
    Under { a: f64, w: f64, b: f64 },
    Critical { a: f64, b: f64 },
    Over { r: f64, s: f64, a: f64, b: f64 },
}

impl Spring {
    fn new(m: f64, k: f64, c: f64, v: f64) -> Self {
        let a = c / (2.0 * m);
        let w2 = k / m;
        assert!(a.is_finite() && w2.is_finite() && w2 > 0.0,
            "spring parameters exceed numerical range");
        let d = a * a - w2;
        if d == 0.0 {
            Self::Critical { a, b: a - v }
        } else if d < 0.0 {
            let w = (-d).sqrt();
            Self::Under { a, w, b: (a - v) / w }
        } else {
            let s = -a - d.sqrt();
            let r = w2 / s;
            let aa = (-v - s) / (r - s);
            Self::Over { r, s, a: aa, b: 1.0 - aa }
        }
    }

    fn error(&self, t: f64) -> f64 {
        match *self {
            Self::Under { a, w, b } => (-a * t).exp() * ((w * t).cos() + b * (w * t).sin()),
            Self::Critical { a, b } => (-a * t).exp() * (1.0 + b * t),
            Self::Over { r, s, a, b } => a * (r * t).exp() + b * (s * t).exp(),
        }
    }

    // Locate the last extremum outside tolerance, then bisect the final crossing.
    // In the oscillating case the extrema form an exponential geometric sequence.
    fn settling_time(&self) -> f64 {
        const EPS: f64 = 0.001;
        let (mut lo, mut hi) = match *self {
            Self::Under { a, w, b } => {
                let phase = b.atan();
                let theta = (a / w).atan();
                let period = std::f64::consts::PI / w;
                let first = (phase - theta) / w;
                let peak_log = b.hypot(1.0).ln() + (w / a.hypot(w)).ln();
                let limit = (peak_log - EPS.ln()) / a;
                let n = ((limit - first) / period).floor();
                let last = first + n * period;
                let lo = last.max(0.0);
                (lo, (last + period).max(period))
            }
            Self::Critical { a, b } => {
                let peak = if b != 0.0 { (b - a) / (a * b) } else { 0.0 };
                let lo = if peak > 0.0 && self.error(peak).abs() >= EPS { peak } else { 0.0 };
                (lo, lo + 1.0 / a)
            }
            Self::Over { r, s, a, b } => {
                let ratio = -b * s / (a * r);
                let peak = ratio.ln() / (r - s);
                let lo = if peak > 0.0 && self.error(peak).abs() >= EPS { peak } else { 0.0 };
                (lo, lo - 1.0 / r)
            }
        };
        for _ in 0..1024 {
            if self.error(hi).abs() <= EPS { break; }
            hi *= 2.0;
        }
        assert!(hi.is_finite() && self.error(hi).abs() <= EPS,
            "spring settling time exceeds numerical range");
        for _ in 0..64 {
            let mid = lo + (hi - lo) * 0.5;
            if self.error(mid).abs() > EPS { lo = mid; } else { hi = mid; }
        }
        hi
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chrome_ease() {
        let e = Easing::CubicBezier(0.25, 0.1, 0.25, 1.0);
        assert!((ease(&e, 0.5, false) - 0.8024033877399112).abs() < 1e-7);
        assert_eq!(ease(&e, -0.5, false), -0.2);
        assert_eq!(ease(&e, 1.5, false), 1.0);
        let flat = Easing::CubicBezier(0.0, 0.0, 0.0, 1.0);
        assert!((ease(&flat, 0.001, false) - 0.028).abs() < 1e-5);
    }
    #[test]
    fn curves() {
        use CurveKind::*;
        use EaseDir::*;
        let c = |k, d, a, b, x| curve(k, d, a, b, x);
        // endpoints and clamping for every family and direction
        for (k, a, b) in [(Power, 3.0, 0.0), (Sine, 0.0, 0.0), (Expo, 0.0, 0.0), (Circ, 0.0, 0.0), (Back, 1.70158, 0.0), (Elastic, 1.0, 0.3), (Bounce, 0.0, 0.0), (Bounces, 3.0, 0.0)] {
            for d in [In, Out, InOut] {
                assert_eq!(c(k, d, a, b, 0.0), 0.0);
                assert_eq!(c(k, d, a, b, 1.0), 1.0);
                assert_eq!(c(k, d, a, b, -1.0), 0.0);
                assert_eq!(c(k, d, a, b, 2.0), 1.0);
                // in-out is point-symmetric about (0.5, 0.5)
                for x in [0.1, 0.3, 0.45] {
                    let s = c(k, InOut, a, b, x) + c(k, InOut, a, b, 1.0 - x);
                    assert!((s - 1.0).abs() < 1e-12, "{k:?} {x}");
                }
            }
        }
        // power2.out = 1 - (1-x)^3 (GSAP)
        assert!((c(Power, Out, 3.0, 0.0, 0.5) - 0.875).abs() < 1e-12);
        assert!((c(Sine, In, 0.0, 0.0, 0.5) - (1.0 - std::f64::consts::FRAC_PI_4.cos())).abs() < 1e-12);
        // back overshoots, elastic oscillates past 1, bounce never exceeds 1
        let back_max = (1..100).map(|i| c(Back, Out, 1.70158, 0.0, i as f64 / 100.0)).fold(0.0, f64::max);
        assert!((back_max - 1.1).abs() < 0.01, "{back_max}");
        assert!((1..100).any(|i| c(Elastic, Out, 1.0, 0.3, i as f64 / 100.0) > 1.0));
        assert!((1..1000).all(|i| c(Bounce, Out, 0.0, 0.0, i as f64 / 1000.0) <= 1.0));
        // Penner bounce touches 1 at x = 1/2.75 and dips back to 0.75
        assert!((c(Bounce, Out, 0.0, 0.0, 1.0 / 2.75) - 1.0).abs() < 1e-12);
        assert!((c(Bounce, Out, 0.0, 0.0, 1.5 / 2.75) - 0.75).abs() < 1e-12);
        // the Easing wrapper routes to the same function
        let e = Easing::Curve { kind: Elastic, dir: Out, a: 1.0, b: 0.3 };
        assert_eq!(ease(&e, 0.37, false), c(Elastic, Out, 1.0, 0.3, 0.37));
        let json: Easing = serde_json::from_str(r#"{"curve": {"kind": "elastic", "dir": "in-out", "a": 1, "b": 0.3}}"#).unwrap();
        assert_eq!(json, Easing::Curve { kind: Elastic, dir: InOut, a: 1.0, b: 0.3 });
    }
    #[test]
    fn step_boundaries() {
        for (pos, at_zero, at_half, before_half) in [
            (StepPos::JumpStart, 0.5, 1.0, 0.5),
            (StepPos::JumpEnd, 0.0, 0.5, 0.0),
            (StepPos::JumpNone, 0.0, 1.0, 0.0),
            (StepPos::JumpBoth, 1.0/3.0, 2.0/3.0, 1.0/3.0),
        ] {
            let e = Easing::Steps(2, pos);
            assert_eq!(ease(&e, 0.0, false), at_zero);
            assert_eq!(ease(&e, 0.0, true), 0.0);
            assert_eq!(ease(&e, 0.5, false), at_half);
            assert_eq!(ease(&e, 0.5, true), before_half);
            assert_eq!(ease(&e, 1.0, false), 1.0);
        }
    }
}
