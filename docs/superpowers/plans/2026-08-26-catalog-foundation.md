# Catalog Foundation (M4.1, plan 1 of 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One invisible SQLite catalog that records every folder opened and every photo in it, and that survives renames and reindexing without losing a row.

**Architecture:** A new `catalog::db` module wrapping `rusqlite`, with the schema versioned through `PRAGMA user_version`. The `app` opens it once at startup and syncs a folder's photos into it whenever a folder is opened, renamed or reindexed. Edits stay in sidecars; only library metadata lives here.

**Tech Stack:** Rust, `rusqlite` (`bundled`), SQLite.

**Spec:** `docs/superpowers/specs/2026-08-26-catalog-database-design.md`

**Build environment:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing rules:** no `Co-Authored-By`; never push; sabotage every new test and **confirm it compiled**; clippy clean checked by **exit code**.

⚠ **Check exit codes, never grep counts** — `cargo test --all | grep -c "test result: FAILED"` returns 0 when the *build* fails:

```bash
cargo test --all > /tmp/t.log 2>&1; echo "exit: $?"
```

⚠ **Check clippy separately from tests.** `cargo test` only *warns* on an unused variable while `clippy -D warnings` rejects it, so a green test run is not evidence. A commit went in with clippy failing during the previous milestone for exactly this reason.

⚠⚠ **This plan is invisible when it works.** Nothing in the UI changes. If you can tell it shipped, something is wrong. The payoff is plan 2 (collections).

⚠⚠ **The dangerous part is that a scan must never delete a photo row.** An unplugged drive or unmounted share would otherwise silently empty every collection referencing it — and collections do not exist yet, so nothing would catch it until plan 2. Task 4 is the one to slow down on.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/catalog/Cargo.toml` (modify) | Adds `rusqlite` with `bundled` |
| `crates/catalog/src/db.rs` (create) | The whole database: open, migrate, sync, query |
| `crates/catalog/src/lib.rs` (modify) | Declares `db` |
| `crates/app/src/main.rs` (modify) | Opens the catalog at startup; syncs on open/rename/reindex |

⚠ **One module, not one per table.** The schema is four small tables and the operations are a handful of statements; splitting by table would scatter a transaction across files. Revisit if `db.rs` passes ~400 lines.

---

### Task 1: The dependency and an empty database that opens

**Files:**
- Modify: `Cargo.toml` (workspace)
- Modify: `crates/catalog/Cargo.toml`
- Create: `crates/catalog/src/db.rs`
- Modify: `crates/catalog/src/lib.rs`

- [ ] **Step 1: Add the dependency**

In the workspace `Cargo.toml`, under `[workspace.dependencies]`:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
```

In `crates/catalog/Cargo.toml`, under `[dependencies]`:

```toml
rusqlite = { workspace = true }
```

⚠ `bundled` compiles SQLite from source. That is a C dependency, but it needs **no system library, no pkg-config entry and no CI change** — strictly fewer build prerequisites than libraw already imposes.

- [ ] **Step 2: Write the failing test**

Create `crates/catalog/src/db.rs`:

```rust
//! The catalog database.
//!
//! ⚠⚠ **Edits do not live here.** They stay in the per-photo JSON sidecars, so they travel with
//! the photograph and survive this application entirely. What lives here is *library* metadata —
//! which folders you have opened, which photos they hold, and (plan 2) collections. The property
//! that split preserves: **delete the catalog and you lose organisation, never work.**
//!
//! ⚠ One catalog, in the config directory, never named or chosen by the user. A catalog per
//! folder would make "show me every 5-star frame this year" impossible across three shoots, which
//! is the single thing a DAM exists for.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_in_memory_catalog_opens_and_reports_its_version() {
        let cat = Catalog::open_in_memory().expect("should open");
        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
    }
}
```

- [ ] **Step 3: Run to verify it fails**

```bash
cd ~/Projects/rustroom
cargo test -p catalog --lib an_in_memory_catalog_opens 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find type Catalog in this scope`.

- [ ] **Step 4: Implement**

Insert above the tests in `db.rs`:

