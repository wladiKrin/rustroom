# Virtual copies (M3.4), plan 2 of 2 — the Library and the UI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Virtual copies are first-class: they appear in the Library, open in Develop, can be created, renamed, deleted and exported, and sync targets them individually.

**Architecture:** A new `entries` module turns a folder scan plus its sidecars into one row per `(file, copy)` — a **pure function with the sidecar loader injected**, so the expansion rules are testable without touching disk. `Library` holds those rows; `Session` gains the copy number and name; the Photo menu gains create/delete; the Develop panel gains an inline rename field backed by a new `ActionKind`.

**Tech Stack:** Rust, `eframe`/`egui` 0.35.

**Spec:** `docs/superpowers/specs/2026-08-25-virtual-copies-design.md`
**Predecessor:** `docs/superpowers/plans/2026-08-25-virtual-copies-storage.md` (complete, green — `PhotoId`, the sidecar envelope, per-copy persist, re-keyed undo, export naming).

**Build environment:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing rules:** no `Co-Authored-By`; never push; sabotage every new test and **confirm it compiled**; clippy clean by **exit code**.

⚠ **Check exit codes, never grep counts** — `cargo test --all | grep -c "test result: FAILED"` returns 0 when the *build* fails:

```bash
cargo test --all > /tmp/t.log 2>&1; echo "exit: $?"
```

⚠ `cd` persists between shell calls. ⚠ zsh does not word-split `$VAR`; use `${=VAR}`. ⚠ `ls` is aliased with colour.

⚠⚠ **This plan lands on the Library, which is the area with this project's worst defect record.** Sparse-versus-dense index confusion has caused three separate bugs here — the crop feedback loop, `render_max_dim`, and the grid's row breaks, the last found only by toggling a filter in real use. The mitigation is that Task 1 makes the expansion a **pure, injected-loader function** so the rules are tested without a GUI, and `visible` stays the single derived list every consumer iterates.

---

### Task 1: `entries` — one row per file-and-copy

**Files:**
- Create: `crates/app/src/entries.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/entries.rs`:

```rust
//! Turning a folder scan into the rows the Library shows.
//!
//! ⚠ One row per `(file, copy)`, not per file. The expansion is a pure function with the
//! sidecar loader **injected**, so every rule below is tested without touching disk or a GUI —
//! deliberately, because this is the code path that has produced three sparse-index bugs.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::{CopyEntry, Sidecar};
    use catalog::CatalogEntry;
    use edit_model::Edits;
    use std::path::PathBuf;

    fn scan(names: &[&str]) -> Vec<CatalogEntry> {
        names
            .iter()
            .map(|n| CatalogEntry {
                path: PathBuf::from(format!("/x/{n}")),
                file_name: n.to_string(),
            })
            .collect()
    }

    fn copy(n: u32, name: Option<&str>) -> CopyEntry {
        CopyEntry {
            copy: n,
            name: name.map(str::to_string),
            edits: Edits::default(),
        }
    }

    #[test]
    fn a_file_with_no_sidecar_is_one_row() {
        let got = build(&scan(&["a.RAF"]), |_| Sidecar::default());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id.copy, 0);
        assert_eq!(got[0].file_name, "a.RAF");
        assert_eq!(got[0].name, None);
    }

    #[test]
    fn the_original_is_always_a_row_even_when_the_sidecar_omits_it() {
        // ⚠ A sidecar can hold copy 1 and no copy 0 — `persist_copy(.., 1, ..)` on a photo whose
        // original was never edited does exactly that. The original is the *file*; it exists
        // whether or not anything is stored for it, and omitting it would make a photo vanish
        // from the browser after making a copy of it.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(1, Some("B&W"))],
        });
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id.copy, 0, "the original comes first");
        assert_eq!(got[1].id.copy, 1);
    }

    #[test]
    fn copies_are_adjacent_to_their_file_and_ascending() {
        let got = build(&scan(&["a.RAF", "b.RAF"]), |p| {
            if p.ends_with("a.RAF") {
                Sidecar {
                    copies: vec![copy(3, None), copy(0, None), copy(1, None)],
                }
            } else {
                Sidecar::default()
            }
        });
        let shape: Vec<(String, u32)> = got
            .iter()
            .map(|e| (e.file_name.clone(), e.id.copy))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("a.RAF".to_string(), 0),
                ("a.RAF".to_string(), 1),
                ("a.RAF".to_string(), 3),
                ("b.RAF".to_string(), 0),
            ]
        );
    }

    #[test]
    fn a_copys_name_comes_from_the_sidecar() {
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(1, Some("B&W"))],
        });
        assert_eq!(got[0].name, None);
        assert_eq!(got[1].name.as_deref(), Some("B&W"));
    }

    #[test]
    fn the_label_falls_back_to_the_copy_number_when_unnamed() {
        // ⚠ One number used everywhere: the identity, the label and the export suffix are the
        // same value. A named copy shows its name *instead of* the number, not as well as.
        let got = build(&scan(&["a.RAF"]), |_| Sidecar {
            copies: vec![copy(0, None), copy(1, None), copy(2, Some("B&W"))],
        });
        assert_eq!(got[0].label(), "a.RAF");
        assert_eq!(got[1].label(), "a.RAF / 1");
        assert_eq!(got[2].label(), "a.RAF / B&W");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cd ~/Projects/rustroom
cargo test -p app --lib entries 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find function build`.

