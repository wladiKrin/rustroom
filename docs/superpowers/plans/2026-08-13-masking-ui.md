# Masking Canvas UI Implementation Plan (M3.1a, plan B of 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Author a graduated-filter mask by dragging it on the canvas and adjusting its sliders, instead of hand-writing JSON.

**Architecture:** A pure `app::mask_tool` mirroring `crop_tool` holds the hit-testing and drag maths; `develop/canvas.rs` gains an overlay that reuses the crop overlay's drag pattern; `develop/panel.rs` gains a Masks section. Selection is view state on `Session`, never in `Edits`.

**Tech Stack:** Rust, egui 0.35. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-13-masking-ui-design.md`

**Build note:** every `cargo` command needs libraw and lcms2 on the pkg-config path. Export once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** (explicit user preference). Never `git push` — the user pushes themselves.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/app/src/mask_tool.rs` | **New, lib module.** Pure hit-testing and drag maths on `LinearGradient`. Mirrors `crop_tool`. | 1 |
| `crates/app/src/lib.rs` | `pub mod mask_tool;` | 1 |
| `crates/app/src/main.rs` | `Session` gains `selected_mask` and `active_mask_handle`. | 2 |
| `crates/app/src/develop/canvas.rs` | `draw_mask_overlay`, and the refined drag precedence. | 2 |
| `crates/app/src/develop/panel.rs` | The Masks section: add, list, delete, and the selected mask's sliders. | 3 |
| `crates/app/src/mask_tool.rs` | Also gains `selection_after_delete` — the rule for where the selection lands when a mask is removed. | 3 |

### ⚠ `mask_tool` belongs in the **lib**, unlike `theme` and `modals`

`crates/app/src/lib.rs` is deliberately egui-free, and `mask_tool` touches only `edit_model`
types and `f32` — exactly like `crop_tool`, which is already a lib module. So it goes in the
lib, it is properly unit-testable, and **the binary-crate dead-code trap does not apply**.

### ⚠ A correction to the spec, found by reading the canvas

The spec says "geometry_mode wins; otherwise a selected mask owns the drag". Reading
`develop/canvas.rs` shows the non-geometry branch currently owns **pan and zoom** — so that
rule as written would make the image un-pannable whenever a mask is selected.

The real rule is one level finer, and is how these tools normally behave:

```rust
if s.geometry_mode {
    // crop overlay
} else {
    // A mask drag is claimed only if it STARTED on one of its handles.
    // Anywhere else on the canvas still pans, exactly as before.
}
```

That makes `active_mask_handle` load-bearing: it records whether *this* drag belongs to the
mask, and it is what stops a pan from dragging a handle it merely passed over.

---

## Task 1: `mask_tool` — the pure maths

**Files:**
- Create: `crates/app/src/mask_tool.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Register the module**

In `crates/app/src/lib.rs`, in alphabetical position — between `histogram` and `history`:

```rust
pub mod mask_tool;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/app/src/mask_tool.rs` containing **only** the imports and the test module:

```rust
//! Pure linear-gradient interaction math on `edit_model::LinearGradient` (normalized [0,1]).

use edit_model::LinearGradient;

#[cfg(test)]
mod tests {
    use super::*;

    /// A gradient running left to right across the middle of a square frame.
    fn horizontal() -> LinearGradient {
        LinearGradient {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
            invert: false,
        }
    }

    #[test]
    fn an_endpoint_is_grabbed_in_preference_to_the_line() {
        // The pointer is on the axis *and* on an endpoint. Moving the whole gradient when the
        // user aimed at an end would be the more annoying of the two mistakes.
        let g = horizontal();
        assert_eq!(hit_test(&g, [0.2, 0.5], 1.0, 0.04), Some(Handle::From));
        assert_eq!(hit_test(&g, [0.8, 0.5], 1.0, 0.04), Some(Handle::To));
    }

    #[test]
    fn the_line_is_grabbed_between_the_endpoints() {
        let g = horizontal();
        assert_eq!(hit_test(&g, [0.5, 0.5], 1.0, 0.04), Some(Handle::Line));
    }

