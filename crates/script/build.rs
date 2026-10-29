//! Build-time commit hash for `--version` (AGENTFX_GIT_HASH): env AGENTFX_GIT_HASH (or
//! REEL_GIT_HASH), else `git rev-parse --short HEAD`. Also exports AGENTFX_ROOT, the
//! repository root (data files and examples for tests and the build-tree fallback).
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let o = Command::new("git").args(args).output().ok()?;
    if !o.status.success() {
        return None;
    }
    Some(String::from_utf8(o.stdout).ok()?.trim().to_string()).filter(|s| !s.is_empty())
}

fn main() {
    println!("cargo:rerun-if-env-changed=AGENTFX_GIT_HASH");
    println!("cargo:rerun-if-env-changed=REEL_GIT_HASH");
    println!("cargo:rerun-if-changed=build.rs");
    let hash = if let Ok(h) = std::env::var("AGENTFX_GIT_HASH").or_else(|_| std::env::var("REEL_GIT_HASH")) {
        h
    } else {
        if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
            // only existing files: a missing one would rerun this script on every build
            let refname = git(&["symbolic-ref", "-q", "HEAD"]);
            for f in ["HEAD", "packed-refs"].into_iter().map(String::from).chain(refname) {
                let path = format!("{dir}/{f}");
                if std::path::Path::new(&path).exists() {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
        }
        git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into())
    };
    println!("cargo:rustc-env=AGENTFX_GIT_HASH={hash}");
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    let root = root.canonicalize().unwrap_or(root);
    println!("cargo:rustc-env=AGENTFX_ROOT={}", root.display());
}
