# Move to folder: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use `- [ ]`.
> **Commits carry no `Co-Authored-By` trailer. Never push.**

**Goal:** right-click → Move to folder…, moving files, sidecars and catalog rows, Immich downloads under
their original names, undoable.

**Spec:** `docs/superpowers/specs/2026-10-01-move-to-folder-design.md`.

**Gate:** `cargo fmt --all --check` · `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace
--all-targets -- -D warnings` · `cargo test --workspace` (filter by name, never `-p`; several filters as
a zsh **array**, with a control count).

## Task 1: the catalog follows a move

`crates/catalog/src/db/moves.rs`:
- `const PER_FILE_TABLES: [&str; 5]` — `relink`'s list, now shared.
- `move_photo(&self, from: &Path, to: &Path) -> Result<(), CatalogError>`: in one transaction, ensure the
  destination folder row, then `UPDATE {table} SET folder_id = ?new, file_name = ?to_name WHERE
  folder_id = ?old AND file_name = ?from_name` for every table. A file the catalog does not know: `Ok`.
- `knows(&self, path: &Path) -> Result<bool, CatalogError>` — a `photos` row at that folder and name.

Tests: marks, a keyword, a copy row, a description and a history follow a move **with a rename**; a
collection keeps the photo; the destination folder is created; `knows` before and after. Commit.

## Task 2: Immich original names

- `schema.rs`: `if from < 10` — `CREATE TABLE immich_names (folder_id … REFERENCES folders(id) ON DELETE
  CASCADE, file_name TEXT NOT NULL, original TEXT NOT NULL, PRIMARY KEY (folder_id, file_name))`;
  `SCHEMA_VERSION = 10`. The test that re-runs from 7 also drops `immich_names` and expects
  `SCHEMA_VERSION`.
- `db/photos.rs` (or a new `db/immich.rs` child module): `set_original_names(folder, &[(file_name,
  original)])` (upsert) and `original_name(path) -> Option<String>`.
- `app/src/immich_catalog.rs::record_assets`: after `sync_folder`, store `(path's own name,
  CatalogEntry::file_name)` for each entry.

Tests: round trip; an upsert replaces; `record_assets` stores the original. Commit.

## Task 3: the lib `photo_move`

`crates/app/src/photo_move.rs`, `pub mod photo_move;`:

```rust
pub struct Planned { pub from: PathBuf, pub to: PathBuf }
/// Where each of `files` lands in `dest`: `name_of` gives the wanted name, `taken` says a destination
/// is occupied. Files sharing a (folder, stem) keep sharing one; a file already in `dest` is left out.
pub fn plan(files: &[PathBuf], dest: &Path, name_of: impl Fn(&Path) -> String,
            taken: impl Fn(&Path) -> bool) -> Vec<Planned>;
/// Move one file with its JSON sidecar and, for a RAW, its XMP; across volumes, copy then delete.
pub fn move_file(p: &Planned) -> io::Result<()>;
pub fn move_file_with(p: &Planned, rename: impl Fn(&Path, &Path) -> io::Result<()>) -> io::Result<()>;
```

Tests (`test_dir::TempDir`): names kept; `-1` on a collision; a pair keeps one stem when only the RAW's
name collides; the same folder plans nothing; an injected name; sidecars; a JPEG leaves the XMP; an
injected `CrossesDevices` copies and removes the source. Commit.

## Task 4: the app — move, update, undo

- `action_log.rs`: `ActionKind::PhotoMove { moves: Vec<(PathBuf, PathBuf)> }`, global.
- `move_flow.rs` (bin, new):
  - `move_targets_to_folder(ctx, clicked)` — the files (distinct, collapsed stacks expanded by
    `stack::key_of_with` over `lib.scan`), the picker, then `move_files`.
  - `move_files(ctx, files, dest) -> usize` — persist the session; skip missing files (counted); plan with
    `target_name` (Immich cache → `original_name`, else own name) and `taken` (exists or `knows`); move
    each, then `move_photo`; push one `PhotoMove`; `after_move`; status.
  - `after_move(ctx, moves)` — re-key the decode cache; forget histories and log entries of every copy of
    each moved file; the Library per source (folder: drop / take back; Immich: unchanged; catalog
    sources: rewrite `scan` and `entries` paths); the session (Develop: reopen at the new path; Library:
    close); `rebuild_entries(None)`.
  - `replay_photo_move(ctx, kind, forward)` — each move (reversed for undo) with `move_file` and
    `move_photo`, then `after_move`.
- `undo.rs`: dispatch both directions.
- `cell_menu.rs`: **Move to folder…** after *Show in Finder*; `CellAction::MoveToFolder` →
  `move_targets_to_folder`.

Tests (`move_flow.rs`, real files in a `TestDir`, the catalog synced): a folder-view move moves the file
and sidecar, keeps the rating, leaves the view, and one undo brings it all back into the view; an Immich
cache file (a cache root under the test dir, `immich_url` set) lands under its original name; an
undownloaded one is skipped and counted; a collapsed RAW+JPEG pair moves together; the open photo's
session closes in the Library (`session` and `pending` both `None`); the menu offers the item. Commit.

## Task 5: sabotages, gate, docs, GUI

Each red, then restored: the JSON sidecar left behind; the pair split; `move_photo` not called; undo not
moving back; the Immich name ignored; the cross-volume fallback removed. Gate the tip. Docs: the journal,
`CLAUDE.md` count, the vault's capabilities *Library* line, `docs/BACKLOG.md` (download-then-move).
Launch the GUI.
