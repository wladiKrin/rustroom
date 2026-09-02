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

use crate::snapshot::{CaptureSnapshot, CatalogSnapshot, KeywordSnapshot, PhotoSnapshot};
use crate::CatalogEntry;
use core_image::Capture;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// The schema this build expects. Bump it whenever [`Catalog::migrate`] gains a step.
pub const SCHEMA_VERSION: i64 = 6;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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

/// One photo row, as [`Catalog::all_photos`] returns it.
///
/// ⚠ Carries `mtime` **from the catalog**, recorded at scan time. That column has existed since
/// M4.1 and nothing has ever read it; a whole-library view is what it was captured for, because
/// walking the filesystem for 10,000 photos is exactly what this view must not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoRow {
    pub path: PathBuf,
    pub copy: u32,
    /// Unix seconds, or `None` if the metadata could not be read when the folder was scanned.
    pub mtime: Option<i64>,
}

/// One catalogued folder, with how many photo rows it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRow {
    pub id: i64,
    pub path: PathBuf,
    pub count: i64,
}

/// A saved rule and the name it is browsed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartRow {
    pub id: i64,
    pub name: String,
    /// The rule, as JSON. ⚠ Text rather than columns: it is a **tree** whose shape the editor will
    /// grow into, and columns would need a migration every time a criterion is added. The
    /// evaluator in `app` is the single authority on what this text means.
    pub rule: String,
}

