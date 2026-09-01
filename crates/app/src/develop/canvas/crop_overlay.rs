//! The crop-rectangle overlay: the dimmed surround, the border and its corner brackets, and
//! the drag that moves them.

use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::canvas_geom;
use crate::{rects_around, Session};
use eframe::egui;
use rustroom_app::crop_tool;

/// How far each corner bracket reaches along its two edges, in screen points.
///
/// ⚠ Shrinks for a small crop — see `canvas_geom::corner_brackets`.
const CROP_BRACKET_ARM: f32 = 22.0;

/// Draw the crop rectangle overlay and handle dragging (reads/writes `s.edits.geometry.crop`).
///
/// Returns whether a drag settled into a new history entry, so the caller can treat that as
/// a photo edit — it is one of the two places a photo edit is committed.
pub(super) fn draw_crop_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    crop_aspect: Option<f32>,
) -> bool {
    // ⚠ Normalized here, from the *baseline* texture, because that is the same source the
    // Crop panel uses to pick its presets. Two different aspect sources would disagree on any
    // photo whose preview and baseline differ, and the crop would drift as you dragged.
    let img_aspect = s.baseline.size_vec2().x / s.baseline.size_vec2().y;
    let norm_aspect = crop_aspect.map(|a| a / img_aspect);
    let g = s.edits.geometry.crop;
    let to_screen = |nx: f32, ny: f32| {
        egui::pos2(
            img_rect.left() + nx * img_rect.width(),
            img_rect.top() + ny * img_rect.height(),
        )
    };
    let crop_rect =
        egui::Rect::from_min_max(to_screen(g.left, g.top), to_screen(g.right, g.bottom));

    let painter = ui.painter_at(img_rect);
    let dim = egui::Color32::from_black_alpha(140);
    for outside in rects_around(img_rect, crop_rect) {
        painter.rect_filled(outside, egui::CornerRadius::ZERO, dim);
    }
    // ⚠⚠ A **wider** clip for the marks. `line_segment` centres its stroke on the path, so at a
    // full-frame crop — where the boundary *is* the image edge — clipping to the image cuts the
    // outer half off every mark. See `canvas_geom::overlay_clip`. The dimming above keeps the
    // tight clip: there is no photograph outside the image to darken.
    let marks = ui.painter_at(canvas_geom::overlay_clip(img_rect));
    // ⚠⚠ **Every mark is drawn twice — a dark stroke first, the light one over it.** A plain
    // white line is invisible against a bright photograph, and the crop border is *hardest* to
    // see in the case that matters most: at a full-frame crop it lies exactly on the image
    // boundary, so there is nothing to distinguish the control from the edge of the picture.
    // Reported in use as the border and the corner dots being easy to miss entirely.
    let halo = egui::Color32::from_black_alpha(180);
    // ⚠ Red, not white and not the theme accent. White vanishes against a bright photograph, and
    // the accent is user-switchable — including a `Neutral` setting whose whole point is to have
    // no colour, which would defeat this. A fixed red is legible on essentially all photographic
    // content and reads unmistakably as a control rather than part of the picture.
    //
    // ⚠ The crop overlay was already painted by hand with fixed colours and deliberately does not
    // follow the accent; this keeps that, rather than introducing a second rule.
    let mark = egui::Color32::from_rgb(255, 60, 60);
    marks.rect_stroke(
        crop_rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(4.0, halo),
        egui::StrokeKind::Middle,
    );
    marks.rect_stroke(
        crop_rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.5, mark),
        egui::StrokeKind::Middle,
    );

    // ⚠ Corner **brackets**, not dots. They are the conventional crop affordance, they read as
    // "grab me" where a dot reads as decoration, and they are far larger targets — the hit test
    // below uses a 0.03 tolerance, which a 4 px dot badly under-advertised.
    for (a, b) in canvas_geom::corner_brackets(crop_rect, CROP_BRACKET_ARM) {
        marks.line_segment(
            [a, b],
            egui::Stroke::new(canvas_geom::CROP_HALO_WIDTH, halo),
        );
        marks.line_segment([a, b], egui::Stroke::new(3.5, mark));
    }

    if let Some(p) = resp.interact_pointer_pos() {
        let nx = ((p.x - img_rect.left()) / img_rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - img_rect.top()) / img_rect.height()).clamp(0.0, 1.0);
        if tool_drag_started(resp) {
            s.active_handle = crop_tool::hit_test(&s.edits.geometry.crop, [nx, ny], 0.03);
        }
        if tool_drag(resp) {
            if let Some(h) = s.active_handle {
                let d = resp.drag_delta();
                let dx = d.x / img_rect.width();
                let dy = d.y / img_rect.height();
                s.edits.geometry.crop =
                    crop_tool::apply_drag(s.edits.geometry.crop, h, dx, dy, norm_aspect);
            }
        }
        if resp.drag_stopped() {
            s.active_handle = None;
            s.history.push(s.edits.clone());
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{canvas_centre, drag_through, DRAG_PASS};
    use crate::RustroomApp;
    use eframe::egui;

    /// A session in Crop mode with an inset crop, so a drag from the centre has room to move it.
    ///
    /// ⚠ Inset deliberately. On a full-frame crop `Handle::Inside` is clamped at every edge, so
    /// the drag would be a no-op and the test would assert nothing.
    fn with_inset_crop(app: &mut RustroomApp) {
        app.tool = rustroom_app::tool::Tool::Crop;
        let s = app.session.as_mut().unwrap();
        s.edits.geometry.crop = edit_model::CropRect {
            left: 0.2,
            top: 0.2,
            right: 0.8,
            bottom: 0.8,
        };
    }

    #[test]
    fn dragging_inside_the_crop_moves_it() {
        // ⚠ The crop overlay is the same shape as the mask overlay — drag lifecycle, handle
        // hit-test, commit on release — so it inherits the same three failure modes. This is the
        // guard that it actually responds to a drag at all.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);
        let before = app.session.as_ref().unwrap().edits.geometry.crop;

        let centre = canvas_centre(&mut app, &ctx);
        let _ = drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 0.0));

        let after = app.session.as_ref().unwrap().edits.geometry.crop;
        assert_ne!(before, after, "a drag inside the crop must move it");
        assert!(
            after.left > before.left,
            "and it must move the way the pointer went; before={before:?} after={after:?}"
        );
    }

    #[test]
    fn a_crop_drag_commits_one_undo_step_on_release() {
        // ⚠⚠ The crop's version of the `drag_stopped` bug. Asserted on the **history**, not on
        // the crop rect: the rect changes during the *drag*, so comparing it would pass with the
        // release handler disabled — the same trap the mask test fell into first.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);
        assert!(!app.session.as_ref().unwrap().history.can_undo());

        let centre = canvas_centre(&mut app, &ctx);
        let _ = drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 8.0));

        assert!(
            app.session.as_ref().unwrap().history.can_undo(),
            "the release must have committed one undo step"
        );
    }

    #[test]
    fn a_crop_drag_that_grabs_nothing_leaves_the_crop_alone() {
        // ⚠ The guard on the guard, as for masks. A drag well outside the crop rect hits no
        // handle, so the crop must not move — otherwise "the crop changed" says nothing about
        // whether the hit-test works.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);
        let before = app.session.as_ref().unwrap().edits.geometry.crop;

        // The frame's top-left corner: outside a crop inset to 0.2..0.8.
        let _ = drag_through(
            &mut app,
            &ctx,
            egui::pos2(12.0, 58.0),
            egui::pos2(30.0, 62.0),
        );

        assert_eq!(
            before,
            app.session.as_ref().unwrap().edits.geometry.crop,
            "a drag outside the crop must not move it"
        );
    }

    #[test]
    fn the_crop_overlay_does_not_ask_for_a_render() {
        // ⚠⚠ **The asymmetry with masks, pinned so nobody "fixes" it.** A crop drag deliberately
        // issues *no* render: `preview_edits` strips the crop in geometry mode, so the overlay
        // itself is the feedback and the pixels underneath do not change. Copying the mask
        // overlay's `s.request` here would re-render the whole frame on every drag frame for no
        // visible difference — and the reverse mistake, assuming masks needed no request either,
        // is exactly the bug that shipped.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);

        let centre = canvas_centre(&mut app, &ctx);
        let counts = drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 0.0));

        assert_eq!(
            counts[DRAG_PASS],
            counts[DRAG_PASS - 1],
            "a crop drag must not ask for a render; counts={counts:?}"
        );
    }
}
