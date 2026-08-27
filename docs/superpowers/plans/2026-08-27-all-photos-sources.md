# "All Photos" and folder management — M4.3a-ii, Plan 1 of 2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the whole catalogued library browsable as one view, and give the user a way to prune what feeds it.

**Architecture:** Three read/delete queries on `Catalog`, a fourth `LibrarySource`, and the three near-identical loaders collapsed into one. The left panel gains a Folders section whose rows re-scan on click.

**Tech Stack:** Rust, `rusqlite` (bundled SQLite), `egui`/`eframe` 0.35. No new dependencies, **no schema migration** — everything needed exists at v3.

**Spec:** `docs/superpowers/specs/2026-08-27-all-photos-and-filtering-design.md`

**Plan 2 (filtering) is a separate document** and covers `LibraryFilter` and the filter bar. Do not start it here.

---

## Ground rules for this repo

- **Branch:** `m4.3a-ii-all-photos`. **Never push** — the user pushes.
- **Commit messages carry no `Co-Authored-By` trailer.**
- **Build env** — every `cargo` command needs this first, in the same shell:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **The GUI binary is `Rustroom`**: bin tests are `cargo test -p app --bin Rustroom`.
- **⚠ In a binary crate an item lands with its first caller** — a `pub fn` with no caller fails
  `clippy -D warnings` as dead code. Ninth instance; it is why the panel task is not split.
- **⚠ Check exit codes, never grep output.** `cargo test --all | grep …` reports *grep's* status,
  and a failed build emits no test-result lines at all.
- **⚠ The sabotage compile check is "did a `test result:` line appear at all"** — **not** a grep for
  `^error:`, which matches cargo's ordinary `error: test failed, to rerun pass …` line and produced
  a false "did not compile" three times in the previous slice.

Verification command used throughout:

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

---

## File Structure

| file | responsibility | new? |
|---|---|---|
| `crates/catalog/src/db.rs` | `PhotoRow`, `FolderRow`, `all_photos`, `folders`, `forget_folder` | modify |
| `crates/app/src/collections.rs` | `scan_of` gains an injected `modified_at` | modify |
| `crates/app/src/main.rs` | `LibrarySource::AllPhotos`; the three loaders collapse into `open_scan`; `open_all_photos`; `open_catalog_folder`; `forget_folder` | modify |
| `crates/app/src/collections_panel.rs` | a Folders section above Collections | modify |
| `crates/app/src/modals/mod.rs` + a confirm dialog | the Forget confirmation | modify |
| `crates/app/src/test_app.rs` | the `Library` fixture follows the field change | modify |

---

## Task 1: Catalog queries

**Files:**
- Modify: `crates/catalog/src/db.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/catalog/src/db.rs`'s test module. Read the existing tests first and follow their
fixture style (they build an in-memory catalog with `Catalog::open_in_memory()` and call
`sync_folder`); copy that setup rather than inventing another.

```rust
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
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"])).unwrap();
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
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"])).unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["two.RAF"])).unwrap();
        let coll = cat.create_collection("Portfolio").unwrap();
        cat.add_to_collection(coll, Path::new("/a/one.RAF"), 0).unwrap();
        cat.add_to_collection(coll, Path::new("/b/two.RAF"), 0).unwrap();
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
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"])).unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["two.RAF"])).unwrap();
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
```

⚠⚠ **The existing helper is `entry(name) -> CatalogEntry`, singular, and it hardcodes the path as
`/x/<name>`** — so it cannot build entries for two different folders and the tests above must not
use it as written. Add a sibling beside it rather than changing it (four existing tests depend on
its `/x/` path):

```rust
    /// Entries for `folder`, one per name. ⚠ The existing `entry` helper hardcodes `/x/`, which
    /// cannot express the two-folder case every whole-library query needs.
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
```

and write the calls as `&entries_in("/a", &["one.RAF", "two.RAF"])`.

⚠ **The "forgetting touches no files" property is asserted in Task 4, not here** — an in-memory
catalog has no files behind it, so a test here could not tell the difference. Do not fake it.

