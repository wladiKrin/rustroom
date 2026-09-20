# Snapshot timeline — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The snapshot strip as a timeline under the section chrome: a Now row with the drift
since the newest snapshot and the Save button, then one dotted row per snapshot with the
accent on the one the photo is at, and copy / delete cells.

**Spec:** `docs/superpowers/specs/2026-09-20-snapshots-timeline-design.md`. **Branch:**
`snapshots-timeline`. No `Co-Authored-By`. Never push. **Gate recipe** as before.

### Task 1: lib pieces
- [ ] `History::steps_since`, `when::short`, `icons::COPY`, `list_row::RowOut::{rect,
  double_clicked}`; tests. Commit.

### Task 2: the strip
- [ ] `snapshots::show` rewritten; tests re-targeted and added. Commit.

### Task 3: gate, GUI, docs
- [ ] Gate; GUI; journal + CLAUDE.md state; commit; report.
