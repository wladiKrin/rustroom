# Immich slice 3 — marks that persist — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A rating, flag, colour label or keyword set on an Immich photo is still there after a restart, and behaves like one set on a local file.

**Architecture:** A new lib module records each opened album's assets as catalog rows under one folder per server — the Immich cache root — which is all `set_marks` and `reload_marks` were missing. The same module decides whether a catalog row belongs in a catalog-wide view, so All Photos and smart collections show an Immich photo only once its file is on disk.

**Tech Stack:** Rust, `rusqlite` via the existing `catalog` crate. No API work: nothing is sent to Immich.

**Spec:** `docs/superpowers/specs/2026-09-23-immich-catalog-rows-design.md`. Read it before Task 1.

---

## Conventions for every task

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **No `Co-Authored-By` trailer. Never push.** Branch is already `immich-catalog-rows`.
- ⚠⚠ **ONE cargo invocation mode.** Always `cargo test --workspace <name-filter>`, **never `-p`** — alternating costs 43–531 s per switch, measured.
- ⚠⚠ **The filter is a SUBSTRING, not a regex,** and one matching nothing **exits 0 having run zero tests.** Check the passed count, never the exit code alone.
- **Write cargo output to a file, read `$?` from its own command.** A `| grep` reports grep's status and has produced a false green here.
- ⚠⚠ **After any scripted edit:** `git diff <commit> -- <file> | grep '^-' | grep '///'` must be empty unless you meant it. A scripted move destroyed a 17-line doc block on 2026-09-23 with the whole suite green.
- ⚠ Plan snippets go stale when `cargo fmt` rewraps. If an anchor does not match, **read the file**; one anchor in a previous plan was off by the single word "that".
- **No egui in `immich_catalog.rs`.** It is a lib module; that is the point of it.
- ⚠ **Never run two gates concurrently** — the temp-dir isolation sites.

## File structure

| File | Responsibility | Size |
|---|---|---|
| `crates/app/src/immich_catalog.rs` | **New.** `record_assets`, `row_is_browsable`, tests | — |
| `crates/app/src/lib.rs` | `pub mod immich_catalog;` | — |
| `crates/app/src/immich_flow.rs` | both openers record their assets | 332 |
| `crates/app/src/sources.rs` | All Photos filters its rows | ~ |
| `crates/app/src/smart_sources.rs` | smart collections filter the same way | ~ |

⚠ Nothing is added to `crates/app/src/immich_source.rs` (**821 lines**, already on the open-items list as wanting a split).

---

### Task 1: The module, and rows that hold marks

