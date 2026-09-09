# Boolean mask operations — the panel and the canvas (M7.1c, plan 2 of 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ⚠ **Conventions that are not negotiable:** commit messages carry **no `Co-Authored-By` trailer**; work stays on branch `m7.1c-boolean-masks`; **never push**.

**Goal:** Make composites authorable — mode chips, a component list, per-component handles on the canvas, and a tint that shows the composite.

**Architecture:** A segmented `New mask | Add to | Subtract from | Intersect with` row above the five kind buttons decides whether a click creates a mask or appends a `Refinement`. `Session.selected_component` picks which component the shape properties and the canvas handles act on. `mask_tool`'s signatures are unchanged — the canvas simply hands it a different `Mask`.

**Prerequisite:** `2026-09-09-boolean-masks-headless.md` must be complete. Spec: `docs/superpowers/specs/2026-09-09-boolean-mask-ops-design.md`.

**⚠⚠ Read this before starting.** This project's record says GUI wiring is where its defects live — *"every one found by using the app and none by a test"*, recorded fourteen times. The headless half is proved by goldens; **this half is proved by the checklist in Task 7**, and the tests below cover wiring, not appearance.

---

## Environment

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ The GUI binary is **`Rustroom`**: `cargo test -p app --bin Rustroom <filter>`. ⚠ `render_engine`, `mask_tool` and `session_flow` are **lib** modules — running them with `--bin` filters them away and prints `0 passed … ok`. **Read the passed count on every filtered run.**

## File structure

| file | responsibility |
|---|---|
| modify `crates/app/src/main.rs` | `Session.selected_component`, `RustroomApp.mask_add_mode` |
| modify `crates/app/src/session_flow.rs` | the two exhaustive `Session` literals |
| **create** `crates/app/src/develop/panel/mask_components.rs` | mode chips, component rows, their tests |
| modify `crates/app/src/develop/panel/masks.rs` | `MaskUi` params struct, the call, per-component shape properties |
| modify `crates/app/src/develop/canvas/mask_overlay.rs` | selected-component routing, dimmed outlines |
| modify `crates/app/src/develop/canvas/mask_tint.rs` | composite tint |

⚠ `masks.rs` (647) and `mask_overlay.rs` (675) are past the ~600 marker and grow here; neither is split, on the user's call. New logic goes in `mask_components.rs`.

---

### Task 1: `selected_component` and its reset rules

**Files:**
- Modify: `crates/app/src/main.rs:716-727`, `crates/app/src/session_flow.rs:489-496`
- Modify: `crates/app/src/mask_tool.rs` (a pure clamp helper + tests)

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/mask_tool.rs`'s test module, beside `selection_after_delete`:

```rust
    #[test]
    fn deleting_a_component_clamps_the_selection() {
        // Three components (base + two refinements); delete the middle one.
        assert_eq!(component_after_delete(2, 1, 3), 1, "the selection shifts down onto the survivor");
        assert_eq!(component_after_delete(1, 2, 3), 1, "a deletion after it leaves it alone");
        assert_eq!(component_after_delete(2, 2, 3), 1, "deleting the selected one steps back");
    }

    #[test]
    fn the_base_is_never_deleted_and_never_stops_being_selectable() {
        // ⚠ Component 0 is the base — deleting it is deleting the mask, which is the × on the
        // mask row, not on a component row. So the clamp can never return anything but a valid
        // index, and 0 is always valid.
        assert_eq!(component_after_delete(0, 1, 2), 0);
    }
