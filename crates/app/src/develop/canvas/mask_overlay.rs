//! The mask overlay: a mask's handles and guides, the brush's painting gesture and cursor, and
//! the memoised coverage tint.

use super::mask_tint::paint_tint;
use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::canvas_geom;
use crate::Session;
use eframe::egui;
use rustroom_app::brush_tool;
use rustroom_app::mask_tool;
use rustroom_app::tool::Tool;

/// The brush tool's live settings, copied out of `RustroomApp` before the session borrow.
///
/// ⚠ A struct rather than four loose parameters: `draw_mask_overlay` already takes five, and a
/// nine-argument function is where argument-order mistakes start.
#[derive(Clone, Copy)]
pub(super) struct BrushTool<'a> {
    pub(super) size: f32,
    pub(super) hardness: f32,
    pub(super) flow: f32,
    pub(super) erase: bool,
    pub(super) show_tint: bool,
    pub(super) auto_mask: bool,
    pub(super) tolerance: f32,
    /// The displayed frame the tint's auto-mask gate reads, when one has landed.
    ///
    /// ⚠ A **borrow**, so `BrushTool` stays `Copy` — `TintFrame` holds an `Arc` and is not. It
    /// lives here rather than as a ninth parameter because clippy's argument limit is a fair
    /// complaint and this project's standing answer to it is a params struct, not an allowance:
    /// `FrameSpec`, `NameField`, `AspectLock` and `GradeUi` are the precedents.
    pub(super) base: Option<&'a TintFrame>,
}

/// The displayed frame the tint's auto-mask gate reads, when one has landed.
///
/// ⚠ Carried beside `BrushTool` rather than on it because it is not `Copy` — it holds an `Arc` —
/// and `BrushTool` is passed by value to keep the overlay's argument list short.
pub(super) type TintFrame = super::mask_tint::TintFrame;