- [ ] **Step 3: Implement**

Insert above the tests:

```rust
use crate::photo_id::PhotoId;
use crate::sidecar::Sidecar;
use catalog::CatalogEntry;
use std::path::Path;

/// One row in the Library: a file, which copy of it, and what to call it.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoEntry {
    pub id: PhotoId,
    /// The file's own name, shared by every copy of it.
    pub file_name: String,
    /// This copy's user-given name, if it has one.
    pub name: Option<String>,
}

impl PhotoEntry {
    /// What the grid, filmstrip and Develop footer show.
    ///
    /// ⚠ A named copy shows its name *instead of* its number, not as well as it. Two labels for
    /// one thing is how a display value drifts from the identity it is meant to describe.
    pub fn label(&self) -> String {
        match (&self.name, self.id.copy) {
            (_, 0) => self.file_name.clone(),
            (Some(n), _) => format!("{} / {n}", self.file_name),
            (None, c) => format!("{} / {c}", self.file_name),
        }
    }
}

/// Expand a folder scan into one row per `(file, copy)`.
///
/// `load` is injected so the rules here are testable without disk. In production it is
/// `sidecar::load_all`; measured at 4.6 ms for 830 files, so it stays inline rather than moving
/// to a worker thread.
pub fn build(scan: &[CatalogEntry], load: impl Fn(&Path) -> Sidecar) -> Vec<PhotoEntry> {
    let mut out = Vec::with_capacity(scan.len());
    for e in scan {
        let s = load(&e.path);
        // ⚠ The original is always a row, whether or not the sidecar mentions it — it *is* the
        // file. A sidecar can legitimately hold copy 1 and no copy 0.
        let mut numbers: Vec<u32> = s.copies.iter().map(|c| c.copy).collect();
        if !numbers.contains(&0) {
            numbers.push(0);
        }
        numbers.sort_unstable();
        numbers.dedup();
        for n in numbers {
            out.push(PhotoEntry {
                id: PhotoId::new(e.path.clone(), n),
                file_name: e.file_name.clone(),
                name: s.get(n).and_then(|c| c.name.clone()),
            });
        }
    }
    out
}
```

Declare it in `crates/app/src/lib.rs` beside the others, alphabetically (after `edit_history`):

```rust
pub mod entries;
```

⚠ `app`'s `Cargo.toml` must depend on `catalog`; it already does.

- [ ] **Step 4: Run**

```bash
cargo test -p app --lib entries 2>&1 | grep -E "test result:|FAILED"
```

Expected: five PASS.

- [ ] **Step 5: Sabotage — drop the always-present original**

Delete the `if !numbers.contains(&0)` block. Confirm it compiles, then run.

Expected: `the_original_is_always_a_row_even_when_the_sidecar_omits_it` FAILS. Restore.

- [ ] **Step 6: Sabotage — leave the numbers unsorted**