```

- [ ] **Step 2: Implement the clamp**

Add to `crates/app/src/mask_tool.rs`:

```rust
/// Which component is selected after the one at `removed` is deleted, given `before` components.
///
/// ⚠ Steps **back** rather than forward when the selected component is the one removed, and never
/// clears: component 0 is the base and always exists, so unlike `selection_after_delete` there is
/// no "nothing selected" state to fall into. Silently selecting a *later* component would leave
/// the next drag editing a shape nobody chose — `selection_after_delete`'s reason.
pub fn component_after_delete(selected: usize, removed: usize, before: usize) -> usize {
    let after = before.saturating_sub(1);
    let next = if removed < selected || (removed == selected && selected > 0) {
        selected - 1
    } else {
        selected
    };
    next.min(after.saturating_sub(1))
}
```

- [ ] **Step 3: Add the session field**

In `crates/app/src/main.rs`, immediately after `selected_mask`:

```rust
    /// Which component of the selected mask the shape properties and the canvas handles act on.
    /// `0` is the base; `j > 0` is the `j-1`th refinement.
    ///
    /// ⚠ **View state, never `Edits`** — `selected_mask`'s rule, for its reason: a selection is
    /// not part of the photograph's description, so it must not reach the sidecar or travel with
    /// a paste.
    ///
    /// ⚠ Reset to 0 whenever `selected_mask` changes, and clamped when the component list
    /// shrinks. A stale index would put the panel's sliders on one shape and the canvas's handles
    /// on another.
    selected_component: usize,
```

Add `selected_component: 0` to **both** exhaustive `Session { .. }` literals in `session_flow.rs` — the compiler will name them, which is what those literals are for.

- [ ] **Step 4: Run**

```bash
cargo test -p app mask_tool 2>&1 | grep "test result"
cargo build --workspace --all-targets 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/app/src/
git commit -m "A mask has a selected component, and deleting one steps back"
```

---

### Task 2: The mode chips, and appending a component

**Files:**
- Create: `crates/app/src/develop/panel/mask_components.rs`
- Modify: `crates/app/src/develop/panel/masks.rs`, `crates/app/src/develop/panel/mod.rs`, `crates/app/src/main.rs`

⚠ **This task and Task 3 land in ONE commit.** `mask_components` is a module of a **binary** crate, so a `pub(super) fn` with no caller is dead code under `-D warnings`. This trap has been recorded at least ten times in this project; an item lands **with** its first caller.

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/develop/panel/mask_components.rs` with the chips, the rows and:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    #[test]
    fn a_kind_button_adds_a_mask_in_new_mode_and_a_component_otherwise() {
        // ⚠ Both halves, and the pair is what makes the test mean anything. A version that always
        // appended a component would pass a "does Subtract append?" test on its own.
        let mut s = session_with_one_mask();
        let mut mode = None;
        click_kind(&mut s, &mut mode, "Add radial filter");
        assert_eq!(s.edits.masks.len(), 2, "New mask mode adds a mask");
        assert!(s.edits.masks[1].refine.is_empty());

        let mut s = session_with_one_mask();
        let mut mode = Some(edit_model::MaskOp::Subtract);
        click_kind(&mut s, &mut mode, "Add radial filter");
        assert_eq!(s.edits.masks.len(), 1, "a refinement adds no mask");
        assert_eq!(s.edits.masks[0].refine.len(), 1);
        assert_eq!(s.edits.masks[0].refine[0].op, edit_model::MaskOp::Subtract);
    }

    #[test]
    fn appending_a_component_selects_it() {
        let mut s = session_with_one_mask();
        let mut mode = Some(edit_model::MaskOp::Add);
        click_kind(&mut s, &mut mode, "Add radial filter");
        assert_eq!(s.selected_component, 1, "the new component becomes the one you edit");
    }

    #[test]
    fn the_chips_are_disabled_with_no_mask_selected() {
        // ⚠ Disabled, never a silent fallback to New. An invisible mode whose meaning depends on
        // selection state is the class of thing this project's record says bites.
        let mut s = session_with_one_mask();
        s.selected_mask = None;
        let labels = enabled_labels(&mut s, &mut None);
        assert!(!labels.iter().any(|l| l == "Subtract from"));
        assert!(labels.iter().any(|l| l == "New mask"));
    }

    #[test]
    fn every_chip_is_drawn_and_named_from_the_op() {
        // ⚠ Built from `MaskOp::ALL`, so a fourth op cannot ship with no chip.
        let mut s = session_with_one_mask();
        let labels = enabled_labels(&mut s, &mut None);
        for op in edit_model::MaskOp::ALL {
            assert!(labels.iter().any(|l| l == op.chip_label()), "missing {}", op.chip_label());
        }
    }
}
```

⚠ `session_with_one_mask`, `click_kind` and `enabled_labels` are local helpers built on `test_ui` — **read `crates/app/src/develop/panel/masks.rs`'s existing test module first** and reuse its `session()` and `click_in_panel` shapes rather than inventing a second harness. Its own comment records that a drag, not a click, is what `test_ui` needs for some widgets, and that a rebuilt fixture must go through the same production code path.

- [ ] **Step 2: Implement the chips**

```rust
//! A mask's components: the mode chips that decide whether a kind button creates a mask or
//! refines one, and the indented list of what a composite is built from.
//!
//! ⚠ Its own module because `develop/panel/masks.rs` is 647 lines and past the ~600 marker.

