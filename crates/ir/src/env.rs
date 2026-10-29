//! Environment variables. Every setting has an `AGENTFX_*` name; the older `REEL_*` name
//! (same suffix) is still read as a fallback, so `AGENTFX_FFMPEG` wins over `REEL_FFMPEG`.
use std::ffi::OsString;

fn legacy(name: &str) -> Option<String> {
    name.strip_prefix("AGENTFX_").map(|s| format!("REEL_{s}"))
}

/// Like `std::env::var`, with the `REEL_*` fallback for an `AGENTFX_*` name.
pub fn var(name: &str) -> Result<String, std::env::VarError> {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => match legacy(name) {
            Some(old) => std::env::var(old),
            None => Err(std::env::VarError::NotPresent),
        },
        r => r,
    }
}

/// Like `std::env::var_os`, with the `REEL_*` fallback for an `AGENTFX_*` name.
pub fn var_os(name: &str) -> Option<OsString> {
    std::env::var_os(name).or_else(|| legacy(name).and_then(std::env::var_os))
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_name() {
        assert_eq!(super::legacy("AGENTFX_FFMPEG").as_deref(), Some("REEL_FFMPEG"));
        assert_eq!(super::legacy("HOME"), None);
    }
}
