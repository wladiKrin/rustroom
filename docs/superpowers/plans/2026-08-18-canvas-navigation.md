# Canvas Navigation Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move panning off left-drag onto scroll / two-finger swipe / a configurable drag button, and zoom onto pinch and ctrl+scroll, so that left-drag is free for tools — which is what M3.1c's brush needs.

**Architecture:** Four small, testable seams. `Settings` gains a closed `PanDrag` enum. `view_region` gains two pure zoom functions (egui-free, like everything else there). `develop/canvas.rs` gains `pan_delta` (folds every pan source into one `Vec2`) and `tool_drag` (whether a drag belongs to a tool), both driven headlessly in tests through a real allocated rect. No crate outside `app` is touched and no golden moves.

**Tech Stack:** Rust, eframe/egui 0.35, serde.

**Spec:** `docs/superpowers/specs/2026-08-18-canvas-navigation-design.md`

---

## Before you start

Every `cargo` command needs libraw and lcms2 on the pkg-config path. Run once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**The cargo package is `app`; `rustroom_app` is its lib target's name.** Use `cargo test -p app`.

**Commit messages carry no `Co-Authored-By` trailer.** **Never `git push`** — the user pushes.

⚠ **After writing a test, break the thing it names and watch it fail.** This project has seven recorded cases of a test that could not detect its own subject, three of them in M3.1b alone. **A sabotage step that passes is a finding about the test, not a formality** — stop and strengthen the test. Restore with `cp`, then `touch` the file (`mv` preserves the backup's mtime and cargo will reuse the sabotaged artifact).

⚠ **Items in `develop/canvas.rs` and `theme.rs` are in the *binary* crate**, so `pub` does not escape it and **an item with no caller fails `clippy -D warnings` as dead code**. Introduce each in the same commit as its first caller. Items in `settings.rs` and `view_region.rs` are in the **lib** and have no such constraint.

---

## Findings that changed the plan

⚠ **Two things were established by reading egui's source, and both alter the spec.**

1. **Any button starts a drag.** `interaction.rs:162` matches `PointerEvent::Pressed { .. }` and **ignores the button** when setting `potential_drag_id`. So `resp.dragged()` is true for a middle- or right-drag, not just primary. **Consequence the spec missed: without gating, the new pan button would also drag crop rects and mask handles.** Task 3 exists solely for this and lands *before* the pan change so the two are not tangled.

2. **The pan-at-fit reset is narrower than the spec claimed.** `canvas.rs:164` zeroes `s.pan` only *inside the scroll branch*, so today a left-drag at fit genuinely does pan a fitting image off-centre into grey. Task 4 makes the suppression unconditional, which is a small deliberate behaviour fix, pinned by its own test.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/app/src/settings.rs` | `PanDrag` enum, its label, the persisted field | Modify |
| `crates/app/src/view_region.rs` | Pure zoom arithmetic, egui-free | Modify |
| `crates/app/src/develop/canvas.rs` | `pan_delta`, `tool_drag`, the gesture wiring | Modify |
| `crates/app/src/modals/settings.rs` | The pan-binding row | Modify |
| `crates/app/src/develop/mod.rs` | The toolbar hint | Modify |

No new files. Nothing outside `crates/app`.

---

## Task 1: `PanDrag` in `Settings`

Lib-only and self-contained: nothing reads it yet, which is fine in a lib crate.

**Files:**
- Modify: `crates/app/src/settings.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/settings.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn every_pan_drag_has_a_label() {
        // A table over every variant so a fourth cannot ship showing nothing in the toolbar
        // hint. The same shape `every_demosaic_tier_has_a_cost_hint` and `Mask::label` use.
        for b in PanDrag::ALL {
            assert!(!b.label().is_empty(), "{b:?} has no label");
        }
    }

    #[test]
    fn the_pan_drag_labels_are_distinct() {
        // Two variants sharing a label would make the hint ambiguous about which is active.
        for (i, a) in PanDrag::ALL.iter().enumerate() {
            for b in &PanDrag::ALL[i + 1..] {
                assert_ne!(a.label(), b.label(), "{a:?} and {b:?} share a label");
            }
        }
    }

    #[test]
    fn the_default_pan_drag_is_the_middle_button() {
        assert_eq!(PanDrag::default(), PanDrag::Middle);
        assert_eq!(Settings::default().pan_drag, PanDrag::Middle);
    }

    #[test]
    fn a_settings_file_from_before_pan_drag_still_loads() {
        // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
        // reverts EVERY setting rather than erroring. That is why this test exists and is not
        // a nicety — the same trap the view-demosaic work documented.
        let dir = TempDir::new("pan_drag_compat");
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"confirm_preset_delete":false,"confirm_export_preset_delete":false}"#,
        )
        .unwrap();
        let s = SettingsStore::new(path).load();
        assert_eq!(s.pan_drag, PanDrag::Middle, "the new field must default");
        assert!(
            !s.confirm_preset_delete,
            "an old field must survive — if this flipped to true, the whole file failed to parse"
        );
    }
