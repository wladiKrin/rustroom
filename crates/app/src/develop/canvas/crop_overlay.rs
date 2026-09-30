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
const CROP_BRACKET_ARM: f32 = 26.0;
/// The length of the bar at the middle of each crop edge.
const CROP_EDGE_BAR: f32 = 22.0;

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
pub(super) fn draw_guides(painter: &egui::Painter, rect: egui::Rect, guide: CompositionGuide) {
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
    // ⚠ The crop's **screen** aspect, which is its pixel aspect: the rect handed in is derived from
    // `frame_rect`, which carries the whole frame's shape, so a fraction of it is a fraction of the
    // real photograph — true whether the frame was rendered whole or as a region. Taking the aspect
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

/// The crop rectangle's position on screen.
///
/// ⚠ Extracted so `canvas::show` places the guides against the **same** rect the overlay draws,
/// rather than recomputing it. Two computations of one rectangle is exactly how the crop border
/// and the guides would come to disagree by a pixel, with nothing to say which was right.
pub(super) fn crop_screen_rect(s: &Session, frame_rect: egui::Rect) -> egui::Rect {
    let g = s.edits.geometry.crop;
    // ⚠⚠ **`frame_rect`, not `img_rect`.** A crop is normalised against the whole frame, so it must
    // be placed against the whole frame's on-screen rect. Once a region render is on, `img_rect`
    // covers only the rendered *slice* — the mask overlay's recorded drift, "appearing to follow
    // the view as you pan".
    let to_screen = |nx: f32, ny: f32| {
        egui::pos2(
            frame_rect.left() + nx * frame_rect.width(),
            frame_rect.top() + ny * frame_rect.height(),
        )
    };
    egui::Rect::from_min_max(to_screen(g.left, g.top), to_screen(g.right, g.bottom))
}

/// How prominently the crop marks are drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Marks {
    /// The Crop tool itself: full strength, with the surround dimmed.
    Active,
    /// Context under another gesture — currently the straighten line. Thinner and dimmer, and
    /// **no dimming of the surround**.
    ///
    /// ⚠ The surround is deliberately left undimmed rather than merely lightened: a straighten
    /// line is very often traced along a horizon that runs *outside* the crop, and darkening
    /// exactly that region fights the gesture the marks are there to support. The composition
    /// guides make the same call under Guide Mode, for the same kind of reason.
    Muted,
}

