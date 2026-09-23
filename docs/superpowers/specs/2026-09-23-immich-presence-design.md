# The downloaded-photo indicator, and the Immich folder's name and count — design

**Date:** 2026-09-23
**Branch:** `immich-presence`
**Status:** approved

Three of the four items reported. ⚠ **The fourth needs no code** — see the last section.

## One fact behind all three: which cache files are actually here

The Folders rail's count, its label, and a per-thumbnail indicator all need the same answer: **which
of this server's assets have been downloaded.**

⚠⚠ **It must not be a stat per photo.** The grid draws hundreds of cells sixty times a second, so
`is_cached` per cell is the `apply_view_demosaic` mistake in a new costume. One **`read_dir`** of the
cache root answers for every asset at once.

```rust
/// The file names present in this server's cache directory — one `read_dir`, not a stat per asset.
pub fn present_names(root: &Path) -> HashSet<OsString>;
```

Cached on `RustroomApp` as `immich_present`, refreshed:

- **at startup**, when a server is configured — so the rail's count is right before any album is
  opened;
- when an Immich album or month opens — catching anything downloaded or deleted outside Rustroom;
- when a download completes — inserting that one name, rather than re-reading the directory.

⚠ `present_names` goes in `immich_source.rs`, which **already owns** `cache_root`, `cache_path` and
`is_cached`. That file is **821 lines**, past the ~600 threshold and already on the open-items list as
wanting a split; this adds ~20 lines to the module that owns the concept rather than inventing a third
Immich module for it. Reported, not hidden.

## 1. The rail says "immich (downloaded)", not a host slug

The Folders rail shows a folder's **basename**, which for the Immich cache root is
`https___immich_example`. It becomes **`immich (downloaded)`**.

⚠ Renamed rather than hidden — the user's call, and it is honest: clicking it opens the cache
directory as an ordinary folder, which shows exactly the downloaded photos. The full path stays as the
row's hover, as it is for every other folder.

```rust
/// What the Folders rail should call this folder.
pub fn folder_display_name(path: &Path, immich_root: Option<&Path>) -> String;
```

⚠ A pure function so the label is testable without a window, and so the rail has one place to ask.

## 2. The count is what is there, not how many rows exist

The rail showed **981** — every catalog row — while the folder holds **17** files. The count now comes
from `immich_present.len()` for that folder.

⚠⚠ **The rows are not wrong and are not being deleted.** A row exists for every asset browsed, which
is what makes a mark on an undownloaded photo persist (slice 3). The *count beside a folder* is a
statement about that folder's contents, and 981 was a statement about something else.

⚠ Only the Immich folder's count is overridden. Every other folder keeps `folders()`'s own count,
including a folder on an unplugged drive — the same line drawn in slice 3, for the same reason.

## 3. A circled down-arrow marks a downloaded photo

Top-left of the thumbnail, **grid only, Immich sources only**.

⚠ Shown on the photos that **are** downloaded, so the ink marks what you have rather than what you do
not. In an album most assets are not downloaded, so this is the smaller mark.

**Hand-painted, not a font glyph, and that is forced.** `icons.rs`'s own doc requires every codepoint
to be verified against the bundled Phosphor 2.1.2 TTF, because *"four separate features in this project
have picked a glyph that turned out to be absent"*. The bundled font's `post` table is **version 3.0 —
no glyph names** — so a codepoint can be verified as *present* but **not** as being the right picture.
Choosing one blind risks shipping the wrong icon, which is worse than tofu because it looks fine. A
circle plus three strokes needs no verification and scales cleanly, exactly as the histogram's clipping
marks already do.

⚠⚠ **The top-left is already taken** by the copy/label badge (`rect.min + (3, 3)`, sized to its text).
When both are present the indicator sits **immediately right of the badge**, so neither moves the
other's established position and they never overlap. A virtual copy of a downloaded Immich photo is
exactly that case.

⚠ `cell_with` gains the flag; `cell` passes `false`, so the **filmstrip is unchanged**. Turning it on
there later is one argument.

## 4. Rating advancing to the next photo — no code change

⚠⚠ **This is already a setting, and it is already on in the user's `settings.json`.** Settings →
**"Move to the next photo after rating"** (`Settings::auto_advance`). The field is written to
`settings.json` as `true`, so **changing the Rust default would not change the user's behaviour** — the
file wins. The toggle is the fix.

⚠ The default stays `true` unless the user asks otherwise: `settings.rs` records it as a deliberate
choice — *"a culling feature nobody finds is a feature that did not ship"* — and flipping a persisted
bool's default is the trap `write_xmp`, `auto_advance` and `gpu_tail` each already document.

## Testing

Pure, no window:
- `present_names` on a directory with two files and one absent name; on a **missing** directory it is
  empty rather than an error.
- `folder_display_name`: the Immich root → `immich (downloaded)`; any other path → its basename; ⚠
  `immich_root: None` → basename, so nothing is renamed when no server is configured.
- ⚠ A folder whose basename *happens* to look like a host slug but is not the configured root keeps
  its own name.

Through the UI harness:
- the rail's Immich row shows the **present** count, and another folder's row still shows
  `folders()`'s count.
- ⚠ The indicator is drawn for a downloaded asset under an Immich source and **not** under a folder
  source — asserted through the decision function rather than by reading pixels.

⚠⚠ What no test reaches: whether the mark reads as "downloaded" at a glance, and whether it collides
with anything in a real album. Seen in use, or not at all.
