//! The geometry-mode block: rotate and flip, aspect presets, Straighten.
//!
//! ⚠ Shown only while the **Crop** tool is selected on the rail. There is no stored
//! `geometry_mode` flag any more — it is derived from `Tool::is_geometry`, so the mode and the
//! selection cannot disagree.

use crate::develop::panel::Edited;
use crate::Session;
use eframe::egui;
use rustroom_app::crop_tool;

/// Draw the geometry controls. The caller decides whether geometry mode is on.
pub fn show(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited) {
    ui.label("Geometry");
    // Quarter turns and mirroring. ⚠ Each button moves the crop by the
    // *same* transform, or the crop lands on different content — see
    // `CropRect::rotated_cw`. Keep the two in step.
    ui.horizontal(|ui| {
        let g = &mut s.edits.geometry;
        let mut turned = false;
        if ui.button("⟲").on_hover_text("Rotate left").clicked() {
            g.orientation.rotate_ccw();
            for _ in 0..3 {
                g.crop = g.crop.rotated_cw();
            }
            turned = true;
        }
        if ui.button("⟳").on_hover_text("Rotate right").clicked() {
            g.orientation.rotate_cw();
            g.crop = g.crop.rotated_cw();
            turned = true;
        }
        if ui.button("⬌").on_hover_text("Flip horizontal").clicked() {
            g.orientation.flip_h();
            g.crop = g.crop.flipped_h();
            turned = true;
        }
        if ui.button("⬍").on_hover_text("Flip vertical").clicked() {
            g.orientation.flip_v();
            g.crop = g.crop.flipped_h();
            g.crop = g.crop.rotated_cw();
            g.crop = g.crop.rotated_cw();
            turned = true;
        }
        if turned {
            // ⚠ `settled` too: a button press is a discrete act, so it
            // is one history entry and one Ctrl+Z — unlike a slider,
            // which pushes only on `drag_stopped`.
            ed.discrete();
        }
    });
    let img_aspect = s.baseline.size_vec2().x / s.baseline.size_vec2().y;
    let presets: [(&str, Option<f32>); 5] = [
        ("Free", None),
        ("1:1", Some(1.0)),
        ("3:2", Some(1.5)),
        ("4:3", Some(4.0 / 3.0)),
        ("16:9", Some(16.0 / 9.0)),
    ];
    ui.horizontal(|ui| {
        for (label, pa) in presets {
            if ui.button(label).clicked() {
                s.edits.geometry.crop = match pa {
                    Some(a) => crop_tool::snap_to_aspect(a / img_aspect),
                    None => edit_model::CropRect::default(),
                };
                ed.discrete();
            }
        }
    });
    ed.slider(ui, &mut s.edits.geometry.angle, -45.0..=45.0, "Straighten");
}