use crate::develop::panel::Edited;
use crate::{theme, Session};
use eframe::egui;
use rustroom_app::mask_tool;

/// Draw the mode chips. Returns nothing — the mode is `*mode`.
///
/// ⚠ The chips are `selectable_label`s, not buttons: a mode with no shown selection is invisible
/// state, which is what the crop aspect lock was fixed for.
pub(super) fn chips(ui: &mut egui::Ui, s: &Session, mode: &mut Option<edit_model::MaskOp>) {
    ui.horizontal_wrapped(|ui| {
        if ui.selectable_label(mode.is_none(), "New mask").clicked() {
            *mode = None;
        }
        // ⚠ Refining needs something to refine. Disabled with a hover rather than falling back to
        // New: a control that silently does something else is worse than one that says it cannot.
        ui.add_enabled_ui(s.selected_mask.is_some(), |ui| {
            for op in edit_model::MaskOp::ALL {
                if ui
                    .selectable_label(*mode == Some(op), op.chip_label())
                    .on_disabled_hover_text("Select a mask first")
                    .clicked()
                {
                    *mode = Some(op);
                }
            }
        });
    });
    theme::hint(
        ui,
        "New mask starts a fresh mask. The other three add a shape to the selected \
         mask instead: Add widens it, Subtract cuts out of it, Intersect keeps only \
         where both reach.",
    );
}
```

- [ ] **Step 3: Route the kind buttons through the mode**

In `masks.rs`, the block that currently does:

```rust
    if let Some(mask) = added {
        s.edits.masks.push(edit_model::MaskedAdjustment { mask, adjustments: ..., });
        s.selected_mask = Some(s.edits.masks.len() - 1);
        ed.discrete();
    }
```

becomes:

```rust
    if let Some(mask) = added {
        match (*ui_state.add_mode, s.selected_mask) {
            // ⚠ A refinement only where there is something to refine. The chips are disabled
            // without a selection, so this arm is the belt to that braces — and it fails toward
            // "add a mask", which is recoverable, rather than toward dropping the click.
            (Some(op), Some(i)) if s.edits.masks.get(i).is_some() => {
                s.edits.masks[i].refine.push(edit_model::Refinement { op, mask });
                s.selected_component = s.edits.masks[i].refine.len();
            }
            _ => {
                s.edits.masks.push(edit_model::MaskedAdjustment {
                    mask,
                    ..Default::default()
                });
                s.selected_mask = Some(s.edits.masks.len() - 1);
                s.selected_component = 0;
            }
        }
        // ⚠ Clear any in-flight handle: it was hit-tested against a different shape.
        s.active_mask_handle = None;
        ed.discrete();
    }
