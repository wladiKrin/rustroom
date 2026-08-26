# Collections (M4.1, plan 2 of 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Manual collections you can create, add photos to, and browse — **across folders**, which is what makes the catalog worth having.

**Architecture:** Collection CRUD on the existing `Catalog`, then a left panel in the Library view. Selecting a collection builds a synthetic `CatalogEntry` scan from its members and loads it exactly as a folder, so `entries`, `thumbs`, `Selection` and the filmstrip all work unchanged.

**Tech Stack:** Rust, `rusqlite`, `eframe`/`egui` 0.35.

**Spec:** `docs/superpowers/specs/2026-08-26-catalog-database-design.md`
**Predecessor:** `docs/superpowers/plans/2026-08-26-catalog-foundation.md` (complete — schema, folder sync, rename/renumber integration).

**Build environment:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing rules:** no `Co-Authored-By`; never push; sabotage every new test and **confirm it compiled**; clippy clean checked by **exit code**, separately from tests.

⚠ `cargo test --all | grep -c "test result: FAILED"` returns 0 when the *build* fails. Check exit codes.

⚠ `cargo test` only *warns* on an unused variable while `clippy -D warnings` rejects it. A green test run is not evidence clippy passes — that cost a bad commit two milestones ago.

---

## ⚠⚠ The design decision this plan makes, beyond the spec

The spec said plan 2 would *"filter the grid"* by a collection. **That would not deliver the feature.** A collection's whole point is that it spans folders — the scenario that chose one invisible catalog over one per folder was *"show me every 5-star frame this year"* across three shoots. Filtering only the folder you happen to have open reproduces exactly the fragmentation that argument rejected.

**So selecting a collection loads its members as the Library's content**, built into a synthetic scan from the stored paths. The photos may come from any folder ever opened.

⚠ This costs almost nothing, because everything downstream keys on paths already: `entries::build`, `ThumbLoader`, `Selection` and the filmstrip need no change at all.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/catalog/src/db.rs` (modify) | Collection CRUD and membership queries |
| `crates/app/src/collections.rs` (create) | Turning a collection's members into a scan; the pure logic |
| `crates/app/src/main.rs` (modify) | `Library` gains its source; open-a-collection |
| `crates/app/src/collections_panel.rs` (create) | The Library-view left panel |
| `crates/app/src/shell.rs` (modify) | Draws the panel in Library view |

---

### Task 1: Collection CRUD

**Files:**
- Modify: `crates/catalog/src/db.rs`

- [ ] **Step 1: Write the failing tests**

Add to `db.rs`'s `mod tests`:

```rust
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
        let got: Vec<String> = cat.collections().unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(got, ["apple", "Middle", "Zebra"], "case-insensitive, like the browser");
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

        cat.remove_from_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        assert_eq!(cat.collections().unwrap().len(), 1, "the collection survives");
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
        // by row id, so moving the row carries the membership — the reason a rename must update
        // the catalog in the same operation as the file move.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
        let c = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(c, Path::new("/x/a.RAF"), 0).unwrap();

        cat.rename_photo(Path::new("/x/a.RAF"), Path::new("/x/b.RAF")).unwrap();

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
```

- [ ] **Step 2: Run to verify it fails**

```bash
cd ~/Projects/rustroom
cargo test -p catalog --lib a_collection_can_be_created 2>&1 | grep -E "^error" | head -3
```

Expected: `no method named create_collection`.

- [ ] **Step 3: Implement**

Add to `db.rs` above `impl Catalog`:

```rust
/// A collection, with how many photos are in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionRow {
    pub id: i64,
    pub name: String,
    pub count: i64,
}
```

Add inside `impl Catalog`:

```rust
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
```

- [ ] **Step 4: Run**

```bash
cargo test -p catalog --lib db:: 2>&1 | grep -E "^test result"
```

Expected: `ok. 22 passed`.

- [ ] **Step 5: Sabotage**

Use the helper from the previous plan (`/tmp/sab.py`), which checks compilation first.

⚠⚠ Confirm the harness works before trusting a clean run: a matcher that greps failure lines by *bare* test name finds nothing, because cargo prints module paths.

1. Drop `ORDER BY lower(c.name), c.name` from `collections` → expect `collections_come_back_in_name_order` **CAUGHT**.
2. Change `add_to_collection`'s `ON CONFLICT ... DO NOTHING` to no conflict clause and the insert to `INSERT OR REPLACE` with a new rowid → simpler: remove the `PRIMARY KEY (collection_id, photo_id)` from the migration → expect `adding_a_photo_to_a_collection_counts_it_once` **CAUGHT**.
3. Make `delete_collection` also run `DELETE FROM photos` → expect `deleting_a_collection_never_touches_the_photos` **CAUGHT**.
4. Make `collection_members` filter on a single folder (`AND f.id = (SELECT min(id) FROM folders)`) → expect `a_collection_holds_photos_from_several_folders` **CAUGHT**.

- [ ] **Step 6: Commit**

```bash
git add crates/catalog/src/db.rs
git commit -m "feat(catalog): manual collections, spanning folders"
```

---

### Task 2: Members into a browsable scan

**Files:**
- Create: `crates/app/src/collections.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/collections.rs`:

```rust
//! Turning a collection's membership into something the browser can show.
//!
//! ⚠⚠ **A collection is browsed, not filtered.** Selecting one replaces the Library's contents
//! with its members, which may come from *any* folder ever opened. Merely filtering the folder
//! you happen to have open would reproduce exactly the fragmentation that choosing one catalog
//! over one-per-folder was meant to avoid.
//!
//! ⚠ Everything downstream keys on paths already — `entries::build`, `ThumbLoader`, `Selection`
//! and the filmstrip need no change at all. That is why this costs almost nothing.

