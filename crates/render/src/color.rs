//! Color adjustment effect (`Effect::Color`): brightness, saturate, contrast, hue-rotate (CSS
//! filter semantics, straight sRGB, each step clamped like a CSS filter chain), then tint,
//! levels and a `.cube` 3D LUT. Works on premultiplied RGBA8 offscreen buffers.
use rayon::prelude::*;
use agentfx_ir::cube::Cube;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

/// LUTs by their IR path string (loaded once per process by `preload`).
fn luts() -> &'static Mutex<HashMap<String, Arc<Cube>>> {
    static L: OnceLock<Mutex<HashMap<String, Arc<Cube>>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Load every LUT the scene's color effects name (relative to `base`, the IR file's dir).
pub fn preload(scene: &agentfx_ir::Scene, base: &Path) -> anyhow::Result<()> {
    for n in &scene.nodes {
        for e in &n.effects {
            if let agentfx_ir::Effect::Color { lut: Some(p), .. } = e {
                let mut m = luts().lock().unwrap();
                if !m.contains_key(p) {
                    let c = Cube::load(&base.join(p)).map_err(|e| anyhow::anyhow!("lut {p}: {e}"))?;
                    m.insert(p.clone(), Arc::new(c));
                }
            }
        }
    }
    Ok(())
}

/// Parameters after evaluation (animatable fields from the effect's tracks).
pub struct ColorCfg<'a> {
    pub brightness: f64,
    pub saturate: f64,
    pub contrast: f64,
    pub hue: f64,
    pub tint: Option<[f64; 4]>,
    pub tint_amount: f64,
    pub levels: Option<[f64; 5]>,
    pub lut: Option<&'a str>,
}

/// CSS `saturate(s)` matrix (Filter Effects 1, feColorMatrix type=saturate).
pub fn saturate_matrix(s: f32) -> [[f32; 3]; 3] {
    [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ]
}

/// CSS `hue-rotate(deg)` matrix (feColorMatrix type=hueRotate).
pub fn hue_matrix(deg: f64) -> [[f32; 3]; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    let (s, c) = (s as f32, c as f32);
    [
        [0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928],
        [0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283],
        [0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072],
    ]
}

fn mat(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| (m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2]).clamp(0.0, 1.0))
}

impl ColorCfg<'_> {
    /// No step changes a pixel: the buffer is left untouched (bit-exact).
    pub fn is_identity(&self) -> bool {
        self.brightness == 1.0 && self.saturate == 1.0 && self.contrast == 1.0 && self.hue.rem_euclid(360.0) == 0.0 && (self.tint.is_none() || self.tint_amount == 0.0) && self.levels.is_none() && self.lut.is_none()
    }
}

