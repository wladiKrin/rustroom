//! "Show in Finder" — hand a photo's file to the platform's file manager, selected.
//!
//! ⚠ A lib module with no egui: the one platform-specific thing here is which program is asked,
//! and that is decided once, in one place, rather than at every menu that wants it.

use std::path::Path;

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