use catalog::CatalogEntry;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_become_a_scan_of_one_entry_per_file() {
        // ⚠ One entry per *file*, not per member: `entries::build` expands copies itself from the
        // sidecar. Passing copies through here would double them.
        let members = vec![
            (PathBuf::from("/x/a.RAF"), 0),
            (PathBuf::from("/x/a.RAF"), 1),
            (PathBuf::from("/y/b.RAF"), 0),
        ];
        let scan = scan_of(&members, |_| None);
        let names: Vec<&str> = scan.iter().map(|e| e.file_name.as_str()).collect();
        assert_eq!(names, ["a.RAF", "b.RAF"]);
    }

    #[test]
    fn a_member_whose_file_has_vanished_is_still_listed() {
        // ⚠ A missing file must not silently disappear from the collection you put it in — an
        // unplugged drive would otherwise look like the collection had emptied itself. It is
        // listed; opening it will fail honestly.
        let members = vec![(PathBuf::from("/gone/x.RAF"), 0)];
        let scan = scan_of(&members, |_| None);
        assert_eq!(scan.len(), 1);
        assert_eq!(scan[0].file_name, "x.RAF");
    }

    #[test]
    fn the_edited_time_is_supplied_by_the_caller() {
        // ⚠ Same layering as `scan_folder`: "last edited" means the sidecar's mtime, and the
        // sidecar naming convention belongs to `app`, not to `catalog`.
        let members = vec![(PathBuf::from("/x/a.RAF"), 0)];
        let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(42);
        let scan = scan_of(&members, |_| Some(when));
        assert_eq!(scan[0].edited, Some(when));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --lib collections 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find function scan_of`.

- [ ] **Step 3: Implement**

Insert above the tests:

```rust
/// Build a browsable scan from a collection's members.
///
/// `edited_at` is injected for the same reason `catalog::CatalogEntry::edited` is filled by its
/// caller: the sidecar naming convention belongs to `app`, and it keeps this function testable
/// without disk.
///
/// ⚠ One entry per **file**, not per member. `entries::build` expands copies from the sidecar
/// itself, so passing copies through here would double every one of them.
pub fn scan_of(
    members: &[(PathBuf, u32)],
    edited_at: impl Fn(&Path) -> Option<std::time::SystemTime>,
) -> Vec<CatalogEntry> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut out = Vec::new();
    for (path, _copy) in members {
        if seen.contains(path) {
            continue;
        }
        seen.push(path.clone());
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // ⚠ A vanished file is still listed. Dropping it would make an unplugged drive look like
        // the collection had emptied itself.
        let modified = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        out.push(CatalogEntry {
            path: path.clone(),
            file_name,
            modified,
            edited: edited_at(path),
        });
    }
    out
}
```

- [ ] **Step 4: Declare the module**

In `crates/app/src/lib.rs`, in alphabetical order among the `pub mod` lines:

```rust
pub mod collections;
```

- [ ] **Step 5: Run**

```bash
cargo test -p app --lib collections 2>&1 | grep -E "^test result"
```

Expected: `ok. 3 passed`.

- [ ] **Step 6: Sabotage**

1. Remove the `seen` de-duplication → expect `members_become_a_scan_of_one_entry_per_file` **CAUGHT**.
2. Skip entries whose `std::fs::metadata` fails → expect `a_member_whose_file_has_vanished_is_still_listed` **CAUGHT**.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/collections.rs crates/app/src/lib.rs
git commit -m "feat(app): turn a collection's members into a browsable scan"
```

