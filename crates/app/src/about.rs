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

    /// The build script really delivers the commit: in a git checkout (CI's included) it is never `unknown`. A
    /// build from a source archive has no `.git`, and skips the check.
    #[test]
    fn a_checkout_build_knows_its_commit() {
        let git = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.git");
        if git.exists() {
            let c = option_env!("RUSTROOM_COMMIT").unwrap_or("unknown");
            assert!(c != "unknown" && c.len() >= 7, "{c}");
            assert!(current().ends_with(&format!("({c})")), "{}", current());
        }
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
