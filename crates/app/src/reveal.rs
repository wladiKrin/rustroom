//! "Show in Finder" — hand a photo's file to the platform's file manager, selected.
//!
//! ⚠ A lib module with no egui: the one platform-specific thing here is which program is asked,
//! and that is decided once, in one place, rather than at every menu that wants it.

use std::path::Path;

/// The menu item that calls [`in_file_manager`], named for this platform's file manager (v0.1).
pub const SHOW_LABEL: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else if cfg!(windows) {
    "Show in Explorer"
} else {
    "Show in file manager"
};

/// Open the file manager with `path` selected (macOS, Windows) or its folder shown (elsewhere).
pub fn in_file_manager(path: &Path) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v0.1: Finder is a Mac's — Windows and Linux name their own file manager.
    #[test]
    fn the_label_names_finder_only_on_a_mac() {
        assert!(
            SHOW_LABEL.starts_with("Show in "),
            "positive control: {SHOW_LABEL}"
        );
        assert_eq!(SHOW_LABEL.contains("Finder"), cfg!(target_os = "macos"));
        if cfg!(windows) {
            assert_eq!(SHOW_LABEL, "Show in Explorer");
        }
    }
}