---

### Task 3: The Library can show a collection

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Give `Library` a source**

Add above `struct Library`:

```rust
/// What the Library is currently showing.
///
/// ⚠ A collection is a *source*, not a filter, so the format filter and the sort orders compose
/// with it unchanged — they operate on whatever scan is loaded.
#[derive(Debug, Clone, PartialEq)]
enum LibrarySource {
    Folder(PathBuf),
    Collection { id: i64, name: String },
}
```

Replace `Library`'s `dir: PathBuf` field with:

```rust
    /// Where these photos came from.
    ///
    /// ⚠ Replaces the old `dir`. `Reindex` re-reads a *folder*; on a collection it re-reads the
    /// membership instead, which is the same intent applied to a different source.
    source: LibrarySource,
```

- [ ] **Step 2: Fix the two readers the compiler names**

`open_folder` constructs `Library`; give it `source: LibrarySource::Folder(dir.clone())`.

`reindex` reads `lib.dir`; change its opening to:

```rust
        let Some(LibrarySource::Folder(dir)) = self.library.as_ref().map(|l| l.source.clone())
        else {
            // ⚠ Reindex renumbers copies in a *folder*. On a collection there is no single
            // folder to re-read, and renumbering photos you are only viewing would be surprising.
            self.status = "Reindex works on a folder, not a collection".into();
            return;
        };
```

- [ ] **Step 3: Add the loader**

Add to `impl RustroomApp`, beside `open_folder`:

```rust
    /// Show a collection's photos, from wherever they live.
    ///
    /// ⚠⚠ The members are re-read from the catalog every time, not cached: a photo added from
    /// another view must appear here without a restart.
    fn open_collection(&mut self, ctx: &egui::Context, id: i64, name: &str) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let members = match cat.collection_members(id) {
            Ok(m) => m,
            Err(e) => {
                self.status = format!("Could not read the collection: {e}");
                return;
            }
        };
        let mut scan = collections::scan_of(&members, sidecar::edited_at);
        catalog::sort(&mut scan, self.settings.library_sort);
        let entries = entries::build(&scan, sidecar::load_all);
        let n = entries.len();
        let paths: Vec<_> = entries.iter().map(|e| e.id.path.clone()).collect();
        let ctx2 = ctx.clone();
        let space = self
            .settings
            .effective_display_space(self.os_manages_colour);
        let loader = ThumbLoader::new(paths, space, move || ctx2.request_repaint());
        self.library = Some(Library {
            source: LibrarySource::Collection {
                id,
                name: name.to_string(),
            },
            scan,
            entries,
            thumbs: (0..n).map(|_| None).collect(),
            selection: Selection::default(),
            loader,
            filter: FormatFilter::default(),
            visible: (0..n).collect(),
        });
        self.view = AppView::Library;
        self.status = if n == 0 {
            format!("“{name}” is empty")
        } else {
            format!("“{name}” — {n} photos")
        };
    }
```

