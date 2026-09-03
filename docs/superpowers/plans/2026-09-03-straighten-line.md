# W4 track B — M5.5b straighten line: Implementation Plan

> **For the track agent:** you are executing this in your **own git worktree**, dispatched as one of
> W4's three parallel tracks. Read `docs/PARALLEL-WORK.md` §2, §3 and §5, then
> `docs/superpowers/specs/2026-09-03-straighten-line-design.md`, before Task 1. Steps use checkbox
> (`- [ ]`) syntax for tracking. **Every decision is settled in the spec — re-open none of them.**

**Goal:** Drag a line along something that should be level and let go; the photo rotates so that line
lands level, and the line stays on screen reading level as proof it worked.

**Architecture:** One pure lib function turns two frame points plus an aspect into a straighten
angle. One new canvas overlay owns the gesture. An armed toggle in the Geometry panel gates it, and
the `Tool::Crop` dispatch gains a third branch. No `Edits` field, no pipeline stage, no golden moves.

**Tech Stack:** Rust, egui 0.35 (bin modules only), `pipeline::perspective::guides` (called, not
modified).

---

## Build environment — do this first, in every shell

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

Without it nothing in this workspace compiles at all.

**Standing conventions, non-negotiable:**
- Commit messages carry **no `Co-Authored-By` trailer**.
- **Never push.** The user pushes.
- `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test --all`
  must all be green before you report done.
- **Use your own uniquely-named log files** — `/tmp/<what>-b.log`, never `/tmp/t.log`. W4's three
  agents share `/tmp`, and on 2026-09-01 two agents both wrote `/tmp/t.log`; one read the other's run
  and saw 901 passed where its own was 1501, which reads as a catastrophic regression and is not
  real.
- **⚠ Check the exit code by redirecting to a file, never through a pipe.** `${PIPESTATUS[0]}` is
  bash and this shell is **zsh**, so it expands to nothing and a check printing `EXIT: ` reads as a
  pass. Do this:
  ```bash
  cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-b.log 2>&1; echo "EXIT=$?"
  ```

---

## File Structure

| File | Responsibility |
| --- | --- |
| **Create** `crates/app/src/straighten_line.rs` | Pure: two frame points + aspect → a straighten angle in degrees. No egui. Lib module, so `pub` is public API and `dead_code` cannot fire. |
| **Create** `crates/app/src/develop/canvas/straighten_overlay.rs` | The gesture and the drawing. Bin module; egui lives here. |
| **Modify** `crates/app/src/lib.rs` | `pub mod straighten_line;` |
| **Modify** `crates/app/src/main.rs` | Two view-state fields at the **end** of `struct Session`, after `visualize` (line ~795). |
| **Modify** `crates/app/src/session_flow.rs:356` | The one production `Session { .. }` literal. |
| **Modify** `crates/app/src/test_app.rs:189` | The test `Session { .. }` literal. |
| **Modify** `crates/app/src/develop/canvas/mod.rs` | `mod straighten_overlay;` and one extra branch in the `Tool::Crop` arm (~line 205). |
| **Modify** `crates/app/src/develop/panel/geometry.rs` | The toggle, immediately **below** the Straighten slider at line 155. |

⚠ **`panel/geometry.rs` (742 lines), `canvas/mod.rs` (992) and `main.rs` are all already over ~600
lines.** §5 requires you to **say so in your report** rather than adding to them silently. You are
not being asked to split them — only to name them.

### ⚠⚠ Surfaces you must NOT touch

`guide_overlay.rs` · `crop_overlay.rs` beyond what the dispatch needs · **writes** to
`Session.guides` or `Session.show_guides` (reads are required — see Task 2) · `edit-model` (no new
`Edits` field) · `pipeline` (you *call* `guides::to_frame`/`from_frame`; you change nothing there) ·
the **53 goldens**, which must stay byte-identical · D's hover read at the end of
`canvas/mod.rs::show` · **C's Auto button in the Guides block at ~line 222 of `panel/geometry.rs`**.

⚠ Your toggle goes at **line 155, beside the Straighten slider** — roughly 70 lines above C's Auto
button. That separation is deliberate placement and is what makes C's rebase clean. Do not put it in
the Guides block.

### Two amendments to §4's ownership table, already agreed

1. **You may write `s.edits.geometry.angle`.** The table bars you from "`Edits`"; that means *add no
   `Edits` field and do not touch the `edit-model` crate*. Writing one existing field is the feature.
2. **You read `show_guides` and `guides`, and write neither.** Mutual exclusion is by gating.

---

## ⚠⚠ The trap that dictates the task boundaries — read before Task 2

**In a binary crate a field that is written but never read is `dead_code`, and
`clippy -D warnings` rejects it.** CLAUDE.md records this trap as having bitten **ten times**. So a
`Session` field cannot land in one commit and gain its reader in the next.

Consequence, and it is why Task 2 is larger than a task usually should be: `straighten_armed`, the
panel toggle that writes it, the canvas branch that reads it, and the overlay all land in **one
commit**. Task 3 then adds `straighten_line` **together with the code that draws it**, for the same
reason.

Plan the commits around that, not around tidiness.

---

## Task 1: `straighten_line.rs` — the angle, pure

**Files:**
- Create: `crates/app/src/straighten_line.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `crates/app/src/lib.rs`, add in alphabetical position among the existing `pub mod` lines:

```rust
pub mod straighten_line;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/app/src/straighten_line.rs` with the tests **only** (no implementation yet) plus a
stub so the file parses:

```rust
//! The straighten angle a drawn line asks for — pure maths, no egui, no canvas.
//!
//! ⚠ Lives in the lib because this is a unit-testable rule: the `crop_tool` / `mask_tool` /
//! `panel_order` pattern. The gesture that produces the two points is in the bin module.

/// Below this, in normalised frame units, a drawn line is treated as a tap and asks for nothing.
pub const MIN_LEN: f32 = 0.01;

