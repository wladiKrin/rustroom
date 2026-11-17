//! The commit this build was made from, for the About window (v0.1): `RUSTROOM_COMMIT`, the short hash of HEAD,
//! or `unknown` — outside a git checkout of THIS repository (a source archive, even one unpacked inside another
//! repository, as a distribution's packaging does) or without a working `git`. ⚠ Never fails the build.
//!
//! ⚠ What is watched must exist in every layout, or cargo reruns this script — and recompiles the whole `app`
//! crate — on EVERY build (v0.1's review): a branch's loose ref file disappears when `git gc` packs refs, and a
//! worktree's `.git` is a file. So: the checkout's own `HEAD`, the shared `refs/heads` DIRECTORY (cargo scans a
//! directory recursively, and a commit writes a loose ref there again even after packing) and `packed-refs`
//! when present. Every new commit rebuilds `app` once — the price of the commit in About.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// `p`, made absolute against `base` when git answered with a relative path.
fn absolute(base: &Path, p: &str) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let workspace = manifest.join("../..");
    let commit = (|| {
        // Only this repository's own checkout: a source archive unpacked inside another repository must not
        // report that repository's commit.
        let top = git(&manifest, &["rev-parse", "--show-toplevel"])?;
        if std::fs::canonicalize(&top).ok()? != std::fs::canonicalize(&workspace).ok()? {
            return None;
        }
        let git_dir = absolute(&manifest, &git(&manifest, &["rev-parse", "--git-dir"])?);
        let common = absolute(
            &manifest,
            &git(&manifest, &["rev-parse", "--git-common-dir"])?,
        );
        for p in [
            git_dir.join("HEAD"),
            common.join("refs/heads"),
            common.join("packed-refs"),
        ] {
            if p.exists() {
                println!("cargo:rerun-if-changed={}", p.display());
            }
        }
        git(&manifest, &["rev-parse", "--short", "HEAD"])
    })()
    .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUSTROOM_COMMIT={commit}");
}
