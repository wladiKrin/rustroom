# Canvas Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the view code out of `main.rs` into focused modules, splitting the 380-line `develop_body` into an adjustments panel and an image canvas.

**Architecture:** Free functions taking `&mut RustroomApp`, in binary-crate modules declared in `main.rs` — the same conventions the modal split established. `main.rs` is left holding state types, decode functions and lifecycle orchestration.

**Tech Stack:** Rust, egui 0.35. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-13-canvas-split-design.md`

**Build note:** every `cargo` command needs libraw and lcms2 on the pkg-config path. Export once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** (explicit user preference). Never `git push` — the user pushes themselves.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/app/src/thumb.rs` | **New.** `cell` (was `thumb_cell`), `selection_border` (was `paint_selection_border`), and the six tests that pin cell layout and click hit-testing. | 1 |
| `crates/app/src/library.rs` | **New.** The Library grid, `grid_columns`, and its two grid-rule tests. | 2 |
| `crates/app/src/filmstrip.rs` | **New.** The Develop filmstrip. | 2 |
| `crates/app/src/theme.rs` | Gains `caption`. | 2 |
| `crates/app/src/develop/canvas.rs` | **New.** The central image canvas, the pending placeholder, the crop overlay. | 3 |
| `crates/app/src/develop/panel.rs` | **New.** The right adjustments panel and the histogram. | 3 |
| `crates/app/src/develop/mod.rs` | **New.** `body` orchestration, `toolbar`, `preset_panel`. | 3 |
| `crates/app/src/shell.rs` | **New.** `impl eframe::App for RustroomApp` — the frame loop and view dispatch. | 4 |
| `crates/app/src/main.rs` | Declares the modules; loses ~1,350 lines. | 1–4 |

### ⚠ Conventions inherited from the modal split — do not rediscover them

- **Binary modules, not lib modules.** `lib.rs` says "no egui here — unit-testable units live in this lib", and that is what keeps `history`, `selection` and the rest testable without a windowing system. Every module here is declared with `mod x;` in `main.rs`.
- **Consequence:** `pub` does not escape the binary, so **every item needs a real caller or `clippy -D warnings` fails on dead code**. This is why `theme::caption` is introduced in task 2, alongside its first caller, not up front.
- **⚠ Privacy runs in two directions and both bit last time.** These modules reach `RustroomApp`'s **private fields** with no `pub` anywhere, because privacy is "visible in the declaring module *and its descendants*". But `main.rs` is the **ancestor**, so each submodule of `develop/` needs `pub mod`, and the top-level ones need `mod` (they are siblings of `main.rs`'s items, reached as `crate::thumb::…`).
- **The bodies are deliberately not reproduced below.** Every move says "move the body of `X`, replacing `self.` with `app.`" rather than printing existing code. Transcribing it would create a second copy that can differ from the first, turning a plan typo into a behavioural bug. The substitution is mechanical and the compiler verifies it exhaustively.
- **⚠ `self` appears bare as an argument, not only as `self.`.** Dispatch sites read `modals::export::collision(self, ctx)`. A blind `self.` → `app.` substitution misses those and produces `expected value, found module 'self'`. Substitute `(self, ctx)` → `(app, ctx)` as well.

---

## Task 1: `thumb.rs` — the shared thumbnail cell

`thumb_cell` is called by the Library grid, the filmstrip **and** six tests. Moving it first means the later moves have a stable target.

**Files:**
- Create: `crates/app/src/thumb.rs`
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Create the module and move both functions**

Create `crates/app/src/thumb.rs` with this header, then move `fn thumb_cell` (around `main.rs:2302`) and `fn paint_selection_border` (around `main.rs:2268`) into it **verbatim**, renaming them `cell` and `selection_border` and making both `pub`:

```rust
//! The thumbnail cell, shared by the Library grid and the Develop filmstrip.
//!
//! ⚠ **`cell` allocates first and paints into the allocated rect.** That is the whole point of
//! it: `Ui::allocate_ui` allocates the space its content *actually used*, so an earlier version
//! that drew an `Image` directly gave a portrait thumbnail a 58.6 px cell where its placeholder
//! had taken 132 — every thumbnail that arrived shifted every cell to its right, and the
//! filmstrip's centred photo drifted as its predecessors loaded.
//!
//! ⚠ **It senses `hover`, not `click`, and that is load-bearing.** In the Library grid the
//! click target is the whole cell — thumbnail *and* filename — so two overlapping click-sensing
//! widgets both sit at distance 0 from the pointer, and egui breaks that tie by registration
//! order: the thumbnail swallows the click and only the grey margin opens the photo. `cell`
//! reserves space and paints; interaction is the caller's to add, which the filmstrip does with
//! `.interact(egui::Sense::click())`.

use eframe::egui;
```

In `crates/app/src/main.rs`, add beside `mod modals;`:

```rust
mod thumb;
```

Update the four production call sites (`main.rs:1826`, `1828`, `1931`, `1940`) from `thumb_cell(` to `thumb::cell(` and `paint_selection_border(` to `thumb::selection_border(`.

```bash
cargo check -p app 2>&1 | tail -3
```

Expected: clean, apart from `dead_code` on nothing — both functions have callers.

- [ ] **Step 2: Move the six cell tests**

Move these from `main.rs`'s `#[cfg(test)] mod tests` into a `#[cfg(test)] mod tests` in `thumb.rs`, together with the helpers they use — `cell_widths`, `grid_cell_size`, `grid_cell_click_lands`, `filmstrip_click_lands`:

1. `the_old_cell_shrank_when_a_portrait_thumbnail_arrived`
2. `a_thumb_cell_keeps_its_width_however_its_thumbnail_loads`
3. `a_library_grid_cell_keeps_its_size_however_its_thumbnail_loads`
4. `clicking_a_library_grid_thumbnail_opens_it`
5. `clicking_a_filmstrip_thumbnail_opens_it`
6. `cells_are_laid_out_at_a_constant_pitch`

Each calls `thumb_cell(` — rename those calls to `cell(` (they are inside `thumb.rs` now, so no path prefix).

⚠ These tests **reimplement the layout loop rather than driving the real one**. They pin the *rule*, not the call site, so a pure move cannot break them — their passing is not strong evidence about this refactor. They move because they document `cell`'s rules and belong beside it.

⚠ **Leave `grid_columns_leaves_room_for_the_gaps`, `the_grid_breaks_rows_by_position_not_by_entry_index` and the `grid_rows` helper in `main.rs` for now.** They pin the *grid's* rules, not the cell's, and they move in task 2 with `library.rs`.

- [ ] **Step 3: Run the tests**

```bash
cargo test -p app --bin Rustroom
```

Expected: `test result: ok. 11 passed` — the same eleven as before, six now reported under `thumb::tests::`.

- [ ] **Step 4: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, 527 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/thumb.rs crates/app/src/main.rs
git commit -m "refactor(app): extract the shared thumbnail cell"
```

---

## Task 2: `library.rs`, `filmstrip.rs`, and `theme::caption`

Both views call `thumb::cell`, and both hold one of `caption`'s two sites — so `caption` is introduced here with a real caller, and gains its second in task 3.

**Files:**
- Create: `crates/app/src/library.rs`, `crates/app/src/filmstrip.rs`
- Modify: `crates/app/src/theme.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Add `caption` to `theme.rs`**

Below `hint`:

```rust
/// A caption: a filename under a thumbnail, a camera model beside a title.
///
/// ⚠ Renders identically to [`hint`] today and means something different — a hint is
/// explanatory text *under a control*, a caption labels a thing. Kept separate so the visual
/// identity pass can move one without the other. Merging two idioms because they currently
/// look alike is how a design system loses the distinctions it exists to hold.
pub fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small());
}
```

- [ ] **Step 2: Create `library.rs`**

Create `crates/app/src/library.rs`:

```rust
//! The Library grid: a page of thumbnails, filtered by format, with selection.

use crate::{theme, thumb, RustroomApp};
use eframe::egui;

/// The Library view: the thumbnail grid and its format-filter chips.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // …body of `library_view`, `self.` → `app.`…
}
```

