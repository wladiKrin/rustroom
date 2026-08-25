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
pub fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    crop_aspect: &mut Option<f32>,
    lens_profile_found: bool,
    ed: &mut Edited,
) {
    // ⚠ The lens **Scale** appears here as well as in the Lens section, and this is the one place
    // it can actually be judged: crop mode previews the *uncropped* frame, which is where the
    // empty corners that pincushion correction leaves are visible at all. In the Edit panel the
    // crop is applied, so if you have cropped even slightly the gaps are already outside it and
    // the slider looks inert.
    //
    // ⚠ Not a second *feature* with two homes — the Presets panel was removed for being that.
    // It is one field, relevant to two different decisions, edited from whichever view can see
    // its effect.
    if lens_profile_found && s.edits.lens_correction.distortion {
        ui.label("Lens");
        ed.slider(ui, &mut s.edits.lens_correction.scale, 0.0..=100.0, "Scale");
        ui.separator();
    }

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
    // ⚠ `selectable_label`, not `button`: the lock is otherwise **invisible state**. Without a
    // shown selection there is no way to tell a locked crop from a free one until you drag a
    // handle and it refuses to move the way you expected.
    ui.horizontal(|ui| {
        for (label, pa) in presets {
            let active = match (pa, *crop_aspect) {
                (None, None) => true,
                (Some(a), Some(cur)) => (a - cur).abs() < 1e-4,
                _ => false,
            };
            if ui.selectable_label(active, label).clicked() {
                *crop_aspect = pa;
                // ⚠ Only a *ratio* reshapes the crop. "Free" merely unlocks, leaving the crop
                // where it is — it used to reset to the full frame, which made unlocking
                // destructive and is now the job of "Reset crop" below.
                if let Some(a) = pa {
                    s.edits.geometry.crop = crop_tool::snap_to_aspect(a / img_aspect);
                    ed.discrete();
                }
            }
        }
    });
    ed.slider(ui, &mut s.edits.geometry.angle, -45.0..=45.0, "Straighten");

    ui.separator();
    // ⚠ Scoped to the Geometry group, the same rule the Edit tool's reset follows: under an
    // exclusive panel each tool resets its own domain and nothing else. Targeted by the named
    // `GEOMETRY_GROUP` rather than a literal index.
    if ui
        .button("Reset crop")
        .on_hover_text("Resets the crop, the straighten angle and any rotation or flip. Leaves the adjustments and any masks alone.")
        .clicked()
    {
        s.edits.merge_from(
            &edit_model::Edits::default(),
            edit_model::EditGroups::only(edit_model::GEOMETRY_GROUP),
        );
        ed.discrete();
    }
}
