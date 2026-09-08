# Metadata writing (M6.2) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-photo title, caption, copyright and creator, edited in the Information dialog,
stored in the catalog, written into every export together with rating, label, keywords and
capture facts, and round-tripped through the `stem.xmp` sidecar.

**Architecture:** A `Description` row per *file* in the catalog (schema v7). The export worker
receives, per job, everything the packet needs and hands `export::metadata` one `FileMetadata`
payload, resolved from the preset's defaults and cut to an `All` / `CopyrightOnly` scope. The `xmp`
crate's owned field set grows by the four Dublin Core properties so the sidecar merge carries them.
The dialog keeps a draft and commits on close through a lib function, then through the one XMP
funnel every mark change already uses.

**Tech Stack:** Rust, rusqlite, quick-xml, chrono, egui 0.35 headless harness (`test_ui` /
`test_app`), serde.

Spec: `docs/superpowers/specs/2026-09-08-metadata-writing-design.md`.

⚠⚠ **Standing conventions, and they override the harness's own default:** commit messages carry
**no `Co-Authored-By` trailer**; **never `git push`** — the user pushes himself. Work on branch
`m6.2-metadata-writing`, which already exists and holds the spec.

⚠ **Every `cargo` command below assumes this was run first in the shell:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ Two files this plan edits are already over the ~600-line mark and must be **named in the
report**: `crates/catalog/src/db.rs` (~3,100 lines with tests) and
`crates/app/src/batch_export.rs` (~1,500). Neither is split here; the additions are small and
the split is not this milestone's job.

## One correction to the spec, made while planning

§3 says the Metadata section's header dot *"shows for `All` always"*. That contradicts the test
`a_section_that_is_doing_something_says_so_in_its_header`, whose comment records why: *"A marker
that is always on and one that is never on are equally useless."* The dot exists to surface
**hidden non-default state**. `All` is the default, so it is not hidden state. The rule is
therefore `is_active() = !is_empty() || scope != MetadataScope::default()` — a field filled in
*or* the scope changed. Task 8 fixes the spec sentence in place.

---

## File map

| file | responsibility in this plan |
|---|---|
| `crates/catalog/src/db.rs` | `Description`, table v7, `description_of`, `set_description`, forget/restore |
| `crates/catalog/src/snapshot.rs` | `DescriptionSnapshot`, a field on `CatalogSnapshot::Folder` |
| `crates/export/Cargo.toml` | `chrono` |
| `crates/export/src/metadata.rs` | `MetadataScope`, `FileMetadata`, `resolve`, the full packet, `embed` |
| `crates/export/src/lib.rs` | re-exports |
| `crates/app/src/batch_export.rs` | `ExportJob` fields, payload built before `embed` |
| `crates/app/src/export_flow.rs` | gather description, marks, keywords per job (bin) |
| `crates/app/examples/deliverable.rs` | signature churn only |
| `crates/app/src/modals/export.rs` | the scope choice, the header dot (bin) |
| `crates/xmp/src/lib.rs` | four owned fields: read, inject, merge |
| `crates/app/src/xmp_sync.rs` | `sidecar_of`, `apply` |
| `crates/app/src/settings.rs` | one doc sentence |
| `crates/app/src/description.rs` | **new** lib module: `DescriptionDraft`, `commit` |
| `crates/app/src/lib.rs` | `pub mod description;` |
| `crates/app/src/modals/mod.rs`, `modals/info.rs`, `library.rs` | the draft in the modal, the block, commit on close (bin) |
| docs | divergence, roadmap, backlog, `CLAUDE.md`, spec status |

---

### Task 1: `Description` in the catalog, schema v7

**Files:**
- Modify: `crates/catalog/src/db.rs` — `SCHEMA_VERSION` (line 19), `migrate` (after the `from < 6` gate, ~line 371), new type near `Marks` (~line 78), new accessors after `capture_of` (~line 812), the two drop lists (~lines 1975 and 2157), the v5 simulation (~line 2192), tests near `a_capture_round_trips` (~line 2640)

- [ ] **Step 1: Write the failing tests**

In the `mod tests` block of `crates/catalog/src/db.rs`, directly after `an_empty_capture_is_still_recorded`:

```rust
    #[test]
    fn a_description_round_trips_and_an_unknown_photo_is_none() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        assert_eq!(cat.description_of(Path::new("/a/one.RAF")).unwrap(), None);
        let d = Description {
            title: "Harbour at dusk".into(),
            caption: "Fishing boats returning, Whitby.".into(),
            copyright: "© 2026 W".into(),
            creator: "W".into(),
        };
        cat.set_description(Path::new("/a/one.RAF"), &d).unwrap();
        assert_eq!(cat.description_of(Path::new("/a/one.RAF")).unwrap(), Some(d));
        // A photo the catalog has never seen is `None`, not an error.
        assert_eq!(cat.description_of(Path::new("/a/ghost.RAF")).unwrap(), None);
    }

    #[test]
    fn two_copies_of_one_file_share_a_description() {
        // ⚠ Per **file**, like keywords and capture: a title says what is in the picture, which
        // every copy shares. Only marks describe the image you made. Spec §1.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        let d = Description {
            title: "Shared".into(),
            ..Description::default()
        };
        cat.set_description(Path::new("/a/one.RAF"), &d).unwrap();
        // There is no copy argument to ask with; the one row serves both.
        assert_eq!(
            cat.description_of(Path::new("/a/one.RAF")).unwrap(),
            Some(d)
        );
    }

    #[test]
    fn an_all_empty_description_removes_the_row() {
        // ⚠ "Absent" and "blank" are one state, so the table holds only photos that say
        // something — and a reader cannot mistake four empty strings for a deliberate
        // assertion of no rights, the blank-copyright problem `ExportMetadata` documents.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.set_description(
            Path::new("/a/one.RAF"),
            &Description {
                caption: "gone soon".into(),
                ..Description::default()
            },
        )
        .unwrap();
        cat.set_description(
            Path::new("/a/one.RAF"),
            &Description {
                caption: "   ".into(),
                ..Description::default()
            },
        )
        .unwrap();
        assert_eq!(cat.description_of(Path::new("/a/one.RAF")).unwrap(), None);
        let left: i64 = cat
            .conn
            .query_row("SELECT count(*) FROM photo_description", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "a blank row survived");
    }

    #[test]
    fn a_version_six_catalog_upgrades_and_keeps_everything() {
        // ⚠⚠ The sixth migration to meet real user data. Adding a table must touch nothing else.
        let db = TempDb::new("v6v7");
        {
            let cat = Catalog::open(&db.0).unwrap();
            cat.sync_folder(Path::new("/x"), &[entry("a.RAF")]).unwrap();
            let k = cat.create_keyword("Paris", None).unwrap();
            cat.assign_keyword(k, Path::new("/x/a.RAF")).unwrap();
            cat.set_marks(
                Path::new("/x/a.RAF"),
                0,
                Marks {
                    rating: 3,
                    ..Marks::default()
                },
            )
            .unwrap();
            cat.set_xmp_mtime(Path::new("/x/a.RAF"), 42).unwrap();
            cat.conn
                .execute_batch(
                    "DROP TABLE photo_description;
                     PRAGMA user_version = 6;",
                )
                .unwrap();
        }

        let cat = Catalog::open(&db.0).expect("a v6 catalog must upgrade");

        assert_eq!(cat.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(cat.photo_count().unwrap(), 1);
        assert_eq!(cat.keywords().unwrap()[0].count, 1, "keyword kept");
        assert_eq!(cat.marks(Path::new("/x/a.RAF"), 0).unwrap().rating, 3);
        assert_eq!(
            cat.xmp_mtimes_in(Path::new("/x")).unwrap(),
            vec![(PathBuf::from("/x/a.RAF"), 42)],
            "the sidecar clock kept"
        );
        assert_eq!(cat.description_of(Path::new("/x/a.RAF")).unwrap(), None);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p catalog description 2>&1 | tail -5`
Expected: compile error, `cannot find type Description` / `no method named description_of`.

- [ ] **Step 3: Add the type, the version, the gate and the accessors**

In `crates/catalog/src/db.rs`, change line 19:

```rust
pub const SCHEMA_VERSION: i64 = 7;
```

After the `impl Marks { … }` block (which ends before `pub struct KeywordRow`, ~line 110), add:

```rust
/// What a photograph says about itself, in words: the four IPTC-shaped fields Rustroom edits.
///
/// ⚠⚠ **Per FILE, not per copy** — keyed like `photo_capture` and `photo_keywords`. A title says
/// what is *in* the picture, which every copy shares; only marks describe the image you made.
/// Lightroom gives virtual copies independent metadata; this is a recorded divergence (spec §1).
///
/// ⚠ Stored in the catalog only, and written into **exports** and the **XMP sidecar** — never
/// into an original. That is what dissolved the design conflict `docs/BACKLOG.md` held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Description {
    pub title: String,
    pub caption: String,
    pub copyright: String,
    pub creator: String,
}

impl Description {
    /// Whether every field is blank.
    ///
    /// ⚠ Trimmed, so a field holding only spaces counts as absent — the rule
    /// `ExportMetadata::is_empty` follows, and for the same reason: a tag whose whole content is
    /// whitespace is the blank-copyright problem in a costume.
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty()
            && self.caption.trim().is_empty()
            && self.copyright.trim().is_empty()
            && self.creator.trim().is_empty()
    }
}
```

In `migrate`, directly after the `if from < 6 { … }` block and before `Ok(())`:

```rust
        if from < 7 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–6. Sixth time this rule meets a
            // database holding real user data.
            //
            // ⚠ Keyed on the **file**, like `photo_capture`: a description says what is in the
            // picture, which every copy shares. See `Description`. Every column is NOT NULL with
            // an empty default, and `set_description` deletes the row when all four are blank,
            // so the table holds only photos that say something.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE photo_description (
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     title     TEXT NOT NULL DEFAULT '',
                     caption   TEXT NOT NULL DEFAULT '',
                     copyright TEXT NOT NULL DEFAULT '',
                     creator   TEXT NOT NULL DEFAULT '',
                     PRIMARY KEY (folder_id, file_name)
                 );
                 PRAGMA user_version = 7;
                 COMMIT;",
            )?;
        }
```

After `capture_of` (the method ending with `Err(e) => Err(e.into()),` / `}` / `}` at ~line 812), add:

```rust
    /// What a photo says about itself, or `None` when it says nothing.
    ///
    /// ⚠ `None` is the ordinary state, not an error: only photos with a description have a row.
    pub fn description_of(&self, photo: &Path) -> Result<Option<Description>, CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(None);
        };
        let row = self.conn.query_row(
            "SELECT d.title, d.caption, d.copyright, d.creator
             FROM photo_description d JOIN folders f ON f.id = d.folder_id
             WHERE f.path = ?1 AND d.file_name = ?2",
            rusqlite::params![folder, name],
            |r| {
                Ok(Description {
                    title: r.get(0)?,
                    caption: r.get(1)?,
                    copyright: r.get(2)?,
                    creator: r.get(3)?,
                })
            },
        );
        match row {
            Ok(d) => Ok(Some(d)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Record what a photo says about itself.
    ///
    /// ⚠ An all-blank description **deletes** the row rather than storing four empty strings,
    /// so "absent" and "blank" are one state — see `Description::is_empty`.
    ///
    /// ⚠ A photo whose folder the catalog does not know is silently skipped: the catalog is not
    /// authoritative and must never be a reason an action fails — the `add_to_collection` rule.
    pub fn set_description(&self, photo: &Path, d: &Description) -> Result<(), CatalogError> {
        let Some((folder, name)) = split(photo) else {
            return Ok(());
        };
        if d.is_empty() {
            self.conn.execute(
                "DELETE FROM photo_description
                 WHERE file_name = ?2
                   AND folder_id = (SELECT id FROM folders WHERE path = ?1)",
                rusqlite::params![folder, name],
            )?;
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO photo_description
                 (folder_id, file_name, title, caption, copyright, creator)
             SELECT id, ?2, ?3, ?4, ?5, ?6 FROM folders WHERE path = ?1
             ON CONFLICT(folder_id, file_name) DO UPDATE SET
                 title = ?3, caption = ?4, copyright = ?5, creator = ?6",
            rusqlite::params![folder, name, d.title, d.caption, d.copyright, d.creator],
        )?;
        Ok(())
    }
```

- [ ] **Step 4: Grow the two drop lists and the v5 simulation**

In `a_version_one_catalog_upgrades_and_keeps_its_rows` (~line 1975) and
`a_version_two_catalog_upgrades_and_keeps_everything` (~line 2157), the `execute_batch` string
begins `"DROP TABLE smart_collections;`. Insert a first line so each reads:

```rust
                    "DROP TABLE photo_description;
                     DROP TABLE smart_collections;
                     DROP TABLE photo_capture;
```

In `a_version_five_catalog_upgrades_and_keeps_everything` (~line 2192) the batch is
`"ALTER TABLE photos DROP COLUMN xmp_mtime;\n PRAGMA user_version = 5;"`. Make it:

```rust
                    "DROP TABLE photo_description;
                     ALTER TABLE photos DROP COLUMN xmp_mtime;
                     PRAGMA user_version = 5;",
```

- [ ] **Step 5: Run the catalog tests**

Run: `cargo test -p catalog 2>&1 | tail -3`
Expected: `test result: ok.` with 4 more tests than before, none failing. In particular
`a_version_one_catalog_upgrades_and_keeps_its_rows` passes — if it fails with "table
photo_description already exists", a drop list was missed.

- [ ] **Step 6: Commit**

```bash
git add crates/catalog/src/db.rs
git commit -m "Catalog: a Description per file, schema v7"
```

---

### Task 2: Forgetting a folder snapshots its descriptions, and undo restores them

⚠ The spec did not name this. `forget_folder` cascades through `photo_description` (its FK is
`ON DELETE CASCADE`) and its snapshot restores photos, marks, captures, memberships and keywords —
so without this task an undone forget would silently lose every caption. *"Forgetting a folder is
the only action that destroys metadata the user made by hand, and a restore that dropped the
ratings would look like it had worked."* Same sentence, one more table.

**Files:**
- Modify: `crates/catalog/src/snapshot.rs` — new struct after `CaptureSnapshot`; field on `Folder`
- Modify: `crates/catalog/src/db.rs` — `forget_folder` (~line 704–748), `restore` (~line 1370–1440), tests

- [ ] **Step 1: Write the failing test**

After `forgetting_a_folder_and_undoing_brings_back_its_marks` (~line 3040):

```rust
    #[test]
    fn forgetting_a_folder_and_undoing_brings_back_its_descriptions() {
        // ⚠ Same sentence as the marks test, one more table: a restore that dropped the
        // captions would look like it had worked.
        let (cat, dir) = with_two_photos();
        let d = Description {
            title: "Kept".into(),
            caption: "through a forget and an undo".into(),
            ..Description::default()
        };
        cat.set_description(&dir.join("a.RAF"), &d).unwrap();
        let fid = cat.folders().unwrap()[0].id;

        let snap = cat.forget_folder(fid).unwrap();
        let left: i64 = cat
            .conn
            .query_row("SELECT count(*) FROM photo_description", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "orphaned description rows survived the forget");

        cat.restore(&snap).unwrap();
        assert_eq!(cat.description_of(&dir.join("a.RAF")).unwrap(), Some(d));
        assert_eq!(cat.description_of(&dir.join("b.RAF")).unwrap(), None);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p catalog brings_back_its_descriptions 2>&1 | tail -5`
Expected: FAIL at the last-but-one assertion: `left == Some(d)` is `None` after restore.

- [ ] **Step 3: Add the snapshot type and field**

In `crates/catalog/src/snapshot.rs`, after `CaptureSnapshot`:

```rust
/// One `photo_description` row.
#[derive(Debug, Clone, PartialEq)]
pub struct DescriptionSnapshot {
    pub file_name: String,
    pub title: String,
    pub caption: String,
    pub copyright: String,
    pub creator: String,
}
```

In `CatalogSnapshot::Folder`, after `captures: Vec<CaptureSnapshot>,`:

```rust
        descriptions: Vec<DescriptionSnapshot>,
```

- [ ] **Step 4: Capture and restore the rows**

In `forget_folder`, after the `captures` query block ends (`.collect();` / `drop(st);`, before the
`collection_photos` query), add:

```rust
        let mut st = tx.prepare(
            "SELECT file_name, title, caption, copyright, creator
             FROM photo_description WHERE folder_id = ?1",
        )?;
        let descriptions: Vec<DescriptionSnapshot> = st
            .query_map([id], |r| {
                Ok(DescriptionSnapshot {
                    file_name: r.get(0)?,
                    title: r.get(1)?,
                    caption: r.get(2)?,
                    copyright: r.get(3)?,
                    creator: r.get(4)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();
        drop(st);
```

and in the `Ok(CatalogSnapshot::Folder { … })` at the end of `forget_folder`, add `descriptions,`
after `captures,`.

Add `DescriptionSnapshot` to the `use crate::snapshot::{…}` line at the top of `db.rs` (it already
imports `CaptureSnapshot`).

In `restore`, the `CatalogSnapshot::Folder { path, photos, captures, memberships, keywords }`
pattern gains `descriptions,` after `captures,`. After the `for c in captures { … }` loop, add:

```rust
                for d in descriptions {
                    tx.execute(
                        "INSERT OR REPLACE INTO photo_description
                             (folder_id, file_name, title, caption, copyright, creator)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            fid,
                            d.file_name,
                            d.title,
                            d.caption,
                            d.copyright,
                            d.creator
                        ],
                    )?;
                }
```

- [ ] **Step 5: Find every other `Folder {` site**

Run: `grep -rn "CatalogSnapshot::Folder {" crates/ --include='*.rs'`
Expected: only `db.rs` (construct and restore, both done) and `snapshot.rs`'s `label`, which
uses `..` and needs nothing. If `crates/app` has a site with a full pattern, add `descriptions: _,`
to it.

- [ ] **Step 6: Run the catalog tests**

Run: `cargo test -p catalog 2>&1 | tail -3`
Expected: `test result: ok.`

- [ ] **Step 7: Commit**

```bash
git add crates/catalog/src/db.rs crates/catalog/src/snapshot.rs
git commit -m "Catalog: a forgotten folder's descriptions come back on undo"
```

---

### Task 3: `MetadataScope` on `ExportMetadata`

**Files:**
- Modify: `crates/export/src/metadata.rs` (struct at line 43, tests `meta()` at line 371)
- Modify: `crates/export/src/lib.rs` (line 7)
- Modify: `crates/app/src/batch_export.rs` (test literal at ~line 1485, ~line 1468)
- Modify: `crates/app/examples/deliverable.rs` (literal at line 56)

- [ ] **Step 1: Write the failing tests**

In `crates/export/src/metadata.rs`'s `mod tests`, after `an_empty_metadata_writes_no_packet`:

```rust
    #[test]
    fn the_scope_is_all_unless_changed_and_activity_follows_either() {
        // ⚠ `All` is the default, for existing presets too (user's call, spec §3): an export
        // gains camera, keywords and rating from now on. That is *not* hidden state, so the
        // header dot must not show for it — `is_active` is "a field filled OR the scope
        // changed", the rule `a_section_that_is_doing_something_says_so_in_its_header` guards.
        let m = ExportMetadata::default();
        assert_eq!(m.scope, MetadataScope::All);
        assert!(!m.is_active(), "an untouched section must not claim activity");
        let filled = ExportMetadata {
            copyright: "(c)".into(),
            ..ExportMetadata::default()
        };
        assert!(filled.is_active());
        let narrowed = ExportMetadata {
            scope: MetadataScope::CopyrightOnly,
            ..ExportMetadata::default()
        };
        assert!(narrowed.is_active(), "a changed scope is state worth showing");
    }

    #[test]
    fn a_preset_written_before_the_scope_existed_loads_as_all() {
        // The exact JSON M6.1b wrote. Container-level `#[serde(default)]` carries the new field.
        let json = r#"{"copyright":"(c) me","creator":"","caption":""}"#;
        let m: ExportMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(m.scope, MetadataScope::All);
        assert_eq!(m.copyright, "(c) me");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p export scope 2>&1 | tail -5`
Expected: compile error, `no field scope` / `cannot find MetadataScope`.

- [ ] **Step 3: Add the enum and field**

Replace the `ExportMetadata` struct and its `impl` (lines 36–59) with:

```rust
/// How much of what the catalog knows goes into the exported file.
///
/// ⚠ **`All` is the default, for existing presets too (user's call).** This is the one
/// deliberate exception to the M6.1 seam's "defaults to do nothing" rule: an existing user's
/// next export gains camera, exposure, keywords and rating — which is the point of the
/// milestone, and what every other tool does by default. Recorded in spec §3.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetadataScope {
    /// Title, caption, rights, creator, rating, label, keywords and the capture facts.
    #[default]
    All,
    /// Rights and creator only — for a client who should not see the shoot's keywords.
    CopyrightOnly,
}

impl MetadataScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All metadata",
            Self::CopyrightOnly => "Copyright only",
        }
    }
}

/// Rights and description to write into every exported file, and how much else goes with them.
///
/// ⚠ Empty means "write nothing", not "write an empty string" — a blank copyright tag is worse
/// than an absent one, because a reader cannot tell it from a deliberate assertion of no rights.
///
/// ⚠ Since M6.2 these three strings are **defaults**: a photo's own title, caption, copyright or
/// creator (its catalog `Description`) wins over them — see [`FileMetadata::resolve`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportMetadata {
    pub copyright: String,
    pub creator: String,
    pub caption: String,
    pub scope: MetadataScope,
}

impl ExportMetadata {
    /// Whether the three default strings are all blank.
    ///
    /// ⚠ Trimmed, so a field holding only spaces counts as absent. A tag whose whole content is
    /// whitespace is the blank-copyright problem in a costume.
    pub fn is_empty(&self) -> bool {
        self.copyright.trim().is_empty()
            && self.creator.trim().is_empty()
            && self.caption.trim().is_empty()
    }