```

- [ ] **Step 4: Fold the panel's parameters into `MaskUi`**

`masks::show` already takes six parameters and this adds a seventh, which is where clippy's `too_many_arguments` fires — a fair complaint this project answers with a params struct (`FrameSpec`, `NameField`, `AspectLock`, `GradeUi` are the precedents). In `masks.rs`:

```rust
/// The panel state a mask's controls need that lives on `RustroomApp` rather than on the mask.
///
/// ⚠ Three `&mut` to **disjoint** fields, which is what makes this legal where a closure capturing
/// `app` would be `E0499` — the same reasoning `BrushTool` already carries.
pub struct MaskUi<'a> {
    pub hsl: &'a mut super::adjustments::HslChannel,
    pub grade: &'a mut super::color_wheel::GradeRange,
    /// `None` = New mask; `Some(op)` = the next kind button refines the selected mask.
    pub add_mode: &'a mut Option<edit_model::MaskOp>,
}
```

`show(ui, s, ui_state: MaskUi<'_>, brush, ed)` — five parameters, one fewer than today. Update the call in `develop/panel/mod.rs` and add `mask_add_mode: Option<edit_model::MaskOp>` to `RustroomApp` (default `None`).

- [ ] **Step 5: Run and commit — with Task 3**

Do not commit yet; `mask_components::rows` (Task 3) is still uncalled. Proceed to Task 3 and commit both.

---

### Task 3: The component list

**Files:**
- Modify: `crates/app/src/develop/panel/mask_components.rs`, `crates/app/src/develop/panel/masks.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn the_components_are_listed_under_their_mask() {
        let mut s = session_with_composite();
        let labels = component_labels(&mut s);
        assert_eq!(labels, vec!["Gradient", "Subtract Radial", "Intersect Luminance"]);
    }

    #[test]
    fn only_the_selected_masks_components_are_listed() {
        // ⚠ A second mask's components must not appear — the list belongs to one mask.
        let mut s = session_with_composite();
        s.edits.masks.push(edit_model::MaskedAdjustment::default());
        let labels = component_labels(&mut s);
        assert_eq!(labels.len(), 3);
    }

    #[test]
    fn clicking_a_component_selects_it_and_drops_the_drag_handle() {
        // ⚠⚠ The handle must go. It was hit-tested against a different shape, and a stale one
        // would keep dragging the component you just left — the failure `selection_after_delete`
        // already guards for a deleted mask.
        let mut s = session_with_composite();
        s.active_mask_handle = Some(mask_tool::Handle::Centre);
        click_component(&mut s, "Intersect Luminance");
        assert_eq!(s.selected_component, 2);
        assert!(s.active_mask_handle.is_none());
    }

    #[test]
    fn deleting_a_component_removes_it_and_clamps_the_selection() {
        let mut s = session_with_composite();
        s.selected_component = 2;
        delete_component(&mut s, 1);
        assert_eq!(s.edits.masks[0].refine.len(), 1);
        assert_eq!(s.selected_component, 1);
    }
```

⚠ Adapt `Handle::Centre` to the real variant name in `mask_tool` — read the enum first.

- [ ] **Step 2: Implement the rows**

```rust
/// Draw the selected mask's component list. Returns `true` if the edits changed.
///
/// ⚠ Indented under the mask row, and **only for the selected mask**: a composite is one mask, so
/// its parts belong inside it rather than beside it in the flat list.
pub(super) fn rows(ui: &mut egui::Ui, s: &mut Session, i: usize, ed: &mut Edited) {
    let Some(m) = s.edits.masks.get(i) else { return };
    let names: Vec<String> = std::iter::once(m.mask.label().to_string())
        .chain(m.refine.iter().map(|r| r.label()))
        .collect();
    let mut remove: Option<usize> = None;
    let mut pick: Option<usize> = None;
    ui.indent("mask_components", |ui| {
        for (j, name) in names.iter().enumerate() {
            ui.horizontal(|ui| {
                // ⚠ The base has no ×: deleting it is deleting the mask, which is the mask row's
                // own button. Two ways to delete one thing is how the two come to disagree.
                if j == 0 {
                    ui.add_space(theme::ROW_BUTTON_WIDTH);
                } else if ui.button("×").on_hover_text("Remove this shape").clicked() {
                    remove = Some(j);
                }
                if ui
                    .selectable_label(s.selected_component == j, name)
                    .clicked()
                {
                    pick = Some(j);
                }
            });
        }
    });
    if let Some(j) = pick {
        s.selected_component = j;
        // ⚠ The handle was hit-tested against the component being left.
        s.active_mask_handle = None;
    }
    if let Some(j) = remove {
        let before = names.len();
        s.edits.masks[i].refine.remove(j - 1);
        s.selected_component = mask_tool::component_after_delete(s.selected_component, j, before);
        s.active_mask_handle = None;
        ed.discrete();
    }
}
```

⚠ `theme::ROW_BUTTON_WIDTH` may not exist — if not, use `ui.add_space(18.0)` with a comment saying it aligns the base row with the rows that carry a ×, or reuse whatever the Presets panel does for the same alignment.

- [ ] **Step 3: Call both from `masks.rs`**

`chips(..)` goes **above** the existing `ui.horizontal_wrapped` of kind buttons. `rows(..)` goes inside the `if let Some(i) = s.selected_mask` block, immediately after the `ui.separator()` and **before** the shape properties.

- [ ] **Step 4: Point the shape properties at the selected component**

The block that currently does `if let Some(m) = s.edits.masks.get_mut(i)` and then matches `m.mask` must resolve the **selected component's** mask:

```rust
            // ⚠ The shape properties edit the SELECTED COMPONENT, not always the base. One
            // resolution site, so the panel and the canvas cannot disagree about which shape the
            // sliders move.
            let j = s.selected_component;
            if let Some(mask) = component_mut(&mut s.edits.masks[i], j) {
                if ui.checkbox(mask.invert_mut(), "Invert").changed() { ed.discrete(); }
                if let edit_model::Mask::Radial(r) = mask { … }
                // … the existing Luminance / Colour / Brush blocks, unchanged apart from `mask`
            }
```

with, in `mask_components.rs`:

```rust
/// The mask at component `j` — `0` is the base, `j > 0` the `j-1`th refinement.
///
/// ⚠ One resolver, used by the panel **and** the canvas. Two derivations of "which shape is
/// selected" is exactly how the sliders and the handles would come to move different things.
pub(super) fn component_mut(
    m: &mut edit_model::MaskedAdjustment,
    j: usize,
) -> Option<&mut edit_model::Mask> {
    match j.checked_sub(1) {
        None => Some(&mut m.mask),
        Some(k) => m.refine.get_mut(k).map(|r| &mut r.mask),
    }
}

/// The shared-borrow twin. ⚠ Both, because the canvas reads and the panel writes.
pub(super) fn component(m: &edit_model::MaskedAdjustment, j: usize) -> Option<&edit_model::Mask> {
    match j.checked_sub(1) {
        None => Some(&m.mask),
        Some(k) => m.refine.get(k).map(|r| &r.mask),
    }
}
```

- [ ] **Step 5: Reset `selected_component` when the mask selection changes**

Every site that writes `s.selected_mask` must also write `s.selected_component = 0`:
the mask-row `selectable_label`, `remove`, `Remove all masks`, and `snapshots.rs:67`.

⚠ Find them with `grep -rn "selected_mask = " crates/app/src` and fix **every** one. A stale component index puts the sliders on a shape the mask does not have.

- [ ] **Step 6: Run**

```bash
cargo test -p app --bin Rustroom mask_components 2>&1 | grep "test result"
cargo test -p app --bin Rustroom panel 2>&1 | grep "test result"
```

Expected: the new tests pass and **every pre-existing panel test passes unchanged** — that is the evidence the routing moved no behaviour for an un-refined mask.

- [ ] **Step 7: Sabotage — always resolve the base**

Change `component_mut` to `Some(&mut m.mask)` unconditionally. Run the panel tests.

Expected: the component tests fail. Restore from a copy.

- [ ] **Step 8: Commit (Tasks 2 and 3 together)**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/app/src/
git commit -m "Mode chips and a component list, and the sliders follow the selection"
```

---

### Task 4: The canvas — the selected component owns the handles

**Files:**
- Modify: `crates/app/src/develop/canvas/mask_overlay.rs`

- [ ] **Step 1: Write the failing test**

Add to `mask_overlay.rs`'s test module, following the existing five-pass drag harness there — ⚠ **read it first**: this project has recorded a canvas drag test that passed vacuously because the harness moved the pointer on a different pass from the one it read.

```rust
    #[test]
    fn a_drag_moves_the_selected_component_not_the_base() {
        // ⚠⚠ The carrier test. Routing every drag to the base renders plausibly and is wrong,
        // and no other test in the suite can see it.
        let mut app = test_app::with_composite();
        app.session.selected_component = 1;
        let before_base = app.session.edits.masks[0].mask.clone();
        drag_on_canvas(&mut app, from_centre_of_the_refinement(), [40.0, 0.0]);
        assert_eq!(app.session.edits.masks[0].mask, before_base, "the base must not move");
        assert_ne!(
            component(&app.session.edits.masks[0], 1).unwrap(),
            &original_refinement(),
            "the selected component must move"
        );
    }
```

- [ ] **Step 2: Route through the selected component**

In `draw_mask_overlay`, replace

```rust
    let mask = m.mask.clone();
```

with

```rust
    // ⚠⚠ **The selected component owns the gesture.** `mask_tool::hit_test` and `apply_drag` are
    // unchanged — the canvas simply hands them a different `Mask`, which is what keeps 3.1b's flat
    // `Handle` decision intact rather than re-litigating it for a tree.
    let j = s.selected_component;
    let Some(mask) = mask_components::component(m, j).cloned() else {
        return false;
    };
```

and every `s.edits.masks.get_mut(i).map(|m| &mut m.mask)` in the brush and analytic arms becomes

```rust
        s.edits.masks.get_mut(i).and_then(|m| mask_components::component_mut(m, j))
```

⚠ There are **four** such sites — two in the brush arm (`drag_started`, motion), one in the analytic drag, and the `RUSTROOM_DEBUG_BRUSH` read. Find them with `grep -n "&mut m.mask\|&m.mask" crates/app/src/develop/canvas/mask_overlay.rs` and fix each; a missed one silently edits the base.

- [ ] **Step 3: Draw the other components dimmed**

`paint_mask` gains a `stroke: egui::Stroke` and a `handles: bool` parameter (⚠ a params struct if clippy objects — it already takes seven). In `draw_mask_overlay`, before painting the selected component:

```rust
    // ⚠ The others draw as dimmed outlines with no handles, so you can see what you are cutting
    // against without a second set of grab points on top of the first — which is the thing the
    // "all components draggable" alternative could not avoid.
    let dim = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90));
    for (k, other) in std::iter::once(&m.mask)
        .chain(m.refine.iter().map(|r| &r.mask))
        .enumerate()
    {
        if k != j {
            paint_mask(&ui.painter_at(img_rect), other, frame_rect, img_rect,
                       &s.edits.geometry, false, None, dim, false);
        }
    }
