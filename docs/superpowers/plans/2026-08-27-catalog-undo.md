# Catalog undo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make all four destructive catalog actions undoable with Ctrl+Z, and confirm a keyword deletion that would take a subtree with it.

**Architecture:** Each delete first `SELECT`s everything its cascade is about to remove into a `CatalogSnapshot`, which rides `ActionKind::CatalogDelete`. Undo replays the snapshot in dependency order with **fresh ids**, rewriting each child row's reference from an old→new map — no ids are preserved, because a cascade makes the snapshot closed under references and reuse of a freed id is a proven collision.

**Tech Stack:** Rust, `rusqlite` (transactions, recursive CTEs), the existing `ActionLog`.

**Read first:** `docs/superpowers/specs/2026-08-27-catalog-undo-design.md`, above all "The measurements that reframed the problem".

**Before you start:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **No `Co-Authored-By` trailer. Never push. Never merge unprompted.** Branch: `catalog-undo`.

⚠ **Check clippy's exit code before every commit** — six commits this week went in with clippy red because a green `cargo test` was mistaken for evidence about it.

⚠⚠ **NO SCHEMA MIGRATION.** This adds no table, no column and no index; the catalog stays at `user_version = 5`. If you find yourself writing one, the design has been misread — the whole point of remapping instead of preserving ids is that no migration is needed.

### The tables, verified

| table | columns |
|---|---|
| `folders` | `id`, `path` |
| `photos` | `id`, `folder_id`, `file_name`, `copy`, `size`, `mtime`, `last_seen`, `rating`, `flag`, `label` |
| `collections` | `id`, `name`, `created` |
| `collection_photos` | `collection_id`, `photo_id`, `added` |
| `keywords` | `id`, `name`, `parent_id` |
| `photo_keywords` | `folder_id`, `file_name`, `keyword_id` |
| `photo_capture` | `folder_id`, `file_name`, `taken`, `camera`, `lens`, `iso`, `shutter`, `aperture`, `focal_len` |
| `smart_collections` | `id`, `name`, `rule`, `created` |

---

### Task 1: The snapshot types

**Files:**
- Create: `crates/catalog/src/snapshot.rs`
- Modify: `crates/catalog/src/lib.rs` (declare the module)

⚠ In `catalog`, beside the schema it mirrors — the row shapes are that crate's business, and `app` should not be able to get them wrong.

- [ ] **Step 1: Write the types**

Create `crates/catalog/src/snapshot.rs`:

```rust
//! What a catalog deletion removed, in enough detail to put it back.
//!
//! ⚠⚠ **These are re-inserted with FRESH ids, not their original ones.** Probing SQLite showed
//! that explicit-rowid restore works *but collides*: without `AUTOINCREMENT` a freed id is handed
//! to the next insert, so deleting a collection, creating another, then undoing fails with
//! `UNIQUE constraint failed`. What must survive a restore is the **relationships**, not the ids —
//! and because a cascade removes exactly the dependent rows, a snapshot is *closed under
//! references*, so remapping is total. The alternative was an `AUTOINCREMENT` migration
//! recreating four tables on live user data.
//!
//! ⚠ The ids here are therefore the **old** ones, kept only so the child rows can be matched to
//! their parents while remapping.

use std::path::PathBuf;

/// One `photos` row, with its marks.
///
/// ⚠ The marks are the point. Forgetting a folder is the only action that destroys metadata the
/// user made by hand, and a restore that dropped the ratings would look like it had worked.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoSnapshot {
    pub id: i64,
    pub file_name: String,
    pub copy: u32,
    pub size: Option<i64>,
    pub mtime: Option<i64>,
    pub last_seen: i64,
    pub rating: u8,
    pub flag: i64,
    pub label: u8,
}

/// One `photo_capture` row.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureSnapshot {
    pub file_name: String,
    pub taken: Option<i64>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub shutter: Option<f64>,
    pub aperture: Option<f64>,
    pub focal_len: Option<f64>,
}

/// One `keywords` row.
#[derive(Debug, Clone, PartialEq)]
pub struct KeywordSnapshot {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
}

/// What a deletion removed.
///
/// ⚠ One enum rather than four action kinds, so `ActionLog` gains a single variant and the restore
/// is one function with one transaction.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogSnapshot {
    Smart {
        name: String,
        rule: String,
        created: i64,
    },
    Collection {
        name: String,
        created: i64,
        /// `(photo_id, added)`. ⚠ These photo ids are **not** remapped — the photos still exist.
        /// A membership whose photo has since gone is skipped on restore.
        members: Vec<(i64, i64)>,
    },
    Keyword {
        /// The subtree, **parents before children**, so a restore can remap `parent_id` to an id
        /// that already exists.
        tree: Vec<KeywordSnapshot>,
        /// `(folder_id, file_name, keyword_id)` for every assignment of any keyword in `tree`.
        assignments: Vec<(i64, String, i64)>,
    },
    Folder {
        path: PathBuf,
        photos: Vec<PhotoSnapshot>,
        captures: Vec<CaptureSnapshot>,
        /// `(collection_id, photo_id, added)` — the photo id is remapped, the collection id is not.
        memberships: Vec<(i64, i64, i64)>,
        /// `(file_name, keyword_id)`; the folder id is remapped.
        keywords: Vec<(String, i64)>,
    },
}

impl CatalogSnapshot {
    /// What to say in the status line after an undo.
    pub fn label(&self) -> String {
        match self {
            Self::Smart { name, .. } => format!("Restored smart collection “{name}”"),
            Self::Collection { name, .. } => format!("Restored collection “{name}”"),
            Self::Keyword { tree, .. } => match tree.first() {
                Some(k) => format!("Restored keyword “{}”", k.name),
                None => "Restored keyword".to_string(),
            },
            Self::Folder { path, photos, .. } => format!(
                "Restored {} with {} photos",
                path.display(),
                photos.len()
            ),
        }
    }
}
```