    #[test]
    fn nothing_is_grabbed_far_from_the_gradient() {
        let g = horizontal();
        assert_eq!(hit_test(&g, [0.5, 0.9], 1.0, 0.04), None);
        assert_eq!(hit_test(&g, [0.5, 0.1], 1.0, 0.04), None);
    }

    #[test]
    fn hit_testing_the_line_is_aspect_correct() {
        // ⚠ The same non-square trap `LinearGradient::coverage` needed, in its third guise.
        // On a 3:1 frame a point 0.05 *normalised* below a horizontal axis is only 0.05 frame
        // *heights* away — but a point 0.05 normalised to the side is 0.15 heights away.
        // Distance to the axis must be measured in screen space or the grab region is
        // anisotropic: generous vertically, mean horizontally.
        let g = LinearGradient {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
            invert: false,
        };
        let aspect = 3.0;
        // 0.06 normalised below the axis = 0.06 heights: outside a 0.04 tolerance.
        assert_eq!(hit_test(&g, [0.5, 0.56], aspect, 0.04), None);
        // 0.03 normalised below = 0.03 heights: inside it.
        assert_eq!(hit_test(&g, [0.5, 0.53], aspect, 0.04), Some(Handle::Line));
    }

    #[test]
    fn dragging_an_endpoint_leaves_the_other_alone() {
        let g = horizontal();
        let out = apply_drag(g, Handle::From, 0.1, -0.2);
        assert_eq!(out.from, [0.3, 0.3]);
        assert_eq!(out.to, g.to, "the far end must not move");
    }

    #[test]
    fn dragging_the_line_moves_both_ends_equally() {
        // The gradient translates: its direction and length are unchanged, which is what
        // makes "reposition without reshaping" possible.
        let g = horizontal();
        let out = apply_drag(g, Handle::Line, 0.1, 0.05);
        assert_eq!(out.from, [0.3, 0.55]);
        assert_eq!(out.to, [0.9, 0.55]);
    }

    #[test]
    fn dragging_clamps_to_the_frame() {
        // An endpoint dragged off the canvas would be unreachable afterwards.
        let g = horizontal();
        let out = apply_drag(g, Handle::To, 5.0, 5.0);
        assert_eq!(out.to, [1.0, 1.0]);
    }