**Files:** create `crates/app/src/immich_catalog.rs`; modify `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create the file with **only** this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ Its own directory per test, keyed by process id **and a counter** — the collision
    /// `thumb_store::TempDir`'s doc says has already bitten this repo twice.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "rustroom_imcat_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn asset(id: &str) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            original_file_name: format!("{id}.jpg"),
            original_mime_type: None,
            width: Some(1),
            height: Some(1),
            thumbhash: None,
            file_modified_at: "2025-01-01T00:00:00.000Z".into(),
            updated_at: "2025-01-02T00:00:00.000Z".into(),
        }
    }

    fn marks(rating: u8) -> catalog::Marks {
        catalog::Marks {
            rating,
            ..Default::default()
        }
    }

    /// ⚠⚠ **The report: a rating on an Immich photo vanished on restart.** `set_marks` returns `Ok(())`
    /// and does nothing when no `photos` row exists, so without `record_assets` the mark was never
    /// stored — and nothing failed at the time.
    #[test]
    fn a_mark_on_an_immich_photo_survives_a_reopened_catalog() {
        let dir = temp_dir("persist");
        let db = dir.join("catalog.sqlite");
        let root = dir.join("immich");
        let scan = crate::immich_source::entries_from_assets(&[asset("one")], &root);
        let path = scan[0].path.clone();

        {
            let cat = catalog::Catalog::open(&db).unwrap();
            record_assets(&cat, &root, &scan).unwrap();
            cat.set_marks(&path, 0, marks(4)).unwrap();
        }
        let cat = catalog::Catalog::open(&db).unwrap();
        let found = cat.marks_in(&root).unwrap();
        assert_eq!(
            found.iter().find(|(p, _, _)| *p == path).map(|(_, _, m)| m.rating),
            Some(4),
            "the rating must still be there after reopening the catalog"
        );
    }

    /// ⚠⚠ **One asset is one photo.** Reached through an album and through the timeline it must carry
    /// the *same* marks — which is why the folder is the server's cache root and not the album.
    #[test]
    fn an_asset_reached_two_ways_has_one_identity() {
        let dir = temp_dir("identity");
        let root = dir.join("immich");
        let cat = catalog::Catalog::open(&dir.join("c.sqlite")).unwrap();

        // The album's list, then the timeline month's list, both holding this asset.
        let from_album = crate::immich_source::entries_from_assets(&[asset("shared")], &root);
        let from_month =
            crate::immich_source::entries_from_assets(&[asset("shared"), asset("other")], &root);
        record_assets(&cat, &root, &from_album).unwrap();
        cat.set_marks(&from_album[0].path, 0, marks(5)).unwrap();
        record_assets(&cat, &root, &from_month).unwrap();

        let found = cat.marks_in(&root).unwrap();
        let shared = found
            .iter()
            .find(|(p, _, _)| *p == from_album[0].path)
            .map(|(_, _, m)| m.rating);
        assert_eq!(
            shared,
            Some(5),
            "recording the month's list must not have given the asset a second, unmarked row"
        );
    }

    /// ⚠⚠ **A photo can be marked before it is downloaded.** The grid fills from server thumbnails, so
    /// rating straight from it — with no file on disk — has to work. `file_facts` records a `None` size
    /// for a missing file and the row is complete enough regardless.
    #[test]
    fn an_asset_with_no_file_on_disk_can_still_be_marked() {
        let dir = temp_dir("nofile");
        let root = dir.join("immich");
        let scan = crate::immich_source::entries_from_assets(&[asset("absent")], &root);
        assert!(!scan[0].path.exists(), "the fixture must not create the file");

        let cat = catalog::Catalog::open(&dir.join("c.sqlite")).unwrap();
        record_assets(&cat, &root, &scan).unwrap();
        cat.set_marks(&scan[0].path, 0, marks(3)).unwrap();
        assert_eq!(cat.marks_in(&root).unwrap().len(), 1);
    }

    /// ⚠ Browsing a second album must not forget the first. `sync_folder` never deletes a row; this
    /// pins it at the level the feature depends on.
    #[test]
    fn recording_a_second_album_keeps_the_first_albums_rows() {
        let dir = temp_dir("accumulate");
        let root = dir.join("immich");
        let cat = catalog::Catalog::open(&dir.join("c.sqlite")).unwrap();
        let a = crate::immich_source::entries_from_assets(&[asset("a1"), asset("a2")], &root);
        let b = crate::immich_source::entries_from_assets(&[asset("b1")], &root);

        record_assets(&cat, &root, &a).unwrap();
        cat.set_marks(&a[0].path, 0, marks(2)).unwrap();
        record_assets(&cat, &root, &b).unwrap();

        assert_eq!(cat.photos_in(&root).unwrap().len(), 3, "all three rows");
        assert_eq!(cat.marks_in(&root).unwrap().len(), 1, "and the mark survived");
    }

    // ── row_is_browsable ──

    #[test]
    fn an_immich_row_with_no_file_is_not_browsable() {
        let dir = temp_dir("browsable");
        let root = dir.join("immich");
        let absent = root.join("nope.jpg");
        assert!(!row_is_browsable(&absent, Some(&root)));
    }

    #[test]
    fn an_immich_row_whose_file_exists_is_browsable() {
        let dir = temp_dir("present");
        let root = dir.join("immich");
        std::fs::create_dir_all(&root).unwrap();
        let there = root.join("yes.jpg");
        std::fs::write(&there, b"x").unwrap();
        assert!(row_is_browsable(&there, Some(&root)));
    }

    /// ⚠⚠ **The unplugged-drive rule is NOT being changed.** A local folder also keeps rows whose files
    /// are absent — deliberately, per `sync_folder`'s doc — and those must stay browsable. Only Immich
    /// rows are checked.
    #[test]
    fn a_local_row_with_no_file_is_still_browsable() {
        let dir = temp_dir("unplugged");
        let root = dir.join("immich");
        let elsewhere = dir.join("some_folder").join("gone.RAF");
        assert!(!elsewhere.exists());
        assert!(
            row_is_browsable(&elsewhere, Some(&root)),
            "an absent local file must not be hidden — that is a different question"
        );
    }

    /// ⚠ No server configured: nothing is an Immich path, so nothing is hidden.
    #[test]
    fn with_no_server_configured_every_row_is_browsable() {
        let dir = temp_dir("noserver");
        assert!(row_is_browsable(&dir.join("anything.jpg"), None));
    }
}
```

