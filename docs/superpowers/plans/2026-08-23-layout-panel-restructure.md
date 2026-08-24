# Panel restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Develop panel's thirteen always-open ungrouped sliders plus seven collapsing sections with five collapsible sections that *are* the edit groups, apply the five agreed ordering changes, and split `develop/panel.rs` into a module so the rail work has somewhere to land.

**Architecture:** `develop/panel.rs` (392 lines) becomes `develop/panel/{mod,adjustments,masks,geometry}.rs`. The `slider` closure that currently threads `changed`/`settled` through one 350-line function becomes a small `Edited` accumulator struct passed explicitly to each section — which is what makes the split possible at all, and dissolves the `E0499` borrow workarounds the current file documents. Section headers are read from `edit_model::EDIT_GROUP_NAMES` rather than hardcoded, so the panel and the Paste/Sync dialogs cannot drift apart.

**Tech Stack:** Rust, egui 0.35 (`CollapsingHeader`, `selectable_value`, `Panel::right`), headless `egui::Context::run_ui` + `RawInput` for interaction tests.

**Spec:** `docs/superpowers/specs/2026-08-23-layout-ia-rework-design.md`. This plan is **step 1 of 3**; the rail (step 2) and menu bar (step 3) are separate plans.

**Scope boundary — read this before starting.** Masks and Geometry stay *in the panel* in this plan; they become rail tools in step 2. What changes for them here is only their **position** (Masks moves to the top, where its drawer will later live) and the **order of a mask's adjustment sliders**. `Reset all` stays a button here; it moves into the Edit menu in step 3.

**Deviation from the spec, deliberate:** the spec names the split `{mod, adjustments, drawers}`. This plan uses `{mod, adjustments, masks, geometry}` instead — step 2 turns Masks and Geometry into *separate* drawers, so splitting them now means step 2 moves two whole files rather than carving one apart.

---

## File Structure

| file | responsibility |
|---|---|
| `crates/app/src/develop/panel/mod.rs` | The `Panel::right`, the scroll area, the histogram, the footer, the `Edited` accumulator, and the `changed → request` / `settled → history.push` logic. Orchestration only. |
| `crates/app/src/develop/panel/adjustments.rs` | The five edit-group sections and `HslChannel`. |
| `crates/app/src/develop/panel/masks.rs` | The Masks section: add/select/delete, shape properties, brush tool settings, per-mask adjustments. |
| `crates/app/src/develop/panel/geometry.rs` | The geometry-mode block: rotate/flip buttons, aspect presets, Straighten. |
| `crates/app/src/develop/panel.rs` | **Deleted** (becomes `panel/mod.rs`). |
| `crates/app/src/main.rs:125-143` | Gains `hsl_channel: HslChannel` beside the existing brush tool state. |

---

### Task 1: Move `panel.rs` to `panel/mod.rs` with no behaviour change

A pure move first, so every later diff shows only real changes.

**Files:**
- Move: `crates/app/src/develop/panel.rs` → `crates/app/src/develop/panel/mod.rs`

- [ ] **Step 1: Record the current test count**

