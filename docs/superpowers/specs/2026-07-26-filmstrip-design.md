# Filmstrip in Develop: Design Spec

**Milestone:** Phase 2 — the step from browse-layout "A" toward "C": a filmstrip along the bottom of the Develop view so you can flip between photos in the current folder while editing.

**Status:** Approved design (2026-07-26). Implementation to follow in one plan.

---

## Goal

While editing in Develop with a folder open, show a **horizontal strip of the folder's thumbnails** at the bottom. Click one — or press **←/→** — to switch to that photo without leaving Develop; the current photo's edits are auto-saved first (lossless). A toggle shows/hides the strip.

## Key idea: reuse `Library` state

The filmstrip is a **horizontal rendering of the existing `Library`** (`entries`, the already-loaded `thumbs`, `selected`) — not a new data source. No new thumbnail loading, no new worker. It's only meaningful when a folder is loaded; a single-file open (`Open file…`) has no folder context, so there's nothing to show.

## Decisions (from brainstorming)

- **Navigation:** click a strip thumbnail **and** ←/→ arrow keys move between photos.
- **Visibility:** a **show/hide toggle** (default on); the strip appears in Develop only when a `Library` is loaded and the toggle is on.
- Grid and filmstrip **share `library.selected`**, so they stay consistent.

---

## Components

### `app` state
- Add `show_filmstrip: bool` to `RustroomApp` (default `true`).
- The "current photo" is `session.path`; `library.selected` tracks its index and is set whenever a photo is opened via the grid or the strip.

### `develop_body` layout
Panel order becomes: **right adjustment panel → bottom filmstrip panel → central image**. The bottom panel is an `egui::Panel::bottom` shown only when `self.show_filmstrip && self.library.is_some()`. It contains a **horizontal `ScrollArea`** of small thumbnail cells (the grid cell, rendered smaller — placeholder gray until a thumb is loaded, image once present). The cell for `library.selected` is outlined. A **Filmstrip** toggle button lives in the Develop toolbar.

### Navigation
- **Click** a strip cell → `open_entry(index)` (the existing method: it persists the current photo's sidecar, decodes the new one, stays in Develop) and sets `library.selected = index`.
- **←/→** → compute prev/next index (clamped to `[0, len-1]`) and `open_entry` it, but **only when no egui widget has keyboard focus** (`ctx.memory(|m| m.focused().is_none())`), so arrows still fine-tune a focused slider.

## Data flow

```
Develop view (folder loaded, show_filmstrip on)
   → bottom Panel renders library.thumbs as a horizontal strip; library.selected highlighted
   → click a cell OR ←/→ (when nothing focused)
       → open_entry(i): persist_current_session (auto-sidecar) → decode photo i → Session
       → library.selected = i  (grid + strip stay in sync)
```

## Error / edge handling

- **No folder loaded** (opened a single file): the filmstrip is simply not shown (nothing to render); the toggle has no effect until a folder is opened.
- A thumbnail still loading shows the gray placeholder in the strip (same as the grid).
- Arrow nav at the ends clamps (first/last photo) — no wraparound.

## Testing strategy

This feature reuses already-tested machinery (`ThumbLoader`, `open_entry`, sidecar persistence, `Library` state); the new code is thin egui view + a trivial prev/next index computation. So: compile + `clippy` + a **manual smoke run** (open a folder, edit a photo, click through the strip and arrow between photos, confirm edits persist per photo, toggle the strip), consistent with prior GUI milestones. No new unit-testable logic or crates.

## Crate structure

Only `crates/app/src/main.rs` changes (state field, a `filmstrip` render method, arrow-nav handling, toolbar toggle). No new crates or files required (a `filmstrip.rs` helper module is optional if `main.rs` grows unwieldy).

## Deferred

Vertical filmstrip orientation, drag-to-reorder, a filmstrip for single-file mode, and rating/flag badges on strip cells.

## Done criteria

With a folder open in Develop: the bottom strip shows the folder's thumbnails with the current photo highlighted; clicking a cell or pressing ←/→ switches photos (edits auto-saved), grid and strip selection stay in sync, and the toggle shows/hides the strip. Single-file editing is unaffected. `cargo test --all` / `clippy --all-targets` / `fmt` green; clean window close.