```

⚠ `show_tint: false` for the dimmed pass — the tint is the composite's (Task 5) and must be drawn once, not once per component.

- [ ] **Step 4: Run**

```bash
cargo test -p app --bin Rustroom mask_overlay 2>&1 | grep "test result"
```

- [ ] **Step 5: Sabotage — route the drag to the base**

Force `let j = 0;`. Run the carrier test.

Expected: **FAIL**. Restore from a copy.

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/app/src/develop/canvas/
git commit -m "The selected component owns the handles; the rest draw dimmed"
```

---

### Task 5: The tint shows the composite

**Files:**
- Modify: `crates/app/src/develop/canvas/mask_tint.rs`, `crates/app/src/develop/panel/masks.rs`

⚠⚠ **Read this before writing code — it reverses a shipped decision, and the reason it may be reversed is a milestone that landed the same day.** `paint_mask`'s own comment refuses to tint a `Luminance` or `Colour` mask, and its argument is exact: *"the only pixels the canvas has are the rendered texture — which already has this mask's adjustment applied"*, so a tint from them would show the mask selecting the tones it has just moved.

**Mask isolation removed that premise.** While the tint is shown, `suppressed_mask` holds this mask's adjustments **out of the view** — the tint and the suppression are derived from the same three facts in `develop::body` — so the displayed frame *is* this mask's base. What remains is a genuine approximation with two named terms, and both go in the code and in the caveats:

1. the frame is **8-bit sRGB after the display rendering and the creative LUT**, where `MaskStage` reads linear working-space pixels;
2. **later masks' adjustments are still in the view**, where this mask's true base is masks `0..i-1` only.

⚠ If that proves misleading in use, the honest fix is the one `paint_mask` already names: coverage computed against the pre-mask buffer on the render worker — *"a real feature, and a different one."* **Do not paper over it with a tuning constant.**

- [ ] **Step 1: Move "Show mask" out of the brush block**

In `masks.rs`, the `ui.checkbox(&mut *brush.show_tint, "Show mask")` currently sits inside `if matches!(m.mask, Mask::Brush(_))`. Move it above the per-kind blocks so it applies to a composite of any kinds, and rename its hover to say what it shows.

⚠ Check `develop::body`'s `suppressed_mask` derivation still agrees — it is derived from the same facts that decide whether the tint is drawn, and moving the checkbox must not desynchronise the two. `grep -n "suppressed_mask" crates/app/src/develop/mod.rs`.

- [ ] **Step 2: Write the failing test**

```rust
    #[test]
    fn the_tint_covers_every_component_not_just_the_base() {
        // A base gradient with a subtracted radial: the tint inside the radial must be weaker
        // than the same point with the radial removed.
        let with = tint_alpha_at(&composite_with_subtracted_radial(), [0.5, 0.5]);
        let without = tint_alpha_at(&base_gradient_only(), [0.5, 0.5]);
        assert!(with < without, "the subtracted radial must cut the tint: {with} vs {without}");
    }
```