/// Draw the selected mask's handles and guide lines, and apply any drag.
///
/// Returns whether a drag settled into a new history entry — the same contract
/// `draw_crop_overlay` has.
///
/// ⚠ **Three lines, not a tint.** A faithful coverage tint needs three quads, because coverage
/// is clamped flat either side of the ramp; two boundary lines state the same thing exactly
/// and do not wash out the photograph being judged.
/// ⚠ **Every coordinate here is against `frame_rect`, never `img_rect`.** A mask is defined in
/// whole-frame normalised coordinates, but `img_rect` covers only the *rendered slice* once a
/// region render is on. Using it made the overlay drift with the view while panning at high
/// zoom — and silently corrupted `aspect` too, since a region's aspect is not the frame's.
/// `img_rect` is still the right clip: there is no photograph outside it to annotate.
pub(super) fn draw_mask_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    brush: BrushTool<'_>,
    tool: Tool,
) -> bool {
    let Some(i) = s.selected_mask else {
        return false;
    };
    let Some(m) = s.edits.masks.get(i) else {
        return false;
    };
    let mask = m.mask.clone();
    if frame_rect.width() <= f32::EPSILON || frame_rect.height() <= f32::EPSILON {
        return false;
    }

    paint_mask(
        &ui.painter_at(img_rect),
        &mask,
        frame_rect,
        img_rect,
        &s.edits.geometry,
        brush.show_tint,
        brush.base,
    );

    let aspect = canvas_geom::frame_aspect(frame_rect);
    let mut moved = false;
    if let Some(p) = resp.interact_pointer_pos() {
        let (nx, ny) = canvas_geom::normalized_point(p, frame_rect);
        if matches!(mask, edit_model::Mask::Brush(_)) {
            // ⚠ No paint mode. Left-drag is free because the navigation rework moved panning onto
            // scroll and a drag button, which is the whole reason that landed first.
            let map = pipeline::brush::FrameMap::new(
                &s.edits.geometry,
                frame_rect.width() as usize,
                frame_rect.height() as usize,
            );
            let radius = (brush.size / map.radius_scale.max(1e-6)).max(1e-4);
            let src = map.inverse_point(p.x - frame_rect.left(), p.y - frame_rect.top());
            if tool_drag_started(resp) {
                if let Some(edit_model::Mask::Brush(b)) =
                    s.edits.masks.get_mut(i).map(|m| &mut m.mask)
                {
                    b.strokes.push(edit_model::BrushStroke {
                        points: vec![src],
                        radius,
                        hardness: brush.hardness,
                        flow: brush.flow,
                        erase: brush.erase,
                        // ⚠ Baked here, at the moment the stroke begins, exactly as `radius`,
                        // `hardness`, `flow` and `erase` are: a stroke keeps what the tool was set
                        // to when it was painted, and toggling the checkbox afterwards changes
                        // only the next one.
                        //
                        // ⚠ **No reference colour is recorded.** It is read at render time from
                        // the pixel under each dab's own centre, which is what keeps both ends of
                        // the comparison in one colour space — see `brush::auto_mask_gate`.
                        auto_mask: brush.auto_mask,
                        tolerance: brush.tolerance,
                    });
                    // ⚠⚠ **Deliberately does NOT set `moved`.** Requesting a render here starts a
                    // full-resolution render of a ONE-POINT stroke you are about to extend. At
                    // 1:1 that takes seconds, occupies the render worker for the whole drag, and
                    // lands *after* the line is finished — repainting the single initial dab over
                    // the correct result, which then has to queue behind it.
                    //
                    // Diagnosed from `RUSTROOM_DEBUG_BRUSH` output showing strokes recorded
                    // correctly (20-52 points) at `max_dim 6246`: the model was right and the
                    // render was stale. Reported as "the initial dot is still there".
                }
            } else if tool_drag(resp) {
                // ⚠ `else if`, not a second `if`: both predicates are true on the frame a drag
                // begins, and without the `else` that frame would record its first point twice.
                if let Some(edit_model::Mask::Brush(b)) =
                    s.edits.masks.get_mut(i).map(|m| &mut m.mask)
                {
                    if let Some(stroke) = b.strokes.last_mut() {
                        let keep = stroke
                            .points
                            .last()
                            .is_none_or(|last| brush_tool::should_record(*last, src, radius));
                        if keep {
                            stroke.points.push(src);
                            // ⚠ Live rendering, enabled by region-scoped coverage: a paint frame
                            // is now bounded by the viewport rather than by the image. Measured
                            // at ~16.6 ms at 1:1 on a full-resolution X-T4 buffer — 9.03 ms
                            // clipped rasterization, 7.2 ms region tail, ~0.4 ms pack — which is
                            // marginal against a 60 Hz budget and recorded as such rather than
                            // claimed as comfortable.
                            //
                            // ⚠ `drag_started` stays suppressed. Rendering a one-point stroke you
                            // are about to extend was pure waste and caused the "initial dot"
                            // bug: a full-resolution render landing after the line was finished
                            // and painting over it. Live rendering means rendering on **motion**,
                            // never on press.
                            moved = true;
                        }
                    }
                }
            }
        } else {
            if tool_drag_started(resp) {
                s.active_mask_handle = mask_tool::hit_test(&mask, [nx, ny], aspect, 0.04);
            }
            if tool_drag(resp) {
                if let Some(h) = s.active_mask_handle {
                    let d = resp.drag_delta();
                    let dx = d.x / frame_rect.width();
                    let dy = d.y / frame_rect.height();
                    if let Some(m) = s.edits.masks.get_mut(i) {
                        mask_tool::apply_drag(&mut m.mask, h, dx, dy);
                        moved = true;
                    }
                }
            }
            if resp.drag_stopped() && s.active_mask_handle.take().is_some() {
                s.history.push(s.edits.clone());
                return true;
            }
        }
    }

    // ⚠⚠ **Outside the `interact_pointer_pos` block on purpose.** On the frame a drag ends egui
    // often reports no interact position, so a `drag_stopped` handler nested inside it simply
    // never runs. That is not hypothetical: it shipped for one build, and the symptom was that
    // after drawing a line only the *first dab* appeared once the tint was hidden — the last
    // render was the one from `drag_started`, when the stroke had a single point. The history
    // push was being skipped too, so the stroke was not undoable.
    if matches!(mask, edit_model::Mask::Brush(_)) && resp.drag_stopped() {
        // The final render for this stroke, and the one that pushes history. Motion frames render
        // too now — see the note there — so this mostly guarantees the last few points land.
        // ⚠ Env-gated instrumentation, kept because this project's own history says to reach for
        // a print when the code reads correctly and the behaviour disagrees — the fit-view
        // investigation went four rounds on reasoning before one `eprintln` reframed it.
        if std::env::var("RUSTROOM_DEBUG_BRUSH").is_ok() {
            if let Some(edit_model::Mask::Brush(b)) = s.edits.masks.get(i).map(|m| &m.mask) {
                eprintln!(
                    "brush: drag_stopped, {} strokes, last has {} points, max_dim {}, region {:?}",
                    b.strokes.len(),
                    b.strokes.last().map_or(0, |st| st.points.len()),
                    s.render_max_dim(),
                    s.requested_region.is_some(),
                );
            }
        }
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        s.history.push(s.edits.clone());
        return true;
    }

    // The brush cursor: a circle at the pointer at the current screen radius. ⚠ Without it the
    // Size slider is guesswork, because a screen-point size has no other on-screen
    // representation. `hover_pos`, not `interact_pointer_pos`: it must show before a drag begins.
    if matches!(mask, edit_model::Mask::Brush(_)) {
        if let Some(hp) = resp.hover_pos() {
            ui.painter_at(img_rect).circle_stroke(
                hp,
                brush.size,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(180)),
            );
        }
    }

    // ⚠ Moving a mask changes the *rendered image*, so it must ask for a re-render — the
    // panel's sliders do exactly this on `changed`. Without it the handles moved while the
    // photo kept showing the mask's previous position, updating only when some later edit
    // happened to request a frame. Unlike `draw_crop_overlay`, which needs no request because
    // `preview_edits` strips the crop in geometry mode and the overlay *is* the feedback.
    if moved {
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    }
    false
}

