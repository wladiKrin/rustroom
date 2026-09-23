# Immich slice 3 — marks that persist — design

**Date:** 2026-09-23
**Branch:** `immich-catalog-rows`
**Status:** approved; two questions answered by the user (scope, All Photos)

## The problem, exactly

Ratings, flags, colour labels and keywords set on an Immich photo vanish on restart. The cause is
two links of one chain:

1. `open_immich_album` / `open_immich_bucket` never call `catalog::sync_folder`, so **no `photos` row
   exists** for an Immich asset.
2. `Catalog::set_marks` opens with `let Some(id) = self.photo_id(..)? else { return Ok(()) }` — **no
   row means it silently does nothing**. Exactly as `test_app::catalog_library`'s doc already warns:
   *"on an uncatalogued library it silently does nothing, and `reload_marks` then reads nothing back.
   The assertion fails against perfectly correct rating code."*

Nothing else is missing. `reload_marks` derives its folders from the entries' parents, which for
Immich is the one cache directory, so it works unchanged the moment rows exist.

## Scope: local only

**The user's choice of three.** Catalog rows so marks persist in Rustroom. **Nothing is sent to
Immich**, so the read-only API key the Settings help text asks for stays sufficient.

⛔ **Not done, by decision:** reading Immich's own `rating`/`isFavorite`, and writing anything back.
Write-back would need `asset.update` on the key — which the help text explicitly tells the user *not*
to grant — and would let a bug in Rustroom alter their library. ⚠ Colour labels and reject flags have
no Immich equivalent in any case.

## One folder row per server, never per album

The catalog keys a photo by `(folder_id, file_name, copy)`, and the Immich cache path is
deterministic from the asset's UUID. So:

- The folder is **`immich_source::cache_root(url)`** — one row per server.
- ⚠⚠ **Not one folder per album.** An asset reached through two albums, or through an album and the
  timeline, is **one photo with one identity**; per-album folders would give it two rows and two
  independent sets of marks. A test pins that a mark set in an album is visible from the timeline.
- Rows **accumulate** as albums are browsed and are never removed. That is `sync_folder`'s documented
  rule — *"absence is not deletion"* — and it is what makes a mark on an asset you have not looked at
  for a month still be there.

⚠ `file_facts` records `size` from `fs::metadata`, which is `None` for an asset that has not been
downloaded, and `mtime` from the entry, which `entries_from_assets` fills from `fileModifiedAt`. So a
row is complete enough to hold marks **before the file exists** — and marking a photo straight from a
server thumbnail, without downloading it, has to work. A test says so.

## All Photos: only what is really on disk

**The user's choice**, and it matches what he said of slice 2: *"images downloaded from immich will be
local and can thus appear in the Dates section, be added to collections etc."* — **downloaded** being
the operative word.

Two call sites read every catalog row: `open_all_photos` (`sources.rs`) and `open_smart`
(`smart_sources.rs`). ⚠ Both must filter, through **one shared function**, or they will drift.

```rust
/// Whether a catalog row should appear in a catalog-wide view.
pub fn row_is_browsable(path: &Path, immich_root: Option<&Path>) -> bool;
```

⚠⚠ **Immich rows only.** A local folder on an unplugged drive *also* has rows whose files are absent,
and the catalog keeps those on purpose. Filtering every absent row would quietly change what an
unplugged drive does, which is a different question nobody asked. The test names this.

⚠ Cost: one `Path::exists` per Immich row when All Photos or a smart collection opens. That is a
stat per row — fine for an explicit action, and it does not touch the per-frame path. ⚠ If a library
of 100k browsed assets ever makes this felt, the fix is a `present` column maintained at download
time, not a cache in front of the stat.

⚠ **Dates needs nothing.** `capture_days` reads capture rows, which the metadata scanner writes only
after reading the file — so an undownloaded asset cannot appear there anyway.

## Placement

⚠⚠ New lib module **`crates/app/src/immich_catalog.rs`** — zero GUI, unit-testable — holding
`record_assets` (the `sync_folder` call with its one-folder-per-server reasoning) and
`row_is_browsable`. **Not added to `immich_source.rs`**, which is **821 lines** and already on the
open-items list as wanting a split; adding a new concern to it would be the "one reasonable-seeming
addition at a time" this repo's standing instruction is about.

## Testing

In `immich_catalog.rs`, with no window and no server:
- A mark set on an asset reached through an **album** is visible when the same asset is reached
  through the **timeline** — one identity, two views.
- Marks survive a fresh `Catalog` opened on the same file — the actual report.
- ⚠ An asset whose file does **not** exist can still be marked and read back.
- Recording album B does not remove album A's rows.
- `row_is_browsable`: an Immich path with no file is **false**; with a file, **true**; ⚠ a
  **non-Immich** path with no file is **true**, because the unplugged-drive rule is not being changed.
- ⚠ `immich_root: None` — no server configured — leaves every row browsable rather than hiding
  things.

In `sources.rs` / `smart_sources.rs`: both filter, asserted through the shared function rather than
by repeating the logic.

⚠⚠ What no test reaches: that a rating set on a real Immich photo is still there after a real
restart. Every Immich defect so far was found by use.