- [ ] **Step 2: Declare it**

In `crates/catalog/src/lib.rs`, beside the other modules:

```rust
pub mod snapshot;
```

- [ ] **Step 3: Build**

Run: `cargo build -p catalog 2>&1 | tail -5`
Expected: clean. ⚠ `catalog` is a **library** crate, so nothing here is dead code.

- [ ] **Step 4: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add crates/catalog/src/snapshot.rs crates/catalog/src/lib.rs
git commit -m "feat(catalog): snapshot types for an undoable deletion"
```

---

### Task 2: Capture the snapshot, and restore it

**Files:**
- Modify: `crates/catalog/src/db.rs` (the four delete fns, around lines 612, 744, 944, 1079)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/catalog/src/db.rs`. ⚠ Read the existing tests first and reuse their fixture helpers rather than writing new ones — they already build an in-memory catalog with folders and photos.

```rust
    /// ⚠⚠ **The collision that motivated the whole design.** Delete a collection, create another
    /// (which takes the freed id, because SQLite reuses `max + 1` without `AUTOINCREMENT`), then
    /// undo. Preserving the original id fails here with `UNIQUE constraint failed`; remapping
    /// cannot.
    #[test]
    fn a_restore_survives_its_id_being_taken_by_something_else() {
        let c = Catalog::open_in_memory().unwrap();
        let id = c.create_collection("Portfolio").unwrap();
        let snap = c.delete_collection(id).unwrap();
        let other = c.create_collection("Travel").unwrap();
        assert_eq!(other, id, "fixture check: the freed id really was reused");

        c.restore(&snap).unwrap();
        let names: Vec<String> = c.collections().unwrap().into_iter().map(|r| r.name).collect();
        assert!(names.contains(&"Portfolio".to_string()));
        assert!(names.contains(&"Travel".to_string()));
    }

    /// ⚠ The marks are the point of a folder restore: they are the only thing a forget destroys
    /// that the user made by hand, and a restore that dropped them would look like it worked.
    #[test]
    fn forgetting_a_folder_and_undoing_brings_back_its_marks() {
        let (c, dir) = catalog_with_two_photos();
        let photo = dir.join("a.RAF");
        c.set_marks(&photo, Marks { rating: 4, flag: Flag::Pick, label: 2 })
            .unwrap();
        let fid = c.folders().unwrap()[0].id;

        let snap = c.forget_folder(fid).unwrap();
        assert_eq!(c.photo_count().unwrap(), 0);

        c.restore(&snap).unwrap();
        assert_eq!(c.photo_count().unwrap(), 2);
        let back = c.marks_in(&dir).unwrap();
        let m = back
            .values()
            .find(|m| m.rating == 4)
            .expect("the rating must come back");
        assert_eq!(m.flag, Flag::Pick);
        assert_eq!(m.label, 2);
    }

    /// ⚠ A keyword takes its whole subtree, so the restore must bring the hierarchy back — not
    /// just the rows, but the parent links, which are remapped.
    #[test]
    fn undoing_a_keyword_deletion_restores_the_subtree_and_its_shape() {
        let c = Catalog::open_in_memory().unwrap();
        let europe = c.create_keyword("Europe", None).unwrap();
        let france = c.create_keyword("France", Some(europe)).unwrap();
        let _paris = c.create_keyword("Paris", Some(france)).unwrap();

        let snap = c.delete_keyword(europe).unwrap();
        assert!(c.keywords().unwrap().is_empty());

        c.restore(&snap).unwrap();
        let rows = c.keywords().unwrap();
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
        let (c, dir) = catalog_with_two_photos();
        let coll = c.create_collection("Portfolio").unwrap();
        c.add_to_collection(coll, &dir.join("a.RAF"), 0).unwrap();
        let fid = c.folders().unwrap()[0].id;

        let snap = c.forget_folder(fid).unwrap();
        // The collection those memberships pointed at is deleted before the undo.
        c.delete_collection(coll).unwrap();

        c.restore(&snap).unwrap();
        assert_eq!(c.photo_count().unwrap(), 2, "the photos still come back");
    }

    #[test]
    fn undoing_a_smart_collection_deletion_restores_its_rule() {
        let c = Catalog::open_in_memory().unwrap();
        let id = c.create_smart("Picks", "{\"All\":[]}").unwrap();
        let snap = c.delete_smart(id).unwrap();
        assert!(c.smart_collections().unwrap().is_empty());

        c.restore(&snap).unwrap();
        let rows = c.smart_collections().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Picks");
        assert_eq!(rows[0].rule, "{\"All\":[]}");
    }

    /// ⚠ A collection's members must come back, not just its name.
    #[test]
    fn undoing_a_collection_deletion_restores_its_members() {
        let (c, dir) = catalog_with_two_photos();
        let coll = c.create_collection("Portfolio").unwrap();
        c.add_to_collection(coll, &dir.join("a.RAF"), 0).unwrap();

        let snap = c.delete_collection(coll).unwrap();
        c.restore(&snap).unwrap();

        let row = c.collections().unwrap().into_iter().next().unwrap();
        assert_eq!(row.name, "Portfolio");
        assert_eq!(row.count, 1, "the membership came back too");
    }
```

