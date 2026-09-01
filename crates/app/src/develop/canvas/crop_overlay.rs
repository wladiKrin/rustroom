//! The crop-rectangle overlay: the dimmed surround, the border and its corner brackets, and
//! the drag that moves them.

use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::canvas_geom;
use crate::{rects_around, Session};
use eframe::egui;
use rustroom_app::composition_guides::{self, CompositionGuide};
use rustroom_app::crop_tool;

/// How far each corner bracket reaches along its two edges, in screen points.
///
/// ⚠ Shrinks for a small crop — see `canvas_geom::corner_brackets`.
const CROP_BRACKET_ARM: f32 = 22.0;

/// The compositional guides' light stroke and the dark halo under it, in screen points.
///
/// ⚠ Thinner than the crop border's 1.5 over 4.0. A guide is a reference you look *through*,
/// and four lines the weight of the control would compete with the photograph rather than help
/// you read it.
///
/// ⚠ Thinned from 1.0/2.5 and greyed from white on 2026-09-01, reported in use as "slightly too
/// thick/white". A guide only has to be *followable*; at white it read as part of the image.
const GUIDE_WIDTH: f32 = 0.8;
const GUIDE_HALO_WIDTH: f32 = 2.0;

/// Draw the compositional guides inside `rect`.
///
/// ⚠⚠ **White over a dark halo, not the crop's red, and not the theme accent.** Three separate
/// reasons, none of them taste:
///
/// * **The halo is what makes it legible on any photograph.** A plain light line vanishes against
///   snow and a plain dark one against a shadow; the crop border already carries exactly this
///   doubling, for exactly this complaint. Drawn dark-then-light, so the light stroke sits in the
///   middle of a darker one and neither ground can swallow it.
/// * **Red is the *control* colour here** — the crop border and its corner brackets — and a guide
///   is not a control: you cannot grab it, and it means something different. Four red lines
///   across the frame would read as four more things to drag.
/// * **The accent is user-switchable, including a `Neutral` setting whose whole point is to have
///   no colour.** The crop overlay is already hand-painted outside the accent system, and this
///   keeps that one rule rather than adding a second.
///
/// ⚠ Both alphas are below the crop border's. A grid drawn at full strength over the picture is
/// the complaint people have about grids, and the guide is meant to be readable *past*.
fn draw_guides(painter: &egui::Painter, rect: egui::Rect, guide: CompositionGuide) {
    if rect.width() <= f32::EPSILON || rect.height() <= f32::EPSILON {
        return;
    }
    let halo = egui::Stroke::new(GUIDE_HALO_WIDTH, egui::Color32::from_black_alpha(110));
    // ⚠ Grey rather than white, and the halo stays: the pair is what makes a guide legible on
    // snow *and* in shadow, which neither alone manages.
    let line = egui::Stroke::new(
        GUIDE_WIDTH,
        egui::Color32::from_rgba_unmultiplied(190, 190, 190, 150),
    );
    // ⚠ The crop's **screen** aspect, which is its pixel aspect: `img_rect` carries the image's
    // own shape, so a fraction of it is a fraction of the real photograph. Taking the aspect
    // from the normalised crop rect instead would call every 3:2 crop square.
    let aspect = rect.width() / rect.height();
    for (a, b) in composition_guides::segments(guide, aspect) {
        let at = |p: [f32; 2]| {
            egui::pos2(
                rect.left() + p[0] * rect.width(),
                rect.top() + p[1] * rect.height(),
            )
        };
        let (a, b) = (at(a), at(b));
        painter.line_segment([a, b], halo);
        painter.line_segment([a, b], line);
    }
}

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
    guide: CompositionGuide,
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
    // ⚠ Drawn **before** the border and the brackets, so the controls sit on top of the guides
    // rather than under them. It also puts the guides in the wider `marks` clip: a diagonal
    // reaches the crop's corners, and at a full-frame crop those are the image's own edges,
    // where a tight clip would shave half the stroke off — the bug `overlay_clip` exists for.
    draw_guides(&marks, crop_rect, guide);
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
    use super::super::test_support::{canvas_centre, drag_through, screen, DRAG_PASS};
    use super::*;
    use crate::RustroomApp;
    use eframe::egui;

    /// How many `LineSegment`s one frame of the canvas paints.
    ///
    /// ⚠ Recursive, because a painter's shapes can arrive nested inside a `Shape::Vec`.
    fn line_segments(app: &mut RustroomApp, ctx: &egui::Context) -> usize {
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
                screen_rect: Some(screen()),
                ..Default::default()
            },
            |ui| {
                super::super::show(app, ui, &c);
            },
        );
        out.shapes.iter().map(|s| count(&s.shape)).sum()
    }

    /// How many more line segments `guide` paints than [`CompositionGuide::Off`] does.
    ///
    /// ⚠⚠ A **difference**, never an absolute count. The crop border's corner brackets are line
    /// segments too, so an absolute number would be a magic constant that any unrelated change
    /// to the overlay would move — and worse, it would still pass if the guides vanished and
    /// something else grew by the same amount.
    fn extra_segments(guide: CompositionGuide) -> usize {
        extra_segments_for(
            guide,
            edit_model::CropRect {
                left: 0.2,
                top: 0.2,
                right: 0.8,
                bottom: 0.8,
            },
        )
    }

    /// The same, over a crop of a chosen shape.
    fn extra_segments_for(guide: CompositionGuide, crop: edit_model::CropRect) -> usize {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.tool = rustroom_app::tool::Tool::Crop;
        app.session.as_mut().unwrap().edits.geometry.crop = crop;

        app.settings.composition_guide = CompositionGuide::Off;
        let off = line_segments(&mut app, &ctx);
        app.settings.composition_guide = guide;
        let on = line_segments(&mut app, &ctx);
        assert!(
            on >= off,
            "{guide:?} painted fewer segments than Off: {on} vs {off}"
        );
        on - off
    }

    #[test]
    fn a_thirds_guide_reaches_the_painter() {
        // ⚠⚠ The one test that proves the geometry is actually *drawn*. Every other guide test
        // in this feature lives in `composition_guides`, where no window exists — so with the
        // `draw_guides` call deleted they all stay green and the feature is silently absent.
        // That is the shape of the sabotage this exists to fail.
        //
        // Four divisions, each stroked twice (halo, then line).
        assert_eq!(extra_segments(CompositionGuide::Thirds), 8);
    }

    #[test]
    fn each_guide_paints_its_own_number_of_lines() {
        // ⚠ The discriminating half: "more segments than Off" would pass for a match arm that
        // returned thirds for every guide. Diagonals is two lines where thirds is four, and the
        // grid is more than either — so any two arms collapsing into one fails here.
        let thirds = extra_segments(CompositionGuide::Thirds);
        let diagonals = extra_segments(CompositionGuide::Diagonals);
        let grid = extra_segments(CompositionGuide::Grid);
        assert_eq!(diagonals, 4, "two diagonals, stroked twice");
        assert!(
            grid > thirds,
            "the grid must be denser than thirds: {grid} vs {thirds}"
        );
    }

    #[test]
    fn the_grid_follows_the_crops_own_shape() {
        // ⚠⚠ The only test that can see the aspect being threaded through. The grid's cells are
        // square, so a wide crop takes *more* columns than a square one — hardcode the aspect to
        // 1.0 in `draw_guides` and the two shapes produce the same figure, which every other
        // test here is blind to. The image is 80×40, so a crop spanning 0.6 of both axes is 2:1
        // while one spanning 0.25 of the width and 0.5 of the height is square.
        let square = extra_segments_for(
            CompositionGuide::Grid,
            edit_model::CropRect {
                left: 0.25,
                top: 0.0,
                right: 0.5,
                bottom: 0.5,
            },
        );
        let wide = extra_segments_for(
            CompositionGuide::Grid,
            edit_model::CropRect {
                left: 0.2,
                top: 0.2,
                right: 0.8,
                bottom: 0.8,
            },
        );
        assert!(
            wide > square,
            "a 2:1 crop must take more grid lines than a square one: {wide} vs {square}"
        );
    }

    #[test]
    fn a_degenerate_crop_paints_no_guide_rather_than_panicking() {
        // ⚠ The crop rect is measured from a live layout, so a frame in which it has collapsed
        // is reachable — and `draw_guides` divides by its height to get the aspect.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.tool = rustroom_app::tool::Tool::Crop;
        let s = app.session.as_mut().unwrap();
        s.edits.geometry.crop = edit_model::CropRect {
            left: 0.5,
            top: 0.5,
            right: 0.5,
            bottom: 0.5,
        };
        app.settings.composition_guide = CompositionGuide::Grid;
        let _ = line_segments(&mut app, &ctx);
    }

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
