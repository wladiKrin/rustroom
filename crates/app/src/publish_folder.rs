//! A folder service's own files (M10.18d): what a publish recorded of each, and clearing a Removed member's.
//! No egui.
//!
//! ⚠⚠ Only a file the service wrote is ever deleted, and only while it is still what was written: its size
//! AND modification time must match the record. A file edited or replaced since is left — and said.

use catalog::db::Place;
use std::path::Path;

/// The record of a file just written.
pub fn place_of(file: &Path) -> std::io::Result<Place> {
    let m = std::fs::metadata(file)?;
    Ok(Place::File {
        path: file.to_path_buf(),
        size: m.len(),
        mtime: mtime(&m),
    })
}

fn mtime(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// What clearing one member's file did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleared {
    Deleted,
    /// Changed since it was published, so left where it is.
    Changed,
    /// Already gone (or not a file the service wrote — an asset).
    Gone,
}

/// Delete the file `place` records, if it is still what was written.
pub fn clear(place: &Place) -> Cleared {
    let Place::File {
        path,
        size,
        mtime: t,
    } = place
    else {
        return Cleared::Gone;
    };
    let Ok(m) = std::fs::metadata(path) else {
        return Cleared::Gone;
    };
    if !m.is_file() || m.len() != *size || mtime(&m) != *t {
        return Cleared::Changed;
    }
    match std::fs::remove_file(path) {
        Ok(()) => Cleared::Deleted,
        Err(_) => Cleared::Changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_file_is_deleted() {
        let d = test_dir::TempDir::new("publish-clear");
        let f = d.join("a.jpg");
        std::fs::write(&f, b"jpeg").unwrap();
        let p = place_of(&f).unwrap();
        assert_eq!(clear(&p), Cleared::Deleted);
        assert!(!f.exists());
    }

    /// Review focus 1: a file edited by hand since it was published stays.
    #[test]
    fn a_changed_file_is_left() {
        let d = test_dir::TempDir::new("publish-changed");
        let f = d.join("a.jpg");
        std::fs::write(&f, b"jpeg").unwrap();
        let p = place_of(&f).unwrap();
        std::fs::write(&f, b"retouched by hand").unwrap();
        assert_eq!(clear(&p), Cleared::Changed);
        assert!(f.exists());
    }

    #[test]
    fn a_missing_file_is_gone() {
        let d = test_dir::TempDir::new("publish-gone");
        let f = d.join("a.jpg");
        std::fs::write(&f, b"jpeg").unwrap();
        let p = place_of(&f).unwrap();
        std::fs::remove_file(&f).unwrap();
        assert_eq!(clear(&p), Cleared::Gone);
        assert_eq!(clear(&Place::Asset { id: "x".into() }), Cleared::Gone);
    }

    #[test]
    fn nothing_else_in_the_folder_is_touched() {
        let d = test_dir::TempDir::new("publish-others");
        let (f, other) = (d.join("a.jpg"), d.join("b.jpg"));
        std::fs::write(&f, b"jpeg").unwrap();
        std::fs::write(&other, b"jpeg").unwrap();
        let p = place_of(&f).unwrap();
        clear(&p);
        assert!(other.exists());
    }
}
