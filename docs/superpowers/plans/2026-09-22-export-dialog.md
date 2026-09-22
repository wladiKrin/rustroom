# Export dialog — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Export dialog at the Wide size with a preset list on the left and summarised,
collapsible sections on the right.

**Spec:** `docs/superpowers/specs/2026-09-22-export-dialog-design.md`. **Branch:**
`export-presets-sheet`. No `Co-Authored-By`. Never push. **Gate recipe** as before.

### Task 1: the summaries
- [ ] `export_summary.rs` with tests. Commit.

### Task 2: the dialog
- [ ] `modals/export.rs` `settings` rewritten over `list_row` and `section::header`; tests
  re-targeted. Commit.

### Task 3: gate, GUI, docs
- [ ] Gate; GUI; journal + CLAUDE.md state; commit; report.