- [ ] **Step 2: Run to verify they fail**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p catalog 2>&1 | tail -20
```

Expected: compile errors — `all_photos`, `folders` and `forget_folder` do not exist.

- [ ] **Step 3: Write the implementation**

Add near `CollectionRow` in `crates/catalog/src/db.rs`:

```rust
/// One photo row, as `all_photos` returns it.
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
```

And in `impl Catalog`, beside the collection queries:

```rust
    /// Every photo the catalog knows, from every folder ever opened.
    ///
    /// ⚠ Ordered by folder then name so the result is stable; the Library re-sorts it by the
    /// user's chosen order anyway, but a stable base makes the sort's tie-break meaningful.
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
    /// every edit in them are untouched — re-opening the folder re-catalogues it, minus the
    /// library metadata. That is the M4.1 split doing its job: *delete the catalog and you lose
    /// organisation, never work.*
    ///
    /// ⚠ Its ratings, flags, labels and keyword assignments **do** go, by a chained cascade:
    /// `folders` → `photos` → `collection_photos`, and `folders` → `photo_keywords`. The second
    /// hop needs `PRAGMA foreign_keys = ON`, which is set per connection in `open`.
    pub fn forget_folder(&self, id: i64) -> Result<(), CatalogError> {
        self.conn
            .execute("DELETE FROM folders WHERE id = ?1", [id])?;
        Ok(())
    }
```

⚠ Check `PathBuf` is already imported in `db.rs` — it is used by `marks_in` — and that `PhotoRow`'s
`mtime` column type matches how `sync_folder` writes it (`file_facts` produces the value; read it
and match the type rather than assuming `i64`).

- [ ] **Step 4: Run to verify they pass**

```bash
cargo test -p catalog 2>&1 | grep -E "^test result:"
```

Expected: all pass, five more than before.

- [ ] **Step 5: Sabotage the pragma and watch the cascade test fail**

In `Catalog::open`, temporarily change `PRAGMA foreign_keys = ON` to `OFF`, then:

```bash
cargo test -p catalog > /tmp/s.log 2>&1
if ! grep -q "^test result:" /tmp/s.log; then echo "DID NOT COMPILE"; else grep -E "^test result:" /tmp/s.log; grep -A 6 "^failures:$" /tmp/s.log | head; fi
```

Expected: `forgetting_a_folder_cascades_through_photos_to_collections_and_keywords` fails. Restore
and re-run green.

- [ ] **Step 6: Commit**

```bash
git add crates/catalog/src/db.rs
git commit -m "feat(catalog): whole-library photo query, folder listing and forget"
```

---

## Task 2: `scan_of` takes the modification times from its caller

**Files:**
- Modify: `crates/app/src/collections.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/app/src/collections.rs`'s test module:

```rust
    #[test]
    fn the_modification_time_is_supplied_by_the_caller() {
        // ⚠ Same layering as `edited_at`, and for a sharper reason: All Photos must not walk the
        // filesystem. `scan_of` used to stat every member, which is 10,000 stats on a view that
        // otherwise touches no files — while the catalog has stored `mtime` since M4.1 and
        // nothing had ever read it.
        let members = vec![(PathBuf::from("/x/a.RAF"), 0)];
        let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(99);
        let scan = scan_of(&members, |_| None, |_| when);
        assert_eq!(scan[0].modified, when);
    }
```

⚠ The three existing tests in this module call `scan_of(&members, |_| None)`. Update each to pass a
third closure — `|_| std::time::SystemTime::UNIX_EPOCH` — and **change nothing else about their
assertions**. Their continuing to pass is the evidence this refactor changed no behaviour.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --lib collections 2>&1 | tail -15
```

Expected: compile error — `scan_of` takes two arguments.

- [ ] **Step 3: Write the implementation**

In `crates/app/src/collections.rs`, change the signature and the body:

```rust
pub fn scan_of(
    members: &[(PathBuf, u32)],
    edited_at: impl Fn(&Path) -> Option<std::time::SystemTime>,
    modified_at: impl Fn(&Path) -> std::time::SystemTime,
) -> Vec<CatalogEntry> {
```

and replace the `let modified = std::fs::metadata(path)…` line with:

```rust
        let modified = modified_at(path);
```

Update the doc comment above the function to say that **both** times are injected, keeping the
existing reasoning about `edited_at` and adding: *"and `modified_at` for the sharper reason that
All Photos must not walk the filesystem — the catalog already recorded these at scan time."*