/// The straighten angle a line drawn from `from` to `to` asks for, in degrees.
///
/// `from` and `to` are in `[0, 1]` frame coordinates; `aspect` is width over height.
/// `None` for a line too short to have a direction.
pub fn angle_for_line(_from: [f32; 2], _to: [f32; 2], _aspect: f32) -> Option<f32> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3:2 frame, so the aspect correction has something to do.
    const A: f32 = 1.5;

    /// A line already level asks for nothing.
    #[test]
    fn a_level_line_asks_for_no_rotation() {
        let a = angle_for_line([0.2, 0.5], [0.8, 0.5], A).expect("a usable line");
        assert!(a.abs() < 1e-4, "expected ~0, got {a}");
    }

    /// A line already plumb asks for nothing — the vertical family, squared to 90°.
    #[test]
    fn a_plumb_line_asks_for_no_rotation() {
        let a = angle_for_line([0.5, 0.2], [0.5, 0.8], A).expect("a usable line");
        assert!(a.abs() < 1e-4, "expected ~0, got {a}");
    }

    /// ⚠⚠ **The aspect correction, and the test has LEVERAGE.** On a 3:2 frame a line whose
    /// normalised delta is `(0.4, 0.4)` is at 45° in *normalised* space and at
    /// `atan2(0.4, 0.4 * 1.5) = 33.69°` on *screen*. Those two answers differ by 11.3°, far
    /// outside any tolerance, so this test can actually see the correction being dropped.
    ///
    /// ⚠ It is deliberately **diagonal**. M3.1a records the sibling failure:
    /// `hit_testing_the_line_is_aspect_correct` "used a horizontal axis with the probe directly
    /// below it, where the perpendicular distance is purely vertical — it passed with the aspect
    /// correction deleted." An axis-aligned line here has a zero component, so scaling it changes
    /// nothing and the test is blind.
    #[test]
    fn the_angle_is_measured_in_screen_space_not_normalised_space() {
        let a = angle_for_line([0.3, 0.3], [0.7, 0.7], A).expect("a usable line");
        // A horizontal family: level it, so the rotation is minus the screen angle.
        assert!(
            (a + 33.690_1).abs() < 0.01,
            "expected the screen angle -33.69, got {a} (45.0 means the aspect was ignored)"
        );
    }

    /// ⚠⚠ **A line has an orientation, not a direction.** `guides.rs`'s `mean_angle` records what
    /// happens without folding both into one half-plane: "two guides traced in opposite
    /// directions average to roughly a right angle — garbage."
    #[test]
    fn drawing_the_same_line_backwards_gives_the_same_angle() {
        let f = angle_for_line([0.2, 0.4], [0.8, 0.6], A).expect("a usable line");
        let b = angle_for_line([0.8, 0.6], [0.2, 0.4], A).expect("a usable line");
        assert!((f - b).abs() < 1e-4, "forwards {f}, backwards {b}");
    }

    /// The family is inferred from the aspect-corrected delta, `Guide::family`'s rule — so a
    /// steep line is squared to plumb rather than dragged 80° to level.
    #[test]
    fn a_steep_line_is_squared_to_plumb_not_levelled() {
        // Screen delta (0.1 * 1.5, 0.6) — clearly vertical.
        let a = angle_for_line([0.45, 0.2], [0.55, 0.8], A).expect("a usable line");
        assert!(
            a.abs() < 20.0,
            "a vertical-family line should ask for a small rotation, got {a}"
        );
    }

    /// A tap, or a line with no length, must not rotate the photograph by an arbitrary amount.
    /// `guide_overlay.rs` drops a zero-length guide on release for the same reason.
    #[test]
    fn a_degenerate_line_asks_for_nothing() {
        assert_eq!(angle_for_line([0.5, 0.5], [0.5, 0.5], A), None);
        assert_eq!(angle_for_line([0.5, 0.5], [0.502, 0.501], A), None);
    }

    /// The slider's own range. The family rule makes this unreachable in practice, which is
    /// exactly why it is asserted rather than assumed.
    #[test]
    fn the_angle_is_clamped_to_the_sliders_range() {
        for (from, to) in [
            ([0.1, 0.9], [0.9, 0.1]),
            ([0.1, 0.1], [0.9, 0.9]),
            ([0.5, 0.1], [0.51, 0.9]),
        ] {
            let a = angle_for_line(from, to, A).expect("a usable line");
            assert!((-45.0..=45.0).contains(&a), "out of range: {a}");
        }
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p app --lib straighten_line > /tmp/sl-b.log 2>&1; echo "EXIT=$?"; tail -20 /tmp/sl-b.log
```

Expected: **FAIL**, every test panicking at `not implemented`. ⚠ Confirm a `test result:` line
appears at all — a file that failed to *compile* prints none, and this project has recorded that
reading exactly like a pass.

- [ ] **Step 4: Write the implementation**

Replace the stub `angle_for_line` with:

```rust
/// The straighten angle a line drawn from `from` to `to` asks for, in degrees.
///
/// `from` and `to` are in `[0, 1]` frame coordinates; `aspect` is width over height.
/// `None` for a line too short to have a direction.
///
/// ⚠⚠ **Measured in SCREEN space, aspect-corrected.** Normalised frame space is anisotropic —
/// `x` spans the width and `y` the height — so an `atan2` on the raw normalised delta gives an
/// angle nobody drew. This is the fourth appearance of that trap in this codebase:
/// `LinearGradient::coverage`, `mask_tool::hit_test` and `guide_overlay` each had to handle it,
/// while `RadialGradient::coverage` deliberately needs **no** correction for a reason specific to
/// axis-aligned ellipses. Do not assume it cancels here — on a 3:2 frame a normalised 45° line is
/// at 33.69° on screen.
///
/// ⚠⚠ **A line has an orientation, not a direction.** The same line drawn right-to-left must give
/// the same answer, so both directions are folded into one half-plane before measuring. Without
/// that, `guides::solve`'s `mean_angle` records the failure: two lines traced opposite ways
/// "average to roughly a right angle — garbage."
///
/// ⚠ The family is inferred from the corrected delta, reusing
/// [`pipeline::perspective::guides::Guide::family`]'s rule (`|dy| >= |dx|` is a vertical, with an
/// exact 45° landing in `Vertical`): a near-horizontal line is levelled to 0°, a near-vertical one
/// squared to 90°. That is what Lightroom's straighten tool does, and it means you draw along
/// whatever is meant to be straight without telling the app which it is.
pub fn angle_for_line(from: [f32; 2], to: [f32; 2], aspect: f32) -> Option<f32> {
    // ⚠ Guarded, not trusted: a degenerate aspect would make every angle NaN, and a NaN angle
    // compares false against every clamp and would reach `Edits` unchanged.
    if !aspect.is_finite() || aspect <= 0.0 {
        return None;
    }
    let (dxn, dyn_) = (to[0] - from[0], to[1] - from[1]);
    if (dxn * dxn + dyn_ * dyn_).sqrt() < MIN_LEN {
        return None;
    }
    // → screen space.
    let (mut dx, mut dy) = (dxn * aspect, dyn_);
    // One half-plane. The axis folded on is the family's own, so the fold cannot flip the family.
    let vertical = dy.abs() >= dx.abs();
    if (vertical && dy < 0.0) || (!vertical && dx < 0.0) {
        dx = -dx;
        dy = -dy;
    }
    // The rotation that takes this direction to its family's axis.
    let angle = if vertical {
        // Angle from vertical, positive clockwise; zeroing it needs exactly that rotation.
        dx.atan2(dy).to_degrees()
    } else {
        // Angle from horizontal; levelling needs its negation.
        -dy.atan2(dx).to_degrees()
    };
    if !angle.is_finite() {
        return None;
    }
    Some(angle.clamp(-45.0, 45.0))
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app --lib straighten_line > /tmp/sl-b.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/sl-b.log
```

Expected: **PASS**, 7 passed.

- [ ] **Step 6: Sabotage each test and watch it fail**

⚠ **This is not a formality.** This project has recorded **sixteen** instances of a test that could
not detect the bug it was named for, and three of those traps are live in this task. Run each
sabotage, confirm a `test result:` line appears (so you know it compiled), confirm the named test
**failed**, then revert.

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Drop the aspect: `let (mut dx, mut dy) = (dxn, dyn_);` | `the_angle_is_measured_in_screen_space_not_normalised_space` |
| 2 | Remove the half-plane fold (delete the `if (vertical && …)` block) | `drawing_the_same_line_backwards_gives_the_same_angle` |
| 3 | Force the horizontal branch: `let vertical = false;` | `a_plumb_line_asks_for_no_rotation` and `a_steep_line_is_squared_to_plumb_not_levelled` |
| 4 | Drop the length guard (delete the `MIN_LEN` `if`) | `a_degenerate_line_asks_for_nothing` |
| 5 | Drop the clamp: `Some(angle)` | `the_angle_is_clamped_to_the_sliders_range` |
| 6 | Negate the horizontal branch: `dy.atan2(dx).to_degrees()` | `the_angle_is_measured_in_screen_space_not_normalised_space` |

⚠ If a sabotage does **not** fail, that is a finding about the test — fix the test, do not move on.
⚠ If a sabotage does not **compile**, it says nothing at all; that is the recorded trap.

- [ ] **Step 7: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-b.log 2>&1; echo "EXIT=$?"
git add crates/app/src/straighten_line.rs crates/app/src/lib.rs
git commit -m "feat(app): the straighten angle a drawn line asks for"
```

---

## Task 2: arm the gesture and apply the angle

**Files:**
- Create: `crates/app/src/develop/canvas/straighten_overlay.rs`
- Modify: `crates/app/src/main.rs` (one field at the end of `struct Session`)
- Modify: `crates/app/src/session_flow.rs`, `crates/app/src/test_app.rs` (the two `Session` literals)
- Modify: `crates/app/src/develop/canvas/mod.rs`
- Modify: `crates/app/src/develop/panel/geometry.rs`

⚠ **One commit.** The field, its writer and its reader must land together — see the dead-code trap
above.

- [ ] **Step 1: Add the two fields to `Session`**

In `crates/app/src/main.rs`, **after** the `visualize: Option<f32>,` field — which is currently the
last one, around line 795 — add:

```rust
    /// The straighten-line gesture is armed: the next drag under `Tool::Crop` draws a line
    /// instead of moving the crop rectangle.
    ///
    /// ⚠ View state on the session, never in `Edits` — the rule `picking_wb` states: "arming a
    /// tool is not part of the photograph's description, so it must not reach the sidecar or
    /// travel with a paste." `show_guides` and `selected_mask` follow it too. `Session` is
    /// per-photo, so this resets on a switch for free.
    straighten_armed: bool,
    /// The line being drawn, in **raw**-frame normalised coordinates, while the drag is live.
    ///
    /// ⚠⚠ **On `Session`, never a local in `canvas::show`.** M5.3b records the exact failure: the
    /// spot drag state "was a **local** in `canvas::show` at first — reset every frame, so the
    /// release saw no gesture", and "one `eprintln` found it after the lifecycle had read
    /// perfectly three times." It is `Session.spot_drag` now, and this follows it.
    straighten_drag: Option<([f32; 2], [f32; 2])>,
```

⚠ **Append at the end, never insert mid-struct.** §2 rule 3: git merges appends cleanly and
conflicts on mid-struct insertions almost every time — and **D is appending a field here too**.

⚠ Both go on `Session`, not `RustroomApp`, and that is what keeps this task simple: the overlay
already receives `&mut Session`, so nothing has to be hoisted into a local and written back the way
`app_spot_tool` is. The spot tool needs that dance only because its *radius* also lives on
`RustroomApp` for the panel's slider to read; nothing here has a second reader.

- [ ] **Step 2: Fix the two `Session` literals the compiler names**

```bash
cargo build -p app > /tmp/build-b.log 2>&1; echo "EXIT=$?"; grep -A3 "missing field" /tmp/build-b.log
```

Add `straighten_armed: false,` and `straighten_drag: None,` to each literal the compiler names.
There are two: `crates/app/src/session_flow.rs:356` (production) and `crates/app/src/test_app.rs:189`
(test). ⚠ Take the list from the compiler, not from this plan — that is what makes it correct if the
file has moved.

- [ ] **Step 3: Write the overlay**

Create `crates/app/src/develop/canvas/straighten_overlay.rs`:

```rust
//! The straighten-line overlay: drag along something that should be level, and on release the
//! photo rotates so it is.
//!
//! ⚠⚠ **Every coordinate here is against `frame_rect`, never `img_rect`** — the rule the mask and
//! guide overlays both carry, for the same reason: once a region render is on, `img_rect` covers
//! only the rendered *slice*, so a line placed against it drifts with the view while panning.
//! `img_rect` survives only as the clip, widened by `overlay_clip` because `line_segment` centres
//! its stroke on the path and a tight clip shaves the outer half of a halo at the frame edge.

use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::canvas_geom;
use crate::Session;
use eframe::egui;
use rustroom_app::tool::Tool;

/// Draw the straighten line and apply the angle it asks for on release.
///
/// Returns whether a photo edit settled into a new history entry — the same contract
/// `draw_crop_overlay` has, so `show` folds it into `photo_edited` and one gesture stays one
/// Ctrl+Z.
///
/// ⚠ Applies the edit itself rather than reporting it outward, which the guide overlay cannot do:
/// a guide *solve* needs `&mut RustroomApp`, where everything here — `edits`, `request`, `history`
/// — is on `Session`, which this already holds. `pick_white_balance` takes `tool` for the same
/// reason and does the same thing.
pub(super) fn draw_straighten_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    frame_rect: egui::Rect,
    img_rect: egui::Rect,
    tool: Tool,
) -> bool {
    // ⚠ The aspect must come from the FRAME, not from `img_rect`: a region's aspect is not the
    // frame's, and taking it from the painted rect silently skews the angle at any zoom. The
    // guide overlay records the identical trap.
    let aspect = frame_rect.width() / frame_rect.height().max(1.0);

    let norm = |p: egui::Pos2| {
        [
            ((p.x - frame_rect.left()) / frame_rect.width()).clamp(0.0, 1.0),
            ((p.y - frame_rect.top()) / frame_rect.height()).clamp(0.0, 1.0),
        ]
    };
    let to_screen = |n: [f32; 2]| {
        egui::pos2(
            frame_rect.left() + n[0] * frame_rect.width(),
            frame_rect.top() + n[1] * frame_rect.height(),
        )
    };

    if let Some(p) = resp.interact_pointer_pos() {
        let at = norm(p);
        if tool_drag_started(resp) {
            s.straighten_drag = Some((at, at));
        }
        if tool_drag(resp) {
            if let Some((_, to)) = s.straighten_drag.as_mut() {
                *to = at;
                // ⚠⚠ **Deliberately NOT applied here.** The guide overlay records that solving
                // live "was built and rejected in use — reported as 'jiggling too much': the whole
                // photo shifts under the cursor on every pixel of movement." The rubber band is
                // the feedback; the rotation lands on release, so the image is frozen for the
                // whole drag.
            }
        }
    }

    let mut edited = false;
    // ⚠⚠ **Read OUTSIDE the `interact_pointer_pos` block.** On the release frame egui often
    // reports no interact position, and the brush canvas records that nesting a `drag_stopped`
    // there "silently skipped both the render and the history push."
    if resp.drag_stopped() {
        if let Some((from, to)) = s.straighten_drag.take() {
            if let Some(a) = rustroom_app::straighten_line::angle_for_line(from, to, aspect) {
                if s.edits.geometry.angle != a {
                    s.edits.geometry.angle = a;
                    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
                    s.history.push(s.edits.clone());
                    edited = true;
                }
            }
            // ⚠ Disarmed after one line, the eyedropper's recorded rule: a mode that stayed armed
            // would turn every later drag on the canvas — including one meant to move the crop —
            // into a rotation. Disarmed even when the line was degenerate, so a stray click does
            // not leave the tool silently armed.
            s.straighten_armed = false;
        }
    }

    // Draw the band in progress. ⚠ `img_rect` is still the right *clip*: there is no photograph
    // outside it to annotate, and `overlay_clip` widens it because `line_segment` centres its
    // stroke on the path — a tight clip shaves the outer half of a halo at the frame edge.
    if let Some((from, to)) = s.straighten_drag {
        let p = ui.painter_at(canvas_geom::overlay_clip(img_rect));
        let (a, b) = (to_screen(from), to_screen(to));
        // ⚠ Halo then mark, the crop overlay's convention: "a white line is invisible against a
        // bright photograph."
        p.line_segment(
            [a, b],
            egui::Stroke::new(5.0, egui::Color32::from_black_alpha(180)),
        );
        p.line_segment([a, b], egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 80, 80)));
    }
    edited
}
```

- [ ] **Step 4: Wire the dispatch**

In `crates/app/src/develop/canvas/mod.rs`:

Add to the module list at the top, in alphabetical position:

```rust
mod straighten_overlay;
```

Add to the `use` block:

```rust
use straighten_overlay::draw_straighten_overlay;
```

⚠ **Nothing is hoisted and no enum is needed.** Both fields live on `Session`, which the overlay
already holds mutably, and the overlay applies the edit itself — so the `app_spot_tool` /
`guide_drag` dance does not apply here. `draw_crop_overlay` is the contract to match: it returns
`bool` and `show` ORs it into `photo_edited`.

Then replace the `Tool::Crop` arm's body. It currently reads:

```rust
                        Tool::Crop => {
                            if s.show_guides {
                                let d = draw_guide_overlay(ui, s, &resp, frame_rect, img_rect);
                                if d != GuideDrag::None {
                                    guide_drag = d;
                                }
                            } else {
                                photo_edited |=
                                    draw_crop_overlay(ui, s, &resp, img_rect, app.crop_aspect);
                            }
                        }
```

Make it:

```rust
                        Tool::Crop => {
                            // ⚠⚠ **Precedence, and it is a three-way decision now.** Guide Mode
                            // stays first and stays exclusive; the straighten line is next; the
                            // crop rectangle is the default. The Geometry tool gives every drag to
                            // one of the three, so without an order they would fight over one
                            // gesture.
                            //
                            // ⚠ `show_guides` is **read** here and never written by the straighten
                            // gesture. Mutual exclusion is by gating — the panel disables the
                            // toggle while guides are in force — which keeps this track out of
                            // `Session.guides` entirely.
                            if s.show_guides {
                                let d = draw_guide_overlay(ui, s, &resp, frame_rect, img_rect);
                                if d != GuideDrag::None {
                                    guide_drag = d;
                                }
                            } else if s.straighten_armed {
                                photo_edited |= draw_straighten_overlay(
                                    ui, s, &resp, frame_rect, img_rect, tool,
                                );
                            } else {
                                photo_edited |=
                                    draw_crop_overlay(ui, s, &resp, img_rect, app.crop_aspect);
                            }
                        }
```

⚠ There is deliberately **no follow-up block after the session borrow ends**, unlike
`guide_drag == GuideDrag::Settled`. That block exists because `app.solve_guides()` needs
`&mut RustroomApp`; everything this gesture touches is on `Session`, so the overlay finishes the job
and there is nothing to write back.

- [ ] **Step 5: Add the panel toggle**

In `crates/app/src/develop/panel/geometry.rs`, immediately after line 155
(`ed.slider(ui, &mut s.edits.geometry.angle, -45.0..=45.0, "Straighten");`) add:

```rust
    // ⚠⚠ **An armed toggle, not a modifier-drag.** `shell.rs`'s `mode_switch` records the rule:
    // "a feature reachable only by a key nothing on screen mentions is a feature nobody finds."
    // The ⌥-drag in `spot_overlay.rs` is the counter-precedent and is a *refinement* of a gesture
    // that already works unmodified; a straighten line has no unmodified form to refine. It is
    // also Lightroom's own primary affordance — the ruler button inside the crop panel.
    //
    // ⚠ Text, not a glyph. The Guide Mode toggle below is text, and four separate features in
    // this project have picked a glyph that turned out to be absent from the bundled fonts and
    // would have shipped a tofu box. A probe is available if an icon is ever wanted; it is not
    // wanted for a control that sits beside a text-labelled sibling.
    //
    // ⚠⚠ **Gated on Guide Mode being off AND no guides existing, and both halves matter.** A
    // straighten line writes `geometry.angle`, one of the three values a guide solve owns, so
    // drawn guides would be left describing a solve that no longer holds — and the hand-edit
    // check ~50 lines below structurally **cannot** catch it, because it captures `solved_before`
    // within this frame and an angle written by the canvas on the *previous* frame is already in
    // `s.edits`. Gating makes that state unreachable instead of guarding against it. "Clear"
    // leaves the solved sliders alone, so the workflow is: Clear, then draw.
    //
    // ⚠ This reads `show_guides` and `guides` and writes neither — the ownership amendment
    // agreed in W4-0.
    let can_straighten = !s.show_guides && s.guides.is_empty();
    if ui
        .add_enabled(
            can_straighten,
            egui::Button::new("Straighten by drawing a line").selected(s.straighten_armed),
        )
        .on_hover_text(
            "Drag along something that should be level — a horizon, a roofline. The photo \
             rotates when you let go.",
        )
        .on_disabled_hover_text("Clear the perspective guides first.")
        .clicked()
    {
        s.straighten_armed = !s.straighten_armed;
        ed.discrete();
    }
```

⚠ `add_enabled` + `Button::selected`, rather than `toggle_value`, because `toggle_value` has no
disabled form and the gate needs one. The visual result is the same selected chip.

- [ ] **Step 6: Build and run the whole suite**

```bash
cargo build -p app > /tmp/build-b.log 2>&1; echo "EXIT=$?"
cargo test --all > /tmp/test-b.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/test-b.log | tail -5
```

Expected: green, and the test count is **1815 plus your 7 new ones = 1822**. ⚠ Assert the count
rather than "still green": W0 records that a change altering the test count moved behaviour.

- [ ] **Step 7: Write the wiring tests**

⚠⚠ **This is the half this project has repeatedly shipped broken.**
`an_armed_eyedropper_sets_the_white_balance_from_a_click` exists because "the solve has its own unit
tests, but nothing else can see that a click on the canvas ever reaches it."

In `crates/app/src/develop/canvas/mod.rs`'s `mod tests`, add:

```rust
    fn crop_app(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, ctx, 80, 60);
        app.tool = rustroom_app::tool::Tool::Crop;
        app
    }

    /// ⚠⚠ The wiring test: an armed drag must reach `angle_for_line` and land in `Edits`.
    #[test]
    fn an_armed_straighten_drag_sets_the_angle_and_one_history_entry() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        app.session.as_mut().expect("a session").straighten_armed = true;
        let before = app
            .session
            .as_ref()
            .expect("a session")
            .edits
            .geometry
            .angle;
        assert_eq!(before, 0.0, "fixture check: the angle starts neutral");
        // A clearly tilted, clearly diagonal drag.
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
        );
        let s = app.session.as_ref().expect("a session");
        assert!(
            s.edits.geometry.angle.abs() > 1.0,
            "the drag did not reach the angle: {}",
            s.edits.geometry.angle
        );
        // ⚠ `can_undo`, not only `Edits`. The headless-canvas work records that "the edits change
        // during the drag… only the undo push happens on release", so an `Edits` assertion alone
        // cannot see a missing release commit.
        assert!(s.history.can_undo(), "the release pushed no history entry");
        // ⚠ And it disarms, or every later drag would rotate the photo instead of moving the crop.
        assert!(!s.straighten_armed, "the mode stayed armed after one line");
    }

    /// The guard on the guard: with the mode unarmed the drag belongs to the crop rectangle, so
    /// "the angle moved after a drag" means nothing without this.
    #[test]
    fn an_unarmed_drag_leaves_the_straighten_angle_alone() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
        );
        let s = app.session.as_ref().expect("a session");
        assert_eq!(
            s.edits.geometry.angle, 0.0,
            "an unarmed drag rotated the photo"
        );
    }

    /// ⚠ Guide Mode keeps precedence. Armed *and* in Guide Mode, the drag must reach the guides.
    #[test]
    fn guide_mode_still_wins_over_an_armed_straighten_line() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.show_guides = true;
            s.straighten_armed = true;
        }
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(20.0, 60.0),
            centre + egui::vec2(20.0, 60.0),
        );
        let s = app.session.as_ref().expect("a session");
        assert_eq!(s.guides.len(), 1, "the drag did not reach the guide overlay");
    }

    /// ⚠ A non-primary drag must leave it alone. egui reports `dragged()` for every button, so a
    /// bare `dragged()` would let the middle-drag pan binding rotate the photo as it passed over
    /// the canvas — and no test using Primary alone can see that.
    #[test]
    fn a_non_primary_drag_does_not_straighten() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        app.session.as_mut().expect("a session").straighten_armed = true;
        super::test_support::drag_through_with(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
            egui::PointerButton::Middle,
        );
        let s = app.session.as_ref().expect("a session");
        assert_eq!(s.edits.geometry.angle, 0.0, "a middle-drag straightened");
    }
```

In `crates/app/src/develop/panel/geometry.rs`'s `mod tests`, add:

```rust
    /// ⚠ The toggle must be unreachable while a guide solve is in force — the gate that makes
    /// this track's non-interference with `Session.guides` structural rather than remembered.
    #[test]
    fn the_straighten_line_toggle_is_disabled_while_guides_are_in_force() {
        let mut s = session();
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.2, 0.2],
            to: [0.2, 0.8],
        });
        let before = s.straighten_armed;
        let _ = click_in_panel("Straighten by drawing a line", &mut s, false);
        assert_eq!(
            s.straighten_armed, before,
            "the toggle armed itself while guides existed"
        );
    }

    /// And it is reachable with no guides, or the test above would pass against a toggle that is
    /// disabled always — including one that is missing entirely.
    #[test]
    fn the_straighten_line_toggle_arms_with_no_guides() {
        let mut s = session();
        assert!(!s.straighten_armed, "fixture check: it starts unarmed");
        let _ = click_in_panel("Straighten by drawing a line", &mut s, false);
        assert!(s.straighten_armed, "the toggle did not arm");
    }
```

- [ ] **Step 8: Run them, and confirm they fail when sabotaged**

```bash
cargo test -p app > /tmp/test-b.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/test-b.log
```

Sabotages, each reverted after:

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Delete the `drag_stopped` apply block in the overlay | `an_armed_straighten_drag_sets_the_angle_and_one_history_entry` |
| 2 | Remove the `s.history.push` line | the same test, on `can_undo` |
| 3 | Remove `s.straighten_armed = false;` | the same test, on the disarm |
| 4 | Change the dispatch to `else if true` | `an_unarmed_drag_leaves_the_straighten_angle_alone` |
| 5 | Reorder the dispatch to test `straighten_armed` first | `guide_mode_still_wins_over_an_armed_straighten_line` |
| 6 | Use bare `resp.dragged()` / `resp.drag_started()` in the overlay | `a_non_primary_drag_does_not_straighten` |
| 7 | Change the gate to `let can_straighten = true;` | `the_straighten_line_toggle_is_disabled_while_guides_are_in_force` |
| 8 | Move the `drag_stopped` block **inside** the `interact_pointer_pos` block | `an_armed_straighten_drag_…` (this is the recorded brush-canvas bug; if it does **not** fail, say so — it may be inert in the harness, and an inert sabotage is not evidence) |

- [ ] **Step 9: Verify the goldens have not moved**

```bash
cargo test -p pipeline --test golden > /tmp/golden-b.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/golden-b.log
git status --short -- crates/pipeline/tests
```

Expected: 53 golden tests pass and `git status` shows **nothing** under `crates/pipeline/tests`.
⚠ Use `git merge-base main HEAD` rather than `git diff main` if you need to check what you touched —
a plain diff shows another branch's merged changes and reads as a violation. §3 records this trap.

- [ ] **Step 10: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-b.log 2>&1; echo "EXIT=$?"
git add -A
git commit -m "feat(develop): straighten by drawing a line, from an armed toggle"
```

---

## Task 3: keep the line, drawn through the resulting angle

The angle lands, and on a subtle 0.4° correction nothing yet tells you the drag registered. The line
is kept and drawn mapped through the correction, so it reads **level** — which is the feedback.

**Files:**
- Modify: `crates/app/src/main.rs`, `session_flow.rs`, `test_app.rs` (one more `Session` field)
- Modify: `crates/app/src/develop/canvas/straighten_overlay.rs`
- Modify: `crates/app/src/develop/canvas/mod.rs`
- Modify: `crates/app/src/develop/panel/geometry.rs` (clear on disarm)

⚠ One commit again: the field and the code that draws it must land together.

- [ ] **Step 1: Add the field**

In `main.rs`, after `straighten_armed`:

```rust
    /// The last straighten line, on the **raw** frame, drawn mapped through the correction so it
    /// reads level — the feedback that says the gesture worked.
    ///
    /// ⚠ View state, never `Edits`, for `straighten_armed`'s reason. Cleared when the mode is
    /// re-armed, the tool changes, or the photo changes (the last is free — `Session` is
    /// per-photo).
    straighten_line: Option<([f32; 2], [f32; 2])>,
```

Then `cargo build -p app` and add `straighten_line: None,` to each literal the compiler names.

- [ ] **Step 2: Store the line on release, in raw-frame coordinates**

⚠⚠ **The cursor is on the *corrected* frame; the stored line must be on the raw one.** That is
`guide_overlay.rs`'s exact pattern — `let raw = guides::from_frame(screen, &hom, angle, aspect);`
with the comment "The cursor is on the *corrected* frame; a guide lives on the raw one."

In `straighten_overlay.rs`, add to the `use` block:

```rust
use pipeline::perspective::guides;
```

At the top of `draw_straighten_overlay`, after `aspect`, add:

```rust
    // ⚠⚠ **`shown_geo`, not `s.edits`** — the transform the *visible* texture was rendered with.
    // Perspective and geometry are prefix stages, so changing them drops the prefix cache: at
    // native zoom the re-render costs hundreds of milliseconds, and mapping through the
    // *requested* edits put the guides at their new positions over the previous image. Reported
    // there as the line jumping on release when fully zoomed in.
    let (shown_persp, shown_angle) = s.shown_geo;
    let hom = pipeline::perspective::Homography::from_edits(&shown_persp, aspect);
    let angle_rad = shown_angle.to_radians();
```

Change the two coordinate closures so the drag works in **raw** coordinates:

```rust
    let raw_at = |p: egui::Pos2| {
        let screen = [
            ((p.x - frame_rect.left()) / frame_rect.width()).clamp(0.0, 1.0),
            ((p.y - frame_rect.top()) / frame_rect.height()).clamp(0.0, 1.0),
        ];
        guides::from_frame(screen, &hom, angle_rad, aspect)
    };
    let to_screen = |raw: [f32; 2]| {
        let n = guides::to_frame(raw, &hom, angle_rad, aspect);
        egui::pos2(
            frame_rect.left() + n[0] * frame_rect.width(),
            frame_rect.top() + n[1] * frame_rect.height(),
        )
    };
```

…and replace `let at = norm(p);` with `let at = raw_at(p);`, deleting `norm`.

⚠ **The angle is still computed from the SCREEN positions, not the raw ones.** A raw-frame delta is
in the un-corrected frame, and what the user drew is what is on screen. So in the `drag_stopped`
block, map both ends forward before measuring:

```rust
    if resp.drag_stopped() {
        if let Some((from, to)) = s.straighten_drag.take() {
            // ⚠ Measured on the **corrected** frame — that is the line the user drew. The stored
            // endpoints are raw so they survive the rotation that is about to be applied.
            let sf = guides::to_frame(from, &hom, angle_rad, aspect);
            let st = guides::to_frame(to, &hom, angle_rad, aspect);
            if let Some(a) = rustroom_app::straighten_line::angle_for_line(sf, st, aspect) {
                // ⚠ The new angle is **relative** to the one already applied: the line was drawn
                // on a frame that already carries `shown_angle`, so levelling it means rotating
                // by that much *more*. Without the addition, straightening twice would undo the
                // first correction rather than refine it.
                //
                // ⚠ Clamped again: `angle_for_line` bounds the *relative* angle, and a sum of two
                // in-range values is not in range. An out-of-range value is one the Straighten
                // slider could neither show nor return to.
                let next = (shown_angle + a).clamp(-45.0, 45.0);
                if s.edits.geometry.angle != next {
                    s.edits.geometry.angle = next;
                    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
                    s.history.push(s.edits.clone());
                    edited = true;
                }
                s.straighten_line = Some((from, to));
            }
            s.straighten_armed = false;
        }
    }
```

⚠ No new parameter: `straighten_line` is on `Session`, which the overlay already holds mutably.
The signature from Task 2 is unchanged.

- [ ] **Step 3: Draw the settled line too**

Replace the drawing block at the end of the overlay:

```rust
    // Draw the band in progress, else the settled line mapped through the correction — which is
    // what makes it read **level** after release. ⚠ `img_rect` is the right *clip*: there is no
    // photograph outside it to annotate.
    let shown = s.straighten_drag.or(s.straighten_line);
    if let Some((from, to)) = shown {
        let p = ui.painter_at(canvas_geom::overlay_clip(img_rect));
        let (a, b) = (to_screen(from), to_screen(to));
        p.line_segment([a, b], egui::Stroke::new(5.0, egui::Color32::from_black_alpha(180)));
        p.line_segment([a, b], egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 80, 80)));
        for q in [a, b] {
            p.circle_filled(q, 5.0, egui::Color32::from_black_alpha(180));
            p.circle_filled(q, 3.0, egui::Color32::from_rgb(255, 80, 80));
        }
    }
