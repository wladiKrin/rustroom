//! Which version of which file the app is looking at.
//!
//! ⚠ This replaces `PathBuf` as a photo's identity. A virtual copy is the same file with
//! different edits, so a path alone can no longer say which photo is meant.

use std::path::{Path, PathBuf};

/// One version of one file: the file's path plus which copy of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhotoId {
    pub path: PathBuf,
    /// `0` is the original. Copies are numbered from 1 and **never reused**.
    ///
    /// ⚠ This one number is the identity, the number shown in the UI, and the export filename
    /// suffix. Keeping them the same value means they cannot drift apart.
    pub copy: u32,
}

impl PhotoId {
    pub fn new(path: PathBuf, copy: u32) -> Self {
        Self { path, copy }
    }

    /// The file itself, as opposed to any of its virtual copies.
    pub fn original(path: PathBuf) -> Self {
        Self { path, copy: 0 }
    }

    pub fn is_original(&self) -> bool {
        self.copy == 0
    }

    /// What an export filename appends: nothing for the original, `_N` for copy N.
    ///
    /// ⚠ Underscore, not hyphen. `batch_export::dest_for` already uses `-N` for a *different*
    /// meaning — "the Nth file wanting this name in this folder", assigned at export time. This
    /// one is a property of the photo and stable forever. One separator per meaning.
    pub fn suffix(&self) -> String {
        if self.is_original() {
            String::new()
        } else {
            format!("_{}", self.copy)
        }
    }

    /// What an export filename appends for a **named** copy: the name, sanitized for a filename.
    ///
    /// ⚠⚠ **The name overrides the number here, and that is the whole point of naming a copy.**
    /// Controlling the export filename is what the name is *for* — `DSCF6223_B&W.jpg` rather than
    /// `DSCF6223_2.jpg`. The number remains the identity; the name is an explicit, user-chosen
    /// label on top of it, not a second derived fact that could silently disagree.
    ///
    /// ⚠ Sanitized through `safe_name`, the single guard that stops a typed name escaping its
    /// directory — the name reaches the filesystem here exactly as a preset's does. A name that
    /// sanitizes to nothing falls back to the number rather than producing a bare `_`.
    pub fn export_suffix(copy: u32, name: Option<&str>) -> String {
        if copy == 0 {
            return String::new();
        }
        match name.map(crate::safe_name::sanitize) {
            Some(n) if !n.is_empty() => format!("_{n}"),
            _ => format!("_{copy}"),
        }
    }

    /// What the UI shows: the file name, plus ` / N` for a copy.
    pub fn label(&self) -> String {
        let name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(unnamed)");
        if self.is_original() {
            name.to_string()
        } else {
            format!("{name} / {}", self.copy)
        }
    }

    /// Borrow the path, for the many APIs that still key on the file rather than the version —
    /// `BufferCache` above all, where copies deliberately share one decoded buffer.
    pub fn file(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_original_is_copy_zero() {
        let id = PhotoId::original(PathBuf::from("/x/a.RAF"));
        assert_eq!(id.copy, 0);
        assert!(id.is_original());
    }

    #[test]
    fn a_copy_is_not_the_original() {
        let id = PhotoId::new(PathBuf::from("/x/a.RAF"), 1);
        assert!(!id.is_original());
    }

    #[test]
    fn two_copies_of_one_file_are_different_photos() {
        // ⚠ The whole point of the type. If these compared equal, two copies would share a
        // history stash and a session, and editing one would appear to edit the other.
        let a = PhotoId::new(PathBuf::from("/x/a.RAF"), 1);
        let b = PhotoId::new(PathBuf::from("/x/a.RAF"), 2);
        assert_ne!(a, b);
    }

    #[test]
    fn the_suffix_is_empty_for_the_original_and_numbered_otherwise() {
        // ⚠ **One number used everywhere**: this same value is the identity, the label and the
        // export filename suffix. A display number offset from the identity number is two facts
        // that can disagree — the failure `render_max_dim` and `fit_image_size` were both fixed
        // for. Deliberately *not* Lightroom's labelling, where the original is "Copy 1".
        assert_eq!(PhotoId::original(PathBuf::from("/x/a.RAF")).suffix(), "");
        assert_eq!(PhotoId::new(PathBuf::from("/x/a.RAF"), 1).suffix(), "_1");
        assert_eq!(PhotoId::new(PathBuf::from("/x/a.RAF"), 12).suffix(), "_12");
    }

    #[test]
    fn a_named_copy_exports_under_its_name() {
        assert_eq!(
            PhotoId::export_suffix(0, Some("B&W")),
            "",
            "copy 0 is the file"
        );
        assert_eq!(PhotoId::export_suffix(2, None), "_2");
        assert_eq!(
            PhotoId::export_suffix(2, Some("Black and White")),
            "_Black and White"
        );
        // ⚠ The *shown* name keeps what you typed; only the filename is sanitized, so "B&W" is
        // still labelled "B&W" in the Library while exporting as `_B_W`.
        assert_eq!(PhotoId::export_suffix(2, Some("B&W")), "_B_W");
    }

    #[test]
    fn an_export_name_cannot_escape_its_directory() {
        // ⚠ The one security-relevant line in this feature: the name is free text that becomes a
        // path. Same guard, and the same reason, as a preset's filename.
        let got = PhotoId::export_suffix(2, Some("../../evil"));
        assert!(!got.contains('/') && !got.contains('.'), "got {got}");
        assert!(!PhotoId::export_suffix(2, Some("a/b")).contains('/'));
    }

    #[test]
    fn a_name_that_sanitizes_to_nothing_falls_back_to_the_number() {
        // ⚠ Otherwise the suffix would be a bare `_`, which reads as a broken filename rather
        // than as a copy.
        assert_eq!(PhotoId::export_suffix(2, Some("   ")), "_2");
        assert_eq!(PhotoId::export_suffix(2, Some("")), "_2");
    }

    #[test]
    fn the_label_names_the_file_and_the_copy() {
        assert_eq!(
            PhotoId::original(PathBuf::from("/x/DSCF6223.RAF")).label(),
            "DSCF6223.RAF"
        );
        assert_eq!(
            PhotoId::new(PathBuf::from("/x/DSCF6223.RAF"), 1).label(),
            "DSCF6223.RAF / 1"
        );
    }
}