Run:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test --all 2>&1 | tail -30
```

Expected: all green. Write the total down; it is the baseline for every later task. As of this plan's writing it is **664 tests, 38 goldens**.

- [ ] **Step 2: Do the move**

```bash
mkdir -p crates/app/src/develop/panel
git mv crates/app/src/develop/panel.rs crates/app/src/develop/panel/mod.rs
```

No other edit is needed: `develop/mod.rs:3` already says `pub mod panel;`, and Rust resolves that to `panel/mod.rs` exactly as it did to `panel.rs`.

- [ ] **Step 3: Verify nothing changed**

```bash
cargo build -p app && cargo test --all 2>&1 | tail -5
```

Expected: same count as Step 1. If the count moved, stop — a pure move cannot change it.

- [ ] **Step 4: Commit**

```bash
git add -A crates/app/src/develop
git commit -m "refactor(app): panel.rs becomes the panel module"
```

---

### Task 2: The `Edited` accumulator replaces the `slider` closure

The current file threads `changed`/`settled` through a single `FnMut` closure captured for 350 lines. Two comments in it (`panel.rs`, the `mask_changed` block) exist solely to work around the fact that a *second* closure capturing the same two locals is `E0499`. A struct with a method has neither problem, and it is what lets the sections become separate functions.

**Files:**
- Modify: `crates/app/src/develop/panel/mod.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/app/src/develop/panel/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_discrete_act_is_both_changed_and_settled() {
        // ⚠ A button press is one history entry and one Ctrl+Z, unlike a slider, which
        // pushes only on `drag_stopped`. Conflating the two is how a single click became
        // either zero undo steps or two.
        let mut e = Edited::default();
        assert!(!e.changed && !e.settled);
        e.discrete();
        assert!(e.changed, "a discrete act must request a render");
        assert!(e.settled, "a discrete act must push one history entry");
    }

    #[test]
    fn an_untouched_frame_records_nothing() {
        // The overwhelmingly common case: the panel runs every frame and must not push a
        // history entry for merely existing.
        let e = Edited::default();
        assert!(!e.changed);
        assert!(!e.settled);
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p app --bin Rustroom a_discrete_act_is_both 2>&1 | tail -20
```

Expected: FAIL — `cannot find type Edited in this scope`.

- [ ] **Step 3: Add `Edited`**

Insert into `crates/app/src/develop/panel/mod.rs`, above `pub fn show`:

```rust
/// Accumulates, over one frame of the panel, whether the photo changed and whether a drag
/// settled into a history entry.
///
/// ⚠ **A struct rather than a closure, and that is what makes the panel splittable.** The
/// previous shape was one `FnMut` capturing `changed` and `settled` for the whole function,
/// so any *second* closure touching either was `E0499` — two simultaneous mutable captures
/// of the same local. Two comments in this file used to describe working around that. A
/// `&mut Edited` passed to each section has neither problem, and it crosses a function
/// boundary, which a capture cannot.
#[derive(Default)]
pub struct Edited {
    /// Something changed this frame; the preview must be re-requested.
    pub changed: bool,
    /// A drag ended or a discrete control was used; push one history entry.
    pub settled: bool,
}

impl Edited {
    /// A slider bound to `value`, recording change and settle separately.
    ///
    /// ⚠ One history entry per *settled* drag, not per changed frame: `r.changed()` and
    /// `r.drag_stopped()` are recorded apart, so dragging Exposure across its range is one
    /// Ctrl+Z rather than two hundred.
    pub fn slider(
        &mut self,
        ui: &mut egui::Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        label: &str,
    ) {
        let r = ui.add(egui::Slider::new(value, range).text(label));
        self.changed |= r.changed();
        self.settled |= r.drag_stopped();
    }

    /// A discrete act — a button, a checkbox, a preset click.
    ///
    /// ⚠ Both flags, unlike a slider: a press is a complete act, so it is one history entry
    /// immediately rather than waiting for a `drag_stopped` that will never come.
    pub fn discrete(&mut self) {
        self.changed = true;
        self.settled = true;
    }
}
```

- [ ] **Step 4: Run the test and watch it pass**

```bash
cargo test -p app --bin Rustroom a_discrete_act_is_both 2>&1 | tail -10
cargo test -p app --bin Rustroom an_untouched_frame 2>&1 | tail -10
```

Expected: both PASS.

- [ ] **Step 5: Sabotage — confirm the test can see the bug it names**

Change `discrete` to set only `changed`:

```rust
    pub fn discrete(&mut self) {
        self.changed = true;
    }
```

```bash
cargo build -p app 2>&1 | grep -c "^error" || true
cargo test -p app --bin Rustroom a_discrete_act_is_both 2>&1 | tail -10
```

Expected: **zero compile errors** (confirm this first — a sabotage that fails to compile looks exactly like one that passes), then FAIL on `a discrete act must push one history entry`. **Restore the line, then `touch crates/app/src/develop/panel/mod.rs`** — restoring a file can leave cargo reusing the sabotaged artifact.

- [ ] **Step 6: Convert the body to use `Edited`**

In `show`, delete these lines:

```rust
                    let mut changed = false;
                    let mut settled = false;
                    let mut slider = |ui: &mut egui::Ui, value: &mut f32, range, label| {
                        let r = ui.add(egui::Slider::new(value, range).text(label));
                        changed |= r.changed();
                        settled |= r.drag_stopped();
                    };
```

and replace with:

```rust
                    let mut ed = Edited::default();
```

Then mechanically rewrite every call site in the file:

- `slider(ui, &mut X, A..=B, "L");` → `ed.slider(ui, &mut X, A..=B, "L");`
- `changed = true; settled = true;` → `ed.discrete();`
- `changed |= r.changed(); settled |= r.drag_stopped();` (the Straighten slider) → `ed.slider(ui, &mut s.edits.geometry.angle, -45.0..=45.0, "Straighten");`
- the trailing `if changed { s.request(..) }` → `if ed.changed { s.request(..) }`
- `if settled { photo_edited |= s.history.push(..) }` → `if ed.settled { .. }`
- the `mask_changed` local and its `if mask_changed { changed = true; settled = true; }` fold → call `ed.discrete()` directly at each site that set it, and delete the local plus its long explanatory comment. **The comment describes a constraint that no longer exists**; leaving it would be a lie.

⚠ Leave the brush-settings copy-into-locals (`brush_size`, `brush_hardness`, `brush_flow`, `brush_erase`, `show_mask_tint`) exactly as they are for now — those are a *different* borrow (`app` vs `app.session`) and Task 5 removes them properly.

- [ ] **Step 7: Verify**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | tail -5
```

Expected: clippy clean, and the **same test count as Task 1 Step 1 plus 2**. Behaviour is unchanged, so no other test may move.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/develop/panel/mod.rs
git commit -m "refactor(app): an Edited accumulator replaces the slider closure"
```

---

### Task 3: `HslChannel` and the pure accessor the selector depends on

**Files:**
- Create: `crates/app/src/develop/panel/adjustments.rs`
- Modify: `crates/app/src/develop/panel/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/develop/panel/adjustments.rs` with only this:

```rust
//! The five edit-group sections of the Develop panel.

use edit_model::HslBand;

/// Which of an [`HslBand`]'s three values the HSL selector is showing.
///
/// ⚠ View state, never part of `Edits` — it must not reach a sidecar, a paste or a preset,
/// for the same reason `Session::selected_mask` does not.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HslChannel {
    #[default]
    Hue,
    Saturation,
    Luminance,
}