⚠ **Check the real names of every helper before running**: `catalog_with_two_photos`,
`create_collection`, `create_keyword`, `create_smart`, `smart_collections`, `marks_in`,
`photo_count`, `set_marks`, `add_to_collection`. Read the existing `mod tests` and adapt — the
signatures matter more than these names.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p catalog restore 2>&1 | tail -20`
Expected: FAIL to compile — the four delete fns still return `Result<(), _>` and `restore` does not
exist.

- [ ] **Step 3: Make the deletes return their snapshot**

In `crates/catalog/src/db.rs`, change each signature to return the snapshot, collecting it
**before** the `DELETE`:

```rust
    /// Delete a smart collection, returning what was removed so it can be restored.
    pub fn delete_smart(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let snap = self.conn.query_row(
            "SELECT name, rule, created FROM smart_collections WHERE id = ?1",
            [id],
            |r| {
                Ok(CatalogSnapshot::Smart {
                    name: r.get(0)?,
                    rule: r.get(1)?,
                    created: r.get(2)?,
                })
            },
        )?;
        self.conn
            .execute("DELETE FROM smart_collections WHERE id = ?1", [id])?;
        Ok(snap)
    }
```

```rust
    /// Delete a collection, returning what was removed. Photos are untouched.
    pub fn delete_collection(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let (name, created) = self.conn.query_row(
            "SELECT name, created FROM collections WHERE id = ?1",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )?;
        let mut stmt = self
            .conn
            .prepare("SELECT photo_id, added FROM collection_photos WHERE collection_id = ?1")?;
        let members = stmt
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);
        self.conn
            .execute("DELETE FROM collections WHERE id = ?1", [id])?;
        Ok(CatalogSnapshot::Collection {
            name,
            created,
            members,
        })
    }
