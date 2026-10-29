//! `.cube` 3D LUT files (Adobe / Resolve format): parse and trilinear sampling.

use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Cube {
    /// Grid points per axis (LUT_3D_SIZE), 2..=256.
    pub size: usize,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// size^3 RGB entries, red index changing fastest.
    pub data: Vec<[f32; 3]>,
}

impl Cube {
    pub fn parse(text: &str) -> Result<Cube, String> {
        let mut size: Option<usize> = None;
        let mut domain_min = [0.0; 3];
        let mut domain_max = [1.0; 3];
        let mut entries: Vec<[f32; 3]> = Vec::new();

        for (index, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            let line_number = index + 1;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let tokens: Vec<&str> = line.split_whitespace().collect();
            let starts_uppercase = tokens[0]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase());
            if starts_uppercase {
                let keyword = tokens[0];
                if keyword == "TITLE" {
                    continue;
                }
                if keyword == "LUT_3D_SIZE" {
                    if tokens.len() != 2 {
                        return Err(format!("line {}: invalid LUT_3D_SIZE", line_number));
                    }
                    size = Some(tokens[1].parse::<usize>().map_err(|_| {
                        format!("line {}: invalid LUT_3D_SIZE", line_number)
                    })?);
                } else if keyword == "DOMAIN_MIN" {
                    if tokens.len() != 4 {
                        return Err(format!("line {}: invalid DOMAIN_MIN", line_number));
                    }
                    let mut values = [0.0; 3];
                    for (i, token) in tokens[1..4].iter().enumerate() {
                        values[i] = token.parse::<f32>().map_err(|_| {
                            format!("line {}: invalid DOMAIN_MIN value", line_number)
                        })?;
                    }
                    if values.iter().any(|v| !v.is_finite()) {
                        return Err(format!("line {}: non-finite DOMAIN_MIN", line_number));
                    }
                    domain_min = values;
                } else if keyword == "DOMAIN_MAX" {
                    if tokens.len() != 4 {
                        return Err(format!("line {}: invalid DOMAIN_MAX", line_number));
                    }
                    let mut values = [0.0; 3];
                    for (i, token) in tokens[1..4].iter().enumerate() {
                        values[i] = token.parse::<f32>().map_err(|_| {
                            format!("line {}: invalid DOMAIN_MAX value", line_number)
                        })?;
                    }
                    if values.iter().any(|v| !v.is_finite()) {
                        return Err(format!("line {}: non-finite DOMAIN_MAX", line_number));
                    }
                    domain_max = values;
                } else if keyword == "LUT_1D_SIZE" {
                    return Err("1D LUTs are not supported".to_string());
                } else if keyword == "LUT_3D_INPUT_RANGE" {
                    if tokens.len() != 3 {
                        return Err(format!("line {}: invalid LUT_3D_INPUT_RANGE", line_number));
                    }
                    let min_value = tokens[1].parse::<f32>().map_err(|_| {
                        format!("line {}: invalid LUT_3D_INPUT_RANGE", line_number)
                    })?;
                    let max_value = tokens[2].parse::<f32>().map_err(|_| {
                        format!("line {}: invalid LUT_3D_INPUT_RANGE", line_number)
                    })?;
                    domain_min = [min_value; 3];
                    domain_max = [max_value; 3];
                } else {
                    continue;
                }
                continue;
            }

            if tokens.len() != 3 {
                return Err(format!("line {}: expected three floats", line_number));
            }
            let mut values = [0.0; 3];
            for (i, token) in tokens.iter().enumerate() {
                values[i] = token.parse::<f32>().map_err(|_| {
                    format!("line {}: expected three floats", line_number)
                })?;
            }
            if values.iter().any(|v| !v.is_finite()) {
                return Err(format!("line {}: non-finite value", line_number));
            }
            entries.push([values[0], values[1], values[2]]);
        }

        let size = size.ok_or_else(|| "missing LUT_3D_SIZE".to_string())?;
        if size < 2 || size > 256 {
            return Err(format!("LUT size {} outside 2..=256", size));
        }
        let expected = size * size * size;
        if entries.len() != expected {
            return Err(format!(
                "LUT data count {} does not match {}",
                entries.len(),
                expected
            ));
        }
        if (0..3).any(|i| domain_max[i] <= domain_min[i]) {
            return Err("domain_max <= domain_min".to_string());
        }

        Ok(Cube {
            size,
            domain_min,
            domain_max,
            data: entries,
        })
    }

    pub fn load(path: &Path) -> Result<Cube, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Cube::parse(&text)
    }

    /// Trilinear interpolation; input mapped from the domain to 0..1 and clamped. Output not clamped.
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let s = self.size - 1;
        let mut i0 = [0usize; 3];
        let mut f = [0f32; 3];
        for c in 0..3 {
            let x = ((rgb[c] - self.domain_min[c]) / (self.domain_max[c] - self.domain_min[c])).clamp(0.0, 1.0) * s as f32;
            let i = (x.floor() as usize).min(s - 1);
            i0[c] = i;
            f[c] = x - i as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[r + g * self.size + b * self.size * self.size];
        let mut out = [0f32; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for db in 0..2 {
                let wb = if db == 0 { 1.0 - f[2] } else { f[2] };
                for dg in 0..2 {
                    let wg = if dg == 0 { 1.0 - f[1] } else { f[1] };
                    for dr in 0..2 {
                        let wr = if dr == 0 { 1.0 - f[0] } else { f[0] };
                        acc += at(i0[0] + dr, i0[1] + dg, i0[2] + db)[k] * wr * wg * wb;
                    }
                }
            }
            *o = acc;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn lut(n: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> String {
        let mut s = format!("TITLE \"t\"\n# comment\nLUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let v = f([r, g, b].map(|x| x as f32 / (n - 1) as f32));
                    s += &format!("{} {} {}\n", v[0], v[1], v[2]);
                }
            }
        }
        s
    }
    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }
    #[test]
    fn identity_and_inverse() {
        let id = Cube::parse(&lut(2, |c| c)).unwrap();
        assert!(near(id.sample([0.25, 0.5, 0.75]), [0.25, 0.5, 0.75]));
        let inv = Cube::parse(&lut(3, |c| c.map(|v| 1.0 - v))).unwrap();
        assert!(near(inv.sample([0.1, 0.6, 0.9]), [0.9, 0.4, 0.1]));
        // a channel swap proves the axis order (red fastest)
        let sw = Cube::parse(&lut(2, |c| [c[2], c[1], c[0]])).unwrap();
        assert!(near(sw.sample([1.0, 0.0, 0.25]), [0.25, 0.0, 1.0]));
    }
    #[test]
    fn domain_and_errors() {
        let id = Cube::parse(&format!("DOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n{}", lut(2, |c| c))).unwrap();
        assert!(near(id.sample([1.0, 1.0, 1.0]), [0.5, 0.5, 0.5]));
        assert!(Cube::parse("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(Cube::parse("LUT_1D_SIZE 2\n0 0 0\n1 1 1\n").is_err());
        assert!(Cube::parse(&lut(2, |c| c).replace("0 0 0", "0 0")).is_err());
    }
}