Delete `numbers.sort_unstable();`. Confirm it compiles, then run.

Expected: `copies_are_adjacent_to_their_file_and_ascending` FAILS. Restore.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/entries.rs crates/app/src/lib.rs
git commit -m "feat(app): one Library row per file and copy"
```

---

### Task 2: The Library holds `PhotoEntry`

**Files:**
- Modify: `crates/app/src/format_filter.rs`
- Modify: `crates/app/src/main.rs` (`Library`, folder load)
- Modify: `crates/app/src/library.rs`, `crates/app/src/filmstrip.rs`, `crates/app/src/thumb.rs` as the compiler requires

- [ ] **Step 1: Widen the format filter**

`FormatFilter::visible_indices` takes `&[CatalogEntry]` and reads each entry's extension. In `crates/app/src/format_filter.rs`, change it to take `&[PhotoEntry]` and read `e.id.path`:

```rust
    pub fn visible_indices(&self, entries: &[PhotoEntry]) -> Vec<usize> {
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.is_visible(&e.id.path))
            .map(|(i, _)| i)
            .collect()
    }
```

and change `is_visible` to take `&Path` rather than `&CatalogEntry`, reading the extension from it. ⚠ Every copy of a file has the same path, so **a filter toggle hides or shows all of a file's copies together** — which is right, and worth a test:

```rust
    #[test]
    fn hiding_a_format_hides_every_copy_of_those_files() {
        // ⚠ Copies share a path, so they share a format. Hiding RAF must not leave a copy of a
        // RAF stranded in the grid.
        use crate::entries::PhotoEntry;
        use crate::photo_id::PhotoId;
        let entries: Vec<PhotoEntry> = [("a.RAF", 0), ("a.RAF", 1), ("b.jpg", 0)]
            .iter()
            .map(|(n, c)| PhotoEntry {
                id: PhotoId::new(std::path::PathBuf::from(format!("/x/{n}")), *c),
                file_name: n.to_string(),
                name: None,
            })
            .collect();
        let mut f = FormatFilter::default();
        f.set_hidden("raf", true);
        assert_eq!(f.visible_indices(&entries), vec![2]);
    }
