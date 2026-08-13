# Modal Split + Theme Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the nine modal dialogs out of `main.rs` into a `modals/` module tree, and introduce `theme` as the one place the app's look is defined.

**Architecture:** Dialogs become free functions taking `&mut RustroomApp` in `crates/app/src/modals/`, grouped by what they serve. `theme` owns the `egui::Style` (reproducing today's look exactly) plus two named idioms, `hint` and `button_row`. Both are **binary-crate modules**, not lib modules.

**Tech Stack:** Rust, egui 0.35. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-13-modal-split-and-theme-design.md`

**Build note:** every `cargo` command needs libraw and lcms2 on the pkg-config path. Export once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** (explicit user preference). Never `git push` — the user pushes themselves.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/app/src/theme.rs` | **New.** The `egui::Style` and `hint` (task 1); `button_row` joins in task 2, when its first caller moves. | 1, 2 |
| `crates/app/src/modals/mod.rs` | **New.** The `Modal` enum, the `draw` dispatch, the `dialog()` helper, and the borrow idiom's documentation. | 2 |
| `crates/app/src/modals/settings.rs` | **New.** The Settings window. | 2 |
| `crates/app/src/modals/edits.rs` | **New.** Paste, sync. | 2 |
| `crates/app/src/modals/presets.rs` | **New.** Save preset, confirm delete preset. | 2 |
| `crates/app/src/modals/export.rs` | **New.** Export settings, save export preset, confirm delete export preset, collision. | 2 |
| `crates/app/src/main.rs` | Declares both modules, applies the style, loses ~700 lines. | 1, 2 |

### ⚠ Both are binary modules, not lib modules

`crates/app/src/lib.rs` opens with *"Rustroom GUI logic (**no egui here** — unit-testable units live in this lib)."* That separation is deliberate and load-bearing: it is what keeps `history`, `selection`, `action_log` and the rest testable without a windowing system. `theme` and `modals` are irreducibly egui, so they are declared with `mod theme;` / `mod modals;` in **`main.rs`**, exactly as `format_arg` is in the CLI crate.

Consequence: `pub` inside them does not escape the binary, so **every item must actually be used or `clippy -D warnings` fails on dead code** — the same trap the CLI's `format_arg` hit.

That constraint sets the shape of the tasks. Task 1 introduces only `style()` and `hint()`, both of which get callers in the same task. **`button_row` is introduced in task 2, at the moment its first real caller moves** — introducing it earlier would mean an unused function and an `#[allow(dead_code)]` put in and taken straight back out.

### Task order, and why

**Task 1 changes content but not location. Task 2 changes location but not content.** Keeping those apart is what makes task 2 reviewable: it should read as a pure move, so any behavioural difference in its diff is a mistake. Doing it the other way round would mean editing hint sites inside files that were themselves in motion.

---

## Task 1: `theme` — the app's visual vocabulary

**Files:**
- Create: `crates/app/src/theme.rs`
- Modify: `crates/app/src/main.rs` — module declaration, `ctx.set_style()` in `new`, four hint call sites

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/theme.rs` containing **only** this:

```rust
//! The one place Rustroom's look is defined.

use eframe::egui;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_style_is_constructible() {
        // ⚠ Thin on purpose. `style()` is a pile of field assignments; what could actually
        // break is it panicking at startup, which would take the whole app with it.
        let s = style();
        assert!(s.text_styles.contains_key(&egui::TextStyle::Body));
    }

    #[test]
    fn hint_text_is_smaller_than_body_text() {
        // ⚠ Today this passes on egui's defaults, so it is a **tripwire, not a discovery**:
        // it starts earning its keep the moment `style()` begins setting `text_styles`
        // itself, at which point a hint that reads at body size is no longer a hint.
        let s = style();
        let body = s.text_styles[&egui::TextStyle::Body].size;
        let small = s.text_styles[&egui::TextStyle::Small].size;
        assert!(small < body, "small {small} should be under body {body}");
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/app/src/main.rs`, add below the `use` block, next to the other declarations:

```rust
mod theme;
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p app --bin rustroom-gui theme
```

Expected: **compile error**, `cannot find function style in this scope` (twice).

- [ ] **Step 4: Write the implementation**

Replace the contents of `crates/app/src/theme.rs` above the `#[cfg(test)]` module with:

```rust
//! The one place Rustroom's look is defined.
//!
//! ⚠ This currently **reproduces egui's defaults exactly** — it changes nothing about how the
//! app looks. That is the entire point of the step: the look has never been designed, and this
//! is the seam through which it will be, in one file rather than in two hundred call sites.
//!
//! The split that makes that cheap: anything expressible in `egui::Style`/`Visuals` — colours,
//! spacing, rounding, fonts — is a central change applied retroactively everywhere, so those
//! choices are cheap to make late. What is expensive late is *structural*: whether buttons
//! carry icons, whether sections have headers, how a control row is composed — which is why
//! this module grows helpers that encode conventions, not just colours.

use eframe::egui;

/// The application style, applied once at startup.
pub fn style() -> egui::Style {
    // Deliberately begins from egui's default and changes nothing. Every future visual
    // decision is a line added here.
    egui::Style::default()
}

/// The small explanatory line under a control.
///
/// ⚠ Distinct from a *caption* — a filename under a thumbnail, a camera model beside a title —
/// which renders identically today but means something different. Those live in the Library
/// grid and the develop toolbar and are deliberately left alone until the canvas split, where
/// their call sites are. Two idioms that merely look alike should not be merged into one
/// helper.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small());
}
```

⚠ `button_row` is **not** added here. It has no caller until task 2 moves the first dialog with a confirm/dismiss pair, and an unused `pub fn` in a binary crate is a `dead_code` error under `-D warnings`.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app --bin rustroom-gui theme
```

Expected: `test result: ok. 2 passed`.

- [ ] **Step 6: Apply the style at startup**

In `RustroomApp::new` in `crates/app/src/main.rs`, add as the **first** statement of the function body, before `let ctx2 = ctx.clone();`:

```rust
        ctx.set_style(theme::style());
```

- [ ] **Step 7: Convert the four hint call sites**

All four are inside dialogs. Each is currently `ui.label(egui::RichText::new(<text>).small());` possibly spread over several lines.

**(a)** In `confirm_delete_export_modal` (around `main.rs:1595`), replace:

```rust
            ui.label(
                egui::RichText::new("This cannot be undone — export presets have no trash.")
                    .small(),
            );
```

with:

```rust
            theme::hint(ui, "This cannot be undone — export presets have no trash.");
```

**(b)** In `export_modal` (around `main.rs:1776`), replace:

```rust
            ui.label(
                egui::RichText::new(
                    "RAW only. Higher is slower — roughly 1.2 s to 11 s per photo. \
                     Independent of the 1:1 view's own quality setting, so judging this \
                     means exporting a frame at two settings and comparing crops.",
                )
                .small(),
            );
```

with:

```rust
            theme::hint(
                ui,
                "RAW only. Higher is slower — roughly 1.2 s to 11 s per photo. \
                 Independent of the 1:1 view's own quality setting, so judging this \
                 means exporting a frame at two settings and comparing crops.",
            );
```

**(c)** In `settings_modal` (around `main.rs:1993`), replace:

```rust
                ui.label(egui::RichText::new(view_quality_hint(tier)).small());
```

with:

```rust
                theme::hint(ui, view_quality_hint(tier));
```

**(d)** In `settings_modal` (around `main.rs:2000`), replace:

```rust
            ui.label(
                egui::RichText::new(
                    "RAW only, and separate from the export setting. Sets the demosaic used \
                     for the zoomed view. A slower tier costs that much extra on every photo \
                     you stop on, not just on export — the full-resolution decode starts on \
                     its own. The measured difference is small, which is why the default is \
                     Fast. Changing this re-decodes the open photo.",
                )
                .small(),
            );
```

with:

```rust
            theme::hint(
                ui,
                "RAW only, and separate from the export setting. Sets the demosaic used \
                 for the zoomed view. A slower tier costs that much extra on every photo \
                 you stop on, not just on export — the full-resolution decode starts on \
                 its own. The measured difference is small, which is why the default is \
                 Fast. Changing this re-decodes the open photo.",
            );
```

⚠ Do **not** convert the `.small()` uses at roughly `main.rs:2229` (the camera model beside the filename) or `main.rs:2568` (the filename under a Library thumbnail). Those are captions, not hints, and they belong to the canvas split. The two in the test module are fixtures and stay as they are.

- [ ] **Step 8: Confirm exactly four sites moved**

```bash
grep -c "theme::hint" crates/app/src/main.rs
```

Expected: `4`.

```bash
grep -c "small()" crates/app/src/main.rs
```

Expected: `4` — the two captions and the two test fixtures.

- [ ] **Step 9: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, all tests pass. `style` and `hint` both have callers, so there should be no `dead_code` report — if there is one, a step above was missed.

- [ ] **Step 10: Commit**

```bash
git add crates/app/src/theme.rs crates/app/src/main.rs
git commit -m "feat(app): a theme module, and hint labels routed through it"
```

---

## Task 2: Move the dialogs into `modals/`

Every step below leaves the tree compiling. Run `cargo check -p app` after each move; the compiler is the safety net here, and it is a good one because every dialog is reached from exactly one place.

⚠ **The dialog bodies are deliberately not reproduced in this plan.** Every step below says "move the body of `X`, replacing `self.` with `app.`" rather than printing ~700 lines of existing code. That is not laziness: this is a *move*, and the source of truth is `main.rs`. Transcribing those bodies into the plan would introduce a second copy that can differ from the first, turning a plan typo into a behavioural bug — whereas "move it verbatim" cannot drift. The substitution is mechanical and the compiler verifies it exhaustively.

**Files:**
- Create: `crates/app/src/modals/mod.rs`, `settings.rs`, `edits.rs`, `presets.rs`, `export.rs`
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Create the module with the dialog helper**

Create `crates/app/src/modals/mod.rs`:

```rust
//! The modal dialogs.
//!
//! Each dialog is a free function taking `&mut RustroomApp`. That is deliberately more
//! explicit than an `impl RustroomApp` block in another file, which would have been a pure
//! cut-and-paste with zero call-site churn — and would have kept every dialog silently able to
//! reach anything on `self`, teaching a reader nothing.
//!
//! ⚠ **No visibility changes were needed for this.** Rust privacy is "visible in the declaring
//! module *and its descendants*", and this module is a descendant of the crate root where
//! `RustroomApp` lives, so its private fields are reachable without a single `pub`.
//!
//! # The borrow idiom
//!
//! Every dialog follows the same shape, and it is load-bearing rather than incidental:
//!
//! ```ignore
//! let mut local = app.settings.field;   // copy out
//! let mut close = false;
//! let dismissed = dialog(ctx, "id", "Title", |ui| { /* mutate locals only */ });
//! close |= dismissed;
//! if local != app.settings.field { /* apply */ }
//! ```
//!
//! The closure cannot touch `app`, because `dialog` already holds it. Reaching for `app`
//! inside the closure has cost compile errors twice in this codebase's history.
//!
//! ⚠ **A dialog body runs every frame it is open.** Any side effect must therefore be guarded
//! by a change comparison — `if tier != app.settings.view_demosaic` — never fired
//! unconditionally. Without that guard, `apply_view_demosaic` would rebuild a decode worker
//! and start an 11-second decode sixty times a second.

use crate::RustroomApp;
use eframe::egui;

/// Show a centred modal with a heading and a separator. Returns whether the backdrop was
/// clicked, which every dialog treats as a dismissal.
///
/// ⚠ `egui::Modal`, not `egui::Window`: a Window opens at the top-left and is genuinely easy
/// to miss, while a Modal centres on the viewport, dims the backdrop and blocks clicks behind
/// it.
pub fn dialog<R>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> bool {
    egui::Modal::new(egui::Id::new(id))
        .show(ctx, |ui| {
            ui.heading(title);
            ui.separator();
            content(ui);
        })
        .backdrop_response
        .clicked()
}
```

In `crates/app/src/main.rs`, add beside `mod theme;`:

```rust
mod modals;
```

Delete `fn modal_dialog` (around `main.rs:2877`) and replace every call to `modal_dialog(` with `modals::dialog(`. There are nine.

```bash
cargo check -p app 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 2: Move the `Modal` enum**

Cut the `Modal` enum and its doc comment (around `main.rs:100–140`) into `modals/mod.rs`, directly below the `use` lines, and make it public:

```rust
/// Which modal dialog is open. At most one at a time, so this is an enum rather than a pile
/// of booleans that could contradict each other.
#[derive(Default, PartialEq)]
pub enum Modal {
    // …the variants, unchanged…
}
```

`Modal`'s variants reference `EditGroups`, so add to `modals/mod.rs`'s imports:

```rust
use edit_model::EditGroups;
```

In `main.rs`, delete the enum and add `use modals::Modal;` beside the other `use` lines, so every existing `Modal::…` reference keeps working unchanged.

```bash
cargo check -p app 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 3: Move the Settings dialog**

Create `crates/app/src/modals/settings.rs`:

```rust
//! The Settings window.

use super::dialog;
use crate::{theme, view_quality_hint, Modal, RustroomApp};
use eframe::egui;

/// The Settings window: preferences that survive a restart.
pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    // …the body of `settings_modal`, with every `self.` replaced by `app.`…
}
```

Move the body of `settings_modal` across verbatim, replacing `self.` with `app.` throughout, and `self.apply_view_demosaic(tier, ctx)` with `app.apply_view_demosaic(tier, ctx)`.

⚠ **Settings keeps its plain `ui.button("Close")`.** It is the one dialog with nothing to confirm — its changes apply immediately — so it has no confirm/dismiss *pair* to encode. Forcing it through `button_row` would mean an always-disabled empty button, which is worse than what it replaces. `button_row` serves the eight dialogs that genuinely have a pair; it arrives in step 4.

Add `mod settings;` to `modals/mod.rs`, delete `settings_modal` from `main.rs`, and change its dispatch site in `draw_modals` to `modals::settings::show(self, ctx)`.

```bash
cargo check -p app 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 4: Move the paste and sync dialogs, and introduce `button_row`**

First add to `crates/app/src/theme.rs`, below `hint`:

```rust
/// The trailing confirm/dismiss button pair.
///
/// Returns `(confirmed, dismissed)`.
///
/// ⚠ This **encodes the order the dialogs already use** — confirm first, dismiss second — it
/// does not impose a new one. An audit of all nine before this refactor found them already
/// uniform, and "Close" appearing only in Settings, the one dialog with nothing to abandon.
/// Its job is to stop that drifting as dialogs are added, which is why it is worth having
/// even though it changes nothing today.
pub fn button_row(
    ui: &mut egui::Ui,
    confirm: &str,
    confirm_enabled: bool,
    dismiss: &str,
) -> (bool, bool) {
    let mut confirmed = false;
    let mut dismissed = false;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(confirm_enabled, egui::Button::new(confirm))
            .clicked()
        {
            confirmed = true;
        }
        if ui.button(dismiss).clicked() {
            dismissed = true;
        }
    });
    (confirmed, dismissed)
}
```

Then create `crates/app/src/modals/edits.rs`:

```rust
//! Dialogs that move edits between photos: paste, and sync.

use super::dialog;
use crate::{edit_group_checkboxes, paste_status, theme, Modal, RustroomApp};
use eframe::egui;

/// The "Paste…" dialog: which groups this paste applies.
pub fn paste(app: &mut RustroomApp, ctx: &egui::Context) {
    // …body of `paste_modal`, `self.` → `app.`…
}

/// The "Sync…" dialog: which groups this sync applies to the rest of the selection.
pub fn sync(app: &mut RustroomApp, ctx: &egui::Context) {
    // …body of `sync_modal`, `self.` → `app.`…
}
```

Both dialogs' trailing button pairs become `theme::button_row(ui, "Paste", true, "Cancel")` and `theme::button_row(ui, "Sync", true, "Cancel")` respectively.

Add `mod edits;` to `modals/mod.rs`, delete both functions from `main.rs`, and point the two dispatch sites at `modals::edits::paste(self, ctx)` / `modals::edits::sync(self, ctx)`.

⚠ `edit_group_checkboxes` and `paste_status` are free functions in `main.rs`. They stay there; the `use crate::{…}` line above imports them. If the compiler reports them as private, they are declared without `pub` **and that is fine** — a descendant module reaches them. If it reports them as *unused* in `main.rs`, that means a dispatch site was missed.

```bash
cargo check -p app 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 5: Move the preset dialogs**

Create `crates/app/src/modals/presets.rs`:

```rust
//! Develop-preset dialogs: saving one, and confirming a deletion.

use super::dialog;
use crate::{edit_group_checkboxes, theme, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::settings::ConfirmOutcome;

/// Naming a new preset and choosing the groups it carries.
pub fn save(app: &mut RustroomApp, ctx: &egui::Context) {
    // …body of `save_preset_modal`, `self.` → `app.`…
}

/// Confirming a preset deletion.
///
/// ⚠ `dont_ask_again` is **scratch state** held in the `Modal` variant, not written to
/// `Settings` until Delete is clicked. Ticking the box and then cancelling must leave the
/// guard armed — see `Settings::apply_delete_confirm`, which is where that rule lives and is
/// tested.
pub fn confirm_delete(app: &mut RustroomApp, ctx: &egui::Context) {
    // …body of `confirm_delete_modal`, `self.` → `app.`…
}
```

Their button pairs become `theme::button_row(ui, "Save", <the existing enabled condition>, "Cancel")` and `theme::button_row(ui, "Delete", true, "Cancel")`.

Add `mod presets;` to `modals/mod.rs`, delete both from `main.rs`, and point the dispatch sites at `modals::presets::save(self, ctx)` / `modals::presets::confirm_delete(self, ctx)`.

```bash
cargo check -p app 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 6: Move the export dialogs**

Create `crates/app/src/modals/export.rs` holding all four, in this order: `settings`, `save_preset`, `confirm_delete`, `collision`.

```rust
//! The export flow's dialogs.
//!
//! Four rather than one file because they are one flow: `save_preset` and `confirm_delete`
//! both **return to** `Modal::ExportSettings` when closed, so the export dialog does not
//! vanish underneath the user.

use super::dialog;
use crate::{theme, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::batch_export::{destination_ready, Collision, Decision, ExportOptions};
use rustroom_app::export_presets::ExportPreset;
use rustroom_app::settings::ConfirmOutcome;

/// Choosing where and how to export the selection.
pub fn settings(app: &mut RustroomApp, ctx: &egui::Context) {
    // …body of `export_modal`, `self.` → `app.`…
}

/// Naming a new export preset.
pub fn save_preset(app: &mut RustroomApp, ctx: &egui::Context) {
    // …body of `save_export_preset_modal`, `self.` → `app.`…
}

/// Confirming an export-preset deletion.
///
/// ⚠ Folds into `confirm_export_preset_delete`, **not** the develop-preset flag. A develop
/// preset goes to a trash and can be restored with Ctrl+Z; an export preset is deleted
/// outright, so disarming the guard on the recoverable deletion must not disarm it on the
/// irrecoverable one.
pub fn confirm_delete(app: &mut RustroomApp, ctx: &egui::Context) {
    // …body of `confirm_delete_export_modal`, `self.` → `app.`…
}

/// A destination filename already exists; the export worker is blocked awaiting an answer.
///
/// ⚠ `Decision::Cancel` carries no `apply_to_all`, so ticking "apply to all" and then
/// cancelling arms nothing — that is unrepresentable rather than enforced by a rule.
pub fn collision(app: &mut RustroomApp, ctx: &egui::Context) {
    // …body of `collision_modal`, `self.` → `app.`…
}
```

⚠ The exact `use` list above is a starting point — add or remove to match what the four bodies actually reference, and let the compiler drive it. Unused imports are a clippy error under `-D warnings`.

Add `mod export;` to `modals/mod.rs`, delete all four from `main.rs`, and point the four dispatch sites at `modals::export::settings(self, ctx)`, `modals::export::save_preset(self, ctx)`, `modals::export::confirm_delete(self, ctx)`, `modals::export::collision(self, ctx)`.

```bash
cargo check -p app 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 7: Move the dispatch**

Move `draw_modals` into `modals/mod.rs` as:

```rust
/// Draw whichever dialog is open.
///
/// ⚠ The local `Which` enum is not ceremony: matching directly on `app.modal` keeps the
/// scrutinee borrowed across the `&mut app` call in each arm, which does not compile. Decide
/// *which* dialog into a plain value first, then dispatch.
pub fn draw(app: &mut RustroomApp, ctx: &egui::Context) {
    // …body of `draw_modals`, `self.` → `app.`, each arm calling the moved functions…
}
```

In `main.rs`, replace the call `self.draw_modals(ctx)` with `modals::draw(self, ctx)`.

```bash
cargo check -p app 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 8: Confirm the move actually shrank `main.rs`**

```bash
wc -l crates/app/src/main.rs crates/app/src/modals/*.rs crates/app/src/theme.rs
```

Expected: `main.rs` around **2,700–2,800** lines, down from 3,419; the `modals/` files totalling roughly 700.

```bash
grep -c "_modal" crates/app/src/main.rs
```

Expected: `0` — no dialog function or dispatch name is left behind.

- [ ] **Step 9: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, **all 526 tests pass** (524 before, plus `theme`'s two). Any test failure here is a genuine regression: this task moved code without changing it.

- [ ] **Step 10: Commit**

```bash
git add crates/app/src/modals crates/app/src/main.rs crates/app/src/theme.rs
git commit -m "refactor(app): move the modal dialogs into their own module"
```

---

## Task 3: Verify and document

**Files:** `CLAUDE.md`

- [ ] **Step 1: Launch the GUI**

```bash
cargo run --release -p app
```

- [ ] **Step 2: Open every dialog and confirm nothing changed but the hints**

Open each of the nine and check it renders, its buttons work, and Cancel still abandons:

1. **Settings** (toolbar) — the view-quality combo and its hint; Close.
2. **Save preset** (left Presets panel → "Save current as preset…") — name field, group checkboxes, Save/Cancel.
3. **Confirm delete preset** — delete a preset; the "Don't ask again" tick, Delete/Cancel. ⚠ Tick it, then press **Cancel**: the confirmation must still appear next time.
4. **Paste…** — copy edits, then Paste… ; group checkboxes, Paste/Cancel.
5. **Sync…** — with several photos selected; Sync/Cancel.
6. **Export…** — destination, quality, long edge, format, demosaic and its hint; Export/Cancel.
7. **Save export preset** — from within Export…; and confirm the export dialog is still behind it on close.
8. **Confirm delete export preset** — the "This cannot be undone" hint.
9. **Collision** — export twice to the same folder without changing the name.

The only thing that may look different is the spacing around the four hint lines. Anything else is a regression.

- [ ] **Step 3: Update `CLAUDE.md`**

Add a section in the "Current status" block, house style: what moved and why; that `theme` and `modals` are **binary** modules because the lib is deliberately egui-free; the free-function-over-`impl`-block reasoning; the hint-versus-caption distinction and why captions were left for the canvas split; that the audit found the dialog conventions already uniform, so `button_row` encodes rather than imposes; and the caveat that `main.rs` is still ~2,780 lines with the canvas split as the next step.

Then set the **▶ NEXT ACTION** to the canvas split — `develop_body` at 380 lines, the crop overlay, the Library grid and the filmstrip — noting it is the one that actually clears the way for M3.1's mask UI. Add this spec and plan to the docs list at the bottom.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-13-modal-split-and-theme.md
git commit -m "docs: record the modal split and theme foundations"
```