⚠ `tint_alpha_at` needs the coverage plane the tint builds, not the uploaded texture. Extract the plane build into a `pub(super) fn plane(...) -> Vec<u8>` so the test can read it — a texture upload is not testable headlessly and this project has already recorded a canvas test that counted meshes and found 0 either way.

- [ ] **Step 3: Build the composite plane**

Replace the tint's single-brush rasterisation with a per-component fold at the capped resolution, reusing `MaskOp::combine` so the tint and the pipeline share one algebra:

- position-dependent components (`Linear`, `Radial`) answer from `analytic_coverage` at the tint's own `(u, v)` and the **frame's** aspect;
- `Brush` components rasterise as today, with the existing `TintBase` gate for an auto-masked one;
- `Luminance` and `Colour` components read the displayed frame, converting sRGB → linear → the same `lightness` / `hue_chroma` the pipeline uses. ⚠ **Reuse `pipeline`'s functions**, exported if need be — a second hue convention here would disagree with the mask everywhere.

Extend `TintCache`'s key from `brush` to the whole `MaskedAdjustment`'s shape (base + refinements) so the memo still hits while merely hovering. ⚠ The memo is load-bearing: `paint_mask` runs on every egui frame, and without it thirty strokes measured ~36 ms per frame with nothing happening.

- [ ] **Step 4: Measure the miss**

```bash
cargo run --release -p app --example bench_masked ~/Desktop/fuji/DSCF6223.RAF
```

⚠ Add a row timing one composite-tint plane build at `TINT_MAX_DIM`, with and without a `Colour` component. A colour component is an Oklab round trip over up to ~1.3 Mpx; if that lands above ~20 ms on a memo miss, **record it as a caveat rather than shipping it silently** — the tint's own history is a feature that was responsive with the mask hidden and sluggish with it shown.

