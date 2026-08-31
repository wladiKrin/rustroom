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
        // ⚠ "Lens scale" here, plain "Scale" in the Edit panel's Lens section — the same field,
        // labelled to disambiguate only in the one view where a *second* Scale slider (the
        // perspective one below) is on screen at the same time. Two adjacent sliders both called
        // "Scale" is ambiguous once the headings have scrolled out of view.
        ed.slider(
            ui,
            &mut s.edits.lens_correction.scale,
            0.0..=100.0,
            "Lens scale",
        );
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
    // ⚠ Here rather than in the Edit panel because this is the only view that can judge it: crop
    // mode previews the *uncropped* frame, which is where the empty corners a keystone opens are
    // visible at all. The Lens `Scale` slider above is here for the identical reason.
    //
    // ⚠ "Scale" therefore appears twice when a lens profile is present — once under Lens, once
    // here. They are different corrections with different autoscales, disambiguated by their
    // headings, and Lightroom likewise has one in Lens Corrections and one in Transform.
    ui.label("Perspective");
    let p = &mut s.edits.perspective;
    ed.slider(ui, &mut p.vertical, -100.0..=100.0, "Vertical")
        .on_hover_text(
            "Keystone about the horizontal axis — for a subject leaning back or forward.",
        );
    ed.slider(ui, &mut p.horizontal, -100.0..=100.0, "Horizontal")
        .on_hover_text("Keystone about the vertical axis — for a subject turned left or right.");
    // ⚠⚠ The glyphs are the whole point of this label. A single bipolar Aspect slider was
    // reported as confusing precisely because "Aspect" says nothing about which end does what,
    // and `⬍ ⬌` names both ends at a glance — the same two glyphs the flip buttons above use, so
    // they already mean "vertical" and "horizontal" in this panel.
    //
    // ⚠ It stays ONE slider rather than two: the two directions are `diag(s, 1/s)`, inverses of
    // one quantity rather than independent axes. Two sliders would be `diag(a, b)`, which factors
    // into a uniform scale × an aspect ratio — so the extra freedom **is** the Scale slider below,
    // and the same look would be reachable two ways. `Orientation`'s single `mirror` flag exists
    // for exactly that reason.
    //
    // ⚠ `↔` and `↕` are ABSENT from every bundled font and would render as tofu boxes — probed,
    // not assumed. `⬌ ⬍ ‹ › « »` are present.
    ed.slider(ui, &mut p.aspect, -100.0..=100.0, "Aspect ⬍ ⬌")
        .on_hover_text("Stretches the frame: negative taller ⬍, positive wider ⬌. Counteracts the squashing a keystone correction causes.");
    ed.slider(ui, &mut p.scale, -100.0..=100.0, "Scale")
        .on_hover_text(
            "Zooms in to hide the empty corners the correction opens. 0 shows the whole frame.",
        );

    ui.separator();
    // ⚠ Scoped to the Geometry group, the same rule the Edit tool's reset follows: under an
    // exclusive panel each tool resets its own domain and nothing else. Targeted by the named
    // `GEOMETRY_GROUP` rather than a literal index.
    //
    // ⚠⚠ **Anything added to `GEOMETRY_GROUP` must be named in the hover text below**, or this
    // button silently resets something it does not mention. Perspective joining the group is
    // exactly what made the previous text ("the crop, the straighten angle and any rotation or
    // flip") wrong — the same failure as the "Reset adjustments" text that enumerated five panel
    // sections and went stale when a sixth arrived.
    if ui
        .button("Reset geometry")
        .on_hover_text("Resets the crop, straighten, rotation, flip and perspective. Leaves the adjustments and any masks alone.")
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
            test_ui::find(&ctx, "Lens scale", |ui| {
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

    /// ⚠⚠ The two Scale sliders must stay distinguishable. They are different corrections with
    /// different autoscales, and once the "Lens" and "Perspective" headings have scrolled out of
    /// view two sliders both called "Scale" say nothing about which is which.
    #[test]
    fn the_lens_scale_and_the_perspective_scale_have_different_labels() {
        let ctx = test_ui::ctx();
        let mut s = session();
        s.edits.lens_correction.distortion = true;
        let mut aspect = None;
        let mut ed = Edited::default();
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(ui, &mut s, &mut aspect, true, &mut ed);
        });
        let scales: Vec<&String> = found
            .iter()
            .map(|(l, _)| l)
            .filter(|l| l.contains("cale"))
            .collect();
        assert_eq!(scales.len(), 2, "expected exactly two, got {scales:?}");
        assert_ne!(scales[0], scales[1], "both are called the same thing");
    }

    /// ⚠ That the sliders are actually drawn. A section added to the wrong branch compiles, runs,
    /// and is simply never on screen — which no other test in this file would notice.
    #[test]
    fn the_perspective_sliders_are_drawn() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut ed = Edited::default();
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(ui, &mut s, &mut aspect, false, &mut ed);
        });
        for want in ["Vertical", "Horizontal", "Aspect", "Scale"] {
            assert!(
                found.iter().any(|(label, _)| label.contains(want)),
                "no “{want}” slider in the Geometry tool; found {:?}",
                found.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
    }

    /// ⚠ The rail must say what the tool now does. Asserted on `Tool::label` rather than by
    /// finding the widget, because the rail is drawn by a different module entirely.
    #[test]
    fn the_rail_calls_the_crop_tool_geometry() {
        assert_eq!(rustroom_app::tool::Tool::Crop.label(), "Geometry");
    }

    /// ⚠⚠ The reset button covers the whole Geometry group, perspective included. Without this
    /// the button's promise is only checked by reading its hover text.
    #[test]
    fn resetting_the_geometry_clears_the_perspective_too() {
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        s.edits.perspective = edit_model::Perspective {
            vertical: 40.0,
            ..Default::default()
        };
        click_in_panel("Reset geometry", &mut s, false);
        assert_eq!(s.edits.perspective, edit_model::Perspective::default());
        assert_eq!(s.edits.geometry, edit_model::Geometry::default());
    }

    /// ⚠⚠ `↔` and `↕` — the obvious choice — are **absent from every bundled font** and would
    /// have shipped as tofu boxes, invisible to every other test. Probed before choosing, which
    /// is the fourth time that has paid off here after the rail's four glyphs, the mark badges
    /// and the keyword checkmarks.
    #[test]
    fn every_perspective_glyph_is_renderable() {
        let ctx = test_ui::ctx();
        // ⚠ Both lines are load-bearing, and leaving them out is why this test failed first.
        // Without `install_fonts` it asks about egui's *defaults*, which production no longer
        // uses; and `has_glyph` needs a frame to have run, because a fresh context has not built
        // its atlas and answers about nothing.
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for g in ['⬍', '⬌'] {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, g)),
                "{g} (U+{:04X}) is not in any bundled font and would render as a tofu box",
                g as u32
            );
        }
    }

    /// ⚠ The Aspect label must name both directions. "Aspect" alone was reported as confusing
    /// precisely because it says nothing about which end does what.
    #[test]
    fn the_aspect_slider_names_both_directions() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut ed = Edited::default();
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(ui, &mut s, &mut aspect, false, &mut ed);
        });
        let label = found
            .iter()
            .map(|(l, _)| l)
            .find(|l| l.contains("Aspect"))
            .expect("no Aspect slider");
        assert!(
            label.contains('⬍') && label.contains('⬌'),
            "the Aspect label names neither direction: {label:?}"
        );
    }
}