- [ ] **Step 2: Declare the module and run, to verify it fails**

Add to `crates/app/src/lib.rs` in alphabetical position:

```rust
pub mod immich_catalog;
```

```bash
cargo test --workspace immich_catalog > /tmp/t.log 2>&1; echo "EXIT: $?"
grep -E '^error' -A 4 /tmp/t.log | head
```

Expected: FAIL to compile — `record_assets` and `row_is_browsable` do not exist.

- [ ] **Step 3: Write the module above the tests**

```rust
//! Immich assets as catalog rows, so a mark set on one is still there tomorrow.
//!
//! ⚠⚠ **Why this exists.** `Catalog::set_marks` opens with `let Some(id) = self.photo_id(..)? else {
//! return Ok(()) }` — with no `photos` row it returns `Ok(())` and does **nothing**. The Immich
//! openers never called `sync_folder`, so every rating, flag, label and keyword set on an Immich
//! photo was silently dropped, and nothing failed at the time.
//!
//! ⚠ **Local only, by decision.** Nothing here talks to Immich. Writing marks back would need
//! `asset.update` on the API key, which the Settings help text tells the user not to grant.
//!
//! ⚠ **No egui.** A lib module, testable with no window and no server.

use std::path::Path;

/// Record an opened album's or month's assets, so their marks have somewhere to live.
///
/// ⚠⚠ **The folder is the server's cache root — one row per server, never one per album.** An asset
/// reached through two albums, or through an album and the timeline, is **one photo with one
/// identity**; a folder per album would give it two rows and two independent sets of marks.
///
/// ⚠ Rows accumulate and are never removed: `sync_folder`'s documented rule is that *absence is not
/// deletion*, which is exactly what makes a mark on an asset you have not looked at for a month still
/// be there. Calling this for a second album therefore adds to the first rather than replacing it.
///
/// ⚠ An asset that has not been downloaded still gets a usable row — `file_facts` records a `None`
/// size for a missing file — because marking a photo straight from a server thumbnail has to work.
pub fn record_assets(
    cat: &catalog::Catalog,
    root: &Path,
    scan: &[catalog::CatalogEntry],
) -> Result<(), catalog::CatalogError> {
    cat.sync_folder(root, scan)
}

/// Whether a catalog row should appear in a **catalog-wide** view — All Photos, or a smart collection.
///
/// ⚠⚠ **Immich rows only, and that restriction is the point.** A local folder on an unplugged drive
/// also holds rows whose files are absent, and the catalog keeps them deliberately. Hiding every
/// absent row would quietly change what an unplugged drive does, which is a different question that
/// nobody asked.
///
/// ⚠ An Immich row is shown only once its file is really on disk — the user's call, and the same line
/// he drew for slice 2: *a photo downloaded from Immich becomes local and joins everything else.*
///
/// ⚠ One `exists` per Immich row, on an explicit action rather than per frame. If a library of 100k
/// browsed assets ever makes it felt, the fix is a `present` column maintained at download time, not
/// a cache in front of the stat.
pub fn row_is_browsable(path: &Path, immich_root: Option<&Path>) -> bool {
    match immich_root {
        Some(root) if crate::immich_source::is_immich_path(root, path) => path.exists(),
        _ => true,
    }
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace immich_catalog > /tmp/t.log 2>&1; echo "EXIT: $?"
grep -cE '^test immich_catalog' /tmp/t.log
grep -E 'FAILED|panicked' /tmp/t.log | head
```

Expected: PASS, 8 tests.

⚠ If `catalog::CatalogError` or `catalog::Marks` is not re-exported at the crate root, follow whatever
path `db.rs` exports and fix the signature — do not widen it to `Box<dyn Error>`.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/immich_catalog.rs crates/app/src/lib.rs
git commit -m "Immich assets become catalog rows, so their marks have somewhere to live

set_marks returns Ok(()) and does nothing when no photos row exists, and the Immich openers never
called sync_folder — so every rating on an Immich photo was silently dropped with nothing failing.

The folder is the server's cache root, one row per server and never one per album: an asset reached
through two albums is one photo with one identity, and a folder per album would give it two
independent sets of marks. Rows accumulate, because absence is not deletion.

row_is_browsable checks Immich rows only. A local folder on an unplugged drive also keeps rows whose
files are absent, on purpose, and hiding those is a different question nobody asked.

Nothing is wired up yet."
```

---

### Task 2: Both openers record their assets

**Files:** modify `crates/app/src/immich_flow.rs`

- [ ] **Step 1: Record in `open_immich_album`**

Find the `catalog::sort(&mut scan, ..)` line in `open_immich_album` and insert directly after it:

```rust
        // ⚠⚠ Without this there is no `photos` row, and `set_marks` then silently does nothing —
        // which is how every rating on an Immich photo used to disappear. See `immich_catalog`.
        if let Some(cat) = &self.catalog {
            if let Err(e) = rustroom_app::immich_catalog::record_assets(cat, &root, &scan) {
                eprintln!("catalog: {e}");
            }
        }
```

⚠ `eprintln!` and carry on, exactly as `open_folder_at` does: a catalog that cannot be written must
not stop the album opening.

- [ ] **Step 2: The same in `open_immich_bucket`**

⚠ Read the function first — it is "identical in shape" to the album one by its own doc, so the same
insertion point applies, but confirm the local variable is also called `root`.

- [ ] **Step 3: Build and run the suite**

```bash
cargo build -p app > /tmp/b.log 2>&1; echo "BUILD EXIT: $?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST EXIT: $?"
```

⚠ `test_app.rs` constructs `RustroomApp` exhaustively, so a break there is visible only to
`cargo test`, never `cargo build`.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/immich_flow.rs
git commit -m "Opening an Immich album or month records its assets in the catalog

A catalog that cannot be written logs and is ignored, as the folder path already does: it must not
stop an album opening."
```

---

### Task 3: All Photos and smart collections show only what is on disk

**Files:** modify `crates/app/src/sources.rs`, `crates/app/src/smart_sources.rs`

- [ ] **Step 1: A helper for the root, once**

The filter needs the configured server's cache root, or `None`. Add to `immich_flow.rs` beside the
other `RustroomApp` helpers (it already has `immich_lists_dir` in the same shape):

```rust
    /// The configured server's cache root, if there is one — what `row_is_browsable` needs.
    pub(crate) fn immich_cache_root(&self) -> Option<std::path::PathBuf> {
        let conn = immich_source::connection(&self.settings)?;
        Some(immich_source::cache_root(&conn.url))
    }
```

- [ ] **Step 2: Filter All Photos**

In `open_all_photos`, right after `let rows = match cat.all_photos() { .. }`, insert:

```rust
        // ⚠⚠ An Immich row exists as soon as the album is browsed, whether or not the photo was ever
        // downloaded. All Photos shows the ones that are really here — the user's call, and the line
        // he drew for slice 2. ⚠ Shared with `open_smart` so the two cannot drift.
        let immich_root = self.immich_cache_root();
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|r| {
                rustroom_app::immich_catalog::row_is_browsable(&r.path, immich_root.as_deref())
            })
            .collect();
```

⚠ `self.catalog` is borrowed by `cat` at that point. If the borrow checker objects to calling
`self.immich_cache_root()`, compute `immich_root` **before** `let Some(cat) = &self.catalog`.

- [ ] **Step 3: Filter smart collections the same way**

In `open_smart`, at `let rows = cat.all_photos().unwrap_or_default();`, apply the identical filter
through the same function. ⚠ Do **not** re-implement the condition.

- [ ] **Step 4: Build and gate**

```bash
cargo build -p app > /tmp/b.log 2>&1; echo "BUILD EXIT: $?"
cargo fmt --all
cargo fmt --all -- --check > /tmp/f.log 2>&1; echo "FMT EXIT: $?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY EXIT: $?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST EXIT: $?"
```

Expected: all **0**, the **74 goldens unmoved** — this touches no pipeline stage.

- [ ] **Step 5: Verify no scripted edit ate anything**

```bash
git diff b2947b6 -- crates/app/src | grep '^-' | grep -v '^---' | grep '///'
```

Expected: empty.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "All Photos and smart collections show an Immich photo only once it is on disk

Both read every catalog row and both filter through one function, so they cannot drift. The check
is Immich-specific: an absent local file stays visible, because the unplugged-drive rule is not
what this changes."
```

---

### Task 4: Look at it

- [ ] **Step 1: Launch**

```bash
cargo run --release -p app
```

- [ ] **Step 2: What to check**

1. Open an Immich album, **rate a photo** (and flag it, and give it a colour label).
2. **Quit and relaunch.** Open the album again — the marks must still be there. **This is the
   feature.**
3. ⚠ Rate a photo you have **not** opened, straight from the grid thumbnail. It must take the mark.
4. Open the **same photo through the Timeline** month it belongs to: the same marks.
5. **All Photos** must not fill with server photos. Download one Immich photo (open it), then look
   again — that one should now be there.
6. A **smart collection** matching, say, rating ≥ 4 should behave the same way.
7. ⚠ Keywords on an Immich photo, since they go through the same rows.

- [ ] **Step 3: Write it up**

⚠ Add a dated section at the top of `docs/JOURNAL.md` and overwrite `CLAUDE.md`'s live state **in
place** — including removing "so marks do not persist on Immich photos" from the out-of-scope list,
which this makes false. ⚠ `CLAUDE.md` is at **406** lines against a ~400 target: something moves out
rather than in.

---

## Self-review against the spec

| Spec requirement | Task |
|---|---|
| Rows exist, so `set_marks` stops being a no-op | 1, 2 |
| One folder per server, not per album; one identity, two views | 1 |
| Rows accumulate; a second album keeps the first's | 1 |
| An undownloaded asset can be marked | 1 |
| Marks survive a reopened catalog | 1 |
| `row_is_browsable`: Immich-absent false, present true | 1 |
| ⚠ a **local** absent row stays browsable | 1 |
| no server ⇒ nothing hidden | 1 |
| Both openers record | 2 |
| All Photos **and** smart collections filter, through one function | 3 |
| New lib module; nothing added to `immich_source.rs` | 1 |
| Nothing sent to Immich | — |
| No golden movement | 3 |
| Seen in use | 4 |

**Known soft spots, flagged rather than hidden:**

- **`record_assets` is a one-line wrapper over `sync_folder`.** Deliberate: it is where the
  one-folder-per-server reasoning lives and what the tests address. If it stays one line after slice 4,
  fold it in.
- **One `exists` per Immich row** when All Photos opens. Stated in the spec with the fix if it is ever
  felt (a `present` column at download time). Not pre-optimised.
- **A downloaded photo does not appear in All Photos until something re-syncs its row** — the row's
  `size` is not what the filter reads, so this is fine, but note the filter stats *live*, which is
  precisely what makes it correct here.
- ⚠ **Immich photos still do not appear under Dates** until downloaded *and* metadata-scanned. That
  falls out of `capture_days` needing a capture row; not changed, and not a regression.
