# "All Photos", folder management and a filter bar — M4.3a-ii

**Date:** 2026-08-27
**Status:** designed, not built
**Slice of:** M4.3 (see `2026-08-27-windowed-browsing-design.md` for the four-way decomposition)

## What this closes

The catalog has held every folder and photo since M4.1, and marks and keywords since M4.2 — but
`open_folder`, `open_collection` and `open_keyword` are the only three loaders, so **the library
cannot be browsed as a whole.** The roadmap calls that *"a real gap, and the thing to fix first."*

M4.3a-i bounded the browser's memory and made it load on demand, which is what makes a view of the
whole library affordable at all. This slice adds the view, the ability to prune what feeds it, and
a way to narrow what it shows.

Not in this slice: **capture metadata** (M4.3b) and **smart collections** (M4.3c).

## Two plans, along the browse/filter seam

The distinction is load-bearing in the feature itself, so it is also where the work divides. Each
half is independently useful and independently verifiable — the split M4.1 and M4.2 both used.

- **Plan 1 — sources.** `Catalog::all_photos`, `LibrarySource::AllPhotos`, one shared loader, a
  Folders section in the left panel, and Forget-folder.
- **Plan 2 — filtering.** `LibraryFilter` and the filter bar.

---

## Decisions taken before designing

### ⚠⚠ The bar filters the loaded source; it is not a source

Selecting a collection **replaces** the Library's contents (M4.1's rule, and the reason a
collection is *browsed, not filtered*). A search bar is the opposite: it narrows what is loaded,
feeding the one derived `visible` list that `Library::refilter` already produces.

**All Photos is what makes filtering sufficient.** Without it, "filter what is loaded" really is
too narrow — that is the fragmentation complaint a single catalog exists to avoid. With it, a
library-wide search is one click away and the bar keeps one honest meaning: *it narrows what you
are looking at.*

Rejected: making search a source (`LibrarySource::Search`), which cannot search *within* a
collection — the commoner gesture — and would rebuild the entries list and restart the loader on
every keystroke. Also rejected: a scope control ("this folder / everywhere"), which adds a second
filtering mechanism beside the format filter; sparse-versus-dense index confusion arising from
exactly that is this project's third-most-repeated bug class.

### ⚠ Keyword stays out of the bar

Browsing a keyword already means *"show me only Paris photos, from anywhere"* — which, once All
Photos exists, is the **same operation** a keyword filter would offer. That is a feature with two
homes, which is what got the Presets panel removed.

The one thing a keyword filter would genuinely add is **composition** — *"in my Portfolio
collection, show the Paris ones"* — and that is not a filter-bar feature. It is precisely what
**smart collections (M4.3c)** are for, and building half of it here would pre-empt that slice with
a weaker version.

### ⚠ Forget-folder ships with All Photos rather than after it

M4.1 recorded: *"the library accumulates every folder ever opened, including accidents — 'Forget
this folder' deferred until it annoys."* **All Photos is the thing that makes it annoy.** Today
nothing surfaces the accumulation; the moment there is a view of the whole library, every test
folder and one-off directory is in it. The first impression of All Photos is exactly when this
would bite, and the alternative is discovering the mess and needing another round to fix it.

---

## Plan 1 — sources

### 1. Catalog: three additions

```rust
/// Every photo the catalog knows, with the folder mtime recorded at scan time.
pub fn all_photos(&self) -> Result<Vec<PhotoRow>, CatalogError>;   // path, copy, mtime
pub fn folders(&self) -> Result<Vec<FolderRow>, CatalogError>;      // id, path, photo count
pub fn forget_folder(&self, id: i64) -> Result<(), CatalogError>;
```

`FolderRow` mirrors `CollectionRow`'s shape (`id`, a display string, `count`), and `folders()`
mirrors its query, counting photos per folder with a correlated subquery.

