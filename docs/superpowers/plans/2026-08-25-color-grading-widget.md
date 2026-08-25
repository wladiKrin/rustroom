# Colour grading (M3.3), plan 2 of 2 — the wheel widget

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Colour grading is authorable from the Develop panel — a segmented `Shadows │ Midtones │ Highlights │ Global` selector over a hue/saturation wheel, with numeric Hue / Saturation / Luminance beneath it and Blending / Balance below that.

**Architecture:** A new bin module `develop/panel/color_wheel.rs` holding a `GradeRange` selector enum (mirroring `HslChannel` exactly) and a painter-drawn wheel widget — egui has none. The wheel is a triangle fan with per-vertex colours: centre neutral, rim at constant Oklab chroma. It is drawn inside the existing **Color** section, after HSL.

**Tech Stack:** `eframe`/`egui` 0.35, `egui::Mesh`, existing `color::oklab`.

**Spec:** `docs/superpowers/specs/2026-08-25-color-grading-design.md`
**Predecessor:** `docs/superpowers/plans/2026-08-25-color-grading-headless.md` (complete — the model, stage and goldens exist and are green).

**Build environment** — every `cargo` command needs:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing rules:** no `Co-Authored-By` trailer; never push; sabotage every new test and **confirm the sabotage compiled**; `cargo clippy --all-targets --all-features -- -D warnings` clean, checked by **exit code**, never through `grep`.

⚠⚠ **This is panel work, which CLAUDE.md records as untested territory.** Every defect in the brush canvas — six of them — was in widget wiring, while the pure functions beside it produced none. The mitigation is that this plan drives the widget through **headless `RawInput`**, the seam that already exists (`clicking_a_segment_switches_the_channel`, `tool_drag_for`) and is simply not used enough. Do not skip Task 3.

⚠ **The binary-crate dead-code rule:** `develop/panel/` is a *bin* module, so a `pub` item with no caller fails `-D warnings`. **Tasks 1 and 2 land in one commit.** This has bitten seven times in this project; it is not negotiable.

---

### Task 1 + 2: The `GradeRange` selector and the wheel widget, wired into the panel

Landed as **one commit** — see the dead-code rule above.

**Files:**
- Create: `crates/app/src/develop/panel/color_wheel.rs`
- Modify: `crates/app/src/develop/panel/mod.rs` (declare the module; pass the new state through)
- Modify: `crates/app/src/develop/panel/adjustments.rs` (the Color section)
- Modify: `crates/app/src/main.rs` (the `grade_range` field on `RustroomApp`)

- [ ] **Step 1: Create the module**

Create `crates/app/src/develop/panel/color_wheel.rs`:

```rust
//! The colour-grading range selector and its hue/saturation wheel.
//!
//! ⚠ egui has no colour wheel, so this one is painted: a triangle fan with per-vertex colours,
//! centre neutral and rim at constant Oklab chroma. Out-of-gamut hues clip at the rim, which
//! honestly shows which hues are reachable rather than hiding it behind a gamut-mapped rim.

use crate::develop::panel::Edited;
use edit_model::{ColorGrading, GradeWheel};
use eframe::egui;

/// Segments around the rim. At the panel's 188 px the arc between two is under 7 px, well
/// below the point where the fan reads as a polygon.
const SEGMENTS: usize = 96;
/// Oklab lightness the wheel is drawn at. Mid-light, so both ends of the hue circle stay
/// legible against the dark panel.
const RIM_L: f32 = 0.72;
/// Oklab chroma at the rim. Chosen for a legible wheel, **not** tied to the stage's `SAT_MAX`:
/// the widget's job is to map angle to hue and radius to saturation, and how far a saturation
/// of 100 actually pushes a pixel is the stage's business, not the picker's.
const RIM_CHROMA: f32 = 0.13;

/// Which grading range the wheel is showing.
///
/// ⚠ View state, never part of `Edits` — it must not reach a sidecar, a paste or a preset, for
/// the same reason `HslChannel` and `Session::selected_mask` do not.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GradeRange {
    #[default]
    Shadows,
    Midtones,
    Highlights,
    Global,
}

impl GradeRange {
    pub const ALL: [GradeRange; 4] = [
        Self::Shadows,
        Self::Midtones,
        Self::Highlights,
        Self::Global,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Shadows => "Shad",
            Self::Midtones => "Mid",
            Self::Highlights => "High",
            Self::Global => "Global",
        }
    }

    /// The wheel this range edits.
    pub fn wheel_mut(self, g: &mut ColorGrading) -> &mut GradeWheel {
        match self {
            Self::Shadows => &mut g.shadows,
            Self::Midtones => &mut g.midtones,
            Self::Highlights => &mut g.highlights,
            Self::Global => &mut g.global,
        }
    }
}

/// An sRGB colour for the rim at hue `deg`.
fn rim_color(deg: f32) -> egui::Color32 {
    let h = deg.to_radians();
    let lin = color::oklab::oklab_to_linear_srgb([RIM_L, RIM_CHROMA * h.cos(), RIM_CHROMA * h.sin()]);
    let enc = |c: f32| (color::linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0).round() as u8;
    egui::Color32::from_rgb(enc(lin[0]), enc(lin[1]), enc(lin[2]))
}

/// Screen offset for a point at Oklab hue `deg` and radius fraction `r`.
///
/// ⚠ Screen `y` runs **down** while the hue angle runs counterclockwise from the +a axis, so
/// the `y` term is negated. Getting this wrong mirrors the wheel and every hue reads reflected.
fn offset(deg: f32, r: f32, radius: f32) -> egui::Vec2 {
    let h = deg.to_radians();
    egui::vec2(r * radius * h.cos(), -r * radius * h.sin())
}

/// The wheel. Dragging sets hue from the angle and saturation from the radius.
pub fn show(ui: &mut egui::Ui, wheel: &mut GradeWheel, diameter: f32, ed: &mut Edited) {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(diameter, diameter),
        egui::Sense::click_and_drag(),
    );
    let centre = rect.center();
    let radius = diameter * 0.5;

    if ui.is_rect_visible(rect) {
        let mut mesh = egui::Mesh::default();
        // Vertex 0 is the hub; the rim follows. A fan rather than a texture: no atlas upload,
        // and it stays crisp at any size.
        mesh.colored_vertex(centre, egui::Color32::from_gray(120));
        for i in 0..=SEGMENTS {
            let deg = i as f32 / SEGMENTS as f32 * 360.0;
            mesh.colored_vertex(centre + offset(deg, 1.0, radius), rim_color(deg));
        }
        for i in 0..SEGMENTS {
            mesh.add_triangle(0, 1 + i as u32, 2 + i as u32);
        }
        ui.painter().add(egui::Shape::mesh(mesh));

        // The handle.
        let p = centre + offset(wheel.hue, (wheel.saturation / 100.0).clamp(0.0, 1.0), radius);
        ui.painter()
            .circle_filled(p, 5.0, egui::Color32::from_gray(250));
        ui.painter()
            .circle_filled(p, 3.0, egui::Color32::from_gray(30));
    }

    // ⚠ `dragged_by(Primary)`, never bare `dragged()`. egui reports a drag for **every**
    // button — `interaction.rs` ignores which one when setting `potential_drag_id` — so a bare
    // check would let the middle-drag pan binding rewrite the grade as it passes over.
    let acted = resp.dragged_by(egui::PointerButton::Primary)
        || resp.clicked_by(egui::PointerButton::Primary);
    if acted {
        if let Some(pos) = resp.interact_pointer_pos() {
            let d = pos - centre;
            wheel.saturation = ((d.length() / radius).min(1.0) * 100.0).clamp(0.0, 100.0);
            // Only re-read the hue while off-centre: at the exact centre `atan2(0, 0)` is 0,
            // which would snap the hue to red every time saturation is dragged to zero.
            if d.length() > 1.0 {
                wheel.hue = (-d.y).atan2(d.x).to_degrees().rem_euclid(360.0);
            }
            ed.changed = true;
        }
    }
    ed.settled |= resp.drag_stopped_by(egui::PointerButton::Primary)
        || resp.clicked_by(egui::PointerButton::Primary);
}
```

- [ ] **Step 2: Declare the module**

In `crates/app/src/develop/panel/mod.rs`, beside the existing `mod` declarations:

```rust
pub mod color_wheel;
```