    /// Whether the section holds anything other than its defaults — what the export dialog's
    /// header dot shows.
    ///
    /// ⚠ Not "whether a packet will be written": under `All` one almost always is, and a dot
    /// that is always on is as useless as one that is never on.
    pub fn is_active(&self) -> bool {
        !self.is_empty() || self.scope != MetadataScope::default()
    }
}
```

In `crates/export/src/lib.rs` line 7:

```rust
pub use metadata::{ExportMetadata, FileMetadata, MetadataScope};
```

(`FileMetadata` arrives in Task 4; until then use `pub use metadata::{ExportMetadata, MetadataScope};`
and extend it there.)

- [ ] **Step 4: Fix the three struct literals**

`crates/export/src/metadata.rs` test helper `meta` (line 371): add `scope: MetadataScope::All,`
after `caption: caption.into(),`. (Task 4 rewrites this helper; the field is needed now to compile.)

`crates/app/src/batch_export.rs` ~line 1485, inside `the_delivery_fields_round_trip`:

```rust
            metadata: ExportMetadata {
                copyright: "© 2026".into(),
                creator: "W".into(),
                caption: "c".into(),
                scope: export::MetadataScope::CopyrightOnly,
            },
```

and in `an_options_file_from_before_the_delivery_fields_still_loads`, after the line
`assert!(opts.metadata.copyright.is_empty() && opts.metadata.creator.is_empty());` add:

```rust
        assert_eq!(
            opts.metadata.scope,
            export::MetadataScope::All,
            "an older preset writes all metadata, the M6.2 default"
        );
```

`crates/app/examples/deliverable.rs` line 56–60: add `..ExportMetadata::default()` as the last
entry of the `ExportMetadata { … }` literal.

- [ ] **Step 5: Run**

Run: `cargo test -p export 2>&1 | tail -3 && cargo test -p app delivery 2>&1 | tail -3 && cargo build --examples -p app 2>&1 | tail -1`
Expected: both `test result: ok.`; the example builds.

- [ ] **Step 6: Commit**

```bash
git add crates/export/src/metadata.rs crates/export/src/lib.rs crates/app/src/batch_export.rs crates/app/examples/deliverable.rs
git commit -m "Export: a metadata scope, All by default"
```

---

### Task 4: `FileMetadata` and `resolve`

**Files:**
- Modify: `crates/export/src/metadata.rs`

- [ ] **Step 1: Write the failing tests**

In `mod tests`, after `a_preset_written_before_the_scope_existed_loads_as_all`:

```rust
    fn photo() -> FileMetadata {
        FileMetadata {
            title: "Harbour".into(),
            caption: "Boats".into(),
            copyright: "".into(),
            creator: "  ".into(),
            rating: 4,
            label: Some("Red".into()),
            keywords: vec![vec!["Europe".into(), "UK".into()], vec!["Sea".into()]],
            capture: core_image::Capture {
                camera: Some("FUJIFILM X-T4".into()),
                iso: Some(200),
                ..Default::default()
            },
        }
    }

    fn defaults() -> ExportMetadata {
        ExportMetadata {
            copyright: "(c) preset".into(),
            creator: "Preset Person".into(),
            caption: "preset caption".into(),
            scope: MetadataScope::All,
        }
    }

    #[test]
    fn the_photos_own_field_wins_and_a_blank_one_falls_back() {
        let r = FileMetadata::resolve(&defaults(), photo());
        assert_eq!(r.caption, "Boats", "the photo's caption beat the preset's");
        assert_eq!(r.copyright, "(c) preset", "an empty field fell back");
        assert_eq!(r.creator, "Preset Person", "a whitespace field fell back too");
        assert_eq!(r.title, "Harbour", "the preset has no title to offer");
        assert_eq!(r.rating, 4);
        assert_eq!(r.keywords.len(), 2);
        assert_eq!(r.capture.iso, Some(200));
    }

    #[test]
    fn copyright_only_keeps_rights_and_creator_and_nothing_else() {
        let opts = ExportMetadata {
            scope: MetadataScope::CopyrightOnly,
            ..defaults()
        };
        let r = FileMetadata::resolve(&opts, photo());
        assert_eq!(r.copyright, "(c) preset");
        assert_eq!(r.creator, "Preset Person");
        assert_eq!(
            (r.title.as_str(), r.caption.as_str(), r.rating, r.label, r.keywords.len()),
            ("", "", 0, None, 0)
        );
        assert_eq!(r.capture, core_image::Capture::default());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p export resolve 2>&1 | tail -5`
Expected: compile error, `cannot find FileMetadata`.

- [ ] **Step 3: Add the type**

After the `impl ExportMetadata { … }` block:

```rust
/// Everything one exported file says about its photograph — the payload the packet is written
/// from.
///
/// ⚠ Plain strings and a `Capture`, no catalog types: this crate has no catalog dependency and
/// keeps none. The app converts its `Description`, `Marks` and keyword paths into this.
///
/// ⚠ `label` is already the XMP text (`Red`, `Blue`…): what a catalog label is *called* is the
/// app's rule (`xmp_sync::LABELS`), not this crate's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileMetadata {
    pub title: String,
    pub caption: String,
    pub copyright: String,
    pub creator: String,
    /// 0 is unrated and writes nothing.
    pub rating: u8,
    pub label: Option<String>,
    /// Keyword paths, root first: `["Europe", "France", "Paris"]`.
    pub keywords: Vec<Vec<String>>,
    pub capture: core_image::Capture,
}

impl FileMetadata {
    /// `opts`'s three default strings under `photo`'s own values, then cut to `opts.scope`.
    ///
    /// ⚠ The photo's non-blank field wins; a blank one takes the preset's. The preset has no
    /// title and gains none — a title is per photo by nature.
    ///
    /// ⚠⚠ The scope is applied **here and nowhere else**: `CopyrightOnly` clears everything but
    /// rights and creator, so the packet writer has one rule — write what is present — and
    /// cannot forget a field the scope should have hidden.
    pub fn resolve(opts: &ExportMetadata, photo: FileMetadata) -> FileMetadata {
        fn pick(own: String, fallback: &str) -> String {
            if own.trim().is_empty() {
                fallback.trim().to_string()
            } else {
                own
            }
        }
        let FileMetadata {
            title,
            caption,
            copyright,
            creator,
            rating,
            label,
            keywords,
            capture,
        } = photo;
        let copyright = pick(copyright, &opts.copyright);
        let creator = pick(creator, &opts.creator);
        match opts.scope {
            MetadataScope::All => FileMetadata {
                title,
                caption: pick(caption, &opts.caption),
                copyright,
                creator,
                rating,
                label,
                keywords,
                capture,
            },
            MetadataScope::CopyrightOnly => FileMetadata {
                copyright,
                creator,
                ..FileMetadata::default()
            },
        }
    }
}
```

Update `crates/export/src/lib.rs` line 7 to `pub use metadata::{ExportMetadata, FileMetadata, MetadataScope};`.

- [ ] **Step 4: Run**

Run: `cargo test -p export 2>&1 | tail -3`
Expected: `test result: ok.`

- [ ] **Step 5: Commit**

```bash
git add crates/export/src/metadata.rs crates/export/src/lib.rs
git commit -m "Export: FileMetadata, resolved from the preset's defaults and cut to the scope"
```

---

### Task 5: The full packet, and `embed` takes the payload

**Files:**
- Modify: `crates/export/Cargo.toml`
- Modify: `crates/export/src/metadata.rs` — `xmp_packet` (line 105), `embed` (line 149), tests

- [ ] **Step 1: Add chrono**

In `crates/export/Cargo.toml` `[dependencies]`, after `epaint_default_fonts = "0.35"`:

```toml
# ⚠ For `exif:DateTimeOriginal` alone — see `metadata::date_text`. Pure Rust, already in the
# tree via `app`. A hand-rolled civil-date conversion is a dozen lines and a thing to get
# silently wrong: the CRC-32 argument again.
chrono = "0.4.45"
```

- [ ] **Step 2: Rewrite the test helper and write the failing tests**

Replace the `meta` helper (top of `mod tests`) with:

```rust
    /// The three preset strings, resolved with no photo of their own — what every export before
    /// M6.2 wrote, so the pre-existing tests below keep their exact assertions.
    fn meta(copyright: &str, creator: &str, caption: &str) -> FileMetadata {
        FileMetadata::resolve(
            &ExportMetadata {
                copyright: copyright.into(),
                creator: creator.into(),
                caption: caption.into(),
                scope: MetadataScope::All,
            },
            FileMetadata::default(),
        )
    }
```

In `an_empty_metadata_writes_no_packet`, change `xmp_packet(&ExportMetadata::default())` to
`xmp_packet(&FileMetadata::default())`. Every other existing test already goes through `meta`
or `round_trip`, whose parameter type becomes `&FileMetadata` (Step 4).

Then add, after `copyright_only_keeps_rights_and_creator_and_nothing_else`:

```rust
    #[test]
    fn all_metadata_writes_each_property_once_where_other_software_looks() {
        let mut p = photo();
        p.capture = core_image::Capture {
            taken: Some(1_787_824_800), // 2026-08-27 10:00:00, naive wall-clock
            camera: Some("FUJIFILM X-T4".into()),
            lens: Some("XF23mmF2 R WR".into()),
            iso: Some(1600),
            shutter: Some(0.004),
            aperture: Some(5.6),
            focal_len: Some(23.0),
        };
        let x = xmp_packet(&FileMetadata::resolve(&defaults(), p)).unwrap();
        // ⚠ Asserted as `<property>…value…</property>`: the discriminating sabotage is swapping
        // two fields, which a bare `contains` cannot see.
        for want in [
            "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Harbour</rdf:li>",
            "<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">Boats</rdf:li>",
            "<dc:rights><rdf:Alt><rdf:li xml:lang=\"x-default\">(c) preset</rdf:li>",
            "<dc:creator><rdf:Seq><rdf:li>Preset Person</rdf:li></rdf:Seq></dc:creator>",
            "<xmp:Rating>4</xmp:Rating>",
            "<xmp:Label>Red</xmp:Label>",
            "<tiff:Model>FUJIFILM X-T4</tiff:Model>",
            "<aux:Lens>XF23mmF2 R WR</aux:Lens>",
            "<exif:DateTimeOriginal>2026-08-27T10:00:00</exif:DateTimeOriginal>",
            "<exif:ISOSpeedRatings><rdf:Seq><rdf:li>1600</rdf:li></rdf:Seq></exif:ISOSpeedRatings>",
            "<exif:ExposureTime>1/250</exif:ExposureTime>",
            "<exif:FNumber>56/10</exif:FNumber>",
            "<exif:FocalLength>230/10</exif:FocalLength>",
        ] {
            assert_eq!(x.matches(want).count(), 1, "{want} not exactly once in\n{x}");
        }
    }

    #[test]
    fn keywords_land_in_both_forms_lightroom_reads() {
        let x = xmp_packet(&FileMetadata::resolve(&defaults(), photo())).unwrap();
        assert!(x.contains("<rdf:li>Europe|UK</rdf:li>"), "no hierarchy:\n{x}");
        assert!(x.contains("<rdf:li>UK</rdf:li>"), "no leaf:\n{x}");
        assert!(x.contains("<rdf:li>Sea</rdf:li>"), "{x}");
        assert!(
            !x.contains("<rdf:li>Europe</rdf:li>"),
            "an interior level leaked into the flat bag:\n{x}"
        );
        assert_eq!(x.matches("<dc:subject>").count(), 1);
        assert_eq!(x.matches("<lr:hierarchicalSubject>").count(), 1);
    }

    #[test]
    fn a_shutter_below_a_second_is_a_unit_fraction_and_above_it_is_tenths() {
        // ⚠ `1/250` is what a photographer wrote; `4/1000` is not. Over a second, tenths.
        assert_eq!(exposure_rational(0.004), "1/250");
        assert_eq!(exposure_rational(0.5), "1/2");
        assert_eq!(exposure_rational(2.5), "25/10");
        assert_eq!(exposure_rational(30.0), "300/10");
        assert_eq!(tenths(5.6), "56/10");
        assert_eq!(tenths(1.8), "18/10");
    }

    #[test]
    fn an_exif_less_capture_writes_no_capture_property() {
        let mut p = photo();
        p.capture = core_image::Capture::default();
        let x = xmp_packet(&FileMetadata::resolve(&defaults(), p)).unwrap();
        for absent in ["tiff:", "exif:D", "exif:I", "exif:E", "exif:F", "aux:L"] {
            assert!(!x.contains(&format!("<{absent}")), "{absent} appeared:\n{x}");
        }
        // The Dublin Core half is still there.
        assert!(x.contains("<dc:title>"));
    }

    #[test]
    fn an_unrated_unlabelled_photo_writes_neither() {
        // ⚠ 0 is "unrated", which XMP spells as the property being **absent** — the rule the
        // sidecar already follows — so a reader cannot mistake it for a deliberate zero.
        let mut p = photo();
        p.rating = 0;
        p.label = None;
        let x = xmp_packet(&FileMetadata::resolve(&defaults(), p)).unwrap();
        assert!(!x.contains("xmp:Rating"), "{x}");
        assert!(!x.contains("xmp:Label"), "{x}");
    }

    #[test]
    fn a_date_is_naive_wall_clock_with_no_zone() {
        // ⚠ `Capture::taken` is naive wall-clock by rule; a `Z` or an offset here would claim a
        // zone the value never had.
        assert_eq!(date_text(1_787_824_800).as_deref(), Some("2026-08-27T10:00:00"));
        assert_eq!(date_text(0).as_deref(), Some("1970-01-01T00:00:00"));
    }
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p export 2>&1 | grep -E "^error" | head -3`
Expected: `xmp_packet` and `embed` still take `&ExportMetadata`; `exposure_rational`, `tenths`,
`date_text` do not exist.

- [ ] **Step 4: Rewrite `xmp_packet` and `embed`**

Replace the whole `xmp_packet` function (its doc comment through its closing brace, lines 96–140)
with:

```rust
/// A non-blank `s`, trimmed.
fn present(s: &str) -> Option<&str> {
    let t = s.trim();
    (!t.is_empty()).then_some(t)
}

