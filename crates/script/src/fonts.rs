//! Custom font files (`font "Brand" src="fonts/Brand.woff2"`): loading (TTF/OTF/TTC as is,
//! WOFF/WOFF2 decoded with `wuff`), the axes a variable font has, and OpenType feature /
//! variation settings as CSS (Blitz hands both to Parley, the Chrome backend reads the same CSS).
use anyhow::{bail, Context, Result};
use std::path::Path;

/// Font file bytes as SFNT (WOFF/WOFF2 decompressed), checked to parse.
pub fn load(path: &Path) -> Result<Vec<u8>> {
    let raw = std::fs::read(path).with_context(|| format!("font file {}", path.display()))?;
    let data = match raw.get(0..4) {
        Some(b"wOFF") => wuff::decompress_woff1(&raw).map_err(|e| anyhow::anyhow!("WOFF decode: {e:?}"))?,
        Some(b"wOF2") => wuff::decompress_woff2(&raw).map_err(|e| anyhow::anyhow!("WOFF2 decode: {e:?}"))?,
        Some([0, 1, 0, 0]) | Some(b"OTTO") | Some(b"true") | Some(b"ttcf") => raw,
        _ => bail!("not a font file (TTF, OTF, WOFF or WOFF2 expected)"),
    };
    match skrifa::raw::FileRef::new(&data) {
        Ok(_) => Ok(data),
        Err(e) => bail!("font does not parse: {e}"),
    }
}

/// Variation axes of the font's first face: (tag, min, default, max).
pub fn axes(data: &[u8]) -> Vec<(String, f32, f32, f32)> {
    use skrifa::MetadataProvider;
    let Ok(f) = skrifa::FontRef::from_index(data, 0) else { return vec![] };
    f.axes().iter().map(|a| (a.tag().to_string(), a.min_value(), a.default_value(), a.max_value())).collect()
}

/// True when the font maps `c` to a glyph.
pub fn has_char(data: &[u8], c: char) -> bool {
    use skrifa::MetadataProvider;
    skrifa::FontRef::from_index(data, 0).ok().and_then(|f| f.charmap().map(c)).is_some()
}

fn tag_ok(t: &str) -> bool {
    t.len() == 4 && t.bytes().all(|b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\'')
}

/// `features="ss01 off, tnum, cv05 2"` -> CSS `font-feature-settings` value
/// (`"ss01" 0, "tnum" 1, "cv05" 2`). Err: the offending entry.
pub fn features_css(s: &str) -> Result<String, String> {
    let mut out = vec![];
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let mut it = part.split_whitespace();
        let tag = it.next().unwrap_or("").trim_matches(['"', '\'']);
        if !tag_ok(tag) {
            return Err(part.to_string());
        }
        let v = match it.next() {
            None | Some("on") => 1,
            Some("off") => 0,
            Some(n) => n.parse::<u32>().map_err(|_| part.to_string())?,
        };
        if it.next().is_some() {
            return Err(part.to_string());
        }
        out.push(format!("\"{tag}\" {v}"));
    }
    Ok(out.join(","))
}

/// `axes="wdth 80, GRAD 50"` -> [(tag, value)]. Err: the offending entry.
pub fn parse_axes(s: &str) -> Result<Vec<(String, f64)>, String> {
    let mut out = vec![];
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let mut it = part.split_whitespace();
        let tag = it.next().unwrap_or("").trim_matches(['"', '\'']);
        let v = it.next().and_then(|v| v.parse::<f64>().ok());
        match (tag_ok(tag), v, it.next()) {
            (true, Some(v), None) if v.is_finite() => out.push((tag.to_string(), v)),
            _ => return Err(part.to_string()),
        }
    }
    Ok(out)
}

/// CSS `font-variation-settings` value for axes (`"wdth" 80,"GRAD" 50`).
pub fn variations_css(axes: &[(String, f64)]) -> String {
    axes.iter().map(|(t, v)| format!("\"{t}\" {}", crate::html::fnum(*v))).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_and_axis_syntax() {
        assert_eq!(features_css("ss01 off, tnum, cv05 2").unwrap(), "\"ss01\" 0,\"tnum\" 1,\"cv05\" 2");
        assert!(features_css("toolong on").is_err());
        assert_eq!(parse_axes("wdth 80, GRAD 50").unwrap(), vec![("wdth".to_string(), 80.0), ("GRAD".to_string(), 50.0)]);
        assert!(parse_axes("wdth").is_err());
    }
}
