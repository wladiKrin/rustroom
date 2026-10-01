# Move photos to another folder — Immich downloads included

**Date:** 2026-10-01 · **Branch:** `move-to-folder` · **Status:** design approved

The author: *"I'd like the option to move images from one folder to another from rustroom directly.
Especially for ones that have been downloaded from immich, this would be a huge help."* Not part of any
milestone: M9.9 only *recognises* a move made outside Rustroom (`Catalog::relink_moved`), and
`rename_file` moves within one folder only.

## Decisions (the author's)

- **Right-click → Move to folder…**, through the system folder picker (which can make a folder).
  Dragging onto the rail's folders was offered and not chosen.
- **An Immich download moves out of the cache** and becomes an ordinary local photo.
- **It takes its original name** (`IMG_1234.HEIC`), not the asset id the cache names it by.

## Design

### What moves

The targets are `menu_targets` (the clicked photo, or the selection it is in), as **files**: copies
share one, and a **collapsed stack** brings every file under its key (a RAW+JPEG pair moves together).
With each file go its **JSON sidecar** (edits, every virtual copy, snapshots) and, for a RAW, its **XMP
sidecar** (`stem.xmp` — a JPEG never takes it, since a pair shares that name). In the catalog every
per-file row follows — `photos`, `photo_keywords`, `photo_capture`, `photo_description`,
`photo_history` — through one list shared with `relink`, so a table added later cannot be forgotten by
one of them; collection membership rides the photo row's id.

### Names

A local file keeps its name. A file in the Immich cache takes its **original name**, which the catalog
learns whenever an album or a month is opened: `record_assets` writes it to a new table,
`immich_names (folder_id, file_name, original)` (schema v10), so the *immich (downloaded)* folder view
renames too. A destination name that is taken — on disk **or** by a catalog row of a vanished file — gets
`-1`, `-2`, Import's rule. Files sharing a stem keep sharing one: the suffix is chosen for the group.
Nothing is ever overwritten. A file already in the destination is left alone.

### Immich

Downloaded photos move. One not yet downloaded has no file and is **skipped**, counted in the status:
*"Moved 5 to “Trips” — 2 aren't downloaded yet"*. The album keeps listing the asset under its cache
path; opening it there downloads a fresh, unedited copy.

### Across volumes

`std::fs::rename` fails with `CrossesDevices` between volumes (the cache is on the system disk; the
destination may not be). Then the file is copied with `ingest::copy::copy_verified` (a size-checked
partial renamed into place) and only then is the original deleted.

### Undo

`ActionKind::PhotoMove { moves: Vec<(PathBuf, PathBuf)> }` — the moves that happened, one step. Undo
moves each back (an Immich download back into the cache under its id name); redo moves them again. A
move whose way back is taken is skipped and the status says so.

### Afterwards

- **The Library:** a folder view drops what left it (and takes back what an undo returns); an Immich
  view keeps its rows on the cache paths; every catalog view (All Photos, collections, keywords, smart,
  dates) shows the photos at their new paths, selection kept.
- **The open photo**, if it moved: Develop reopens it at its new path; in the Library its session (saved
  first) is closed, so nothing starts decoding. Its in-memory history and log entries are dropped — the
  persisted history moved with the catalog row and is reloaded when it opens.
- **Caches:** the decode cache is re-keyed (`rename_file`'s rule); thumbnails re-render.

### Placement

- **Lib `photo_move`** (no egui): `plan` (names, groups, collisions) and `move_file` (the file and its
  sidecars, the cross-volume fallback), tested on temp directories with an injected `rename`.
- **Catalog:** `move_photo(from, to)` beside `relink` in `db/moves.rs`, `knows(path)`, and the
  `immich_names` table with `set_original_names` / `original_name`.
- **Bin `move_flow.rs`:** the picker, `move_files`, the Library and session update, the undo replay.
- `cell_menu.rs`: the item and `CellAction::MoveToFolder`; `undo.rs`: the dispatch.

## Tests

- **Plan:** names kept; a collision gets `-1`; a pair keeps one stem; the same folder plans nothing; an
  Immich name; a catalog-only collision.
- **Files:** the JSON sidecar and a RAW's XMP move; a JPEG leaves the XMP; the cross-volume fallback
  copies and deletes.
- **Catalog:** every per-file table follows a move, a renamed one included; collections survive; v10
  migrates; original names round-trip; `record_assets` stores them.
- **App:** a move from a folder view moves the files, keeps the marks, leaves the view, and undoes back;
  an Immich download lands under its original name; an undownloaded one is skipped and counted; a
  collapsed pair moves together; the open photo's session closes in the Library; the menu offers the
  item.
- **Sabotages, each red:** the sidecar left behind; a pair split by a collision; the catalog not
  following; undo not moving back; the Immich name ignored; no cross-volume fallback.

## Docs

The journal, `CLAUDE.md` (test count), the vault's capabilities (*Library*), `docs/BACKLOG.md` (the
download-then-move follow-up).