/// `5.6` → `56/10`. ⚠ Not reduced: exiftool renders `56/10` as `5.6` and `230/10` as
/// `23.0 mm`; a reduced fraction would be prettier in the XML and identical everywhere anyone
/// looks.
fn tenths(x: f32) -> String {
    format!("{}/10", (x * 10.0).round() as i64)
}

/// An exposure time as a photographer wrote it: `1/250` below a second, tenths from one up.
fn exposure_rational(secs: f32) -> String {
    if secs < 1.0 {
        format!("1/{}", (1.0 / secs).round() as i64)
    } else {
        tenths(secs)
    }
}

/// `Capture::taken` as `YYYY-MM-DDThh:mm:ss`, no zone.
///
/// ⚠ No zone, and that is the point: the value is naive wall-clock by `Capture::taken`'s own
/// rule, so writing a `Z` or an offset would claim a zone the value never had.
fn date_text(secs: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string())
}

/// An `rdf:Bag` of `items` under `tag`.
fn bag(tag: &str, items: &[String]) -> String {
    let mut s = format!("   <{tag}><rdf:Bag>\n");
    for it in items {
        s.push_str(&format!("    <rdf:li>{}</rdf:li>\n", escape(it)));
    }
    s.push_str(&format!("   </rdf:Bag></{tag}>\n"));
    s
}

