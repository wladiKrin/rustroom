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
pub const SCHEMA_VERSION: i64 = 1;

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

    /// Bring the schema up to [`SCHEMA_VERSION`].
    ///
    /// ⚠⚠ **Forward-only, by construction: every step is gated on `from < n`.** A newer Rustroom
    /// may have written a schema this build does not know, and downgrading it would destroy
    /// whatever that version added.
    ///
    /// ⚠ All four tables are created in one step although collections are used by a later slice.
    /// A migration that has already run on someone's machine cannot be edited afterwards, so the
    /// choice is to write it once now or add a second migration later for no benefit.
    fn migrate(&self) -> Result<(), CatalogError> {
        let from: i64 = self.version()?;
        if from < 1 {
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE folders (
                     id        INTEGER PRIMARY KEY,
                     path      TEXT NOT NULL UNIQUE,
                     last_seen INTEGER NOT NULL
                 );
                 CREATE TABLE photos (
                     id        INTEGER PRIMARY KEY,
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     copy      INTEGER NOT NULL,
                     size      INTEGER,
                     mtime     INTEGER,
                     last_seen INTEGER NOT NULL,
                     UNIQUE(folder_id, file_name, copy)
                 );
                 CREATE TABLE collections (
                     id      INTEGER PRIMARY KEY,
                     name    TEXT NOT NULL UNIQUE,
                     created INTEGER NOT NULL
                 );
                 CREATE TABLE collection_photos (
                     collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
                     photo_id      INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
                     added         INTEGER NOT NULL,
                     PRIMARY KEY (collection_id, photo_id)
                 );
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        Ok(())
    }

    /// Record `folder` and every photo in `files`, adding what is new and refreshing what is not.
    ///
    /// ⚠⚠ **This never deletes a photo row.** A folder on an unplugged drive scans as empty, and
    /// removing rows then would silently empty every collection referencing them. `last_seen`
    /// records what was present; absence is not deletion.
    ///
    /// ⚠ One transaction. Half a folder recorded is worse than none, and it is also much faster:
    /// SQLite commits per statement otherwise, which is a disk sync per photo.
    pub fn sync_folder(&self, folder: &Path, files: &[CatalogEntry]) -> Result<(), CatalogError> {
        let now = unix_now();
        let folder_key = folder.to_string_lossy().to_string();
        self.conn.execute_batch("BEGIN;")?;
        let result = (|| -> Result<(), CatalogError> {
            self.conn.execute(
                "INSERT INTO folders (path, last_seen) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET last_seen = ?2",
                rusqlite::params![folder_key, now],
            )?;
            let folder_id: i64 = self.conn.query_row(
                "SELECT id FROM folders WHERE path = ?1",
                [&folder_key],
                |r| r.get(0),
            )?;
            for f in files {
                let (size, mtime) = file_facts(f);
                self.conn.execute(
                    "INSERT INTO photos (folder_id, file_name, copy, size, mtime, last_seen)
                     VALUES (?1, ?2, 0, ?3, ?4, ?5)
                     ON CONFLICT(folder_id, file_name, copy)
                     DO UPDATE SET size = ?3, mtime = ?4, last_seen = ?5",
                    rusqlite::params![folder_id, f.file_name, size, mtime, now],
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// The file names recorded for `folder`.
    pub fn photos_in(&self, folder: &Path) -> Result<Vec<String>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT p.file_name FROM photos p
             JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1",
        )?;
        let rows = stmt.query_map([&key], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    pub fn folder_count(&self) -> Result<i64, CatalogError> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM folders", [], |r| r.get(0))?)
    }

    pub fn photo_count(&self) -> Result<i64, CatalogError> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM photos", [], |r| r.get(0))?)
    }

    /// The row id for one version of one file, if the catalog knows it.
    pub fn photo_id(&self, photo: &Path, copy: u32) -> Result<Option<i64>, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT p.id FROM photos p
             JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1 AND p.file_name = ?2 AND p.copy = ?3",
        )?;
        let mut rows = stmt.query(rusqlite::params![folder, name, copy])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }

    /// Record a virtual copy of an already-known file.
    ///
    /// ⚠ Copies never come from a folder scan — they live in the sidecar — so they are added here
    /// explicitly when one is created.
    pub fn add_copy(&self, photo: &Path, copy: u32) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        let now = unix_now();
        self.conn.execute(
            "INSERT INTO photos (folder_id, file_name, copy, last_seen)
             SELECT f.id, ?2, ?3, ?4 FROM folders f WHERE f.path = ?1
             ON CONFLICT(folder_id, file_name, copy) DO UPDATE SET last_seen = ?4",
            rusqlite::params![folder, name, copy, now],
        )?;
        Ok(())
    }

    /// Follow a file rename.
    ///
    /// ⚠⚠ **Every copy moves**, because copies share the file. Moving only copy 0 would strand
    /// the rest under a name that no longer exists — and their collection memberships with them.
    ///
    /// ⚠ A photo the catalog has never seen is not an error. The catalog is not authoritative and
    /// must never be a reason a rename fails.
    ///
    /// ⚠ Restricted to renames **within one folder**, which is all `file_rename::target` can
    /// produce — it builds the new path with `with_file_name`. A cross-folder move is a different
    /// operation and is not supported here.
    pub fn rename_photo(&self, from: &Path, to: &Path) -> Result<(), CatalogError> {
        let (Some((from_folder, from_name)), Some((to_folder, to_name))) = (split(from), split(to))
        else {
            return Ok(());
        };
        if from_folder != to_folder {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE photos SET file_name = ?3
             WHERE file_name = ?2
               AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
            rusqlite::params![from_folder, from_name, to_name],
        )?;
        Ok(())
    }

    /// Follow a `Reindex` renumbering of one copy.
    pub fn renumber_copy(&self, photo: &Path, from: u32, to: u32) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        self.conn.execute(
            "UPDATE photos SET copy = ?4
             WHERE file_name = ?2 AND copy = ?3
               AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
            rusqlite::params![folder, name, from, to],
        )?;
        Ok(())
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
fn file_facts(f: &CatalogEntry) -> (Option<i64>, Option<i64>) {
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

    /// A temp path that removes itself.
    ///
    /// ⚠ Uniqueness is **structural** — process id plus a counter — never the clock. Two tests
    /// entering the same tick collide, `cargo test` runs them in parallel, and this project has
    /// paid for that twice already.
    struct TempDb(PathBuf);

    impl TempDb {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "rustroom_catalogdb_{tag}_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.join("catalog.db"))
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            if let Some(p) = self.0.parent() {
                let _ = std::fs::remove_dir_all(p);
            }
        }
    }

    fn entry(name: &str) -> CatalogEntry {
        CatalogEntry {
            path: PathBuf::from(format!("/x/{name}")),
            file_name: name.to_string(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            edited: None,
        }
    }

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

    #[test]
    fn syncing_a_folder_records_its_photos() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        let mut got = cat.photos_in(Path::new("/x")).unwrap();
        got.sort();
        assert_eq!(got, vec!["a.RAF".to_string(), "b.RAF".to_string()]);
    }

    #[test]
    fn syncing_the_same_folder_twice_does_not_duplicate_it() {
        // ⚠ Opening a folder is the *normal* case, not an exceptional one — someone browsing
        // their library re-opens the same folder daily. Duplicated rows would multiply forever
        // and every collection query would return the same photo many times.
        let cat = Catalog::open_in_memory().unwrap();
        let files = [entry("a.RAF")];
        cat.sync_folder(Path::new("/x"), &files).unwrap();
        cat.sync_folder(Path::new("/x"), &files).unwrap();
        assert_eq!(cat.photos_in(Path::new("/x")).unwrap().len(), 1);
        assert_eq!(cat.folder_count().unwrap(), 1);
    }

    #[test]
    fn two_folders_with_a_same_named_file_are_two_photos() {
        // ⚠ `DSCF0001.RAF` exists in every folder a Fuji has ever filled. Keying on file name
        // alone would merge them; the UNIQUE constraint is on (folder, name, copy).
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("DSCF0001.RAF")])
            .unwrap();
        cat.sync_folder(Path::new("/y"), &[entry("DSCF0001.RAF")])
            .unwrap();
        assert_eq!(cat.photo_count().unwrap(), 2);
    }

    #[test]
    fn a_folder_whose_files_have_vanished_keeps_its_rows() {
        // ⚠⚠ **The most important test in this plan.** An unplugged drive or an unmounted share
        // scans as empty. Deleting rows then would silently empty every collection referencing
        // them — and collections have no UI yet, so nothing would catch it. A scan adds and
        // updates; it never removes.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        cat.sync_folder(Path::new("/x"), &[]).unwrap();
        assert_eq!(
            cat.photos_in(Path::new("/x")).unwrap().len(),
            2,
            "an empty scan must not delete anything"
        );
    }

    #[test]
    fn renaming_a_file_moves_its_row_rather_than_adding_one() {
        // ⚠⚠ Rustroom renames files itself. If the catalog is not told, the row keeps the old
        // name, the next scan adds a *second* row under the new one, and any collection
        // membership stays attached to a photo that no longer exists.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let id = cat.photo_id(Path::new("/x/a.RAF"), 0).unwrap().unwrap();

        cat.rename_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();

        assert_eq!(cat.photo_count().unwrap(), 1, "no second row");
        assert_eq!(
            cat.photo_id(Path::new("/x/b.RAF"), 0).unwrap(),
            Some(id),
            "the same row, under the new name"
        );
        assert_eq!(cat.photo_id(Path::new("/x/a.RAF"), 0).unwrap(), None);
    }

    #[test]
    fn renaming_moves_every_copy_of_the_file() {
        // ⚠ Copies share the file, so a rename moves all of them. Moving only copy 0 would
        // strand the rest under a name that no longer exists.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 2).unwrap();

        cat.rename_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();

        for copy in [0, 1, 2] {
            assert!(
                cat.photo_id(Path::new("/x/b.RAF"), copy).unwrap().is_some(),
                "copy {copy} did not follow the rename"
            );
        }
        assert_eq!(cat.photo_count().unwrap(), 3);
    }

    #[test]
    fn renumbering_a_copy_moves_its_row() {
        // ⚠ `Reindex` closes gaps in copy numbering. The catalog must follow, or the row for the
        // old number becomes unreachable while a new one appears empty.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 3).unwrap();
        let id = cat.photo_id(Path::new("/x/a.RAF"), 3).unwrap().unwrap();

        cat.renumber_copy(Path::new("/x/a.RAF"), 3, 1).unwrap();

        assert_eq!(cat.photo_id(Path::new("/x/a.RAF"), 1).unwrap(), Some(id));
        assert_eq!(cat.photo_id(Path::new("/x/a.RAF"), 3).unwrap(), None);
        assert_eq!(cat.photo_count().unwrap(), 2);
    }

    #[test]
    fn renaming_a_photo_the_catalog_has_never_seen_is_harmless() {
        // ⚠ The catalog is not authoritative and must never be a reason a rename fails. A photo
        // in a folder that was never opened simply has no row to move.
        let cat = Catalog::open_in_memory().unwrap();
        cat.rename_photo(Path::new("/x/ghost.RAF"), Path::new("/x/other.RAF"))
            .expect("must not error");
        assert_eq!(cat.photo_count().unwrap(), 0);
    }
}