```

```rust
    /// Delete a keyword **and its whole subtree**, returning what was removed.
    ///
    /// ⚠ The tree comes back parents-first (`ORDER BY depth`), which is what lets a restore remap
    /// each `parent_id` to an id that already exists.
    pub fn delete_keyword(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE sub(id, name, parent_id, depth) AS (
                 SELECT id, name, parent_id, 0 FROM keywords WHERE id = ?1
                 UNION ALL
                 SELECT k.id, k.name, k.parent_id, s.depth + 1
                 FROM keywords k JOIN sub s ON k.parent_id = s.id
             )
             SELECT id, name, parent_id FROM sub ORDER BY depth",
        )?;
        let tree: Vec<KeywordSnapshot> = stmt
            .query_map([id], |r| {
                Ok(KeywordSnapshot {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent_id: r.get(2)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);

        let ids: Vec<i64> = tree.iter().map(|k| k.id).collect();
        let mut assignments = Vec::new();
        for kid in &ids {
            let mut stmt = self.conn.prepare(
                "SELECT folder_id, file_name, keyword_id FROM photo_keywords WHERE keyword_id = ?1",
            )?;
            assignments.extend(
                stmt.query_map([kid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .filter_map(Result::ok),
            );
        }
        self.conn
            .execute("DELETE FROM keywords WHERE id = ?1", [id])?;
        Ok(CatalogSnapshot::Keyword { tree, assignments })
    }
```

```rust
    /// Forget a folder, returning everything the cascade removed.
    ///
    /// ⚠ This is the only action that destroys metadata the user made by hand, which is why the
    /// photo rows carry their marks.
    pub fn forget_folder(&self, id: i64) -> Result<CatalogSnapshot, CatalogError> {
        let path: PathBuf = self.conn.query_row(
            "SELECT path FROM folders WHERE id = ?1",
            [id],
            |r| Ok(PathBuf::from(r.get::<_, String>(0)?)),
        )?;

        let mut stmt = self.conn.prepare(
            "SELECT id, file_name, copy, size, mtime, last_seen, rating, flag, label
             FROM photos WHERE folder_id = ?1",
        )?;
        let photos: Vec<PhotoSnapshot> = stmt
            .query_map([id], |r| {
                Ok(PhotoSnapshot {
                    id: r.get(0)?,
                    file_name: r.get(1)?,
                    copy: r.get(2)?,
                    size: r.get(3)?,
                    mtime: r.get(4)?,
                    last_seen: r.get(5)?,
                    rating: r.get(6)?,
                    flag: r.get(7)?,
                    label: r.get(8)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);

        let mut stmt = self.conn.prepare(
            "SELECT file_name, taken, camera, lens, iso, shutter, aperture, focal_len
             FROM photo_capture WHERE folder_id = ?1",
        )?;
        let captures: Vec<CaptureSnapshot> = stmt
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
        drop(stmt);

        let mut stmt = self.conn.prepare(
            "SELECT cp.collection_id, cp.photo_id, cp.added
             FROM collection_photos cp JOIN photos p ON p.id = cp.photo_id
             WHERE p.folder_id = ?1",
        )?;
        let memberships: Vec<(i64, i64, i64)> = stmt
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);

        let mut stmt = self
            .conn
            .prepare("SELECT file_name, keyword_id FROM photo_keywords WHERE folder_id = ?1")?;
        let keywords: Vec<(String, i64)> = stmt
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);

        self.conn
            .execute("DELETE FROM folders WHERE id = ?1", [id])?;
        Ok(CatalogSnapshot::Folder {
            path,
            photos,
            captures,
            memberships,
            keywords,
        })
    }
```

⚠ `self.conn` is not `&mut`, so `prepare` borrows must be **dropped** before the `execute` —
hence the explicit `drop(stmt)` calls. Do not remove them.

- [ ] **Step 4: Write `restore`**

Add to `impl Catalog`:

```rust
    /// Put back what a deletion removed.
    ///
    /// ⚠⚠ **Fresh ids, with every child reference remapped** — never the original ids. SQLite
    /// reuses a freed id for the next insert, so a preserved id collides the moment anything else
    /// was created before the undo, which is a case a user hits by simply carrying on working.
    /// A cascade removes exactly the dependent rows, so the snapshot is closed under references
    /// and the remap is total.
    ///
    /// ⚠ **One transaction**, so a partial failure leaves the catalog exactly as it was.
    ///
    /// ⚠ **A row that can no longer be satisfied is skipped, not fatal** — a membership whose
    /// collection has since gone, an assignment whose keyword has. The restore puts back
    /// everything it still can: the catalog is not authoritative and must never be a reason an
    /// action fails.
    pub fn restore(&self, snap: &CatalogSnapshot) -> Result<(), CatalogError> {
        let tx = self.conn.unchecked_transaction()?;
        match snap {
            CatalogSnapshot::Smart { name, rule, created } => {
                tx.execute(
                    "INSERT INTO smart_collections (name, rule, created) VALUES (?1, ?2, ?3)",
                    rusqlite::params![name, rule, created],
                )?;
            }
            CatalogSnapshot::Collection { name, created, members } => {
                tx.execute(
                    "INSERT INTO collections (name, created) VALUES (?1, ?2)",
                    rusqlite::params![name, created],
                )?;
                let new_id = tx.last_insert_rowid();
                for (photo_id, added) in members {
                    // ⚠ Skipped, not fatal: the photo may have been forgotten meanwhile.
                    let _ = tx.execute(
                        "INSERT OR IGNORE INTO collection_photos (collection_id, photo_id, added)
                         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM photos WHERE id = ?2)",
                        rusqlite::params![new_id, photo_id, added],
                    );
                }
            }
            CatalogSnapshot::Keyword { tree, assignments } => {
                let mut map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
                // ⚠ Parents first — the snapshot is ordered by depth, so a parent's new id is
                // always in `map` by the time a child needs it.
                for k in tree {
                    let parent = k.parent_id.and_then(|p| map.get(&p).copied()).or(k.parent_id
                        .filter(|p| !tree.iter().any(|t| t.id == *p)));
                    tx.execute(
                        "INSERT INTO keywords (name, parent_id) VALUES (?1, ?2)",
                        rusqlite::params![k.name, parent],
                    )?;
                    map.insert(k.id, tx.last_insert_rowid());
                }
                for (folder_id, file_name, keyword_id) in assignments {
                    let Some(new_kid) = map.get(keyword_id) else {
                        continue;
                    };
                    let _ = tx.execute(
                        "INSERT OR IGNORE INTO photo_keywords (folder_id, file_name, keyword_id)
                         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM folders WHERE id = ?1)",
                        rusqlite::params![folder_id, file_name, new_kid],
                    );
                }
            }
            CatalogSnapshot::Folder { path, photos, captures, memberships, keywords } => {
                tx.execute(
                    "INSERT INTO folders (path) VALUES (?1)",
                    [path.to_string_lossy()],
                )?;
                let new_folder = tx.last_insert_rowid();
                let mut map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
                for p in photos {
                    tx.execute(
                        "INSERT INTO photos
                         (folder_id, file_name, copy, size, mtime, last_seen, rating, flag, label)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        rusqlite::params![
                            new_folder, p.file_name, p.copy, p.size, p.mtime, p.last_seen,
                            p.rating, p.flag, p.label
                        ],
                    )?;
                    map.insert(p.id, tx.last_insert_rowid());
                }
                for c in captures {
                    tx.execute(
                        "INSERT OR REPLACE INTO photo_capture
                         (folder_id, file_name, taken, camera, lens, iso, shutter, aperture, focal_len)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        rusqlite::params![
                            new_folder, c.file_name, c.taken, c.camera, c.lens, c.iso,
                            c.shutter, c.aperture, c.focal_len
                        ],
                    )?;
                }
                for (collection_id, photo_id, added) in memberships {
                    let Some(new_pid) = map.get(photo_id) else {
                        continue;
                    };
                    // ⚠ Skipped, not fatal: the collection may have been deleted meanwhile.
                    let _ = tx.execute(
                        "INSERT OR IGNORE INTO collection_photos (collection_id, photo_id, added)
                         SELECT ?1, ?2, ?3
                         WHERE EXISTS (SELECT 1 FROM collections WHERE id = ?1)",
                        rusqlite::params![collection_id, new_pid, added],
                    );
                }
                for (file_name, keyword_id) in keywords {
                    let _ = tx.execute(
                        "INSERT OR IGNORE INTO photo_keywords (folder_id, file_name, keyword_id)
                         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM keywords WHERE id = ?3)",
                        rusqlite::params![new_folder, file_name, keyword_id],
                    );
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
```

⚠ **Check `unchecked_transaction` is available** on this `rusqlite` version — `self.conn` is behind
`&self`, so `transaction()` (which needs `&mut`) will not compile. If it is absent, use explicit
`BEGIN`/`COMMIT` via `execute_batch` and say so in a comment.

⚠ The keyword `parent` expression is deliberately convoluted: a subtree's **root** may have a
parent *outside* the deleted set, which still exists and must be kept; a child's parent is inside
and must be remapped. Simplify it only if the shape test still passes.

- [ ] **Step 5: Fix the four call sites**

`crates/app/src/collections_panel.rs` lines ~552, ~609, ~645 and `crates/app/src/main.rs` ~2410 all
discard the result with `let _ =`. They will still compile — ⚠ **which is the danger**: the
snapshot would be silently thrown away and the feature would be absent with a green build. Task 3
wires them up; for now just confirm the workspace builds.

Run: `cargo test -p catalog 2>&1 | tail -5`
Expected: PASS, all six new tests.

- [ ] **Step 6: Sabotage**

Change the collection restore to use an explicit id (`INSERT INTO collections (id, name, created)`
with the snapshot's original id).
Run: `cargo test -p catalog a_restore_survives 2>&1 | tail -20`
Expected: **FAIL** with `UNIQUE constraint failed` — the exact collision the design exists to
avoid. ⚠ Confirm a `test result:` line appears, then restore.

Then remove the `WHERE EXISTS (SELECT 1 FROM collections ...)` guard from the folder restore.
Expected: **FAIL** on `a_membership_whose_collection_is_gone_is_skipped_and_the_rest_restores`.

- [ ] **Step 7: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add -A
git commit -m "feat(catalog): snapshot a deletion and restore it with remapped ids"
```

---

### Task 3: Wire it into the undo log

**Files:**
- Modify: `crates/app/src/action_log.rs` (the `ActionKind` enum and `reachable`)
- Modify: `crates/app/src/main.rs` (`undo_one`, `redo_one`, `forget_folder`)
- Modify: `crates/app/src/collections_panel.rs` (the three delete arms)

⚠⚠ **The call sites currently compile while discarding the snapshot.** That is the one way this
feature ships absent with everything green — check each of the four by reading, not by building.

- [ ] **Step 1: Add the action**

In `crates/app/src/action_log.rs`, add to `ActionKind` after `PresetDelete`:

```rust
    /// A catalog deletion — a collection, a smart collection, a keyword subtree, or a folder.
    ///
    /// ⚠ Carries the whole snapshot because nothing else holds it, and **boxed** for the reason
    /// `Sync` and `CopyDelete` are: an inline payload would make every entry in the log that
    /// large, including the zero-sized `PhotoEdit`.
    CatalogDelete {
        snapshot: Box<catalog::snapshot::CatalogSnapshot>,
    },
```

and extend `reachable`'s **global** arm, beside `CopyDelete` and `PresetDelete`:

```rust
        | ActionKind::CatalogDelete { .. }
```

⚠ **Global, not photo-scoped.** A catalog deletion is not about the photo on screen, so it must be
undoable from wherever you are — the same reasoning `PresetDelete` and `Sync` carry.

- [ ] **Step 2: Add the test**

Append to `mod tests` in `crates/app/src/action_log.rs`:

```rust
    /// ⚠ Reachable from anywhere, unlike a `PhotoEdit`: a catalog deletion is not about the photo
    /// you happen to be looking at.
    #[test]
    fn a_catalog_deletion_is_undoable_from_any_photo() {
        let mut log = ActionLog::default();
        log.push(
            ActionKind::CatalogDelete {
                snapshot: Box::new(catalog::snapshot::CatalogSnapshot::Smart {
                    name: "Picks".into(),
                    rule: "{}".into(),
                    created: 0,
                }),
            },
            a(),
        );
        assert!(
            log.undo(b()).is_some(),
            "a catalog deletion must be reachable from a different photo"
        );
    }
```

⚠ `a()` and `b()` are the existing helpers in that module; check their exact form.

- [ ] **Step 3: Build to find the exhaustive matches**

Run: `cargo build --workspace --all-targets 2>&1 | grep -E "E0004|not covered" | head`
Expected: `undo_one` and `redo_one` in `main.rs` do not cover the new variant. That is the
exhaustive match doing its job.

- [ ] **Step 4: Handle it in both directions**

In `crates/app/src/main.rs`, add to `undo_one`:

```rust
            ActionKind::CatalogDelete { snapshot } => {
                if let Some(cat) = &self.catalog {
                    match cat.restore(&snapshot) {
                        Ok(()) => self.status = snapshot.label(),
                        Err(e) => self.status = format!("Could not restore: {e}"),
                    }
                }
                // ⚠ The panel reads the catalog every frame, so it picks this up on its own; but
                // a Library showing the deleted source must not be reopened — `LibrarySource`
                // holds the **old** id, which the remap has invalidated. The view stays put.
            }
```

and to `redo_one`:

```rust
            ActionKind::CatalogDelete { snapshot } => {
                // ⚠ Re-deleting needs the *current* id, which the restore changed. Look it up by
                // name rather than trusting the snapshot's stale one.
                if let Some(cat) = &self.catalog {
                    let _ = redo_catalog_delete(cat, &snapshot);
                }
            }
```

with the helper beside it:

```rust
/// Re-apply a catalog deletion after an undo.
///
/// ⚠⚠ **Looks the target up by name, because the restore gave it a new id.** The snapshot's id is
/// the one it had before the *first* deletion and refers to nothing now — using it would delete
/// whatever happens to hold that id today, which is the worst possible outcome.
fn redo_catalog_delete(
    cat: &catalog::db::Catalog,
    snap: &catalog::snapshot::CatalogSnapshot,
) -> Result<(), catalog::db::CatalogError> {
    use catalog::snapshot::CatalogSnapshot as S;
    match snap {
        S::Smart { name, .. } => {
            if let Some(r) = cat.smart_collections()?.into_iter().find(|r| &r.name == name) {
                cat.delete_smart(r.id)?;
            }
        }
        S::Collection { name, .. } => {
            if let Some(r) = cat.collections()?.into_iter().find(|r| &r.name == name) {
                cat.delete_collection(r.id)?;
            }
        }
        S::Keyword { tree, .. } => {
            if let Some(root) = tree.first() {
                if let Some(r) = cat
                    .keywords()?
                    .into_iter()
                    .find(|r| r.name == root.name && r.parent_id.is_none() == root.parent_id.is_none())
                {
                    cat.delete_keyword(r.id)?;
                }
            }
        }
        S::Folder { path, .. } => {
            if let Some(r) = cat.folders()?.into_iter().find(|r| &r.path == path) {
                cat.forget_folder(r.id)?;
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Push the action at all four delete sites**

`crates/app/src/collections_panel.rs`:

```rust
                let Some(cat) = &app.catalog else { return };
                if let Ok(snap) = cat.delete_collection(id) {
                    app.actions.push(
                        ActionKind::CatalogDelete { snapshot: Box::new(snap) },
                        None,
                    );
                    app.status = "Deleted collection — Ctrl+Z to restore".to_string();
                }
```

⚠ The same shape for `delete_smart` and `delete_keyword`, with their own status text. ⚠ **Read the
surrounding borrow structure first** — these sit in the post-panel `match action` block where
`app.catalog` and `app.status` are both reachable, but `cat` must be dropped before `app.status` is
assigned.

`crates/app/src/main.rs`'s `forget_folder` likewise, with
`"Forgot the folder — Ctrl+Z to restore"`.

- [ ] **Step 6: Verify all four are wired**

Run: `grep -n "delete_collection\|delete_smart\|delete_keyword\|forget_folder" crates/app/src/collections_panel.rs crates/app/src/main.rs | grep "let _ ="`
Expected: **no output.** ⚠ A remaining `let _ =` is a snapshot thrown away and a feature silently
absent.

- [ ] **Step 7: Full green**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
cargo test -p pipeline --test golden 2>&1 | tail -2
```
Expected: `clippy exit: 0`, no failures, **49 goldens unmoved** — this touches no pipeline code.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(catalog): Ctrl+Z restores a deleted collection, keyword or folder"
```

---

### Task 4: Confirm a keyword deletion that takes a subtree

**Files:**
- Modify: `crates/catalog/src/db.rs` (a count query)
- Modify: `crates/app/src/modals/mod.rs` (the `Modal` enum and a dialog)
- Modify: `crates/app/src/collections_panel.rs` (route the delete through it)

- [ ] **Step 1: The counts**

⚠ Reuses the recursive CTE `keyword_members` already has; nothing new walks the subtree.

```rust
    /// How much a keyword deletion would take with it: `(descendants, assignments)`.
    ///
    /// ⚠ Descendants **excludes** the keyword itself, and assignments **includes** the whole
    /// subtree's — the two numbers the confirmation says out loud.
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
```

Test it:

```rust
    #[test]
    fn the_delete_scope_counts_the_subtree_but_not_the_keyword_itself() {
        let c = Catalog::open_in_memory().unwrap();
        let europe = c.create_keyword("Europe", None).unwrap();
        let france = c.create_keyword("France", Some(europe)).unwrap();
        c.create_keyword("Paris", Some(france)).unwrap();
        assert_eq!(c.keyword_delete_scope(europe).unwrap().0, 2);
        assert_eq!(c.keyword_delete_scope(france).unwrap().0, 1);
        assert_eq!(
            c.keyword_delete_scope(europe).unwrap(),
            (2, 0),
            "no assignments yet"
        );
    }
```

- [ ] **Step 2: The dialog**

Add to `Modal` in `crates/app/src/modals/mod.rs`:

```rust
    /// Confirming a keyword deletion that would take descendants or assignments with it.
    ConfirmDeleteKeyword {
        id: i64,
        name: String,
        descendants: usize,
        assignments: usize,
    },
```

and the dialog, modelled on `confirm_forget_folder`:

```rust
fn confirm_delete_keyword(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::ConfirmDeleteKeyword { id, name, descendants, assignments } = &app.modal else {
        return;
    };
    let (id, name, descendants, assignments) = (*id, name.clone(), *descendants, *assignments);
    let mut confirmed = false;
    let mut cancelled = false;

    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "confirm_delete_keyword",
        "Delete keyword",
        theme::DialogSize::Compact,
        |ui| {
            ui.label(format!("Delete “{name}”?"));
            let mut what = Vec::new();
            if descendants > 0 {
                what.push(format!(
                    "{descendants} keyword{} beneath it",
                    if descendants == 1 { "" } else { "s" }
                ));
            }
            if assignments > 0 {
                what.push(format!(
                    "the keyword from {assignments} photo{}",
                    if assignments == 1 { "" } else { "s" }
                ));
            }
            crate::theme::hint(
                ui,
                &format!("This also deletes {}. Ctrl+Z restores it.", what.join(" and ")),
            );
            ui.separator();
            let (yes, no) = crate::theme::button_row(ui, "Delete", true, "Cancel");
            confirmed = yes;
            cancelled = no;
        },
    );

    if confirmed {
        app.delete_keyword_now(id);
        app.modal = Modal::None;
    } else if cancelled || dismissed {
        app.modal = Modal::None;
    }
}
```

⚠ Add `ConfirmDeleteKeyword` to the `Which` enum and both matches in `draw`, and register
`("confirm_delete_keyword", DialogSize::Compact)` in
`modals::tests::every_dialog_has_a_class_and_the_ids_are_unique`, whose count becomes **15**.

- [ ] **Step 3: Route the delete**

In `collections_panel.rs`'s `DeleteKeyword` arm: read the scope, and **only** open the dialog when
it is non-zero.

```rust
        Action::DeleteKeyword(id, name) => {
            let scope = app
                .catalog
                .as_ref()
                .and_then(|c| c.keyword_delete_scope(id).ok())
                .unwrap_or((0, 0));
            let name = name.clone();
            // ⚠⚠ Confirmed **only when the deletion removes more than the keyword itself.** A leaf
            // nobody has used is an unsurprising deletion, and a dialog for it is the false alarm
            // `banding_warning` already rejects: one that fires on ordinary practice trains people
            // to dismiss the one that matters.
            if scope.0 > 0 || scope.1 > 0 {
                app.modal = Modal::ConfirmDeleteKeyword {
                    id,
                    name,
                    descendants: scope.0,
                    assignments: scope.1,
                };
            } else {
                app.delete_keyword_now(id);
            }
        }
```

⚠ `Action::DeleteKeyword` currently carries only the id (`collections_panel.rs:377` builds it as
`Action::DeleteKeyword(k.id)`, and `k.name` is in scope right there). Change the variant to
`DeleteKeyword(i64, String)` and that call to `Action::DeleteKeyword(k.id, k.name.clone())`.

⚠⚠ **`delete_keyword_now` is the one implementation both routes share**, so the confirmed and
unconfirmed paths cannot drift — the rule `apply_cell_action` and `name_edit` already follow. It is
Task 3's delete-and-push arm, lifted onto `RustroomApp`:

```rust
    /// Delete a keyword and record it, however the deletion was reached.
    ///
    /// ⚠ One implementation for the confirmed and unconfirmed routes. Two copies of "delete, then
    /// push the undo entry" is how one of them ends up not pushing.
    fn delete_keyword_now(&mut self, id: i64) {
        let Some(cat) = &self.catalog else { return };
        match cat.delete_keyword(id) {
            Ok(snap) => {
                self.actions.push(
                    ActionKind::CatalogDelete {
                        snapshot: Box::new(snap),
                    },
                    None,
                );
                self.status = "Deleted keyword — Ctrl+Z to restore".to_string();
            }
            Err(e) => self.status = format!("Could not delete the keyword: {e}"),
        }
    }
```

⚠ `self.status` is assigned after the `cat` borrow ends — the shape every method in `main.rs`
uses.

- [ ] **Step 4: Test the firing rule**

```rust
    /// ⚠ Both halves. A dialog that never fires and one that always fires are equally wrong, and
    /// only the pair of assertions tells them apart.
    #[test]
    fn a_bare_keyword_is_deleted_without_a_dialog_and_a_parent_is_not() {
        let c = catalog::db::Catalog::open_in_memory().unwrap();
        let leaf = c.create_keyword("Paris", None).unwrap();
        assert_eq!(c.keyword_delete_scope(leaf).unwrap(), (0, 0), "no dialog");

        let parent = c.create_keyword("Europe", None).unwrap();
        c.create_keyword("France", Some(parent)).unwrap();
        assert_ne!(c.keyword_delete_scope(parent).unwrap(), (0, 0), "dialog");
    }
```

⚠ Put it in `catalog`'s tests, since it is about the scope query rather than the dialog.

- [ ] **Step 5: Full green and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git add -A
git commit -m "feat(catalog): confirm a keyword deletion that takes a subtree"
```

---

### Task 5: Look at it

⚠ The wiring — that Ctrl+Z reaches these and the panel refreshes — is what the harness cannot see.

- [ ] **Step 1: Launch**

```bash
cargo run --release -p app 2>&1 | tail -5 &
```

⚠ **Back the catalog up first**, because this task deliberately deletes things:

```bash
cp "$HOME/Library/Application Support/rustroom/catalog.db" /tmp/catalog.backup.db
```

⚠ Check that path exists first; if the catalog lives elsewhere, find it before deleting anything.

- [ ] **Step 2: Each of the four**

Delete a collection, press Ctrl+Z: it comes back **with its photos in it**. Then a smart
collection, then a keyword with children — the confirmation must name the count — and each undo
brings it back.

- [ ] **Step 3: Forget a folder and undo**

The one that matters. Rate a photo 4 stars first, forget the folder, then Ctrl+Z: the folder, its
photos **and the rating** must all come back.

- [ ] **Step 4: The collision, by hand**

Delete a collection, create a **new** one, *then* Ctrl+Z. Both must exist afterwards. This is the
case that motivated remapping and the one an id-preserving design fails.

- [ ] **Step 5: A bare keyword is not confirmed**

Create a keyword with no children and no photos, delete it: **no dialog**, and Ctrl+Z still
restores it.

- [ ] **Step 6: Docs**

Add a section to `CLAUDE.md`'s "Current status" in the house style — the two probes and what they
overturned, the ⚠ findings, the test count, the accepted caveats. Remove the **Undo for catalog
deletions** entry from `docs/BACKLOG.md`. Update the `▶ NEXT ACTION`.

```bash
git add -A && git commit -m "docs: catalog deletions are undoable"
```

⚠ **Do not push and do not merge.** Report the branch state.
