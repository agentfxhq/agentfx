//! Path helpers that behave the same on every platform.
use std::path::{Path, PathBuf};

/// `std::fs::canonicalize`, but on Windows without the verbatim prefix (`\\?\C:\x` becomes
/// `C:\x`). A verbatim path takes `/` literally, so a later `join("out/video.mp4")` would
/// name a file that does not exist, and some tools (ffmpeg concat lists, Python) reject
/// the prefix. Elsewhere this is exactly `std::fs::canonicalize`.
pub fn canonicalize(p: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(p).map(simplify)
}

/// Drop a Windows verbatim disk prefix (`\\?\C:\...`) when the plain form is equivalent
/// (shorter than `MAX_PATH`, valid UTF-8). UNC and device paths, and every path on other
/// platforms, are returned unchanged.
pub fn simplify(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if let Some(Component::Prefix(pre)) = p.components().next() {
            if let Prefix::VerbatimDisk(_) = pre.kind() {
                if let Some(rest) = p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
                    if rest.len() < 260 {
                        return PathBuf::from(rest);
                    }
                }
            }
        }
    }
    p
}

/// The user's home directory: `$HOME`, else (Windows) `%USERPROFILE%`. None when neither is set.
pub fn home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
    #[cfg(windows)]
    let home = home.or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()));
    home.map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_joins_with_slashes() {
        let dir = canonicalize(".").unwrap();
        assert!(dir.is_absolute());
        // the build tree's manifest must resolve through a `/` join on every platform
        assert!(dir.join("src/paths.rs").is_file(), "{}", dir.display());
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_disk_prefix_is_dropped() {
        assert_eq!(simplify(PathBuf::from(r"\\?\C:\x\y")), PathBuf::from(r"C:\x\y"));
        assert_eq!(simplify(PathBuf::from(r"\\?\UNC\srv\share\x")), PathBuf::from(r"\\?\UNC\srv\share\x"));
        assert_eq!(simplify(PathBuf::from(r"C:\x")), PathBuf::from(r"C:\x"));
    }
}