impl HslChannel {
    pub const ALL: [HslChannel; 3] = [Self::Hue, Self::Saturation, Self::Luminance];

    pub fn label(self) -> &'static str {
        match self {
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Luminance => "Luminance",
        }
    }

    /// The band field this channel edits.
    pub fn value_mut(self, band: &mut HslBand) -> &mut f32 {
        match self {
            Self::Hue => &mut band.hue,
            Self::Saturation => &mut band.saturation,
            Self::Luminance => &mut band.luminance,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_channel_edits_its_own_field() {
        // ⚠ The load-bearing test of the whole selector. Every value is distinct, and each
        // is written through `value_mut` and then read from the *other two* fields as well —
        // so a swapped match arm cannot pass by coincidence, which asserting one field at a
        // time would allow.
        let mut b = HslBand::default();
        *HslChannel::Hue.value_mut(&mut b) = 11.0;
        *HslChannel::Saturation.value_mut(&mut b) = 22.0;
        *HslChannel::Luminance.value_mut(&mut b) = 33.0;
        assert_eq!(b.hue, 11.0, "Hue wrote the wrong field");
        assert_eq!(b.saturation, 22.0, "Saturation wrote the wrong field");
        assert_eq!(b.luminance, 33.0, "Luminance wrote the wrong field");
    }

    #[test]
    fn every_channel_has_a_distinct_label() {
        // A table over ALL, so a fourth channel cannot ship sharing a label with another —
        // the selector would then have two identical-looking segments.
        let mut seen: Vec<&str> = HslChannel::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(seen.len(), 3);
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 3, "two channels share a label");
    }

    #[test]
    fn all_lists_every_variant() {
        // ⚠ `ALL` is what the selector iterates. A variant missing from it is unreachable in
        // the UI while compiling perfectly — exactly the kind of silent gap that ships.
        for c in HslChannel::ALL {
            // A match with no wildcard: adding a variant breaks this, forcing a decision.
            match c {
                HslChannel::Hue | HslChannel::Saturation | HslChannel::Luminance => {}
            }
        }
        assert_eq!(HslChannel::ALL.len(), 3);
    }
}
```

Add to `crates/app/src/develop/panel/mod.rs`, at the top:

```rust
pub mod adjustments;
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p app --bin Rustroom adjustments:: 2>&1 | tail -15
```

Expected: PASS — the type and its tests land together, because a `pub` item with no caller in a **binary** crate is a `dead_code` error under `-D warnings`, so it cannot be committed one step earlier. (This trap has bitten this codebase three times: `theme::button_row`, the CLI's `format_arg`, `ELLIPSE_SEGMENTS`.)

- [ ] **Step 3: Sabotage — swap two match arms**

```rust
            Self::Hue => &mut band.saturation,
            Self::Saturation => &mut band.hue,
```

```bash
cargo build -p app 2>&1 | grep -c "^error:" || true
cargo test -p app --bin Rustroom each_channel_edits_its_own_field 2>&1 | tail -10
```

Expected: **zero compile errors first**, then FAIL with `Hue wrote the wrong field`. Restore, then `touch crates/app/src/develop/panel/adjustments.rs`.

- [ ] **Step 4: Add the app-level state**

In `crates/app/src/main.rs`, in the `RustroomApp` struct near line 143 (beside `show_mask_tint`):

```rust
    /// Which HSL channel the Color section's selector is showing. View state, per app —
    /// like a brush size, it is a property of the tool and not of the photograph.
    hsl_channel: develop::panel::adjustments::HslChannel,
