//! The `<file>.json` edit sidecar.
//!
//! Extracted from `main.rs` so `sync` can reach it and so it is testable at all.

use edit_model::{EditLoadError, Edits};
use std::path::{Path, PathBuf};

/// The sidecar path for a source file: the full filename with `.json` **appended**
/// (`photo.RAF` → `photo.RAF.json`), not the extension replaced — so a RAW and a JPEG
/// sharing a basename get distinct sidecars.
pub fn path(photo: &Path) -> PathBuf {
    let mut s = photo.as_os_str().to_os_string();
    s.push(".json");
    PathBuf::from(s)
}

/// A photo's stored edits. `None` when there is no sidecar **or it is malformed** — a
/// hand-edited file with a typo should read as "no edits", not take anything down.
pub fn load(photo: &Path) -> Option<Edits> {
    Edits::from_json_file(&path(photo)).ok()
}

pub fn save(photo: &Path, edits: &Edits) -> Result<(), EditLoadError> {
    edits.to_json_file(&path(photo))
}

/// Delete a photo's sidecar. Absent is not an error.
pub fn remove(photo: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path(photo)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp directory for one test, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rustroom_sidecar_{tag}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn photo(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_path_appends_json_to_the_whole_filename() {
        // Not "replace the extension": a RAW and a JPEG sharing a basename must get
        // distinct sidecars.
        assert_eq!(
            path(Path::new("/a/photo.RAF")),
            PathBuf::from("/a/photo.RAF.json")
        );
        assert_eq!(
            path(Path::new("/a/photo.jpg")),
            PathBuf::from("/a/photo.jpg.json")
        );
        assert_ne!(path(Path::new("/a/p.RAF")), path(Path::new("/a/p.jpg")));
    }

    #[test]
    fn save_then_load_round_trips() {
        let d = TempDir::new("roundtrip");
        let photo = d.photo("x.RAF");
        let e = Edits {
            exposure: 1.5,
            ..Edits::default()
        };
        save(&photo, &e).unwrap();
        assert_eq!(load(&photo), Some(e));
    }

    #[test]
    fn an_absent_or_malformed_sidecar_loads_as_none() {
        // The same forgiving posture the preset and settings stores take.
        let d = TempDir::new("absent");
        let photo = d.photo("x.RAF");
        assert_eq!(load(&photo), None);
        std::fs::write(path(&photo), "{ not json").unwrap();
        assert_eq!(load(&photo), None);
    }

    #[test]
    fn removing_an_absent_sidecar_is_not_an_error() {
        let d = TempDir::new("remove");
        let photo = d.photo("x.RAF");
        remove(&photo).unwrap();
        save(&photo, &Edits::default()).unwrap();
        assert!(path(&photo).exists());
        remove(&photo).unwrap();
        assert!(!path(&photo).exists());
    }
}