/// Draw the crop rectangle: the dimmed surround, the border and the corner brackets.
///
/// ⚠ Split out so the straighten overlay can show the framing without inheriting the crop tool's
/// *interaction* — one drawing rule with two callers, rather than a second copy that would drift
/// in colour and weight. `test_support.rs` states the general form of that rule.
pub(super) fn draw_crop_marks(
    ui: &egui::Ui,
    crop_rect: egui::Rect,
    img_rect: egui::Rect,
    how: Marks,
) {
    if how == Marks::Active {
        let painter = ui.painter_at(img_rect);
        let dim = egui::Color32::from_black_alpha(140);
        for outside in rects_around(img_rect, crop_rect) {
            painter.rect_filled(outside, egui::CornerRadius::ZERO, dim);
        }
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
    // ⚠ Muted is thinner *and* dimmer, the treatment the composition guides already use when
    // they are context rather than the tool in hand (0.8 over a 2.0 halo, 190 at alpha 150).
    let muted = how == Marks::Muted;
    let halo = egui::Color32::from_black_alpha(if muted { 110 } else { 180 });
    // ⚠ A light neutral over a dark halo, not red and not the theme accent (the author's call,
    // 2026-09-28, closing P-07: red was the one loud colour on an otherwise neutral surround).
    // The halo is what keeps a light line visible on a bright photograph — see above — and the
    // accent is user-switchable, including a `Neutral` setting whose whole point is to have no
    // colour. The **handles** carry the affordance instead: corner brackets and edge bars, bold
    // enough to read as "grab me" at a glance.
    let mark = if muted {
        egui::Color32::from_rgba_unmultiplied(235, 235, 235, 150)
    } else {
        egui::Color32::from_rgb(235, 235, 235)
    };
    let (halo_w, mark_w) = if muted { (2.0, 0.8) } else { (4.0, 1.5) };
    // ⚠⚠ The guides are **no longer drawn here.** They moved to `canvas::show` so every tool
    // gets them, not only Crop — the View menu entry and the `O` key did nothing in the other
    // three. Crop still passes the **crop rect**, because the preview strips the crop and that
    // rect is the photograph-to-be.
    marks.rect_stroke(
        crop_rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(halo_w, halo),
        egui::StrokeKind::Middle,
    );
    marks.rect_stroke(
        crop_rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(mark_w, mark),
        egui::StrokeKind::Middle,
    );

    // ⚠ Corner **brackets**, not dots. They are the conventional crop affordance, they read as
    // "grab me" where a dot reads as decoration, and they are far larger targets — the hit test
    // below uses a 0.03 tolerance, which a 4 px dot badly under-advertised.
    for (a, b) in canvas_geom::corner_brackets(crop_rect, CROP_BRACKET_ARM) {
        marks.line_segment(
            [a, b],
            egui::Stroke::new(
                if muted {
                    3.0
                } else {
                    canvas_geom::CROP_HALO_WIDTH + 3.0
                },
                halo,
            ),
        );
        marks.line_segment(
            [a, b],
            egui::Stroke::new(if muted { 1.6 } else { 5.0 }, mark),
        );
    }
    // ⚠ Edge bars too: `Handle::{Left, Right, Top, Bottom}` have always been draggable and
    // nothing said so. Drawn in both states, like the brackets — the muted treatment is the
    // same marks, thinner and dimmer, so guide mode and the straighten tool show the same crop.
    for (a, b) in canvas_geom::edge_bars(crop_rect, CROP_EDGE_BAR) {
        marks.line_segment(
            [a, b],
            egui::Stroke::new(
                if muted {
                    2.5
                } else {
                    canvas_geom::CROP_HALO_WIDTH + 1.0
                },
                halo,
            ),
        );
        marks.line_segment(
            [a, b],
            egui::Stroke::new(if muted { 1.2 } else { 3.5 }, mark),
        );
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
    frame_rect: egui::Rect,
    img_rect: egui::Rect,
    crop_aspect: Option<f32>,
) -> bool {
    // ⚠ Normalized here, from the *baseline* texture, because that is the same source the
    // Crop panel uses to pick its presets. Two different aspect sources would disagree on any
    // photo whose preview and baseline differ, and the crop would drift as you dragged.
    let img_aspect = s.baseline.size_vec2().x / s.baseline.size_vec2().y;
    let norm_aspect = crop_aspect.map(|a| a / img_aspect);
    // ⚠ Both the crop border and the composition guides are placed from this one function, so
    // they cannot drift apart by a pixel.
    let crop_rect = crop_screen_rect(s, frame_rect);

    draw_crop_marks(ui, crop_rect, img_rect, Marks::Active);

    if let Some(p) = resp.interact_pointer_pos() {
        // ⚠⚠ **The frame, not the painted slice — head 2 of M3.1a's bug.** The crop is stored in
        // frame-normalised coordinates, so a pointer position must be normalised the same way or
        // the hit-test grabs the wrong handle at any zoom. Silent: it feels like the handles have
        // moved.
        let nx = ((p.x - frame_rect.left()) / frame_rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - frame_rect.top()) / frame_rect.height()).clamp(0.0, 1.0);
        if tool_drag_started(resp) {
            s.active_handle = crop_tool::hit_test(&s.edits.geometry.crop, [nx, ny], 0.03);
        }
        if tool_drag(resp) {
            if let Some(h) = s.active_handle {
                let d = resp.drag_delta();
                // ⚠⚠ **Head 3, and the one M3.1a records as invisible until someone zoomed in:**
                // "the drag deltas were divided by the region's dimensions, so a handle dragged
                // while zoomed moved by the wrong amount." `frame_rect`'s on-screen width is
                // `full * pk * pscale` and `pscale` is `fit * zoom / pk`, so the render resolution
                // cancels — which is exactly the property
                // `a_crop_drag_moves_by_the_frame_not_the_rendered_slice` asserts.
                let dx = d.x / frame_rect.width();
                let dy = d.y / frame_rect.height();
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

    /// Seed the session as though a **region** frame had landed, at a given render resolution.
    ///
    /// ⚠⚠ **Without this, nothing in this file can be tested at all.** `img_rect` and `frame_rect`
    /// are bit-identical whenever `rendered_region` is `None`, and no frame ever lands in the
    /// canvas harness — frames are pulled into textures by `shell.rs`, which this does not run. So
    /// a coordinate test would pass against either rect and prove nothing. W4's track D hit exactly
    /// this: its `frame_rect` sabotage came back *inert* for this reason.
    fn with_region(app: &mut RustroomApp, rendered_long: f32) {
        let s = app.session.as_mut().unwrap();
        s.zoom = 2.0;
        s.rendered_long = rendered_long;
        s.rendered_region = Some(rustroom_app::render_engine::RegionRect {
            x: 3,
            y: 2,
            w: 40,
            h: 20,
        });
    }

    /// How far a fixed drag moves the crop, at a given render resolution.
    fn crop_move_at(rendered_long: f32) -> f32 {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);
        with_region(&mut app, rendered_long);
        let before = app.session.as_ref().unwrap().edits.geometry.crop;
        let centre = canvas_centre(&mut app, &ctx);
        let _ = drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 0.0));
        app.session.as_ref().unwrap().edits.geometry.crop.left - before.left
    }

    /// ⚠⚠ **Head 3 of M3.1a's three-headed bug.** That work records that fixing the mask overlay's
    /// placement left two silent faults behind, one being that "the drag deltas were divided by the
    /// region's dimensions, so a handle dragged while zoomed moved by the wrong amount" — which
    /// "would have surfaced later as 'handles feel wrong when zoomed' with no obvious cause."
    ///
    /// ⚠⚠ **The property, and it needs no absolute number.** `frame_rect`'s width is
    /// `full * pk * pscale`, and `pscale` is `fit * zoom / pk` — so the `pk` cancels and the frame's
    /// on-screen width is **independent of the render resolution**. `img_rect`'s width is the
    /// *texture's* size times `pscale`, which is **not**. So the same pointer travel must move the
    /// crop by the same amount at two different render resolutions, and does so only if the delta
    /// is taken against the frame.
    #[test]
    fn a_crop_drag_moves_by_the_frame_not_the_rendered_slice() {
        let a = crop_move_at(40.0);
        let b = crop_move_at(80.0);
        assert!(a > 0.0, "the crop did not move at all at 40: {a}");
        assert!(b > 0.0, "the crop did not move at all at 80: {b}");
        assert!(
            (a - b).abs() < 1e-4,
            "the same drag moved the crop by {a} at one render resolution and {b} at another — \
             the delta is being divided by the rendered slice's width, not the frame's"
        );
    }

    /// As [`crop_move_at`], with the view panned first.
    fn crop_move_panned(pan: egui::Vec2) -> f32 {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);
        with_region(&mut app, 40.0);
        app.session.as_mut().unwrap().pan = pan;
        let before = app.session.as_ref().unwrap().edits.geometry.crop;
        let centre = canvas_centre(&mut app, &ctx);
        // ⚠ The grab point follows the pan, because the crop has moved on screen with the frame.
        let _ = drag_through(
            &mut app,
            &ctx,
            centre + pan,
            centre + pan + egui::vec2(20.0, 0.0),
        );
        app.session.as_ref().unwrap().edits.geometry.crop.left - before.left
    }

    /// The bounding box of every line segment the canvas paints, at a given render resolution.
    ///
    /// With the composition guide off, the only segments are the crop border and its brackets — so
    /// this box *is* where the crop is drawn.
    fn painted_bounds(rendered_long: f32) -> egui::Rect {
        painted_bounds_with(
            rendered_long,
            edit_model::CropRect {
                left: 0.2,
                top: 0.2,
                right: 0.8,
                bottom: 0.8,
            },
        )
    }

    /// As [`painted_bounds`], for a given crop.
    fn painted_bounds_with(rendered_long: f32, crop: edit_model::CropRect) -> egui::Rect {
        fn walk(s: &egui::Shape, acc: &mut egui::Rect) {
            match s {
                egui::Shape::LineSegment { points, .. } => {
                    for p in points {
                        *acc = acc.union(egui::Rect::from_min_max(*p, *p));
                    }
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, acc)),
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.tool = rustroom_app::tool::Tool::Crop;
        app.session.as_mut().unwrap().edits.geometry.crop = crop;
        with_region(&mut app, rendered_long);
        let c = ctx.clone();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen()),
                ..Default::default()
            },
            |ui| {
                super::super::show(&mut app, ui, &c);
            },
        );
        let mut acc = egui::Rect::NOTHING;
        out.shapes.iter().for_each(|s| walk(&s.shape, &mut acc));
        acc
    }

    /// ⚠⚠ **Head 1: where the crop is DRAWN.** `crop_screen_rect` places the border, the corner
    /// brackets, the dimming and the composition guides. Nothing about a drag can see it — the
    /// drag tests below pass with the placement reverted — so it needs an assertion on the painted
    /// geometry, which is what M3.1a means by the placement being the one visible head while the
    /// other two stayed silent.
    ///
    /// ⚠ The property is the same one the drag test uses and needs no absolute coordinates:
    /// `frame_rect`'s on-screen size is independent of the render resolution (`pk` cancels against
    /// `pscale`), and `img_rect`'s is not. So the crop must be drawn in the same place at two
    /// different render resolutions.
    #[test]
    fn the_crop_is_drawn_against_the_frame_not_the_rendered_slice() {
        let a = painted_bounds(40.0);
        let b = painted_bounds(80.0);
        assert!(
            a.is_finite() && a.width() > 1.0,
            "nothing was painted: {a:?}"
        );
        assert!(
            (a.min - b.min).length() < 0.5 && (a.max - b.max).length() < 0.5,
            "the crop was drawn at {a:?} at one render resolution and {b:?} at another — it is \
             being placed against the rendered slice, so it slides with the view"
        );
    }

    /// ⚠⚠ **Head 2: WHICH handle a press grabs.** The crop is stored in frame-normalised
    /// coordinates, so a pointer position normalised against the rendered slice lands somewhere
    /// else entirely and grabs the wrong handle — or `Inside`, which moves the whole rectangle.
    ///
    /// ⚠ Grabs the crop's **left edge**, not its middle: a press near the centre still resolves to
    /// `Inside` under either normalisation, which is why the drag tests above cannot see this.
    /// Asserts the **right** edge is untouched, since `Inside` would carry it along.
    #[test]
    fn a_press_on_the_left_edge_grabs_that_edge_and_not_the_whole_crop() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_inset_crop(&mut app);
        with_region(&mut app, 40.0);
        // ⚠ A crop whose left edge sits near the middle of the frame, so the press lands inside
        // `img_rect`. The canvas allocates its response over `img_rect` — what is actually painted
        // — so with a region render on, a press outside the rendered slice is never delivered at
        // all. A handle outside the slice genuinely cannot be grabbed; that is a property of the
        // region rendering, not of this change.
        {
            let s = app.session.as_mut().unwrap();
            s.edits.geometry.crop = edit_model::CropRect {
                left: 0.45,
                top: 0.2,
                right: 0.95,
                bottom: 0.8,
            };
        }
        let before = app.session.as_ref().unwrap().edits.geometry.crop;
        let centre = canvas_centre(&mut app, &ctx);

        // Just inside the drawn left edge, comfortably within the 0.03 grab tolerance and far from
        // the crop's own centre.
        let drawn = painted_bounds_with(40.0, before);
        let at = egui::pos2(drawn.left() + 1.0, centre.y);
        let _ = drag_through(&mut app, &ctx, at, at + egui::vec2(12.0, 0.0));

        let after = app.session.as_ref().unwrap().edits.geometry.crop;
        assert!(
            after.left > before.left + 1e-4,
            "the left edge did not move: {} -> {}",
            before.left,
            after.left
        );
        assert!(
            (after.right - before.right).abs() < 1e-4,
            "the right edge moved too ({} -> {}), so the press grabbed Inside rather than the \
             left edge — the pointer was normalised against the wrong rect",
            before.right,
            after.right
        );
    }

    /// ⚠⚠ **A pan must not change what a drag does — head 1 of M3.1a's bug.** Panning shifts
    /// `frame_rect`'s origin; `img_rect`'s origin moves with it *by a different amount* once a
    /// region is on, so a placement or hit-test still reading `img_rect` makes the crop appear to
    /// slide under the pointer. That is the original mask-overlay symptom, "appearing to follow
    /// the view as you pan".
    ///
    /// ⚠ The first version of this test panned and asserted the crop had not changed — which is
    /// **inert**, because nothing was dragging and no code path could have changed it. It has to
    /// perform the same drag panned and unpanned and compare.
    #[test]
    fn panning_does_not_change_what_a_crop_drag_does() {
        let plain = crop_move_at(40.0);
        let panned = crop_move_panned(egui::vec2(40.0, 25.0));
        assert!(plain > 0.0, "the unpanned drag did nothing: {plain}");
        assert!(panned > 0.0, "the panned drag did nothing: {panned}");
        assert!(
            (plain - panned).abs() < 1e-4,
            "the same drag moved the crop by {plain} unpanned and {panned} panned — the crop is \
             being placed or hit-tested against the rendered slice, so it slides with the view"
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
