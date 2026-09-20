# Top bar and Library restyle — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The top bar with an accent app-mode pill and an icon pill for the Library modes, the
Develop toolbar regrouped with an accent Export, the Library's left panel on the section chrome
with painted rows, and grid cells with rounded thumbnails and a marks row.

**Spec:** `docs/superpowers/specs/2026-09-20-shell-restyle-design.md`. **Branch:**
`shell-restyle`. No `Co-Authored-By`. Never push. **Gate recipe** as before.

### Task 1: icons and the segmented module
- [ ] Eleven new constants in `icons.rs`, `ALL` grown. Move `segmented*` into
  `crates/app/src/segmented.rs`, re-exported from `theme`; add `pill` and `chip` with tests.
  Commit.

### Task 2: the list row and the header
- [ ] `list_row.rs` with tests; `section::Header::draggable` + `HeaderOut::response`. Commit.

### Task 3: the Library body
- [ ] `collections_panel.rs` over the header and the rows; `thumb::{cell_with, marks_row}`,
  accent border; the grid cell in `library.rs`. Commit.

### Task 4: the top bar
- [ ] `shell.rs`: the pills, search, filter, chips, slider, ⋯ menu; `develop::toolbar`
  regrouped with Export. Commit.

### Task 5: gate, GUI, docs
- [ ] Gate; GUI; journal + CLAUDE.md state; commit; report.