```

- [ ] **Step 4: Clear it at the two places that must**

⚠ Nothing has to be threaded: `straighten_line` is on `Session`, which the overlay already holds
mutably, exactly like `straighten_drag`. The `canvas/mod.rs` dispatch call is unchanged from Task 2.

In `panel/geometry.rs`, inside the toggle's `clicked()` block, add:

```rust
        // Re-arming discards the previous line: it described a correction that is no longer the
        // one being drawn.
        s.straighten_line = None;
```

And in `interaction.rs`'s `select_tool`, beside where `visualize` is cleared, clear it on a tool
change — ⚠ **unconditionally**, not gated on the new tool. M5.3b records that a
`t != Tool::Retouch` guard's sabotage went **uncaught** because the guard was unreachable, and "an
unreachable guard is an untested one."

- [ ] **Step 5: Test that it reads level**

Add to `canvas/mod.rs`'s `mod tests`:

```rust
    /// ⚠⚠ The point of keeping the line: after release it must read **level on screen**, which is
    /// the only feedback that says a subtle correction worked. Asserted through the same forward
    /// map the overlay draws with, so a stored-in-the-wrong-space line cannot pass.
    #[test]
    fn the_settled_straighten_line_reads_level_afterwards() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        app.session.as_mut().expect("a session").straighten_armed = true;
        super::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
        );
        let (from, to) = app
            .session
            .as_ref()
            .expect("a session")
            .straighten_line
            .expect("the line was not kept");
        let s = app.session.as_ref().expect("a session");
        let (persp, angle) = s.shown_geo;
        let aspect = s.fit_image_size.x / s.fit_image_size.y.max(1.0);
        let hom = pipeline::perspective::Homography::from_edits(&persp, aspect);
        let a = pipeline::perspective::guides::to_frame(from, &hom, angle.to_radians(), aspect);
        let b = pipeline::perspective::guides::to_frame(to, &hom, angle.to_radians(), aspect);
        // Levelled: the mapped line's screen-space rise is ~0.
        let rise = (b[1] - a[1]).abs();
        assert!(
            rise < 0.02,
            "the line did not come out level: rise {rise} (from {a:?} to {b:?})"
        );
    }