- [ ] **Step 3: Add the app state**

In `crates/app/src/main.rs`, find the `hsl_channel` field on `RustroomApp` and add beside it:

```rust
    /// Which colour-grading range the wheel is showing. View state, like `hsl_channel`.
    grade_range: crate::develop::panel::color_wheel::GradeRange,
```

If `RustroomApp` is constructed with an explicit struct literal rather than `..Default::default()`, initialise it there too with `GradeRange::default()`.

- [ ] **Step 4: Thread it into `adjustments::show`**

In `crates/app/src/develop/panel/mod.rs`, at the `adjustments::show(..)` call, pass `&mut app.grade_range` as a further argument. ⚠ It is a *disjoint field borrow* alongside `&mut app.session`, which is legal — the pattern `hsl_channel` already uses, and the reason this file exists.

In `crates/app/src/develop/panel/adjustments.rs`, add the parameter to `show`:

```rust
pub fn show(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    hsl_channel: &mut HslChannel,
    grade_range: &mut color_wheel::GradeRange,
    ed: &mut Edited,
) {
```

and add `use crate::develop::panel::color_wheel;` to its imports.

- [ ] **Step 5: Draw the block in the Color section**

In `adjustments.rs`, at the end of the `SECTIONS[2]` (Color) body, after the HSL band loop:

```rust
        // ⚠ A hairline and a plain label, not a nested CollapsingHeader — the idiom already
        // used for the Presence block inside Tone. Colour grading belongs to the `Color` edit
        // group, and a header here would put the panel and the Sync dialog out of step.
        ui.separator();
        ui.label("Color Grading");
        ui.horizontal(|ui| {
            for r in color_wheel::GradeRange::ALL {
                ui.selectable_value(grade_range, r, r.label());
            }
        });
        {
            // Scoped so the borrow of `color_grading` ends before Blending and Balance,
            // which need the struct itself.
            let w = grade_range.wheel_mut(&mut edits.color_grading);
            color_wheel::show(ui, w, ui.available_width().min(188.0), ed);
            ed.slider(ui, &mut w.hue, 0.0..=360.0, "Hue");
            ed.slider(ui, &mut w.saturation, 0.0..=100.0, "Saturation");
            ed.slider(ui, &mut w.luminance, -100.0..=100.0, "Luminance");
        }
        ui.separator();
        let g = &mut edits.color_grading;
        ed.slider(ui, &mut g.blending, 0.0..=100.0, "Blending");
        ed.slider(ui, &mut g.balance, -100.0..=100.0, "Balance");
```

⚠ `ui.available_width().min(188.0)` cannot be evaluated while `ui` is borrowed by the call — hoist it to a local on the line above if the borrow checker objects.

- [ ] **Step 6: Build and check the lint**

```bash
cargo build -p app 2>&1 | tail -20
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy exit: $?"
```