```

and in the initialiser near line 233:

```rust
            hsl_channel: Default::default(),
```

⚠⚠ **CORRECTED DURING EXECUTION — Task 3 cannot be committed on its own.** The original plan
expected clippy to be clean here and predicted a commit. It is not: `ALL`, `label`, `value_mut`
and the new `hsl_channel` field have **no production caller** until Task 4 builds the sections,
so `-D warnings` rejects all four as dead code:

```
error: variants `Saturation` and `Luminance` are never constructed
error: associated items `ALL`, `label`, and `value_mut` are never used
error: field `hsl_channel` is never read
```

This is the **binary-crate dead-code trap**, whose fourth instance this is after `theme::button_row`,
the CLI's `format_arg` and `ELLIPSE_SEGMENTS`. The plan half-accounted for it — it noted the *type*
must land with its tests — but missed that the *methods and the field* need a production caller too.

**So: do not commit after Task 3. Continue straight into Task 4 and commit them together.** The rule
that generalises: in a bin crate an item lands **with its first caller**, always.

---

### Task 4: The five sections, derived from the edit groups

This is the task that fixes problems 2 and 3 and applies ordering changes 1–4.

**Files:**
- Modify: `crates/app/src/develop/panel/adjustments.rs`
- Modify: `crates/app/src/develop/panel/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `adjustments.rs`:

```rust
    #[test]
    fn the_panel_sections_are_the_edit_groups() {
        // ⚠ The point of the whole restructure: the panel teaches the same five words the
        // Paste, Sync and Save-preset dialogs already use. Reading them out of
        // EDIT_GROUP_NAMES rather than hardcoding strings is what makes that true by
        // construction — rename a group and the header follows, instead of drifting.
        let names: Vec<&str> = SECTIONS.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec!["White Balance", "Tone", "Color", "Detail", "Effects"]
        );
    }

    #[test]
    fn geometry_and_masks_are_the_two_groups_the_panel_does_not_show() {
        // They are rail tools, not adjustment sections. Stated as a property over the full
        // group list so that a *seventh* group added later cannot silently go missing from
        // the panel with nothing failing.
        let shown: Vec<&str> = SECTIONS.iter().map(|s| s.name()).collect();
        let missing: Vec<&str> = edit_model::EDIT_GROUP_NAMES
            .iter()
            .copied()
            .filter(|n| !shown.contains(n))
            .collect();
        assert_eq!(missing, vec!["Geometry", "Masks"]);
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p app --bin Rustroom the_panel_sections_are 2>&1 | tail -15
```

Expected: FAIL — `cannot find value SECTIONS in this scope`.

- [ ] **Step 3: Add `SECTIONS`**

Add to `adjustments.rs`, above the tests:

```rust
/// A panel section, identified by its index into [`edit_model::EDIT_GROUP_NAMES`].
///
/// ⚠ The name is *read from* that table, never written here. The panel and the Paste / Sync
/// / Save-preset dialogs then cannot disagree about what a group is called, and renaming a
/// group updates the header for free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Section(usize);

impl Section {
    pub fn name(self) -> &'static str {
        edit_model::EDIT_GROUP_NAMES[self.0]
    }
}

/// The five adjustment sections, in the order the decisions are made in.
///
/// ⚠ **Deliberately not pipeline order.** The pipeline runs noise reduction and dehaze
/// *before* white balance, because that is where the maths wants them; NR is *judged* last,
/// at 1:1, once the tone is settled. The panel follows the workflow. Do not "fix" the
/// mismatch — Lightroom's panels diverge from its pipeline for the same reason.
///
/// Geometry (index 5) and Masks (index 6) are absent because they are rail tools.
pub const SECTIONS: [Section; 5] = [
    Section(0), // White Balance
    Section(1), // Tone
    Section(2), // Color
    Section(3), // Detail
    Section(4), // Effects
];
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p app --bin Rustroom adjustments:: 2>&1 | tail -12
```

Expected: all PASS.

- [ ] **Step 5: Sabotage — drop a section**

Remove `Section(2)` from the array and change the type to `[Section; 4]`.

```bash
cargo build -p app 2>&1 | grep -c "^error:" || true
cargo test -p app --bin Rustroom geometry_and_masks_are 2>&1 | tail -10
```

Expected: zero compile errors in the **test** target, then FAIL — `missing` gains `"Color"`. Restore and `touch`.

- [ ] **Step 6: Write the section bodies**

Add to `adjustments.rs`:

```rust
use crate::develop::panel::Edited;
use eframe::egui;

/// Draw the five adjustment sections.
///
/// ⚠ Takes `hsl_channel` as its own parameter rather than reaching through the app: the
/// caller holds `&mut app.session` at the same time, and Rust's disjoint field borrows make
/// two separate `&mut` parameters legal where a closure capturing `app` twice is `E0499`.
/// That is the second reason this file exists.
pub fn show(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    hsl_channel: &mut HslChannel,
    ed: &mut Edited,
) {
    // White Balance and Tone open by default; the rest start collapsed, because a panel
    // that opens with 30 sliders showing is the problem this section structure removes.
    section(ui, SECTIONS[0], true, |ui| {
        ed.slider(ui, &mut edits.white_balance.temp, -100.0..=100.0, "Temp");
        ed.slider(ui, &mut edits.white_balance.tint, -100.0..=100.0, "Tint");
    });

    section(ui, SECTIONS[1], true, |ui| {
        ed.slider(ui, &mut edits.exposure, -5.0..=5.0, "Exposure");
        ed.slider(ui, &mut edits.contrast, -100.0..=100.0, "Contrast");
        ed.slider(ui, &mut edits.highlights, -100.0..=100.0, "Highlights");
        ed.slider(ui, &mut edits.shadows, -100.0..=100.0, "Shadows");
        ed.slider(ui, &mut edits.whites, -100.0..=100.0, "Whites");
        ed.slider(ui, &mut edits.blacks, -100.0..=100.0, "Blacks");
        // ⚠ A hairline, not a header. These are Lightroom's "Presence" block and they are
        // local-contrast tools rather than tonal ones — but they belong to the `Tone` edit
        // group, and a header here would put the panel and the Sync dialog out of step.
        ui.separator();
        ed.slider(ui, &mut edits.local_contrast.texture, -100.0..=100.0, "Texture");
        ed.slider(ui, &mut edits.local_contrast.clarity, -100.0..=100.0, "Clarity");
        ed.slider(ui, &mut edits.dehaze.amount, -100.0..=100.0, "Dehaze");
    });

    section(ui, SECTIONS[2], false, |ui| {
        // ⚠ Vibrance first: Adobe's order, and the gentler control. Saturation clips
        // already-saturated colour far more readily, so it is the one offered second.
        ed.slider(ui, &mut edits.vibrance, -100.0..=100.0, "Vibrance");
        ed.slider(ui, &mut edits.saturation, -100.0..=100.0, "Saturation");
        ui.separator();
        // ⚠ A selector, not three nested CollapsingHeaders. Nesting cost a third level *and*
        // put a "Saturation" header under a "Saturation" slider meaning something else.
        ui.horizontal(|ui| {
            for c in HslChannel::ALL {
                ui.selectable_value(hsl_channel, c, c.label());
            }
        });
        for (i, name) in edit_model::HSL_BAND_NAMES.iter().enumerate() {
            ed.slider(
                ui,
                hsl_channel.value_mut(&mut edits.hsl.bands[i]),
                -100.0..=100.0,
                name,
            );
        }
    });

    section(ui, SECTIONS[3], false, |ui| {
        // ⚠ Noise Reduction above Sharpening: you must denoise before you can judge
        // sharpening, and it is the pipeline's own order. A deliberate divergence from
        // Lightroom, which puts Sharpening first.
        ui.label("Noise Reduction");
        let nr = &mut edits.noise_reduction;
        ed.slider(ui, &mut nr.luminance, 0.0..=100.0, "Luminance");
        ed.slider(ui, &mut nr.detail, 0.0..=100.0, "Detail");
        ed.slider(ui, &mut nr.color, 0.0..=100.0, "Color");
        ed.slider(ui, &mut nr.smoothness, 0.0..=100.0, "Smoothness");
        ui.separator();
        ui.label("Sharpening");
        let sh = &mut edits.sharpening;
        ed.slider(ui, &mut sh.amount, 0.0..=100.0, "Amount");
        // The one slider not on a 0..100 scale: a physical quantity in full-resolution
        // pixels, and naming it in pixels is what makes "scaled for the render" legible.
        ed.slider(ui, &mut sh.radius, 0.5..=3.0, "Radius (px)");
        ed.slider(ui, &mut sh.detail, 0.0..=100.0, "Detail");
        ed.slider(ui, &mut sh.masking, 0.0..=100.0, "Masking");
    });

    section(ui, SECTIONS[4], false, |ui| {
        let v = &mut edits.vignette;
        ed.slider(ui, &mut v.amount, -100.0..=100.0, "Amount");
        ed.slider(ui, &mut v.midpoint, 0.0..=100.0, "Midpoint");
        ed.slider(ui, &mut v.roundness, -100.0..=100.0, "Roundness");
        ed.slider(ui, &mut v.feather, 0.0..=100.0, "Feather");
    });
}

/// One collapsible section, headed by its edit group's name.
fn section(ui: &mut egui::Ui, s: Section, open_by_default: bool, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(s.name())
        .default_open(open_by_default)
        .show(ui, body);
}
```

