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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn session() -> Session {
        let ctx = test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.session.take().unwrap()
    }

    /// An off-centre, non-square crop — so every rigid transform of it is distinguishable.
    ///
    /// ⚠ A centred or full-frame crop is invariant under most of these, which would make the
    /// tests below pass with the crop left alone entirely.
    fn asymmetric() -> edit_model::CropRect {
        edit_model::CropRect {
            left: 0.1,
            top: 0.2,
            right: 0.6,
            bottom: 0.5,
        }
    }

    /// ⚠ Approximate, always. `1.0 - (1.0 - 0.1)` is `0.100000024`, so a crop put through any
    /// rigid transform and back does not compare exactly — an earlier version of
    /// `flipping_twice_is_the_identity` demanded `assert_eq!` and failed against correct code. A
    /// millionth of the frame is sub-pixel on any image.
    fn assert_crop_close(a: edit_model::CropRect, b: edit_model::CropRect) {
        for (x, y) in [
            (a.left, b.left),
            (a.top, b.top),
            (a.right, b.right),
            (a.bottom, b.bottom),
        ] {
            assert!((x - y).abs() < 1e-6, "crop drifted: {a:?} vs {b:?}");
        }
    }

    fn click_in_panel(label: &str, s: &mut Session, lens: bool) -> Edited {
        let ctx = test_ui::ctx();
        let mut ed = Edited::default();
        let mut aspect = None;
        test_ui::click(&ctx, label, |ui| {
            ed = Edited::default();
            show(ui, s, &mut aspect, lens, &mut ed);
        });
        ed
    }

    #[test]
    fn every_orientation_button_moves_the_crop_with_it() {
        // ⚠⚠ The invariant this panel's own comment names: each button must move the crop by the
        // *same* transform it applies to the orientation, or the crop lands on different content.
        // A button that turns the image and leaves the crop behind looks right on an uncropped
        // photo and is silently wrong on every cropped one — the same shape as the crop-factor
        // bug that was invisible on 400 files and total on the one that was cropped.
        for label in ["⟲", "⟳", "⬌", "⬍"] {
            let mut s = session();
            s.edits.geometry.crop = asymmetric();
            let before = s.edits.geometry;

            click_in_panel(label, &mut s, false);

            let after = s.edits.geometry;
            assert_ne!(
                before.orientation, after.orientation,
                "{label} did not change the orientation"
            );
            assert_ne!(
                before.crop, after.crop,
                "{label} changed the orientation but left the crop behind"
            );
        }
    }

    #[test]
    fn turning_left_then_right_returns_both_to_where_they_started() {
        // ⚠ Rotating left is implemented as *three* clockwise crop turns, which is the fiddly
        // half. A wrong count leaves the crop somewhere else while the orientation is back.
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        let before = s.edits.geometry;

        click_in_panel("⟳", &mut s, false);
        click_in_panel("⟲", &mut s, false);

        assert_eq!(before.orientation, s.edits.geometry.orientation);
        assert_crop_close(before.crop, s.edits.geometry.crop);
    }

    #[test]
    fn flipping_twice_is_the_identity() {
        for label in ["⬌", "⬍"] {
            let mut s = session();
            s.edits.geometry.crop = asymmetric();
            let before = s.edits.geometry;
            click_in_panel(label, &mut s, false);
            click_in_panel(label, &mut s, false);
            assert_eq!(
                before.orientation, s.edits.geometry.orientation,
                "{label} twice should undo itself"
            );
            assert_crop_close(before.crop, s.edits.geometry.crop);
        }
    }

    #[test]
    fn the_lens_scale_appears_only_when_a_profile_matched_and_distortion_is_on() {
        // ⚠ The slider is here *as well as* in the Lens section because this is the one view
        // where its effect is visible — crop mode previews the uncropped frame. Showing it with
        // no profile would offer a control that cannot do anything.
        let ctx = test_ui::ctx();
        let seen = |lens: bool, distortion: bool| {
            let mut s = session();
            s.edits.lens_correction.distortion = distortion;
            let mut aspect = None;
            let mut ed = Edited::default();
            test_ui::find(&ctx, "Scale", |ui| {
                show(ui, &mut s, &mut aspect, lens, &mut ed);
            })
            .is_some()
        };
        assert!(
            seen(true, true),
            "a matched profile with distortion on shows it"
        );
        assert!(!seen(false, true), "no profile, no slider");
        assert!(!seen(true, false), "distortion off, no slider");
    }
}