⚠ Keep the existing comment about a vanished file still being listed. Its `unwrap_or(UNIX_EPOCH)`
moves to the *callers* that stat; note that in the doc so the rule is not lost.

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test -p app --lib collections 2>&1 | grep -E "^test result:"
```

Expected: 4 pass. ⚠ `cargo build -p app` will now fail at `open_collection` and `open_keyword` —
that is expected and Task 3 fixes it, so **do not commit yet.**

---

## Task 3: One loader, and `AllPhotos`

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the variant**

In `crates/app/src/main.rs`, add to `enum LibrarySource`:

```rust
    /// Every photo the catalog knows, from every folder ever opened.
    AllPhotos,
```

Build and let the compiler name every non-exhaustive `match`:

```bash
cargo build -p app 2>&1 | grep -E "^error" -A 8 | head -40
```

Fix each site it names. Expected sites include `Reindex`'s guard and anything that formats the
source for the status line. ⚠ **The compiler is the authority here, not this list.**

For `reindex`, widen only the message — the `let … else` on `LibrarySource::Folder` already
refuses correctly:

```rust
            self.status = "Reindex works on a folder, not a collection or All Photos".into();
```

- [ ] **Step 2: Collapse the three loaders into one**

`open_folder`, `open_collection` and `open_keyword` end with the same fifteen lines. Extract, as a
method on `RustroomApp`:

```rust
    /// Build and show a `Library` from an already-sorted scan.
    ///
    /// ⚠ The one place a `Library` is constructed. Four callers had grown three copies of this
    /// between them, and a fourth was the point at which they became one — a new field on
    /// `Library` had to be added in three places, which is how two of them drift.
    fn open_scan(
        &mut self,
        ctx: &egui::Context,
        source: LibrarySource,
        scan: Vec<catalog::CatalogEntry>,
    ) -> usize {
        let entries = entries::build(&scan, sidecar::load_all);
        let n = entries.len();
        let ctx2 = ctx.clone();
        let space = self
            .settings
            .effective_display_space(self.os_manages_colour);
        let loader = ThumbLoader::new(space, move || ctx2.request_repaint());
        self.library = Some(Library {
            source,
            scan,
            entries,
            cache: rustroom_app::thumb_cache::ThumbCache::with_default_budget(),
            last_want: Vec::new(),
            selection: Selection::default(),
            loader,
            // A fresh `Library` per load, so the filter resets by construction.
            filter: FormatFilter::default(),
            visible: (0..n).collect(),
            marks: std::collections::HashMap::new(),
            min_stars: 0,
        });
        self.reload_marks();
        self.view = AppView::Library;
        n
    }
```

Rewrite the three existing loaders to end with it. `open_collection` becomes, in full:

```rust
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
        let mut scan = rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::Collection {
                id,
                name: name.to_string(),
            },
            scan,
        );
        self.status = if n == 0 {
            format!("“{name}” is empty")
        } else {
            format!("“{name}” — {n} photos")
        };
    }