⚠ This file does not use `theme`; do not add the import. The footer (filename and camera model) stays in `mod.rs`.

- [ ] **Step 7: Call it from `mod.rs`**

In `panel/mod.rs`'s `show`, replace the whole run of thirteen `ed.slider(...)` calls **and** the `Hue` / `Saturation` / `Luminance` / `Sharpening` / `Noise Reduction` / `Vignette` `CollapsingHeader` blocks with:

```rust
                    adjustments::show(ui, &mut s.edits, &mut app.hsl_channel, &mut ed);
```

Keep the histogram above it, and keep the Masks section, `Reset all`, the geometry block and the footer below it — those move in Tasks 5 and 6.

⚠ This changes `show`'s borrow shape: it now needs `&mut app.session` and `&mut app.hsl_channel` simultaneously. That is legal (disjoint fields, edition 2021 closure capture), but the existing `if let Some(s) = &mut app.session` binding must stay a *field* borrow — do not introduce a `let app = &mut *app;` alias, which would collapse them into one.

- [ ] **Step 8: Verify**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
cargo test --all 2>&1 | tail -5
```

Expected: clean; test count is the Task 1 baseline **+ 7** (2 from Task 2, 3 from Task 3, 2 here).

- [ ] **Step 9: Commit**

```bash
git add crates/app/src/develop/panel/ crates/app/src/main.rs
git commit -m "feat(app): the panel's sections are the edit groups"
```

⚠ This commit carries **Task 3 and Task 4 together**, for the reason recorded under Task 3.

---

### Task 5: Masks moves to the top, and its sliders mirror the global order

**Files:**
- Create: `crates/app/src/develop/panel/masks.rs`
- Modify: `crates/app/src/develop/panel/mod.rs`

- [ ] **Step 1: Move the Masks section into its own file**

Create `crates/app/src/develop/panel/masks.rs` and move **lines 120–261** of the pre-Task-1 `develop/panel.rs` — from `let mut mask_changed = false;` through the closing `}` of the `if mask_changed { .. }` fold, i.e. the five brush-setting locals, the `egui::CollapsingHeader::new("Masks").show(..)` block at line 129, the five write-backs at line 256, and the fold at line 257. Use `git show HEAD~N:crates/app/src/develop/panel.rs` if the numbers have already shifted. It becomes:

```rust
//! The Masks section: add, select, delete, shape properties, brush settings, adjustments.
//!
//! ⚠ Sits at the **top** of the panel, above the adjustment sections. Not aesthetic: step 2
//! of this rework turns it into a rail tool's drawer, which opens exactly here — so it is
//! already in its final position and that step only has to move its trigger.

use crate::develop::panel::Edited;
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::mask_tool;

/// Brush tool settings, which live on the app rather than on the mask.
///
/// ⚠ They are the *tool's* state and apply to the next stroke, exactly as a brush size does
/// in any editor. Each stroke records its own copy at the moment it is drawn.
pub struct BrushTool<'a> {
    pub size: &'a mut f32,
    pub hardness: &'a mut f32,
    pub flow: &'a mut f32,
    pub erase: &'a mut bool,
    pub show_tint: &'a mut bool,
}

pub fn show(
    ui: &mut egui::Ui,
    s: &mut crate::Session,
    brush: BrushTool<'_>,
    ed: &mut Edited,
) {
    // Verbatim lines 120-261 of the pre-Task-1 `develop/panel.rs`, with the three
    // transformations listed under this code block applied. Do not retype it — move it.
}
```

⚠ **`BrushTool` is a borrow struct, and it replaces the copy-into-locals workaround.** The current code copies five app fields into locals and writes them back after the closure, because the closure captured `app` while `s` also borrowed it. Passing five `&mut` to *disjoint* fields is legal and needs no write-back. Delete the locals and the long comment explaining them — again, that comment would otherwise describe a constraint that no longer exists.

⚠ Every `mask_changed = true` in the moved body becomes `ed.discrete()` directly, and the `mask_changed` local disappears with its comment. `slider(ui, ...)` becomes `ed.slider(ui, ...)`.

- [ ] **Step 2: Apply ordering change 5 — the mask's adjustments mirror the global order**

In the moved body, the per-mask adjustment run currently ends:

```rust
                                ed.slider(ui, &mut a.saturation, -100.0..=100.0, "Saturation");
                                ed.slider(ui, &mut a.vibrance, -100.0..=100.0, "Vibrance");
