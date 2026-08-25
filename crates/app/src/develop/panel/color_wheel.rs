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
