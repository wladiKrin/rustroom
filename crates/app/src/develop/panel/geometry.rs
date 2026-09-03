//! The geometry-mode block: rotate and flip, aspect presets, Straighten.
//!
//! ⚠ Shown only while the **Crop** tool is selected on the rail. There is no stored
//! `geometry_mode` flag any more — it is derived from `Tool::is_geometry`, so the mode and the
//! selection cannot disagree.

use crate::develop::panel::Edited;
use crate::Session;
use eframe::egui;
use rustroom_app::composition_guides::CompositionGuide;
use rustroom_app::crop_tool;

/// Draw the geometry controls. The caller decides whether geometry mode is on.
pub fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    crop_aspect: &mut Option<f32>,
    composition_guide: &mut CompositionGuide,
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
    // ⚠⚠ Labelled **Composition**, never "Guide" — this panel already has a "Guide Mode" toggle
    // further down for the *perspective* guides you trace onto a building, and two unrelated
    // controls sharing a word in one panel is how someone learns the wrong model. The menu item
    // and the shortcuts dialog say "Composition guide" for the same reason.
    //
    // ⚠⚠ **`selectable_label`s, not a `ComboBox` — and the deciding reason was found by writing
    // the test.** A `ComboBox` allocates raw space and paints its selected text by hand
    // (`combo_box.rs`: `allocate_space` then `ui.interact`), so it **sets no AccessKit label at
    // all** and `test_ui` cannot see it — an *interactive* widget that is nonetheless
    // unfindable, which is a different hole from the documented "a plain `ui.label` produces no
    // node". That would have left the one control this feature adds untestable, so the guard
    // against it shipping absent could not exist. It is also the boundary CLAUDE.md names for
    // adopting `egui_kittest` rather than reimplementing.
    //
    // ⚠ Independently the better control: chips are the idiom the aspect row directly above
    // already uses, every option is visible without opening anything, and picking one is a
    // single click where a combo costs two. `horizontal_wrapped` is what lets the full names
    // stay — a 240 pt panel takes them over two lines rather than forcing a second, shorter
    // vocabulary for the same five things.
    //
    // ⚠ It changes no `Edits`, so it must **not** touch `ed`: the guide is a view preference,
    // not part of the photograph, so it neither re-renders nor pushes a history entry. The
    // caller persists it, guarded on an actual change.
    //
    // ⚠ The shortcut is **hover text on the row**, not a `theme::hint` line under it. This panel
    // already scrolls, and a permanent line of prose for a chord the View menu and the generated
    // Keyboard-shortcuts dialog both list is not worth a row of it — the perspective sliders
    // below carry their explanations the same way.
    ui.label("Composition");
    ui.horizontal_wrapped(|ui| {
        for g in CompositionGuide::ALL {
            if ui
                .selectable_label(*composition_guide == g, g.label())
                .clicked()
            {
                *composition_guide = g;
            }
        }
    })
    .response
    .on_hover_text("Drawn inside the crop rectangle. Press O to cycle.");
    ed.slider(ui, &mut s.edits.geometry.angle, -45.0..=45.0, "Straighten");
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
    //
    // ⚠⚠ **It deliberately does NOT touch `ed`.** Arming changes no `Edits` *and* changes nothing
    // about the render, so `ed.discrete()` here would issue a redundant whole-frame request every
    // time the mode is armed or disarmed. The Composition chips above carry the rule outright —
    // "it changes no `Edits`, so it must not touch `ed`" — and `picking_wb`'s own toggle in
    // `adjustments.rs` follows it. **Guide Mode below is the exception rather than the model:**
    // it re-renders because `show_guides` unpins the fit rung in `canvas::show`, which this does
    // not.
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
    }

    ui.separator();
    // ⚠ Here rather than in the Edit panel because this is the only view that can judge it: crop
    // mode previews the *uncropped* frame, which is where the empty corners a keystone opens are
    // visible at all. The Lens `Scale` slider above is here for the identical reason.
    //
    // ⚠ "Scale" therefore appears twice when a lens profile is present — once under Lens, once
    // here. They are different corrections with different autoscales, disambiguated by their
    // headings, and Lightroom likewise has one in Lens Corrections and one in Transform.
    ui.label("Perspective");
    // ⚠⚠ Captured before the sliders so an *manual* edit to one of the three values a guide solve
    // owns can be told from the solve's own write. The panel is drawn **before** the canvas, so
    // this frame's solve has not happened yet and cannot be mistaken for a hand edit.
    let solved_before = (
        s.edits.perspective.vertical,
        s.edits.perspective.horizontal,
        s.edits.geometry.angle,
    );
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
            "−100 fits the whole corrected image inside the frame; 0 leaves it raw, which \
             clips whatever the correction pushes off the edges; +100 zooms in until no black \
             corners remain.",
        );

    // ⚠⚠ **Editing by hand throws the guides away, because the solve no longer describes what
    // you see.** Only the three values a solve *writes* count — Vertical, Horizontal and
    // Straighten. Aspect and Scale are untouched by a solve, so adjusting them keeps the guides,
    // which is the difference between "you overrode the answer" and "you framed it differently".
    if (
        s.edits.perspective.vertical,
        s.edits.perspective.horizontal,
        s.edits.geometry.angle,
    ) != solved_before
        && !s.guides.is_empty()
    {
        s.guides.clear();
        s.active_guide = None;
    }

    ui.separator();
    ui.label("Guides");
    ui.horizontal(|ui| {
        // ⚠ The toggle deliberately does **not** disable the sliders above. They are the *output*
        // of a solve, and watching them move is what explains where the numbers came from.
        if ui
            .toggle_value(&mut s.show_guides, "Guide Mode")
            .on_hover_text(
                "Draw a line along something that should be vertical or horizontal — a door \
                 frame, a roofline. Two of a kind solve the tilt when you let go. Right-click a \
                 line to remove it.",
            )
            .changed()
        {
            ed.discrete();
        }
        let any = !s.guides.is_empty();
        if ui
            .add_enabled(any, egui::Button::new("Clear"))
            .on_hover_text("Remove all guides. The sliders they produced are left alone.")
            .clicked()
        {
            s.guides.clear();
            s.active_guide = None;
        }
    });
    crate::theme::hint(
        ui,
        "Two lines of a kind set the tilt. Editing Vertical, Horizontal or Straighten by hand \
         clears them.",
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
            show(
                ui,
                s,
                &mut aspect,
                &mut CompositionGuide::Off,
                lens,
                &mut ed,
            );
        });
        ed
    }

    #[test]
    fn every_composition_guide_has_a_chip_in_the_panel() {
        // ⚠⚠ The guard against the control shipping *absent*, which is the failure mode a pure
        // `composition_guides` test structurally cannot see: delete the chips and every one of
        // those tests stays green while the panel offers no way to pick a guide at all.
        //
        // ⚠ Stated over `ALL`, so a sixth guide reachable only from the keyboard is a failure
        // rather than something nobody notices — the rule
        // `every_tool_is_reachable_from_the_photo_menu` already follows.
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut guide = CompositionGuide::Off;
        let labels: Vec<String> = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            let mut ed = Edited::default();
            show(ui, &mut s, &mut aspect, &mut guide, false, &mut ed);
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect();

        for g in CompositionGuide::ALL {
            assert!(
                labels.iter().any(|l| l == g.label()),
                "{g:?} has no chip; drawn: {labels:?}"
            );
        }
    }

    #[test]
    fn clicking_a_composition_chip_selects_that_guide() {
        // ⚠ The other half: a chip that is drawn but wired to nothing looks identical in the
        // test above. ⚠ And it clicks a guide that is **not** the one already set, or a control
        // that ignored the click entirely would pass.
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut guide = CompositionGuide::Off;
        test_ui::click(&ctx, CompositionGuide::Diagonals.label(), |ui| {
            let mut ed = Edited::default();
            show(ui, &mut s, &mut aspect, &mut guide, false, &mut ed);
        });
        assert_eq!(guide, CompositionGuide::Diagonals);
    }

    #[test]
    fn drawing_the_composition_control_is_not_a_photo_edit() {
        // ⚠ The guide is a view preference, so merely drawing its control must leave `Edited`
        // alone — otherwise the panel would re-render the frame and push a history entry on
        // every pass, which reads as the photo changing by itself.
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut guide = CompositionGuide::Grid;
        let mut ed = Edited::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ed = Edited::default();
            show(ui, &mut s, &mut aspect, &mut guide, false, &mut ed);
        });
        assert!(!ed.changed && !ed.settled, "the guide is not an edit");
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
                show(
                    ui,
                    &mut s,
                    &mut aspect,
                    &mut CompositionGuide::Off,
                    lens,
                    &mut ed,
                );
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
            show(
                ui,
                &mut s,
                &mut aspect,
                &mut CompositionGuide::Off,
                true,
                &mut ed,
            );
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
            show(
                ui,
                &mut s,
                &mut aspect,
                &mut CompositionGuide::Off,
                false,
                &mut ed,
            );
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
            show(
                ui,
                &mut s,
                &mut aspect,
                &mut CompositionGuide::Off,
                false,
                &mut ed,
            );
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

    /// ⚠ That the control is drawn at all. A block added to the wrong branch compiles, runs, and
    /// is simply never on screen — which no other test in this file would notice.
    #[test]
    fn the_guides_toggle_and_clear_are_drawn() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let (mut aspect, mut ed) = (None, Edited::default());
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(
                ui,
                &mut s,
                &mut aspect,
                &mut CompositionGuide::Off,
                false,
                &mut ed,
            );
        });
        for want in ["Guide Mode", "Clear"] {
            assert!(
                found.iter().any(|(l, _)| l.contains(want)),
                "no “{want}” control; found {:?}",
                found.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
    }

    /// ⚠⚠ Clearing must leave the **sliders** alone. The guides are the input and the sliders are
    /// the output, so throwing the scaffolding away cannot undo the correction it produced — that
    /// is the whole point of solving *into* the sliders rather than storing the guides.
    #[test]
    fn clearing_the_guides_leaves_the_correction_alone() {
        let mut s = session();
        s.show_guides = true;
        s.edits.perspective.vertical = 42.0;
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.1, 0.1],
            to: [0.2, 0.9],
        });
        click_in_panel("Clear", &mut s, false);
        assert!(s.guides.is_empty(), "the guides should be gone");
        assert_eq!(s.edits.perspective.vertical, 42.0, "the solve must survive");
    }

    /// ⚠⚠ **The correction stays visible while guides are drawn — the live solve depends on it.**
    /// An earlier version reset perspective and straighten here so guides sat on the raw frame,
    /// which removed a feedback loop but made the sub-mode feel inert: you drew, let go, and had
    /// to toggle out to see anything. The loop is handled by *mapping the guides* through the
    /// live transform instead, so both move together.
    #[test]
    fn the_guides_preview_keeps_the_correction_and_only_drops_the_crop() {
        let mut e = edit_model::Edits::default();
        e.perspective.vertical = 40.0;
        e.geometry.angle = 3.0;
        e.geometry.crop = asymmetric();

        let p = crate::preview_edits(&e, rustroom_app::tool::Tool::Crop);
        assert_eq!(
            p.perspective.vertical, 40.0,
            "the correction must stay visible"
        );
        assert_eq!(p.geometry.angle, 3.0);
        assert_eq!(
            p.geometry.crop,
            edit_model::CropRect::default(),
            "only the crop is reset, so the whole frame shows under the overlay"
        );
    }

    /// ⚠⚠ **Editing by hand throws the guides away**, because the solve no longer describes what
    /// is on screen. Driven through the real panel rather than by poking the flag, so the
    /// before/after capture is what is under test.
    #[test]
    fn editing_the_solved_sliders_by_hand_clears_the_guides() {
        let ctx = test_ui::ctx();
        let mut s = session();
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.1, 0.1],
            to: [0.2, 0.9],
        });
        // One frame with nothing touched: the guides must survive it.
        let (mut aspect, mut ed) = (None, Edited::default());
        let _ = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(
                ui,
                &mut s,
                &mut aspect,
                &mut CompositionGuide::Off,
                false,
                &mut ed,
            );
        });
        assert_eq!(s.guides.len(), 1, "an untouched frame must not clear them");

        // ⚠ The edit has to happen **inside** the panel body, through the real widget. Setting
        // the field beforehand does not clear — and must not: the solve itself writes that field
        // from outside, as does a paste, and neither is someone overriding the answer.
        s.edits.perspective.vertical = 60.0;
        // Clicking a slider's centre sets it to the middle of its range, i.e. 0 here.
        click_in_panel("Vertical", &mut s, false);
        assert_ne!(s.edits.perspective.vertical, 60.0, "the click must move it");
        assert!(s.guides.is_empty(), "a hand edit must clear them");
    }

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

    /// ⚠ The other half: Aspect and Scale are **not** written by a solve, so adjusting them is
    /// "you framed it differently", not "you overrode the answer". A rule that cleared on any
    /// perspective change would fail this.
    #[test]
    fn adjusting_aspect_or_scale_keeps_the_guides() {
        let ctx = test_ui::ctx();
        let mut s = session();
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.1, 0.1],
            to: [0.2, 0.9],
        });
        let (mut aspect, mut ed) = (None, Edited::default());
        for change in [10.0f32, -40.0] {
            s.edits.perspective.aspect = change;
            s.edits.perspective.scale = change;
            let _ = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                show(
                    ui,
                    &mut s,
                    &mut aspect,
                    &mut CompositionGuide::Off,
                    false,
                    &mut ed,
                );
            });
        }
        assert_eq!(s.guides.len(), 1, "Aspect and Scale must not clear them");
    }
}
