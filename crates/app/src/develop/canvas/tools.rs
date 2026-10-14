//! The per-tool overlays: exactly one of them takes the gestures, by the precedence below.

use super::crop_overlay::{self, draw_crop_overlay};
use super::eye_overlay;
use super::eyedropper::{pick_fringe, pick_negative, pick_white_balance};
use super::guide_overlay::{draw_guide_overlay, GuideDrag};
use super::mask_overlay::{draw_mask_overlay, BrushTool};
use super::spot_overlay::{draw_spot_overlay, sampler_for};
use super::straighten_overlay::draw_straighten_overlay;
use super::view::Placement;
use super::{CanvasInputs, CanvasOutcome};
use crate::Session;
use eframe::egui;
use rustroom_app::tool::Tool;

/// The current tool's overlay and gestures.
pub(super) fn dispatch(
    ui: &mut egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    place: &Placement,
    inp: &CanvasInputs,
    out: &mut CanvasOutcome,
) {
    let Placement {
        img_rect,
        frame_rect,
        ..
    } = *place;
    let CanvasInputs {
        tool,
        app_pan_drag,
        app_erase_drag,
        app_brush_size,
        app_brush_hardness,
        app_brush_erase,
        app_brush_auto_mask,
        app_brush_tolerance,
        ref app_tint_frame,
        app_crop_aspect,
        app_brush_flow,
        app_show_depth_map,
        ..
    } = *inp;
    // ⚠ A behaviour change, and an intended one: mask handles and brush
    // painting now belong to the Masks tool rather than to "any mode that is
    // not Geometry". A stray drag in Edit can no longer paint a stroke.
    //
    // ⚠ An exhaustive match, not `_ => {}`: a fifth tool must force a
    // decision about what the canvas does under it.
    match tool {
        Tool::Crop => {
            // ⚠⚠ **Exclusive**: while Guides is on the crop overlay is neither
            // drawn nor interactive. The Geometry tool gives every drag to the
            // crop rectangle, so without this the two would fight over one
            // gesture — and the crop rectangle is invisible here anyway, since
            // the preview resets the crop.
            //
            // ⚠⚠ **Precedence, and it is a three-way decision now.** Guide Mode
            // stays first and stays exclusive; the straighten line is next; the
            // crop rectangle is the default. Without an order the three would
            // fight over one gesture.
            //
            // ⚠ `show_guides` is **read** here and never written by the
            // straighten gesture. Mutual exclusion is by gating — the panel
            // disables the toggle while guides are in force — which keeps that
            // track out of `Session.guides` entirely.
            if s.show_guides {
                // ⚠ Guide Mode shows the muted crop rectangle for the same reason
                // the straighten mode does: the preview strips the crop, so
                // without it there is nothing on screen saying what will actually
                // be kept. ⚠ This is **not** the composition-guide rule below,
                // which stays — those divide the photograph-to-be, and under
                // Guide Mode there is no crop rect *they* may divide.
                crop_overlay::draw_crop_marks(
                    ui,
                    crop_overlay::crop_screen_rect(s, frame_rect),
                    img_rect,
                    crop_overlay::Marks::Muted,
                );
                let d = draw_guide_overlay(ui, s, resp, frame_rect, img_rect);
                if d != GuideDrag::None {
                    out.guide_drag = d;
                }
            } else if s.straighten_armed {
                // ⚠ The crop rectangle stays on screen while the straighten mode
                // is armed, muted and non-interactive. It used to vanish — the
                // dispatch replaced `draw_crop_overlay` entirely, as Guide Mode
                // does — which left the composition guides dividing a rectangle
                // nobody could see. Lightroom keeps it visible too.
                crop_overlay::draw_crop_marks(
                    ui,
                    crop_overlay::crop_screen_rect(s, frame_rect),
                    img_rect,
                    crop_overlay::Marks::Muted,
                );
                out.photo_edited |=
                    draw_straighten_overlay(ui, s, resp, frame_rect, img_rect, tool);
            } else {
                out.photo_edited |=
                    draw_crop_overlay(ui, s, resp, frame_rect, img_rect, app_crop_aspect);
            }
        }
        Tool::Masks => {
            out.photo_edited |= draw_mask_overlay(
                ui,
                s,
                resp,
                img_rect,
                frame_rect,
                BrushTool {
                    size: app_brush_size,
                    hardness: app_brush_hardness,
                    flow: app_brush_flow,
                    erase: app_brush_erase,
                    auto_mask: app_brush_auto_mask,
                    tolerance: app_brush_tolerance,
                    base: app_tint_frame.as_ref(),
                    pan_drag: app_pan_drag,
                    erase_drag: app_erase_drag,
                    show_depth: app_show_depth_map,
                },
                tool,
            );
        }
        Tool::Edit if s.picking_wb => {
            // ⚠⚠ **A second, click-sensing interaction rather than changing the
            // canvas's own sense.** Making `resp` `click_and_drag` does give
            // `clicked()`, and it also changes when egui promotes a press to a
            // drag — which broke both mask-drag tests outright. This one exists
            // only while the eyedropper is armed, so no other gesture can see it.
            //
            // ⚠ Registered *after* the canvas response, which is what makes it
            // win the click: egui breaks a hit-test tie by registration order,
            // taking the last one as the one on top.
            //
            // ⚠ `Crosshair` because egui's `CursorIcon` has no pipette — the
            // set maps onto the OS cursors, and neither winit nor CSS defines
            // one. Crosshair is the conventional stand-in for a colour pick and
            // is what makes the armed state visible where the pointer actually
            // is, rather than only in the panel.
            let pick = ui
                .interact(
                    img_rect,
                    ui.id().with("wb_eyedropper"),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::Crosshair);
            out.photo_edited |= pick_white_balance(s, &pick, frame_rect, tool);
        }
        // ⚠ The white-balance eyedropper's twin (M9.6): its own click-sensing
        // interaction, only while armed, and the same crosshair.
        // ⚠ M10.11a: the film-base picker, the white-balance eyedropper's twin.
        Tool::Edit if s.picking_negative => {
            let pick = ui
                .interact(
                    img_rect,
                    ui.id().with("negative_eyedropper"),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::Crosshair);
            if let Some(m) = pick_negative(s, &pick, frame_rect, tool) {
                out.photo_edited |= m.1;
                out.fringe_status = Some(m.0);
            }
        }
        Tool::Edit if s.picking_fringe => {
            let pick = ui
                .interact(
                    img_rect,
                    ui.id().with("fringe_eyedropper"),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::Crosshair);
            if let Some(m) = pick_fringe(s, &pick, frame_rect, tool) {
                out.photo_edited |= m.1;
                out.fringe_status = Some(m.0);
            }
        }
        Tool::Retouch => {
            // ⚠ Built here rather than inside the overlay so the shared borrow of
            // `s` it holds ends before the overlay takes `&mut s`.
            let sample = sampler_for(s);
            // ⚠ M9.11: patch gestures only in a patch mode. Eyes are drawn in
            // every mode but take gestures only in theirs — one click, one
            // meaning. Patch outlines are not drawn in an eye mode.
            if out.app_eye_tool.active.is_none() {
                out.photo_edited |= draw_spot_overlay(
                    ui,
                    s,
                    resp,
                    img_rect,
                    frame_rect,
                    &mut out.app_spot_tool,
                    &sample,
                );
            }
            out.photo_edited |= eye_overlay::draw_eye_overlay(
                ui,
                s,
                resp,
                img_rect,
                frame_rect,
                &mut out.app_eye_tool,
                &sample,
                &mut out.eye_status,
            );
        }
        Tool::Edit | Tool::Presets => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::canvas::test_support::{crop_app, retouch_app, spots};

    /// How many line segments a frame paints.
    fn segments(app: &mut crate::RustroomApp, ctx: &egui::Context) -> usize {
        fn count(s: &egui::Shape) -> usize {
            match s {
                egui::Shape::LineSegment { .. } => 1,
                egui::Shape::Vec(v) => v.iter().map(count).sum(),
                _ => 0,
            }
        }
        let c = ctx.clone();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::develop::canvas::test_support::screen()),
                ..Default::default()
            },
            |ui| {
                crate::develop::canvas::show(app, ui, &c);
            },
        );
        out.shapes.iter().map(|s| count(&s.shape)).sum()
    }

    /// ⚠⚠ **The crop rectangle stays on screen while the straighten mode is armed.** It used to
    /// vanish outright — the dispatch replaced `draw_crop_overlay` entirely, as Guide Mode does —
    /// which left the composition guides dividing a rectangle nobody could see. Reported in use;
    /// Lightroom keeps it visible too.
    ///
    /// ⚠ Counts the **corner brackets**, which only the crop marks paint: eight segments, and
    /// nothing else in the straighten overlay draws in eights. Asserting merely "more than zero"
    /// would pass against the straighten line's own two segments.
    #[test]
    fn the_crop_rectangle_stays_visible_while_straighten_is_armed() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let unarmed = segments(&mut app, &ctx);
        app.session.as_mut().expect("a session").straighten_armed = true;
        let armed = segments(&mut app, &ctx);
        assert_eq!(
            armed, unarmed,
            "arming the straighten mode changed how much of the crop rectangle is drawn: \
             {unarmed} unarmed, {armed} armed"
        );
        assert!(
            armed >= 8,
            "the corner brackets are not being drawn: {armed}"
        );
    }

    /// ⚠ Guide Mode shows it too, for the same reason: the preview strips the crop, so without the
    /// rectangle there is nothing on screen saying what will be kept.
    ///
    /// ⚠ This is **not** the composition-guide rule, which stays as it was — under Guide Mode no
    /// crop rect is passed to `draw_guides`, because a guide dividing a rectangle nobody keeps is
    /// worse than none. Drawing the crop *rectangle* is a different thing from dividing it.
    #[test]
    fn the_crop_rectangle_stays_visible_in_guide_mode() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let plain = segments(&mut app, &ctx);
        app.session.as_mut().expect("a session").show_guides = true;
        let guiding = segments(&mut app, &ctx);
        assert_eq!(
            guiding, plain,
            "turning on Guide Mode changed how much of the crop rectangle is drawn: \
             {plain} plain, {guiding} in Guide Mode"
        );
        assert!(
            guiding >= 8,
            "the corner brackets are not being drawn: {guiding}"
        );
    }

    /// ⚠ Visible is not interactive. The muted rectangle is context; a drag while armed belongs to
    /// the straighten line, and the crop must not move with it. Without this, "the crop is drawn"
    /// could be satisfied by re-enabling the whole crop overlay.
    #[test]
    fn the_visible_crop_rectangle_is_not_draggable_while_armed() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.straighten_armed = true;
            s.edits.geometry.crop = edit_model::CropRect {
                left: 0.2,
                top: 0.2,
                right: 0.8,
                bottom: 0.8,
            };
        }
        let before = app.session.as_ref().expect("a session").edits.geometry.crop;
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(40.0, 10.0),
            centre + egui::vec2(40.0, 10.0),
        );
        let after = app.session.as_ref().expect("a session").edits.geometry.crop;
        assert_eq!(before, after, "the crop moved during a straighten drag");
    }

    /// ⚠ Guide Mode keeps precedence. Armed *and* in Guide Mode, the drag must reach the guides.
    #[test]
    fn guide_mode_still_wins_over_an_armed_straighten_line() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.show_guides = true;
            s.straighten_armed = true;
        }
        crate::develop::canvas::test_support::drag_through(
            &mut app,
            &ctx,
            centre - egui::vec2(20.0, 60.0),
            centre + egui::vec2(20.0, 60.0),
        );
        let s = app.session.as_ref().expect("a session");
        assert_eq!(
            s.guides.len(),
            1,
            "the drag did not reach the guide overlay"
        );
    }

    /// ⚠⚠ The guard on the guard. Without it, "a spot appeared after a click" means nothing — it
    /// would pass against a version that placed spots under every tool.
    #[test]
    fn a_click_under_another_tool_places_no_spot() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        app.tool = rustroom_app::tool::Tool::Edit;
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        assert!(spots(&app).is_empty(), "Edit placed a retouch spot");
    }
}
