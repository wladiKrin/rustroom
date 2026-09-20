# Retouch panel — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Retouch panel as a patch list first (a Next patch row, one row per patch with an
eye, a hold-out toggle and a delete), the settings under it, Find dust at the end.

**Architecture:** The shared row painter gains a glyph enum, a meta text and a second toggle;
per-patch outline and hold-out flags are session view state synced every frame, with `view_edits`
removing held patches; the canvas reads the outline flags instead of one app flag.

**Spec:** `docs/superpowers/specs/2026-09-20-retouch-panel-design.md`. **Branch:** `retouch-panel`.
No `Co-Authored-By`. Never push. **Gate recipe** as before.

### Task 1: session flags and the view
- [ ] `Session::{spot_shown, spot_held}`; `sync_spot_flags`, `all_spots_shown`, `set_all_spots_shown`;
  `view_edits` removes held + drag-suppressed patches, highest first; tests.
- [ ] `app.show_spots` removed; `draw_spot_overlay` loses `show_outlines` and filters by `spot_shown`;
  `delete_selected_spot` drops the flags with the patch.

### Task 2: the row and the glyphs
- [ ] `RowGlyph { Mask(MaskKind), Spot(SpotMode) }`, `meta`, `hold` on `RowSpec`; `RowOut.hold`;
  `mask_glyph::{paint_spot, paint_hold}` with tests.

### Task 3: the panel
- [ ] `Tool::Retouch.panel_heading() → None`; `retouch::show` rewritten; tests re-targeted and added.

### Task 4: gate, GUI, docs
- [ ] Gate; GUI; journal + CLAUDE.md state; commit; report.
