//! Time remap of a subtree (`rewind`, `timemap`): a layer's local time as a piecewise
//! function of its parent's time. Resolved in the compiler: every track of the layer and of
//! its descendants is re-evaluated through the (composed) remap at every output frame and
//! baked into per-frame linear keys, so the renderer needs nothing new. Exact at frame
//! times for any ease (springs, steps, cubic), trim paths, split-text staggers (child layers),
//! particles (seeded, baked tracks) and repeats; between frames (motion-blur sub-samples) the
//! baked track is linear.
use crate::ir::{Key, Track};
use crate::values::Ease;
use std::collections::HashMap;

/// One remap event: from parent time `t0` on, local = `l0` + curve(parent - t0), where the
/// curve passes through `keys` (parent offset, local offset, ease of the segment starting
/// there) and continues after the last key with slope `tail` (1 = play on, 0 = hold).
#[derive(Debug, Clone)]
pub struct MapEv {
    pub t0: f64,
    pub l0: f64,
    pub keys: Vec<(f64, f64, Ease)>,
    pub tail: f64,
}

/// Remap of one layer: identity until the first event; each later event starts from the
/// local time the map has reached at its start ("later events start from where things are").
#[derive(Debug, Clone, Default)]
pub struct TimeMap {
    pub evs: Vec<MapEv>,
}

impl TimeMap {
    pub fn eval(&self, t: f64) -> f64 {
        let Some(e) = self.evs.iter().rev().find(|e| t >= e.t0 - 1e-9) else { return t };
        let r = t - e.t0;
        let k = &e.keys;
        if k.is_empty() {
            return e.l0 + e.tail * r;
        }
        if r <= k[0].0 {
            return e.l0 + k[0].1;
        }
        for i in 0..k.len() - 1 {
            if r < k[i + 1].0 {
                let span = (k[i + 1].0 - k[i].0).max(1e-9);
                let p = k[i].2.sample(((r - k[i].0) / span).clamp(0.0, 1.0));
                return e.l0 + k[i].1 + (k[i + 1].1 - k[i].1) * p;
            }
        }
        let last = &k[k.len() - 1];
        e.l0 + last.1 + e.tail * (r - last.0)
    }
    /// Add an event at parent time `t0` (events must be added in time order).
    pub fn push(&mut self, t0: f64, mut keys: Vec<(f64, f64, Ease)>, tail: f64) {
        let l0 = self.eval(t0);
        keys.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        if keys.first().map(|k| k.0 > 1e-9).unwrap_or(false) {
            keys.insert(0, (0.0, 0.0, Ease::Linear));
        }
        self.evs.retain(|e| e.t0 < t0 - 1e-9);
        self.evs.push(MapEv { t0, l0, keys, tail });
    }
    pub fn start(&self) -> Option<f64> {
        self.evs.first().map(|e| e.t0)
    }
}

/// Bake `t` through the composed remap `local` (identity before `start`), keyed at every
/// output frame from `start` to `total`. Keys before `start` stay as they are (symbolic).
pub fn bake_track(t: &Track, local: &dyn Fn(f64) -> f64, start: f64, fps: f64, total: f64) -> Track {
    let orig = t.clone();
    let mut out = Track { layer: t.layer.clone(), ch: t.ch, base: t.base.clone(), keys: vec![] };
    // keep the original keys up to the segment that contains `start` (its start key is exact,
    // and no output frame lies between that key and the first baked frame, so the segment's
    // shape before `start` is only ever seen at frames < start, which keep their values)
    let ks = orig.keys.iter().rposition(|k| k.t <= start + 1e-9);
    let from = match ks {
        Some(i) => {
            out.keys.extend(orig.keys[..=i].iter().cloned());
            if let Some(k) = out.keys.last_mut() {
                k.ease = Ease::Linear;
            }
            orig.keys[i].t
        }
        None => f64::NEG_INFINITY,
    };
    let f0 = ((from.max(0.0) * fps).floor() as i64).max(0);
    let f1 = (total * fps).ceil() as i64;
    for f in f0..=f1 {
        let ft = f as f64 / fps;
        if ft <= from + 1e-9 {
            continue;
        }
        // not re-quantized: the value is exactly what the forward track gives at that local
        // time (its keys are already quantized), so a rewound frame equals its forward frame
        let v = orig.eval(local(ft));
        // drop the middle of runs of identical values (a hold is no change, no damage)
        let n = out.keys.len();
        if n >= 2 && out.keys[n - 1].v == v && out.keys[n - 2].v == v {
            out.keys[n - 1].t = ft;
            continue;
        }
        out.keys.push(Key { t: ft, v, ease: Ease::Linear });
    }
    out
}

/// Apply the remaps: `maps` per layer id, `parent` per layer id. Every track of a remapped
/// layer or of a descendant is baked through the composed map (outermost ancestor first).
pub fn apply(tracks: &mut [Track], maps: &HashMap<String, TimeMap>, parent: &HashMap<String, String>, fps: f64, total: f64) -> usize {
    if maps.is_empty() {
        return 0;
    }
    let mut chains: HashMap<String, Vec<&TimeMap>> = HashMap::new();
    let mut n = 0;
    for t in tracks.iter_mut() {
        let chain = chains.entry(t.layer.clone()).or_insert_with(|| {
            let mut c = vec![];
            let mut cur = Some(t.layer.clone());
            while let Some(id) = cur {
                if let Some(m) = maps.get(&id) {
                    c.push(m);
                }
                cur = parent.get(&id).cloned();
            }
            c.reverse();
            c
        });
        if chain.is_empty() || t.keys.is_empty() {
            continue;
        }
        let start = chain.iter().filter_map(|m| m.start()).fold(f64::INFINITY, f64::min);
        let local = |x: f64| chain.iter().fold(x, |acc, m| m.eval(acc));
        *t = bake_track(t, &local, start, fps, total);
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rewind_then_play_and_hold() {
        let mut m = TimeMap::default();
        m.push(1.0, vec![(0.0, 0.0, Ease::Linear), (0.5, -2.0, Ease::Linear)], 1.0);
        assert_eq!(m.eval(0.5), 0.5);
        assert!((m.eval(1.25) - 0.0).abs() < 1e-12); // 4x back: 1.0 - 4 * 0.25
        assert!((m.eval(1.5) - (-1.0)).abs() < 1e-12);
        assert!((m.eval(2.5) - 0.0).abs() < 1e-12); // plays on at 1x
        m.push(3.0, vec![(0.0, 0.0, Ease::Linear)], 0.0); // freeze from 3.0
        assert!((m.eval(5.0) - m.eval(3.0)).abs() < 1e-12);
    }
}
