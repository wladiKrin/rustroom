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
    let lin =
        color::oklab::oklab_to_linear_srgb([RIM_L, RIM_CHROMA * h.cos(), RIM_CHROMA * h.sin()]);
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
        let p = centre
            + offset(
                wheel.hue,
                (wheel.saturation / 100.0).clamp(0.0, 1.0),
                radius,
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive one drag from the wheel's centre out to `offset_from_centre`, and report the wheel.
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
        assert!(
            right.x > 9.9 && right.y.abs() < 0.1,
            "hue 0 is not right: {right:?}"
        );
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
    fn the_handle_returns_to_where_you_dragged() {
        // ⚠ The drag reads an angle with `(-d.y).atan2(d.x)` while the paint writes one with
        // `offset` — two independent computations that must agree. Negate one and not the
        // other and the handle renders mirrored from the point you dragged to, with every
        // *value* still correct. Neither the angle test nor the drag test can see that on its
        // own; only the round trip can. Same shape as the `render_max_dim` and
        // `fit_image_size` disagreements this project has already paid for twice.
        for d in [
            egui::vec2(60.0, 0.0),
            egui::vec2(0.0, -60.0),
            egui::vec2(-40.0, 35.0),
            egui::vec2(30.0, 45.0),
        ] {
            let w = drag_to(d, egui::PointerButton::Primary);
            let back = offset(w.hue, (w.saturation / 100.0).clamp(0.0, 1.0), 100.0);
            assert!(
                (back - d).length() < 1.0,
                "dragged to {d:?} but the handle paints at {back:?}"
            );
        }
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
