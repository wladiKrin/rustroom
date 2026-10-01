# Remove from collection — and collection views that show only their own copies

**Date:** 2026-10-01 · **Branch:** `remove-from-collection` · **Status:** design approved

Found while fixing the live views: **nothing in the app takes a photo out of a collection.**
`Catalog::remove_from_collection` has existed since M4.1, uncalled. Adding is not undoable either.

## Decisions (the author's)

- **Three routes, all of them:** the right-click menu, the Delete / Backspace key, and a `−` on each
  collection row in the rail.
- **Both adding and removing are undo steps.**
- **A collection or smart-collection view shows only its member copies** — measured by a throwaway
  test on 2026-10-01: a collection holding only copy 1 of a file showed copy 0 as well, because the
  view is built per *file* (`collections::scan_of`) and `entries::build` expands every copy the
  sidecar lists.

## Design

### Routes (removing never touches a file)

- **Right-click**, while the Library is showing a collection: **Remove from “<name>”** under *Add to
  collection*, on `menu_targets` — the clicked photo, or the whole selection when the click is inside
  it — the rule *Add to collection* already follows.
- **Delete / Backspace** in the Library **grid** while it shows a collection: the selection leaves the
  collection. Through `grid_keys`, which runs only when nothing has keyboard focus. Not in Develop
  (Retouch owns the key there) and not in Compare or Survey. ⚠ **Not a menu-bar key equivalent**: a
  Backspace equivalent in the macOS menu can fire while the search field is being typed in. Listed in
  Help → Keyboard shortcuts (`modals::EXTRA_KEYS`).
- **Rail:** each collection row gets `−` (*Remove from*, hover "Remove N selected") beside `+`, enabled
  with a selection — from any view, the keyword rows' `+`/`−` pair.

### One funnel, undoable

`ActionKind::CollectionChange { collection, add, members: Vec<PhotoId> }` — `members` are only the
copies whose membership **changed**, so undoing an add never takes out a photo that was already in.
`Catalog::add_to_collection` / `remove_from_collection` return whether they changed a row. Every route
— the rail's `+` and `−`, the menu's add and remove, the key — goes through one
`change_collection(collection, add, ids)`, which applies, records one step when something changed, and
says so: *"Added 3 to “Trips” — Ctrl+Z to undo"*, *"Removed 2 from “Trips” — Ctrl+Z to undo"*,
*"Already in “Trips”"*, *"Not in “Trips”"*. Undo inverts on `members`, redo re-applies.

### Member copies only

- `LibrarySource::is_per_copy()`: a collection and a smart collection. A keyword or a date belongs to the
  file, so their views keep every copy.
- `Library::copies: Option<HashSet<PhotoId>>` — the copies a per-copy source holds. `browser_entries`
  (the one function both entry-building paths go through) drops every row not in it.
- `smart_members` returns the **copies** that matched, not one row per matching file.
- The live refresh compares the copies as well as the files, so a removed copy leaves at once.
- **A virtual copy made while viewing a collection joins that collection** (Lightroom's rule) — without it
  the new copy would vanish the moment it was made. ⚠ In a smart-collection view a new copy stays only
  if it matches the rule; copies do not inherit the original's rating today, so under "3 stars and up"
  a fresh copy drops out. Known, left.

### Placement

A new bin module `collection_ops.rs`: the funnel, the routes, *Add to collection* moved out of
`catalog_ops`, and the replay. `collections_panel.rs` (over 600 lines of code), `cell_menu.rs` and
`library/keys.rs` gain only wiring.

## Tests

- **Catalog:** add and remove report a change; twice reports none.
- **Funnel:** records only what changed; undo leaves a photo that was already in; undo and redo; nothing
  changed, nothing pushed; each route (rail `+`, rail `−`, menu add, menu remove, key) one step.
- **Key:** Delete and Backspace remove in a collection view; nothing outside one. (The focus gate is
  `grid_keys`' caller's, shared with the culling keys.)
- **Copies:** a collection holding copy 1 shows only copy 1; a smart collection shows only matching
  copies; a keyword view still shows every copy; removing one copy of two leaves the other on screen and
  takes the removed one away (live refresh); a copy made in a collection view joins it.
- **Sabotages, each red:** the funnel recording every target; `copies` ignored by `browser_entries`; the
  refresh comparing files only; the key acting outside a collection view; the new copy not joining.

## Docs

The journal, `CLAUDE.md` (test count), the vault's `01-lightroom-capabilities.md` (*Collections*).