- [ ] **Step 5: Correct `paint_mask`'s comment in place**

The `Luminance | Colour => {}` arm's long comment is now half wrong. ⚠ **Correct it in place rather than deleting it** — this project's habit, and the reasoning is still right about *why* it was true and about what an honest fix would need. Say what changed: mask isolation, the two residual error terms, and that a lone range mask still draws nothing on the canvas because it has no shape to annotate.

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/app/src/develop/
git commit -m "The tint shows the composite, which mask isolation is what makes honest"
```

---

### Task 6: The full gate

- [ ] **Step 1: Run every gate, each exit code from its own file**

```bash
cargo fmt --check > /tmp/f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
grep -c "^test golden_.* ok$" /tmp/t.log
grep "^test result" /tmp/t.log | awk '{s+=$4} END {print "passed:", s}'
```

Expected: all zero, **63 goldens ran**, and none moved (`git status --short crates/pipeline/tests/golden/` is empty). This plan touches no pixel path, so a moved golden means something leaked out of the app layer.

- [ ] **Step 2: Commit anything outstanding**

---

### Task 7: ⚠⚠ The GUI checklist — this is where the defects are

**This project has recorded fourteen times that GUI wiring defects are found by use and not by a test.** Launch the app and work through every item; report what breaks rather than what passes.

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app &
```

- [ ] **1. The named gesture, on a face.** Add a radial over a face, pick `Subtract from`, add a brush, paint out the eyes. The adjustment must reach the face and not the eyes.
- [ ] **2. The other named gesture.** Add a graduated filter over a sky, pick `Intersect with`, add a luminance range, pull it to the brighter half. The gradient must stop at the ridge.
- [ ] **3. Union.** Two overlapping radials on one mask with `Add to`, carrying +1 EV. ⚠ The overlap must be lifted **once**, not twice — which is the whole reason union is not "two separate masks".
- [ ] **4. The component list.** Click each row; the sliders below must follow, and the canvas handles must move to that shape while the others go dim.
- [ ] **5. Delete a middle component.** The list closes up, the selection lands on a survivor, and the picture updates.
- [ ] **6. ⚠ Masked noise reduction or dehaze on a composite.** Set masked NR, then drag a subtracted radial. **The denoised region must follow the shape.** This is `capture_projection` — if the shape moves and the denoised area does not, the prefix key is stale.
- [ ] **7. ⚠ Pan and zoom to 1:1 with a composite containing a brush.** Coverage is cached per `(mask, component)`; a wrong index shows as one component rendering positionally while its tint shows it gated.
- [ ] **8. ⚠ An unpainted brush base with an added radial.** The radial must render. This is §4.3's hazard, and its failure mode is the mask simply not appearing.
- [ ] **9. Copy the photo's edits and paste onto another.** The composite must travel whole — it rides `MASKS_GROUP` and needs no code, so this checks nothing broke it.
- [ ] **10. Quit and reopen.** The composite must come back from the sidecar; a mask with no refinements must not have gained a `"refine": []` key (`cat <photo>.RAF.json`).
- [ ] **11. The tint.** Tick "Show mask" on a composite and confirm the red tint traces the **composite**, not the base alone.

- [ ] **Step 2: Fix what the checklist finds, then update `CLAUDE.md`**

Add the milestone's section, and ⚠ **move the RESUME block's NEXT ACTION to M7.3b** — Phase 7's last remaining item — recording that M7.1c is closed pending confirmation by use.

---

## Done when

- `fmt` / `clippy --all-targets --all-features -D warnings` / `test --workspace` green **together**, 63 goldens **run** and none moved.
- Every checklist item above has been exercised on a real photograph and what broke has been fixed.
- ⚠ **Not before the user reports it working.** This project's record is that a milestone is closed when it is *merged, green and seen* — the third of those is the one only they can supply.
