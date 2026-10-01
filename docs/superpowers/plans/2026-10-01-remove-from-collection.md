# Remove from collection: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use `- [ ]`.
> **Commits carry no `Co-Authored-By` trailer. Never push.**

**Goal:** take photos out of a collection from the menu, the key and the rail, undoably; make adding
undoable; show a collection or smart collection's member copies only.

**Spec:** `docs/superpowers/specs/2026-10-01-remove-from-collection-design.md`.

**Gate:** `cargo fmt --all --check` · `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace
--all-targets -- -D warnings` · `cargo test --workspace` (filter by test name, never `-p`).

## Task 1: the catalog reports a change

`crates/catalog/src/db/collections.rs`: `add_to_collection` and `remove_from_collection` →
`Result<bool, CatalogError>` = `execute(..)? > 0` (a photo the catalog does not know: `Ok(false)`).
Callers compile unchanged (`.is_ok()`, `.unwrap()`). Test: add → `true`, again → `false`; remove →
`true`, again → `false`. Commit.

## Task 2: member copies only

- `library_state.rs`: `LibrarySource::is_per_copy(&self) -> bool` (`Collection | Smart`).
  `Library::copies: Option<HashSet<PhotoId>>`. `browser_entries(scan, stacking, expanded, remote,
  copies: Option<&HashSet<PhotoId>>)` filters `entries::build`'s rows by it **before** `stack::group`.
  `rebuild_entries` passes `lib.copies.as_ref()`.
- `sources.rs`: `open_scan` → `open_scan_with(ctx, source, scan, copies)`; `open_scan` passes `None`.
  `open_collection` passes the members as a set.
- `smart_sources.rs`: `smart_members` keeps the matching entries' **ids** (`(path, copy)` per match);
  `open_smart` passes them as the set.
- `source_refresh.rs`: a helper `copies_of(source, members) -> Option<HashSet<PhotoId>>`
  (`is_per_copy`); `refresh_source` rebuilds when the files changed **or** the copies did.
- `test_app::library_of_names`: `copies: None`.
- Tests: a collection holding copy 1 of a two-copy file shows copy 1 only (real sidecar, `TestDir`,
  `add_copy`); a smart collection rating copy 1 in shows copy 1 only; a keyword view shows both; removing
  copy 1 of a collection holding both refreshes to copy 0 only. Commit.

## Task 3: the funnel and its undo

- `action_log.rs`: `ActionKind::CollectionChange { collection: i64, add: bool, members: Vec<PhotoId> }`,
  global in `reachable`.
- `collection_ops.rs` (bin, new):
  - `change_collection(&mut self, collection, add, ids: &[PhotoId]) -> usize` — applies, collects
    the changed, pushes one step when any, sets the status (the four sentences in the spec, with the
    collection's name from `cat.collections()`).
  - `selected_photo_ids(&self) -> Vec<PhotoId>`.
  - `add_targets_to_collection` moved here from `catalog_ops`; `remove_targets_from_collection(collection,
    clicked)`; `change_selection_in_collection(collection, add)` for the rail; `remove_selection_from_
    viewed_collection()` for the key.
  - `replay_collection_change(kind, forward)`.
- `undo.rs`: dispatch `CollectionChange` to the replay, both directions.
- `collections_panel.rs`: `Action::AddSelection` → `change_selection_in_collection(id, true)`.
- Tests (in `collection_ops.rs`, `for_test` + `library_of(3)` + `catalog_library`): adding records only
  what changed and its undo leaves the one already in; remove, undo, redo; nothing changed, nothing
  pushed. Commit.

## Task 4: the routes

- `cell_menu.rs`: `CellAction::RemoveFromCollection(i64)`; while the source is a collection, a **Remove
  from “<name>”** button under *Add to collection* (the source's id and name are passed in by the
  caller, which knows the Library); `apply_cell_action` → `remove_targets_from_collection`.
- Rail (`collections_panel.rs`): a third `RowAction` `−` / verb "Remove from" / hover "Remove N selected",
  enabled with a selection → `Action::RemoveSelection(id)` → `change_selection_in_collection(id, false)`.
- `library/keys.rs`: Delete or Backspace sets `out.remove_from_collection`; `library/mod.rs` applies
  `remove_selection_from_viewed_collection()` after the grid.
- `modals/mod.rs` `EXTRA_KEYS`: a *Library* line, "Remove from the collection you are viewing" — "Delete or
  ⌫".
- Tests: the menu lists the item in a collection view and not elsewhere (nodes); the rail's `−` removes
  (drawn panel, `labelled_centres`); the key removes in a collection view and does nothing in a folder;
  the hover sweep passes. Commit.

## Task 5: a virtual copy made in a collection view joins it

`copies.rs::create_virtual_copy`: after `add_copy`, when `selected_collection()` is `Some(c)`,
`add_to_collection(c, path, n)` and insert the new id into `lib.copies` before `rebuild_entries`. Test:
make a copy while viewing a collection — the new copy is a member and is on screen. Commit.

## Task 6: sabotages, gate, docs, GUI

Sabotages, each red then restored: the funnel recording every target; `browser_entries` ignoring
`copies`; the refresh comparing files only; the key acting outside a collection view; the new copy not
joining. Gate on the tip. Docs: the journal, `CLAUDE.md` (test count), the vault's capabilities
*Collections* line. Launch the GUI.