```rust
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// The schema this build expects. Bump it whenever `migrate` gains a step.
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
        let conn = Connection::open(path)?;
        Self::prepare(conn)
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
        // this, deleting a folder would leave orphaned photo rows and (in plan 2) orphaned
        // collection memberships pointing at nothing.
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
}
```

⚠ `collections` and `collection_photos` are created now although plan 2 uses them. Creating all four in one migration means plan 2 needs no schema change at all — and a migration that has already run on a user's machine cannot be edited afterwards.

- [ ] **Step 5: Declare the module and add `dirs`**

In `crates/catalog/src/lib.rs`, at the top:

```rust
pub mod db;
```

In `crates/catalog/Cargo.toml`, under `[dependencies]`:

```toml
dirs = { workspace = true }
```

- [ ] **Step 6: Run**

```bash
cargo test -p catalog --lib an_in_memory_catalog_opens 2>&1 | grep -E "^test result"
```

Expected: `ok. 1 passed`. ⚠ The first build compiles SQLite and takes a minute or two.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/catalog/Cargo.toml crates/catalog/src/db.rs crates/catalog/src/lib.rs
git commit -m "feat(catalog): a versioned SQLite catalog that opens and migrates"
```

---

### Task 2: Migration is idempotent and forward-only

**Files:**
- Modify: `crates/catalog/src/db.rs`

- [ ] **Step 1: Write the failing tests**

Add to `db.rs`'s `mod tests`:

```rust
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
                "rustroom_catalog_{tag}_{}_{}",
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

    #[test]
    fn opening_an_existing_catalog_does_not_migrate_it_again() {
        // ⚠⚠ The guard against the most destructive shape a migration bug can take: a `CREATE
        // TABLE` that runs a second time either errors or, if written with `IF NOT EXISTS`,
        // quietly masks a version mismatch. Opening twice must be a no-op the second time.
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
        // would destroy whatever that version added. `migrate` is forward-only by construction:
        // every step is gated on `from < n`.
        let db = TempDb::new("future");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.conn
                .execute_batch("PRAGMA user_version = 99;")
                .unwrap();
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
        // would leave photo rows pointing at nothing — and in plan 2, collection memberships
        // pointing at those.
        let cat = Catalog::open_in_memory().unwrap();
        let on: i64 = cat
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(on, 1);
    }
```

- [ ] **Step 2: Run to verify they pass**

```bash
cargo test -p catalog --lib db:: 2>&1 | grep -E "^test result"
```

Expected: `ok. 5 passed`. These pass against Task 1's implementation — they are regression guards on properties it already has, which is why they are their own task rather than bundled in.

- [ ] **Step 3: Sabotage**

Use a helper that checks compilation first:

```bash
cat > /tmp/sab.py <<'PY'
import subprocess, shutil
def run(label, path, old, new, expect, target):
    bak = path + ".bak"; shutil.copy(path, bak)
    s = open(path).read(); assert old in s, f"{label}: pattern not found"
    open(path,"w").write(s.replace(old, new, 1))
    c = subprocess.run(["cargo","test",*target],capture_output=True,text=True)
    out = c.stdout + c.stderr
    compiled = "error[E" not in out and "could not compile" not in out
    hits = [l for l in out.splitlines() if "FAILED" in l and expect in l]
    print(f"{label}: " + ("SABOTAGE DID NOT COMPILE — meaningless" if not compiled
          else "CAUGHT" if hits else "NOT CAUGHT"))
    shutil.move(bak, path); subprocess.run(["touch", path])