```

⚠ `set_hidden` may be named differently — read the existing tests in that file and use whatever they use to hide a format.

- [ ] **Step 2: Change `Library.entries`**

In `crates/app/src/main.rs`, change `entries: Vec<CatalogEntry>` to `entries: Vec<PhotoEntry>`, and at the folder load replace:

```rust
            Ok(entries) => {
                let n = entries.len();
                let paths: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
```

with:

```rust
            Ok(scan) => {
                // ⚠ One row per (file, copy). Reading every sidecar costs 4.6 ms for 830 files,
                // measured — small enough to stay inline rather than moving to the loader thread.
                let entries = entries::build(&scan, sidecar::load_all);
                let n = entries.len();
                // ⚠ Thumbnails are per *file*, and copies of one file share a path — so this
                // list deliberately repeats a path once per copy, keeping `thumbs`
                // index-parallel to `entries`. Copies therefore show identical thumbnails, which
                // is the accepted limitation recorded in the spec.
                let paths: Vec<_> = entries.iter().map(|e| e.id.path.clone()).collect();
```

- [ ] **Step 3: Fix the consumers**

```bash
cargo build -p app 2>&1 | grep -E "^error" -A 4 | head -30
```

Expected breakages and their fixes:
- `library.rs:81` — `theme::caption(ui, &lib.entries[i].file_name)` becomes `&lib.entries[i].label()`.
- Anything reading `e.path` becomes `e.id.path`.
- `open_entry` takes `e.id.clone()` rather than `e.path.clone()` — Task 3 uses it.
- `open_photo_is_selected` compares `e.id.path == s.path`; ⚠ it must now compare the **whole id**, or opening copy 1 while copy 0 is selected would count as "the open photo is selected" and a paste would write to the wrong version.

- [ ] **Step 4: Badge the copies**

In `crates/app/src/thumb.rs`, where the cell is painted, draw a small badge in the top-left when
`entry.id.copy != 0`, using the accent colour so it follows the theme:

```rust
    // ⚠ Copies of one file show a **pixel-identical** thumbnail — Library thumbnails are the
    // camera's embedded JPEG, not a render of the edits. The badge and the label are therefore
    // the *only* thing distinguishing them, which is the accepted limitation in the spec and the
    // reason edit-accurate thumbnails are backlogged.
    if copy != 0 {
        let r = egui::Rect::from_min_size(rect.min + egui::vec2(3.0, 3.0), egui::vec2(16.0, 12.0));
        ui.painter()
            .rect_filled(r, egui::CornerRadius::same(2), ui.visuals().selection.bg_fill);
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            copy.to_string(),
            egui::FontId::proportional(9.0),
            ui.visuals().extreme_bg_color,
        );
    }
```

⚠ `thumb::cell` is shared by the Library grid **and** the filmstrip, so this badges both from one
change — and `cell` must keep allocating before painting, or a cell's geometry starts depending on
what is drawn in it, which is the bug that made the filmstrip reflow.

- [ ] **Step 5: Run**

```bash
cargo test --all > /tmp/t.log 2>&1; echo "exit: $?"
```

Expected: `exit: 0`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(app): the Library lists copies alongside their files"
```

---

### Task 3: A session knows which copy it is

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the fields**

On `Session`, beside `path`:

```rust
    /// Which copy of `path` this session is editing. `0` is the original.
    ///
    /// ⚠ Added beside `path` rather than replacing it with a `PhotoId`: `path` has ~20 call
    /// sites that genuinely want the *file* — the decode, the buffer cache, the thumbnail. Only
    /// the sidecar, the undo scope and the label want the version.
    copy: u32,
    /// This copy's user-given name.
    name: Option<String>,
```

- [ ] **Step 2: Make `open_photo` real**

```rust
    fn open_photo(&self) -> Option<PhotoId> {
        self.session
            .as_ref()
            .map(|s| PhotoId::new(s.path.clone(), s.copy))
    }
```

⚠ Delete the "until the Library knows about copies — plan 2 of M3.4" comment; it is now done.

- [ ] **Step 3: Open the right copy**

`open_entry` currently takes an index and reads `e.path`. Make it carry the whole `PhotoId` through to session creation, and where the session's edits are loaded replace `sidecar::load(&path)` with `sidecar::load_copy(&path, copy)`.

⚠ The stash and `take_matching` call sites from plan 1 lose their `PhotoId::original` scaffolding and use the real id. Search for `PhotoId::original` in `main.rs` — **every remaining one is a bug after this task**, except any that genuinely mean "the file".

- [ ] **Step 4: Persist to the right copy**

In `persist_current_session`, replace `sidecar::persist(&s.path, &s.edits)` with:

```rust
            sidecar::persist_copy(&s.path, s.copy, &s.edits)
```

⚠ This is the line that makes two copies of one file keep different edits. Without it the last-saved copy silently overwrites the other.

- [ ] **Step 5: Show the label**

Wherever the Develop panel footer shows the filename, show the entry's label instead, so a copy is identifiable while editing it.

- [ ] **Step 6: Verify by hand, on a real folder**

```bash
cargo run --release -p app
```

Hand-write a sidecar with two copies for one of the Fuji RAFs, open the folder, and confirm both rows appear, open independently, and hold different edits. ⚠ This is the first point at which the feature is observable; do not skip it.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(app): a session edits one copy, not one file"
```

---

### Task 4: Create and delete a virtual copy

**Files:**
- Modify: `crates/app/src/menu.rs`, `crates/app/src/action_log.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Add the commands**

In `crates/app/src/menu.rs`, add to `Command`:

```rust
    CreateVirtualCopy,
    DeleteVirtualCopy,
```

and to the **Photo** section, after the tool items:

```rust
                action(
                    "Create virtual copy",
                    Command::CreateVirtualCopy,
                    Some(Shortcut::cmd('\'')),
                ),
                action("Delete virtual copy", Command::DeleteVirtualCopy, None),
```

⚠ `Shortcut::cmd('\'')` must **not** be in `safe_as_native_accelerator`'s allow-list — a native accelerator is consumed before winit sees the key and would break typing an apostrophe into the rename field Task 5 adds. Check that function and confirm it excludes this; if it does not, exclude it and say why in a comment.

- [ ] **Step 2: Add the undo entry**

In `crates/app/src/action_log.rs`:

```rust
    /// A deleted virtual copy, held so undo can put it back.
    ///
    /// ⚠ Carries the whole `CopyEntry` because nothing else holds it — unlike `PhotoEdit`, whose
    /// payload lives in `Session::history`, and `PresetDelete`, whose payload lives in the trash.
    CopyDelete {
        path: PathBuf,
        entry: Box<crate::sidecar::CopyEntry>,
    },
```

⚠ Boxed for the reason `Sync` is: `CopyEntry` holds an `Edits`, and an inline one would make *every* entry in the log that large, including the zero-sized `PhotoEdit`.

Add its `reachable` arm — a copy deletion is scoped to its file's photos, so treat it like `PresetDelete` and return `true`; a deleted copy no longer has a `PhotoId` to be reachable from.

- [ ] **Step 3: Write the tests**

```rust
    #[test]
    fn creating_a_copy_takes_the_current_edits_and_the_next_number() {
        // ⚠ Copies the *current* state rather than starting neutral — "try a variation" should be
        // one keystroke, not one keystroke plus redoing your work.
        let mut s = Sidecar {
            copies: vec![CopyEntry {
                copy: 0,
                name: None,
                edits: Edits {
                    exposure: 1.5,
                    ..Edits::default()
                },
            }],
        };
        let n = s.next_copy();
        s.set_edits(
            n,
            Edits {
                exposure: 1.5,
                ..Edits::default()
            },
        );
        assert_eq!(n, 1);
        assert_eq!(s.get(1).unwrap().edits.exposure, 1.5);
        assert_eq!(s.get(0).unwrap().edits.exposure, 1.5, "the source is intact");
    }
```

Put it in `crates/app/src/sidecar.rs`, where `Sidecar` lives.

- [ ] **Step 4: Implement the commands**

In `main.rs`'s command dispatcher:

- **Create** — load the sidecar, take `next_copy()`, `set_edits(n, current edits)`, `persist_all`, rebuild the Library entries, open the new copy. Push `ActionKind::PhotoEdit` is **wrong** here; push nothing for now and note that undo of a *creation* is out of scope (deleting the copy is the way back), or add `CopyCreate` if it falls out cheaply — do not force it.
- **Delete** — refuse when `s.copy == 0`; otherwise remove the entry from the sidecar, `persist_all`, push `ActionKind::CopyDelete`, rebuild entries, open the original.

⚠ The menu item must be **disabled** on an original rather than hidden, so the rule explains itself. Follow whatever `add_enabled` pattern the menu renderer already uses.

- [ ] **Step 5: Run and verify by hand**

```bash
cargo test --all > /tmp/t.log 2>&1; echo "exit: $?"
cargo run --release -p app
```

Create a copy, confirm a new row appears with the next number and the same edits; edit it and confirm the original is untouched; delete it and confirm Ctrl+Z brings it back.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(app): create and delete virtual copies"
```

---

### Task 5: The inline rename field

**Files:**
- Modify: `crates/app/src/action_log.rs`, `crates/app/src/develop/panel/mod.rs` (or wherever the footer lives), `crates/app/src/main.rs`

- [ ] **Step 1: Add the undo entry and its test**

```rust
    /// A copy renamed. `from`/`to` are `None` for "no name".
    Rename {
        id: PhotoId,
        from: Option<String>,
        to: Option<String>,
    },
```

with `reachable` scoping it to its photo exactly as `PhotoEdit` does — `open == Some(id)`.

```rust
    #[test]
    fn a_rename_is_undoable_only_from_its_own_copy() {
        use crate::photo_id::PhotoId;
        let path = std::path::PathBuf::from("/a.RAF");
        let one = PhotoId::new(path.clone(), 1);
        let two = PhotoId::new(path, 2);
        let mut log = ActionLog::default();
        log.push(
            ActionKind::Rename {
                id: one.clone(),
                from: None,
                to: Some("B&W".into()),
            },
            Some(&one),
        );
        assert!(log.undo(Some(&two)).is_none(), "a sibling must not reach it");
        assert!(log.undo(Some(&one)).is_some());
    }
```

- [ ] **Step 2: Draw the field**

In the Develop panel footer, where the filename shows, replace the label with a `TextEdit` bound to a scratch `String` on `RustroomApp` (`rename_draft: Option<String>`):

```rust
    // ⚠ A **scratch** draft, not the session's name directly. Writing straight through would
    // push one undo entry per keystroke, and would commit a half-typed name the moment focus
    // moved. Same shape the export dialog's `export_draft` uses, and for the same reason.
    let r = ui.add(egui::TextEdit::singleline(draft).desired_width(f32::INFINITY));
    if r.gained_focus() {
        // Clicking in selects the whole name, so typing replaces it. Verified against
        // egui 0.35: `CCursorRange` is re-exported at `egui::text_selection::CCursorRange`
        // and `CCursorRange::two` takes anything `Into<CCursor>`.
        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id) {
            let end = draft.chars().count();
            st.cursor.set_char_range(Some(egui::text_selection::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(end),
            )));
            st.store(ui.ctx(), r.id);
        }
    }
    // ⚠ Read the key press into a local *before* touching `r`. `Response` methods take the
    // context lock internally, and holding one across `ui.input(..)` panics in epaint's mutex —
    // the re-entrant lock the canvas-navigation work already paid for.
    let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if r.lost_focus() || entered {
        // Commit: Enter, or clicking anywhere else. No dialog.
    }