/// Apply in place to a premultiplied RGBA8 buffer.
pub fn apply(buf: &mut [u8], w: usize, h: usize, cfg: &ColorCfg) {
    if cfg.is_identity() || w == 0 || h == 0 {
        return;
    }
    let lut = cfg.lut.and_then(|p| luts().lock().unwrap().get(p).cloned());
    let b = cfg.brightness as f32;
    let sat = (cfg.saturate != 1.0).then(|| saturate_matrix(cfg.saturate as f32));
    let ct = cfg.contrast as f32;
    let hue = (cfg.hue.rem_euclid(360.0) != 0.0).then(|| hue_matrix(cfg.hue));
    let tint = cfg.tint.filter(|_| cfg.tint_amount != 0.0).map(|t| ([t[0] as f32, t[1] as f32, t[2] as f32], (cfg.tint_amount * t[3]) as f32));
    let lv = cfg.levels.map(|l| l.map(|v| v as f32));
    let px = |c: [f32; 3]| -> [f32; 3] {
        let mut c = c;
        if b != 1.0 {
            c = c.map(|v| (v * b).clamp(0.0, 1.0));
        }
        if let Some(m) = &sat {
            c = mat(m, c);
        }
        if ct != 1.0 {
            c = c.map(|v| (v * ct + 0.5 - 0.5 * ct).clamp(0.0, 1.0));
        }
        if let Some(m) = &hue {
            c = mat(m, c);
        }
        if let Some((t, k)) = tint {
            let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            c = std::array::from_fn(|i| (c[i] + (l * t[i] - c[i]) * k).clamp(0.0, 1.0));
        }
        if let Some(l) = lv {
            c = c.map(|v| {
                let x = ((v - l[0]) / (l[1] - l[0])).clamp(0.0, 1.0).powf(1.0 / l[2]);
                (l[3] + x * (l[4] - l[3])).clamp(0.0, 1.0)
            });
        }
        if let Some(cube) = &lut {
            c = cube.sample(c).map(|v| v.clamp(0.0, 1.0));
        }
        c
    };
    buf[..w * h * 4].par_chunks_mut(w * 4).for_each(|row| {
        for p in row.chunks_exact_mut(4) {
            let a = p[3];
            if a == 0 {
                continue;
            }
            let af = a as f32;
            let c = px([p[0] as f32 / af, p[1] as f32 / af, p[2] as f32 / af].map(|v| v.min(1.0)));
            for i in 0..3 {
                p[i] = (c[i] * af + 0.5).min(af) as u8;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cfg() -> ColorCfg<'static> {
        ColorCfg { brightness: 1.0, saturate: 1.0, contrast: 1.0, hue: 0.0, tint: None, tint_amount: 1.0, levels: None, lut: None }
    }
    fn one(p: [u8; 4], c: &ColorCfg) -> [u8; 4] {
        let mut b = p.to_vec();
        apply(&mut b, 1, 1, c);
        [b[0], b[1], b[2], b[3]]
    }
    #[test]
    fn identity_is_bit_exact() {
        let mut b: Vec<u8> = (0..400).map(|i| (i * 37 % 256) as u8).collect();
        for p in b.chunks_exact_mut(4) {
            p[0] = p[0].min(p[3]);
            p[1] = p[1].min(p[3]);
            p[2] = p[2].min(p[3]);
        }
        let a = b.clone();
        apply(&mut b, 10, 10, &ColorCfg { hue: 360.0, ..cfg() });
        assert_eq!(a, b);
    }
    #[test]
    fn css_reference_values() {
        // brightness(0.5) halves; contrast(2) pushes away from 0.5; saturate(0) = luma weights
        assert_eq!(one([200, 100, 50, 255], &ColorCfg { brightness: 0.5, ..cfg() }), [100, 50, 25, 255]);
        assert_eq!(one([191, 64, 128, 255], &ColorCfg { contrast: 2.0, ..cfg() }), [255, 0, 129, 255]);
        let g = one([255, 0, 0, 255], &ColorCfg { saturate: 0.0, ..cfg() });
        assert_eq!(g, [54, 54, 54, 255]);
        // hue-rotate(180) of pure red (Chrome: rgb(0, 109, 109) after clamping)
        let r = one([255, 0, 0, 255], &ColorCfg { hue: 180.0, ..cfg() });
        assert_eq!(r, [0, 109, 109, 255]);
        // premultiplied: half-transparent white at brightness 0.5 stays half-transparent grey
        assert_eq!(one([128, 128, 128, 128], &ColorCfg { brightness: 0.5, ..cfg() }), [64, 64, 64, 128]);
    }
    #[test]
    fn tint_and_levels() {
        let t = one([255, 255, 255, 255], &ColorCfg { tint: Some([1.0, 0.5, 0.0, 1.0]), ..cfg() });
        assert_eq!(t, [255, 128, 0, 255]);
        let l = one([128, 128, 128, 255], &ColorCfg { levels: Some([0.25, 0.75, 1.0, 0.0, 1.0]), ..cfg() });
        assert_eq!(l, [129, 129, 129, 255]);
    }
}