PY
```

⚠⚠ **Confirm the harness works before trusting a clean run.** A matcher that greps failure lines by *bare* test name finds nothing, because cargo prints module paths — that made all seven sabotages in an earlier round report "NOT CAUGHT" when every one was caught. The helper above matches by substring for that reason.

Sabotages:
1. Remove the `if from < 1` gate so `migrate` always runs → expect `opening_an_existing_catalog_does_not_migrate_it_again` **CAUGHT**.
2. Change the gate to `if from != 1` → expect `a_catalog_from_the_future_is_left_alone` **CAUGHT**.
3. Delete the `PRAGMA foreign_keys = ON;` line → expect `foreign_keys_are_enforced` **CAUGHT**.

- [ ] **Step 4: Commit**

```bash
git add crates/catalog/src/db.rs
git commit -m "test(catalog): pin that migration is idempotent and forward-only"
```

---

### Task 3: Recording a folder and its photos

**Files:**
- Modify: `crates/catalog/src/db.rs`

- [ ] **Step 1: Write the failing tests**

Add to `db.rs`'s `mod tests`:

```rust
    fn entry(name: &str) -> CatalogEntry {
        CatalogEntry {
            path: PathBuf::from(format!("/x/{name}")),
            file_name: name.to_string(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            edited: None,
        }
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
        // them — and collections do not exist until plan 2, so nothing would catch it. A scan
        // adds and updates; it never removes.
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
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p catalog --lib syncing_a_folder 2>&1 | grep -E "^error" | head -3
```

Expected: `no method named sync_folder`.

- [ ] **Step 3: Implement**

Add to `db.rs`, inside `impl Catalog`:

```rust
    /// Record `folder` and every photo in `files`, adding what is new and refreshing what is not.
    ///
    /// ⚠⚠ **This never deletes a photo row.** A folder on an unplugged drive scans as empty, and
    /// removing rows then would silently empty every collection referencing them. `last_seen`
    /// records what was present; absence is not deletion.
    ///
    /// ⚠ One transaction. Half a folder recorded is worse than none, and it is also much slower:
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
```

And at module level in `db.rs`:

```rust
use crate::CatalogEntry;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
```

- [ ] **Step 4: Run**

```bash
cargo test -p catalog --lib db:: 2>&1 | grep -E "^test result"
```

Expected: `ok. 9 passed`.

- [ ] **Step 5: Sabotage**

1. Change the photos `ON CONFLICT ... DO UPDATE` to `DO NOTHING` and the `INSERT` to plain `INSERT OR IGNORE`, then add a second unconditional `INSERT` → expect `syncing_the_same_folder_twice_does_not_duplicate_it` **CAUGHT**. (Simpler equivalent: drop `UNIQUE(folder_id, file_name, copy)` from the migration and delete the `ON CONFLICT` clause.)
2. Add `DELETE FROM photos WHERE folder_id = ?1;` before the insert loop → expect `a_folder_whose_files_have_vanished_keeps_its_rows` **CAUGHT**.
3. Change the `photos` UNIQUE constraint to `UNIQUE(file_name, copy)` → expect `two_folders_with_a_same_named_file_are_two_photos` **CAUGHT**.

- [ ] **Step 6: Commit**

```bash
git add crates/catalog/src/db.rs
git commit -m "feat(catalog): record folders and photos, adding without ever deleting"
```

---

### Task 4: Renames and reindexing keep the row

**Files:**
- Modify: `crates/catalog/src/db.rs`

⚠⚠ **This is the highest-risk part of the milestone.** Rustroom renames files itself and `Reindex` renumbers copies; both must move the catalog row in the same operation, or plan 2's collections silently lose members. It is the same class as the rule virtual copies needed twice — *the stash and the log must let go together*.

- [ ] **Step 1: Write the failing tests**

Add to `db.rs`'s `mod tests`:

```rust
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
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p catalog --lib renaming_a_file 2>&1 | grep -E "^error" | head -3
```

Expected: `no method named photo_id`.

- [ ] **Step 3: Implement**

Add to `db.rs`, inside `impl Catalog`:

```rust
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
    /// ⚠ Copies never come from a folder scan — they live in the sidecar — so they are added
    /// here explicitly when one is created.
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
    /// ⚠ A photo the catalog has never seen is not an error. The catalog is not authoritative
    /// and must never be a reason a rename fails.
    pub fn rename_photo(&self, from: &Path, to: &Path) -> Result<(), CatalogError> {
        let (Some((from_folder, from_name)), Some((to_folder, to_name))) = (split(from), split(to))
        else {
            return Ok(());
        };
        self.conn.execute(
            "UPDATE photos SET file_name = ?4
             WHERE file_name = ?2
               AND folder_id = (SELECT id FROM folders WHERE path = ?1)
               AND ?1 = ?3",
            rusqlite::params![from_folder, from_name, to_folder, to_name],
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
```

And at module level in `db.rs`:

```rust
/// A photo path as `(folder, file name)`, the pair the schema keys on.
fn split(photo: &Path) -> Option<(String, String)> {
    let folder = photo.parent()?.to_string_lossy().to_string();
    let name = photo.file_name()?.to_string_lossy().to_string();
    Some((folder, name))
}
```

⚠ `rename_photo`'s `AND ?1 = ?3` restricts it to renames **within one folder**, which is all `file_rename::target` can produce — it builds the new path with `with_file_name`. A cross-folder move is a different operation and is not supported here.

- [ ] **Step 4: Run**

```bash
cargo test -p catalog --lib db:: 2>&1 | grep -E "^test result"
```

Expected: `ok. 13 passed`.

- [ ] **Step 5: Sabotage**

1. Add `AND copy = 0` to `rename_photo`'s `UPDATE` → expect `renaming_moves_every_copy_of_the_file` **CAUGHT**.
2. Make `rename_photo` an `INSERT` of a new row instead of an `UPDATE` → expect `renaming_a_file_moves_its_row_rather_than_adding_one` **CAUGHT**.
3. Make `renumber_copy` a no-op (`return Ok(());` first line) → expect `renumbering_a_copy_moves_its_row` **CAUGHT**.

- [ ] **Step 6: Commit**

```bash
git add crates/catalog/src/db.rs
git commit -m "feat(catalog): follow renames and copy renumbering"
```

---

### Task 5: Wire it into the app

**Files:**
- Modify: `crates/app/src/main.rs`

⚠ The catalog is opened once and held on `RustroomApp`. Every call site **degrades to doing nothing** when it is absent: losing the catalog loses organisation, not work, so a database failure must never take the app down or block a rename.

- [ ] **Step 1: Add the field**

In `crates/app/src/main.rs`, add to `struct RustroomApp`:

```rust
    /// The one catalog. `None` when it could not be opened.
    ///
    /// ⚠⚠ **Invisible by design.** Nothing in the UI mentions it; it exists so that plan 2's
    /// collections — and later ratings and search — have somewhere to live. If you can tell it
    /// shipped, something is wrong.
    ///
    /// ⚠ `Option` rather than a hard failure: a catalog that will not open must not stop you
    /// editing photographs, because edits live in the sidecars and do not need it.
    catalog: Option<catalog::db::Catalog>,
```

In `RustroomApp::new`, before the `Self { .. }` literal:

```rust
        // ⚠ Opened once, at startup. A failure is logged to the status line rather than fatal.
        let catalog = match catalog::db::Catalog::open(&catalog::db::Catalog::user_default_path()) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("catalog unavailable: {e}");
                None
            }
        };
```

and add `catalog,` to the literal.

In `crates/app/src/test_app.rs`, add to the exhaustive literal in `for_test`:

```rust
        // ⚠ In memory, so tests never touch the user's real catalog.
        catalog: catalog::db::Catalog::open_in_memory().ok(),
```

- [ ] **Step 2: Sync on folder open**

In `open_folder`, immediately after the sorted `scan` is obtained and before the `Library` is built:

```rust
                // ⚠ After the sort, before the entries are expanded: the catalog records the
                // *files*, and copies are added when they are created rather than scanned.
                if let Some(cat) = &self.catalog {
                    if let Err(e) = cat.sync_folder(&dir, &scan) {
                        eprintln!("catalog: {e}");
                    }
                }
```

- [ ] **Step 3: Follow a rename**

In `rename_file`, immediately after the sidecar has been moved and before `self.cache.rekey(from, to);`:

```rust
        // ⚠⚠ In the same operation as the file move. A catalog row and the file it names are two
        // facts that must move as one — the same rule the history stash and the action log needed
        // twice. Failure is logged, never fatal: the file has already moved.
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.rename_photo(from, to) {
                eprintln!("catalog: {e}");
            }
        }
```

- [ ] **Step 4: Follow a new copy, a deleted copy and a renumbering**

In `create_virtual_copy`, immediately after `sidecar::persist_copy` succeeds and before the entries are rebuilt:

```rust
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.add_copy(&path, n) {
                eprintln!("catalog: {e}");
            }
        }
