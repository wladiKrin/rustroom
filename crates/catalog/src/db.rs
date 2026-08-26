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
pub const SCHEMA_VERSION: i64 = 3;

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

/// Pick / reject, or neither.
///
/// ⚠ Stored as `1 / -1 / 0` rather than as text: the neutral value is then `0`, the same as an
/// unrated star and an unset label, so "no mark" needs no special case anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Flag {
    #[default]
    None,
    Pick,
    Reject,
}

impl Flag {
    fn to_i64(self) -> i64 {
        match self {
            Flag::None => 0,
            Flag::Pick => 1,
            Flag::Reject => -1,
        }
    }

    fn from_i64(v: i64) -> Self {
        match v {
            1 => Flag::Pick,
            -1 => Flag::Reject,
            // ⚠ Anything else reads as unflagged rather than erroring. The column is an integer a
            // future version could widen, and a catalog is never a reason to fail.
            _ => Flag::None,
        }
    }
}

/// Everything a photo is marked with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Marks {
    /// 0..=5, where 0 is unrated.
    pub rating: u8,
    pub flag: Flag,
    /// 0 is no label; 1..=5 are the colours.
    pub label: u8,
}

impl Marks {
    pub const MAX_RATING: u8 = 5;
    /// ⚠ Four, not five: keys `6`–`9` are what set a label, so a fifth colour would be
    /// unreachable — and an unreachable value is a bug nobody can see.
    pub const MAX_LABEL: u8 = 4;

    /// Whether anything is set.
    pub fn is_empty(self) -> bool {
        self == Marks::default()
    }

    /// ⚠ Clamped on the way in, not trusted. The value arrives from a key handler and one day
    /// from an import; storing 9 stars would render as garbage in every badge assuming 0..=5.
    fn clamped(self) -> Self {
        Self {
            rating: self.rating.min(Self::MAX_RATING),
            flag: self.flag,
            label: self.label.min(Self::MAX_LABEL),
        }
    }
}

/// A keyword in the tree, flattened for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeywordRow {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    /// How deep in the tree, for indenting. Roots are 0.
    pub depth: usize,
    /// How many *files* carry this keyword directly — **not** counting children.
    pub count: i64,
}

