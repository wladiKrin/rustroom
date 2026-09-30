//! Fixtures the catalog tests share across the `db` modules.

use crate::CatalogEntry;
use std::path::PathBuf;

/// A temp path that removes itself.
///
/// ⚠ Uniqueness is **structural** — process id plus a counter — never the clock. Two tests
/// entering the same tick collide, `cargo test` runs them in parallel, and this project has
/// paid for that twice already.
pub(super) struct TempDb(pub(super) PathBuf);

impl TempDb {
    pub(super) fn new(tag: &str) -> Self {
        Self(test_dir::path(&format!("catalogdb-{tag}")).join("catalog.db"))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        if let Some(p) = self.0.parent() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

/// Entries for `folder`, one per name.
///
/// ⚠ The `entry` helper below hardcodes `/x/`, which cannot express the two-folder case every
/// whole-library query needs — and four existing tests depend on that path, so this is a
/// sibling rather than a change to it.
pub(super) fn entries_in(folder: &str, names: &[&str]) -> Vec<CatalogEntry> {
    names
        .iter()
        .map(|n| CatalogEntry {
            path: PathBuf::from(folder).join(n),
            file_name: n.to_string(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            edited: None,
        })
        .collect()
}

pub(super) fn entry(name: &str) -> CatalogEntry {
    CatalogEntry {
        path: PathBuf::from(format!("/x/{name}")),
        file_name: name.to_string(),
        modified: std::time::SystemTime::UNIX_EPOCH,
        edited: None,
    }
}