```

⚠ Use whatever local names `create_virtual_copy` already has for the photo's path and the new copy number; the surrounding code names them when it calls `persist_copy`.

In `reindex`, inside the `for (from, _) in moves` loop that already forgets the history — extend it so each move is followed:

```rust
            for (from, to) in moves {
                let old = PhotoId::new(e.path.clone(), from);
                self.histories.forget(&old);
                self.actions.forget_edits_of(&old);
                // ⚠ The catalog follows the same renumbering, or its row for the old number
                // becomes unreachable while the new one looks empty.
                if let Some(cat) = &self.catalog {
                    if let Err(err) = cat.renumber_copy(&e.path, from, to) {
                        eprintln!("catalog: {err}");
                    }
                }
            }
```

⚠ The existing loop binds `(from, _)`; it must become `(from, to)`.

- [ ] **Step 5: Verify**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy: $?"
cargo test --all > /tmp/t.log 2>&1; echo "test: $?"
```

Expected: both `0`.

⚠ **Then run the GUI and confirm nothing changed.** That is the acceptance criterion for this plan:

```bash
cargo run --release -p app
```

Open a folder, rename a photo, make a copy, hit Reindex. Everything must behave exactly as before. Then confirm the catalog was written:

```bash
ls -la ~/Library/Application\ Support/rustroom/catalog.db
```

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/main.rs crates/app/src/test_app.rs
git commit -m "feat(app): open the catalog and keep it in step with the library"
```

---

### Task 6: Record it

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add a section**

Follow the established format — what it does, the ⚠ findings, accepted caveats. Include specifically:

- **Edits stay in sidecars; only library metadata is in the catalog.** The property that preserves: delete the catalog and you lose organisation, never work.
- ⚠⚠ **One invisible catalog, and why a catalog *per folder* was rejected**: three shoots in three folders would be three databases, making "every 5-star frame this year" impossible — the one thing a DAM exists for, defeated by the default behaviour.
- ⚠ **A scan never deletes a photo row**, because an unplugged drive scans as empty.
- ⚠ **Renames and `Reindex` update the catalog in the same operation** — the same rule the history stash and the action log needed twice.
- ⚠ `size`/`mtime` are recorded but unread, for a future re-link.
- ⚠ **`bundled` SQLite** — a C compile, but no system library and no CI change.
- **Accepted caveats** from the spec: accumulation, no re-linking, no import step, one catalog, and that this plan is invisible.

- [ ] **Step 2: Update the state line**

In the RESUME block, update the test count and note that Phase 4 has started.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the catalog foundation"
```

