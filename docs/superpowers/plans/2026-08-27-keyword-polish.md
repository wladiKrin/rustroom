# Keyword and menu polish — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans.

**Goal:** Enter-to-create, a photo context menu that adds to collections and toggles keywords
across a selection, a `−` beside the panel's `+`, and a filmstrip hover name.

**Architecture:** One new catalog query (`keyword_counts`, direct assignments only), one shared
menu body in `library.rs` called by both the grid and the filmstrip, two new `CellAction`
variants that act on the selection rather than the index.

**Spec:** `docs/superpowers/specs/2026-08-27-keyword-polish-design.md`

---

### Task 1: `Catalog::keyword_counts`

**Files:** modify `crates/catalog/src/db.rs`

- [ ] Add, near `unassign_keyword`:

```rust
/// How many of `files` carry each keyword — the tri-state a menu needs.
///
/// ⚠⚠ **Direct assignments only, never the subtree.** `keyword_members` expands descendants
/// because a *search* for `Europe` must find `Paris`; a menu toggle removes an **assignment**, so
/// a photo tagged only `Paris` must show `Europe` unchecked. Otherwise unticking it does nothing.
pub fn keyword_counts(
    &self,
    files: &[PathBuf],
) -> Result<std::collections::HashMap<i64, usize>, CatalogError> {
    let mut stmt = self.conn.prepare(
        "SELECT pk.keyword_id FROM photo_keywords pk
         JOIN folders f ON f.id = pk.folder_id
         WHERE f.path = ?1 AND pk.file_name = ?2",
    )?;
    let mut counts = std::collections::HashMap::new();
    for file in files {
        let Some((folder, name)) = split(file) else {
            continue;
        };
        let rows = stmt.query_map(rusqlite::params![folder, name], |r| r.get::<_, i64>(0))?;
        for id in rows.filter_map(Result::ok) {
            *counts.entry(id).or_insert(0) += 1;
        }
    }
    Ok(counts)
}
```

- [ ] Test: two files, one tagged `parent` + `child`, one tagged `child` only; assert
      `counts[&child] == 2` and `counts[&parent] == 1` — the second is what fails if the query
      expands the subtree.
- [ ] Sabotage: make the query recurse into descendants; watch that test fail.
- [ ] `cargo test -p catalog`; commit.

### Task 2: the shared menu body

**Files:** modify `crates/app/src/library.rs`

- [ ] Extend `CellAction` with `AddToCollection(i64)` and `SetKeyword { id: i64, on: bool }`, both
      documented as acting on the **selection**, not on the index.
- [ ] Add `pub fn selected_files(lib: &Library, clicked: usize) -> Vec<PathBuf>` — the selection's
      distinct files when `clicked` is selected, otherwise the clicked photo alone. Test both.
- [ ] Add `pub fn cell_menu(ui, catalog: Option<&catalog::Catalog>, files: &[PathBuf], copies:
      &[(PathBuf, u32)]) -> Option<CellAction>` drawing Edit / Rename / Information… (the last two
      `add_enabled(files.len() == 1)`) plus the two submenus.
- [ ] Handle the two new variants in `apply_cell_action`.
- [ ] Replace the grid's inline menu with a `cell_menu` call.

### Task 3: the filmstrip

**Files:** modify `crates/app/src/filmstrip.rs`

- [ ] Replace its inline menu with the same `cell_menu` call.
- [ ] Add `.on_hover_text(lib.entries[i].label())` to the cell response.

### Task 4: Enter creates, and the panel's `−`

**Files:** modify `crates/app/src/collections_panel.rs`

- [ ] For each of the three name fields: keep the `named` guard, capture the `TextEdit` response,
      and fire the same action on `resp.lost_focus() && ui.input(|i| i.key_pressed(Enter)) &&
      named`.
- [ ] Add a `−` button beside each keyword row's `+`, enabled with a selection, hovering
      "Remove from N selected", raising `Action::UntagSelection(id)`.
- [ ] Add `RustroomApp::unassign_keyword_from_selection`, mirroring
      `assign_keyword_to_selection` including its per-file de-duplication.

### Task 5: backlog, docs, verify

- [ ] Add the dialog-sizing entry to `docs/BACKLOG.md`.
- [ ] `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings` (**check the
      exit code**), `cargo test --all`.
- [ ] Update CLAUDE.md with a section for this round.
- [ ] Launch the GUI.