- [ ] **Step 4: Verify**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy: $?"
cargo test --all > /tmp/t.log 2>&1; echo "test: $?"
```

Expected: both `0`.

⚠ `open_collection` has no caller yet, and in a **binary** crate that is `dead_code` which `-D warnings` rejects. If clippy fails for that reason, land Tasks 3 and 4 as one commit — this is the eighth instance of that trap in this project.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): the Library can show a collection as well as a folder"
```

---

### Task 4: The collections panel

**Files:**
- Create: `crates/app/src/collections_panel.rs`
- Modify: `crates/app/src/main.rs`
- Modify: `crates/app/src/shell.rs`

- [ ] **Step 1: Write the panel**

Create `crates/app/src/collections_panel.rs`:

```rust
//! The Library's collections list.
//!
//! ⚠ A **left** panel, in the Library view only — Lightroom's placement, and it competes with
//! nothing. The Presets panel was removed for having two homes; this has one.

use crate::RustroomApp;
use eframe::egui;

/// Draw the collections panel. Returns nothing: every action is applied here.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ⚠ Collected and applied after the list closes, the shape every panel in this app uses:
    // egui borrows `app` through the closure, so a click cannot act immediately.
    enum Action {
        None,
        Open(i64, String),
        Delete(i64),
        AddSelection(i64),
    }
    let mut action = Action::None;

    egui::Panel::left("collections")
        .default_size(180.0)
        .show(ui, |ui| {
            ui.heading("Collections");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut app.new_collection)
                        .hint_text("New collection")
                        .desired_width(110.0),
                );
                // ⚠ Guarded on a non-empty name: a nameless collection cannot be told from
                // another nameless one, and the store keys on the name.
                let named = !app.new_collection.trim().is_empty();
                if ui.add_enabled(named, egui::Button::new("+")).clicked() {
                    if let Some(cat) = &app.catalog {
                        let _ = cat.create_collection(app.new_collection.trim());
                    }
                    app.new_collection.clear();
                }
            });
            ui.separator();

            let rows = app
                .catalog
                .as_ref()
                .and_then(|c| c.collections().ok())
                .unwrap_or_default();
            if rows.is_empty() {
                ui.label("No collections yet.");
            }
            let selected = app.selected_collection();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for c in &rows {
                    ui.horizontal(|ui| {
                        if ui.button("×").on_hover_text("Delete").clicked() {
                            action = Action::Delete(c.id);
                        }
                        let label = format!("{} ({})", c.name, c.count);
                        if ui.selectable_label(selected == Some(c.id), label).clicked() {
                            action = Action::Open(c.id, c.name.clone());
                        }
                        // ⚠ Enabled only with a selection, and it names the count, so it can
                        // never be ambiguous about what it is about to do.
                        let n = app.library.as_ref().map(|l| l.selection.len()).unwrap_or(0);
                        if ui
                            .add_enabled(n > 0, egui::Button::new("+"))
                            .on_hover_text(format!("Add {n} selected"))
                            .clicked()
                        {
                            action = Action::AddSelection(c.id);
                        }
                    });
                }
            });
        });

    match action {
        Action::None => {}
        Action::Open(id, name) => app.open_collection(ctx, id, &name),
        Action::Delete(id) => {
            if let Some(cat) = &app.catalog {
                let _ = cat.delete_collection(id);
            }
            // ⚠ If the deleted collection was on screen, there is nothing left to show.
            if app.selected_collection() == Some(id) {
                app.library = None;
                app.status = "Collection deleted".into();
            }
        }
        Action::AddSelection(id) => {
            let picked: Vec<_> = app
                .library
                .as_ref()
                .map(|l| {
                    l.selection
                        .iter()
                        .filter_map(|i| l.entries.get(i))
                        .map(|e| e.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            let mut n = 0;
            if let Some(cat) = &app.catalog {
                for pid in &picked {
                    if cat.add_to_collection(id, &pid.path, pid.copy).is_ok() {
                        n += 1;
                    }
                }
            }
            app.status = format!("Added {n} to the collection");
        }
    }
}
```