Expected: builds, `clippy exit: 0`. Fix anything reported before continuing.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(app): a colour-grading wheel in the Color section"
```

---

### Task 3: Drive the widget headlessly

⚠ This is the task that exists because CLAUDE.md records six canvas bugs, none caught by a test, beside ~30 tests on the pure functions that produced none. The pure parts here (`wheel_mut`, `offset`, `rim_color`) are the easy half; the **drag lifecycle** is where the defects live.

**Files:**
- Modify: `crates/app/src/develop/panel/color_wheel.rs` (add a `tests` module)

- [ ] **Step 1: Write the tests**

Append to `crates/app/src/develop/panel/color_wheel.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Drive one primary-button drag from the wheel's centre to `to`, and report the wheel.
    ///
    /// ⚠ Three passes: lay out with the pointer in place, press, then move-and-hold. egui
    /// resolves interaction against the widgets registered on the *previous* pass, so a
    /// single-pass harness reads zeros and passes vacuously — the bug that made
    /// `a_left_drag_does_not_pan` inert once already.
    fn drag_to(offset_from_centre: egui::Vec2, button: egui::PointerButton) -> GradeWheel {
        let ctx = egui::Context::default();
        let mut wheel = GradeWheel::default();
        let mut ed = Edited::default();
        let mut centre = egui::Pos2::ZERO;
        const D: f32 = 200.0;

        for pass in 0..3 {
            let target = centre + offset_from_centre;
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 600.0),
                )),
                ..Default::default()
            };
            match pass {
                1 => {
                    input.events.push(egui::Event::PointerMoved(centre));
                    input.events.push(egui::Event::PointerButton {
                        pos: centre,
                        button,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                2 => input.events.push(egui::Event::PointerMoved(target)),
                _ => {}
            }
            let _ = ctx.run_ui(input, |ui| {
                let before = ui.cursor().min;
                show(ui, &mut wheel, D, &mut ed);
                centre = before + egui::vec2(D * 0.5, D * 0.5);
            });
        }
        wheel
    }

    #[test]
    fn each_range_edits_its_own_wheel() {
        // Every value distinct, and each read back from the *other three* as well, so a
        // swapped match arm cannot pass by coincidence.
        let mut g = ColorGrading::default();
        GradeRange::Shadows.wheel_mut(&mut g).hue = 11.0;
        GradeRange::Midtones.wheel_mut(&mut g).hue = 22.0;
        GradeRange::Highlights.wheel_mut(&mut g).hue = 33.0;
        GradeRange::Global.wheel_mut(&mut g).hue = 44.0;
        assert_eq!(g.shadows.hue, 11.0, "Shadows wrote the wrong wheel");
        assert_eq!(g.midtones.hue, 22.0, "Midtones wrote the wrong wheel");
        assert_eq!(g.highlights.hue, 33.0, "Highlights wrote the wrong wheel");
        assert_eq!(g.global.hue, 44.0, "Global wrote the wrong wheel");
    }

    #[test]
    fn all_lists_every_variant_with_a_distinct_label() {
        for r in GradeRange::ALL {
            // No wildcard: a fifth variant breaks this and forces a decision.
            match r {
                GradeRange::Shadows
                | GradeRange::Midtones
                | GradeRange::Highlights
                | GradeRange::Global => {}
            }
        }
        let mut seen: Vec<&str> = GradeRange::ALL.iter().map(|r| r.label()).collect();
        assert_eq!(seen.len(), 4);
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 4, "two ranges share a label");
    }

    #[test]
    fn the_angle_convention_matches_oklab() {
        // Right is hue 0, and **up** is hue 90 — screen y runs down, so the widget negates it.
        // A mirrored wheel is the single easiest error to ship here and the hardest to see.
        let r = 10.0;
        let right = offset(0.0, 1.0, r);
        let up = offset(90.0, 1.0, r);
        assert!(right.x > 9.9 && right.y.abs() < 0.1, "hue 0 is not right: {right:?}");
        assert!(up.y < -9.9 && up.x.abs() < 0.1, "hue 90 is not up: {up:?}");
    }

    #[test]
    fn dragging_right_sets_hue_zero_and_dragging_up_sets_ninety() {
        let right = drag_to(egui::vec2(60.0, 0.0), egui::PointerButton::Primary);
        assert!(
            right.hue < 1.0 || right.hue > 359.0,
            "drag right gave hue {}",
            right.hue
        );
        let up = drag_to(egui::vec2(0.0, -60.0), egui::PointerButton::Primary);
        assert!((up.hue - 90.0).abs() < 1.0, "drag up gave hue {}", up.hue);
    }

    #[test]
    fn the_radius_sets_the_saturation_and_clamps_at_the_rim() {
        // 60 px on a 200 px wheel is 60% of the 100 px radius.
        let half = drag_to(egui::vec2(60.0, 0.0), egui::PointerButton::Primary);
        assert!(
            (half.saturation - 60.0).abs() < 2.0,
            "saturation {}",
            half.saturation
        );
        // Well beyond the rim: still exactly 100, never more.
        let past = drag_to(egui::vec2(400.0, 0.0), egui::PointerButton::Primary);
        assert_eq!(past.saturation, 100.0);
    }

    #[test]
    fn a_non_primary_drag_leaves_the_wheel_alone() {
        // ⚠ egui reports `dragged()` for **every** button, so without the `dragged_by`
        // gating the middle-drag pan binding would rewrite the grade whenever it crossed
        // the wheel. Exactly the trap the canvas-navigation work recorded.
        let w = drag_to(egui::vec2(60.0, 0.0), egui::PointerButton::Middle);
        assert_eq!(w.saturation, 0.0, "a middle-drag moved the wheel");
        assert_eq!(w.hue, 0.0);
    }
}
```

- [ ] **Step 2: Run them**

```bash
cargo test -p app --bin Rustroom color_wheel 2>&1 | grep -E "^test |test result:|panicked" | head -12
```

Expected: six PASS. ⚠ If `drag_to` reports zeros, the harness is at fault before the widget is — check that the pointer moves on a pass *before* the one you read, per the recorded `a_left_drag_does_not_pan` failure.

- [ ] **Step 3: Sabotage 1 — un-negate the y axis**

In `offset`, change `-r * radius * h.sin()` to `r * radius * h.sin()`. Confirm it compiles, then run.

Expected: `the_angle_convention_matches_oklab` and `dragging_right_sets_hue_zero_and_dragging_up_sets_ninety` FAIL. Restore.

- [ ] **Step 4: Sabotage 2 — accept any button**

Change `resp.dragged_by(egui::PointerButton::Primary)` to `resp.dragged()`. Confirm it compiles, then run.

Expected: `a_non_primary_drag_leaves_the_wheel_alone` FAILS. Restore.

- [ ] **Step 5: Sabotage 3 — drop the rim clamp**

Change `(d.length() / radius).min(1.0)` to `d.length() / radius`. Confirm it compiles, then run.

Expected: `the_radius_sets_the_saturation_and_clamps_at_the_rim` FAILS. Restore and confirm all six pass.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/develop/panel/color_wheel.rs
git commit -m "test(app): drive the colour-grading wheel headlessly"
```