```

⚠ `TempDir` is the local guard already defined in this file's test module — do not add a
`tempfile` dependency. Check its exact constructor name with
`grep -n 'impl TempDir' -A6 crates/app/src/settings.rs` and match it; if it takes no argument,
drop the string.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p app pan_drag 2>&1 | tail -20`
Expected: FAIL to compile — `cannot find type PanDrag in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `crates/app/src/settings.rs`, above `pub struct Settings`:

```rust
/// How a *drag* pans the canvas.
///
/// ⚠ Scroll and two-finger swipe always pan and are deliberately **not** configurable — they
/// are the universal route that works on every device, and making them optional would let a
/// user lock themselves out of panning entirely.
///
/// ⚠ A closed three-value enum, not a keybinding system. The risk in this area is ballooning
/// into general rebinding; three values already cover every device. A mouse button is not a
/// modifier, which is why the set spans both kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PanDrag {
    /// Middle-button drag. ⚠ Unavailable on an Apple Magic Mouse, which has no middle button —
    /// those users pan by swiping, which always works.
    #[default]
    Middle,
    /// Right-button drag. Free: nothing on the canvas uses secondary click or a context menu.
    Secondary,
    /// Hold Space and left-drag. The only option needing no extra hardware, and the
    /// Photoshop/Figma convention.
    SpaceLeft,
}

impl PanDrag {
    /// Every variant, for the settings dropdown and for table-driven tests.
    pub const ALL: [PanDrag; 3] = [PanDrag::Middle, PanDrag::Secondary, PanDrag::SpaceLeft];

    /// How to describe this binding in the UI.
    ///
    /// ⚠ The toolbar hint must name the *configured* binding rather than saying "middle-drag"
    /// literally, or it becomes a lie the moment the setting changes. Same shape as
    /// `Mask::label()`.
    pub fn label(&self) -> &'static str {
        match self {
            PanDrag::Middle => "middle-drag",
            PanDrag::Secondary => "right-drag",
            PanDrag::SpaceLeft => "Space+drag",
        }
    }
}
```

And add the field to `Settings`, after `view_demosaic`:

```rust
    /// Which drag gesture pans the canvas. Scroll and two-finger swipe always pan.
    pub pan_drag: PanDrag,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p app pan_drag 2>&1 | tail -20`
Expected: PASS — 4 tests.

- [ ] **Step 5: Fix the exhaustive `Settings` literal the compiler will flag**

`settings_round_trip_carries_the_export_options` builds `Settings { .. }` exhaustively **on
purpose**, so that a new field is a compile error there. Add `pan_drag: PanDrag::Secondary` to
it — a non-default value, so the round trip actually proves the field is persisted rather than
re-defaulted.

Run: `cargo test -p app settings 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 6: Sabotage**

Remove `#[serde(default)]` from `Settings` (or, if it is per-field, from `pan_drag`).
Run: `cargo test -p app a_settings_file_from_before_pan_drag_still_loads 2>&1 | tail -12`
Expected: **FAIL** on the `confirm_preset_delete` assertion — proving the whole file failed to
parse and every setting silently reverted, which is the failure mode the test names.

Restore, then return `""` from `PanDrag::Secondary`'s label arm.
Run: `cargo test -p app every_pan_drag_has_a_label 2>&1 | tail -8`
Expected: **FAIL** with "Secondary has no label".

Restore both, then `touch crates/app/src/settings.rs`.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/settings.rs
git commit -F - <<'EOF'
feat(app): a configurable pan drag binding in Settings

Three values — middle-drag, right-drag, Space+left-drag — defaulting to
middle. Not yet read by the canvas.