```

⚠ **This test may legitimately need `shown_geo` to have advanced.** `shown_geo` is written at the
frame-landing site in `shell.rs`, which the canvas harness does not run — so if the assertion fails
because `shown_geo` is still `(default, 0.0)`, that is the *harness*, not the code. In that case
assert against `s.edits.geometry.angle` instead, and **write a comment saying why**, rather than
loosening the tolerance until it passes. CLAUDE.md's rule: when a new test fails, ask whether the
assertion is right before touching the code.

- [ ] **Step 6: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Store the **screen** points instead of the raw ones (`s.straighten_line = Some((sf, st))`) | `the_settled_straighten_line_reads_level_afterwards` |
| 2 | Drop the `shown_angle +` in the settled angle | the same test |
| 3 | Never set `s.straighten_line` | the same test, on the `expect` |

- [ ] **Step 7: Full suite, goldens, commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-b.log 2>&1; echo "EXIT=$?"
cargo test --all > /tmp/test-b.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/test-b.log | tail -5
git status --short -- crates/pipeline/tests
git add -A
git commit -m "feat(develop): the drawn straighten line stays, mapped through the angle"
```

---

## Task 4: verify, run the GUI, report

- [ ] **Step 1: The three gates, each with its exit code read from a file**

```bash
cargo fmt --check > /tmp/fmt-b.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-b.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/test-b.log 2>&1; echo "TEST=$?"; grep "test result" /tmp/test-b.log
```

