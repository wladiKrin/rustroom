//! What follows a photo recognised as moved on disk (M9.9): its sidecars.
//!
//! ⚠ The catalog half — rows following the file — is `Catalog::relink_moved`. This is the file
//! half, and the rule is one line: a sidecar left in the old folder is moved beside the photo,
//! **never over one already there**. A sidecar that travelled with the file (Finder moved both)
//! is the ordinary case and needs nothing.

use std::path::Path;

/// The status-line note for `n` recognised moves — " · Recognised 3 moved photos", or nothing.
/// One wording for opening a folder and Reindex.
pub fn recognised(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => " · Recognised 1 moved photo".to_string(),
        n => format!(" · Recognised {n} moved photos"),
    }
}

/// Move `from`'s left-behind sidecars — Rustroom's edits (`.RAF.json`) and the XMP (`stem.xmp`) —
/// beside `to`. Returns how many moved. An error moving one is skipped: the photo's catalog data
/// has already followed it, and a failed rename leaves the sidecar where it was, intact.
pub fn carry_sidecars(from: &Path, to: &Path) -> usize {
    let pairs = [
        (crate::sidecar::path(from), crate::sidecar::path(to)),
        (
            crate::xmp_sync::sidecar_path(from),
            crate::xmp_sync::sidecar_path(to),
        ),
    ];
    pairs
        .iter()
        .filter(|(old, new)| old.is_file() && !new.exists())
        .filter(|(old, new)| std::fs::rename(old, new).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = test_dir::path(&format!("carry-{tag}"));
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::create_dir_all(d.join("b")).unwrap();
        d
    }

    #[test]
    fn left_behind_sidecars_follow_the_photo() {
        let d = dir("follow");
        let (from, to) = (d.join("a/x.RAF"), d.join("b/x.RAF"));
        std::fs::write(crate::sidecar::path(&from), "edits").unwrap();
        std::fs::write(crate::xmp_sync::sidecar_path(&from), "xmp").unwrap();
        assert_eq!(carry_sidecars(&from, &to), 2);
        assert_eq!(
            std::fs::read_to_string(crate::sidecar::path(&to)).unwrap(),
            "edits"
        );
        assert_eq!(
            std::fs::read_to_string(crate::xmp_sync::sidecar_path(&to)).unwrap(),
            "xmp"
        );
        assert!(!crate::sidecar::path(&from).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⚠⚠ Never over one already there: the sidecar beside the photo is the one that travelled
    /// with it (or newer), and the left-behind one is kept, not destroyed.
    #[test]
    fn an_existing_sidecar_is_never_overwritten() {
        let d = dir("keep");
        let (from, to) = (d.join("a/x.RAF"), d.join("b/x.RAF"));
        std::fs::write(crate::sidecar::path(&from), "old").unwrap();
        std::fs::write(crate::sidecar::path(&to), "travelled").unwrap();
        assert_eq!(carry_sidecars(&from, &to), 0);
        assert_eq!(
            std::fs::read_to_string(crate::sidecar::path(&to)).unwrap(),
            "travelled"
        );
        assert_eq!(
            std::fs::read_to_string(crate::sidecar::path(&from)).unwrap(),
            "old"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