    #[test]
    fn invert_survives_a_drag() {
        // It is not a positional property, so no drag should disturb it.
        let g = LinearGradient {
            invert: true,
            ..horizontal()
        };
        assert!(apply_drag(g, Handle::Line, 0.1, 0.1).invert);
        assert!(apply_drag(g, Handle::From, 0.1, 0.1).invert);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p app --lib mask_tool
```

Expected: **compile error**, `cannot find function hit_test in this scope` and
`cannot find type Handle in this scope`.

- [ ] **Step 4: Write the implementation**

Insert between the `use` line and the `#[cfg(test)]` module:

```rust
/// What a drag is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    From,
    To,
    /// The whole gradient — both endpoints move together.
    Line,
}

/// Which handle (if any) is under normalized point `p`, within `tol`.
///
/// `aspect` is the frame's width divided by its height.
///
/// ⚠ **Distances are measured in screen space, not normalized space** — the same requirement
/// `LinearGradient::coverage` has, for the same reason. Without it the grab region is
/// anisotropic: on a 3:1 frame you could grab the line from three times further away
/// horizontally than vertically. Working in units of frame height makes it round.
///
/// Endpoints are tested before the line, so aiming at an end moves that end rather than the
/// whole gradient.
pub fn hit_test(g: &LinearGradient, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle> {
    // Everything below is in units of frame height.
    let pt = (p[0] * aspect, p[1]);
    let a = (g.from[0] * aspect, g.from[1]);
    let b = (g.to[0] * aspect, g.to[1]);
    let dist = |q: (f32, f32)| ((pt.0 - q.0).powi(2) + (pt.1 - q.1).powi(2)).sqrt();

    if dist(a) <= tol {
        return Some(Handle::From);
    }
    if dist(b) <= tol {
        return Some(Handle::To);
    }

    // Distance to the segment, not the infinite line: past either end it is the endpoint
    // distance, which the two checks above already rejected.
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return None;
    }
    let t = (((pt.0 - a.0) * dx + (pt.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    let closest = (a.0 + t * dx, a.1 + t * dy);
    if dist(closest) <= tol {
        return Some(Handle::Line);
    }
    None
}

/// Apply a normalized drag `(dx, dy)` to `handle`, clamped to the frame.
///
/// ⚠ Clamping matters: an endpoint dragged off the canvas would be unreachable afterwards.
pub fn apply_drag(g: LinearGradient, handle: Handle, dx: f32, dy: f32) -> LinearGradient {
    let mv = |p: [f32; 2]| [(p[0] + dx).clamp(0.0, 1.0), (p[1] + dy).clamp(0.0, 1.0)];
    let mut out = g;
    match handle {
        Handle::From => out.from = mv(g.from),
        Handle::To => out.to = mv(g.to),
        Handle::Line => {
            out.from = mv(g.from);
            out.to = mv(g.to);
        }
    }
    out
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app --lib mask_tool
```

Expected: `test result: ok. 8 passed`.

- [ ] **Step 6: Prove the aspect test can fail**

Temporarily drop the correction — change `let pt = (p[0] * aspect, p[1]);` to
`let pt = (p[0], p[1]);` and the two `* aspect` in `a` and `b` likewise — then run:

```bash
cargo test -p app --lib mask_tool
```

Expected: `hit_testing_the_line_is_aspect_correct` **fails**. Restore and re-run.

⚠ If restoring from a copy rather than by editing, `touch crates/app/src/mask_tool.rs`
afterwards — `cp -p` preserves the older mtime and cargo reuses the sabotaged artifact.

- [ ] **Step 7: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, **556 tests** (548 before, plus eight here).

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/mask_tool.rs crates/app/src/lib.rs
git commit -m "feat(app): pure hit-testing and drag maths for gradient masks"
```

---

## Task 2: Selection state and the canvas overlay

**Files:**
- Modify: `crates/app/src/main.rs` — two `Session` fields
- Modify: `crates/app/src/develop/canvas.rs` — `draw_mask_overlay` and the drag precedence

- [ ] **Step 1: Add the `Session` fields**

In `struct Session` in `crates/app/src/main.rs`, beside `active_handle`:

```rust
    /// Which mask the canvas is editing, if any. Index into `edits.masks`.
    ///
    /// ⚠ **View state, not an edit.** It must not reach the sidecar, transfer on a paste or
    /// appear in a preset — `Edits` is the file format, and a selection is not part of the
    /// photograph's description. `Session` is per-photo, so this resets on a switch for free.
    selected_mask: Option<usize>,
    /// The mask handle this drag is moving, if the drag started on one.
    ///
    /// ⚠ Load-bearing for panning: a drag that did *not* start on a handle must still pan the
    /// image, so this records whether the mask claimed the gesture.
    active_mask_handle: Option<mask_tool::Handle>,
```

In `finalize_session`'s `Session { .. }` literal, beside `active_handle: None`:

```rust
            selected_mask: None,
            active_mask_handle: None,
```

Add to `main.rs`'s imports:

```rust
use rustroom_app::mask_tool;
```

- [ ] **Step 2: Write the overlay**

In `crates/app/src/develop/canvas.rs`, add below `draw_crop_overlay`:

```rust
/// Draw the selected mask's handles and guide lines, and apply any drag.
///
/// Returns whether a drag settled into a new history entry — the same contract
/// `draw_crop_overlay` has.
///
/// ⚠ **Three lines, not a tint.** A faithful coverage tint needs three quads, because coverage
/// is clamped flat either side of the ramp; two boundary lines state the same thing exactly
/// and do not wash out the photograph being judged.
fn draw_mask_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
) -> bool {
    let Some(i) = s.selected_mask else {
        return false;
    };
    let Some(m) = s.edits.masks.get(i) else {
        return false;
    };
    let edit_model::Mask::Linear(g) = m.mask;

    let to_screen = |p: [f32; 2]| {
        egui::pos2(
            img_rect.left() + p[0] * img_rect.width(),
            img_rect.top() + p[1] * img_rect.height(),
        )
    };
    let (a, b) = (to_screen(g.from), to_screen(g.to));

    let painter = ui.painter_at(img_rect);
    let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
    painter.line_segment([a, b], stroke);

    // ⚠ The perpendicular is computed **in screen space**, from the on-screen axis — the same
    // trap as `coverage` and `hit_test`, in its third guise. A normalised-space perpendicular
    // is visibly not perpendicular on a 3:2 frame.
    let axis = b - a;
    let len = axis.length();
    if len > f32::EPSILON {
        let perp = egui::vec2(-axis.y, axis.x) / len;
        // Long enough to cross the image from any point inside it.
        let reach = img_rect.width() + img_rect.height();
        for end in [a, b] {
            painter.line_segment(
                [end - perp * reach, end + perp * reach],
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120)),
            );
        }
    }
    for end in [a, b] {
        painter.circle_filled(end, 5.0, egui::Color32::WHITE);
    }

    let aspect = img_rect.width() / img_rect.height();
    if let Some(p) = resp.interact_pointer_pos() {
        let nx = ((p.x - img_rect.left()) / img_rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - img_rect.top()) / img_rect.height()).clamp(0.0, 1.0);
        if resp.drag_started() {
            s.active_mask_handle = mask_tool::hit_test(&g, [nx, ny], aspect, 0.04);
        }
        if resp.dragged() {
            if let Some(h) = s.active_mask_handle {
                let d = resp.drag_delta();
                let dx = d.x / img_rect.width();
                let dy = d.y / img_rect.height();
                if let Some(m) = s.edits.masks.get_mut(i) {
                    let edit_model::Mask::Linear(cur) = m.mask;
                    m.mask = edit_model::Mask::Linear(mask_tool::apply_drag(cur, h, dx, dy));
                }
            }
        }
        if resp.drag_stopped() && s.active_mask_handle.take().is_some() {
            s.history.push(s.edits.clone());
            return true;
        }
    }
    false
}
```

⚠ Add `use rustroom_app::mask_tool;` to `canvas.rs`'s imports.

⚠ `let edit_model::Mask::Linear(g) = m.mask;` is an irrefutable pattern **only while `Mask` has
one variant**. When 3.1b adds `Radial`, this becomes a compile error — which is the right
outcome, since the overlay must then decide what to draw.

- [ ] **Step 3: Wire the precedence**

In `canvas::show`, the branch currently reads `if s.geometry_mode { … } else { pan/zoom }`.
Replace the `else` branch's opening so a mask drag is tried first and pan is the fallback:

```rust
                    if s.geometry_mode {
                        photo_edited |= draw_crop_overlay(ui, s, &resp, img_rect);
                    } else {
                        photo_edited |= draw_mask_overlay(ui, s, &resp, img_rect);
                        // ⚠ Pan only when the mask did not claim this gesture. Without the
                        // `active_mask_handle` guard, dragging a handle would also pan the
                        // image underneath it.
                        if resp.dragged() && s.active_mask_handle.is_none() {
                            s.pan += resp.drag_delta();
                        }
```

leaving the scroll/zoom code that follows exactly as it is.

- [ ] **Step 4: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, 556 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/main.rs crates/app/src/develop/canvas.rs
git commit -m "feat(app): draggable gradient handles on the canvas"
```

---

## Task 3: The Masks panel section

**Files:**
- Modify: `crates/app/src/mask_tool.rs` — `selection_after_delete` and its tests
- Modify: `crates/app/src/develop/panel.rs` — the Masks section

- [ ] **Step 1: Write the failing test for selection clamping**

⚠ Deleting a mask can leave `selected_mask` indexing past the end. The rule is worth a pure
function and a test rather than an inline `if` nobody checks.

Add to `crates/app/src/mask_tool.rs`'s `mod tests`:

```rust
    #[test]
    fn deleting_a_mask_keeps_the_selection_valid() {
        // Deleting *before* the selection shifts it down; deleting the selection itself drops
        // it; deleting after leaves it alone. The same class of rule `Selection::clamp`
        // already owns for the Library grid.
        assert_eq!(selection_after_delete(Some(2), 0, 3), Some(1));
        assert_eq!(selection_after_delete(Some(2), 2, 3), None);
        assert_eq!(selection_after_delete(Some(0), 2, 3), Some(0));
        assert_eq!(selection_after_delete(None, 1, 3), None);
    }

    #[test]
    fn deleting_the_last_mask_clears_the_selection() {
        assert_eq!(selection_after_delete(Some(0), 0, 1), None);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p app --lib mask_tool
```

Expected: **compile error**, `cannot find function selection_after_delete in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/app/src/mask_tool.rs`, below `apply_drag`:

```rust
/// Where the selection lands after the mask at `removed` is deleted from a list of `len`.
///
/// ⚠ Deleting the selected mask clears the selection rather than moving it to a neighbour —
/// the same rule `Selection::clamp` follows for the Library, and for the same reason: silently
/// selecting a *different* thing is worse than selecting nothing, because the next slider drag
/// would then edit something the user never chose.
pub fn selection_after_delete(
    selected: Option<usize>,
    removed: usize,
    len: usize,
) -> Option<usize> {
    let sel = selected?;
    if sel >= len || removed >= len {
        return None;
    }
    match sel.cmp(&removed) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(sel - 1),
        std::cmp::Ordering::Less => Some(sel),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app --lib mask_tool
```

Expected: `test result: ok. 10 passed`.

- [ ] **Step 5: Add the Masks section to the panel**

In `crates/app/src/develop/panel.rs`, inside the `ScrollArea` and **after** the last existing
`CollapsingHeader`, add:

```rust
                        egui::CollapsingHeader::new("Masks").show(ui, |ui| {
                            if ui.button("Add graduated filter").clicked() {
                                s.edits.masks.push(edit_model::MaskedAdjustment {
                                    mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                                        // A visible default: top to bottom across the middle
                                        // third, so the handles land on screen and the ramp is
                                        // immediately obvious.
                                        from: [0.5, 0.33],
                                        to: [0.5, 0.67],
                                        invert: false,
                                    }),
                                    adjustments: edit_model::LocalAdjustments::default(),
                                });
                                s.selected_mask = Some(s.edits.masks.len() - 1);
                                settled = true;
                            }

                            let mut remove: Option<usize> = None;
                            for i in 0..s.edits.masks.len() {
                                ui.horizontal(|ui| {
                                    let picked = s.selected_mask == Some(i);
                                    if ui
                                        .selectable_label(picked, format!("Gradient {}", i + 1))
                                        .clicked()
                                    {
                                        s.selected_mask = if picked { None } else { Some(i) };
                                    }
                                    if ui.button("Delete").clicked() {
                                        remove = Some(i);
                                    }
                                });
                            }
                            if let Some(i) = remove {
                                s.selected_mask = mask_tool::selection_after_delete(
                                    s.selected_mask,
                                    i,
                                    s.edits.masks.len(),
                                );
                                s.edits.masks.remove(i);
                                settled = true;
                            }

                            if let Some(i) = s.selected_mask {
                                if let Some(m) = s.edits.masks.get_mut(i) {
                                    ui.separator();
                                    let a = &mut m.adjustments;
                                    slider(ui, &mut a.white_balance.temp, -100.0..=100.0, "Temp");
                                    slider(ui, &mut a.white_balance.tint, -100.0..=100.0, "Tint");
                                    slider(ui, &mut a.exposure, -5.0..=5.0, "Exposure");
                                    slider(ui, &mut a.contrast, -100.0..=100.0, "Contrast");
                                    slider(ui, &mut a.highlights, -100.0..=100.0, "Highlights");
                                    slider(ui, &mut a.shadows, -100.0..=100.0, "Shadows");
                                    slider(ui, &mut a.whites, -100.0..=100.0, "Whites");
                                    slider(ui, &mut a.blacks, -100.0..=100.0, "Blacks");
                                    slider(ui, &mut a.saturation, -100.0..=100.0, "Saturation");
                                    slider(ui, &mut a.vibrance, -100.0..=100.0, "Vibrance");
                                }
                            }

                            theme::hint(
                                ui,
                                "Select a mask to drag its handles on the image. The two \
                                 outer lines mark where the effect starts and reaches full \
                                 strength.",
                            );
                        });
```

⚠ Add `use rustroom_app::mask_tool;` to `panel.rs`'s imports if it is not already there.

⚠ **The panel's placement is provisional**, as the spec requires — this is the concept that
caused the layout work to be parked, and the layout pass after M3.1 gives it a real home.

⚠ `slider` is the closure already defined in this function; it records `changed` and `settled`
so masks inherit "one history entry per settled drag" with no new machinery. `remove` is
collected and applied *after* the loop because mutating the vector while iterating it would
panic.

⚠ **If the borrow checker objects**, it will be because `slider` is an `FnMut` captured by the
`CollapsingHeader` closure alongside `&mut s`. The established fix in this codebase is the one
`develop_body` already uses: read what you need into locals, mutate after the closure closes.
Do **not** reach for `RefCell` — two compile errors of this exact shape are already recorded in
`CLAUDE.md`, and both were solved by restructuring rather than by interior mutability.

- [ ] **Step 6: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, **558 tests** (556 plus the two clamping tests).

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/develop/panel.rs crates/app/src/mask_tool.rs
git commit -m "feat(app): a Masks panel section for adding and adjusting gradients"
```

---

## Task 4: Verify and document

**Files:** `CLAUDE.md`

- [ ] **Step 1: Launch the GUI**

```bash
cargo run --release -p app
```

- [ ] **Step 2: Walk the feature**

1. Open a RAW. In the right panel, expand **Masks** and click **Add graduated filter**. Three
   white lines and two dots appear across the middle of the image.
2. Drag **Exposure** to −2. The lower part of the image darkens, ramping between the two outer
   lines. ⚠ Above the top line it should be untouched and below the bottom line fully affected
   — that is the clamp working.
3. Drag an **endpoint**: only that end moves. Drag the **line between them**: the whole gradient
   translates without changing angle or length.
4. ⚠ **Drag somewhere away from the handles: the image should still pan.** That is the
   precedence rule; if panning is dead while a mask is selected, `active_mask_handle` is not
   guarding it.
5. **Ctrl+Z** once after a handle drag returns the gradient, not the slider — one gesture, one
   undo step.
6. **Deselect** by clicking the selected row again: handles disappear, the effect stays.
7. **Delete** the mask: the effect goes and the canvas is clean.
8. **Switch photos and back**: the mask persists via the sidecar, and the selection is cleared
   because it is view state.
9. **Export** with a mask active and confirm the exported file shows it.

- [ ] **Step 3: Update `CLAUDE.md`**

Add a section in the "Current status" block, house style: that M3.1a is complete, both halves;
the no-new-mode decision and ⚠ the refined precedence rule that a drag only belongs to the mask
if it *started* on a handle, or panning dies; guide lines rather than a tint and why; that
`mask_tool` is a **lib** module unlike `theme`/`modals`, so it is properly testable; ⚠ the
aspect correction appearing a **third** time — coverage, hit-testing, and the drawn
perpendicular; `selection_after_delete` following `Selection::clamp`'s rule that a vanished
selection clears rather than moves; that `let Mask::Linear(g) = ..` is irrefutable only while
there is one variant, and becomes a useful compile error at 3.1b; and the caveats — provisional
placement, no reordering or renaming, no keyboard nudging, and that **M3.1 still does not close
until 3.1c's brush**.

Then set the **▶ NEXT ACTION** to **3.1b — the radial gradient**, noting it is the test of
whether 3.1a's shape was right: if `Mask` gaining a variant is mostly a matter of a coverage
function, a hit-test arm and an overlay arm, the architecture held.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-13-masking-ui.md
git commit -m "docs: record the masking canvas UI"
```