All three must print `=0`. ⚠ `cargo test` being green is **not** evidence about clippy — this
project has recorded **seven** commits that shipped with clippy failing while tests passed.

- [ ] **Step 2: Confirm the goldens are byte-identical**

```bash
git diff --stat $(git merge-base main HEAD) -- crates/pipeline/tests
```

Expected: **empty**. No track in W4 has any business moving a golden.

- [ ] **Step 3: Diff the ⚠ comments**

```bash
git show $(git merge-base main HEAD):crates/app/src/develop/canvas/mod.rs | grep "⚠" | sort > /tmp/warn-before-b.txt
grep "⚠" crates/app/src/develop/canvas/mod.rs | sort > /tmp/warn-after-b.txt
comm -23 /tmp/warn-before-b.txt /tmp/warn-after-b.txt
```

Expected: **empty** — you added comments, you removed none. W0's rule: a move can drop a recorded
finding silently and no test will notice.

- [ ] **Step 4: ⚠⚠ RUN IT IN THE GUI**

```bash
cargo run --release -p app
```

Every defect in this project's canvas record was found by use and none by a test before it was
reported. Do all of this, on a real photo, and write down what you saw:

1. Open a photo with a visible horizon or roofline. Select the **Geometry** tool (`2`).
2. Click **Straighten by drawing a line**. Drag along the horizon. Release.
   - The photo rotates so the horizon is level.
   - **The drawn line is still there and reads level.**
   - The button is no longer armed.
   - The Straighten slider shows the new angle.
