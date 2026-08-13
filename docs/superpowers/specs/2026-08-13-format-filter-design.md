# Filtering the Library by file format — design

Date: 2026-08-13
Status: approved

## Goal

A camera set to RAW+JPEG fills a folder with two files per shot, and the Library shows both.
Give the browser a toggle per file format actually present in the folder, so one can be hidden.

App-layer only: no pipeline, `edit-model` or stage is touched, and the 29 goldens must stay
byte-identical.

## 1. A derived index list, not a filtered `entries`

`Library { entries, thumbs, selection, loader }` is **index-parallel**: `thumbs[i]` belongs to
`entries[i]`, `Selection` stores indices into `entries`, and `ThumbLoader` reports results by
index. Filtering `entries` itself would therefore mean rebuilding `thumbs`, remapping every
stored index, and restarting the loader — on every toggle.

So `entries` stays complete and the filter produces `visible: Vec<usize>`. Toggling is then a
recompute of a few hundred `usize`s with no thumbnail reload and no loader involvement.

The cost, stated plainly: **every place that iterates `0..entries.len()` must iterate `visible`
instead** — the grid, the filmstrip, and arrow navigation. Indices stored anywhere (selection,
`open_entry`) remain indices into `entries`, which is what makes the rest of the app oblivious.

## 2. `app::format_filter`

A new module, pure and `egui`-free, for the same reason `view_region` and `full_res` are:

```rust
/// Extensions present in a folder, lowercased, with how many entries each has.
/// Sorted by extension so the toggles do not reorder between scans.
pub fn formats_present(entries: &[CatalogEntry]) -> Vec<(String, usize)>;

#[derive(Default)]
pub struct FormatFilter {
    hidden: BTreeSet<String>,
}

impl FormatFilter {
    pub fn is_hidden(&self, ext: &str) -> bool;
    pub fn toggle(&mut self, ext: &str);
    pub fn is_visible(&self, entry: &CatalogEntry) -> bool;
    pub fn visible_indices(&self, entries: &[CatalogEntry]) -> Vec<usize>;
}
```

⚠ **It stores what is *hidden*, not what is shown.** With a shown-set, a format that appears
after a folder reload — a new file type dropped into the directory — would be absent from the
set and silently invisible. Hidden-by-exception makes "everything is visible" the default state
and needs no maintenance when the folder changes.

Extensions are compared lowercased, so `.JPG` and `.jpg` are one toggle. An entry with no
extension is always visible: it cannot be filtered by a control that does not exist for it.

## 3. ⚠ The hazard is selection, not display

Select twenty photos, hide their format, and the selection still holds them. A sync or a paste
would then write to photos that are not on screen — silent, and exactly the kind of thing that
is discovered as data loss rather than as a bug.

So **a filter change prunes the selection** to visible entries, and must repair `Selection`'s
invariant while doing so: `active == Some(i)` implies `i` is selected, and `active == None`
implies the selection is empty. `Selection` gains:

```rust
/// Drop every selected index not in `visible`, restoring the active/selected invariant.
pub fn retain_visible(&mut self, visible: &[usize]);
```

⚠ **Amended during implementation.** This section originally said the active photo should move
to another surviving member. `Selection::clamp` already handles the identical situation the
other way — it clears the whole set, on the stated grounds that "targets with no source are not
a usable state" — and `retain_visible` follows it. Two rules for "the active index went away"
would be a smell, and silently *moving* the active photo changes which photo a subsequent sync
copies **from**, with nothing on screen to announce it. Clearing is visible and recoverable.

⚠ **Ctrl+A is the deliberate exception.** `Selection::select_indices` (which replaces
`select_all`, since "all" now means "all visible") *does* adopt a new active photo when the
current one is outside the set. The difference is defensible: Ctrl+A is an explicit "select
these" from the user, so taking a source from what they asked for is expected; a filter change
is a side effect. Both behaviours are pinned by tests that name the distinction.

**The open photo in Develop is deliberately *not* closed** when its format is hidden. Hiding a
format is a browsing statement, not a "discard my work" one, and closing the session would
throw away an undo history. Arrow navigation simply stays within the visible set.

## 4. UI

A row of `toggle_value` chips in the Library toolbar, labelled with the uppercase extension and
the count — `RAF (411)`, `JPG (411)`.

- **Shown only when the folder has ≥2 formats.** A single toggle can only ever blank the grid,
  which is a footgun with no upside.
- **The filmstrip respects the filter too.** It is the same list and the same intent; filtering
  one view and not the other would be the surprising choice.
- **The filter resets when a folder is loaded.** Persisting it is a `Settings` field, and "I
  always want RAW only" is a real want but a speculative one — an obvious follow-up if it bites.
- Hiding everything shows the grid's existing empty state rather than being prevented. It is
  self-evidently recoverable, and blocking the last toggle is a rule to explain rather than a
  behaviour to discover.

## 5. Testing

| test | asserts |
|---|---|
| `formats_present_counts_each_extension` | sorted, lowercased, deduped, with counts; mixed case collapses |
| `hiding_a_format_removes_only_its_entries` | `visible_indices` drops exactly one format's entries and preserves order |
| `an_unknown_extension_defaults_to_visible` | the hidden-not-shown choice, as a property: a format absent from the filter is visible |
| `toggling_twice_restores_the_folder` | round trip |
| `pruning_drops_hidden_entries_from_the_selection` | the data-loss guard of §3 |
| `pruning_the_active_photo_clears_the_selection` | the amended rule of §3, matching `clamp` |
| `select_indices_takes_a_source_from_a_sparse_set` | Ctrl+A over a filtered folder adopts the first visible photo — the deliberate exception |
| `stepping_from_a_hidden_index_lands_on_a_neighbour` | arrow navigation when the open photo's format was hidden |
| `pruning_everything_empties_the_selection` | `active` becomes `None`, not a dangling index |
| the 29 goldens | byte-identical — this feature touches no pixels |

`Selection`'s existing tests carry a `check()` helper that asserts the invariant after every
operation; `retain_visible` is added to that treatment rather than tested in isolation.

## Non-goals

- **Grouping RAW vs non-RAW into two toggles.** Per-extension is what was asked for and is more
  honest about a folder holding, say, both `.RAF` and `.DNG`.
- **Persisting the filter**, per §4.
- **Filtering by anything else** — rating, date, keyword. That is the Phase 3 DAM work.
- **Recursive scanning.** `scan_folder` stays non-recursive.

## Accepted caveats

1. The filter is per-session and resets on folder load.
2. Hidden photos remain decoded in `BufferCache` if they were visited — the filter is a view,
   not an eviction.
3. Thumbnails for hidden entries still load in the background: `ThumbLoader` works from the
   full list, which is what keeps toggling instant.