Move `fn library_view` (around `main.rs:1859`) and the free `fn grid_columns` (around `main.rs:2261`) into it. Make `grid_columns` private to the module — it has no caller outside.

⚠ Inside the moved body, change the filename label to use the new helper:

```rust
theme::caption(ui, &lib.entries[i].file_name);
```

replacing the `ui.label(egui::RichText::new(&lib.entries[i].file_name).small())` form.

⚠ `grid_columns`' doc comment carries the reason it exists — that `n` cells need
`n*CELL + (n-1)*spacing`, and that the failure rate *rises with window width* because the gap
total grows with the column count while the leftover width stays uniform in `[0, CELL)`. Move
it with the function; that explanation is why the bug went unnoticed on a small monitor.

Move `grid_columns_leaves_room_for_the_gaps`, `the_grid_breaks_rows_by_position_not_by_entry_index` and the `grid_rows` helper into a `#[cfg(test)] mod tests` in `library.rs`.

- [ ] **Step 3: Create `filmstrip.rs`**

Create `crates/app/src/filmstrip.rs`:

```rust
//! The Develop filmstrip: the loaded folder's thumbnails along the bottom.

use crate::{thumb, RustroomApp};
use eframe::egui;

/// The bottom filmstrip. Resizable; thumbnails scale with the panel height.
///
/// ⚠ `ui.set_min_height(ui.available_height())` is load-bearing: a resizable egui panel stores
/// its *content's* rendered rect as its size each frame, so content smaller than the panel
/// makes it shrink back every frame. Applies to any future resizable panel whose content does
/// not fill the resize axis.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // …body of `filmstrip`, `self.` → `app.`…
}
```

Move `fn filmstrip` (around `main.rs:1804`) into it.

- [ ] **Step 4: Declare and repoint**

In `main.rs`, add beside `mod thumb;`:

```rust
mod filmstrip;
mod library;
```

Replace the two call sites:

- `self.library_view(ui, ctx)` → `library::show(self, ui, ctx)`
- `self.filmstrip(ui, ctx)` → `filmstrip::show(self, ui, ctx)`

```bash
cargo check -p app 2>&1 | tail -3
```

Expected: clean. ⚠ `CELL` is a **function-local** `const` declared inside `library_view`, not a crate-level one — it moves with the body and needs no import.

- [ ] **Step 5: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, 527 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/library.rs crates/app/src/filmstrip.rs crates/app/src/theme.rs crates/app/src/main.rs
git commit -m "refactor(app): move the Library grid and filmstrip into their own modules"
```

---

## Task 3: `develop/` — the panel/canvas split

The substantive task. `develop_body` runs `main.rs:1396–1773` and has a clean internal seam:

| lines | what | goes to |
|---|---|---|
| 1396–1405 | header, `handle_develop_arrows`, `photo_edited` decl, `preset_panel` call | `mod.rs` |
| 1406–1599 | `egui::Panel::right("panel")` — the adjustments | `panel.rs` |
| 1601–1603 | the filmstrip call | `mod.rs` |
| 1605–1758 | `egui::CentralPanel` — the image | `canvas.rs` |
| 1760–1772 | recording `photo_edited` in the action log | `mod.rs` |

⚠ `photo_edited` is written in exactly two places: `main.rs:1585` (a slider settling, inside the panel) and `main.rs:1722` (`draw_crop_overlay`, inside the canvas). One per half — which is what makes the return-value split clean.

**Files:**
- Create: `crates/app/src/develop/mod.rs`, `panel.rs`, `canvas.rs`
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Create `develop/canvas.rs`**

```rust
//! The central image canvas: the rendered photo, zoom and pan, and the crop overlay.

use crate::RustroomApp;
use eframe::egui;