---

## Self-review

**Spec coverage:** schema → Task 1; migration idempotent/forward-only → Task 2; folder and photo recording, never deleting → Task 3; rename and renumber integration → Tasks 4 and 5; `size`/`mtime` recorded → Task 3's `file_facts`; catalog location via `dirs` → Task 1; invisibility → Task 5's GUI check; collections tables → created in Task 1's migration, used in plan 2.

**Placeholders:** none — every step carries the code or the exact command. Task 5 Step 4 defers to local names that already exist in the surrounding function rather than inventing them, and says so.

**Type consistency:** `Catalog::{open, open_in_memory, user_default_path, version, sync_folder, photos_in, folder_count, photo_count, photo_id, add_copy, rename_photo, renumber_copy}` are defined in Tasks 1, 3 and 4 and used in Tasks 2–5. `CatalogError`, `SCHEMA_VERSION`, `split`, `unix_now` and `file_facts` are defined in Tasks 1, 3 and 4. `CatalogEntry` is the existing type from `catalog::lib`, with the `edited` field added during the browser work.

⚠ **One known risk, flagged rather than hidden:** Task 5's edits land in `main.rs` at sites described by their surrounding code rather than by line number, because that file has moved twice in recent work. If a described anchor is not found, read the function before editing — do not guess.
