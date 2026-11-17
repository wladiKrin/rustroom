//! The About window's version line (v0.1): the package version, and the commit of a build made from a git
//! checkout — `build.rs` reads it into `RUSTROOM_COMMIT`, `unknown` outside git. No egui.

/// `Version 0.1.0 (a1b2c3d)`, or `Version 0.1.0` when the commit is unknown — a build from a source archive.
pub fn version_line(version: &str, commit: &str) -> String {
    match commit.trim() {
        "" | "unknown" => format!("Version {version}"),
        c => format!("Version {version} ({c})"),
    }
}

/// This build's line.
pub fn current() -> String {
    version_line(
        env!("CARGO_PKG_VERSION"),
        option_env!("RUSTROOM_COMMIT").unwrap_or("unknown"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_git_build_names_its_commit() {
        assert_eq!(version_line("0.1.0", "a1b2c3d"), "Version 0.1.0 (a1b2c3d)");
    }

    #[test]
    fn a_build_outside_git_shows_the_version_alone() {
        assert_eq!(version_line("0.1.0", "unknown"), "Version 0.1.0");
        assert_eq!(version_line("0.1.0", ""), "Version 0.1.0");
    }

    /// The build script really delivers the commit: whenever git itself can see this checkout as its own (CI's
    /// included) it is never `unknown`. Where it cannot — a source archive, no `git`, a container's "dubious
    /// ownership" — the build script cannot either, and this skips (the review's point: it used to test the host).
    #[test]
    fn a_checkout_build_knows_its_commit() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let top = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(manifest)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok());
        let ours = top.is_some_and(|t| {
            std::fs::canonicalize(t.trim()).ok()
                == std::fs::canonicalize(manifest.join("../..")).ok()
        });
        if !ours {
            eprintln!("skipping: git does not see this checkout");
            return;
        }
        let c = option_env!("RUSTROOM_COMMIT").unwrap_or("unknown");
        assert!(c != "unknown" && c.len() >= 7, "{c}");
        assert!(current().ends_with(&format!("({c})")), "{}", current());
    }

    #[test]
    fn the_current_line_is_this_packages_version() {
        assert!(
            current().starts_with(&format!("Version {}", env!("CARGO_PKG_VERSION"))),
            "{}",
            current()
        );
    }
}