```

⚠ The commit path must: normalise an empty string to `None`; do nothing when the name is unchanged (or Ctrl+Z gains a no-op step); write it through `sidecar` read-modify-write; and push `ActionKind::Rename`.

⚠ ⚠ **`ui.input` must not be called while a `Response` is borrowed from the same context** — the re-entrant lock that panicked in the canvas-navigation work. Read the key press into a local `bool` before touching `r`.

- [ ] **Step 3: Wire undo/redo**

In the `ActionKind::Rename` arms of `undo_one`/`redo_one`, write the opposite name back through the sidecar and refresh the Library entry and the session's `name`.

- [ ] **Step 4: Verify by hand**

```bash
cargo run --release -p app
```

Click the name, confirm it selects entirely; type, press Enter, confirm the grid row updates; Ctrl+Z, confirm the old name returns. Then click in, type, and click on the canvas instead of pressing Enter — it must commit.

⚠ **Panel work is untested territory in this project** — six bugs in the brush canvas, all found by using it, none by tests. Expect defects here and record them.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(app): rename a virtual copy inline"
```

---

### Task 6: Sync targets a copy

**Files:**
- Modify: `crates/app/src/sync.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Change the target type**

`sync::apply(targets: &[PathBuf], ..)` becomes `targets: &[PhotoId]`, and the body becomes read-modify-write of the whole envelope:

```rust
    for target in targets {
        let before = sidecar::load_all(&target.path);
        let mut edits = before
            .get(target.copy)
            .map(|c| c.edits.clone())
            .unwrap_or_default();
        edits.merge_from(source, groups);
        let mut after = before.clone();
        after.set_edits(target.copy, edits);
        match sidecar::persist_all(&target.path, &after) {
            Ok(()) => prior.push((target.clone(), before)),
            ...
        }
    }
