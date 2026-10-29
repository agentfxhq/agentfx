//! Interpolation of animated paint/layout CSS values (repaint states, M1 native path).
//! Supported: px lengths, unitless numbers, percentages, colors (#hex, rgb()/rgba(),
//! a few keywords), and space-separated lists of those with matching shapes.
//! Colors interpolate in premultiplied sRGB (CSS Color 4 for legacy colors).
//! Anything else is an error (fail loudly).
use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Len(f64, &'static str),
    Color([f64; 4]),
}

fn hex(s: &str) -> Option<[f64; 4]> {
    let h = s.strip_prefix('#')?;
    let d = |c: &str| u8::from_str_radix(c, 16).ok().map(|v| v as f64 / 255.0);
    let dd = |c: char| u8::from_str_radix(&c.to_string().repeat(2), 16).ok().map(|v| v as f64 / 255.0);
    let ch: Vec<char> = h.chars().collect();
    Some(match ch.len() {
        3 => [dd(ch[0])?, dd(ch[1])?, dd(ch[2])?, 1.0],
        4 => [dd(ch[0])?, dd(ch[1])?, dd(ch[2])?, dd(ch[3])?],
        6 => [d(&h[0..2])?, d(&h[2..4])?, d(&h[4..6])?, 1.0],
        8 => [d(&h[0..2])?, d(&h[2..4])?, d(&h[4..6])?, d(&h[6..8])?],
        _ => return None,
    })
}

fn func_color(s: &str) -> Option<[f64; 4]> {
    let inner = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|p| !p.is_empty()).collect();
    if parts.len() < 3 || parts.len() > 4 {
        return None;
    }
    let ch = |p: &str| -> Option<f64> {
        if let Some(v) = p.strip_suffix('%') { v.parse::<f64>().ok().map(|v| v / 100.0) } else { p.parse::<f64>().ok().map(|v| v / 255.0) }
    };
    let a = match parts.get(3) {
        Some(p) => if let Some(v) = p.strip_suffix('%') { v.parse::<f64>().ok()? / 100.0 } else { p.parse::<f64>().ok()? },
        None => 1.0,
    };
    Some([ch(parts[0])?, ch(parts[1])?, ch(parts[2])?, a])
}

fn tokens(s: &str) -> Result<Vec<Tok>> {
    // split at top-level whitespace (keep rgb(...) together)
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    let mut words = Vec::new();
    for c in s.trim().chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    for w in words {
        let w = w.to_ascii_lowercase();
        let t = if let Some(c) = hex(&w).or_else(|| func_color(&w)) {
            Tok::Color(c)
        } else if w == "transparent" {
            Tok::Color([0.0; 4])
        } else if w == "white" {
            Tok::Color([1.0, 1.0, 1.0, 1.0])
        } else if w == "black" {
            Tok::Color([0.0, 0.0, 0.0, 1.0])
        } else if let Some(v) = w.strip_suffix("px") {
            Tok::Len(v.parse()?, "px")
        } else if let Some(v) = w.strip_suffix('%') {
            Tok::Len(v.parse()?, "%")
        } else if let Ok(v) = w.parse::<f64>() {
            Tok::Len(v, "")
        } else {
            bail!("cannot interpolate CSS value token {w:?} (in {s:?})");
        };
        out.push(t);
    }
    if out.is_empty() {
        bail!("empty CSS value");
    }
    Ok(out)
}

fn fmt_color(c: [f64; 4]) -> String {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("rgba({}, {}, {}, {})", b(c[0]), b(c[1]), b(c[2]), (c[3].clamp(0.0, 1.0) * 1e4).round() / 1e4)
}

/// Value of `a` interpolated towards `b` by `u` (CSS animation semantics). `u` may
/// overshoot (eased); lengths/numbers extrapolate, colors clamp.
pub fn interpolate(a: &str, b: &str, u: f64) -> Result<String> {
    // step easings (discrete holds) never interpolate: any value works, incl. var()
    if a == b || u <= 0.0 {
        return Ok(a.to_string());
    }
    if u >= 1.0 {
        return Ok(b.to_string());
    }
    let (ta, tb) = (tokens(a)?, tokens(b)?);
    if ta.len() != tb.len() {
        bail!("cannot interpolate {a:?} -> {b:?} (different shapes)");
    }
    let mut out = Vec::new();
    for (x, y) in ta.iter().zip(&tb) {
        out.push(match (*x, *y) {
            // unitless 0 interpolates with lengths
            (Tok::Len(p, ua), Tok::Len(q, ub)) if ua == ub || (p == 0.0 && ua.is_empty()) || (q == 0.0 && ub.is_empty()) => {
                let unit = if ua.is_empty() { ub } else { ua };
                let v = p + (q - p) * u;
                format!("{}{unit}", (v * 1e4).round() / 1e4)
            }
            (Tok::Color(p), Tok::Color(q)) => {
                // premultiplied interpolation
                let pa = [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]];
                let qa = [q[0] * q[3], q[1] * q[3], q[2] * q[3], q[3]];
                let m: Vec<f64> = (0..4).map(|i| pa[i] + (qa[i] - pa[i]) * u).collect();
                let al = m[3].clamp(0.0, 1.0);
                if al <= 0.0 { fmt_color([0.0; 4]) } else { fmt_color([m[0] / al, m[1] / al, m[2] / al, al]) }
            }
            _ => bail!("cannot interpolate {a:?} -> {b:?} (incompatible tokens)"),
        });
    }
    Ok(out.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lengths_colors_lists() {
        assert_eq!(interpolate("12px", "30px", 0.5).unwrap(), "21px");
        assert_eq!(interpolate("0", "420px", 0.25).unwrap(), "105px");
        assert_eq!(interpolate("#000", "#fff", 0.5).unwrap(), "rgba(128, 128, 128, 1)");
        assert_eq!(interpolate("0 12px", "4px 20px", 0.5).unwrap(), "2px 16px");
        assert_eq!(interpolate("rgba(0,0,0,0)", "#ff0000", 0.5).unwrap(), "rgba(255, 0, 0, 0.5)");
        assert!(interpolate("auto", "10px", 0.5).is_err());
        assert!(interpolate("1px 2px", "3px", 0.5).is_err());
    }
}