- [ ] **Step 2: Add the two app fields**

In `struct RustroomApp`:

```rust
    /// Scratch text for the new-collection field.
    new_collection: String,
```

Initialise it to `String::new()` in both `RustroomApp::new` and `test_app::for_test`.

Add to `impl RustroomApp`:

```rust
    /// Which collection the Library is showing, if it is showing one.
    fn selected_collection(&self) -> Option<i64> {
        match self.library.as_ref().map(|l| &l.source) {
            Some(LibrarySource::Collection { id, .. }) => Some(*id),
            _ => None,
        }
    }
```

- [ ] **Step 3: Draw it**

In `crates/app/src/shell.rs`, in the `match self.view` arm for `AppView::Library`, before `library::show`:

```rust
            AppView::Library => {
                collections_panel::show(self, ui, &ctx);
                library::show(self, ui, &ctx);
            }
```

⚠ **Panel order is load-bearing** — egui allocates side panels in call order and the central panel takes what is left, so the collections panel must be drawn *before* the grid.

Declare the module in `main.rs`:

```rust
mod collections_panel;
```

- [ ] **Step 4: Verify and look at it**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy: $?"
cargo test --all > /tmp/t.log 2>&1; echo "test: $?"
cargo run --release -p app
```

Open a folder, select a few photos, make a collection, add them, click the collection. Then open a *different* folder, add one of its photos, and click the collection again — **both folders' photos must appear together.** That is the acceptance criterion for the whole milestone.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/collections_panel.rs crates/app/src/main.rs crates/app/src/shell.rs
git commit -m "feat(app): a collections panel in the Library"
```

---

### Task 5: Record it

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add a section** following the established format. Include:

- ⚠⚠ **A collection is browsed, not filtered** — selecting one replaces the Library's contents with its members across every folder. Filtering only the open folder would reproduce the fragmentation that choosing one catalog was meant to avoid.
- ⚠ It costs almost nothing because everything downstream keys on paths: `entries::build`, `ThumbLoader`, `Selection` and the filmstrip are unchanged.
- ⚠ `Library.dir` became `LibrarySource`, and `Reindex` refuses on a collection.
- ⚠ Deleting a collection never touches photos; `ON DELETE CASCADE` removes memberships and stops there.
- ⚠ A member whose file has vanished is still listed.
- Accepted caveats: no smart collections (M4.3); no nesting; a collection cannot be renamed; no drag-and-drop.

- [ ] **Step 2: Update the state line and mark M4.1 complete.**

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record collections"
```

---

## Self-review

**Spec coverage:** collection CRUD → Task 1; membership across folders → Task 1; browsing → Tasks 2 and 3; the panel → Task 4; "deleting a collection never touches photos" → Task 1's test; the rename integration asserted end to end → Task 1's `a_renamed_file_stays_in_its_collections`.

**Placeholders:** none.

**Type consistency:** `CollectionRow`, `create_collection`, `collections`, `delete_collection`, `add_to_collection`, `remove_from_collection`, `collection_members` are defined in Task 1 and used in Tasks 3 and 4. `collections::scan_of` is defined in Task 2 and used in Task 3. `LibrarySource` and `selected_collection` are defined in Tasks 3 and 4.

⚠ **Known risk:** Task 3 renames a `Library` field, so the compiler will name every reader. Read each before editing — `reindex` is the only one that needs real thought, and the plan gives its replacement in full.