/// Draw the image canvas.
///
/// Returns whether a photo edit settled into a new history entry — the crop overlay is the one
/// thing here that changes `Edits`. The caller ORs this with the panel's answer and records a
/// single action-log entry, so one gesture stays one Ctrl+Z.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    let mut photo_edited = false;
    // …lines 1605–1758 of `develop_body`, `self.` → `app.`…
    photo_edited
}
```

Also move `fn draw_pending_canvas` (around `main.rs:2110`) and `fn draw_crop_overlay` (around `main.rs:2147`) into `canvas.rs` as private functions — hence the bare `use crate::RustroomApp;` above, with no import for either. Keep `draw_crop_overlay`'s existing doc comment: it explains that it returns whether a drag settled, for the same borrow reason.

- [ ] **Step 2: Create `develop/panel.rs`**

```rust
//! The right-hand adjustments panel: histogram, then every Develop control.

use crate::{theme, RustroomApp};
use eframe::egui;

/// Draw the adjustments panel.
///
/// Returns whether a photo edit settled into a new history entry.
///
/// ⚠ One history entry per *settled* drag, not per changed frame: the `slider` closure records
/// `r.drag_stopped()` separately from `r.changed()`, so dragging Exposure across its range is
/// one Ctrl+Z rather than two hundred.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui) -> bool {
    let mut photo_edited = false;
    // …lines 1406–1599 of `develop_body`, `self.` → `app.`…
    photo_edited
}
```

Also move `fn draw_histogram` (around `main.rs:2346`) into `panel.rs` as a private function — its only caller is at the top of this panel.

⚠ Inside the moved body, change the camera-model label to the new helper:

```rust
theme::caption(ui, &s.make_model);
```

- [ ] **Step 3: Create `develop/mod.rs`**

```rust
//! The Develop view: toolbar, preset panel, adjustments panel, canvas.

pub mod canvas;
pub mod panel;

use crate::{filmstrip, RustroomApp};
use eframe::egui;

/// The Develop editor body: preset panel, adjustments panel, filmstrip, canvas.
///
/// ⚠ `photo_edited` is collected from both halves and recorded **once**, after they have both
/// run. Each half sets it inside a closure that already holds `&mut app` through `app.session`,
/// so neither can touch the action log itself — which is why they return a `bool` instead.
pub fn body(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // …lines 1396–1405 and 1760–1772, with the two halves called in between…
}

/// Develop-mode toolbar buttons, shown in the top panel when a photo is open.
pub fn toolbar(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // …body of `develop_toolbar`, `self.` → `app.`…
}

/// The left preset panel: save the current edits as a preset, apply one, or delete one.
pub fn preset_panel(app: &mut RustroomApp, ui: &mut egui::Ui) {
    // …body of `preset_panel`, `self.` → `app.`…
}
```

The body of `body` is the header and footer of the old `develop_body`, with the two halves called between them:

```rust
    app.handle_develop_arrows(ctx);
    let mut photo_edited = false;
    if app.show_presets && app.session.is_some() {
        preset_panel(app, ui);
    }
    photo_edited |= panel::show(app, ui);
    if app.show_filmstrip && app.library.is_some() {
        filmstrip::show(app, ui, ctx);
    }
    photo_edited |= canvas::show(app, ui, ctx);
    // Recorded in the same order as everything else, so "delete a preset, move a slider,
    // Ctrl+Z twice" undoes the slider and then restores the preset.
    if photo_edited {
        if let Some(path) = app.open_photo() {
            // ⚠ `Some(path.as_path())`, not `Some(&path)`: the parameter is `Option<&Path>`,
            // and deref coercion does not reach inside `Some`.
            app.actions.push(
                ActionKind::PhotoEdit { path: path.clone() },
                Some(path.as_path()),
            );
        }
    }