```

`open_keyword` follows the identical shape with its own message. `open_folder` keeps its scan,
sort, `sync_folder` and error handling, and ends with `let n = self.open_scan(ctx,
LibrarySource::Folder(dir.clone()), scan);` followed by its existing status lines.

Add the shared stat helper as a free function near them:

```rust
/// The modification time `scan_of` used to read for itself.
///
/// ⚠ `UNIX_EPOCH` when the metadata cannot be read, so an unreadable file sorts to the oldest end
/// rather than failing the whole load — the rule `CatalogEntry::modified` already documents.
fn fs_modified(path: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}
```

- [ ] **Step 3: Add `open_all_photos`**

```rust
    /// Show every photo the catalog knows, from every folder ever opened.
    ///
    /// ⚠⚠ **This does not walk the filesystem.** The times come from the catalog, recorded when
    /// each folder was last scanned — which is what makes the view affordable at 10,000 photos,
    /// and is why `mtime` was captured in M4.1. Consequence: a file changed outside Rustroom
    /// since its folder was last scanned shows a stale time here. Clicking that folder in the
    /// panel re-scans it and refreshes them.
    fn open_all_photos(&mut self, ctx: &egui::Context) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let rows = match cat.all_photos() {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("Could not read the catalog: {e}");
                return;
            }
        };
        // ⚠ A lookup rather than a stat per photo. Built once here, consulted by `scan_of`.
        let mtimes: std::collections::HashMap<PathBuf, std::time::SystemTime> = rows
            .iter()
            .filter_map(|r| {
                r.mtime.map(|s| {
                    (
                        r.path.clone(),
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(s.max(0) as u64),
                    )
                })
            })
            .collect();
        let members: Vec<(PathBuf, u32)> =
            rows.into_iter().map(|r| (r.path, r.copy)).collect();
        let mut scan = rustroom_app::collections::scan_of(&members, sidecar::edited_at, |p| {
            mtimes
                .get(p)
                .copied()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(ctx, LibrarySource::AllPhotos, scan);
        self.status = if n == 0 {
            "The catalog is empty — open a folder to add photos.".into()
        } else {
            format!("All photos — {n}")
        };
    }
```

⚠ `s.max(0)` guards a negative unix time, which `SystemTime` addition cannot represent — it would
panic rather than produce a wrong sort. Match the actual column type read in Task 1.

- [ ] **Step 4: Add `open_catalog_folder`**

```rust
    /// Open a folder named by the catalog, re-scanning it.
    ///
    /// ⚠⚠ **A folder row re-scans; it does not browse from the catalog.** "One idiom for the whole
    /// panel" was the first instinct and it is wrong here: collections and keywords *span* folders
    /// and genuinely cannot scan, while a folder row names one real directory and can. So there is
    /// no such thing as a stale panel-browsed folder, refreshing needs no new button (click the row
    /// again), and `Reindex` keeps working because the source is a `Folder`.
    ///
    /// ⚠ An unreadable directory falls back to the catalog's last-seen rows rather than showing an
    /// empty folder. An unplugged drive must not look like the folder emptied itself — the rule
    /// `sync_folder` already follows in never deleting a photo row.
    fn open_catalog_folder(&mut self, ctx: &egui::Context, dir: &std::path::Path) {
        if catalog::scan_folder(dir).is_ok() {
            self.open_folder_at(ctx, dir.to_path_buf());
            return;
        }
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let names = cat.photos_in(dir).unwrap_or_default();
        let members: Vec<(PathBuf, u32)> =
            names.into_iter().map(|n| (dir.join(n), 0)).collect();
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(ctx, LibrarySource::Folder(dir.to_path_buf()), scan);
        self.status = format!("{} is unreadable — showing {n} photos last seen", dir.display());
    }
```

⚠ This needs `open_folder` split into the dialog half and the work half. `open_folder(ctx)` keeps
the `rfd::FileDialog::new().pick_folder()` and calls `open_folder_at(ctx, dir)`; **everything else
moves into `open_folder_at`.** Do that split first, confirm the build is green, then add the method
above.

- [ ] **Step 5: Verify**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: green. ⚠ `open_all_photos`, `open_catalog_folder` and `forget_folder`'s UI have no caller
yet — **they are methods on a bin-crate type, so `dead_code` fires.** That is the ninth instance of
this trap, and it means **Tasks 3 and 4 land as one commit.** Do not commit yet; go to Task 4.

---

## Task 4: The Folders panel and Forget

**Files:**
- Modify: `crates/app/src/collections_panel.rs`
- Modify: `crates/app/src/main.rs`
- Modify: `crates/app/src/modals/mod.rs` and the modal it dispatches to

- [ ] **Step 1: Add the panel section**

In `crates/app/src/collections_panel.rs`, extend the `Action` enum:

```rust
        OpenAllPhotos,
        OpenFolder(std::path::PathBuf),
        ForgetFolder(i64, std::path::PathBuf, i64),
    }
