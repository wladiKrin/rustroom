# Masks panel — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the Masks panel as the approved option (A first, then **C** at the user's
request the same day): a segmented mode control and a row of kind icons, painted mask and
component rows, a Shape section, and the mask's adjustments under the same four sections as the
Edit panel.

> ⚠ Executed as written for A through Task 7, then Task 5 was redone for C in one further commit:
> the Add mask menu and the Refine row became the segmented mode + kind icon row, the header's
> overlay toggle went back to the Show mask checkbox, and `mask_add_mode` returned.

**Architecture:** Kind templates and labels move to a lib module (`mask_kind`); per-group
differ/reset moves to `edit-model` (`LocalGroup`); the header gains a toggle and a trailing text;
the panel modules (`masks`, `mask_components`, `mask_adjust`) are rewritten on top of those.

**Tech Stack:** Rust, egui 0.35, the AccessKit harness.

**Spec:** `docs/superpowers/specs/2026-09-19-masks-panel-design.md`. **Branch:** `masks-panel`.
No `Co-Authored-By`. Never push.

**Gate recipe:** as in `2026-09-19-develop-panel-order-and-chrome.md`.

---

### Task 1: `LocalGroup` in edit-model
**Files:** create `crates/edit-model/src/local_group.rs`; modify `lib.rs` (`mod local_group; pub use local_group::LocalGroup;`).
- [ ] Tests first: `one_moved_field_differs_in_exactly_its_group` (for each group, a representative field changed → `group_differs` true for it, false for the others), `reset_group_clears_exactly_its_group`, `resetting_every_group_reaches_the_default`.
- [ ] Implement `LocalGroup { WhiteBalance, Tone, Color, Detail }`, `ALL`, `name()`, `LocalAdjustments::{group_differs, reset_group}` via a private `copy_group(&mut self, from: &Self, g)` with an exhaustive destructure.
- [ ] `cargo test -p edit-model local_group`; commit `LocalGroup: a mask's adjustments differ and reset by section`.

### Task 2: `mask_kind` in the app lib
**Files:** create `crates/app/src/mask_kind.rs`; `lib.rs` gains `pub mod mask_kind;`.
- [ ] `MaskKind { Gradient, Radial, Brush, Luminance, Colour }`, `ALL`, `menu_label()`, `template()` (the five defaults moved verbatim from `masks.rs` with their comments).
- [ ] Tests: `every_kind_templates_its_own_variant`, `menu_labels_are_distinct`.
- [ ] Commit `MaskKind: the five mask kinds, their menu labels and templates`.

### Task 3: the header's toggle and trailing text
**Files:** `develop/panel/section.rs`, `adjustments.rs`, `panel/mod.rs`.
- [ ] `Header::new(name)` + builder methods; `HeaderOut { state, reset, toggled }`; the toggle painted left of reset as text (accent when on, weak when off), `WidgetInfo::selected(WidgetType::Checkbox, ...)`; trailing weak text left of that.
- [ ] Tests: `a_toggle_is_reachable_by_its_label_and_reports_a_click`, `trailing_text_is_not_a_widget`.
- [ ] Move the two callers; bin tests green; commit `section::header takes a toggle and a trailing text`.

### Task 4: glyphs
**Files:** create `develop/panel/mask_glyph.rs`; `panel/mod.rs` registers it.
- [ ] `pub fn paint(painter: &egui::Painter, rect: egui::Rect, kind: MaskKind, colour: egui::Color32)` and `pub fn kind_of(&Mask) -> MaskKind`.
- [ ] Test: each kind adds at least one shape to a painter (`ctx.run_ui`, count `out.shapes`).
- [ ] Commit `Hand-painted glyphs for the five mask kinds`.

### Task 5: the list, the Add menu, the Refine row
**Files:** `masks.rs`, `mask_components.rs`, `tool.rs`, `main.rs`, `test_app.rs`, `panel/mod.rs`.
- [ ] `mask_components`: remove `chips`; add `pub fn row(ui, RowSpec { glyph, op, name, accessible, selected, current, deletable, indent }) -> RowOut { clicked, delete }`; `rows` repainted with it; `pub fn refine_row(ui, s, i, ed)`.
- [ ] `masks::show`: MASKS header (toggle overlay, reset = remove all), Add mask row (pinned width), the mask rows, `rows` + `refine_row` for the selected mask, the empty hint; SHAPE header with the shape block; then `mask_adjust::show`.
- [ ] `Tool::Masks.panel_heading() → None`; fix `only_edit_shows_the_adjustment_stack` → `[Edit, Masks]`.
- [ ] Remove `mask_add_mode`; `MaskUi` loses `add_mode`.
- [ ] Rewrite the masks tests: `add(kind, s)` = click "Add mask" then the kind's `menu_label()`; `refine(op, kind, s)` = click `op.label()` then the kind; the mode tests become menu tests; new `the_overlay_toggle_flips_show_tint`, `the_masks_reset_removes_every_mask`.
- [ ] Bin tests green; commit `The Masks panel: an Add mask menu, painted rows, a Refine row, a Shape section`.

### Task 6: the adjustment sections
**Files:** `mask_adjust.rs`.
- [ ] Four `section::header`s (ids `("mask", name)`), `modified` from `group_differs`, reset → `reset_group` + `ed.discrete()`; Tone Curve and Color Grading inline under sub-labels.
- [ ] Tests re-targeted per the spec.
- [ ] Commit `A mask's adjustments under the Edit panel's four sections`.

### Task 7: rail icons
- [ ] `GLYPH_SIZE = 26.0`, `RAIL_WIDTH = 48.0`; commit `Larger rail icons`.

### Task 8: gate, GUI, docs
- [ ] Gate; GUI in release; journal + CLAUDE.md state; commit; report (masks.rs size).