```

Swap them, so the whole run reads Temp, Tint, Exposure, Contrast, Highlights, Shadows, Whites, Blacks, **Vibrance, Saturation** — the same order the global panel now uses. Add above it:

```rust
                                // ⚠ The same order as the global panel: White Balance, then
                                // Tone, then Color with Vibrance first. One layout learned
                                // once, and it keeps holding as either list changes.
```

- [ ] **Step 3: Call it from the top of the panel**

In `panel/mod.rs`'s `show`, immediately after the histogram and its `ui.separator()`, and **before** `adjustments::show`:

```rust
                    masks::show(
                        ui,
                        s,
                        masks::BrushTool {
                            size: &mut app.brush_size,
                            hardness: &mut app.brush_hardness,
                            flow: &mut app.brush_flow,
                            erase: &mut app.brush_erase,
                            show_tint: &mut app.show_mask_tint,
                        },
                        &mut ed,
                    );
```

Add `pub mod masks;` beside `pub mod adjustments;`.

- [ ] **Step 4: Verify**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
cargo test --all 2>&1 | tail -5
```

Expected: clean, count unchanged from Task 4 (this task adds no tests — it is a move plus a two-line reorder; the mask *model* is already covered by `mask_tool`'s 21 tests, which must all still pass).

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/develop/panel/
git commit -m "feat(app): Masks leads the panel, and its sliders mirror the global order"
```

---

### Task 6: Geometry into its own file, and the footer stays put

**Files:**
- Create: `crates/app/src/develop/panel/geometry.rs`
- Modify: `crates/app/src/develop/panel/mod.rs`

- [ ] **Step 1: Move the geometry block**

Create `crates/app/src/develop/panel/geometry.rs`:

```rust
//! The geometry-mode block: rotate and flip, aspect presets, Straighten.
//!
//! ⚠ Shown only while `geometry_mode` is on. Step 2 of this rework makes that a derived
//! property of the selected rail tool rather than a stored flag, and moves this into a
//! drawer — which is why it is already a separate file.

use crate::develop::panel::Edited;
use eframe::egui;
use rustroom_app::crop_tool;

pub fn show(ui: &mut egui::Ui, s: &mut crate::Session, ed: &mut Edited) {
    // Verbatim lines 271-336 of the pre-Task-1 `develop/panel.rs`, with the two
    // transformations listed under this code block applied. Do not retype it — move it.
}
```

Move **lines 271–336** of the pre-Task-1 `develop/panel.rs` — the body of `if s.geometry_mode {` at line 269, excluding its own `ui.separator()` at line 270 and excluding the closing brace. That is the `ui.label("Geometry")`, the four orientation buttons, the aspect-preset row and the Straighten slider. Convert `settled = true` → `ed.discrete()` for the orientation buttons and the aspect presets, and the Straighten slider's explicit `changed |= ar.changed(); settled |= ar.drag_stopped();` pair to `ed.slider(ui, &mut s.edits.geometry.angle, -45.0..=45.0, "Straighten")`.

⚠ Keep the comment pairing each orientation button with its `CropRect` transform. Rotating a cropped photo without moving the crop leaves it on entirely different pixels, and that comment is the only thing keeping the two in step.

- [ ] **Step 2: Call it**

In `panel/mod.rs`, replace the geometry block with:

```rust
                    if s.geometry_mode {
                        ui.separator();
                        geometry::show(ui, s, &mut ed);
                    }
```

Add `pub mod geometry;`.

- [ ] **Step 3: Verify the panel file is now orchestration only**

```bash
wc -l crates/app/src/develop/panel/*.rs
```

Expected: `mod.rs` well under 150 lines; no file in the module over ~200.

- [ ] **Step 4: Full verification**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20
cargo test --all 2>&1 | tail -5
```

Expected: clean, count unchanged from Task 4.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/develop/panel/
git commit -m "refactor(app): the geometry block gets its own file"
```

---

### Task 7: A headless interaction test for the HSL selector

The brush canvas work produced six bugs, every one in canvas or panel wiring and none caught by the ~30 tests on the pure functions beside it. The selector is exactly that kind of wiring: `HslChannel::value_mut` is proven, and *nothing yet proves the segments are wired to it*.

**Files:**
- Modify: `crates/app/src/develop/panel/adjustments.rs`

- [ ] **Step 1: Write the failing test**

Add to `adjustments.rs`'s `tests` module:

```rust
    use eframe::egui;

    /// Click the segment labelled `want` and report the channel afterwards.
    ///
    /// Three passes: lay out with the pointer in place, press, release — egui resolves a
    /// click against the widgets registered on the *previous* pass, which is why a
    /// single-pass harness reads zeros and passes vacuously. That exact harness bug made
    /// `a_left_drag_does_not_pan` inert once already.
    fn click_segment(want: HslChannel) -> HslChannel {
        let ctx = egui::Context::default();
        let mut channel = HslChannel::Hue;
        let mut target = egui::Pos2::ZERO;
        for pass in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(target));
            if pass == 1 || pass == 2 {
                input.events.push(egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| {
                    for c in HslChannel::ALL {
                        let r = ui.selectable_value(&mut channel, c, c.label());
                        if c == want && pass == 0 {
                            target = r.rect.center();
                        }
                    }
                });
            });
        }
        channel
    }

    #[test]
    fn clicking_a_segment_switches_the_channel() {
        // ⚠ `value_mut` being right proves nothing about the segments being wired to it.
        // Every non-default channel is exercised, because a selector stuck on its default
        // would pass a test that only ever clicked Hue.
        assert_eq!(click_segment(HslChannel::Saturation), HslChannel::Saturation);
        assert_eq!(click_segment(HslChannel::Luminance), HslChannel::Luminance);
        assert_eq!(click_segment(HslChannel::Hue), HslChannel::Hue);
    }
```

- [ ] **Step 2: Run it**

```bash
cargo test -p app --bin Rustroom clicking_a_segment 2>&1 | tail -15
```

Expected: PASS.

- [ ] **Step 3: Sabotage — confirm the harness is not reading zeros**

Replace the harness loop bound `for pass in 0..3` with `for pass in 0..1`.

```bash
cargo build -p app 2>&1 | grep -c "^error:" || true
cargo test -p app --bin Rustroom clicking_a_segment 2>&1 | tail -10
```

Expected: zero compile errors, then FAIL. **If it passes, the test is vacuous and must be fixed before continuing** — that is the eighth recorded instance of a test naming a bug it cannot detect, and it was a harness fault exactly like this one. Restore and `touch`.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/develop/panel/adjustments.rs
git commit -m "test(app): the HSL selector's segments are wired to the channel"
```

---

### Task 8: Look at it, then record the result

- [ ] **Step 1: Full green**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all 2>&1 | tail -5
```

Expected: all green. **No golden may move** — this plan touches no pipeline code, so a moved golden means something is badly wrong.

- [ ] **Step 2: Run the GUI and actually look**

```bash
cargo run --release -p app
```

Check, in this order — these are the things a test cannot see:

1. The panel opens with **White Balance and Tone expanded**, everything else collapsed.
2. **Masks is at the top**, above White Balance.
3. **Color** holds Vibrance above Saturation, then a `Hue │ Saturation │ Luminance` selector over eight band sliders. Switching the segment changes which eight values you are editing — verify by setting Red's Hue, switching to Saturation, and confirming Red reads 0 again.
4. **Detail** shows Noise Reduction above Sharpening.
5. **Tone** has a hairline before Texture.
6. Dragging any slider still gives **one** Ctrl+Z, not two hundred.
7. Selecting a mask still shows its adjustments, now ending Vibrance then Saturation.

- [ ] **Step 3: Update CLAUDE.md**

Add a section recording: the five sections and why they are the edit groups; the panel-order-is-not-pipeline-order rule; the five ordering changes with change 3 flagged as a deliberate Lightroom divergence; `Edited` replacing the closure and the two `E0499` workarounds it dissolved; and that this is **step 1 of 3**, with the rail and menu bar still to come.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the panel restructure"
```

---

## Self-review notes

Checked against the spec's **panel** sections:

- Five sections = edit groups → Task 4. ✅
- Ordering changes 1 (HSL selector) → Task 4; 2 (Vibrance first) → Task 4; 3 (NR first) → Task 4; 4 (Presence hairline) → Task 4; 5 (mask mirror) → Task 5. ✅
- `develop/panel/` split → Tasks 1, 5, 6. ✅
- Masks position → Task 5. ✅
- Sabotage-with-compile-check on every new test → Tasks 2, 3, 4, 7. ✅
- `app.hsl_channel` as view state that never reaches `Edits` → Task 3, asserted in the doc comment and structurally true because it is a field of `RustroomApp`, not of `Edits`.

**Deliberately out of scope here, and named in the spec as later steps:** the rail, the drawer behaviour, `geometry_mode` becoming derived, the tool surviving a photo switch, `Reset all` moving to the Edit menu, the menu bar, the toolbar slimming, and removing `show_presets`.