/// A collection, with how many photos are in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionRow {
    pub id: i64,
    pub name: String,
    pub count: i64,
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
        if from < 2 {
            // ⚠⚠ A **new gate**, never an edit to version 1. A migration that has already run on
            // someone's machine cannot be changed, and this is the first one to meet a catalog
            // holding real folders, photos and collections.
            //
            // ⚠ `NOT NULL DEFAULT 0` means existing rows acquire "unmarked" with no backfill, and
            // 0 is the neutral value for all three — so "no mark" needs no special case anywhere.
            self.conn.execute_batch(
                "BEGIN;
                 ALTER TABLE photos ADD COLUMN rating INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE photos ADD COLUMN flag   INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE photos ADD COLUMN label  INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version = 2;
                 COMMIT;",
            )?;
        }
        if from < 3 {
            // ⚠⚠ A **new gate**, never an edit to versions 1 or 2. Second time this rule applies
            // to a database holding real user data.
            //
            // ⚠⚠ The `COALESCE` index is load-bearing. A plain `UNIQUE(parent_id, name)` would
            // allow **two root-level "Paris" keywords**, because SQLite treats NULLs as distinct
            // in a unique constraint — silent duplicates that look identical in the tree.
            //
            // ⚠⚠ `photo_keywords` is keyed on the **file**, not on `photos.id` as
            // `collection_photos` is. That is not an inconsistency: a keyword describes what is
            // *in* the picture, which every copy of a file shares, while a collection membership
            // and a rating describe a particular version. **Do not unify them.**
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE keywords (
                     id        INTEGER PRIMARY KEY,
                     name      TEXT NOT NULL,
                     parent_id INTEGER REFERENCES keywords(id) ON DELETE CASCADE
                 );
                 CREATE UNIQUE INDEX keywords_unique
                     ON keywords(COALESCE(parent_id, 0), name);
                 CREATE TABLE photo_keywords (
                     folder_id  INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name  TEXT NOT NULL,
                     keyword_id INTEGER NOT NULL REFERENCES keywords(id) ON DELETE CASCADE,
                     PRIMARY KEY (folder_id, file_name, keyword_id)
                 );
                 PRAGMA user_version = 3;
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
            rusqlite::params![&from_folder, &from_name, &to_name],
        )?;
        // ⚠⚠ `photo_keywords` is keyed by file name, so it must move in the same operation — the
        // third instance of this integration, after `photos` itself and collection membership via
        // row id. A rename that forgot would silently strip a file's keywords.
        self.conn.execute(
            "UPDATE photo_keywords SET file_name = ?3
             WHERE file_name = ?2
               AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
            rusqlite::params![&from_folder, &from_name, &to_name],
        )?;
        Ok(())
    }

    /// What a photo is marked with. Unknown photos read as unmarked.
    pub fn marks(&self, photo: &Path, copy: u32) -> Result<Marks, CatalogError> {
        let Some(id) = self.photo_id(photo, copy)? else {
            return Ok(Marks::default());
        };
        let (rating, flag, label): (i64, i64, i64) = self.conn.query_row(
            "SELECT rating, flag, label FROM photos WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(Marks {
            rating: rating.clamp(0, Marks::MAX_RATING as i64) as u8,
            flag: Flag::from_i64(flag),
            label: label.clamp(0, Marks::MAX_LABEL as i64) as u8,
        })
    }

    /// Mark a photo. ⚠ A photo the catalog does not know is silently skipped — the catalog is
    /// never a reason an action fails.
    pub fn set_marks(&self, photo: &Path, copy: u32, marks: Marks) -> Result<(), CatalogError> {
        let Some(id) = self.photo_id(photo, copy)? else {
            return Ok(());
        };
        let m = marks.clamped();
        self.conn.execute(
            "UPDATE photos SET rating = ?2, flag = ?3, label = ?4 WHERE id = ?1",
            rusqlite::params![id, m.rating as i64, m.flag.to_i64(), m.label as i64],
        )?;
        Ok(())
    }

    /// Every *marked* photo in a folder, as `(path, copy, marks)`.
    ///
    /// ⚠⚠ One query per rebuild, never one per grid cell: the grid draws sixty times a second
    /// over hundreds of thumbnails, and a query per cell would be a database round trip per photo
    /// per frame. Only marked rows come back, so an unmarked folder costs almost nothing.
    pub fn marks_in(&self, folder: &Path) -> Result<Vec<(PathBuf, u32, Marks)>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT p.file_name, p.copy, p.rating, p.flag, p.label
             FROM photos p JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1 AND (p.rating != 0 OR p.flag != 0 OR p.label != 0)",
        )?;
        let rows = stmt.query_map([&key], |r| {
            let name: String = r.get(0)?;
            let copy: u32 = r.get(1)?;
            let rating: i64 = r.get(2)?;
            let flag: i64 = r.get(3)?;
            let label: i64 = r.get(4)?;
            Ok((
                PathBuf::from(&key).join(name),
                copy,
                Marks {
                    rating: rating.clamp(0, Marks::MAX_RATING as i64) as u8,
                    flag: Flag::from_i64(flag),
                    label: label.clamp(0, Marks::MAX_LABEL as i64) as u8,
                },
            ))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Create a keyword under `parent`, or return the existing one of that name and parent.
    ///
    /// ⚠ A duplicate is not an error, for the reason `create_collection` gives: the name is the
    /// identity a user thinks in.
    pub fn create_keyword(&self, name: &str, parent: Option<i64>) -> Result<i64, CatalogError> {
        self.conn.execute(
            "INSERT INTO keywords (name, parent_id) VALUES (?1, ?2)
             ON CONFLICT DO NOTHING",
            rusqlite::params![name, parent],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM keywords WHERE name = ?1 AND COALESCE(parent_id, 0) = COALESCE(?2, 0)",
            rusqlite::params![name, parent],
            |r| r.get(0),
        )?)
    }

    /// The whole tree, depth-first and name-ordered within each level.
    ///
    /// ⚠ Ordered in SQL so the tree does not reshuffle between frames.
    pub fn keywords(&self) -> Result<Vec<KeywordRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE tree(id, name, parent_id, depth, path) AS (
                 SELECT id, name, parent_id, 0, lower(name)
                 FROM keywords WHERE parent_id IS NULL
                 UNION ALL
                 SELECT k.id, k.name, k.parent_id, t.depth + 1, t.path || '/' || lower(k.name)
                 FROM keywords k JOIN tree t ON k.parent_id = t.id
             )
             SELECT t.id, t.name, t.parent_id, t.depth,
                    (SELECT count(*) FROM photo_keywords pk WHERE pk.keyword_id = t.id)
             FROM tree t ORDER BY t.path",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(KeywordRow {
                id: r.get(0)?,
                name: r.get(1)?,
                parent_id: r.get(2)?,
                depth: r.get::<_, i64>(3)? as usize,
                count: r.get(4)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Remove a keyword. ⚠ Its **children go with it**, and every assignment of any of them.
    pub fn delete_keyword(&self, id: i64) -> Result<(), CatalogError> {
        self.conn
            .execute("DELETE FROM keywords WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Tag a **file** — every copy of it, now and in future.
    ///
    /// ⚠ Keyed on `(folder, file_name)` rather than on a photo row, which is what makes that true
    /// with no inheritance rule to maintain.
    pub fn assign_keyword(&self, keyword: i64, photo: &Path) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO photo_keywords (folder_id, file_name, keyword_id)
             SELECT f.id, ?2, ?3 FROM folders f WHERE f.path = ?1
             ON CONFLICT DO NOTHING",
            rusqlite::params![folder, name, keyword],
        )?;
        Ok(())
    }

    pub fn unassign_keyword(&self, keyword: i64, photo: &Path) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        self.conn.execute(
            "DELETE FROM photo_keywords
             WHERE keyword_id = ?3 AND file_name = ?2
               AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
            rusqlite::params![folder, name, keyword],
        )?;
        Ok(())
    }

    /// Every photo carrying `keyword` **or any of its descendants**, as `(path, copy)`.
    ///
    /// ⚠⚠ The subtree is resolved by a recursive CTE at query time, never stored. Storing the
    /// implication would mean rewriting every affected row whenever a keyword is re-parented —
    /// two facts that can disagree, which this project has been bitten by twice.
    ///
    /// ⚠ Joins back to `photos` on `(folder, file_name)`, so **every copy of a tagged file**
    /// comes back. That is the point of keying on the file.
    pub fn keyword_members(&self, keyword: i64) -> Result<Vec<(PathBuf, u32)>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE sub(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT k.id FROM keywords k JOIN sub ON k.parent_id = sub.id
             )
             SELECT DISTINCT f.path, p.file_name, p.copy
             FROM photo_keywords pk
             JOIN sub       ON sub.id = pk.keyword_id
             JOIN folders f ON f.id = pk.folder_id
             JOIN photos  p ON p.folder_id = pk.folder_id AND p.file_name = pk.file_name
             ORDER BY lower(f.path), lower(p.file_name), p.copy",
        )?;
        let rows = stmt.query_map([keyword], |r| {
            let folder: String = r.get(0)?;
            let name: String = r.get(1)?;
            let copy: u32 = r.get(2)?;
            Ok((PathBuf::from(folder).join(name), copy))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Create `name`, or return the existing collection of that name.
    ///
    /// ⚠ A duplicate is **not** an error. The name is the identity a user thinks in, and failing
    /// the second time "Portfolio" is used would be a bug rather than a safeguard.
    pub fn create_collection(&self, name: &str) -> Result<i64, CatalogError> {
        self.conn.execute(
            "INSERT INTO collections (name, created) VALUES (?1, ?2)
             ON CONFLICT(name) DO NOTHING",
            rusqlite::params![name, unix_now()],
        )?;
        Ok(self
            .conn
            .query_row("SELECT id FROM collections WHERE name = ?1", [name], |r| {
                r.get(0)
            })?)
    }

    /// Every collection, name-ordered, with its size.
    ///
    /// ⚠ Ordered in SQL and **case-insensitively**, matching the browser's own rule. A list that
    /// reshuffles between frames is the sparse-index class of bug this project has hit three
    /// times in the Library.
    pub fn collections(&self) -> Result<Vec<CollectionRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.name, (SELECT count(*) FROM collection_photos cp
                                   WHERE cp.collection_id = c.id)
             FROM collections c
             ORDER BY lower(c.name), c.name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CollectionRow {
                id: r.get(0)?,
                name: r.get(1)?,
                count: r.get(2)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Remove a collection. ⚠ Its memberships go with it; **the photos do not.**
    pub fn delete_collection(&self, id: i64) -> Result<(), CatalogError> {
        self.conn
            .execute("DELETE FROM collections WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Put one version of one file into a collection.
    ///
    /// ⚠ Idempotent, because adding twice is the normal case — a user re-selects and re-adds.
    ///
    /// ⚠ A photo the catalog does not know is silently skipped: the catalog is not authoritative
    /// and must never be a reason an action fails.
    pub fn add_to_collection(
        &self,
        collection: i64,
        photo: &Path,
        copy: u32,
    ) -> Result<(), CatalogError> {
        let Some(photo_id) = self.photo_id(photo, copy)? else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO collection_photos (collection_id, photo_id, added) VALUES (?1, ?2, ?3)
             ON CONFLICT(collection_id, photo_id) DO NOTHING",
            rusqlite::params![collection, photo_id, unix_now()],
        )?;
        Ok(())
    }

    pub fn remove_from_collection(
        &self,
        collection: i64,
        photo: &Path,
        copy: u32,
    ) -> Result<(), CatalogError> {
        let Some(photo_id) = self.photo_id(photo, copy)? else {
            return Ok(());
        };
        self.conn.execute(
            "DELETE FROM collection_photos WHERE collection_id = ?1 AND photo_id = ?2",
            rusqlite::params![collection, photo_id],
        )?;
        Ok(())
    }

    /// Everything in a collection, as `(path, copy)` — **across every folder**.
    ///
    /// ⚠⚠ Cross-folder is the whole reason the catalog is one database rather than one per
    /// folder. If this could not span folders, that decision was wrong.
    pub fn collection_members(&self, collection: i64) -> Result<Vec<(PathBuf, u32)>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, p.file_name, p.copy
             FROM collection_photos cp
             JOIN photos p  ON p.id = cp.photo_id
             JOIN folders f ON f.id = p.folder_id
             WHERE cp.collection_id = ?1
             ORDER BY lower(f.path), lower(p.file_name), p.copy",
        )?;
        let rows = stmt.query_map([collection], |r| {
            let folder: String = r.get(0)?;
            let name: String = r.get(1)?;
            let copy: u32 = r.get(2)?;
            Ok((PathBuf::from(folder).join(name), copy))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
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

    #[test]
    fn a_collection_can_be_created_and_listed() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_collection("Portfolio").unwrap();
        let got = cat.collections().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, id);
        assert_eq!(got[0].name, "Portfolio");
        assert_eq!(got[0].count, 0, "a new collection is empty");
    }

    #[test]
    fn collections_come_back_in_name_order() {
        // ⚠ Ordered in SQL, not by the caller. A list that reshuffles between frames is the
        // sparse-index class of bug this project has hit three times in the Library.
        let cat = Catalog::open_in_memory().unwrap();
        for n in ["Zebra", "apple", "Middle"] {
            cat.create_collection(n).unwrap();
        }
        let got: Vec<String> = cat
            .collections()
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            got,
            ["apple", "Middle", "Zebra"],
            "case-insensitive, like the browser"
        );
    }

    #[test]
    fn creating_a_collection_that_already_exists_returns_the_same_one() {
        // ⚠ Not an error: the name is the identity a user thinks in, and failing on a duplicate
        // would make "add to Portfolio" fail the second time it is used.
        let cat = Catalog::open_in_memory().unwrap();
        let a = cat.create_collection("Portfolio").unwrap();
        let b = cat.create_collection("Portfolio").unwrap();
        assert_eq!(a, b);
        assert_eq!(cat.collections().unwrap().len(), 1);
    }

    #[test]
    fn adding_a_photo_to_a_collection_counts_it_once() {
        // ⚠ Adding twice is the normal case — a user re-selects and re-adds. The PRIMARY KEY on
        // (collection, photo) is what makes it idempotent, asserted through the API rather than
        // merely declared in the schema.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let c = cat.create_collection("Portfolio").unwrap();

        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        assert_eq!(cat.collections().unwrap()[0].count, 1);
    }

    #[test]
    fn a_collection_holds_photos_from_several_folders() {
        // ⚠⚠ The whole reason the catalog is one database rather than one per folder. If this
        // could not work, the design decision that chose a single catalog was wrong.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.sync_folder(Path::new("/y"), &[entry("b.RAF")]).unwrap();
        let c = cat.create_collection("Best").unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
        cat.add_to_collection(c, Path::new("/y/b.RAF"), 0).unwrap();

        let mut members = cat.collection_members(c).unwrap();
        members.sort();
        assert_eq!(
            members,
            vec![
                (PathBuf::from("/x/a.RAF"), 0),
                (PathBuf::from("/y/b.RAF"), 0)
            ]
        );
    }

    #[test]
    fn removing_a_photo_leaves_the_collection_and_the_photo() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        cat.remove_from_collection(c, Path::new("/x/a.RAF"), 0)
            .unwrap();

        assert_eq!(
            cat.collections().unwrap().len(),
            1,
            "the collection survives"
        );
        assert_eq!(cat.collections().unwrap()[0].count, 0);
        assert_eq!(cat.photo_count().unwrap(), 1, "and so does the photo");
    }

    #[test]
    fn deleting_a_collection_never_touches_the_photos() {
        // ⚠⚠ The guard people actually worry about. `ON DELETE CASCADE` removes the *memberships*
        // and must stop there — a collection is a view onto photos, not a container of them.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        cat.delete_collection(c).unwrap();

        assert!(cat.collections().unwrap().is_empty());
        assert_eq!(cat.photo_count().unwrap(), 2, "the photos are untouched");
    }

    #[test]
    fn a_renamed_file_stays_in_its_collections() {
        // ⚠⚠ The integration plan 1 built `rename_photo` for, asserted end to end. Membership is
        // by row id, so moving the row carries the membership — which is the reason a rename must
        // update the catalog in the same operation as the file move.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        cat.rename_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();

        assert_eq!(
            cat.collection_members(c).unwrap(),
            vec![(PathBuf::from("/x/b.RAF"), 0)],
            "the membership must follow the rename"
        );
    }

    #[test]
    fn adding_a_photo_the_catalog_does_not_know_is_harmless() {
        // ⚠ The catalog is not authoritative, so nothing may fail because of it.
        let cat = Catalog::open_in_memory().unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(c, Path::new("/ghost/x.RAF"), 0)
            .expect("must not error");
        assert_eq!(cat.collections().unwrap()[0].count, 0);
    }

    #[test]
    fn a_version_one_catalog_upgrades_and_keeps_its_rows() {
        // ⚠⚠ **The first migration to meet real user data.** Someone has a v1 catalog with
        // folders, photos and collections in it right now. Upgrading must add the columns and
        // touch nothing else — a migration that drops and recreates would destroy a library.
        let db = TempDb::new("v1v2");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
            let c = cat.create_collection("Portfolio").unwrap();
            cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
            // Pretend this catalog was written by the build that shipped v1.
            //
            // ⚠⚠ **Every new migration makes this simulation incomplete.** It must undo
            // *everything* later versions added, or `migrate` re-runs a step whose objects still
            // exist and fails with "table already exists". Adding v3's tables to the drop list
            // was exactly that — the failure is loud, but only if this test exists.
            cat.conn
                .execute_batch(
                    "DROP TABLE photo_keywords;
                     DROP TABLE keywords;
                     ALTER TABLE photos DROP COLUMN rating;
                     ALTER TABLE photos DROP COLUMN flag;
                     ALTER TABLE photos DROP COLUMN label;
                     PRAGMA user_version = 1;",
                )
                .unwrap();
        }

        let cat = Catalog::open(&db.0).expect("a v1 catalog must upgrade");

        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(cat.photo_count().unwrap(), 1, "the photo survived");
        assert_eq!(
            cat.collections().unwrap()[0].count,
            1,
            "and so did its collection membership"
        );
        assert_eq!(
            cat.marks(Path::new("/x/a.RAF"), 0).unwrap(),
            Marks::default(),
            "an upgraded row is unmarked"
        );
    }

    #[test]
    fn marks_round_trip() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let want = Marks {
            rating: 4,
            flag: Flag::Pick,
            label: 2,
        };
        cat.set_marks(Path::new("/x/a.RAF"), 0, want).unwrap();
        assert_eq!(cat.marks(Path::new("/x/a.RAF"), 0).unwrap(), want);
    }

    #[test]
    fn copies_of_one_file_are_marked_separately() {
        // ⚠⚠ The decision this feature was designed around: a rating describes **the image you
        // made**, so a black-and-white copy can be rated apart from its colour original. That is
        // why copies exist. If this failed, per-copy storage was pointless.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();

        cat.set_marks(
            Path::new("/x/a.RAF"),
            0,
            Marks {
                rating: 1,
                ..Marks::default()
            },
        )
        .unwrap();
        cat.set_marks(
            Path::new("/x/a.RAF"),
            1,
            Marks {
                rating: 5,
                ..Marks::default()
            },
        )
        .unwrap();

        assert_eq!(cat.marks(Path::new("/x/a.RAF"), 0).unwrap().rating, 1);
        assert_eq!(cat.marks(Path::new("/x/a.RAF"), 1).unwrap().rating, 5);
    }

    #[test]
    fn marks_survive_a_rename_and_a_renumbering() {
        // ⚠⚠ Marks live on the row, and plan 1 made renames and `Reindex` move the row. This
        // asserts that end to end — it is what stops a cull being undone by housekeeping.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 3).unwrap();
        let want = Marks {
            rating: 5,
            flag: Flag::Pick,
            label: 1,
        };
        cat.set_marks(Path::new("/x/a.RAF"), 3, want).unwrap();

        cat.rename_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();
        cat.renumber_copy(Path::new("/x/b.RAF"), 3, 1).unwrap();

        assert_eq!(cat.marks(Path::new("/x/b.RAF"), 1).unwrap(), want);
    }

    #[test]
    fn an_out_of_range_rating_is_clamped_rather_than_stored() {
        // ⚠ The value reaches here from a key handler and a future import. Storing 9 stars would
        // render as garbage in every badge that assumes 0..=5.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.set_marks(
            Path::new("/x/a.RAF"),
            0,
            Marks {
                rating: 9,
                ..Marks::default()
            },
        )
        .unwrap();
        // ⚠⚠ Asserted on what is **stored**, not on what `marks` returns. `marks` clamps on the
        // read path too, so reading it back would pass with the write clamp deleted — sabotage
        // said so. Defence in depth is fine; a test that cannot see the layer it names is not.
        // A stored 9 would also be seen by any future `WHERE rating >= 3` query.
        let stored: i64 = cat
            .conn
            .query_row("SELECT rating FROM photos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            stored, 5,
            "the out-of-range value must not reach the column"
        );
    }

    #[test]
    fn marks_of_a_folder_come_back_in_one_query() {
        // ⚠ The grid draws sixty times a second. One query per rebuild, never one per cell.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        cat.set_marks(
            Path::new("/x/a.RAF"),
            0,
            Marks {
                rating: 3,
                ..Marks::default()
            },
        )
        .unwrap();

        let got = cat.marks_in(Path::new("/x")).unwrap();
        assert_eq!(got.len(), 1, "only marked photos are returned");
        assert_eq!(got[0].0, PathBuf::from("/x/a.RAF"));
        assert_eq!(got[0].1, 0);
        assert_eq!(got[0].2.rating, 3);
    }

    #[test]
    fn marking_a_photo_the_catalog_does_not_know_is_harmless() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.set_marks(
            Path::new("/ghost/x.RAF"),
            0,
            Marks {
                rating: 3,
                ..Marks::default()
            },
        )
        .expect("must not error");
    }

    #[test]
    fn a_version_two_catalog_upgrades_and_keeps_everything() {
        // ⚠⚠ The second migration to meet real user data — folders, photos, collections *and*
        // marks now. Upgrading must add tables and touch nothing else.
        let db = TempDb::new("v2v3");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
            let c = cat.create_collection("Portfolio").unwrap();
            cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
            cat.set_marks(
                Path::new("/x/a.RAF"),
                0,
                Marks {
                    rating: 4,
                    ..Marks::default()
                },
            )
            .unwrap();
            cat.conn
                .execute_batch(
                    "DROP TABLE photo_keywords;
                     DROP TABLE keywords;
                     PRAGMA user_version = 2;",
                )
                .unwrap();
        }

        let cat = Catalog::open(&db.0).expect("a v2 catalog must upgrade");

        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(cat.photo_count().unwrap(), 1);
        assert_eq!(cat.collections().unwrap()[0].count, 1, "collection kept");
        assert_eq!(
            cat.marks(Path::new("/x/a.RAF"), 0).unwrap().rating,
            4,
            "marks kept"
        );
        assert!(cat.keywords().unwrap().is_empty());
    }

    #[test]
    fn two_root_keywords_of_the_same_name_are_refused() {
        // ⚠⚠ **The `COALESCE` index is the only thing preventing this.** A plain
        // `UNIQUE(parent_id, name)` lets both through, because SQLite treats NULLs as *distinct*
        // in a unique constraint — you would get two identical-looking roots in the tree with no
        // way to tell them apart.
        let cat = Catalog::open_in_memory().unwrap();
        let a = cat.create_keyword("Paris", None).unwrap();
        let b = cat.create_keyword("Paris", None).unwrap();
        assert_eq!(a, b, "the second must return the existing one");
        assert_eq!(cat.keywords().unwrap().len(), 1);
    }

    #[test]
    fn the_same_name_under_different_parents_is_allowed() {
        // ⚠ The discriminating other half. "Paris" under both France and Texas is legitimate, and
        // an over-broad unique constraint would refuse it.
        let cat = Catalog::open_in_memory().unwrap();
        let fr = cat.create_keyword("France", None).unwrap();
        let tx = cat.create_keyword("Texas", None).unwrap();
        let a = cat.create_keyword("Paris", Some(fr)).unwrap();
        let b = cat.create_keyword("Paris", Some(tx)).unwrap();
        assert_ne!(a, b);
        assert_eq!(cat.keywords().unwrap().len(), 4);
    }

    #[test]
    fn a_keyword_applies_to_every_copy_of_its_file() {
        // ⚠⚠ The decision the schema shape exists for. A keyword describes what is *in* the
        // picture, which is identical across copies — so it is keyed on the file, and no
        // inheritance rule is needed because there is only one fact.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();
        let k = cat.create_keyword("Paris", None).unwrap();

        cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        let mut members = cat.keyword_members(k).unwrap();
        members.sort();
        assert_eq!(
            members,
            vec![
                (PathBuf::from("/x/a.RAF"), 0),
                (PathBuf::from("/x/a.RAF"), 1)
            ],
            "both copies must match a keyword assigned once"
        );
    }

    #[test]
    fn a_copy_created_after_tagging_still_matches() {
        // ⚠ The exact case that would need an "inherit on create" rule under per-copy storage.
        // Keyed on the file, it needs nothing at all.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let k = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();

        assert_eq!(cat.keyword_members(k).unwrap().len(), 2);
    }

    #[test]
    fn a_parent_keyword_finds_its_childrens_photos() {
        // ⚠⚠ The payoff, and the reason hierarchy was chosen over flat: searching Europe finds
        // photos tagged Paris. Derived by a recursive query, never stored.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        let france = cat.create_keyword("France", Some(europe)).unwrap();
        let paris = cat.create_keyword("Paris", Some(france)).unwrap();

        cat.assign_keyword(paris, Path::new("/x/a.RAF")).unwrap();
        cat.assign_keyword(europe, Path::new("/x/b.RAF")).unwrap();

        let mut from_parent = cat.keyword_members(europe).unwrap();
        from_parent.sort();
        assert_eq!(
            from_parent,
            vec![
                (PathBuf::from("/x/a.RAF"), 0),
                (PathBuf::from("/x/b.RAF"), 0)
            ],
            "Europe must find the photo tagged only Paris"
        );
        assert_eq!(
            cat.keyword_members(paris).unwrap().len(),
            1,
            "but Paris must not find Europe's"
        );
    }

    #[test]
    fn deleting_a_parent_removes_its_children_and_their_assignments() {
        // ⚠ Destructive and deliberate. Re-parenting orphans to the root would silently flatten a
        // taxonomy you built, which is worse than an obvious deletion.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        let paris = cat.create_keyword("Paris", Some(europe)).unwrap();
        cat.assign_keyword(paris, Path::new("/x/a.RAF")).unwrap();

        cat.delete_keyword(europe).unwrap();

        assert!(cat.keywords().unwrap().is_empty(), "both are gone");
        assert_eq!(cat.photo_count().unwrap(), 1, "the photo is untouched");
    }

    #[test]
    fn a_renamed_file_keeps_its_keywords() {
        // ⚠⚠ `photo_keywords` is keyed by **file name**, so `rename_photo` must update it in the
        // same operation — the third instance of that integration.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let k = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        cat.rename_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF"))
            .unwrap();

        assert_eq!(
            cat.keyword_members(k).unwrap(),
            vec![(PathBuf::from("/x/b.RAF"), 0)]
        );
    }

    #[test]
    fn unassigning_leaves_the_keyword_and_the_photo() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let k = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        cat.unassign_keyword(k, Path::new("/x/a.RAF")).unwrap();

        assert!(cat.keyword_members(k).unwrap().is_empty());
        assert_eq!(cat.keywords().unwrap().len(), 1, "the keyword survives");
        assert_eq!(cat.photo_count().unwrap(), 1, "and so does the photo");
    }

    #[test]
    fn keywords_come_back_depth_first_in_name_order() {
        // ⚠ Ordered in SQL, so the tree does not reshuffle between frames — the sparse-index
        // class of bug this project has hit three times in the Library.
        let cat = Catalog::open_in_memory().unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        cat.create_keyword("Spain", Some(europe)).unwrap();
        cat.create_keyword("France", Some(europe)).unwrap();
        cat.create_keyword("Asia", None).unwrap();

        let got: Vec<(String, usize)> = cat
            .keywords()
            .unwrap()
            .into_iter()
            .map(|k| (k.name, k.depth))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Asia".to_string(), 0),
                ("Europe".to_string(), 0),
                ("France".to_string(), 1),
                ("Spain".to_string(), 1),
            ]
        );
    }
}