```

⚠ **`prior` must snapshot the whole `Sidecar`, not one copy's `Edits`.** Two targets in one file are two read-modify-write cycles, and undoing a sync that touched both must restore both — a per-copy snapshot would restore only the last one written. Update `SyncAction` accordingly.

- [ ] **Step 2: Write the test**

```rust
    #[test]
    fn syncing_two_copies_of_one_file_restores_both_on_undo() {
        // ⚠ The case a per-copy `prior` gets wrong: both writes land in one file, so a snapshot
        // of one copy's *edits* loses the other's original state and undo restores only one.
        use crate::photo_id::PhotoId;
        use crate::sidecar::{self, CopyEntry, Sidecar};

        let d = TempDir::new("two_copies_undo");
        let photo = d.0.join("a.RAF");
        let before = Sidecar {
            copies: vec![
                CopyEntry {
                    copy: 0,
                    name: None,
                    edits: Edits { exposure: 1.0, ..Edits::default() },
                },
                CopyEntry {
                    copy: 1,
                    name: Some("B&W".into()),
                    edits: Edits { exposure: 2.0, ..Edits::default() },
                },
            ],
        };
        sidecar::save_all(&photo, &before).unwrap();

        let targets = vec![
            PhotoId::new(photo.clone(), 0),
            PhotoId::new(photo.clone(), 1),
        ];
        let source = Edits { exposure: 9.0, ..Edits::default() };
        let report = apply(&targets, &source, EditGroups::ALL);
        assert!(report.failed.is_empty());

        let after = sidecar::load_all(&photo);
        assert_eq!(after.get(0).unwrap().edits.exposure, 9.0);
        assert_eq!(after.get(1).unwrap().edits.exposure, 9.0);

        undo(&report.prior);

        let back = sidecar::load_all(&photo);
        assert_eq!(back.get(0).unwrap().edits.exposure, 1.0, "copy 0 not restored");
        assert_eq!(back.get(1).unwrap().edits.exposure, 2.0, "copy 1 not restored");
        assert_eq!(
            back.get(1).unwrap().name.as_deref(),
            Some("B&W"),
            "the name must survive a sync and its undo"
        );
    }