---

### Task 4: Run it, and look at it

- [ ] **Step 1: Full green check**

```bash
cargo fmt --all --check; echo "fmt exit: $?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep -cE "test result: FAILED"
```

Expected: `fmt exit: 0`, `clippy exit: 0`, `0` failures. Golden count still 43 — this plan touches no pipeline code, so **any golden movement is a bug**.

- [ ] **Step 2: Launch the GUI in the background**

```bash
cargo run --release -p app
```

Run it in the background and tell the user what to look at: open a photo, go to the **Color** section of the Edit panel, and check that (a) the wheel renders as a hue circle rather than tofu or a flat disc, (b) dragging inside it moves the image's colour in the direction the handle points, (c) the four range segments switch which wheel is being edited, and (d) Blending and Balance visibly change how far each range reaches.

- [ ] **Step 3: Record the outcome**

⚠ Whatever comes back from actually using it belongs in CLAUDE.md, especially anything the tests missed — that record is what makes the next panel feature cheaper. Expect defects here: this is the area the project has never had covered.

---

### Task 5: Documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `~/Documents/obsidian/private/opensource_lightroom/03-plan-of-action.md` (mark M3.3 done)

- [ ] **Step 1: Write the CLAUDE.md entry**

Add a section for colour grading in the same style as the ones above it, and update the RESUME block:

- **▶ NEXT ACTION** becomes **M3.2 lens corrections** (a new C dependency, with the build risk libraw carried) or **M3.4 virtual copies** (smallest and most independent) — state both and that M3.3 is closed.
- Test and golden counts: **43 goldens**, and the real total from `cargo test --all`.
- The measured preview cost, from the bench run.
- The findings worth carrying: the LUT collapsing three ranges into one stage; Oklab *L* rather than physical luminance as the range selector, **correcting the note the RESUME block carried**; the softmax max-subtraction being load-bearing; the golden clipping measurement that caught 144 pinned samples at the first ramp top; and that the hue-convention test is the only thing standing between a swapped `a`/`b` and a wrong convention baked silently into the references.
- Add the two new doc paths to the docs list at the bottom.

- [ ] **Step 2: Commit**

```bash
git add -A
git commit -m "docs: record M3.3 colour grading"
```

---

## Definition of done

- The wheel is authorable from the Color section, with a range selector, numeric readouts, Blending and Balance.
- Six headless widget tests, each sabotaged and watched to fail.
- 43 goldens unchanged; `fmt`, `clippy -D warnings` and `test --all` green.
- The GUI launched and looked at, and whatever that surfaced recorded.
- CLAUDE.md updated; the branch left unpushed for the user.
