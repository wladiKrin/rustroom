//! The per-tool overlays: exactly one of them takes the gestures, by the precedence below.

use super::crop_overlay::{self, draw_crop_overlay};
use super::eye_overlay;
use super::eyedropper::{pick_fringe, pick_white_balance};
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
