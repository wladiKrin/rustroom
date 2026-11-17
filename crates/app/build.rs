//! The commit this build was made from, for the About window (v0.1): `RUSTROOM_COMMIT`, the short hash of HEAD,
//! or `unknown` outside a git checkout (a source archive) or without `git`. ⚠ Never fails the build.
//!
//! ⚠ Re-run when HEAD moves — so every new commit rebuilds the `app` crate once; that is the price of the
//! commit in About.

use std::path::Path;
use std::process::Command;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let git = Path::new(&manifest).join("../../.git");
    let head = git.join("HEAD");
    if head.exists() {
        println!("cargo:rerun-if-changed={}", head.display());
        // HEAD names a branch: its ref file (or packed-refs) is what moves on a commit.
        if let Ok(text) = std::fs::read_to_string(&head) {
            if let Some(r) = text.trim().strip_prefix("ref: ") {
                println!("cargo:rerun-if-changed={}", git.join(r).display());
            }
        }
        let packed = git.join("packed-refs");
        if packed.exists() {
            println!("cargo:rerun-if-changed={}", packed.display());
        }
    }
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&manifest)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUSTROOM_COMMIT={commit}");
}
