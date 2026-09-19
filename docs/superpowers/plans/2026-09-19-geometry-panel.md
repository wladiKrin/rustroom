# Geometry panel — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Crop & Geometry panel as four sections (Crop, Straighten, Perspective, Lens) with
the Edit and Masks panels' chrome, a segmented aspect control, a composition dropdown, a level
toggle beside the angle, and the guides on one row.

**Architecture:** The per-section dot and reset come from a `GeoGroup` partition in `edit-model`
(the `LocalGroup` pattern); the panel is rewritten on `section::header`, `theme::segmented_sized`
and the existing widgets. Nothing in the canvas or the pipeline changes.

**Spec:** `docs/superpowers/specs/2026-09-19-geometry-panel-design.md`. **Branch:**
`geometry-panel`. No `Co-Authored-By`. Never push. **Gate recipe** as before.

### Task 1: `GeoGroup` in edit-model
- [ ] `crates/edit-model/src/geo_group.rs`: `GeoGroup { Crop, Straighten, Perspective }`, `ALL`,
  `name()`; `Edits::{geo_group_differs, reset_geo_group}` via `copy_geo_group` (exhaustive
  `Geometry { orientation, angle, crop }` destructure). Three tests as in the spec. Commit.

### Task 2: the panel
- [ ] `tool.rs`: `Crop => None`; the unheaded test → `[Edit, Crop, Masks]`.
- [ ] `geometry.rs`: rewrite `show` into `crop`, `straighten`, `perspective`, `lens` bodies under
  four headers; `Chip` gains `PartialEq`; `PRESETS` label "W:H"; `level_button`; drop the
  hint lines and Reset geometry.
- [ ] Tests re-targeted per the spec; bin tests green. Commit.

### Task 3: gate, GUI, docs
- [ ] Gate; GUI; journal + CLAUDE.md state; commit; report.