⚠⚠ **`forget_folder` relies on a *chained* cascade, and that deserves its own test.** Deleting a
`folders` row cascades to `photos` and to `photo_keywords` — both hold `folder_id` — and then the
deleted `photos` rows cascade to `collection_photos`. That **second hop only happens because
`PRAGMA foreign_keys = ON` is set per connection.** M4.1 already has a test for that pragma
precisely because nothing else would notice its absence; this is the first feature to depend on
two cascades in series.

⚠ **No new migration.** Every table and column this needs already exists at schema v3.

### 2. `LibrarySource::AllPhotos` and one shared loader

`open_folder`, `open_collection` and `open_keyword` are near-identical: build a scan, sort it,
expand it into entries, build a `ThumbLoader`, construct a `Library`. A fourth copy is the point
at which they become one `open_scan(ctx, source, scan)`. **Most of this task is deletion.**

⚠ **`collections::scan_of` gains an injected `modified_at`, exactly as it already has
`edited_at`.** Today it calls `std::fs::metadata` per member; at 10,000 photos that is 10,000
stats on a view that otherwise touches the filesystem not at all. The catalog has stored `mtime`
since M4.1 and **nothing has ever read it** — CLAUDE.md flags exactly that, noting it was captured
because a scan is the only chance to. All Photos passes a lookup built from the catalog;
collections and keywords pass `fs::metadata`, unchanged. One function, one rule — *the caller
supplies the times* — which is the shape `edited` is already in.

⚠ `Reindex` refuses on `AllPhotos` for the reason it already refuses on a collection: it renumbers
copies in a *folder*, and All Photos has no single folder to re-read. The existing guard is a `let
… else` on `LibrarySource::Folder`, so this needs only its message widened.

### 3. The Folders section

The left panel gains a **Folders** section above Collections, with **All Photos (n)** as its first
row and each catalogued folder beneath it, showing a photo count and a `×` to forget.

⚠⚠ **A folder row re-scans on click; it does not browse from the catalog.** "One idiom for the
whole panel" was the first instinct and it is wrong here: collections and keywords *span* folders
and genuinely cannot scan, while a folder row names one real directory and can. Re-scanning is
simply more correct, and it has three consequences worth stating:

1. There is no such thing as a stale panel-browsed folder, because browsing one *is* scanning it.
2. **Refreshing needs no new button** — click the row again.
3. `Reindex` keeps working on it, because the source is a `Folder`.

⚠ **A folder whose directory is unreadable falls back to the catalog's last-seen record** rather
than appearing empty. An unplugged drive must not look like the folder emptied itself — the same
rule `sync_folder` follows in refusing to delete rows, and `collections::scan_of` in listing
vanished members.

⚠ **Deliberately NOT added: a separate lightweight "Refresh" button.** `Reindex` bundles two
things — it re-scans *and* renumbers copies, which changes unnamed copies' export filenames and
drops their undo stacks. Two buttons whose difference is "one of them also silently renumbers" is
worse than one button with a clear hover. Splitting `Reindex` into refresh-and-renumber is a real
cleanup and belongs in its own change.

### 4. Forget-folder

**Always confirms; no "don't ask again".** A deleted preset goes to a 20-entry trash and is
restorable by Ctrl+Z; this has neither. The dialog names the photo count and says plainly that the
folder's ratings, flags, labels and keyword assignments go with it.

⚠ **It deletes catalog rows and never touches a file.** The photos, their sidecars and their edits
are untouched — re-opening the folder re-catalogues it, minus the library metadata. That is the
M4.1 split doing its job (*delete the catalog and you lose organisation, never work*), and it is
the thing people actually worry about, so it gets its own test.

⚠ If the forgotten folder is the one currently shown, the Library is closed rather than left
displaying rows that no longer exist.

---

## Plan 2 — filtering

### 5. `LibraryFilter`

