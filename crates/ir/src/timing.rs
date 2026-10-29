//! CSS easing functions, including the discontinuity's before side.
use crate::{Easing, StepPos};

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