/// One calendar day that has photos on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayRow {
    pub year: i32,
    pub month: u32,
    pub day: u32,
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
        if from < 4 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–3. A migration that has already run
            // on someone's machine cannot be changed. Third time this rule applies to a database
            // holding real user data.
            //
            // ⚠⚠ Keyed on the **file**, like `photo_keywords` and unlike marks: capture metadata
            // describes the *exposure*, which every copy of a file shares, while a rating
            // describes the image you made, which they do not. **Do not unify them.**
            //
            // ⚠ Every column is nullable and a row exists as soon as a file has been *read*, even
            // if it carried no EXIF at all. The row's presence is what stops the background pass
            // re-reading that file on every folder open — 11–15 ms each time, forever, for
            // nothing.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE photo_capture (
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     taken     INTEGER,
                     camera    TEXT,
                     lens      TEXT,
                     iso       INTEGER,
                     shutter   REAL,
                     aperture  REAL,
                     focal_len REAL,
                     PRIMARY KEY (folder_id, file_name)
                 );
                 PRAGMA user_version = 4;
                 COMMIT;",
            )?;
        }
        if from < 5 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–4. Fourth time this rule meets a
            // database holding real user data.
            //
            // ⚠ The rule is **JSON text**, not columns — see `SmartRow::rule`.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE smart_collections (
                     id      INTEGER PRIMARY KEY,
                     name    TEXT NOT NULL UNIQUE,
                     rule    TEXT NOT NULL,
                     created INTEGER NOT NULL
                 );
                 PRAGMA user_version = 5;
                 COMMIT;",
            )?;
        }
        if from < 6 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–5. Fifth time this rule meets a
            // database holding real user data.
            //
            // ⚠ When this photo's `stem.xmp` sidecar was last **imported or written by
            // Rustroom**, in unix seconds — not when the photo was edited. The whole conflict
            // rule for XMP interchange is "a sidecar newer than this is a foreign edit and
            // wins", so `0` (which every existing row acquires with no backfill) means "never
            // seen", and any sidecar on disk therefore imports on the next folder open. That is
            // the correct behaviour for a catalog that predates this column.
            self.conn.execute_batch(
                "BEGIN;
                 ALTER TABLE photos ADD COLUMN xmp_mtime INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version = 6;
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

    /// When each photo's XMP sidecar was last imported or written, for a whole folder.
    ///
    /// ⚠⚠ **Copy 0 only, and one query per folder.** XMP has no notion of a virtual copy, so
    /// only the original's row carries the timestamp — and the grid rule applies here too: this
    /// is called once per folder open, never once per photo.
    ///
    /// ⚠ Rows with `0` are returned like any other. `0` means "never seen", which is exactly
    /// what the caller compares against, so filtering them out would hide the case that matters
    /// most: a folder rated in Lightroom that Rustroom has never opened.
    pub fn xmp_mtimes_in(&self, folder: &Path) -> Result<Vec<(PathBuf, i64)>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT p.file_name, p.xmp_mtime
             FROM photos p JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1 AND p.copy = 0",
        )?;
        let rows = stmt.query_map([&key], |r| {
            let name: String = r.get(0)?;
            let secs: i64 = r.get(1)?;
            Ok((PathBuf::from(&key).join(name), secs))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Record that this photo's sidecar, as of `secs`, is one Rustroom has seen.
    ///
    /// ⚠ Copy 0, because that is the only row an XMP sidecar describes. ⚠ A photo the catalog
    /// does not know is silently skipped — `set_marks`' rule, for its reason: the catalog is
    /// never why an action fails.
    pub fn set_xmp_mtime(&self, photo: &Path, secs: i64) -> Result<(), CatalogError> {
        let Some(id) = self.photo_id(photo, 0)? else {
            return Ok(());
        };
        self.conn.execute(
            "UPDATE photos SET xmp_mtime = ?2 WHERE id = ?1",
            rusqlite::params![id, secs],
        )?;
        Ok(())
    }

    /// Every photo the catalog knows, from every folder ever opened.
    ///
    /// ⚠ Ordered by folder then name so the result is stable; the Library re-sorts it by the
    /// user's chosen order anyway, but a stable base makes that sort's tie-break meaningful.
    pub fn all_photos(&self) -> Result<Vec<PhotoRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, p.file_name, p.copy, p.mtime
             FROM photos p JOIN folders f ON f.id = p.folder_id
             ORDER BY f.path, p.file_name, p.copy",
        )?;
        let rows = stmt.query_map([], |r| {
            let folder: String = r.get(0)?;
            let name: String = r.get(1)?;
            Ok(PhotoRow {
                path: PathBuf::from(folder).join(name),
                copy: r.get(2)?,
                mtime: r.get(3)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every catalogued folder, with its photo count.
    pub fn folders(&self) -> Result<Vec<FolderRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.path, (SELECT count(*) FROM photos p WHERE p.folder_id = f.id)
             FROM folders f
             ORDER BY lower(f.path), f.path",
        )?;
        let rows = stmt.query_map([], |r| {
            let path: String = r.get(1)?;
            Ok(FolderRow {
                id: r.get(0)?,
                path: PathBuf::from(path),
                count: r.get(2)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Remove a folder from the catalog.
    ///
    /// ⚠⚠ **This deletes catalog rows and never touches a file.** The photos, their sidecars and
    /// every edit in them are untouched — re-opening the folder catalogues it again, minus the
    /// library metadata. That is the M4.1 split doing its job: *delete the catalog and you lose
    /// organisation, never work.*
    ///
    /// ⚠ Its ratings, flags, labels and keyword assignments **do** go, by a chained cascade:
    /// `folders` → `photos` → `collection_photos`, and `folders` → `photo_keywords`. The second
    /// hop needs `PRAGMA foreign_keys = ON`, which is set per connection in `open`.
    /// ⚠ It returns everything it removed, so the deletion can be undone. The read happens
    /// **before** the delete for the obvious reason, and the cascade is what makes the snapshot
    /// closed under references — every row it takes is in here.
    pub fn forget_folder(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let path: String =
            tx.query_row("SELECT path FROM folders WHERE id = ?1", [id], |r| r.get(0))?;

        let mut st = tx.prepare(
            "SELECT id, file_name, copy, size, mtime, last_seen, rating, flag, label
             FROM photos WHERE folder_id = ?1",
        )?;
        let photos: Vec<PhotoSnapshot> = st
            .query_map([id], |r| {
                Ok(PhotoSnapshot {
                    id: r.get(0)?,
                    file_name: r.get(1)?,
                    copy: r.get::<_, i64>(2)? as u32,
                    size: r.get(3)?,
                    mtime: r.get(4)?,
                    last_seen: r.get(5)?,
                    rating: r.get::<_, i64>(6)?.clamp(0, Marks::MAX_RATING as i64) as u8,
                    flag: r.get(7)?,
                    label: r.get::<_, i64>(8)?.clamp(0, Marks::MAX_LABEL as i64) as u8,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut st = tx.prepare(
            "SELECT file_name, taken, camera, lens, iso, shutter, aperture, focal_len
             FROM photo_capture WHERE folder_id = ?1",
        )?;
        let captures: Vec<CaptureSnapshot> = st
            .query_map([id], |r| {
                Ok(CaptureSnapshot {
                    file_name: r.get(0)?,
                    taken: r.get(1)?,
                    camera: r.get(2)?,
                    lens: r.get(3)?,
                    iso: r.get(4)?,
                    shutter: r.get(5)?,
                    aperture: r.get(6)?,
                    focal_len: r.get(7)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut st = tx.prepare(
            "SELECT cp.collection_id, cp.photo_id, cp.added
             FROM collection_photos cp JOIN photos p ON p.id = cp.photo_id
             WHERE p.folder_id = ?1",
        )?;
        let memberships: Vec<(i64, i64, i64)> = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut st =
            tx.prepare("SELECT file_name, keyword_id FROM photo_keywords WHERE folder_id = ?1")?;
        let keywords: Vec<(String, i64)> = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(CatalogSnapshot::Folder {
            path: PathBuf::from(path),
            photos,
            captures,
            memberships,
            keywords,
        })
    }

    /// Record what a file's EXIF said.
    ///
    /// ⚠⚠ **An all-`None` `Capture` is still stored.** The row's presence is what marks the file
    /// as read; skipping it would make the background pass re-read that file on every folder open
    /// forever.
    pub fn set_capture(&self, photo: &Path, c: &Capture) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO photo_capture
                 (folder_id, file_name, taken, camera, lens, iso, shutter, aperture, focal_len)
             SELECT id, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9 FROM folders WHERE path = ?1
             ON CONFLICT(folder_id, file_name) DO UPDATE SET
                 taken = ?3, camera = ?4, lens = ?5, iso = ?6,
                 shutter = ?7, aperture = ?8, focal_len = ?9",
            rusqlite::params![
                folder,
                name,
                c.taken,
                c.camera,
                c.lens,
                c.iso,
                c.shutter,
                c.aperture,
                c.focal_len,
            ],
        )?;
        Ok(())
    }

    /// What was recorded for a file, or `None` if it has never been read.
    ///
    /// ⚠ `Some(Capture::default())` and `None` mean different things: read-and-had-nothing versus
    /// never-read.
    pub fn capture_of(&self, photo: &Path) -> Result<Option<Capture>, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(None);
        };
        let row = self.conn.query_row(
            "SELECT c.taken, c.camera, c.lens, c.iso, c.shutter, c.aperture, c.focal_len
             FROM photo_capture c JOIN folders f ON f.id = c.folder_id
             WHERE f.path = ?1 AND c.file_name = ?2",
            rusqlite::params![folder, name],
            |r| {
                Ok(Capture {
                    taken: r.get(0)?,
                    camera: r.get(1)?,
                    lens: r.get(2)?,
                    iso: r.get::<_, Option<i64>>(3)?.map(|v| v as u32),
                    shutter: r.get::<_, Option<f64>>(4)?.map(|v| v as f32),
                    aperture: r.get::<_, Option<f64>>(5)?.map(|v| v as f32),
                    focal_len: r.get::<_, Option<f64>>(6)?.map(|v| v as f32),
                })
            },
        );
        match row {
            Ok(c) => Ok(Some(c)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The file names in `folder` with no capture row yet — the background pass's queue.
    ///
    /// ⚠ **DISTINCT on `file_name`**: `photos` has one row per (file, copy) while capture is per
    /// file, so without it a photo with three copies would be queued and read three times.
    pub fn files_without_capture(&self, folder: &Path) -> Result<Vec<String>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT p.file_name
             FROM photos p JOIN folders f ON f.id = p.folder_id
             WHERE f.path = ?1
               AND NOT EXISTS (SELECT 1 FROM photo_capture c
                               WHERE c.folder_id = p.folder_id AND c.file_name = p.file_name)
             ORDER BY p.file_name",
        )?;
        let rows = stmt.query_map([&key], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Create a smart collection, or return the existing one of that name.
    ///
    /// ⚠ A duplicate name is not an error, for the reason `create_collection` gives: the name is
    /// the identity a user thinks in, so failing the second time "Keepers" is used is a bug.
    pub fn create_smart(&self, name: &str, rule: &str) -> Result<i64, CatalogError> {
        self.conn.execute(
            "INSERT INTO smart_collections (name, rule, created) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO NOTHING",
            rusqlite::params![name, rule, unix_now()],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM smart_collections WHERE name = ?1",
            [name],
            |r| r.get(0),
        )?)
    }

    /// Every smart collection, ordered by name.
    pub fn smart_collections(&self) -> Result<Vec<SmartRow>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, rule FROM smart_collections ORDER BY lower(name), name")?;
        let rows = stmt.query_map([], |r| {
            Ok(SmartRow {
                id: r.get(0)?,
                name: r.get(1)?,
                rule: r.get(2)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Replace a smart collection's rule, leaving its row and name alone.
    pub fn set_smart_rule(&self, id: i64, rule: &str) -> Result<(), CatalogError> {
        self.conn.execute(
            "UPDATE smart_collections SET rule = ?2 WHERE id = ?1",
            rusqlite::params![id, rule],
        )?;
        Ok(())
    }

    /// Remove a smart collection.
    ///
    /// ⚠ It is a **query**, so nothing it matched is affected — unlike forgetting a folder, which
    /// takes ratings and keywords with it. No confirmation, following `delete_collection`.
    pub fn delete_smart(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let (name, rule, created) = tx.query_row(
            "SELECT name, rule, created FROM smart_collections WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        tx.execute("DELETE FROM smart_collections WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(CatalogSnapshot::Smart {
            name,
            rule,
            created,
        })
    }

    /// Every calendar day with photos on it, ascending.
    ///
    /// ⚠⚠ Grouped by SQL's `strftime`, which works **because `taken` is naive wall-clock
    /// seconds**: the date that comes back is exactly the one the camera wrote, with no timezone
    /// arithmetic anywhere. That is the payoff of the storage decision in plan 1.
    ///
    /// ⚠ Day granularity, not month, because the useful granularity depends on the library —
    /// measured, 738 of 830 photos here fall in one month while six distinct days discriminate,
    /// and a ten-year archive is the other way round. The caller aggregates upward.
    ///
    /// ⚠ `WHERE taken IS NOT NULL` is **defence in depth, not the mechanism** — verified by
    /// removing it, which changed nothing: a NULL date makes `strftime` return NULL, the `i32`
    /// deserialization then fails, and `filter_map(Result::ok)` drops the row. It is kept because
    /// relying on an error being swallowed is not a thing to build on, and because forming the
    /// group at all is wasted work. **No test can distinguish the two**, and inventing one that
    /// appeared to would be worse than saying so.
    pub fn capture_days(&self) -> Result<Vec<DayRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(strftime('%Y', taken, 'unixepoch') AS INTEGER),
                    CAST(strftime('%m', taken, 'unixepoch') AS INTEGER),
                    CAST(strftime('%d', taken, 'unixepoch') AS INTEGER),
                    count(*)
             FROM photo_capture WHERE taken IS NOT NULL
             GROUP BY 1, 2, 3 ORDER BY 1, 2, 3",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(DayRow {
                year: r.get(0)?,
                month: r.get(1)?,
                day: r.get(2)?,
                count: r.get(3)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every photo taken in a period, as `(path, copy)`. `None` widens: a day, a month, a year.
    ///
    /// ⚠ Joins back to `photos` on `(folder_id, file_name)`, so **every copy** of a matching file
    /// comes back — capture is per file, a browsable source is per (file, copy). The rule
    /// `keyword_members` already follows.
    pub fn photos_in_period(
        &self,
        year: i32,
        month: Option<u32>,
        day: Option<u32>,
    ) -> Result<Vec<(PathBuf, u32)>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, p.file_name, p.copy
             FROM photo_capture c
             JOIN photos p ON p.folder_id = c.folder_id AND p.file_name = c.file_name
             JOIN folders f ON f.id = c.folder_id
             WHERE c.taken IS NOT NULL
               AND CAST(strftime('%Y', c.taken, 'unixepoch') AS INTEGER) = ?1
               AND (?2 IS NULL OR CAST(strftime('%m', c.taken, 'unixepoch') AS INTEGER) = ?2)
               AND (?3 IS NULL OR CAST(strftime('%d', c.taken, 'unixepoch') AS INTEGER) = ?3)
             ORDER BY f.path, p.file_name, p.copy",
        )?;
        let rows = stmt.query_map(rusqlite::params![year, month, day], |r| {
            let folder: String = r.get(0)?;
            let name: String = r.get(1)?;
            Ok((PathBuf::from(folder).join(name), r.get::<_, u32>(2)?))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every photo taken in **any** of these periods, as `(path, copy)`, de-duplicated.
    ///
    /// ⚠ `photos_in_period` stays as the single-period case rather than being folded into this —
    /// it is what a plain click uses, and routing it through here would be the same query with
    /// more machinery.
    ///
    /// ⚠⚠ An **empty** slice matches nothing, not everything: it is the state the date panel is
    /// in before anything is clicked, and showing the whole library then would be surprising.
    ///
    /// ⚠ De-duplicated explicitly rather than by a `DISTINCT` over a union, because the periods
    /// can **overlap** — a day and the month containing it are both legitimate selections, and a
    /// photo in both must appear once.
    pub fn photos_in_periods(
        &self,
        periods: &[(i32, Option<u32>, Option<u32>)],
    ) -> Result<Vec<(PathBuf, u32)>, CatalogError> {
        let mut out: Vec<(PathBuf, u32)> = Vec::new();
        for &(year, month, day) in periods {
            for hit in self.photos_in_period(year, month, day)? {
                if !out.contains(&hit) {
                    out.push(hit);
                }
            }
        }
        Ok(out)
    }

    /// Every capture row in a folder, as `(path, capture)`.
    ///
    /// ⚠⚠ One query per folder per rebuild, never one per photo — the rule `marks_in` follows,
    /// because the grid draws sixty times a second over hundreds of thumbnails.
    ///
    /// ⚠ Keyed by path, so a file with several copies appears once: capture is per file.
    /// ⚠ Only files that have been *read* come back; the rest are simply absent.
    pub fn captures_in(&self, folder: &Path) -> Result<Vec<(PathBuf, Capture)>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT c.file_name, c.taken, c.camera, c.lens, c.iso, c.shutter, c.aperture,
                    c.focal_len
             FROM photo_capture c JOIN folders f ON f.id = c.folder_id
             WHERE f.path = ?1",
        )?;
        let rows = stmt.query_map([&key], |r| {
            let name: String = r.get(0)?;
            Ok((
                PathBuf::from(&key).join(name),
                Capture {
                    taken: r.get(1)?,
                    camera: r.get(2)?,
                    lens: r.get(3)?,
                    iso: r.get::<_, Option<i64>>(4)?.map(|v| v as u32),
                    shutter: r.get::<_, Option<f64>>(5)?.map(|v| v as f32),
                    aperture: r.get::<_, Option<f64>>(6)?.map(|v| v as f32),
                    focal_len: r.get::<_, Option<f64>>(7)?.map(|v| v as f32),
                },
            ))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every distinct camera the catalog has seen, sorted.
    pub fn distinct_cameras(&self) -> Result<Vec<String>, CatalogError> {
        self.distinct_capture_column("camera")
    }

    /// Every distinct lens the catalog has seen, sorted.
    pub fn distinct_lenses(&self) -> Result<Vec<String>, CatalogError> {
        self.distinct_capture_column("lens")
    }

    /// ⚠ The column name is interpolated, which is safe **only** because both callers pass a
    /// literal — SQLite cannot bind an identifier. Private for exactly that reason.
    fn distinct_capture_column(&self, col: &str) -> Result<Vec<String>, CatalogError> {
        let sql = format!(
            "SELECT DISTINCT {col} FROM photo_capture
             WHERE {col} IS NOT NULL AND {col} != '' ORDER BY lower({col}), {col}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
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

    /// How much a keyword deletion would take with it: `(descendants, assignments)`.
    ///
    /// ⚠ Descendants **excludes** the keyword itself, and assignments **includes** the whole
    /// subtree's — the two numbers the confirmation says out loud.
    ///
    /// ⚠ The same recursive CTE `keyword_members` uses, so nothing new walks the subtree.
    pub fn keyword_delete_scope(&self, id: i64) -> Result<(usize, usize), CatalogError> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE sub(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT k.id FROM keywords k JOIN sub ON k.parent_id = sub.id
             )
             SELECT
               (SELECT count(*) FROM sub) - 1,
               (SELECT count(*) FROM photo_keywords WHERE keyword_id IN (SELECT id FROM sub))",
        )?;
        let (d, a): (i64, i64) = stmt.query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok((d.max(0) as usize, a.max(0) as usize))
    }

    /// Remove a keyword. ⚠ Its **children go with it**, and every assignment of any of them.
    pub fn delete_keyword(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        // ⚠ The same recursive CTE `keyword_members` uses, and for the same reason: the cascade
        // takes the whole subtree, so the snapshot must hold the whole subtree. The `depth`
        // ordering is what puts **parents before children**, which is what lets the restore remap
        // `parent_id` to an id that already exists.
        let mut st = tx.prepare(
            "WITH RECURSIVE sub(id, name, parent_id, depth) AS (
                 SELECT id, name, parent_id, 0 FROM keywords WHERE id = ?1
                 UNION ALL
                 SELECT k.id, k.name, k.parent_id, sub.depth + 1
                 FROM keywords k JOIN sub ON k.parent_id = sub.id
             )
             SELECT id, name, parent_id FROM sub ORDER BY depth, id",
        )?;
        let tree: Vec<KeywordSnapshot> = st
            .query_map([id], |r| {
                Ok(KeywordSnapshot {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent_id: r.get(2)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(st);

        let mut assignments = Vec::new();
        for k in &tree {
            let mut st = tx.prepare(
                "SELECT folder_id, file_name, keyword_id FROM photo_keywords WHERE keyword_id = ?1",
            )?;
            let rows = st.query_map([k.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            assignments.extend(rows.filter_map(Result::ok));
        }

        tx.execute("DELETE FROM keywords WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(CatalogSnapshot::Keyword { tree, assignments })
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

    /// How many of `files` carry each keyword — the tri-state a photo menu needs.
    ///
    /// ⚠⚠ **Direct assignments only, never the subtree.** `keyword_members` expands descendants
    /// because a *search* for `Europe` must find `Paris`; a menu toggle removes an
    /// **assignment**, so a photo tagged only `Paris` must show `Europe` unchecked. Ticking it
    /// from the subtree would make unticking it appear to do nothing.
    pub fn keyword_counts(
        &self,
        files: &[PathBuf],
    ) -> Result<std::collections::HashMap<i64, usize>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT pk.keyword_id FROM photo_keywords pk
             JOIN folders f ON f.id = pk.folder_id
             WHERE f.path = ?1 AND pk.file_name = ?2",
        )?;
        let mut counts = std::collections::HashMap::new();
        for file in files {
            let Some((folder, name)) = split(file) else {
                continue;
            };
            let rows = stmt.query_map(rusqlite::params![folder, name], |r| r.get::<_, i64>(0))?;
            for id in rows.filter_map(Result::ok) {
                *counts.entry(id).or_insert(0) += 1;
            }
        }
        Ok(counts)
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
    pub fn delete_collection(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        let (name, created) = tx.query_row(
            "SELECT name, created FROM collections WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut st =
            tx.prepare("SELECT photo_id, added FROM collection_photos WHERE collection_id = ?1")?;
        let members: Vec<(i64, i64)> = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(st);
        tx.execute("DELETE FROM collections WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(CatalogSnapshot::Collection {
            name,
            created,
            members,
        })
    }

    /// Put back what a deletion removed.
    ///
    /// ⚠⚠ **Everything is re-inserted with FRESH ids, and the old ones only match children to
    /// parents.** Preserving them collides: SQLite hands a freed rowid to the next insert, so
    /// deleting a collection, creating another and then undoing fails on the UNIQUE index. Because
    /// a cascade removes exactly the dependent rows, a snapshot is closed under references, so
    /// remapping is total.
    ///
    /// ⚠ One transaction, so a restore is all or nothing — a half-restored folder would be worse
    /// than one that failed.
    ///
    /// ⚠ A row whose *other* end has since gone — a membership whose collection was deleted after
    /// the folder was forgotten — is **skipped, not fatal**. The rest still lands, which is the
    /// rule `add_to_collection` already states: the catalog is never a reason an action fails.
    pub fn restore(&self, snap: &CatalogSnapshot) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        match snap {
            CatalogSnapshot::Smart {
                name,
                rule,
                created,
            } => {
                tx.execute(
                    "INSERT INTO smart_collections (name, rule, created) VALUES (?1, ?2, ?3)",
                    rusqlite::params![name, rule, created],
                )?;
            }
            CatalogSnapshot::Collection {
                name,
                created,
                members,
            } => {
                tx.execute(
                    "INSERT INTO collections (name, created) VALUES (?1, ?2)",
                    rusqlite::params![name, created],
                )?;
                let cid = tx.last_insert_rowid();
                for (photo_id, added) in members {
                    // ⚠ The photo ids are **not** remapped — the photos were never deleted. One
                    // that has since gone is skipped by the `SELECT ... WHERE EXISTS`.
                    tx.execute(
                        "INSERT INTO collection_photos (collection_id, photo_id, added)
                         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM photos WHERE id = ?2)",
                        rusqlite::params![cid, photo_id, added],
                    )?;
                }
            }
            CatalogSnapshot::Keyword { tree, assignments } => {
                let mut map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
                // ⚠ `tree` is parents-first, so a child's remapped parent always already exists.
                for k in tree {
                    let parent = k.parent_id.map(|p| map.get(&p).copied().unwrap_or(p));
                    tx.execute(
                        "INSERT INTO keywords (name, parent_id) VALUES (?1, ?2)",
                        rusqlite::params![k.name, parent],
                    )?;
                    map.insert(k.id, tx.last_insert_rowid());
                }
                for (folder_id, file_name, keyword_id) in assignments {
                    let Some(kid) = map.get(keyword_id) else {
                        continue;
                    };
                    tx.execute(
                        "INSERT OR IGNORE INTO photo_keywords (folder_id, file_name, keyword_id)
                         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM folders WHERE id = ?1)",
                        rusqlite::params![folder_id, file_name, kid],
                    )?;
                }
            }
            CatalogSnapshot::Folder {
                path,
                photos,
                captures,
                memberships,
                keywords,
            } => {
                let path = path.to_string_lossy().to_string();
                let last_seen = photos.iter().map(|p| p.last_seen).max().unwrap_or(0);
                tx.execute(
                    "INSERT INTO folders (path, last_seen) VALUES (?1, ?2)
                     ON CONFLICT(path) DO UPDATE SET last_seen = ?2",
                    rusqlite::params![path, last_seen],
                )?;
                let fid: i64 =
                    tx.query_row("SELECT id FROM folders WHERE path = ?1", [&path], |r| {
                        r.get(0)
                    })?;

                let mut map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
                for p in photos {
                    tx.execute(
                        "INSERT INTO photos
                             (folder_id, file_name, copy, size, mtime, last_seen, rating, flag, label)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                         ON CONFLICT(folder_id, file_name, copy) DO UPDATE SET
                             size = ?4, mtime = ?5, last_seen = ?6,
                             rating = ?7, flag = ?8, label = ?9",
                        rusqlite::params![
                            fid,
                            p.file_name,
                            p.copy,
                            p.size,
                            p.mtime,
                            p.last_seen,
                            p.rating,
                            p.flag,
                            p.label,
                        ],
                    )?;
                    let nid: i64 = tx.query_row(
                        "SELECT id FROM photos WHERE folder_id = ?1 AND file_name = ?2 AND copy = ?3",
                        rusqlite::params![fid, p.file_name, p.copy],
                        |r| r.get(0),
                    )?;
                    map.insert(p.id, nid);
                }

                for c in captures {
                    tx.execute(
                        "INSERT OR REPLACE INTO photo_capture
                             (folder_id, file_name, taken, camera, lens, iso, shutter, aperture, focal_len)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        rusqlite::params![
                            fid,
                            c.file_name,
                            c.taken,
                            c.camera,
                            c.lens,
                            c.iso,
                            c.shutter,
                            c.aperture,
                            c.focal_len,
                        ],
                    )?;
                }

                for (collection_id, photo_id, added) in memberships {
                    let Some(pid) = map.get(photo_id) else {
                        continue;
                    };
                    tx.execute(
                        "INSERT OR IGNORE INTO collection_photos (collection_id, photo_id, added)
                         SELECT ?1, ?2, ?3
                         WHERE EXISTS (SELECT 1 FROM collections WHERE id = ?1)",
                        rusqlite::params![collection_id, pid, added],
                    )?;
                }

                for (file_name, keyword_id) in keywords {
                    tx.execute(
                        "INSERT OR IGNORE INTO photo_keywords (folder_id, file_name, keyword_id)
                         SELECT ?1, ?2, ?3
                         WHERE EXISTS (SELECT 1 FROM keywords WHERE id = ?3)",
                        rusqlite::params![fid, file_name, keyword_id],
                    )?;
                }
            }
        }
        tx.commit()?;
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

    /// Entries for `folder`, one per name.
    ///
    /// ⚠ The `entry` helper below hardcodes `/x/`, which cannot express the two-folder case every
    /// whole-library query needs — and four existing tests depend on that path, so this is a
    /// sibling rather than a change to it.
    fn entries_in(folder: &str, names: &[&str]) -> Vec<CatalogEntry> {
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
                    "DROP TABLE smart_collections;
                     DROP TABLE photo_capture;
                     DROP TABLE photo_keywords;
                     DROP TABLE keywords;
                     ALTER TABLE photos DROP COLUMN xmp_mtime;
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
                    "DROP TABLE smart_collections;
                     DROP TABLE photo_capture;
                     DROP TABLE photo_keywords;
                     DROP TABLE keywords;
                     ALTER TABLE photos DROP COLUMN xmp_mtime;
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
    fn a_version_five_catalog_upgrades_and_keeps_everything() {
        // ⚠⚠ The fifth migration to meet real user data — folders, photos, collections, marks
        // and keywords all present. Adding a column must touch nothing else.
        let db = TempDb::new("v5v6");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
            let c = cat.create_collection("Portfolio").unwrap();
            cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();
            let k = cat.create_keyword("Paris", None).unwrap();
            cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();
            cat.set_marks(
                Path::new("/x/a.RAF"),
                0,
                Marks {
                    rating: 3,
                    label: 2,
                    ..Marks::default()
                },
            )
            .unwrap();
            cat.conn
                .execute_batch(
                    "ALTER TABLE photos DROP COLUMN xmp_mtime;
                     PRAGMA user_version = 5;",
                )
                .unwrap();
        }

        let cat = Catalog::open(&db.0).expect("a v5 catalog must upgrade");

        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(cat.photo_count().unwrap(), 1);
        assert_eq!(cat.collections().unwrap()[0].count, 1, "collection kept");
        assert_eq!(cat.keywords().unwrap()[0].count, 1, "keyword kept");
        let m = cat.marks(Path::new("/x/a.RAF"), 0).unwrap();
        assert_eq!((m.rating, m.label), (3, 2), "marks kept");
        // ⚠ An upgraded row has never had its sidecar seen, which is what makes an existing
        // folder rated in Lightroom import on the next open rather than being ignored forever.
        assert_eq!(
            cat.xmp_mtimes_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), 0)]
        );
    }

    #[test]
    fn an_xmp_mtime_round_trips_and_defaults_to_zero() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        assert_eq!(
            cat.xmp_mtimes_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), 0)],
            "a fresh row has never been seen"
        );
        cat.set_xmp_mtime(Path::new("/x/a.RAF"), 1_700_000_000)
            .unwrap();
        assert_eq!(
            cat.xmp_mtimes_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), 1_700_000_000)]
        );
    }

    #[test]
    fn xmp_mtimes_come_back_for_copy_zero_only() {
        // ⚠ XMP has no notion of a virtual copy, so only the original carries the timestamp.
        // Without the `copy = 0` clause a three-copy photo would come back three times and the
        // caller's map would be built from whichever row happened to be last.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 1).unwrap();
        cat.add_copy(Path::new("/x/a.RAF"), 2).unwrap();
        cat.set_xmp_mtime(Path::new("/x/a.RAF"), 42).unwrap();

        let rows = cat.xmp_mtimes_in(Path::new("/x")).unwrap();
        assert_eq!(rows, vec![(PathBuf::from("/x/a.RAF"), 42)]);
    }

    #[test]
    fn setting_the_xmp_mtime_of_an_unknown_photo_is_harmless() {
        let cat = Catalog::open_in_memory().unwrap();
        assert!(cat.set_xmp_mtime(Path::new("/nowhere/a.RAF"), 7).is_ok());
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
    fn keyword_counts_count_direct_assignments_and_not_the_subtree() {
        // ⚠⚠ The trap this exists for: `keyword_members` deliberately expands descendants, so a
        // search for Europe finds a photo tagged only Paris. A menu **toggle** must not — it
        // removes an assignment, and a ticked Europe that unticking cannot clear reads as the
        // control being broken.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF"), entry("b.RAF")])
            .unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        let paris = cat.create_keyword("Paris", Some(europe)).unwrap();
        cat.assign_keyword(paris, Path::new("/x/a.RAF")).unwrap();
        cat.assign_keyword(europe, Path::new("/x/a.RAF")).unwrap();
        cat.assign_keyword(paris, Path::new("/x/b.RAF")).unwrap();

        let files = vec![PathBuf::from("/x/a.RAF"), PathBuf::from("/x/b.RAF")];
        let counts = cat.keyword_counts(&files).unwrap();

        assert_eq!(counts.get(&paris), Some(&2), "both carry Paris directly");
        assert_eq!(
            counts.get(&europe),
            Some(&1),
            "only one carries Europe itself — b.RAF is tagged Paris, which is not the same thing"
        );
    }

    #[test]
    fn an_untagged_file_contributes_no_counts() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        cat.create_keyword("Paris", None).unwrap();

        let counts = cat.keyword_counts(&[PathBuf::from("/x/a.RAF")]).unwrap();

        assert!(counts.is_empty(), "an absent keyword is absent, not zero");
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

    #[test]
    fn all_photos_spans_every_folder() {
        // ⚠ The point of the whole slice: `open_folder`, `open_collection` and `open_keyword`
        // were the only loaders, so the catalog held everything and nothing could see it whole.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["three.RAF"]))
            .unwrap();
        let rows = cat.all_photos().unwrap();
        assert_eq!(rows.len(), 3);
        let mut paths: Vec<String> = rows
            .iter()
            .map(|r| r.path.to_string_lossy().to_string())
            .collect();
        paths.sort();
        assert_eq!(paths, ["/a/one.RAF", "/a/two.RAF", "/b/three.RAF"]);
    }

    #[test]
    fn all_photos_includes_every_copy() {
        // A virtual copy is its own row and its own identity — it must appear here as one.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        let rows = cat.all_photos().unwrap();
        assert_eq!(rows.len(), 2);
        let mut copies: Vec<u32> = rows.iter().map(|r| r.copy).collect();
        copies.sort();
        assert_eq!(copies, [0, 1]);
    }

    #[test]
    fn folders_are_listed_with_their_photo_counts() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["three.RAF"]))
            .unwrap();
        let rows = cat.folders().unwrap();
        assert_eq!(rows.len(), 2);
        let by_path: Vec<(String, i64)> = rows
            .iter()
            .map(|f| (f.path.to_string_lossy().to_string(), f.count))
            .collect();
        assert!(by_path.contains(&("/a".to_string(), 2)), "{by_path:?}");
        assert!(by_path.contains(&("/b".to_string(), 1)), "{by_path:?}");
    }

    #[test]
    fn forgetting_a_folder_cascades_through_photos_to_collections_and_keywords() {
        // ⚠⚠ A **chained** cascade, and the first feature to depend on two hops in series:
        // folders -> photos -> collection_photos, and folders -> photo_keywords directly. The
        // second hop happens only because `PRAGMA foreign_keys = ON` is set per connection —
        // SQLite defaults it OFF, which is why that pragma already has a test of its own.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["two.RAF"]))
            .unwrap();
        let coll = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(coll, Path::new("/a/one.RAF"), 0)
            .unwrap();
        cat.add_to_collection(coll, Path::new("/b/two.RAF"), 0)
            .unwrap();
        let kw = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(kw, Path::new("/a/one.RAF")).unwrap();

        let id = cat
            .folders()
            .unwrap()
            .into_iter()
            .find(|f| f.path == Path::new("/a"))
            .unwrap()
            .id;
        cat.forget_folder(id).unwrap();

        // ⚠⚠ THE ASSERTIONS THAT MATTER ARE THE **RAW COUNTS**, AND FINDING THAT OUT COST A
        // SABOTAGE. `all_photos`, `collection_members` and `keyword_members` all JOIN through
        // `folders` — so with the cascade disabled the orphaned rows simply drop out of those
        // queries and every join-based assertion below passes vacuously. The first version of
        // this test asserted only on those, and `PRAGMA foreign_keys = OFF` left it green.
        //
        // `photo_count`, and the counts inside `collections()` and `keywords()`, do **not** join
        // `folders` — they count the rows themselves, which is exactly what a missing cascade
        // leaves behind. Orphans are invisible to the UI and accumulate forever, and worse: a
        // folder re-added at the same path gets a *new* id, so the old orphans are unreachable.
        assert_eq!(
            cat.photo_count().unwrap(),
            1,
            "orphaned photo rows survived"
        );
        assert_eq!(
            cat.collections().unwrap()[0].count,
            1,
            "an orphaned collection membership survived"
        );
        assert_eq!(
            cat.keywords().unwrap()[0].count,
            0,
            "an orphaned keyword assignment survived"
        );

        // And the user-visible effect, which the raw counts do not by themselves prove.
        assert_eq!(cat.all_photos().unwrap().len(), 1, "photo rows survived");
        assert_eq!(cat.folders().unwrap().len(), 1, "the folder row survived");
        assert_eq!(
            cat.collection_members(coll).unwrap().len(),
            1,
            "the membership of the forgotten folder's photo survived"
        );
        assert!(
            cat.keyword_members(kw).unwrap().is_empty(),
            "the keyword assignment survived"
        );
    }

    #[test]
    fn forgetting_a_folder_leaves_the_other_folders_alone() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["two.RAF"]))
            .unwrap();
        let id = cat
            .folders()
            .unwrap()
            .into_iter()
            .find(|f| f.path == Path::new("/a"))
            .unwrap()
            .id;
        cat.forget_folder(id).unwrap();
        let left = cat.all_photos().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].path, Path::new("/b/two.RAF"));
    }

    #[test]
    fn forgetting_a_folder_deletes_no_files() {
        // ⚠⚠ The M4.1 split, asserted: *delete the catalog and you lose organisation, never work.*
        // Edits live in the per-photo sidecars and survive this entirely. Task 1's in-memory tests
        // structurally cannot see this — they have no files behind them.
        let db = TempDb::new("nofiles");
        let dir = db.0.parent().unwrap().to_path_buf();
        let photo = dir.join("one.RAF");
        std::fs::write(&photo, b"not really a raw").unwrap();
        let sidecar = dir.join("one.RAF.json");
        std::fs::write(&sidecar, b"{}").unwrap();

        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            &dir,
            &[CatalogEntry {
                path: photo.clone(),
                file_name: "one.RAF".to_string(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            }],
        )
        .unwrap();
        let id = cat.folders().unwrap()[0].id;
        cat.forget_folder(id).unwrap();

        assert!(photo.exists(), "the photo was deleted");
        assert!(sidecar.exists(), "the sidecar was deleted");
        assert!(cat.all_photos().unwrap().is_empty());
    }

    #[test]
    fn a_capture_round_trips() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        let c = Capture {
            taken: Some(1_779_046_001),
            camera: Some("FUJIFILM X-T4".into()),
            lens: Some("XF23mmF2 R WR".into()),
            iso: Some(1600),
            shutter: Some(0.02),
            aperture: Some(2.0),
            focal_len: Some(23.0),
        };
        cat.set_capture(Path::new("/a/one.RAF"), &c).unwrap();
        assert_eq!(cat.capture_of(Path::new("/a/one.RAF")).unwrap(), Some(c));
    }

    #[test]
    fn an_empty_capture_is_still_recorded() {
        // ⚠⚠ A file with no EXIF must be *marked as read*, or the background pass re-reads it on
        // every folder open forever — 11–15 ms of file reading each time, for nothing.
        //
        // ⚠ `Some(default)` and `None` mean different things here: read-and-had-nothing versus
        // never-read.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        assert_eq!(cat.capture_of(Path::new("/a/one.RAF")).unwrap(), None);
        cat.set_capture(Path::new("/a/one.RAF"), &Capture::default())
            .unwrap();
        assert_eq!(
            cat.capture_of(Path::new("/a/one.RAF")).unwrap(),
            Some(Capture::default())
        );
        assert_eq!(
            cat.files_without_capture(Path::new("/a")).unwrap(),
            vec!["two.RAF".to_string()],
            "a file recorded as having no EXIF must not be queued again"
        );
    }

    #[test]
    fn capture_is_per_file_so_every_copy_shares_it() {
        // ⚠⚠ Per *file*, like `photo_keywords` and unlike marks: capture metadata describes the
        // **exposure**, which every copy of a file shares. A rating describes the image you made,
        // which they do not.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 2).unwrap();
        // ⚠ Three photo rows, and the queue must name the file **once** — without DISTINCT it
        // would be read three times.
        assert_eq!(
            cat.files_without_capture(Path::new("/a")).unwrap(),
            vec!["one.RAF".to_string()]
        );
        let c = Capture {
            iso: Some(400),
            ..Default::default()
        };
        cat.set_capture(Path::new("/a/one.RAF"), &c).unwrap();
        assert_eq!(cat.capture_of(Path::new("/a/one.RAF")).unwrap(), Some(c));
        assert!(cat
            .files_without_capture(Path::new("/a"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn forgetting_a_folder_cascades_to_its_capture_rows() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.set_capture(
            Path::new("/a/one.RAF"),
            &Capture {
                iso: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
        let id = cat.folders().unwrap()[0].id;
        cat.forget_folder(id).unwrap();
        // ⚠ The **raw** count, not a join — the lesson from `forget_folder`'s own cascade test,
        // where every join-based assertion passed vacuously with the cascade disabled.
        let left: i64 = cat
            .conn
            .query_row("SELECT count(*) FROM photo_capture", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "orphaned capture rows survived");
    }

    /// A capture at a given wall-clock date.
    ///
    /// ⚠ The stored value is naive wall-clock seconds, so building it from a civil date with no
    /// timezone step is not a bug — it is the convention both readers use. See `Capture::taken`.
    fn on_day(y: i32, m: u32, d: u32) -> Capture {
        Capture {
            taken: Some(days_from_civil(y, m, d) * 86_400),
            ..Default::default()
        }
    }

    /// Days since the epoch for a civil date — Howard Hinnant's `days_from_civil`.
    ///
    /// ⚠ Written out rather than pulling `chrono` into `catalog` for one test helper. The
    /// algorithm is standard and the test below pins it against known dates.
    fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as i64;
        let mp = ((m + 9) % 12) as i64;
        let doy = (153 * mp + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era as i64 * 146_097 + doe - 719_468
    }

    #[test]
    fn the_day_helper_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2026, 5, 17), 20590);
    }

    #[test]
    fn capture_days_groups_by_wall_clock_date() {
        // ⚠ The payoff of storing wall-clock: SQL's `strftime` gives back exactly the date the
        // camera wrote, with no timezone arithmetic anywhere.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            Path::new("/a"),
            &entries_in("/a", &["one.RAF", "two.RAF", "three.RAF"]),
        )
        .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        cat.set_capture(Path::new("/a/three.RAF"), &on_day(2026, 7, 9))
            .unwrap();
        assert_eq!(
            cat.capture_days().unwrap(),
            vec![
                DayRow {
                    year: 2026,
                    month: 5,
                    day: 17,
                    count: 2
                },
                DayRow {
                    year: 2026,
                    month: 7,
                    day: 9,
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn a_file_with_no_date_is_absent_from_the_tree() {
        // ⚠ A capture row exists for it (it was read) but it has nothing to place on a calendar.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &Capture::default())
            .unwrap();
        assert!(cat.capture_days().unwrap().is_empty());
    }

    #[test]
    fn a_period_narrows_from_a_year_down_to_a_day() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["three.RAF"]))
            .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &on_day(2026, 7, 9))
            .unwrap();
        cat.set_capture(Path::new("/b/three.RAF"), &on_day(2025, 7, 9))
            .unwrap();

        // ⚠ Spans folders, like every other browsable source.
        assert_eq!(cat.photos_in_period(2026, None, None).unwrap().len(), 2);
        assert_eq!(cat.photos_in_period(2026, Some(7), None).unwrap().len(), 1);
        assert_eq!(
            cat.photos_in_period(2026, Some(7), Some(9)).unwrap().len(),
            1
        );
        assert_eq!(
            cat.photos_in_period(2026, Some(5), Some(9)).unwrap().len(),
            0
        );
        assert_eq!(cat.photos_in_period(2025, None, None).unwrap().len(), 1);
    }

    #[test]
    fn a_period_returns_every_copy_of_a_matching_file() {
        // ⚠ Capture is per file; a browsable source is per (file, copy). Every copy of a photo
        // taken that day belongs in the result — the rule `keyword_members` already follows.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        let mut copies: Vec<u32> = cat
            .photos_in_period(2026, Some(5), Some(17))
            .unwrap()
            .into_iter()
            .map(|(_, c)| c)
            .collect();
        copies.sort();
        assert_eq!(copies, [0, 1]);
    }

    #[test]
    fn captures_in_returns_one_row_per_file_that_has_one() {
        // ⚠ One query per folder, never one per photo — the rule `marks_in` already follows.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        let got = cat.captures_in(Path::new("/a")).unwrap();
        // ⚠ Keyed by **path**, so a file with two copies is one entry — capture is per file.
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, PathBuf::from("/a/one.RAF"));
        assert_eq!(got[0].1.taken, on_day(2026, 5, 17).taken);
    }

    #[test]
    fn distinct_cameras_and_lenses_come_back_sorted_and_deduplicated() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            Path::new("/a"),
            &entries_in("/a", &["one.RAF", "two.RAF", "three.RAF"]),
        )
        .unwrap();
        let mk = |cam: &str, lens: &str| Capture {
            camera: Some(cam.into()),
            lens: Some(lens.into()),
            ..Default::default()
        };
        cat.set_capture(
            Path::new("/a/one.RAF"),
            &mk("FUJIFILM X-T4", "XF23mmF2 R WR"),
        )
        .unwrap();
        cat.set_capture(
            Path::new("/a/two.RAF"),
            &mk("FUJIFILM X-T4", "XF16-80mmF4 R OIS WR"),
        )
        .unwrap();
        cat.set_capture(Path::new("/a/three.RAF"), &Capture::default())
            .unwrap();
        assert_eq!(
            cat.distinct_cameras().unwrap(),
            vec!["FUJIFILM X-T4".to_string()]
        );
        assert_eq!(
            cat.distinct_lenses().unwrap(),
            vec![
                "XF16-80mmF4 R OIS WR".to_string(),
                "XF23mmF2 R WR".to_string()
            ]
        );
    }

    #[test]
    fn a_smart_collection_round_trips() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        let rows = cat.smart_collections().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].name, "Keepers");
        assert_eq!(rows[0].rule, r#"{"All":[]}"#);
    }

    #[test]
    fn a_duplicate_smart_name_returns_the_existing_one() {
        // ⚠ The rule `create_collection` and `create_keyword` already follow: the name is the
        // identity a user thinks in, so failing the second time "Keepers" is used is a bug.
        let cat = Catalog::open_in_memory().unwrap();
        let a = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        let b = cat.create_smart("Keepers", r#"{"Any":[]}"#).unwrap();
        assert_eq!(a, b);
        assert_eq!(cat.smart_collections().unwrap().len(), 1);
    }

    #[test]
    fn a_rule_can_be_replaced_without_changing_the_row() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        cat.set_smart_rule(id, r#"{"Any":[]}"#).unwrap();
        let rows = cat.smart_collections().unwrap();
        assert_eq!(rows.len(), 1, "editing must not create a second row");
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].rule, r#"{"Any":[]}"#);
    }

    #[test]
    fn deleting_a_smart_collection_touches_no_photos() {
        // ⚠ The property people actually worry about — a smart collection is a *query*, and
        // nothing it matched is affected by removing it. Unlike forgetting a folder, which takes
        // ratings and keywords with it.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        let id = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        cat.delete_smart(id).unwrap();
        assert!(cat.smart_collections().unwrap().is_empty());
        assert_eq!(cat.photo_count().unwrap(), 1);
    }

    #[test]
    fn several_periods_come_back_as_one_set() {
        // ⚠ A union, not a repeated query: a photo in two of the requested periods (a day and the
        // month containing it) must appear **once**.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            Path::new("/a"),
            &entries_in("/a", &["one.RAF", "two.RAF", "three.RAF"]),
        )
        .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &on_day(2026, 7, 9))
            .unwrap();
        cat.set_capture(Path::new("/a/three.RAF"), &on_day(2025, 1, 1))
            .unwrap();

        let got = cat
            .photos_in_periods(&[(2026, Some(5), Some(17)), (2026, Some(7), Some(9))])
            .unwrap();
        assert_eq!(got.len(), 2);

        // ⚠ Overlapping periods must not double a photo.
        let overlapping = cat
            .photos_in_periods(&[(2026, Some(5), Some(17)), (2026, Some(5), None)])
            .unwrap();
        assert_eq!(
            overlapping.len(),
            1,
            "an overlapping period doubled a photo"
        );
    }

    #[test]
    fn an_empty_period_list_matches_nothing() {
        // ⚠ Not "everything": an empty selection is the state the panel is in before any date is
        // clicked, and showing the whole library then would be surprising.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        assert!(cat.photos_in_periods(&[]).unwrap().is_empty());
    }

    /// A catalog with one folder of two photos, for the restore tests.
    fn with_two_photos() -> (Catalog, PathBuf) {
        let cat = Catalog::open_in_memory().unwrap();
        let dir = PathBuf::from("/a");
        cat.sync_folder(&dir, &entries_in("/a", &["a.RAF", "b.RAF"]))
            .unwrap();
        (cat, dir)
    }

    /// ⚠⚠ **The collision that motivated the whole design.** Delete a collection, create another
    /// (which takes the freed id, because SQLite reuses `max + 1` without `AUTOINCREMENT`), then
    /// undo. Preserving the original id fails here with `UNIQUE constraint failed`; remapping
    /// cannot.
    #[test]
    fn a_restore_survives_its_id_being_taken_by_something_else() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_collection("Portfolio").unwrap();
        let snap = cat.delete_collection(id).unwrap();
        let other = cat.create_collection("Travel").unwrap();
        assert_eq!(other, id, "fixture check: the freed id really was reused");

        cat.restore(&snap).unwrap();
        let names: Vec<String> = cat
            .collections()
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert!(names.contains(&"Portfolio".to_string()));
        assert!(names.contains(&"Travel".to_string()));
    }

    /// ⚠ The marks are the point of a folder restore: they are the only thing a forget destroys
    /// that the user made by hand, and a restore that dropped them would look like it worked.
    #[test]
    fn forgetting_a_folder_and_undoing_brings_back_its_marks() {
        let (cat, dir) = with_two_photos();
        cat.set_marks(
            &dir.join("a.RAF"),
            0,
            Marks {
                rating: 4,
                flag: Flag::Pick,
                label: 2,
            },
        )
        .unwrap();
        let fid = cat.folders().unwrap()[0].id;

        let snap = cat.forget_folder(fid).unwrap();
        assert_eq!(cat.photo_count().unwrap(), 0);

        cat.restore(&snap).unwrap();
        assert_eq!(cat.photo_count().unwrap(), 2);
        let back = cat.marks_in(&dir).unwrap();
        let (_, _, m) = back
            .iter()
            .find(|(_, _, m)| m.rating == 4)
            .expect("the rating must come back");
        assert_eq!(m.flag, Flag::Pick);
        assert_eq!(m.label, 2);
    }

    /// ⚠ A keyword takes its whole subtree, so the restore must bring the hierarchy back — not
    /// just the rows, but the parent links, which are remapped.
    #[test]
    fn undoing_a_keyword_deletion_restores_the_subtree_and_its_shape() {
        let cat = Catalog::open_in_memory().unwrap();
        let europe = cat.create_keyword("Europe", None).unwrap();
        let france = cat.create_keyword("France", Some(europe)).unwrap();
        cat.create_keyword("Paris", Some(france)).unwrap();

        let snap = cat.delete_keyword(europe).unwrap();
        assert!(cat.keywords().unwrap().is_empty());

        cat.restore(&snap).unwrap();
        let rows = cat.keywords().unwrap();
        assert_eq!(rows.len(), 3, "all three come back");
        let by_name = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();
        let (e, f, p) = (by_name("Europe"), by_name("France"), by_name("Paris"));
        assert_eq!(e.parent_id, None, "Europe is still a root");
        assert_eq!(f.parent_id, Some(e.id), "France still hangs off Europe");
        assert_eq!(p.parent_id, Some(f.id), "and Paris off France");
    }

    /// ⚠⚠ **A row that can no longer be satisfied is skipped, not fatal.** The rest of the restore
    /// must still land — the rule `add_to_collection` already states, that the catalog must never
    /// be a reason an action fails. Asserted by *count*, so "it did not error" cannot pass it.
    #[test]
    fn a_membership_whose_collection_is_gone_is_skipped_and_the_rest_restores() {
        let (cat, dir) = with_two_photos();
        let coll = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(coll, &dir.join("a.RAF"), 0).unwrap();
        let fid = cat.folders().unwrap()[0].id;

        let snap = cat.forget_folder(fid).unwrap();
        // The collection those memberships pointed at is deleted before the undo.
        cat.delete_collection(coll).unwrap();

        cat.restore(&snap).unwrap();
        assert_eq!(cat.photo_count().unwrap(), 2, "the photos still come back");
    }

    #[test]
    fn undoing_a_smart_collection_deletion_restores_its_rule() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_smart("Picks", "{\"All\":[]}").unwrap();
        let snap = cat.delete_smart(id).unwrap();
        assert!(cat.smart_collections().unwrap().is_empty());

        cat.restore(&snap).unwrap();
        let rows = cat.smart_collections().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Picks");
        assert_eq!(rows[0].rule, "{\"All\":[]}");
    }

    /// ⚠ A collection's members must come back, not just its name.
    #[test]
    fn undoing_a_collection_deletion_restores_its_members() {
        let (cat, dir) = with_two_photos();
        let coll = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(coll, &dir.join("a.RAF"), 0).unwrap();

        let snap = cat.delete_collection(coll).unwrap();
        cat.restore(&snap).unwrap();

        let row = cat.collections().unwrap().into_iter().next().unwrap();
        assert_eq!(row.name, "Portfolio");
        assert_eq!(row.count, 1, "the membership came back too");
    }
    #[test]
    fn the_delete_scope_counts_the_subtree_but_not_the_keyword_itself() {
        let c = Catalog::open_in_memory().unwrap();
        let europe = c.create_keyword("Europe", None).unwrap();
        let france = c.create_keyword("France", Some(europe)).unwrap();
        c.create_keyword("Paris", Some(france)).unwrap();
        assert_eq!(c.keyword_delete_scope(europe).unwrap(), (2, 0));
        assert_eq!(c.keyword_delete_scope(france).unwrap().0, 1);
    }

    /// ⚠ Both halves. A dialog that never fires and one that always fires are equally wrong, and
    /// only the pair of assertions tells them apart.
    #[test]
    fn a_bare_keyword_is_deleted_without_a_dialog_and_a_parent_is_not() {
        let c = Catalog::open_in_memory().unwrap();
        let leaf = c.create_keyword("Paris", None).unwrap();
        assert_eq!(c.keyword_delete_scope(leaf).unwrap(), (0, 0), "no dialog");

        let parent = c.create_keyword("Europe", None).unwrap();
        c.create_keyword("France", Some(parent)).unwrap();
        assert_ne!(c.keyword_delete_scope(parent).unwrap(), (0, 0), "dialog");
    }

    /// ⚠ An assignment alone is enough to confirm, with no descendants at all — the second half
    /// of the rule, which the descendant tests cannot see.
    #[test]
    fn an_assignment_alone_is_enough_to_need_a_confirmation() {
        let c = Catalog::open_in_memory().unwrap();
        c.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        let k = c.create_keyword("Paris", None).unwrap();
        c.assign_keyword(k, Path::new("/a/one.RAF")).unwrap();
        assert_eq!(c.keyword_delete_scope(k).unwrap(), (0, 1));
    }
}
