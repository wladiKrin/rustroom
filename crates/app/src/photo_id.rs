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

    /// The filename stem a photo exports under, before any de-duplication suffix.
    ///
    /// ⚠⚠ **A copy's name *is* the export name — it is not appended to the file's.** Naming
    /// copy 2 "Warm" gives `Warm.jpg`, not `DSCF6223_Warm.jpg`. That is what the name is for, and
    /// it is why the field shows a whole filename rather than a bare label: what you type is what
    /// you get. Only an *unnamed* copy falls back to `{stem}_{copy}`, which is also exactly what
    /// its field shows, so the default and the export cannot disagree.
    ///
    /// ⚠ Any extension on the name is dropped — the export format decides that, and
    /// `Warm.RAF` exported as JPEG must not become `Warm.RAF.jpg`.
    ///
    /// ⚠ Sanitized through `safe_name`, the single guard that stops a typed name escaping its
    /// directory — the name reaches the filesystem here exactly as a preset's does. A name that
    /// sanitizes to nothing falls back to the number rather than producing an empty filename.
    pub fn export_stem(file_stem: &str, copy: u32, name: Option<&str>) -> String {
        if copy == 0 {
            // ⚠ Copy 0 *is* the file, so its name is the filename and a stored one is ignored.
            return file_stem.to_string();
        }
        let typed = name
            .map(|n| n.rsplit_once('.').map_or(n, |(before, _)| before))
            .map(crate::safe_name::sanitize);
        match typed {
            Some(n) if !n.is_empty() => n,
            _ => format!("{file_stem}_{copy}"),
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
    fn a_copys_name_replaces_the_filename_rather_than_extending_it() {
        // ⚠⚠ The name *is* the export name. Appending it to the file's stem — which is what this
        // did first — means a copy called "Warm" comes out as `DSCF6223_Warm`, and there is then
        // no way to give a copy a name of its own at all.
        assert_eq!(PhotoId::export_stem("DSCF6223", 2, Some("Warm")), "Warm");
        assert!(!PhotoId::export_stem("DSCF6223", 2, Some("Warm")).contains("DSCF6223"));
    }

    #[test]
    fn an_unnamed_copy_falls_back_to_stem_and_number() {
        // ⚠ The same string `file_rename::default_copy_name` shows in the field, so the default
        // shown and the default exported cannot disagree.
        assert_eq!(PhotoId::export_stem("DSCF6223", 2, None), "DSCF6223_2");
        assert_eq!(
            PhotoId::export_stem("DSCF6223", 0, Some("ignored")),
            "DSCF6223",
            "copy 0 is the file, so a stored name is not its name"
        );
    }

    #[test]
    fn an_extension_on_the_name_is_dropped() {
        // ⚠ The field shows a whole filename, so the name usually carries one — and the *export
        // format* decides the extension. Without this, a JPEG export of "Warm.RAF" would be
        // `Warm.RAF.jpg`.
        assert_eq!(
            PhotoId::export_stem("a", 1, Some("DSCF6223_1.RAF")),
            "DSCF6223_1"
        );
        assert_eq!(PhotoId::export_stem("a", 1, Some("Warm.jpg")), "Warm");
    }

    #[test]
    fn an_export_name_cannot_escape_its_directory() {
        // ⚠ The one security-relevant line in this feature: the name is free text that becomes a
        // path. Same guard, and the same reason, as a preset's filename.
        let got = PhotoId::export_stem("a", 2, Some("../../evil"));
        assert!(!got.contains('/') && !got.contains('.'), "got {got}");
        assert!(!PhotoId::export_stem("a", 2, Some("a/b")).contains('/'));
    }

    #[test]
    fn a_name_that_sanitizes_to_nothing_falls_back_to_the_number() {
        // ⚠ Otherwise the export would have an empty filename.
        assert_eq!(PhotoId::export_stem("a", 2, Some("   ")), "a_2");
        assert_eq!(PhotoId::export_stem("a", 2, Some("")), "a_2");
        assert_eq!(PhotoId::export_stem("a", 2, Some(".RAF")), "a_2");
    }
}
