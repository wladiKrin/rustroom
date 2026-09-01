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
}
