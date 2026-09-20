# Presets panel — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Presets panel as a sectioned list: a header with the count, Save current…, one
painted row per preset saying what it carries and lit when the photo already has it, Undo delete
under the list.

**Spec:** `docs/superpowers/specs/2026-09-20-presets-panel-design.md`. **Branch:**
`presets-panel`. No `Co-Authored-By`. Never push. **Gate recipe** as before.

### Task 1: lib helpers
- [ ] `presets::groups_summary`, `Preset::matches`, tests. Commit.

### Task 2: the panel
- [ ] `RowGlyph::Preset` + `mask_glyph::paint_star`; `panel_heading` removed; `presets::show`
  rewritten; tests re-targeted. Commit.

### Task 3: gate, GUI, docs
- [ ] Gate; GUI; journal + CLAUDE.md state; commit; report.