```

⚠ Add `use rustroom_app::action_log::ActionKind;` to `develop/mod.rs`'s imports for that last block.

⚠ **Panel order matters.** egui allocates side panels in call order, so `preset_panel` (left) → adjustments (right) → filmstrip (bottom) → canvas (central) must be preserved exactly. The central panel takes what is left, so calling it earlier would starve the others.

- [ ] **Step 4: Declare and repoint**

In `main.rs`, add beside `mod filmstrip;`:

```rust
mod develop;
```

Delete `develop_body`, `develop_toolbar`, `preset_panel`, `draw_pending_canvas`, `draw_crop_overlay` and `draw_histogram`. Repoint their call sites:

- `self.develop_body(ui, ctx)` → `develop::body(self, ui, ctx)`
- `self.develop_toolbar(ui, ctx)` → `develop::toolbar(self, ui, ctx)`

```bash
cargo check -p app 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 5: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, 527 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/develop crates/app/src/main.rs
git commit -m "refactor(app): split the Develop body into an adjustments panel and a canvas"
```

---

## Task 4: `shell.rs` — the frame loop

**Files:**
- Create: `crates/app/src/shell.rs`
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Move the trait impl**

Create `crates/app/src/shell.rs`:

```rust
//! The frame loop: the top toolbar, the Library/Develop dispatch, and the per-frame pumps.

use crate::{develop, library, modals, AppView, RustroomApp};
use eframe::egui;

impl eframe::App for RustroomApp {
    // …the whole `fn ui` body, unchanged — `self` stays `self` here…
}
```

⚠ **This one is not a `self.` → `app.` substitution.** It is a trait impl moving to another module of the same crate, which Rust allows, so `self` remains `self` and the body is copied verbatim. Only the `use` list is new — the list above is a starting point, so add or remove to match what the body actually references and let the compiler drive it. Unused imports are a clippy error under `-D warnings`.

In `main.rs`, add beside `mod develop;`:

```rust
mod shell;
```

and delete the `impl eframe::App for RustroomApp` block.

- [ ] **Step 2: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, 527 tests pass.

- [ ] **Step 3: Confirm the shape of the result**

```bash
wc -l crates/app/src/main.rs crates/app/src/shell.rs crates/app/src/thumb.rs crates/app/src/library.rs crates/app/src/filmstrip.rs crates/app/src/develop/*.rs
```

Expected: `main.rs` around **1,300–1,500**, down from 2,762; no other file over ~350.

```bash
grep -cE "fn develop_body|fn library_view|fn thumb_cell|fn draw_crop_overlay" crates/app/src/main.rs
```

Expected: `0`.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/shell.rs crates/app/src/main.rs
git commit -m "refactor(app): move the frame loop into its own module"
```

---

## Task 5: Verify and document

**Files:** `CLAUDE.md`

- [ ] **Step 1: Launch the GUI**

```bash
cargo run --release -p app
```

- [ ] **Step 2: Walk both views**

This refactor should be **pixel-identical**. Anything visibly different is a regression.

1. **Library** — open a folder; the grid fills in, the format chips filter it, clicking a thumbnail selects and double-click opens. Resize the window: columns reflow without the last one overflowing.
2. **Develop** — sliders move the image; a full drag of Exposure is **one** Ctrl+Z, not many.
3. **Crop** — enter Geometry mode, drag the rectangle and the Straighten slider; one undo step per settled drag.
4. **Filmstrip** — click a thumbnail to switch photos; drag its top border to resize and confirm it does not shrink back on the next frame.
5. **Zoom** — scroll to 1:1 and pan; the region re-renders rather than magnifying.
6. **Presets panel** — toggle it on the left; applying a preset is one undo step.

- [ ] **Step 3: Update `CLAUDE.md`**

Add a section in the "Current status" block, house style: the line counts before and after; the panel/canvas split and why `photo_edited` became a return value; that panel *order* is load-bearing for egui's layout; `theme::caption` and why it stays separate from `hint`; which tests moved where and the honest note that the relocated layout tests pin rules rather than call sites; and the caveat that `main.rs` is still ~1,400 lines with the orchestration layer as a possible third split that is **not** being proposed.

Then set the **▶ NEXT ACTION** to **Phase 3**, noting it is a phase rather than a feature and needs decomposing into sub-projects during brainstorming. Add this spec and plan to the docs list at the bottom, and update the crate-layout line's `bin-only modules=` list to include the new modules.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-13-canvas-split.md
git commit -m "docs: record the canvas split"
```