/// The XMP packet for `meta`, or `None` when there is nothing to say.
///
/// ⚠ Every property is one the Dublin Core, XMP, Lightroom, TIFF, EXIF or `aux` namespace
/// already names, so the values land where other software looks: `dc:rights` (exiftool
/// "Rights"), `dc:creator` ("Creator"), `dc:description` ("Description"), `dc:title` ("Title"),
/// `xmp:Rating`, `xmp:Label`, `dc:subject` and `lr:hierarchicalSubject` (Lightroom's two keyword
/// forms), `tiff:Model`, `aux:Lens`, `exif:DateTimeOriginal`, `exif:ISOSpeedRatings`,
/// `exif:ExposureTime`, `exif:FNumber`, `exif:FocalLength`.
///
/// ⚠ `rights`, `description` and `title` are language alternatives (`rdf:Alt` with `x-default`)
/// while `creator` and `ISOSpeedRatings` are sequences — those asymmetries are the schemas' own,
/// not a slip.
///
/// ⚠⚠ **`tiff:Model` carries the whole stored camera name and there is no `tiff:Make`.** The
/// catalog holds one normalised `"{MAKE} {model}"` string (`Capture::camera_name`), and the make
/// cannot be split back off it soundly — `NIKON CORPORATION NIKON Z 6`. Writing the name whole
/// is honest; inventing a Make is not. Spec §3's accepted caveat.
///
/// ⚠ The scope has already been applied by [`FileMetadata::resolve`]: this writes what is
/// present and knows nothing about `CopyrightOnly`.
pub fn xmp_packet(meta: &FileMetadata) -> Option<String> {
    let mut body = String::new();
    let alt = |tag: &str, v: &str| {
        format!(
            "   <{tag}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></{tag}>\n",
            escape(v)
        )
    };
    let seq = |tag: &str, v: &str| {
        format!(
            "   <{tag}><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></{tag}>\n",
            escape(v)
        )
    };
    let text = |tag: &str, v: &str| format!("   <{tag}>{}</{tag}>\n", escape(v));

    if let Some(t) = present(&meta.title) {
        body.push_str(&alt("dc:title", t));
    }
    if let Some(c) = present(&meta.caption) {
        body.push_str(&alt("dc:description", c));
    }
    if let Some(r) = present(&meta.copyright) {
        body.push_str(&alt("dc:rights", r));
    }
    if let Some(c) = present(&meta.creator) {
        body.push_str(&seq("dc:creator", c));
    }
    if meta.rating > 0 {
        body.push_str(&text("xmp:Rating", &meta.rating.to_string()));
    }
    if let Some(l) = meta.label.as_deref().and_then(present) {
        body.push_str(&text("xmp:Label", l));
    }
    if !meta.keywords.is_empty() {
        // ⚠ Both forms, Lightroom's own habit: the hierarchy in `lr:hierarchicalSubject`, and
        // each path's **leaf** in the flat `dc:subject` bag, which is all an application that
        // reads only the flat form can use. The same six lines live in `xmp::injection`; the
        // spec (§3) records why the two skeletons are not shared.
        let mut leaves: Vec<String> = Vec::new();
        for p in &meta.keywords {
            if let Some(leaf) = p.last() {
                if !leaves.contains(leaf) {
                    leaves.push(leaf.clone());
                }
            }
        }
        let paths: Vec<String> = meta.keywords.iter().map(|p| p.join("|")).collect();
        body.push_str(&bag("dc:subject", &leaves));
        body.push_str(&bag("lr:hierarchicalSubject", &paths));
    }

    let c = &meta.capture;
    let positive = |v: &f32| *v > 0.0 && v.is_finite();
    if let Some(m) = c.camera.as_deref().and_then(present) {
        body.push_str(&text("tiff:Model", m));
    }
    if let Some(l) = c.lens.as_deref().and_then(present) {
        body.push_str(&text("aux:Lens", l));
    }
    if let Some(d) = c.taken.and_then(date_text) {
        body.push_str(&text("exif:DateTimeOriginal", &d));
    }
    if let Some(i) = c.iso {
        body.push_str(&seq("exif:ISOSpeedRatings", &i.to_string()));
    }
    if let Some(s) = c.shutter.filter(positive) {
        body.push_str(&text("exif:ExposureTime", &exposure_rational(s)));
    }
    if let Some(a) = c.aperture.filter(positive) {
        body.push_str(&text("exif:FNumber", &tenths(a)));
    }
    if let Some(f) = c.focal_len.filter(positive) {
        body.push_str(&text("exif:FocalLength", &tenths(f)));
    }

    if body.is_empty() {
        return None;
    }
    Some(format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Rustroom\">\n\
         \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         \x20 <rdf:Description rdf:about=\"\"\n\
         \x20   xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
         \x20   xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
         \x20   xmlns:lr=\"http://ns.adobe.com/lightroom/1.0/\"\n\
         \x20   xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\"\n\
         \x20   xmlns:exif=\"http://ns.adobe.com/exif/1.0/\"\n\
         \x20   xmlns:aux=\"http://ns.adobe.com/exif/1.0/aux/\">\n\
         {body}\
         \x20 </rdf:Description>\n\
         \x20</rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end=\"w\"?>"
    ))
}
```

Change `embed`'s signature to:

```rust
pub fn embed(path: &Path, format: ExportFormat, meta: &FileMetadata) -> Result<(), ExportError> {
```

and its doc's last paragraph to *"A no-op when the packet would be empty — including no read and
no write — so an export with nothing to say is byte-for-byte what it was before this existed."*
The body is unchanged.

Also update the module doc's second paragraph (line 11–13) to read:

```rust
//! ⚠ Since M6.2 the per-photo half of these fields lives in the catalog (`Description`) and
//! reaches this crate as a [`FileMetadata`]; the three preset strings are its defaults. Nothing
//! is ever read back from an export, and re-exporting is what changes them.
```

- [ ] **Step 5: Update `round_trip` and `every_format_carries_the_packet_and_still_decodes`**

`round_trip`'s parameter becomes `m: &FileMetadata`. In
`every_format_carries_the_packet_and_still_decodes`, replace `let m = meta("(c) 2026 Rustroom", "Wladi", "Probe");`
with the **full** payload, so the container tests run against the biggest packet this crate writes:

```rust
        let mut p = photo();
        p.capture.taken = Some(1_787_824_800);
        p.capture.shutter = Some(0.004);
        let m = FileMetadata::resolve(&defaults(), p);
```

Any other test calling `embed(&path, fmt, &meta(...))` or `xmp_packet(&meta(...))` compiles as is,
since `meta` now returns a `FileMetadata`.

- [ ] **Step 6: Run**

Run: `cargo test -p export 2>&1 | tail -3`
Expected: `test result: ok.` with every pre-existing metadata test still passing.

- [ ] **Step 7: Commit**

```bash
git add crates/export/Cargo.toml crates/export/src/metadata.rs Cargo.lock
git commit -m "Export: the full packet — description, marks, keywords and capture facts"
```

---

### Task 6: The worker carries the payload, and a caption reaches the file

**Files:**
- Modify: `crates/app/src/batch_export.rs` — `ExportJob` (line 30), `export_one` (line 323), `run`'s destructure (~line 456) and call (~line 560), tests (`exported_with` ~line 745, `jobs` ~line 896, `jobs_with` ~line 917, every `export_one(` call)
- Modify: `crates/app/examples/deliverable.rs` (line 75)

- [ ] **Step 1: Write the failing tests**

In `mod tests` of `batch_export.rs`, after `an_export_carries_its_metadata`:

```rust
    #[test]
    fn a_photos_own_caption_reaches_the_file_and_beats_the_presets() {
        // ⚠⚠ The carrier test W5 taught: the packet writer is thoroughly tested in its own
        // crate, and nothing there proves the worker builds the payload from the job. Deleting
        // the `file_metadata` call leaves `export` green and this red.
        let d = TempDir::new("e2e_desc");
        let mut opts = opts(&d, None);
        opts.format = ExportFormat::Tiff8;
        opts.metadata.caption = "preset caption".into();
        opts.metadata.copyright = "(c) preset".into();
        let mut job = jobs(&d, &["one.RAF"]).remove(0);
        job.description = catalog::db::Description {
            title: "Harbour at dusk".into(),
            caption: "Boats returning".into(),
            ..Default::default()
        };
        job.marks = catalog::db::Marks {
            rating: 4,
            label: 2,
            ..Default::default()
        };
        job.keywords = vec![vec!["Europe".into(), "UK".into()]];
        let (report, _) = run_export(vec![job], opts, Vec::new(), ok_decode());
        assert_eq!(report.exported, 1, "{:?}", report.failed);
        let bytes = std::fs::read(d.0.join("one.tif")).unwrap();
        let has = |s: &str| bytes.windows(s.len()).any(|w| w == s.as_bytes());
        assert!(has("Harbour at dusk"), "the title never reached the file");
        assert!(has("Boats returning"), "the photo's caption never reached the file");
        assert!(!has("preset caption"), "the preset's caption overrode the photo's");
        assert!(has("(c) preset"), "the preset's copyright did not fill the blank");
        assert!(has("<xmp:Rating>4</xmp:Rating>"), "the rating is missing");
        assert!(has("<xmp:Label>Blue</xmp:Label>"), "label 2 is Blue");
        assert!(has("Europe|UK"), "the keyword path is missing");
    }

    #[test]
    fn copyright_only_strips_the_photos_own_words_at_export() {
        let d = TempDir::new("e2e_scope");
        let mut opts = opts(&d, None);
        opts.format = ExportFormat::Tiff8;
        opts.metadata.copyright = "(c) preset".into();
        opts.metadata.scope = export::MetadataScope::CopyrightOnly;
        let mut job = jobs(&d, &["one.RAF"]).remove(0);
        job.description.caption = "Boats returning".into();
        job.marks.rating = 5;
        let (report, _) = run_export(vec![job], opts, Vec::new(), ok_decode());
        assert_eq!(report.exported, 1);
        let bytes = std::fs::read(d.0.join("one.tif")).unwrap();
        let has = |s: &str| bytes.windows(s.len()).any(|w| w == s.as_bytes());
        assert!(has("(c) preset"));
        assert!(!has("Boats returning"), "the scope was not applied");
        assert!(!has("xmp:Rating"), "the scope was not applied to the rating");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app reaches_the_file 2>&1 | grep -E "^error" | head -3`
Expected: `no field description on ExportJob`.

- [ ] **Step 3: Grow `ExportJob` and `export_one`, build the payload in `run`**

`ExportJob` (line 30) gains three fields after `capture`:

```rust
    /// What the photo says about itself, from the catalog. ⚠ Carried for the same reason as
    /// `capture`: the worker has no catalog connection. Per **file**.
    pub description: catalog::db::Description,
    /// This copy's marks — the rating and label the export carries. Per **copy**.
    pub marks: catalog::db::Marks,
    /// The file's keyword paths, root first, from `xmp_sync::keyword_paths`.
    pub keywords: Vec<Vec<String>>,
```

Add, directly above `pub fn export_one`:

```rust
/// The unresolved payload a job carries, in the export crate's terms.
///
/// ⚠ This is the one place a catalog `Description`, `Marks` and keyword paths become the plain
/// `FileMetadata` the packet is written from. `FileMetadata::resolve` then lays the preset's
/// defaults under it and applies the scope.
pub fn file_metadata(job: &ExportJob) -> export::FileMetadata {
    export::FileMetadata {
        title: job.description.title.clone(),
        caption: job.description.caption.clone(),
        copyright: job.description.copyright.clone(),
        creator: job.description.creator.clone(),
        rating: job.marks.rating,
        label: crate::xmp_sync::label_to_xmp(job.marks.label).map(str::to_string),
        keywords: job.keywords.clone(),
        capture: job.capture.clone(),
    }
}
```

`export_one` gains a last parameter, `meta: &export::FileMetadata`, and its last line becomes:

```rust
    export::metadata::embed(dest, opts.format, meta).map_err(|e| e.to_string())
```

In `run`, the destructure (~line 456) becomes:

```rust
        ExportJob {
            id,
            name,
            edits,
            capture,
            description,
            marks,
            keywords,
        },
```

Immediately after `let source = id.path.clone();` add:

```rust
        let meta = export::FileMetadata::resolve(
            &opts.metadata,
            file_metadata(&ExportJob {
                id: id.clone(),
                name: name.clone(),
                edits: edits.clone(),
                capture: capture.clone(),
                description,
                marks,
                keywords,
            }),
        );
```

⚠ That rebuilds the job only to hand it to `file_metadata`; it is the cheapest correct shape
without restructuring the loop, and cloning `Edits` once per exported file is nothing beside a
decode. If the loop's `for (done, ExportJob { … }) in jobs.into_iter().enumerate()` is instead
changed to bind the whole job (`for (done, job) in …` then `let ExportJob { id, name, edits,
capture, .. } = &job;`) the clone goes away — either is acceptable; the test does not care.

The call at ~line 560 becomes:

```rust
            .and_then(|(img, baseline)| {
                export_one(img, &edits, &dest, &opts, resampler, baseline, &meta)
            })
```

- [ ] **Step 4: Fix every other `export_one` call and the two job helpers**

Test helpers `jobs` and `jobs_with`: add to each `ExportJob { … }` literal

```rust
                description: Default::default(),
                marks: Default::default(),
                keywords: Vec::new(),
```

Every test call `export_one(<image>, &Edits::default(), &<dest>, &<opts>, pipeline::Resampler::default(), color::wb::Illuminant::default())`
(lines ~673, 689, 706, 833) gains a seventh argument `&export::FileMetadata::default()`.

In `exported_with` (~line 745 and ~757), the two calls gain, respectively:

```rust
            &export::FileMetadata::resolve(&base.metadata, export::FileMetadata::default()),
```
and
```rust
            &export::FileMetadata::resolve(&changed.metadata, export::FileMetadata::default()),
```

`crates/app/examples/deliverable.rs` line 75: add a seventh argument
`&export::FileMetadata::resolve(&o.metadata, export::FileMetadata::default()),`.

- [ ] **Step 5: Run**

Run: `cargo test -p app batch_export 2>&1 | tail -3 && cargo build --examples -p app 2>&1 | tail -1`
Expected: `test result: ok.`; the example builds. `an_export_carries_its_metadata` still passes.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/batch_export.rs crates/app/examples/deliverable.rs
git commit -m "Export worker: the job carries description, marks and keywords to the packet"
```

---

### Task 7: `export_jobs` gathers the payload from the catalog

**Files:**
- Modify: `crates/app/src/export_flow.rs` (lines 25–60)

⚠ A bin module with no headless test of its own; `export_jobs` reads `self.library`, which the
existing tests build through `test_app::library_of`. The proof that the data flows is the manual
done-when in Task 13 plus the two carrier tests already written. Keep this task to the gather.

- [ ] **Step 1: Fill the fields**

In the single-file branch (`.map(|(id, edits)| ExportJob { … })`), add:

```rust
                    description: Default::default(),
                    marks: Default::default(),
                    keywords: Vec::new(),
```

In the Library branch, after `let capture = …unwrap_or_default();` add:

```rust
                // ⚠ Per **file**, like `capture`, and read here rather than held on `Library`:
                // nothing reads descriptions per frame, so a per-folder map would be machinery
                // for a consumer that does not exist (spec §1). One query per selected photo.
                let description = self
                    .catalog
                    .as_ref()
                    .and_then(|c| c.description_of(&id.path).ok().flatten())
                    .unwrap_or_default();
                // ⚠ Per **copy** — the rating and label of *this* version.
                let marks = lib.marks.get(&id).copied().unwrap_or_default();
                let keywords = self
                    .catalog
                    .as_ref()
                    .map(|c| rustroom_app::xmp_sync::keyword_paths(c, &id.path))
                    .unwrap_or_default();
```

and add `description, marks, keywords,` to the `ExportJob { … }` literal after `capture,`.

- [ ] **Step 2: Build and run the app tests**

Run: `cargo test -p app 2>&1 | tail -3`
Expected: `test result: ok.`

- [ ] **Step 3: Commit**

```bash
git add crates/app/src/export_flow.rs
git commit -m "Export flow: each job carries its description, marks and keyword paths"
```

---

### Task 8: The scope in the export dialog, and the header dot

**Files:**
- Modify: `crates/app/src/modals/export.rs` (lines 59–85, tests ~line 640)
- Modify: `docs/superpowers/specs/2026-09-08-metadata-writing-design.md` §3 (the dot sentence)

- [ ] **Step 1: Write the failing test**

After `a_section_that_is_doing_something_says_so_in_its_header`:

```rust
    #[test]
    fn a_narrowed_scope_marks_the_metadata_header() {
        // ⚠ `All` is the default and therefore not hidden state; `CopyrightOnly` is a choice
        // someone made and forgot, which is exactly what the marker exists for.
        let narrowed = with_app(|app| {
            app.export_draft.metadata.scope = export::MetadataScope::CopyrightOnly;
        });
        assert!(
            narrowed
                .iter()
                .any(|l| l.starts_with("Metadata") && l.len() > "Metadata".len()),
            "a changed scope must show in the header: {narrowed:?}"
        );
        let idle = with_app(|_| {});
        assert!(
            idle.iter().any(|l| l == "Metadata"),
            "All is the default and must carry no marker: {idle:?}"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p app narrowed_scope 2>&1 | tail -5`
Expected: FAIL — the header reads `Metadata` with no marker.

- [ ] **Step 3: Wire the scope**

In `delivery_sections`, change the header line to:

```rust
    egui::CollapsingHeader::new(format!("Metadata{}", on(opts.metadata.is_active())))
```

After the Caption row (`ui.end_row();` following `text_edit_singleline(&mut opts.metadata.caption)`), add:

```rust
                    ui.label("Include");
                    ui.horizontal(|ui| {
                        for s in [
                            export::MetadataScope::All,
                            export::MetadataScope::CopyrightOnly,
                        ] {
                            ui.selectable_value(&mut opts.metadata.scope, s, s.label());
                        }
                    });
                    ui.end_row();
```

Replace the hint text with:

```rust
            theme::hint(
                ui,
                "Written into the exported file as XMP. The original is never modified. \
                 A photo's own title, caption, copyright and creator (Information dialog) win \
                 over the fields above; leave a field empty to write nothing for it. \
                 \"All metadata\" also writes the camera, exposure, keywords and rating.",
            );
```

- [ ] **Step 4: Correct the spec sentence**

In the spec's §3, replace *"the section header's "doing something" dot shows for `All` always,
and for `CopyrightOnly` when a field is filled"* with:

> the section header's "doing something" dot shows when a field is filled **or the scope is not
> the default** — `All` is the default and therefore not hidden state, which is the rule
> `a_section_that_is_doing_something_says_so_in_its_header` guards. *(Corrected while planning.)*

- [ ] **Step 5: Run**

Run: `cargo test -p app modals::export 2>&1 | tail -3`
Expected: `test result: ok.`

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/modals/export.rs docs/superpowers/specs/2026-09-08-metadata-writing-design.md
git commit -m "Export dialog: choose All metadata or Copyright only"
```

---

### Task 9: The `xmp` crate owns `dc:title`, `dc:description`, `dc:rights`, `dc:creator`

**Files:**
- Modify: `crates/xmp/src/lib.rs` — `Sidecar` (line 14), `Owned` (line 60), `owned` (line 75), `description_attributes` (~line 165), `read` (~line 195), `injection` (~line 430), tests (`sample` ~line 743, `a_keyword_with_markup_characters_is_escaped` ~line 794)

- [ ] **Step 1: Write the failing tests**

In `mod tests`, after `an_entity_in_a_keyword_is_unescaped`:

```rust
    /// The four description fields the way Lightroom writes them — as elements — beside a
    /// develop setting that must survive.
    const DESCRIBED: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:Exposure2012="+0.50">
   <dc:title>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Harbour at dusk</rdf:li>
    </rdf:Alt>
   </dc:title>
   <dc:description>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Boats &amp; nets</rdf:li>
    </rdf:Alt>
   </dc:description>
   <dc:rights>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">© 2026 W</rdf:li>
    </rdf:Alt>
   </dc:rights>
   <dc:creator>
    <rdf:Seq>
     <rdf:li>W</rdf:li>
     <rdf:li>Second Credit</rdf:li>
    </rdf:Seq>
   </dc:creator>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn the_four_description_fields_read_from_elements() {
        let s = read(DESCRIBED).unwrap();
        assert_eq!(s.title.as_deref(), Some("Harbour at dusk"));
        assert_eq!(s.caption.as_deref(), Some("Boats & nets"), "entity not unescaped");
        assert_eq!(s.rights.as_deref(), Some("© 2026 W"));
        // ⚠ A multi-entry creator reads as its **first**: the catalog holds one creator string
        // and a co-credit is not a field Rustroom has. Spec §4's accepted caveat.
        assert_eq!(s.creator.as_deref(), Some("W"));
    }

    #[test]
    fn a_description_field_reads_from_an_attribute() {
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
   dc:title="Attr title" dc:rights="Attr rights"/>
 </rdf:RDF>
</x:xmpmeta>"#;
        let s = read(text).unwrap();
        assert_eq!(s.title.as_deref(), Some("Attr title"));
        assert_eq!(s.rights.as_deref(), Some("Attr rights"));
        assert_eq!(s.caption, None);
    }

    #[test]
    fn a_fresh_packet_writes_the_description_fields_and_omits_unset_ones() {
        let s = Sidecar {
            title: Some("T".into()),
            caption: None,
            rights: Some("R & Co".into()),
            creator: Some("C".into()),
            ..Sidecar::default()
        };
        let text = write(&s, None).unwrap();
        assert!(text.contains("<dc:title"), "{text}");
        assert!(!text.contains("dc:description"), "{text}");
        assert!(text.contains("R &amp; Co"), "{text}");
        assert!(text.contains("<rdf:Seq"), "creator is a sequence:\n{text}");
        assert_eq!(read(&text).unwrap(), s, "did not survive the round trip");
    }

    #[test]
    fn merging_replaces_a_foreign_title_and_keeps_every_other_byte() {
        let ours = Sidecar {
            title: Some("Our title".into()),
            caption: None,
            ..Sidecar::default()
        };
        let out = write(&ours, Some(DESCRIBED)).unwrap();
        assert!(out.contains(r#"crs:Exposure2012="+0.50""#), "develop setting lost:\n{out}");
        let back = read(&out).unwrap();
        assert_eq!(back.title.as_deref(), Some("Our title"));
        // ⚠ Owned fields we do not set are **removed**, not kept: the catalog holds the truth
        // (a newer sidecar imported before we ever wrote), and this is what lets a caption
        // deleted in Rustroom disappear from Lightroom — the rule the rating already follows.
        assert_eq!(back.caption, None, "a stale caption survived the merge:\n{out}");
        assert_eq!(back.rights, None);
        assert_eq!(back.creator, None);
        assert_eq!(out.matches("<dc:title").count(), 1, "a second title appeared:\n{out}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p xmp description 2>&1 | grep -E "^error" | head -3`
Expected: `no field title on Sidecar`.

- [ ] **Step 3: Grow `Sidecar` and `Owned`**

`Sidecar` gains, after `keywords`:

```rust
    /// `dc:title`, the `x-default` alternative.
    pub title: Option<String>,
    /// `dc:description` — what the app calls the caption.
    pub caption: Option<String>,
    /// `dc:rights` — what the app calls the copyright.
    pub rights: Option<String>,
    /// `dc:creator`. ⚠ A multi-entry sequence reads as its **first** and is written back as one:
    /// the catalog holds one creator string, and a co-credit is not a field Rustroom has.
    pub creator: Option<String>,
```

Update the module doc's first line to *"rating, colour label, keywords and the four description
fields"* and the `Owned` doc to *"The eight fields this crate owns"*. `Owned` gains:

```rust
    Title,
    Description,
    Rights,
    Creator,
```

`owned` gains four arms before `_ => None`:

```rust
        (NS_DC, b"title") => Some(Owned::Title),
        (NS_DC, b"description") => Some(Owned::Description),
        (NS_DC, b"rights") => Some(Owned::Rights),
        (NS_DC, b"creator") => Some(Owned::Creator),
```

- [ ] **Step 4: Read them**

Add a helper above `description_attributes`:

```rust
/// Set `slot` to `t` unless it already holds a value — the first `rdf:li` of an `Alt` or `Seq`
/// wins, and an attribute seen earlier is not overwritten by an element seen later.
fn first(slot: &mut Option<String>, t: String) {
    if slot.is_none() {
        *slot = Some(t);
    }
}
```

In `description_attributes`, the `match owned(&ns, local.as_ref())` gains:

```rust
            Some(Owned::Title) => first(&mut out.title, value()?.into_owned()),
            Some(Owned::Description) => first(&mut out.caption, value()?.into_owned()),
            Some(Owned::Rights) => first(&mut out.rights, value()?.into_owned()),
            Some(Owned::Creator) => first(&mut out.creator, value()?.into_owned()),
```

In `read`, replace the `Kind::Li if bag.is_some() => { … }` arm with:

```rust
                    Kind::Li if bag.is_some() => {
                        let t = text_of(&mut reader, &e)?;
                        match bag {
                            Some(Owned::Hierarchical) => push_unique(&mut hier, split_path(&t)),
                            Some(Owned::Subject) => push_unique(
                                &mut flat,
                                split_path(&t).into_iter().take(1).collect(),
                            ),
                            Some(Owned::Title) => first(&mut out.title, t),
                            Some(Owned::Description) => first(&mut out.caption, t),
                            Some(Owned::Rights) => first(&mut out.rights, t),
                            Some(Owned::Creator) => first(&mut out.creator, t),
                            Some(Owned::Rating) | Some(Owned::Label) | None => {}
                        }
                    }
```

(The `Kind::Owned(o) => { bag = Some(o); depth += 1; }` arm already routes the four new element
forms into `bag`, since they are neither `Rating` nor `Label`.)

- [ ] **Step 5: Write them**

Add two helpers after `bag`:

```rust
const NS_DC_STR: &str = "http://purl.org/dc/elements/1.1/";

/// A language-alternative property with one `x-default` entry — `dc:title`, `dc:description`,
/// `dc:rights`.
fn lang_alt(tag: &str, v: &str) -> String {
    format!(
        "   <{tag} xmlns:dc=\"{NS_DC_STR}\"><rdf:Alt xmlns:rdf=\"{NS_RDF_STR}\">\
         <rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></{tag}>\n",
        esc(v)
    )
}

/// A sequence with one entry — `dc:creator`.
fn seq_one(tag: &str, v: &str) -> String {
    format!(
        "   <{tag} xmlns:dc=\"{NS_DC_STR}\"><rdf:Seq xmlns:rdf=\"{NS_RDF_STR}\">\
         <rdf:li>{}</rdf:li></rdf:Seq></{tag}>\n",
        esc(v)
    )
}
```

In `injection`, after the `label` block and before the keywords block:

```rust
    if let Some(t) = &s.title {
        out.push_str(&lang_alt("dc:title", t));
    }
    if let Some(c) = &s.caption {
        out.push_str(&lang_alt("dc:description", c));
    }
    if let Some(r) = &s.rights {
        out.push_str(&lang_alt("dc:rights", r));
    }
    if let Some(c) = &s.creator {
        out.push_str(&seq_one("dc:creator", c));
    }
```

- [ ] **Step 6: Fix the two full struct literals in the tests**

`sample()` and `a_keyword_with_markup_characters_is_escaped` build `Sidecar { rating, label,
keywords }` — add `..Sidecar::default()` as the last entry of each.

- [ ] **Step 7: Run**

Run: `cargo test -p xmp 2>&1 | tail -3`
Expected: `test result: ok.`; `merging_keeps_every_foreign_byte_and_changes_only_the_rating` and
`an_unset_field_is_absent_from_a_fresh_packet` still pass.

- [ ] **Step 8: Commit**

```bash
git add crates/xmp/src/lib.rs
git commit -m "xmp: own dc:title, dc:description, dc:rights and dc:creator"
```

---

### Task 10: The sidecar carries the description both ways

**Files:**
- Modify: `crates/app/src/xmp_sync.rs` — module doc (line 3), `sidecar_of` (line 120), `apply` (line 195), tests
- Modify: `crates/app/src/settings.rs` (~line 342)

- [ ] **Step 1: Write the failing tests**

In `mod tests` of `xmp_sync.rs`, after `writing_then_reading_round_trips_through_the_catalog`:

```rust
    #[test]
    fn a_description_reaches_the_sidecar_and_comes_back() {
        let (_d, cat, photo) = one_raw("desc");
        let d = catalog::db::Description {
            title: "Harbour".into(),
            caption: "Boats".into(),
            copyright: "© W".into(),
            creator: "W".into(),
        };
        cat.set_description(&photo, &d).unwrap();
        assert!(write_for(&cat, &photo));
        let text = std::fs::read_to_string(sidecar_path(&photo)).unwrap();
        let back = xmp::read(&text).unwrap();
        assert_eq!(back.title.as_deref(), Some("Harbour"));
        assert_eq!(back.caption.as_deref(), Some("Boats"));
        assert_eq!(back.rights.as_deref(), Some("© W"));
        assert_eq!(back.creator.as_deref(), Some("W"));

        // And the other direction, from a fresh catalog.
        let (_d2, cat2, photo2) = one_raw("desc_back");
        apply(&cat2, &photo2, &back);
        assert_eq!(cat2.description_of(&photo2).unwrap(), Some(d));
    }

    #[test]
    fn an_absent_description_field_clears_the_catalogs() {
        // ⚠ The rating's rule, one field over: *no* `dc:description` in a newer sidecar means
        // the caption was removed elsewhere, and that must reach us.
        let (_d, cat, photo) = one_raw("desc_clear");
        cat.set_description(
            &photo,
            &catalog::db::Description {
                caption: "stale".into(),
                ..Default::default()
            },
        )
        .unwrap();
        apply(&cat, &photo, &xmp::Sidecar::default());
        assert_eq!(cat.description_of(&photo).unwrap(), None);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app xmp_sync::tests::a_description 2>&1 | tail -5`
Expected: FAIL — `back.title` is `None`.

- [ ] **Step 3: Fill `sidecar_of` and `apply`**

`sidecar_of` becomes:

```rust
pub fn sidecar_of(cat: &Catalog, photo: &Path) -> xmp::Sidecar {
    let m = cat.marks(photo, 0).unwrap_or_default();
    let d = cat.description_of(photo).ok().flatten().unwrap_or_default();
    // ⚠ Blank means absent, so a sidecar never carries an empty `dc:title`.
    let field = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    xmp::Sidecar {
        // ⚠ `0` is "unrated", which XMP spells as the property being **absent** rather than as
        // `xmp:Rating` 0 — so an unrated photo's sidecar carries no rating and a reader cannot
        // mistake it for a deliberate zero.
        rating: (m.rating > 0).then_some(m.rating),
        label: label_to_xmp(m.label).map(str::to_string),
        keywords: keyword_paths(cat, photo),
        title: field(&d.title),
        caption: field(&d.caption),
        rights: field(&d.copyright),
        creator: field(&d.creator),
    }
}
```

In `apply`, after the `cat.set_marks(…)` call and before the keyword loop:

```rust
    // ⚠ Absent means cleared, the rating's rule: `set_description` deletes the row when all
    // four are blank, so a sidecar with no description leaves no row behind.
    let _ = cat.set_description(
        photo,
        &catalog::db::Description {
            title: s.title.clone().unwrap_or_default(),
            caption: s.caption.clone().unwrap_or_default(),
            copyright: s.rights.clone().unwrap_or_default(),
            creator: s.creator.clone().unwrap_or_default(),
        },
    );
```

Update the module doc line 3 to *"Ratings, colour labels, keywords and the four description
fields are what Rustroom shares with Lightroom."* and `apply`'s doc to *"Put a sidecar's rating,
label, keywords and description into the catalog, for copy 0 (the description is per file)."*

- [ ] **Step 4: Fix the full `Sidecar` literals in this file's tests**

`importing_sets_the_rating_and_the_label` and `a_label_outside_the_four_leaves_the_catalog_value_alone`
build `xmp::Sidecar { rating, label, keywords: Vec::new() }` — add `..xmp::Sidecar::default()`.
Run `cargo test -p app xmp_sync 2>&1 | grep -E "^error" ` to find any other.

- [ ] **Step 5: The settings sentence**

In `crates/app/src/settings.rs`, the `write_xmp` doc's first sentence becomes:

```rust
    /// Write a `stem.xmp` sidecar beside every RAW when its rating, colour label, keywords or
    /// description change, so Lightroom and exiftool can read them.
```

- [ ] **Step 6: Run**

Run: `cargo test -p app xmp_sync 2>&1 | tail -3`
Expected: `test result: ok.`

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/xmp_sync.rs crates/app/src/settings.rs
git commit -m "XMP sync: the description rides the sidecar in both directions"
```

---

### Task 11: `description.rs` — the draft and its commit

**Files:**
- Create: `crates/app/src/description.rs`
- Modify: `crates/app/src/lib.rs` (add `pub mod description;` between `decode_worker` and `edit_clipboard`)

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/description.rs`:

```rust
//! The Information dialog's editable description: a draft, and the one rule about committing it.
//!
//! ⚠ No egui here — a lib module, so the commit rule is testable without a window. The dialog
//! (`modals/info.rs`) owns the fields; this owns *when a write happens*.

use catalog::db::{Catalog, Description};
use std::path::Path;

/// What the dialog opened with, and what it holds now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DescriptionDraft {
    pub original: Description,
    pub edited: Description,
}

impl DescriptionDraft {
    /// Read `photo`'s description once, for the dialog to edit.
    ///
    /// ⚠ Called on the dialog's first frame and cached in the modal — a catalog read in a body
    /// that runs sixty times a second is the `apply_view_demosaic` mistake.
    pub fn open(cat: Option<&Catalog>, photo: &Path) -> Self {
        let original = cat
            .and_then(|c| c.description_of(photo).ok().flatten())
            .unwrap_or_default();
        Self {
            edited: original.clone(),
            original,
        }
    }

    pub fn changed(&self) -> bool {
        self.edited != self.original
    }
}

/// Write the draft if it changed. Returns whether anything was written.
///
/// ⚠⚠ **Only when changed**, and the reason is the sidecar: the caller rewrites `stem.xmp` on
/// `true`, and *"rewriting the same bytes moves the modification time, which is the one value
/// the whole conflict rule turns on"* (`apply_mark_to`). A dialog opened to look and closed
/// unchanged must leave the file alone.
pub fn commit(cat: &Catalog, photo: &Path, draft: &DescriptionDraft) -> bool {
    if !draft.changed() {
        return false;
    }
    cat.set_description(photo, &draft.edited).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn catalogued() -> (Catalog, PathBuf) {
        let cat = Catalog::open_in_memory().unwrap();
        let photo = PathBuf::from("/a/one.RAF");
        cat.sync_folder(
            Path::new("/a"),
            &[catalog::CatalogEntry {
                path: photo.clone(),
                file_name: "one.RAF".into(),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            }],
        )
        .unwrap();
        (cat, photo)
    }

    #[test]
    fn opening_reads_what_the_catalog_holds_and_nothing_for_a_fresh_photo() {
        let (cat, photo) = catalogued();
        let fresh = DescriptionDraft::open(Some(&cat), &photo);
        assert_eq!(fresh, DescriptionDraft::default());
        assert!(!fresh.changed());

        cat.set_description(
            &photo,
            &Description {
                title: "Stored".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let d = DescriptionDraft::open(Some(&cat), &photo);
        assert_eq!(d.original.title, "Stored");
        assert_eq!(d.edited, d.original);
        // No catalog at all (single-file mode) is an empty draft, not a panic.
        assert_eq!(DescriptionDraft::open(None, &photo), DescriptionDraft::default());
    }

    #[test]
    fn commit_writes_only_when_the_draft_changed() {
        let (cat, photo) = catalogued();
        let unchanged = DescriptionDraft::open(Some(&cat), &photo);
        assert!(!commit(&cat, &photo, &unchanged), "an untouched draft wrote");
        assert_eq!(cat.description_of(&photo).unwrap(), None);

        let mut d = unchanged;
        d.edited.caption = "Boats returning".into();
        assert!(commit(&cat, &photo, &d));
        assert_eq!(
            cat.description_of(&photo).unwrap().unwrap().caption,
            "Boats returning"
        );
    }

    #[test]
    fn clearing_every_field_commits_as_a_removal() {
        let (cat, photo) = catalogued();
        cat.set_description(
            &photo,
            &Description {
                caption: "old".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut d = DescriptionDraft::open(Some(&cat), &photo);
        d.edited.caption.clear();
        assert!(d.changed());
        assert!(commit(&cat, &photo, &d));
        assert_eq!(cat.description_of(&photo).unwrap(), None);
    }
}
```

- [ ] **Step 2: Register the module and run**

Add `pub mod description;` to `crates/app/src/lib.rs` in alphabetical position (after
`pub mod decode_worker;`).

Run: `cargo test -p app description:: 2>&1 | tail -3`
Expected: `test result: ok.` (3 passed). ⚠ These pass on first run because the implementation
and tests were written together in one file; the discriminating check is the next step.

- [ ] **Step 3: Sabotage check**

Temporarily change `if !draft.changed()` to `if false` and run the same command: expected
`commit_writes_only_when_the_draft_changed` FAILS with "an untouched draft wrote". Revert.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/description.rs crates/app/src/lib.rs
git commit -m "description: a draft for the Information dialog, committed only when changed"
```

---

### Task 12: The Information dialog edits the description and commits on close

**Files:**
- Modify: `crates/app/src/modals/mod.rs` (`PhotoInfo` at line 155)
- Modify: `crates/app/src/library.rs` (line 211)
- Modify: `crates/app/src/modals/info.rs` (lines 80–100, ~200–245, tests)

- [ ] **Step 1: Write the failing tests**

In `mod tests` of `info.rs`:

```rust
    use crate::modals::Modal;
    use catalog::db::Description;
    use rustroom_app::description::DescriptionDraft;

    fn app_with_one_catalogued_photo() -> (crate::RustroomApp, crate::PhotoId) {
        let mut app = crate::test_app::for_test();
        app.view = crate::AppView::Library;
        app.library = Some(crate::test_app::library_of(1));
        // ⚠ Without this `set_description` is a no-op on an uncatalogued photo and every
        // assertion below fails against correct code — see `catalog_library`.
        crate::test_app::catalog_library(&mut app);
        let id = app.library.as_ref().unwrap().entries[0].id.clone();
        (app, id)
    }

    /// ⚠⚠ The dialog-to-catalog carrier: every M5.3b defect was in dialog and panel wiring and
    /// none was caught by a test before it was reported. The draft is set directly — typing into
    /// an egui `TextEdit` is egui's to prove — and the **close** is a real click, because the
    /// commit-on-close is the wiring that would be forgotten.
    #[test]
    fn closing_the_dialog_commits_a_changed_description() {
        let ctx = crate::test_ui::ctx();
        let (mut app, id) = app_with_one_catalogued_photo();
        app.modal = Modal::PhotoInfo {
            id: id.clone(),
            dims: Some((0, 0)),
            description: Some(DescriptionDraft {
                original: Description::default(),
                edited: Description {
                    caption: "Dusk over the harbour".into(),
                    ..Default::default()
                },
            }),
        };
        crate::test_ui::click(&ctx, "Close", |_| show(&mut app, &ctx));
        assert!(app.modal == Modal::None, "the dialog did not close");
        let got = app
            .catalog
            .as_ref()
            .unwrap()
            .description_of(&id.path)
            .unwrap()
            .expect("the caption never reached the catalog");
        assert_eq!(got.caption, "Dusk over the harbour");
    }

    #[test]
    fn closing_an_untouched_dialog_writes_nothing() {
        let ctx = crate::test_ui::ctx();
        let (mut app, id) = app_with_one_catalogued_photo();
        app.modal = Modal::PhotoInfo {
            id: id.clone(),
            dims: Some((0, 0)),
            description: None,
        };
        // One frame fills the draft from the catalog (empty); the click then closes it.
        crate::test_ui::click(&ctx, "Close", |_| show(&mut app, &ctx));
        assert!(app.modal == Modal::None);
        assert_eq!(
            app.catalog.as_ref().unwrap().description_of(&id.path).unwrap(),
            None,
            "an unchanged draft produced a row"
        );
    }

    #[test]
    fn the_description_fields_are_offered() {
        let ctx = crate::test_ui::ctx();
        let (mut app, id) = app_with_one_catalogued_photo();
        app.modal = Modal::PhotoInfo {
            id,
            dims: Some((0, 0)),
            description: None,
        };
        let drawn: Vec<String> =
            crate::test_ui::labelled_centres(&ctx, Vec::new(), |_| show(&mut app, &ctx))
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        // ⚠ A `TextEdit` produces an AccessKit node; its *label* is drawn as a plain label,
        // which does not. So what is checked is that the dialog still offers Close and has
        // grown, not the field captions themselves — stated rather than papered over.
        assert!(drawn.iter().any(|l| l == "Close"), "{drawn:?}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app modals::info 2>&1 | grep -E "^error" | head -3`
Expected: `variant Modal::PhotoInfo has no field named description`.

- [ ] **Step 3: The draft in the modal**

In `crates/app/src/modals/mod.rs`, `PhotoInfo` becomes:

```rust
    PhotoInfo {
        id: crate::PhotoId,
        dims: Option<(u32, u32)>,
        /// The editable description, read from the catalog on the **first frame** and cached
        /// here like `dims`, then committed when the dialog closes — see `info::show`.
        description: Option<rustroom_app::description::DescriptionDraft>,
    },
```

In `crates/app/src/library.rs` line 211:

```rust
            app.modal = crate::modals::Modal::PhotoInfo {
                id,
                dims: None,
                description: None,
            };
```

- [ ] **Step 4: The dialog**

In `info.rs`, replace the top of `show` (from `let Modal::PhotoInfo { id, dims } = &app.modal else`
through the `dims` block) with:

```rust
    let Modal::PhotoInfo {
        id,
        dims,
        description,
    } = &app.modal
    else {
        return;
    };
    let id = id.clone();
    // ⚠⚠ Read **once**, when the dialog opens, and cached in the modal's own state. A dialog body
    // runs sixty times a second, and a whole-file read there would be the `apply_view_demosaic`
    // mistake in its most expensive costume yet — ~15 ms per frame. The description draft follows
    // the same rule for the same reason, one catalog query instead of one file read.
    let dims = match *dims {
        Some(d) => d,
        None => {
            let d = if catalog::is_raw(&id.path) {
                raw_decode::dimensions(&id.path)
            } else {
                image_decode::dimensions(&id.path)
            };
            d.unwrap_or((0, 0))
        }
    };
    let mut draft = match description {
        Some(d) => d.clone(),
        None => rustroom_app::description::DescriptionDraft::open(app.catalog.as_ref(), &id.path),
    };
```

(The old block wrote `app.modal = Modal::PhotoInfo { … dims: Some(d) }` inside the `None` arm;
that write now happens once at the end of `show`, below, for both fields together.)

After the Capture `grid(…)` block and its `ui.separator();`, before the Marks block, insert:

```rust
            ui.label(egui::RichText::new("Description").strong());
            grid(ui, "info_description", |ui| {
                ui.label(egui::RichText::new("Title").weak());
                ui.text_edit_singleline(&mut draft.edited.title);
                ui.end_row();
                ui.label(egui::RichText::new("Caption").weak());
                ui.add(
                    egui::TextEdit::multiline(&mut draft.edited.caption)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();
                ui.label(egui::RichText::new("Copyright").weak());
                ui.text_edit_singleline(&mut draft.edited.copyright);
                ui.end_row();
                ui.label(egui::RichText::new("Creator").weak());
                ui.text_edit_singleline(&mut draft.edited.creator);
                ui.end_row();
            });
            // ⚠ The load-bearing sentence, in the export dialog's words: a field marked
            // "Copyright" is exactly where someone expects the never-modify-the-original
            // promise to break.
            theme::hint(
                ui,
                "Shared by every copy of this file. Written into exports and, for RAW files, \
                 the XMP sidecar. The original is never modified.",
            );
            ui.separator();
```

Replace the tail of `show` (from `if close || dismissed {` to the end of the function) with:

```rust
    if close || dismissed {
        // ⚠ Commit on close, whichever way it closed — and only when changed, or the sidecar
        // rewrite below would move its mtime for nothing (`description::commit`).
        if let Some(cat) = &app.catalog {
            if rustroom_app::description::commit(cat, &id.path, &draft) {
                // ⚠ Not gated on `is_original()` the way `apply_mark_to` is: a description is
                // per **file**, so editing it from a copy's dialog changes what the sidecar says.
                app.write_xmp_for(&[id.path.clone()]);
            }
        }
        app.modal = Modal::None;
    } else {
        app.modal = Modal::PhotoInfo {
            id,
            dims: Some(dims),
            description: Some(draft),
        };
    }
```

⚠ `draft` is moved into the closure by `&mut` capture and used after it; the `dialog(…)` call takes
`|ui| { … }` as `FnOnce`, so borrow it mutably inside and it is free again afterwards — the same
shape `close` already has.

- [ ] **Step 5: Run**

Run: `cargo test -p app modals::info 2>&1 | tail -3 && cargo test -p app library 2>&1 | tail -3`
Expected: both `test result: ok.`

- [ ] **Step 6: Launch the app and use it**

Run in the background: `cargo run --release -p app`. Open a folder, right-click a photo →
Information…, type a title and a caption, Close, reopen: the text is there. Then Export (Metadata
section → All metadata) and check:

```bash
exiftool -Title -Description -Rights -Creator -Rating -Subject -HierarchicalSubject -Model -Lens -DateTimeOriginal -ISO -ExposureTime -FNumber -FocalLength <exported file>
```

Expected: every field present. And on the original:

```bash
exiftool -Title -Description <original RAW>
```

Expected: nothing printed — the done-when.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/modals/mod.rs crates/app/src/modals/info.rs crates/app/src/library.rs
git commit -m "Information dialog: edit the description, committed on close"
```

---

### Task 13: Docs, and the full verification

**Files:**
- Modify: `~/Documents/obsidian/private/opensource_lightroom/01-lightroom-capabilities.md` (Metadata row line 25; the divergences list)
- Modify: `~/Documents/obsidian/private/opensource_lightroom/03-plan-of-action.md` (phase table line 80; M6.2 entry line 412)
- Modify: `docs/BACKLOG.md` (line 29)
- Modify: `docs/superpowers/specs/2026-09-08-metadata-writing-design.md` (status line)
- Modify: `CLAUDE.md` (resume block; crate layout lines for `catalog`, `export`, `xmp`; a new section)

- [ ] **Step 1: Run the whole suite, each from its own exit code**

```bash
cargo fmt --all -- --check; echo "fmt exit $?"
```
```bash
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2; echo "clippy exit ${PIPESTATUS[0]}"
```
```bash
cargo test --all 2>&1 | tee /tmp/rustroom_m62_tests.txt | grep -E "^test result" ; echo "test exit ${PIPESTATUS[0]}"
```
```bash
grep -E "^test .*golden.* ok$" /tmp/rustroom_m62_tests.txt | wc -l
```

Expected: `fmt exit 0`, `clippy exit 0`, `test exit 0`, every `test result` line `ok`, and the
golden count **59** — none moved, since nothing here touches a pixel. Record the new total test
count (sum of the `passed` numbers) for CLAUDE.md.

- [ ] **Step 2: The capabilities note**

Line 25's Metadata row: replace *"⚠ Ours is **read-only and partial** — only what the decoder
happens to surface (camera, lens, focal length, orientation). No IPTC, no writing, no custom
fields."* with:

> ✅ Title, caption, copyright and creator per photo (**M6.2, 2026-09-08**), edited in the
> Information dialog, written into exports and the XMP sidecar — never into the original. Capture
> EXIF is read into the catalog (M4.3b) and written into exports. ⚠ No custom fields, no GPS, no
> IPTC-IIM.

Change its marker from 🟡 to ✅. In the "deliberate divergences from Lightroom" section add:

> - **Descriptions are per file, not per virtual copy.** A title says what is *in* the picture,
>   which every copy shares; only ratings, flags and labels describe the image you made. Same rule
>   as keywords. (M6.2)

- [ ] **Step 3: The roadmap**

Line 80's Phase 6 row becomes:

```
| **6 — Delivery & interchange** | ✅ M6.1 ✅ · M6.2 ✅ · M6.3 ✅ · M6.4 ✅ · M6.5 ✅ *(2026-09-08)* |
```

The M6.2 heading at line 412 becomes `**M6.2 — Metadata writing** ✅ *(2026-09-08)*`, and append
to its bullets:

```
- ✅ Done as designed: a `Description` per file in the catalog (schema v7), edited in the
  Information dialog, embedded at export with rating, label, keywords and capture facts under an
  "All metadata / Copyright only" choice, and round-tripped through the XMP sidecar.
  Spec `2026-09-08-metadata-writing-design.md`.
```

Also mark M6.5's heading ✅ if it still reads 🟡 (auto-advance merged 2026-09-08).

- [ ] **Step 4: The backlog**

`docs/BACKLOG.md` line 29 becomes:

```
| EXIF/IPTC metadata editing | ✅ **M6.2, 2026-09-08** — edited in the catalog, embedded at export, in the sidecar |
```

- [ ] **Step 5: Spec status**

Change the spec's `Status:` line to `Status: **built 2026-09-08.** Plan `2026-09-08-metadata-writing.md`.`

- [ ] **Step 6: CLAUDE.md**

In the crate layout block, append to the `catalog/` line: `+ Description per file (v7)`; to the
`export/` line: `+ metadata::{FileMetadata, MetadataScope} — the full XMP packet`; and add an
`xmp/` line if none exists: `│   ├── xmp/                  # ✅ stem.xmp read/merge-write: rating, label, keywords, title/caption/rights/creator`.

In the **RESUME HERE** block: update the *State at handoff* sentence to the new test count, 59
goldens, branch `m6.2-metadata-writing` awaiting merge (⚠ do not merge or push — ask), and
replace the **▶ NEXT ACTION** paragraph with:

> **▶ NEXT ACTION:** ask whether to merge `m6.2-metadata-writing`; **Phase 6 is closed with it.**
> Then the open roadmap inconsistency (M5.5c custom aspect ratios marked ✅ but not built) or
> **Phase 7 🟡** (M7.6 tool gaps), the two **M5.4b leftovers**, and the measured range-mask
> coverage cache. Brainstorm → spec → plan → executing-plans, inline, on a branch.

Add a section **"M6.2 — metadata writing"** above the M6.5c section, in this file's idiom, carrying:
what shipped; the three user decisions (wide scope, per file, `All` default); the dot-rule
correction from this plan's top; the forget/undo gap the spec missed (Task 2); the `tiff:Model`
caveat; the multi-entry-creator caveat; the two carrier tests and why the dialog test sets the
draft rather than typing; and the exiftool done-when result.

- [ ] **Step 7: Commit**

```bash
git add CLAUDE.md docs/BACKLOG.md docs/superpowers/specs/2026-09-08-metadata-writing-design.md
git commit -m "Record M6.2, and close Phase 6"
```

(The Obsidian vault is not in this repository; its two files are saved, not committed.)

- [ ] **Step 8: Report**

State: the test delta, that 59 goldens ran and none moved, the branch name, that it is **not**
merged and **not** pushed, the two oversized files touched (`db.rs`, `batch_export.rs`), and
what the GUI check showed.