```

⚠ `undo(&report.prior)` and `report.failed`/`report.prior` are whatever `sync.rs` already calls
them — read the existing tests in that file and match the real names. `TempDir` already exists
there; ⚠ give it a tag no other test in the file uses, and note that `sidecar.rs`'s `TempDir` was
made structurally unique after two tests with the same tag deleted each other's directory.

- [ ] **Step 3: Fix the call sites and run**

```bash
cargo test --all > /tmp/t.log 2>&1; echo "exit: $?"
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(app): sync writes to one copy, not one file"
```

---

### Task 7: Run it, and record

- [ ] **Step 1: Full green check**

```bash
cargo fmt --all --check; echo "fmt: $?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy: $?"
cargo test --all > /tmp/t.log 2>&1; echo "test: $?"
ls crates/pipeline/tests/golden/ | wc -l
```

Expected `0`, `0`, `0`, `47`. ⚠ This plan touches no pipeline code, so **any golden movement is a bug**.

- [ ] **Step 2: Launch the GUI and exercise the whole loop**

```bash
cargo run --release -p app
```

Create a copy, edit it differently from the original, rename it, export both, confirm the files are `X.jpg` and `X_1.jpg` and that they differ. Then delete the copy and Ctrl+Z it back.

- [ ] **Step 3: Record in CLAUDE.md**

A section in the established style, plus the RESUME block:

- **▶ NEXT ACTION**: Phase 3 is complete apart from the three parked slices in `docs/BACKLOG.md` (perspective correction, creative LUTs, snapshots). The next milestone is **Phase 4 — the Library/DAM half**.
- Test and golden counts from the real run.
- The findings worth carrying: the sidecar migration needing **two** things to be dangerous, not one; `BufferCache` staying path-keyed being what delivers "no storage cost", guarded by a test that fails to *compile* if changed; the stash and the log having to re-key **together**; the two export numbering schemes; the 4.6 ms folder-scan measurement; and whatever the GUI surfaces.
- Add both plan paths and the spec path to the docs list.

- [ ] **Step 4: Mark M3.4 in the roadmap**

Mark M3.4 🟡 in `03-plan-of-action.md`: virtual copies done, **snapshots** outstanding and backlogged.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: record M3.4 virtual copies"
```

---

## Definition of done

- Copies appear in the Library, open independently, and hold different edits.
- Create, delete (undoable), and inline rename (undoable) all work.
- Export produces `X.jpg` and `X_1.jpg`, and they differ.
- Sync targets a copy, and undoing a two-copy sync restores both.
- 47 goldens unchanged; `fmt`, `clippy -D warnings`, `test --all` green by exit code.
- CLAUDE.md and the roadmap updated; the branch left unpushed.