```

and draw a **Folders** section **above** the Collections heading, inside the same panel, following
the existing sections' idiom exactly (a heading, a separator, a `ScrollArea`, `selectable_label`
rows with a `×`):

```rust
            ui.heading("Folders");
            let all = app
                .catalog
                .as_ref()
                .and_then(|c| c.photo_count().ok())
                .unwrap_or(0);
            let showing_all = matches!(
                app.library.as_ref().map(|l| &l.source),
                Some(crate::LibrarySource::AllPhotos)
            );
            if ui
                .selectable_label(showing_all, format!("All Photos ({all})"))
                .clicked()
            {
                action = Action::OpenAllPhotos;
            }
            let folders = app
                .catalog
                .as_ref()
                .and_then(|c| c.folders().ok())
                .unwrap_or_default();
            for f in &folders {
                ui.horizontal(|ui| {
                    if ui
                        .button("×")
                        .on_hover_text("Forget this folder")
                        .clicked()
                    {
                        action = Action::ForgetFolder(f.id, f.path.clone(), f.count);
                    }
                    // ⚠ The basename is what fits and what identifies; the full path is the hover.
                    let shown = f
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| f.path.to_string_lossy().to_string());
                    let selected = matches!(
                        app.library.as_ref().map(|l| &l.source),
                        Some(crate::LibrarySource::Folder(p)) if p == &f.path
                    );
                    if ui
                        .selectable_label(selected, format!("{shown} ({})", f.count))
                        .on_hover_text(f.path.to_string_lossy())
                        .clicked()
                    {
                        action = Action::OpenFolder(f.path.clone());
                    }
                });
            }
            ui.separator();
```

⚠ Read the file first and match its existing structure — where the heading, the `ScrollArea` and
the `ui.separator()` calls sit. The block above is the content, not the exact placement.

- [ ] **Step 2: Dispatch the three actions**

In the `match action` block at the bottom of the same file:

```rust
        Action::OpenAllPhotos => app.open_all_photos(ctx),
        Action::OpenFolder(path) => app.open_catalog_folder(ctx, &path),
        Action::ForgetFolder(id, path, count) => {
            app.modal = Modal::ConfirmForgetFolder { id, path, count };
        }
```

⚠ `Modal` is declared at `crates/app/src/modals/mod.rs:66` and the field on `RustroomApp` is
`modal`. Follow the shape of `ConfirmDeletePreset` — a struct variant with named fields — but
**without** its `dont_ask_again`, for the reason in Step 3.

- [ ] **Step 3: Add the confirmation modal**

Add a `Modal::ConfirmForgetFolder { id: i64, path: PathBuf, count: i64 }` variant and a dialog
following the existing `ConfirmDeletePreset` one — **`egui::Modal`, confirm-first/dismiss-second
button order, via `theme::button_row`**. Read that dialog and mirror it.

The body:

```rust
    ui.label(format!("Forget “{}”?", path.display()));
    theme::hint(
        ui,
        &format!(
            "Removes {count} photos from the catalog, with their ratings, flags, labels and \
             keywords. No files are deleted and no edits are lost — re-opening the folder \
             catalogues it again."
        ),
    );
```

⚠ **No "don't ask again".** A deleted preset goes to a 20-entry trash and is restorable by Ctrl+Z;
this has neither. Suppressing a guard on the *irrecoverable* action is exactly the distinction
`the_two_delete_guards_are_independent` exists to protect.

On confirm:

```rust
            if let Some(cat) = &app.catalog {
                let _ = cat.forget_folder(id);
            }
            // ⚠ If the forgotten folder is what is on screen, close the Library rather than leave
            // it showing rows that no longer exist anywhere — the rule the collection and keyword
            // deletions already follow.
            let showing = matches!(
                app.library.as_ref().map(|l| &l.source),
                Some(LibrarySource::Folder(p)) if p == &path
            );
            if showing {
                app.library = None;
            }
            app.status = format!("Forgot “{}”", path.display());