```rust
pub struct LibraryFilter {
    pub format: FormatFilter,   // contained, not absorbed
    pub text: String,
    pub min_stars: u8,
    pub flag: Option<Flag>,     // None = any; Some(Flag::None) = unflagged
    pub label: Option<u8>,      // None = any; Some(0) = unlabelled
}

impl LibraryFilter {
    pub fn matches(&self, entry: &PhotoEntry, marks: Marks) -> bool;
    pub fn is_active(&self) -> bool;
}
```

`Library` loses `filter` and `min_stars` and gains one `filter: LibraryFilter`; `refilter` becomes
a single call over the entries.

⚠ **`FormatFilter` is contained, not merged.** That module is tested and its *hidden-by-exception*
rule is deliberate — it stores what is hidden so a format appearing later is visible by default.
Absorbing it would churn a working thing for no gain.

⚠ **Text matches `entries::shown_name`, case-insensitively, not `file_name`.** That is the one rule
the Library caption, the rename field and the default export name already share. Matching
`file_name` would mean searching for what a copy is called and not finding it — which is precisely
the bug that made copy names invisible in the grid, where the Library was the last consumer
bypassing the shared rule.

⚠ **`Some(Flag::None)` and `Some(0)` are meaningful**, not degenerate: "show me the unflagged ones"
and "show me the unlabelled ones" are real culling questions. `None` is what means "any".

### 6. The filter bar

A search field plus compact star / flag / label controls in the Library toolbar, right of the
existing sort and grid-size controls and left of the format chips.

⚠ **The filter resets when the source changes**, as `FormatFilter` already does. A stale search
that empties a newly opened folder reads as breakage rather than as a filter.

⚠ **No new keyboard work is needed, and that is worth checking rather than assuming.** The
Library's bare culling keys (`0`–`9`, `p`, `x`, `u`) and its arrow navigation are already gated on
`memory().focused()`, which is exactly the gate a text field needs. Escape to leave the field is
added so the keyboard is recoverable without the mouse.

⚠ The filmstrip follows for free: both views read the one `visible` list.

---

## Testing

**Catalog** — `all_photos` spans folders; `folders` counts correctly; **the chained cascade**
(folder → photos → collection_photos, and folder → photo_keywords); **forgetting touches no
files**; forgetting a folder leaves other folders intact.

**`scan_of`** — the injected `modified_at` is used rather than the filesystem; the existing three
tests keep their assertions.

**`LibraryFilter`** — each field in isolation and in combination; text matches a *copy's* name;
`Some(Flag::None)` matches unflagged rather than everything; an empty filter matches everything.

**Headless UI** — the filter narrows `visible`; the selection is pruned with it (`refilter` does
both halves, and a filter that hides a selected photo while a sync could still write to it is the
hazard `retain_visible` exists for).

### Sabotages to run

- `PRAGMA foreign_keys = OFF` → the chained-cascade test fails
- text matched against `file_name` instead of `shown_name` → the copy-name test fails
- `refilter` recomputes `visible` without pruning the selection → the selection test fails
- `LibraryFilter::matches` ignores one field → that field's test fails

⚠ Each sabotage is checked for **compile errors first**, and the check is *"did a `test result:`
line appear at all"* — not a grep for `^error:`, which matches cargo's ordinary test-failure line
and produced a false "did not compile" three times in the previous slice.

## Accepted caveats

1. **Forgetting is destructive with no undo** — deliberately, and confirmed every time.
2. **All Photos' mtimes are as fresh as each folder's last scan.** Clicking a folder row re-scans
   it and refreshes them; All Photos itself does not walk the filesystem.
3. **All Photos lists photos whose files have vanished**, following the same *absence is not
   deletion* rule as collections.
4. **No keyword in the bar** — panel only, by the decision above.
5. **No saved searches** — that is M4.3c.
6. **No re-linking of moved files**, unchanged from M4.1: a moved photo appears as new, and the
   old row stays until its folder is forgotten.