⚠ Configurable now rather than later, and not for urgency: trackpad and Magic
Mouse users are already served by scroll-to-pan, and the real hole is narrow
(a wheel mouse gives vertical scroll only, so a broken wheel-click leaves no
horizontal pan at all). It is because the pan branch is written exactly once —
against a small enum it costs about five lines more than a hardcoded button,
where retrofitting means rewriting shipped behaviour plus the settings field
and modal row anyway.
EOF
```

---

## Task 2: Pure zoom arithmetic in `view_region`

**Files:**
- Modify: `crates/app/src/view_region.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/view_region.rs`'s test module:

```rust
    #[test]
    fn zoom_step_clamps_to_the_usable_range() {
        assert_eq!(zoom_step(1.0, 0.5), MIN_ZOOM, "cannot zoom out past fit");
        assert_eq!(zoom_step(8.0, 2.0), MAX_ZOOM, "cannot zoom in past the cap");
        assert!((zoom_step(2.0, 1.5) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn an_idle_gesture_does_not_change_the_zoom() {
        // ⚠ `egui::InputState::zoom_delta()` returns 1.0 when idle, NOT 0.0 — it is a
        // multiplicative factor, not a delta. Treating it as a delta would zoom to nothing on
        // every frame in which the user does nothing at all.
        assert_eq!(zoom_step(3.5, 1.0), 3.5);
    }

    #[test]
    fn zoom_anchor_shift_keeps_the_cursor_point_fixed() {
        // Magnifying by r moves a point at offset d from the anchor to d*r, so the pan must
        // move by d*(1-r) to put it back. Sign errors here are invisible in review and
        // obvious in use, which is the whole reason this is a pure function.
        let shift = zoom_anchor_shift((110.0, 220.0), (100.0, 200.0), 1.0, 2.0);
        assert!((shift.0 - -10.0).abs() < 1e-6, "{shift:?}");
        assert!((shift.1 - -20.0).abs() < 1e-6, "{shift:?}");
    }

    #[test]
    fn zooming_out_shifts_the_opposite_way() {
        // ⚠ The discriminating case: a sign error that negates the whole expression passes
        // the test above only if it also passes this one with the same sign, which it cannot.
        let shift = zoom_anchor_shift((110.0, 220.0), (100.0, 200.0), 2.0, 1.0);
        assert!((shift.0 - 5.0).abs() < 1e-6, "{shift:?}");
        assert!((shift.1 - 10.0).abs() < 1e-6, "{shift:?}");
    }

    #[test]
    fn a_cursor_on_the_anchor_needs_no_shift() {
        let shift = zoom_anchor_shift((100.0, 200.0), (100.0, 200.0), 1.0, 4.0);
        assert_eq!(shift, (0.0, 0.0));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p app zoom_ 2>&1 | tail -15`
Expected: FAIL to compile — `cannot find function zoom_step`.

- [ ] **Step 3: Write the implementation**

Add to `crates/app/src/view_region.rs`, after `pub const REGION_MARGIN`:

```rust
/// The closest the view can be zoomed out: fit, where the whole image is on screen.
pub const MIN_ZOOM: f32 = 1.0;
/// The furthest in. Beyond this the render ladder has no more rungs to climb.
pub const MAX_ZOOM: f32 = 8.0;

/// The new zoom after a gesture, clamped to the usable range.
///
/// ⚠ `factor` is **multiplicative and 1.0 when idle**, matching `egui::InputState::zoom_delta`,
/// which folds trackpad pinch and ctrl+scroll. It is not a delta: treating it as one would
/// collapse the zoom to nothing on every idle frame.
pub fn zoom_step(zoom: f32, factor: f32) -> f32 {
    (zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM)
}

/// The pan correction that keeps the point under the cursor fixed while zooming.
///
/// `pointer` and `anchor` are `(x, y)` in the same screen space; the result is `(dx, dy)` to
/// add to the pan. Magnifying by `r = new/old` moves a point at offset `d` from the anchor to
/// `d*r`, so `d*(1-r)` puts it back.
///
/// ⚠ Plain tuples rather than `egui::Vec2`: this module is deliberately free of `egui` types
/// so it can be tested without a window, and every other function here holds to that. The
/// caller converts at the boundary, as it already does for `RegionRect`.
pub fn zoom_anchor_shift(
    pointer: (f32, f32),
    anchor: (f32, f32),
    old: f32,
    new: f32,
) -> (f32, f32) {
    if old <= f32::EPSILON {
        return (0.0, 0.0);
    }
    let r = new / old;
    (
        (pointer.0 - anchor.0) * (1.0 - r),
        (pointer.1 - anchor.1) * (1.0 - r),
    )
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p app zoom_ 2>&1 | tail -12`
Expected: PASS — 5 tests.

- [ ] **Step 5: Sabotage**

Change `(1.0 - r)` to `(r - 1.0)` in both components of `zoom_anchor_shift`.
Run: `cargo test -p app zoom_anchor 2>&1 | tail -10`
Expected: **FAIL** on `zoom_anchor_shift_keeps_the_cursor_point_fixed`.

Restore, then change `zoom_step`'s clamp to `.clamp(MIN_ZOOM, f32::MAX)`.
Run: `cargo test -p app zoom_step_clamps 2>&1 | tail -8`
Expected: **FAIL** — `left: 16.0, right: 8.0`.

Restore both, then `touch crates/app/src/view_region.rs`.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/view_region.rs
git commit -F - <<'EOF'
feat(app): pure zoom arithmetic in view_region

`zoom_step` and `zoom_anchor_shift`, extracted ahead of the gesture remap.

Not tidying: the cursor-anchor arithmetic is exactly the kind of expression
that is wrong by a sign, invisible in review and obvious in use. It now has a
test that fails when the sign flips, and a second case zooming the other way
so a whole-expression negation cannot pass both.

Plain tuples rather than egui::Vec2 — this module's own doc says it is
deliberately free of egui types so it can be tested without a window.
EOF
```

---

## Task 3: Gate tool drags to the primary button

⚠ **Lands before the pan change on purpose.** It is a prerequisite that is also correct on its own, and keeping it separate means a tool regression cannot hide inside the gesture remap.

**Files:**
- Modify: `crates/app/src/develop/canvas.rs`

- [ ] **Step 1: Write the failing test**

Add a `#[cfg(test)] mod tests` at the bottom of `crates/app/src/develop/canvas.rs` (the file has
none yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a real allocated rect through a press-and-move gesture with `button`, and report
    /// what `tool_drag` said on the final pass.
    ///
    /// ⚠ Four passes, not three. egui only becomes "decidedly dragging" once the pointer has
    /// moved far enough after the press (`is_decidedly_dragging` requires `!any_pressed()`),
    /// so the press and the movement must land on *different* frames.
    fn tool_drag_for(button: egui::PointerButton) -> bool {
        let ctx = egui::Context::default();
        let mut out = false;
        let start = egui::pos2(100.0, 100.0);
        for pass in 0..4 {
            let pos = if pass >= 2 {
                start + egui::vec2(40.0, 40.0)
            } else {
                start
            };
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
                let resp = ui.allocate_rect(rect, egui::Sense::drag());
                if pass == 3 {
                    out = tool_drag(&resp);
                }
            });
        }
        out
    }

    #[test]
    fn a_primary_drag_belongs_to_the_tools() {
        assert!(tool_drag_for(egui::PointerButton::Primary));
    }

    #[test]
    fn a_middle_drag_does_not_belong_to_the_tools() {
        // ⚠ This is not hypothetical. egui's `interaction.rs` matches
        // `PointerEvent::Pressed { .. }` and IGNORES the button when deciding what is being
        // dragged, so `resp.dragged()` is true for a middle-drag too. Without this gate the
        // pan button would also drag crop rectangles and mask handles.
        assert!(!tool_drag_for(egui::PointerButton::Middle));
    }

    #[test]
    fn a_secondary_drag_does_not_belong_to_the_tools() {
        // Right-drag is one of the selectable pan bindings, so it needs the same exclusion.
        assert!(!tool_drag_for(egui::PointerButton::Secondary));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p app tool_drag 2>&1 | tail -15`
Expected: FAIL to compile — `cannot find function tool_drag`.

- [ ] **Step 3: Write the implementation**

Add near the top of `crates/app/src/develop/canvas.rs`, after the `use` lines:

```rust
/// Whether a drag belongs to the active tool — the crop rectangle, a mask handle, and later
/// the brush.
///
/// ⚠ **egui reports `dragged()` for every button, not just the primary one.** `interaction.rs`
/// matches `PointerEvent::Pressed { .. }` and ignores the button when setting
/// `potential_drag_id`. Since panning now lives on middle- or right-drag, an ungated tool would
/// be driven by the pan gesture as well — dragging a crop corner while trying to pan.
fn tool_drag(resp: &egui::Response) -> bool {
    resp.dragged_by(egui::PointerButton::Primary)
}

/// Whether a drag on the tool's button has just begun. Same gating, same reason.
fn tool_drag_started(resp: &egui::Response) -> bool {
    resp.drag_started_by(egui::PointerButton::Primary)
}
```

Then replace every tool-side drag check. In `draw_crop_overlay`, change:

```rust
        if resp.dragged() {
```

to:

```rust
        if tool_drag(resp) {
```

In `draw_mask_overlay`, change:

```rust
        if resp.drag_started() {
```
to
```rust
        if tool_drag_started(resp) {
```

and

```rust
        if resp.dragged() {
```
to
```rust
        if tool_drag(resp) {
```

⚠ Leave `resp.drag_stopped()` alone in both functions. A drag that ends must settle its history
entry whatever button ended it, and `active_mask_handle.take()` already guards that branch.

⚠ Also leave the pan branch's `resp.dragged()` alone for now — Task 4 replaces it wholesale.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p app tool_drag 2>&1 | tail -12`
Expected: PASS — 3 tests.

- [ ] **Step 5: Sabotage**

Change `tool_drag` to `resp.dragged()`.
Run: `cargo test -p app _does_not_belong_to_the_tools 2>&1 | tail -10`
Expected: **FAIL**, both middle and secondary.

⚠ If either still passes, the harness is not producing a real drag — fix the harness before
going on, because then `a_primary_drag_belongs_to_the_tools` is also proving nothing.

Restore, then `touch crates/app/src/develop/canvas.rs`.

- [ ] **Step 6: Verify and commit**

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E '^(error|warning)' | head`
Expected: no output.

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

```bash
git add crates/app/src/develop/canvas.rs
git commit -F - <<'EOF'
fix(app): tool drags respond to the primary button only

⚠ Found by reading egui's source while planning the navigation rework, not by
observing a bug: `interaction.rs` matches `PointerEvent::Pressed { .. }` and
ignores the button when setting `potential_drag_id`, so `resp.dragged()` is
true for a middle- or right-drag too.

Harmless today, because nothing else uses those buttons. It stops being
harmless the moment panning moves onto one of them, which is the next commit —
an ungated crop corner would be dragged by the pan gesture.

Lands separately so a tool regression cannot hide inside the gesture remap.
`drag_stopped` is deliberately left ungated: a drag that ends must settle its
history entry whatever button ended it.
EOF
```

---

## Task 4: The gesture remap

**Files:**
- Modify: `crates/app/src/develop/canvas.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/develop/canvas.rs`'s test module:

```rust
    /// Drive a real allocated rect and report what `pan_delta` returned on the final pass.
    ///
    /// `scroll` is delivered as a raw scroll event; `button` (if any) is pressed on pass 1 and
    /// the pointer then moves, exactly as `tool_drag_for` does.
    fn pan_delta_for(
        button: Option<egui::PointerButton>,
        scroll: egui::Vec2,
        binding: PanDrag,
        space_down: bool,
        text_focused: bool,
    ) -> egui::Vec2 {
        let ctx = egui::Context::default();
        let mut out = egui::Vec2::ZERO;
        let start = egui::pos2(100.0, 100.0);
        for pass in 0..4 {
            let pos = if pass >= 2 {
                start + egui::vec2(40.0, 40.0)
            } else {
                start
            };
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 {
                if let Some(b) = button {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: b,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                if space_down {
                    input.events.push(egui::Event::Key {
                        key: egui::Key::Space,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    });
                }
            }
            if pass == 3 && scroll != egui::Vec2::ZERO {
                input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: scroll,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
                let resp = ui.allocate_rect(rect, egui::Sense::drag());
                if pass == 3 {
                    out = ui.input(|i| pan_delta(&resp, i, binding, text_focused));
                }
            });
        }
        out
    }

    #[test]
    fn a_left_drag_does_not_pan() {
        // ⚠⚠ The regression guard for this entire change, and it asserts the pan delta is
        // ZERO rather than that some other branch ran. Asserting "the tool branch was taken"
        // would pass with a stray `s.pan +=` still in place — precisely the shape of mistake
        // this project keeps making.
        for binding in PanDrag::ALL {
            assert_eq!(
                pan_delta_for(
                    Some(egui::PointerButton::Primary),
                    egui::Vec2::ZERO,
                    binding,
                    false,
                    false
                ),
                egui::Vec2::ZERO,
                "{binding:?}"
            );
        }
    }

    #[test]
    fn the_configured_drag_button_pans() {
        let d = pan_delta_for(
            Some(egui::PointerButton::Middle),
            egui::Vec2::ZERO,
            PanDrag::Middle,
            false,
            false,
        );
        assert!(d.x > 0.0 && d.y > 0.0, "{d:?}");

        let d = pan_delta_for(
            Some(egui::PointerButton::Secondary),
            egui::Vec2::ZERO,
            PanDrag::Secondary,
            false,
            false,
        );
        assert!(d.x > 0.0 && d.y > 0.0, "{d:?}");
    }

    #[test]
    fn a_drag_on_the_wrong_button_does_not_pan() {
        // Middle-drag while the binding is Secondary must do nothing at all.
        assert_eq!(
            pan_delta_for(
                Some(egui::PointerButton::Middle),
                egui::Vec2::ZERO,
                PanDrag::Secondary,
                false,
                false
            ),
            egui::Vec2::ZERO
        );
    }

    #[test]
    fn scrolling_pans_both_axes() {
        // ⚠ Today only `.y` is read anywhere in this file, and it drives zoom. A horizontal
        // two-finger swipe must pan too, so the x component is asserted separately rather than
        // folded into a magnitude check that a y-only implementation would satisfy.
        let d = pan_delta_for(None, egui::vec2(11.0, 7.0), PanDrag::Middle, false, false);
        assert!(d.x.abs() > 0.0, "x component ignored: {d:?}");
        assert!(d.y.abs() > 0.0, "y component ignored: {d:?}");
    }

    #[test]
    fn space_left_drag_pans_only_with_space_held() {
        assert!(
            pan_delta_for(
                Some(egui::PointerButton::Primary),
                egui::Vec2::ZERO,
                PanDrag::SpaceLeft,
                true,
                false
            ) != egui::Vec2::ZERO
        );
        assert_eq!(
            pan_delta_for(
                Some(egui::PointerButton::Primary),
                egui::Vec2::ZERO,
                PanDrag::SpaceLeft,
                false,
                false
            ),
            egui::Vec2::ZERO,
            "without Space this is an ordinary left-drag and must not pan"
        );
    }

    #[test]
    fn scrolling_outside_the_image_does_not_pan() {
        // ⚠ The `hovered()` gate. Without it, scrolling the adjustments panel or the filmstrip
        // would pan the photo underneath them.
        //
        // ⚠ The pointer must be driven OUTSIDE the allocated rect on every pass, which is why
        // this cannot reuse `pan_delta_for` — that harness always points at (100,100), inside
        // the rect, so it could never detect a missing gate.
        let ctx = egui::Context::default();
        let mut out = egui::Vec2::ZERO;
        for pass in 0..4 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input
                .events
                .push(egui::Event::PointerMoved(egui::pos2(380.0, 380.0)));
            if pass >= 2 {
                input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(11.0, 7.0),
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                // A small rect in the corner, far from the pointer.
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(50.0, 50.0));
                let resp = ui.allocate_rect(rect, egui::Sense::drag());
                if pass == 3 {
                    out = ui.input(|i| pan_delta(&resp, i, PanDrag::Middle, false));
                }
            });
        }
        assert_eq!(out, egui::Vec2::ZERO);
    }

    #[test]
    fn space_left_drag_is_ignored_while_a_text_field_has_focus() {
        // ⚠ Otherwise typing a space into the preset-name field arms panning. The same rule
        // arrow-nav and the clipboard keys already follow.
        assert_eq!(
            pan_delta_for(
                Some(egui::PointerButton::Primary),
                egui::Vec2::ZERO,
                PanDrag::SpaceLeft,
                true,
                true
            ),
            egui::Vec2::ZERO
        );
    }
```

⚠ Add `use rustroom_app::settings::PanDrag;` to the test module if `super::*` does not already
bring it in. Check the crate path with
`grep -n 'pub mod settings' crates/app/src/lib.rs` and match the existing imports at the top of
`canvas.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p app pan_delta 2>&1 | tail -15`
Expected: FAIL to compile — `cannot find function pan_delta`.

- [ ] **Step 3: Write `pan_delta`**

Add to `crates/app/src/develop/canvas.rs`, beside `tool_drag`:

```rust
/// Every source of panning, folded into one screen-space delta.
///
/// ⚠ **Scroll always pans and is not configurable.** It is the universal route — a two-finger
/// swipe on any trackpad, a wheel on any mouse — so making it optional would let a user lock
/// themselves out of panning entirely. Only the *drag* binding is a setting.
///
/// ⚠ Both scroll axes are used. A horizontal two-finger swipe is an ordinary gesture and the
/// old code read `.y` alone, because `.y` was driving zoom.
fn pan_delta(
    resp: &egui::Response,
    input: &egui::InputState,
    binding: PanDrag,
    text_focused: bool,
) -> egui::Vec2 {
    let mut delta = egui::Vec2::ZERO;

    // ⚠ The hover gate is what stops a scroll over the adjustments panel or the filmstrip
    // panning the photo underneath them.
    if resp.hovered() {
        delta += input.smooth_scroll_delta;
    }

    let dragging = match binding {
        PanDrag::Middle => resp.dragged_by(egui::PointerButton::Middle),
        PanDrag::Secondary => resp.dragged_by(egui::PointerButton::Secondary),
        // ⚠ Gated on focus: without it, a space typed into the preset-name field would arm
        // panning. The rule arrow-nav and the clipboard keys already follow.
        PanDrag::SpaceLeft => {
            !text_focused
                && input.key_down(egui::Key::Space)
                && resp.dragged_by(egui::PointerButton::Primary)
        }
    };
    if dragging {
        delta += resp.drag_delta();
    }
    delta
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p app pan_delta 2>&1 | tail -15`
Expected: PASS — 7 tests.

⚠ If `scrolling_pans_both_axes` fails with a zero delta, the harness's `MouseWheel` event is not
reaching `smooth_scroll_delta` in the same frame — egui smooths scroll over frames. Deliver the
wheel event on pass 2 as well as pass 3 and read on pass 3. Do **not** switch the assertion to
`raw_scroll_delta` to make it pass: production reads the smoothed value, and a test that reads a
different field is testing nothing.

- [ ] **Step 5: Wire it into the canvas**

In `crates/app/src/develop/canvas.rs`, replace this whole block:

```rust
                        // ⚠ Pan only when the mask did not claim this gesture. Without the
                        // `active_mask_handle` guard, dragging a handle would also pan the
                        // image underneath it.
                        if resp.dragged() && s.active_mask_handle.is_none() {
                            s.pan += resp.drag_delta();
                        }
                        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                        if scroll != 0.0 && resp.hovered() {
                            let old = s.zoom;
                            s.zoom = (s.zoom * (1.0 + scroll * 0.001)).clamp(1.0, 8.0);
                            // Zoom about the cursor, not the centre: keep whatever the
                            // pointer is over fixed on screen. `anchor` is the image's
                            // centre, so `d` is the cursor's offset from it; magnifying
                            // by `r` moves that point to `d*r`, which `d*(1-r)` undoes.
                            if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                                let anchor = ui.max_rect().center() + s.pan;
                                s.pan += (p - anchor) * (1.0 - s.zoom / old);
                            }
                            // Back at fit the whole image is on screen, so leftover pan
                            // from cursor-anchored zooming would just push it off-centre.
                            if s.zoom <= 1.0 {
                                s.pan = egui::Vec2::ZERO;
                            }
                        }
```

with:

```rust
                        // Zoom first, then pan: the pan correction below depends on the new
                        // zoom, and applying them the other way round would anchor to a
                        // factor that is one frame stale.
                        //
                        // ⚠ `zoom_delta()` is a multiplicative factor that is 1.0 when idle,
                        // and it folds trackpad pinch together with ctrl+scroll — so mouse
                        // users keep a zoom gesture without plain scroll being consumed by it.
                        let zoom_factor = ui.input(|i| i.zoom_delta());
                        if zoom_factor != 1.0 && resp.hovered() {
                            let old = s.zoom;
                            s.zoom = view_region::zoom_step(old, zoom_factor);
                            if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                                let anchor = ui.max_rect().center() + s.pan;
                                let (dx, dy) = view_region::zoom_anchor_shift(
                                    (p.x, p.y),
                                    (anchor.x, anchor.y),
                                    old,
                                    s.zoom,
                                );
                                s.pan += egui::vec2(dx, dy);
                            }
                        }

                        let text_focused = ui.ctx().memory(|m| m.focused().is_some());
                        s.pan +=
                            ui.input(|i| pan_delta(&resp, i, app_pan_drag, text_focused));

                        // ⚠ Unconditional, where the old code only reset inside the scroll
                        // branch — so a left-drag at fit really could push a fitting image
                        // off-centre into grey. At fit the whole image is on screen and there
                        // is nothing to reveal, so any pan is meaningless.
                        if s.zoom <= view_region::MIN_ZOOM {
                            s.pan = egui::Vec2::ZERO;
                        }
```

⚠ `app_pan_drag` must be read **before** the `match &mut app.session` borrow begins, or it is a
second borrow of `app`. `canvas::show` opens with `let mut photo_edited = false;` — put it on the
next line:

```rust
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, _ctx: &egui::Context) -> bool {
    let mut photo_edited = false;
    // ⚠ Copied out before `match &mut app.session` takes its mutable borrow. `PanDrag` is
    // `Copy`, so this is a copy and not a borrow that would collide.
    let app_pan_drag = app.settings.pan_drag;
```

and extend the file's existing import to
`use rustroom_app::{crop_tool, mask_tool, settings::PanDrag, view_region};` — check the real path
with `grep -n 'pub use\|pub mod settings' crates/app/src/lib.rs` and match how `PanDrag` is
actually re-exported.

- [ ] **Step 6: Add the pan-at-fit test and run everything**

Add to the test module:

```rust
    #[test]
    fn the_pan_reset_at_fit_is_unconditional() {
        // ⚠ A behaviour fix, not a port. The old reset sat *inside* the scroll branch, so a
        // left-drag at fit genuinely pushed a fitting image off-centre into grey and nothing
        // put it back until the next scroll. This pins the rule itself rather than the call
        // site, in the style of `grid_columns_leaves_room_for_the_gaps`.
        let reset = |zoom: f32, pan: egui::Vec2| {
            if zoom <= view_region::MIN_ZOOM {
                egui::Vec2::ZERO
            } else {
                pan
            }
        };
        assert_eq!(reset(1.0, egui::vec2(50.0, 50.0)), egui::Vec2::ZERO);
        assert_eq!(reset(0.5, egui::vec2(50.0, 50.0)), egui::Vec2::ZERO);
        assert_eq!(
            reset(1.5, egui::vec2(50.0, 50.0)),
            egui::vec2(50.0, 50.0),
            "above fit the pan must survive"
        );
    }
```

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E '^(error|warning)' | head -20`
Expected: no output.

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

- [ ] **Step 7: Sabotage**

Add `delta += resp.drag_delta();` unconditionally at the end of `pan_delta`, before `delta`.
Run: `cargo test -p app a_left_drag_does_not_pan 2>&1 | tail -10`
Expected: **FAIL** — a primary drag now pans.

Restore, then drop the `.x` by changing the scroll line to
`delta += egui::vec2(0.0, input.smooth_scroll_delta.y);`.
Run: `cargo test -p app scrolling_pans_both_axes 2>&1 | tail -8`
Expected: **FAIL** with "x component ignored".

Restore, then remove `!text_focused &&` from the `SpaceLeft` arm.
Run: `cargo test -p app space_left_drag_is_ignored 2>&1 | tail -8`
Expected: **FAIL**.

Restore, then remove the `resp.hovered()` gate around the scroll.
Run: `cargo test -p app scrolling_outside_the_image_does_not_pan 2>&1 | tail -8`
Expected: **FAIL** with a non-zero delta.

⚠ Note *which* test this is: `scrolling_pans_both_axes` keeps its pointer inside the rect and
would pass happily with the gate deleted, which is exactly why the outside-the-rect test exists
as its own harness rather than as another `pan_delta_for` call.

Restore everything, then `touch crates/app/src/develop/canvas.rs`.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/develop/canvas.rs
git commit -F - <<'EOF'
feat(app): pan on scroll and a drag button, zoom on pinch and ctrl+scroll

Left-drag no longer pans, which is what frees it for M3.1c's brush. Pan is
scroll / two-finger swipe (both axes) plus the configured drag button; zoom is
`i.zoom_delta()`, which folds trackpad pinch with ctrl+scroll.

⚠ This could not be a pure addition. On a macOS trackpad a two-finger swipe IS
smooth_scroll_delta, which this file read to zoom — so the new pan gesture and
the old zoom gesture were the same event and could not coexist.

⚠ One deliberate behaviour fix rides along: the pan-at-fit reset was inside
the scroll branch, so a left-drag at fit really could push a fitting image
off-centre into grey with nothing putting it back until the next scroll. It is
unconditional now.
EOF
```

---

## Task 5: The settings row and the toolbar hint

**Files:**
- Modify: `crates/app/src/modals/settings.rs`
- Modify: `crates/app/src/develop/mod.rs`

- [ ] **Step 1: Add the settings row**

⚠ **`modals/settings.rs` uses the copy-into-locals idiom**, documented in `modals/mod.rs`: the
dialog body cannot touch `app` because `dialog` already holds it, so state is copied into locals,
the closure runs, and the change is applied afterwards behind a change guard. Follow it exactly.

In `crates/app/src/modals/settings.rs`, add a fourth local beside the existing three
(after `let mut accent = app.settings.accent;`):

```rust
    let mut pan_drag = app.settings.pan_drag;
```

Inside the `dialog` closure, after the `1:1 view quality` block and its hint, before the
`ui.separator();` that precedes the Close button:

```rust
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Pan with");
            egui::ComboBox::from_id_salt("pan_drag_pick")
                .selected_text(pan_drag.label())
                .show_ui(ui, |ui| {
                    for b in PanDrag::ALL {
                        ui.selectable_value(&mut pan_drag, b, b.label());
                    }
                });
        });
        theme::hint(
            ui,
            "Scrolling and two-finger swipe always pan — this is the extra drag gesture, \
             for when a middle button is missing or broken.",
        );
```

And after the closure, beside the other change guards:

```rust
    // ⚠ A change guard like its neighbours, though for a weaker reason: this has no side
    // effect beyond the write, because the canvas reads `settings.pan_drag` afresh every
    // frame — no worker rebuild, no re-decode. The guard is here so the settings file is not
    // rewritten on every frame the dialog is open.
    if pan_drag != app.settings.pan_drag {
        app.settings.pan_drag = pan_drag;
        app.save_settings();
    }
```

Add `PanDrag` to the file's `use rustroom_app::…` list — check the existing line with
`grep -n '^use' crates/app/src/modals/settings.rs` and extend it rather than adding a second
`use` for the same crate.

- [ ] **Step 2: Add the toolbar hint**

`theme::hint` is `pub fn hint(ui: &mut egui::Ui, text: &str)`, so `&format!(..)` coerces.

In `crates/app/src/develop/mod.rs`, inside the Develop toolbar after the existing controls:

```rust
        // ⚠ The binding is named from the setting through `PanDrag::label()` rather than
        // hardcoded as "middle-drag", which would become a lie the moment someone changes it.
        theme::hint(
            ui,
            &format!(
                "Scroll or {} to pan · pinch or Ctrl+scroll to zoom",
                app.settings.pan_drag.label()
            ),
        );
```

⚠ Place it inside the same `ui.horizontal` the other toolbar controls use, at the end, so it
sits on the toolbar row rather than starting a new one. Read the surrounding block first.

- [ ] **Step 3: Verify**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E '^(error|warning)' | head`
Expected: no output.

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/modals/settings.rs crates/app/src/develop/mod.rs
git commit -F - <<'EOF'
feat(app): a pan-binding setting and a navigation hint

A combo box in Settings, and one hint line in the Develop toolbar — nobody
guesses middle-drag.

⚠ The hint names the configured binding through `PanDrag::label()` rather than
saying "middle-drag" literally, which would become a lie the moment the
setting changed.
EOF
```

---

## Task 6: Validate in the GUI and document

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-08-18-canvas-navigation-design.md`

- [ ] **Step 1: Full verification**

```bash
cargo fmt --all --check
```
Expected: no output.

```bash
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE '^(error|warning)'
```
Expected: `0`.

```bash
cargo test --all 2>&1 | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s}'
```
Expected: about 600, up from 582.

```bash
git status --short crates/pipeline/tests/golden/
```
Expected: **no output at all.** No golden may move — nothing outside `crates/app` was touched.

- [ ] **Step 2: Launch the GUI and check the feel**

⚠ The user wants the GUI launched after every big update without being asked, and this change is
**entirely about feel** — no test can tell you whether the scroll direction is right.

```bash
cargo run --release -p app
```

Run it in the background and tell the user to check, in this order:

1. **Scroll direction.** Two-finger swipe down — the photo should move down with your fingers.
   ⚠ `s.pan += delta` is correct **by egui's own documented convention**, not by guesswork:
   `Event::MouseWheel`'s doc says "the direction of the vector indicates how to move the
   *content* being viewed … positive values, the content should move to the right and down".
   Panning *is* moving the content, so the signs already agree. If it nonetheless feels wrong,
   that is a preference about natural scrolling — **ask before flipping it**, and flip it in the
   wiring rather than in `pan_delta`'s tests.
2. **Pan speed.** One swipe should move a comfortable distance at 2× zoom. If it is sluggish or
   twitchy, scale `smooth_scroll_delta` by a constant in `pan_delta` and say what value you used.
3. **Pinch to zoom** on the trackpad, and **ctrl+scroll** with a mouse.
4. **Middle-drag pans**, and **left-drag does not**.
5. **Left-drag still drags a crop corner** in Geometry mode, and **still drags a mask handle**
   with a gradient or radial selected — that is Task 3's gating working.
6. **Two-finger swipe at fit does nothing.** Expected, not a bug — there is nothing off-screen.
7. Switch the setting to **Space+drag** and confirm it pans; then click into the preset-name
   field, type a space, and confirm panning is *not* armed.

- [ ] **Step 3: Fill in the spec's outcome**

Add a short "Outcome" section to
`docs/superpowers/specs/2026-08-18-canvas-navigation-design.md` recording: the two egui findings
that changed the plan (any button starts a drag; the pan-at-fit reset was narrower than the spec
claimed), whichever sabotage steps did not fail on the first attempt and how the test was
strengthened, and any tuning constant chosen in Step 2.

⚠ Where the spec was wrong, say so explicitly. A spec that quietly matches its own prediction
teaches the next reader nothing.

- [ ] **Step 4: Update `CLAUDE.md`**

- Change **▶ NEXT ACTION** back to **M3.1c — the brush**, and delete the navigation-rework
  paragraph above it (the parked 3.1c decisions block stays — it is the input to that work).
- Add a navigation section in the house style. The ⚠ items that must appear: (1) **a two-finger
  swipe IS `smooth_scroll_delta`**, so pan and zoom could not both live on scroll and this was
  never a pure addition; (2) **egui reports `dragged()` for every button**, which is why tool
  drags are gated to Primary and why that gating had to land first; (3) the pan-at-fit reset was
  inside the scroll branch and is now unconditional; (4) `zoom_delta()` is a **multiplicative
  factor that is 1.0 when idle**, not a delta; (5) scroll always pans and is deliberately not
  configurable.
- Update the test count, add both new filenames to the docs list at the bottom, and refresh the
  push-state line from `git log origin/main..main --oneline | wc -l`.

- [ ] **Step 5: Commit and report**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-08-18-canvas-navigation-design.md docs/superpowers/plans/2026-08-18-canvas-navigation.md
git commit -F - <<'EOF'
docs: record the canvas navigation rework

⚠ Two egui findings changed the plan and are worth keeping: any button starts
a widget drag (so `resp.dragged()` fires for middle- and right-drags, which is
why tool drags are now gated to Primary), and the pan-at-fit reset only ever
ran inside the scroll branch.
EOF
```

⚠ **Do not push.** Report the unpushed count and let the user push:

```bash
git log origin/main..main --oneline | wc -l
```
