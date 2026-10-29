//! Mesh gradients (`fill=mesh(CxR, c0, c1, ...)`): a cols x rows grid of colors over the box,
//! interpolated bicubically (Catmull-Rom, clamped ends) in OKLab with straight alpha, rastered
//! once into a content-addressed PNG (`cache/mesh/<hash>.png`) and drawn as an image fill
//! stretched over the box (static: cached by the renderer like any image). Both backends use
//! the same raster, so the HTML preview matches direct mode.
use crate::values::Rgba;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Raster size per axis (the field is smooth; bilinear upscaling is exact enough, and an
/// ordered sub-LSB dither keeps 8-bit bands invisible).
pub const RES: u32 = 512;

fn srgb_to_lin(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}
fn lin_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// sRGB (0..1) -> OKLab.
pub fn oklab(c: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = c.map(srgb_to_lin);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s, 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s, 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s]
}

/// OKLab -> sRGB (0..1, clamped).
pub fn oklab_to_srgb(c: [f64; 3]) -> [f64; 3] {
    let l = c[0] + 0.3963377774 * c[1] + 0.2158037573 * c[2];
    let m = c[0] - 0.1055613458 * c[1] - 0.0638541728 * c[2];
    let s = c[0] - 0.0894841775 * c[1] - 1.2914855480 * c[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    let r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
    let g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
    let b = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;
    [r, g, b].map(|v| lin_to_srgb(v.clamp(0.0, 1.0)).clamp(0.0, 1.0))
}

fn catmull(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
}

/// Mesh value at (u, v) in 0..1: OKLab + alpha, Catmull-Rom along both axes.
pub fn sample(cols: usize, rows: usize, grid: &[[f64; 4]], u: f64, v: f64) -> [f64; 4] {
    let at = |x: isize, y: isize| grid[(y.clamp(0, rows as isize - 1) as usize) * cols + x.clamp(0, cols as isize - 1) as usize];
    let axis = |n: usize, w: f64| -> (isize, f64) {
        if n < 2 {
            return (0, 0.0);
        }
        let x = w.clamp(0.0, 1.0) * (n - 1) as f64;
        let i = (x.floor() as isize).min(n as isize - 2);
        (i, x - i as f64)
    };
    let (ix, fx) = axis(cols, u);
    let (iy, fy) = axis(rows, v);
    std::array::from_fn(|k| {
        let row = |y: isize| catmull(at(ix - 1, y)[k], at(ix, y)[k], at(ix + 1, y)[k], at(ix + 2, y)[k], fx);
        catmull(row(iy - 1), row(iy), row(iy + 1), row(iy + 2), fy)
    })
}

/// Straight-alpha RGBA8 raster of the mesh (`w` x `h`), 4x4 ordered dither.
pub fn raster(cols: u32, rows: u32, colors: &[Rgba], w: u32, h: u32) -> Vec<u8> {
    let (c, r) = (cols.max(1) as usize, rows.max(1) as usize);
    let grid: Vec<[f64; 4]> = (0..c * r).map(|k| {
        let col = colors.get(k).or(colors.last()).cloned().unwrap_or(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 });
        let l = oklab([col.r, col.g, col.b]);
        [l[0], l[1], l[2], col.a]
    }).collect();
    const BAYER: [f64; 16] = [0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0];
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let u = (x as f64 + 0.5) / w as f64;
            let v = (y as f64 + 0.5) / h as f64;
            let s = sample(c, r, &grid, u, v);
            let rgb = oklab_to_srgb([s[0], s[1], s[2]]);
            let d = (BAYER[((y % 4) * 4 + x % 4) as usize] + 0.5) / 16.0;
            let o = ((y * w + x) * 4) as usize;
            for k in 0..3 {
                out[o + k] = (rgb[k] * 255.0 + d).floor().clamp(0.0, 255.0) as u8;
            }
            out[o + 3] = (s[3].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    out
}

/// Content-addressed PNG of the mesh in `<reel-script home>/cache/mesh/` (written once).
pub fn png(cols: u32, rows: u32, colors: &[Rgba]) -> std::io::Result<PathBuf> {
    let mut hs = Sha256::new();
    hs.update(format!("mesh1 {cols}x{rows} {RES}"));
    for c in colors {
        hs.update(format!(" {:.6},{:.6},{:.6},{:.6}", c.r, c.g, c.b, c.a));
    }
    let name: String = hs.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect();
    let dir = crate::run::cache_dir("mesh");
    let path = dir.join(format!("{name}.png"));
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    let px = raster(cols, rows, colors, RES, RES);
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    {
        let f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut enc = png::Encoder::new(f, RES, RES);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(std::io::Error::other)?;
        wr.write_image_data(&px).map_err(std::io::Error::other)?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oklab_roundtrip_and_corners() {
        for c in [[1.0, 0.0, 0.0], [0.2, 0.5, 0.9], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]] {
            let r = oklab_to_srgb(oklab(c));
            for k in 0..3 {
                assert!((r[k] - c[k]).abs() < 1e-6, "{c:?} -> {r:?}");
            }
        }
        let cols = [Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }, Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }, Rgba { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }, Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }];
        let px = raster(2, 2, &cols, 64, 64);
        // corners are (nearly) the authored colors; the center is a blend, not the first color
        assert!(px[0] >= 250 && px[2] <= 6, "{:?}", &px[..4]);
        let tr = ((0 * 64 + 63) * 4) as usize;
        assert!(px[tr + 2] >= 250 && px[tr] <= 6);
        let c = ((32 * 64 + 32) * 4) as usize;
        assert!(px[c] > 60 && px[c + 1] > 60 && px[c + 2] > 60, "{:?}", &px[c..c + 4]);
    }
}
