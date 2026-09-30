//! The catalog database.
//!
//! ⚠⚠ **Edits do not live here.** They stay in the per-photo JSON sidecars, so they travel with
//! the photograph and survive this application entirely. What lives here is *library* metadata —
//! which folders you have opened, which photos they hold, and collections. The property that
//! split preserves: **delete the catalog and you lose organisation, never work.**
//!
//! ⚠ One catalog, in the config directory, never named or chosen by the user. A catalog per
//! folder would make "show me every 5-star frame this year" impossible across three shoots, which
//! is the single thing a DAM exists for.

use crate::CatalogEntry;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// The schema this build expects. Bump it whenever [`Catalog::migrate`] gains a step.
pub const SCHEMA_VERSION: i64 = 9;

/// What can go wrong talking to the catalog.
///
/// ⚠ A catalog failure must never take the app down: losing the catalog loses organisation, not
/// work, so every caller in `app` degrades to "no catalog" rather than propagating.
#[derive(Debug)]
pub enum CatalogError {
    Sql(rusqlite::Error),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::Sql(e) => write!(f, "catalog: {e}"),
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<rusqlite::Error> for CatalogError {
    fn from(e: rusqlite::Error) -> Self {
        CatalogError::Sql(e)
    }
}

/// Capture metadata and descriptions.
mod capture;
/// Collections.
mod collections;
/// The capture-date tree and the distinct cameras and lenses.
mod dates;
/// A photo's undo history (M9.7) — its own file, because this one is already the largest in the
/// crate. A child module, so it reaches the private connection.
mod history;
/// What an import compares a card against (M9.12).
mod imported;
/// Renaming, re-parenting and merging keywords (M9.8) — its own file for `history`'s reason.
mod keywords;
/// Flags, ratings, colour labels, and the XMP stamp.
mod marks;
/// Recognising a photo moved on disk between known folders (M9.9).
mod moves;
/// Folders and their photos: sync, rename, copies, listings.
mod photos;
/// Forgetting a folder, and restoring what a deletion took.
mod restore;
/// The schema's migrations, version 1 to [`SCHEMA_VERSION`].
mod schema;
/// Smart collections.
mod smart;
#[cfg(test)]
mod test_support;
pub use capture::Description;
pub use collections::CollectionRow;
pub use dates::DayRow;
pub use imported::Fingerprint;
pub use keywords::{Change as KeywordChange, KeywordMerge, KeywordRow};
pub use marks::{Flag, Marks};
pub use photos::{FolderRow, PhotoRow};
pub use smart::SmartRow;

pub struct Catalog {
    conn: Connection,
}

impl Catalog {
    /// Open (creating if absent) the catalog at `path`, migrating it to [`SCHEMA_VERSION`].
    pub fn open(path: &Path) -> Result<Self, CatalogError> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self::prepare(Connection::open(path)?)
    }

    /// A catalog that exists only for the duration of the call — the tests' workhorse.
    pub fn open_in_memory() -> Result<Self, CatalogError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    /// Where the one catalog lives.
    ///
    /// ⚠ Through `dirs`, like `SettingsStore::user_default`, so it lands in the platform's own
    /// convention — `~/Library/Application Support/rustroom/` on macOS, **not** `~/.config`.
    pub fn user_default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustroom")
            .join("catalog.db")
    }

    fn prepare(conn: Connection) -> Result<Self, CatalogError> {
        // ⚠ Foreign keys are OFF by default in SQLite and must be enabled per connection. Without
        // this, deleting a folder would leave orphaned photo rows and orphaned collection
        // memberships pointing at nothing.
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let cat = Self { conn };
        cat.migrate()?;
        Ok(cat)
    }

    pub fn version(&self) -> Result<i64, CatalogError> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A photo path as `(folder, file name)`, the pair the schema keys on.
fn split(photo: &Path) -> Option<(String, String)> {
    let folder = photo.parent()?.to_string_lossy().to_string();
    let name = photo.file_name()?.to_string_lossy().to_string();
    Some((folder, name))
}

/// Size and mtime, for a future re-link of a moved file.
///
/// ⚠ Nothing reads these yet, and that is deliberate: they cost nothing to record during a scan
/// the caller is already doing, and the alternative is discovering later that the data was never
/// captured for any photo anyone owns.
pub(super) fn file_facts(f: &CatalogEntry) -> (Option<i64>, Option<i64>) {
    let mtime = f
        .modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64);
    let size = std::fs::metadata(&f.path).ok().map(|m| m.len() as i64);
    (size, mtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::TempDb;

    #[test]
    fn an_in_memory_catalog_opens_and_reports_its_version() {
        let cat = Catalog::open_in_memory().expect("should open");
        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn opening_an_existing_catalog_does_not_migrate_it_again() {
        // ⚠⚠ The guard against the most destructive shape a migration bug can take: a `CREATE
        // TABLE` that runs a second time either errors or, written with `IF NOT EXISTS`, quietly
        // masks a version mismatch. Opening twice must be a no-op the second time.
        let db = TempDb::new("twice");
        {
            let cat = Catalog::open(&db.0).unwrap();
            assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        }
        let again = Catalog::open(&db.0).expect("reopening must succeed");
        assert_eq!(again.version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn a_catalog_from_the_future_is_left_alone() {
        // ⚠ A newer Rustroom may have written a schema this build does not know. Downgrading it
        // would destroy whatever that version added.
        let db = TempDb::new("future");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        }
        let cat = Catalog::open(&db.0).expect("a future catalog must still open");
        assert_eq!(cat.version().unwrap(), 99, "it must not be downgraded");
    }

    #[test]
    fn the_four_tables_exist_after_a_migration() {
        let cat = Catalog::open_in_memory().unwrap();
        for table in ["folders", "photos", "collections", "collection_photos"] {
            let n: i64 = cat
                .conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {table} is missing");
        }
    }

    #[test]
    fn foreign_keys_are_enforced() {
        // ⚠ SQLite has them OFF by default, per connection. Without the pragma, deleting a folder
        // would leave photo rows pointing at nothing — and collection memberships pointing at
        // those.
        let cat = Catalog::open_in_memory().unwrap();
        let on: i64 = cat
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(on, 1);
    }
}