3. **Ctrl+Z once** — the rotation goes, in one step.
4. Arm it again and drag a **vertical** — a door frame. It should square to plumb, not swing 80°.
5. Arm it and drag **right-to-left** along the same horizon. Same result as left-to-right.
6. Arm it and **click without dragging**. Nothing should happen.
7. **Zoom in to 1:1**, then arm and drag. The line must not jump on release.
8. Turn **Guide Mode** on: the straighten button must be **disabled**. Draw two guides, solve, turn
   Guide Mode off — the button is still disabled. Press **Clear** — it becomes available.
9. With the mode **unarmed**, drag on the canvas: the crop rectangle moves, as before.
10. **Middle-drag** while armed: the view pans, nothing rotates.

- [ ] **Step 5: Report**

Your report must say:

- **What you ran in the GUI** and what you saw for each of the ten checks above — not only what
  passed in the suite.
- The **test count before and after** (1815 → your number), not "still green".
- **That the 53 goldens are byte-identical.**
- **The files you touched that are already over ~600 lines** — `canvas/mod.rs` (992),
  `panel/geometry.rs` (742), `main.rs` — per §5.
- **Where `straighten_line` ended up living** (`Session` or `RustroomApp`) and why, if the borrow
  checker moved it.
- **Any sabotage that did not fail**, and any that was **inert**. An inert sabotage is not evidence,
  and saying so is worth more than a clean-looking table.
- **Anything you had to decide that this plan did not settle.** ⚠ Report it, do not silently pick —
  §6 exists because "an agent given an open design choice does not stall; it picks one silently and
  the choice ships."
- ⚠ If the instructions genuinely conflict — for instance if `cargo test --all` cannot be green
  without touching a file you were told not to — **choose the green gate and say so**. W3 has two
  recorded precedents of exactly that being the right call, both flagged rather than hidden.

**Do not push.** The orchestrator merges **B → D → C**; you are first, so you rebase onto nothing.