/// Points used to approximate a mask ellipse. egui has no ellipse primitive, so it is drawn as
/// a closed polyline; 64 is smooth at any zoom this canvas reaches.
const ELLIPSE_SEGMENTS: usize = 64;

/// Paint one mask's handles and guides.
///
/// ⚠ **Split from `draw_mask_overlay` deliberately, on painting rather than on the whole
/// function.** The drag plumbing — hit-testing, applying the drag, pushing history and calling
/// `s.request` — is shared by every shape and must be written once. Duplicating it per shape is
/// exactly how the two would drift, and it is where both of 3.1a's real-use bugs lived.
///
/// ⚠ `frame_rect` places everything; `clip` only bounds what is drawn. They differ once a
/// region render is on, and using `clip` for placement is the bug that made the overlay drift
/// with the view while panning at high zoom.
fn paint_mask(
    painter: &egui::Painter,
    mask: &edit_model::Mask,
    frame_rect: egui::Rect,
    clip: egui::Rect,
    geometry: &edit_model::Geometry,
    show_tint: bool,
    base: Option<&TintFrame>,
) {
    let to_screen = |p: [f32; 2]| {
        egui::pos2(
            frame_rect.left() + p[0] * frame_rect.width(),
            frame_rect.top() + p[1] * frame_rect.height(),
        )
    };
    let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);

    match mask {
        edit_model::Mask::Linear(g) => {
            let (a, b) = (to_screen(g.from), to_screen(g.to));
            painter.line_segment([a, b], stroke);

            // ⚠ The perpendicular is computed **in screen space**, from the on-screen axis —
            // the same trap as `coverage` and `hit_test`, in its third guise. A
            // normalised-space perpendicular is visibly not perpendicular on a 3:2 frame.
            let axis = b - a;
            let len = axis.length();
            if len > f32::EPSILON {
                let perp = egui::vec2(-axis.y, axis.x) / len;
                // Long enough to cross the clip region from any point inside it.
                let reach = clip.width() + clip.height();
                for end in [a, b] {
                    painter.line_segment(
                        [end - perp * reach, end + perp * reach],
                        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120)),
                    );
                }
            }
            for end in [a, b] {
                painter.circle_filled(end, 5.0, egui::Color32::WHITE);
            }
        }
        edit_model::Mask::Radial(r) => {
            let points: Vec<egui::Pos2> = (0..ELLIPSE_SEGMENTS)
                .map(|i| {
                    let th = i as f32 / ELLIPSE_SEGMENTS as f32 * std::f32::consts::TAU;
                    to_screen([
                        r.center[0] + r.radius[0] * th.cos(),
                        r.center[1] + r.radius[1] * th.sin(),
                    ])
                })
                .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
            // Centre plus the four edge handles, in the order `mask_tool::hit_test_radial`
            // tests them.
            let handles = [
                r.center,
                [r.center[0] - r.radius[0], r.center[1]],
                [r.center[0] + r.radius[0], r.center[1]],
                [r.center[0], r.center[1] - r.radius[1]],
                [r.center[0], r.center[1] + r.radius[1]],
            ];
            for h in handles {
                painter.circle_filled(to_screen(h), 5.0, egui::Color32::WHITE);
            }
        }
        edit_model::Mask::Brush(b) => {
            // ⚠ There are no handles — a stroke is not a shape with grab points, the same
            // observation that makes `mask_tool::hit_test` return `None` for a brush.
            if show_tint {
                paint_tint(painter, b, geometry, frame_rect, base);
            }
        }
        // ⚠⚠ **A selected luminance range draws NOTHING, and that is a decision rather than an
        // omission.** It is not a shape: it has no position, no handles and no boundary anywhere
        // on the canvas, so there is no annotation to place. Its feedback is the two sliders in
        // the panel and the rendered image itself.
        //
        // ⚠ A coverage tint was considered and rejected as **actively misleading**, not merely
        // expensive. The tint machinery (`mask_tint`) rasterizes from a brush's *strokes*, which
        // are geometry the canvas holds; a range mask's coverage is a function of the photograph's
        // own pixels, and the only pixels the canvas has are the **rendered texture — which
        // already has this mask's adjustment applied**. Tinting from those would show the mask
        // selecting the tones it has just moved, i.e. exactly the "mask chasing its own effect"
        // failure that `CoverageSource::Range` keys on `base` to avoid, made visible and wrong.
        //
        // Doing it honestly needs coverage computed against the *pre-mask* buffer, which lives on
        // the render worker — a real feature, and a different one.
        // ⚠⚠ **A selected colour range draws nothing either, and the reasoning above transfers
        // unchanged.** It has no position, so there is nothing to annotate; and a coverage tint
        // would be misleading for exactly the same reason, one step worse — the rendered texture
        // already carries this mask's adjustment, and a mask that carries a saturation or
        // white-balance move has changed the very hues its tint would be measured from. Doing it
        // honestly needs coverage against the pre-mask buffer, which lives on the render worker.
        edit_model::Mask::Luminance(_) | edit_model::Mask::Colour(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{canvas_centre, drag_through, DRAG_PASS};
    use crate::RustroomApp;
    use eframe::egui;

    /// A session with one linear-gradient mask selected and the Masks tool active.
    ///
    /// ⚠ The endpoints are set explicitly. `LinearGradient::default()` puts *both* at `[0,0]`,
    /// which is a degenerate mask whose handles coincide — a drag on it would be testing nothing
    /// in particular. `from` sits at the frame centre so its screen position is the viewport
    /// centre, which is what makes the drag points below deterministic.
    fn with_selected_mask(app: &mut RustroomApp) {
        app.tool = rustroom_app::tool::Tool::Masks;
        let s = app.session.as_mut().unwrap();
        s.edits.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                from: [0.5, 0.5],
                to: [0.5, 0.9],
                invert: false,
            }),
            adjustments: Default::default(),
        });
        s.selected_mask = Some(0);
    }
    /// A session with an empty brush mask selected and the Masks tool active.
    fn with_selected_brush(app: &mut RustroomApp) {
        app.tool = rustroom_app::tool::Tool::Masks;
        let s = app.session.as_mut().unwrap();
        s.edits.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(edit_model::Brush::default()),
            adjustments: Default::default(),
        });
        s.selected_mask = Some(0);
    }

    fn painted_stroke(app: &RustroomApp) -> edit_model::BrushStroke {
        let s = app.session.as_ref().unwrap();
        let edit_model::Mask::Brush(b) = &s.edits.masks[0].mask else {
            panic!("expected a brush mask");
        };
        b.strokes.last().expect("a stroke was painted").clone()
    }

    #[test]
    fn painting_with_auto_mask_on_bakes_it_into_the_stroke() {
        // ⚠⚠ **The carrier test, and the W5 creative-LUT lesson applied.** Every pipeline and
        // golden test for this feature would pass against a build where the checkbox reached
        // nothing at all — which is exactly how creative LUTs shipped green and 100%
        // non-functional. This drives the real gesture and reads what it wrote.
        //
        // ⚠ A headless `RawInput` drag lifecycle, which this file's own record says would have
        // caught three of M3.1c-B's six canvas bugs.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_brush(&mut app);
        app.brush_auto_mask = true;
        app.brush_tolerance = 40.0;

        let centre = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 0.0));

        let stroke = painted_stroke(&app);
        assert!(stroke.auto_mask, "the checkbox never reached the stroke");
        assert_eq!(
            stroke.tolerance, 40.0,
            "the slider never reached the stroke"
        );
    }

    #[test]
    fn painting_with_auto_mask_off_leaves_the_stroke_plain() {
        // ⚠ The discriminating half: a gesture that hardcoded `true` would pass the test above.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_brush(&mut app);
        app.brush_auto_mask = false;

        let centre = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 0.0));

        assert!(!painted_stroke(&app).auto_mask);
    }

    #[test]
    fn dragging_a_mask_handle_asks_for_a_render() {
        // ⚠⚠ The regression guard for a bug that shipped: `draw_mask_overlay` mutated `Edits` but
        // never called `s.request`, so the image kept showing the mask's previous position until
        // some *unrelated* edit happened to request a frame. That is why moving a slider appeared
        // to "fix" it.
        //
        // ⚠ The cause was an inherited false analogy — it was modelled on `draw_crop_overlay`,
        // which needs no request *because* `preview_edits` strips the crop in geometry mode, so
        // the overlay itself is the feedback. Masks are not stripped.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_mask(&mut app);

        // ⚠⚠ **Sampled on the drag frame itself, not after the gesture.** Measuring at the end
        // does not discriminate: a *later* frame issues a request of its own once `Edits` has
        // changed, so the count rises either way and the test passes with `s.request` deleted —
        // verified by sabotage. In the real app that later frame never comes, because egui idles
        // after the drag; that is precisely why the bug was visible as "the image does not update
        // until you touch a slider".
        let centre = canvas_centre(&mut app, &ctx);
        let counts = drag_through(&mut app, &ctx, centre, centre + egui::vec2(40.0, 0.0));
        assert!(
            counts[DRAG_PASS] > counts[DRAG_PASS - 1],
            "the drag frame itself must ask for a render; counts={counts:?}"
        );
    }

    #[test]
    fn a_drag_that_misses_every_handle_asks_for_nothing() {
        // ⚠⚠ **The guard on the guard, and it is not optional.** `show` issues renders for
        // reasons of its own, so "the count went up after a drag" only means something if it
        // *stays put* when the drag grabs nothing. An earlier version of the harness pressed
        // after the pointer had already moved, so every drag missed — and the sibling test
        // passed anyway. This is what would have caught that.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_mask(&mut app);

        // Warm up: let the first frames issue whatever they issue, then measure.
        drag_through(
            &mut app,
            &ctx,
            egui::pos2(12.0, 58.0),
            egui::pos2(14.0, 60.0),
        );
        let before = app.session.as_ref().unwrap().engine.requests_issued();
        // Far from `from` [0.5,0.5] and `to` [0.5,0.9], in the frame's top-left corner.
        drag_through(
            &mut app,
            &ctx,
            egui::pos2(12.0, 58.0),
            egui::pos2(30.0, 60.0),
        );
        let after = app.session.as_ref().unwrap().engine.requests_issued();

        assert_eq!(
            before, after,
            "a drag grabbing no handle must not ask for a render; before={before} after={after}"
        );
    }

    #[test]
    fn a_mask_drag_commits_one_undo_step_on_release() {
        // ⚠⚠ The regression guard for the third bug shape: `drag_stopped` was nested inside
        // `if let Some(p) = resp.interact_pointer_pos()`, and on the frame a drag ends egui often
        // reports **no** interact position — so the commit was silently skipped.
        //
        // ⚠ Asserted on the **history**, not on `Edits`. The edits change during the *drag*, from
        // `apply_drag`; only the undo push happens on release. An earlier version of this test
        // compared edits and passed with the release handler disabled entirely — caught by
        // sabotage, and exactly the "a test names a bug it cannot detect" trap this project has
        // now hit a dozen times.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_mask(&mut app);
        assert!(
            !app.session.as_ref().unwrap().history.can_undo(),
            "nothing to undo before the drag"
        );

        let centre = canvas_centre(&mut app, &ctx);
        let _ = drag_through(&mut app, &ctx, centre, centre + egui::vec2(60.0, 10.0));

        let s = app.session.as_ref().unwrap();
        assert!(
            s.history.can_undo(),
            "the release must have committed one undo step"
        );
        assert_ne!(
            s.edits,
            edit_model::Edits::default(),
            "and the mask must actually have moved"
        );
    }

    #[test]
    fn the_mask_overlay_is_placed_against_the_frame_not_the_view() {
        // ⚠⚠ The regression guard for the drift bug: the overlay was placed against `img_rect`,
        // which once a region render is on covers only the *rendered slice*. The mask then
        // followed the visible part of the image as you panned.
        //
        // ⚠ Asserted through `canvas_geom::frame_rect` rather than by reading pixels: the frame's
        // *size* must not change with the pan, which is precisely what `img_rect` would do.
        let full = egui::vec2(80.0, 40.0);
        let at = egui::pos2(200.0, 150.0);
        let unpanned = super::canvas_geom::frame_rect(at, egui::Vec2::ZERO, full, 1.0, 2.0);
        let panned = super::canvas_geom::frame_rect(at, egui::vec2(50.0, -20.0), full, 1.0, 2.0);
        assert_eq!(
            unpanned.size(),
            panned.size(),
            "panning must not resize the frame the mask is placed against"
        );
        assert_eq!(panned.center() - unpanned.center(), egui::vec2(50.0, -20.0));
    }
}