```

⚠ **A folder shown as part of `AllPhotos` must also be handled** — decide and comment: reopening
All Photos is the honest response, since some of its rows are now gone. Match on `AllPhotos` too
and call `app.open_all_photos(ctx)`.

- [ ] **Step 4: Verify**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: green.

- [ ] **Step 5: Write the "no files are touched" test**

This is the property people actually worry about, and Task 1 could not assert it because an
in-memory catalog has no files.

⚠ `crates/catalog/src/db.rs`'s existing `TempDb::new(tag)` returns a path to `<dir>/catalog.db` and
removes `<dir>` on drop, so `db.0.parent().unwrap()` is a real temp **directory** to put photos in —
no new helper is needed, and its uniqueness is already structural (process id plus a counter, never
the clock). Write:

```rust
    #[test]
    fn forgetting_a_folder_deletes_no_files() {
        // ⚠⚠ The M4.1 split, asserted: *delete the catalog and you lose organisation, never work.*
        // Edits live in the per-photo sidecars and survive this entirely.
        let db = TempDb::new("nofiles");
        let dir = db.0.parent().unwrap().to_path_buf();
        let photo = dir.join("one.RAF");
        std::fs::write(&photo, b"not really a raw").unwrap();
        let sidecar = dir.join("one.RAF.json");
        std::fs::write(&sidecar, b"{}").unwrap();

        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(&dir, &entries_in("/a", &["one.RAF"])).unwrap();
        let id = cat.folders().unwrap()[0].id;
        cat.forget_folder(id).unwrap();

        assert!(photo.exists(), "the photo was deleted");
        assert!(sidecar.exists(), "the sidecar was deleted");
        assert!(cat.all_photos().unwrap().is_empty());
    }
```

⚠ `TempDb` must stay alive for the whole test — binding it to `db` rather than `_` is what stops
`Drop` removing the directory immediately. Its `Drop` deletes the parent directory, so the photo and
sidecar go with it when the test ends.

- [ ] **Step 6: Sabotage and verify**

Run each, confirming a `test result:` line appears before believing the outcome:

| sabotage | expected |
|---|---|
| `forget_folder` also does `DELETE FROM photos` explicitly *before* the folder delete | **nothing fails** — the cascade would have done it anyway. Record it as inert. |
| `open_catalog_folder` always takes the catalog fallback path | **nothing fails automatically** — no test covers the re-scan. Record it, and check by hand. |
| `PRAGMA foreign_keys = OFF` | the cascade test fails (already run in Task 1) |

⚠ Two of these are expected to be inert. Say so rather than inventing a test that appears to cover
them; a sabotage that does not change behaviour is not evidence about the test.

- [ ] **Step 7: Run the app**

```bash
cargo run --release -p app
```

Check by hand — none of this is test-covered:

1. The left panel shows **Folders** above Collections, with **All Photos (n)** and each folder.
2. Clicking **All Photos** shows photos from *both* your folders in one grid.
3. Clicking a folder row opens it, and **adding a file to that folder and clicking the row again
   shows the new file** — that is the re-scan, and the reason there is no Refresh button.
4. `Reindex` still works on a panel-opened folder, and says something sensible on All Photos.
5. `×` on a folder asks first, names the count, and afterwards that folder is gone from the panel
   while **its files are still on disk**.

- [ ] **Step 8: Commit Tasks 2, 3 and 4 together**

```bash
git add -A
git commit -m "feat(app): browse the whole catalog, and forget a folder from it"
```

---

## Task 5: Docs

**Files:**
- Modify: `CLAUDE.md`, the spec, and the Obsidian roadmap

- [ ] **Step 1: Update the docs**

- Spec header → `**Status:** plan 1 built 2026-08-27; plan 2 outstanding`.
- `CLAUDE.md` — a section in house style: what changed, the ⚠ findings (the chained cascade and its
  pragma dependency; folder rows re-scanning rather than browsing, and why that reverses "one idiom
  for the whole panel"; `mtime` finally being read after being stored unused since M4.1; the
  loaders collapsing into one; forgetting deleting no files), the accepted caveats, the new test
  count, and the next action set to **Plan 2 — the filter bar**.
- The Obsidian roadmap — mark M4.3a-ii plan 1 done under the four-way decomposition.

- [ ] **Step 2: Final verification and commit**

```bash
rustup update stable
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
echo "exit=$?"
```

⚠ Check the exit code, not the output. Then commit. **Do not push.**

---

## Definition of done

- `fmt`, `clippy --all-targets --all-features -D warnings`, `test --all` all exit 0.
- **All 47 goldens byte-identical** — nothing here touches `pipeline`.
- All Photos shows photos from more than one folder in one grid, confirmed on the real catalog.
- Forgetting a folder removes its catalog rows, keeps its files, and is confirmed first.
- A folder row re-scans on click, confirmed by adding a file and clicking again.
