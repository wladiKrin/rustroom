//! The retouch overlay: every spot's destination and source, and the gestures that place, move
//! and size them.
//!
//! ⚠⚠ **Every coordinate goes against `frame_rect`, never `img_rect`.** That is the M3.1a drift
//! bug: once a region render is on, `img_rect` covers only the rendered *slice*, so an overlay
//! placed against it follows the view while panning and takes the region's aspect rather than the
//! frame's. `img_rect` survives as the **clip** and nothing else.
//!
//! ⚠ **Recorded honestly: no test can see that distinction.** The two rects differ only under a
//! *region* render, which happens at high zoom, and the headless harness always renders the whole
//! image — so `img_rect == frame_rect` there and substituting one for the other changes nothing.
//! The sabotage was run and is inert. Inventing a test that appeared to cover it would be worse
//! than saying so.
//!
//! ⚠⚠ **Spots are stored post-lens, PRE-PERSPECTIVE (since M7.3b) and this draws on the
//! post-geometry frame**, so each one maps forward through [`SpotMap::point`]: the keystone,
//! then `FrameMap`'s orientation, straighten and crop. Unlike the analytic masks, whose coordinates
//! are already frame-relative, that mapping is not optional: without it a cropped, straightened
//! or keystoned photo draws every spot in the wrong place.

use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::canvas_geom;
use crate::Session;
use eframe::egui;
use pipeline::brush::SpotMap;
use rustroom_app::spot_tool::{self, Handle, SpotTool};
use rustroom_app::tool::Tool;

/// Segments in a drawn circle. egui has no ellipse primitive; the radial mask uses the same count.
const CIRCLE_SEGMENTS: usize = 48;

/// A drag shorter than this, in screen points, is a click that wandered.
///
/// ⚠ Without it a click with a shaky hand would size a spot to ~0, which covers nothing and can
/// never be grabbed again.
const CLICK_SLOP: f32 = 3.0;

/// The smallest a handle may be to grab, in **screen points**.
///
/// ⚠ [`spot_tool::MIN_RADIUS`] allows a spot about 2 pt across at fit zoom. Without a floor here
/// such a spot could be placed and then never selected, moved or deleted again — which is why
/// lowering that floor and adding this had to happen together.
const GRAB_POINTS: f32 = 7.0;

/// What the pointer is currently doing.
///
/// ⚠⚠ **The two placing variants carry SCREEN positions, updated on every drag frame, because
/// egui frequently reports no `interact_pointer_pos` on the release frame.** Reading the pointer
/// only at `drag_stopped` therefore places nothing at all — which is precisely the trap
/// `draw_mask_overlay` already records for its own release handling, met here from the other
/// side. Tracking the latest position as the drag runs makes the release frame need no pointer.
#[derive(Clone, Copy, PartialEq, Default)]
pub(crate) enum SpotDrag {
    #[default]
    None,
    /// Sizing a new spot: where the press landed, and where the pointer has reached.
    Sizing { at: egui::Pos2, to: egui::Pos2 },
    /// Choosing a source for a new spot (⌥-drag): same two points.
    Sourcing { at: egui::Pos2, to: egui::Pos2 },
    /// Moving one end of an existing spot.
    Moving(usize, Handle),
}

impl SpotDrag {
    /// Record the pointer's latest position, whichever placing gesture is running.
    fn advance(&mut self, p: egui::Pos2) {
        match self {
            Self::Sizing { to, .. } | Self::Sourcing { to, .. } => *to = p,
            _ => {}
        }
    }
}

/// Draw the overlay and run the gestures. `true` when the edits changed.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_spot_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    tool: &mut SpotTool,
    sample: &dyn Fn([f32; 2]) -> [f32; 3],
) -> bool {
    if frame_rect.width() <= f32::EPSILON || frame_rect.height() <= f32::EPSILON {
        return false;
    }
    let aspect = canvas_geom::frame_aspect(frame_rect);
    let map = SpotMap::new(
        &s.edits.geometry,
        s.edits.perspective,
        frame_rect.width() as usize,
        frame_rect.height() as usize,
    );
    // ⚠ `radius_scale` is the source long edge in frame pixels, so dividing by it converts a
    // screen distance into the long-edge units a spot's radius is measured in.
    let min_grab = if map.radius_scale > 0.0 {
        GRAB_POINTS / map.radius_scale
    } else {
        0.0
    };
    let mut changed = false;

    // A canvas point → the stage's input space, which is where a spot lives.
    let to_source = |p: egui::Pos2| -> [f32; 2] {
        let (nx, ny) = canvas_geom::normalized_point(p, frame_rect);
        map.inverse_point(nx * frame_rect.width(), ny * frame_rect.height())
    };

    // ---- the click gesture ------------------------------------------------------------------
    // ⚠⚠ **A second, click-sensing interaction — the canvas's own `resp` is `Sense::drag()` and
    // never reports `clicked()`.** Making `resp` `click_and_drag` instead also changes when egui
    // promotes a press to a drag, which broke both mask-drag tests outright when the white-balance
    // eyedropper tried exactly that. egui's hit test resolves click- and drag-sensing widgets
    // **separately**, so a click-only widget on top and the drag-sensing canvas beneath each get
    // their own events.
    //
    // ⚠ Registered *after* `resp`, which is what makes it win the click: egui breaks a hit-test
    // tie by registration order, taking the last as the one on top.
    let click = ui
        .interact(
            img_rect,
            ui.id().with("retouch_click"),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::Crosshair);
    if click.clicked() {
        if let Some(p) = click.interact_pointer_pos() {
            let at = to_source(p);
            // ⚠ Hit-test BEFORE placement: a click on an existing handle selects it rather than
            // stacking a new spot on top of it.
            match spot_tool::hit_test(&s.edits.spots, at, aspect, min_grab) {
                Some((i, _)) => {
                    s.selected_spot = Some(i);
                    s.selected_eye = None;
                }
                None => {
                    let sp = place_with_source(at, *tool, &s.edits.spots, aspect, sample);
                    s.edits.spots.push(sp);
                    s.selected_spot = Some(s.edits.spots.len() - 1);
                    s.selected_eye = None;
                    changed = true;
                }
            }
        }
    }

    // ---- the drag gestures ------------------------------------------------------------------
    if tool_drag_started(resp) {
        if let Some(p) = resp.interact_pointer_pos() {
            let at = to_source(p);
            s.spot_drag = match spot_tool::hit_test(&s.edits.spots, at, aspect, min_grab) {
                Some((i, h)) => {
                    s.selected_spot = Some(i);
                    s.selected_eye = None;
                    // ⚠⚠ **The patch leaves the view for the duration of the drag**, so the
                    // blemish underneath is visible and can be lined up against. Without this its
                    // old effect stays on screen and hides the very thing being aimed at.
                    //
                    // ⚠ One render here and one on release — not one per frame, which is what was
                    // reported as laggy. `SpotRemovalStage` is in the prefix, so each costs a
                    // whole-buffer noise-reduction and dehaze pass.
                    s.suppressed_spot = Some(i);
                    s.request(
                        s.view_edits(Tool::Retouch),
                        s.render_max_dim(),
                        s.requested_region,
                    );
                    SpotDrag::Moving(i, h)
                }
                None if ui.input(|i| i.modifiers.alt) => SpotDrag::Sourcing { at: p, to: p },
                None => SpotDrag::Sizing { at: p, to: p },
            };
        }
    }

    if tool_drag(resp) {
        if let Some(p) = resp.interact_pointer_pos() {
            s.spot_drag.advance(p);
        }
        // ⚠⚠ **Sized on every drag frame, not only on release** — reported as "no way to gauge
        // the size" and "the slider should adapt already while dragging". The tool is what the
        // panel's Size slider reads, so writing it here is what makes the slider track the
        // gesture; the preview circle below reads the same value, so the two cannot disagree.
        if let SpotDrag::Sizing { at, to } = s.spot_drag {
            if (to - at).length() > CLICK_SLOP {
                tool.radius = spot_tool::drag_radius(to_source(at), to_source(to), aspect);
            }
        }
        if let SpotDrag::Moving(i, h) = s.spot_drag {
            // ⚠ `drag_delta` is in **screen** points while a spot lives in source-normalised
            // units, so the delta is converted by mapping both ends of it through the same inverse
            // the press used. Scaling by `frame_rect` alone would be wrong on a cropped or
            // straightened photo, where the map is not a plain scale.
            if let Some(p) = resp.interact_pointer_pos() {
                let now = to_source(p);
                let was = to_source(p - resp.drag_delta());
                if let Some(sp) = s.edits.spots.get_mut(i) {
                    spot_tool::apply_drag(sp, h, now[0] - was[0], now[1] - was[1]);
                }
                // ⚠⚠ **Deliberately NO render while the drag runs — the moving circle IS the
                // feedback**, which is `draw_crop_overlay`'s model rather than the mask overlay's.
                // Reported as "very laggy, as it adapts the retouching on the go", and the reason
                // is structural: `SpotRemovalStage` is in the **prefix**, so moving a spot drops
                // the prefix cache and re-runs noise reduction and dehaze on the whole buffer —
                // ~117 ms each at native resolution. A mask handle can render live because
                // `MaskStage` is in the cheap tail.
                //
                // ⚠ An earlier version did request here, citing the M3.1a bug. That reasoning was
                // right for masks and wrong here, and only using it showed which. The release path
                // below still requests, so the pixels are never left stale.
                //
                // ⚠⚠ **And the move is REPORTED on release, not here.** Reporting it on every drag
                // frame logged one undo action per frame with no history step behind any of them.
            }
        }
    }

    if resp.drag_stopped() {
        // ⚠ The pointer may not be reported on this frame, so the gesture's own record is used
        // and a live position only refines it.
        let live = resp.interact_pointer_pos();
        let placed = match s.spot_drag {
            // ⚠⚠ **A gesture that never moved is left to the click path, and this is what keeps
            // the two disjoint.** egui runs the whole drag lifecycle for a plain click on a
            // `Sense::drag()` widget — `drag_started` and `drag_stopped` both fire with a zero
            // delta — so without this guard a single click placed **two** spots: one from the
            // click interaction and one from here.
            SpotDrag::Sizing { at, to } if (live.unwrap_or(to) - at).length() > CLICK_SLOP => {
                let to = live.unwrap_or(to);
                let centre = to_source(at);
                // ⚠⚠ **The gesture writes back into the tool**, which is what makes the panel's
                // Size slider follow the drag. One value, two affordances. Storing the radius only
                // on the spot would leave the slider showing a number that no longer describes
                // what the next click will place.
                //
                // ⚠ The radius was already written on every drag frame, so the slider could
                // follow it; this only settles the last position, which the release frame may
                // report more precisely than the previous drag frame did.
                tool.radius = spot_tool::drag_radius(centre, to_source(to), aspect);
                Some(place_with_source(
                    centre,
                    *tool,
                    &s.edits.spots,
                    aspect,
                    sample,
                ))
            }
            // ⚠ Same rule: an ⌥-click that never moved has chosen no source, so it falls to
            // the click path and gets an auto-picked one.
            SpotDrag::Sourcing { at, to } if (live.unwrap_or(to) - at).length() > CLICK_SLOP => {
                let to = live.unwrap_or(to);
                Some(spot_tool::place(to_source(at), to_source(to), *tool))
            }
            _ => None,
        };
        if let Some(sp) = placed {
            s.edits.spots.push(sp);
            s.selected_spot = Some(s.edits.spots.len() - 1);
            s.selected_eye = None;
            changed = true;
        }
        if matches!(s.spot_drag, SpotDrag::Moving(..)) {
            changed = true;
        }
        if s.spot_drag != SpotDrag::None {
            // ⚠ Cleared before the request, or the render that is meant to bring the patch back
            // would be built from the suppressed view and leave it invisible.
            s.suppressed_spot = None;
            // ⚠⚠ A drag MUST request a render. `draw_crop_overlay` deliberately does not, because
            // `preview_edits` strips the crop in geometry mode so the overlay *is* the feedback —
            // copying that here is the exact false analogy that shipped the M3.1a mask bug, where
            // the image did not update until an unrelated slider was touched.
            s.request(
                s.view_edits(Tool::Retouch),
                s.render_max_dim(),
                s.requested_region,
            );
            s.spot_drag = SpotDrag::None;
        }
    }

    // ⚠⚠ **The overlay records its own history step, and reports the edit only if one was
    // recorded** — the mask and crop overlays' rule. `develop::body` logs an `ActionKind` for a
    // `true`, and a logged action with no history step behind it makes Ctrl+Z pop the step
    // *before* this one (`History::push`'s doc) — which is what every spot placement did until
    // M9.11. A move that ended where it began pushes nothing and so reports nothing.
    let changed = changed && s.history.push(s.edits.clone());

    // ---- the overlay ------------------------------------------------------------------------
    // ⚠ Widened by `overlay_clip`: `line_segment` centres its stroke on the path, so a halo drawn
    // on a spot at the very edge of the frame would have its outer half shaved off.
    let painter = ui.painter_at(canvas_geom::overlay_clip(img_rect));
    // ⚠⚠ **Only the COMMITTED outlines are suppressed; every gesture still works and the
    // in-progress preview is still drawn.** Hiding them is for judging what still needs
    // retouching, so placing a patch while they are hidden must behave normally — and a gesture
    // with no preview would be blind again, which is the complaint this same round fixed. A
    // toggle that disabled the tool would be a mode, which is not what was asked for.
    // ⚠ Per patch since 2026-09-20: the row's eye. A flag past the end — a patch that has not
    // been through `sync_spot_flags` yet — draws, as outlines always did.
    for (i, sp) in s
        .edits
        .spots
        .iter()
        .enumerate()
        .filter(|(i, _)| s.spot_shown.get(*i).copied().unwrap_or(true))
    {
        let selected = s.selected_spot == Some(i);
        let c = to_screen(&map, sp.centre, frame_rect);
        let src = to_screen(&map, sp.source, frame_rect);
        // ⚠ `radius_scale_at` is the spot space's long edge in frame pixels **where the patch
        // sits** — the quantity the stage multiplies by, carried through the keystone, which
        // magnifies one end of the frame and shrinks the other. So the destination and the
        // source are each sized at their own position.
        let r = sp.radius * map.radius_scale_at(sp.centre);
        let r_src = sp.radius * map.radius_scale_at(sp.source);
        // ⚠ Doubled: a dark halo under a light mark. A thin white circle is invisible against a
        // bright sky — the fix the crop overlay needed after being reported.
        //
        // ⚠⚠ **An unselected patch is drawn much fainter than a selected one**, and the gap is
        // deliberately large: on a frame with thirty patches the outlines otherwise become the
        // picture. Reported as wanting them dimmer, alongside the toggle that removes them
        // entirely.
        let (halo, mark) = if selected { (140, 255) } else { (70, 90) };
        for (w, col) in [
            (3.0, egui::Color32::from_black_alpha(halo)),
            (1.0, egui::Color32::from_white_alpha(mark)),
        ] {
            let stroke = egui::Stroke::new(w, col);
            painter.line_segment([c, src], stroke);
            circle(&painter, c, r, stroke);
            circle(&painter, src, r_src, stroke);
        }
    }

    // ⚠⚠ **The gesture in progress is drawn too, and without it the drag is blind** — reported as
    // "no preview of what's happening, no way to gauge the size". Nothing is committed until the
    // release, so until this existed a size-drag showed the photograph and nothing else.
    //
    // ⚠ It reads `tool.radius`, the *same* value the drag just wrote and the panel's Size slider
    // shows, so the circle, the slider and the spot that lands cannot disagree.
    match s.spot_drag {
        SpotDrag::Sizing { at, to } if (to - at).length() > CLICK_SLOP => {
            preview(
                &painter,
                at,
                None,
                tool.radius * map.radius_scale_at(to_source(at)),
            );
        }
        // ⚠ An ⌥-drag keeps the stored radius and is choosing a *source*, so both discs are drawn
        // with a line between them: the gesture's whole content is the pairing.
        SpotDrag::Sourcing { at, to } if (to - at).length() > CLICK_SLOP => {
            preview(
                &painter,
                at,
                Some(to),
                tool.radius * map.radius_scale_at(to_source(at)),
            );
        }
        _ => {}
    }

    changed
}

/// The gesture in progress: a destination circle, optionally a source circle and the line to it.
///
/// ⚠ Dashed-thin rather than solid, so a preview never reads as a placed patch. Same doubled
/// stroke as the committed spots, for the same reason: a light mark alone vanishes on a bright sky.
fn preview(painter: &egui::Painter, at: egui::Pos2, source: Option<egui::Pos2>, r: f32) {
    for (w, col) in [
        (3.0, egui::Color32::from_black_alpha(140)),
        (1.5, egui::Color32::from_white_alpha(230)),
    ] {
        let stroke = egui::Stroke::new(w, col);
        circle(painter, at, r, stroke);
        if let Some(src) = source {
            circle(painter, src, r, stroke);
            painter.line_segment([at, src], stroke);
        } else {
            // ⚠ A radial tick from the centre out to the rim: it is what makes the *size* legible
            // while dragging, since a circle alone gives the eye nothing to measure against.
            painter.line_segment([at, at + egui::vec2(r, 0.0)], stroke);
        }
    }
}

fn to_screen(map: &SpotMap, p: [f32; 2], frame: egui::Rect) -> egui::Pos2 {
    let (x, y) = map.point(p);
    frame.min + egui::vec2(x, y)
}

fn circle(painter: &egui::Painter, c: egui::Pos2, r: f32, stroke: egui::Stroke) {
    let pts: Vec<egui::Pos2> = (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let t = k as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            c + egui::vec2(r * t.cos(), r * t.sin())
        })
        .collect();
    painter.add(egui::Shape::closed_line(pts, stroke));
}

/// Place a spot, auto-picking the source and falling back to a fixed offset.
///
/// ⚠ The fallback is **3r straight up** (or down, when up would leave the frame), clamped: a spot
/// the user can drag beats a click that silently did nothing.
fn place_with_source(
    centre: [f32; 2],
    tool: SpotTool,
    existing: &[edit_model::Spot],
    aspect: f32,
    sample: &dyn Fn([f32; 2]) -> [f32; 3],
) -> edit_model::Spot {
    let source = rustroom_app::spot_source::pick(sample, centre, tool.radius, aspect, existing)
        .unwrap_or_else(|| {
            let up = centre[1] - 3.0 * tool.radius;
            let y = if up >= 0.0 {
                up
            } else {
                (centre[1] + 3.0 * tool.radius).min(1.0)
            };
            [centre[0], y]
        });
    spot_tool::place(centre, source, tool)
}

/// A sampler over the spot-removal stage's input space, built from the session.
///
/// ⚠⚠ **`SpotMap::inverse_point` returns post-lens coordinates while `s.buffer` is the raw
/// decode.** Indexing one with the other is wrong by the distortion — ~3.5% at the corners, which
/// on a 6246 px frame is ~220 px and therefore larger than a spot radius. The search would sample a
/// different piece of the picture and return a confidently wrong source, silently. The lens stage
/// exposes its inverse, which is exactly the direction needed.
///
/// ⚠ **No homography step since M7.3b**: spots live BEFORE the perspective now, so the only map
/// between spot space and the decode is the lens.
///
/// ⚠⚠ **A new approximation M7.3b introduced, stated rather than argued away.** Noise reduction
/// and dehaze now run AHEAD of spots, and this sampler replays neither, so the auto-pick scores
/// candidates on pixels the stage never sees. Noise reduction barely matters to a ring mean. Dehaze
/// is spatially varying and **could** change which candidate scores best. Unmeasured. Replaying it
/// per click would cost a whole prefix, and a wrong pick costs one drag of the source.
///
/// ⚠ A missing correction is the identity, not an error: a JPEG has no lens profile, so this
/// degrades to a plain buffer read.
/// ⚠ Returns an **owned** closure: everything it needs is cloned or `Copy`, so it does not borrow
/// the session and can therefore be built before `draw_spot_overlay` takes `&mut Session`. The
/// buffer is behind an `Arc`, so "cloning" it copies a pointer.
pub(super) fn sampler_for(s: &Session) -> impl Fn([f32; 2]) -> [f32; 3] {
    let buf = s.buffer.clone();
    let (w, h) = (buf.width(), buf.height());
    let corr = s.lens.shot.as_ref().and_then(|shot| {
        lens_profile::Corrector::new(
            &shot.profile,
            &shot.shot,
            w,
            h,
            lens_profile::Corrections {
                distortion: s.edits.applied_lens_correction().distortion,
                tca: s.edits.applied_lens_correction().tca,
                vignetting: s.edits.applied_lens_correction().vignetting,
                scale: s.edits.applied_lens_correction().scale,
            },
        )
    });
    move |p: [f32; 2]| {
        // stage input, `[0, 1]` normalised → pixels
        let (x, y) = (p[0] * w as f32, p[1] * h as f32);
        match &corr {
            // → lens-correction input, in pixels, **per channel** because TCA displaces them
            // differently. Reading each channel at its own coordinate costs nothing and is more
            // faithful than picking green for all three.
            Some(c) => {
                let per = c.source(x, y);
                let mut out = [0.0f32; 3];
                for (ch, o) in out.iter_mut().enumerate() {
                    *o = read(&buf, per[ch][0], per[ch][1])[ch];
                }
                out
            }
            None => read(&buf, x, y),
        }
    }
}

fn read(buf: &core_image::ImageBuf, x: f32, y: f32) -> [f32; 3] {
    if buf.width() == 0 || buf.height() == 0 {
        return [0.0; 3];
    }
    let xi = (x.round() as i64).clamp(0, buf.width() as i64 - 1) as usize;
    let yi = (y.round() as i64).clamp(0, buf.height() as i64 - 1) as usize;
    buf.pixel(xi, yi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::canvas::test_support::{retouch_app, spots};

    /// ⚠ A click places exactly one spot, and the auto-source must put the source somewhere else:
    /// a source equal to the destination copies the blemish onto itself.
    #[test]
    fn a_retouch_click_places_one_spot_with_a_source_elsewhere() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        let placed = spots(&app);
        assert_eq!(placed.len(), 1, "the click did not place exactly one spot");
        assert_ne!(
            placed[0].source, placed[0].centre,
            "the source sits on the destination"
        );
    }

    /// ⚠ A second click on a placed spot **selects** it rather than stacking another on top —
    /// the hit-test-before-placement rule, and what makes a crowded frame editable.
    #[test]
    fn clicking_a_placed_spot_selects_it_rather_than_stacking() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(spots(&app).len(), 1, "the second click stacked a spot");
        assert_eq!(
            app.session.as_ref().unwrap().selected_spot,
            Some(0),
            "the second click did not select the spot it landed on"
        );
    }

    /// ⚠⚠ A placed spot must be ONE history step. Before M9.11 the overlay changed the edits and
    /// reported the change, but never pushed — so the action log recorded a step the history did
    /// not have, and Ctrl+Z popped the step *before* the spot too.
    #[test]
    fn placing_a_spot_is_one_history_step() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let depth = app.session.as_ref().unwrap().history.undo_depth();
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().unwrap();
        assert_eq!(
            s.edits.spots.len(),
            1,
            "fixture check: the click placed a spot"
        );
        assert_eq!(
            s.history.undo_depth(),
            depth + 1,
            "the placement was not recorded"
        );
        assert_eq!(
            s.history.get(),
            &s.edits,
            "history's present is not the edits"
        );
    }

    /// And a move is one step for the whole gesture, not one per drag frame.
    #[test]
    fn moving_a_spot_is_one_history_step() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        let depth = app.session.as_ref().unwrap().history.undo_depth();
        let before = spots(&app)[0].centre;
        crate::develop::canvas::test_support::drag_through(
            &mut app,
            &ctx,
            centre,
            centre + egui::vec2(30.0, 0.0),
        );
        let s = app.session.as_ref().unwrap();
        assert_ne!(
            s.edits.spots[0].centre, before,
            "fixture check: the drag moved the spot"
        );
        assert_eq!(
            s.history.undo_depth(),
            depth + 1,
            "a move was not exactly one step"
        );
        assert_eq!(s.history.get(), &s.edits);
    }

    /// ⚠⚠ The render is sampled **on the drag frame itself** (`DRAG_PASS`), not after the
    /// gesture. A later frame issues one anyway, and in the real app that frame never comes
    /// because egui idles after a drag — which is exactly how the M3.1a mask bug hid.
    #[test]
    fn a_retouch_drag_sizes_the_spot_and_asks_for_a_render() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        let before = app.spot_tool.radius;
        let counts = crate::develop::canvas::test_support::drag_through(
            &mut app,
            &ctx,
            centre,
            centre + egui::vec2(60.0, 0.0),
        );
        let placed = spots(&app);
        assert_eq!(placed.len(), 1, "the drag did not place exactly one spot");
        assert!(
            placed[0].radius > before,
            "the drag did not size the spot: {} vs {before}",
            placed[0].radius
        );
        // ⚠ And the tool keeps it, so the panel's Size slider follows the gesture.
        assert!(
            (app.spot_tool.radius - placed[0].radius).abs() < 1e-6,
            "the drag did not write the radius back into the tool"
        );
        // ⚠ A *placement* happens on release, so its render belongs on the release frame — not
        // the drag frame, which is where a **move** must request (see the test below). Asserting
        // "somewhere in the gesture" would be the weak form this project warns about, so the
        // frame is named: nothing before it, a request on it.
        assert_eq!(
            counts[crate::develop::canvas::test_support::DRAG_PASS],
            counts[0],
            "a placement rendered before the gesture finished: {counts:?}"
        );
        assert!(
            *counts.last().unwrap() > counts[crate::develop::canvas::test_support::DRAG_PASS],
            "the placement never asked for a render: {counts:?}"
        );
    }

    /// ⚠⚠ **Spots live BEFORE the perspective since M7.3b**, so a click on a keystoned photo must
    /// store the homography's *source* of the clicked point — `SpotMap::inverse_point` — and not
    /// `FrameMap`'s answer, which is the post-perspective point the brush would use.
    ///
    /// ⚠ The crop moves the clicked point off the frame centre, where a keystone might leave it
    /// in place, and the fixture check below proves the two answers differ before comparing.
    #[test]
    fn a_click_on_a_keystoned_photo_lands_in_pre_perspective_space() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let crop = edit_model::CropRect {
            left: 0.5,
            top: 0.3,
            right: 1.0,
            bottom: 1.0,
        };
        let persp = edit_model::Perspective {
            vertical: 50.0,
            horizontal: 50.0,
            scale: 0.0,
            ..Default::default()
        };
        {
            let s = app.session.as_mut().expect("a session");
            s.edits.geometry.crop = crop;
            s.edits.perspective = persp;
            // ⚠⚠ A frame that has already LANDED at the cropped size. No render runs in this
            // harness, so `fit_image_size` would otherwise still be the uncropped 80x60 and the
            // canvas frame would carry the wrong aspect. A crop alone cannot see that (the
            // cropped-photo test above passes either way); a keystone can, because the homography
            // is defined in half-height units. The straighten test's `shown_geo` does the same.
            s.fit_image_size = egui::vec2(40.0, 42.0);
        }
        // The 80x60 test photo cropped to its right half and lower 70%: a 40x42 frame. Only the
        // aspect matters to the normalised answer, so native pixels are as good as screen ones.
        let geometry = edit_model::Geometry {
            crop,
            ..Default::default()
        };
        let (fw, fh) = (40usize, 42usize);
        let want = pipeline::brush::SpotMap::new(&geometry, persp, fw, fh)
            .inverse_point(fw as f32 / 2.0, fh as f32 / 2.0);
        let brush = pipeline::brush::FrameMap::new(&geometry, fw, fh)
            .inverse_point(fw as f32 / 2.0, fh as f32 / 2.0);
        assert!(
            (want[0] - brush[0]).abs() + (want[1] - brush[1]).abs() > 0.02,
            "fixture check: the keystone must move the point ({want:?} vs {brush:?})"
        );

        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        let placed = spots(&app);
        assert_eq!(placed.len(), 1, "the click placed nothing");
        let got = placed[0].centre;
        assert!(
            (got[0] - want[0]).abs() < 0.02 && (got[1] - want[1]).abs() < 0.02,
            "stored {got:?}, pre-perspective {want:?}, post-perspective (the old answer) {brush:?}"
        );
    }

    /// ⚠⚠ **A spot must land where you clicked on a CROPPED photo**, which is what
    /// `FrameMap::inverse_point` buys and what nothing else here can see. Spots are stored in
    /// pre-geometry source coordinates while the canvas shows the cropped frame, so treating a
    /// canvas position as if it were already a source position is invisible on an uncropped photo
    /// and wrong on every cropped one.
    ///
    /// With the right half of the frame cropped, the centre of what is shown is source x = 0.75.
    /// Skipping the map would store 0.5.
    #[test]
    fn a_click_on_a_cropped_photo_lands_where_it_was_clicked() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.edits.geometry.crop = edit_model::CropRect {
                left: 0.5,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            };
        }
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        let placed = spots(&app);
        assert_eq!(placed.len(), 1, "the click placed nothing");
        assert!(
            (placed[0].centre[0] - 0.75).abs() < 0.02,
            "the crop was not undone: stored x = {} (0.5 means the map was skipped)",
            placed[0].centre[0]
        );
    }

    /// How many shapes one frame of the canvas paints.
    ///
    /// ⚠ The overlay is *appearance*, which this harness cannot assert on directly — but egui's
    /// `FullOutput` carries the shapes it painted, and a count is enough to see that a whole
    /// group of outlines stopped being drawn.
    fn painted(app: &mut crate::RustroomApp, ctx: &egui::Context) -> usize {
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
        out.shapes.len()
    }

    /// ⚠⚠ **Turning the outlines off must actually stop drawing them.** Reported as wanting a way
    /// to "remove the skeletons... such that one can better see the picture": with thirty patches
    /// the circles become the picture. A test on the flag alone would pass against a canvas that
    /// ignored it entirely, which is exactly what a sabotage found.
    #[test]
    fn hiding_the_patches_paints_fewer_shapes() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(spots(&app).len(), 1, "setup: nothing was placed");

        // ⚠ Per patch since 2026-09-20: the row's eye, `Session::spot_shown`.
        app.session.as_mut().unwrap().spot_shown = vec![true];
        let shown = painted(&mut app, &ctx);
        app.session.as_mut().unwrap().spot_shown = vec![false];
        let hidden = painted(&mut app, &ctx);
        assert!(
            hidden < shown,
            "hiding the patches painted just as much: {hidden} vs {shown}"
        );
        // ⚠ And the patch itself survives — this is a *view* toggle, not a delete.
        assert_eq!(spots(&app).len(), 1, "hiding the outlines removed a patch");
    }

    /// ⚠⚠ **A non-primary drag must place nothing.** egui reports `dragged()` for *every* button,
    /// so a tool reading a bare `dragged()` would place a spot as the middle-drag pan gesture
    /// passed over the canvas. `tool_drag` is the guard, and no test using Primary alone can see
    /// it removed — this one was added because that sabotage went uncaught.
    #[test]
    fn a_middle_drag_places_no_spot_and_pans_instead() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::drag_through_with(
            &mut app,
            &ctx,
            centre,
            centre + egui::vec2(60.0, 0.0),
            egui::PointerButton::Middle,
        );
        assert!(
            spots(&app).is_empty(),
            "the pan gesture placed a retouch spot"
        );
    }

    /// ⚠⚠ **The patch must leave the VIEW while it is being moved**, so the blemish underneath is
    /// visible and the new position can be lined up against it. Reported as "it shows the previous
    /// retouching there, I think it should just disappear, so I can once again better align the
    /// positions".
    ///
    /// ⚠ Both halves are asserted. That it disappears is half the property; that it comes back on
    /// release is the other, and without that a moved patch would stay invisible for good — the
    /// worse bug of the two.
    #[test]
    fn a_moving_patch_leaves_the_view_and_returns_on_release() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(spots(&app).len(), 1, "setup: nothing was placed");

        // ⚠ Sampled *during* the drag rather than after it: the suppression exists only for the
        // gesture's duration, so a check afterwards could only ever see it cleared.
        let mut during = None;
        for pass in 0..4 {
            let pos = if pass <= 1 {
                centre
            } else {
                centre + egui::vec2(30.0, 0.0)
            };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pass == 1 {
                events.push(egui::Event::PointerButton {
                    pos: centre,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            if pass == 3 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                });
            }
            let c = ctx.clone();
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::develop::canvas::test_support::screen()),
                    events,
                    ..Default::default()
                },
                |ui| {
                    crate::develop::canvas::show(&mut app, ui, &c);
                },
            );
            if pass == crate::develop::canvas::test_support::DRAG_PASS {
                during = app.session.as_ref().unwrap().suppressed_spot;
            }
        }
        assert_eq!(
            during,
            Some(0),
            "the patch stayed in the view while it was being moved"
        );
        assert_eq!(
            app.session.as_ref().unwrap().suppressed_spot,
            None,
            "the patch never came back after the move"
        );
    }

    /// ⚠ The suppression is *view* state: it must never reach the stored edits, or a moved patch
    /// would be deleted rather than hidden.
    #[test]
    fn suppressing_a_patch_leaves_the_stored_edits_alone() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        let stored = spots(&app);
        app.session.as_mut().unwrap().suppressed_spot = Some(0);
        let _ = painted(&mut app, &ctx);
        assert_eq!(spots(&app), stored, "suppression edited the stored spots");
        // ⚠ And the *view* really does leave it out, or the mechanism is inert.
        let s = app.session.as_ref().unwrap();
        assert!(
            s.view_edits(rustroom_app::tool::Tool::Retouch)
                .spots
                .is_empty(),
            "the suppressed patch is still in the rendered edits"
        );
    }

    /// ⚠⚠ **Moving a spot must NOT render while the drag runs — the moving circle is the
    /// feedback.** This reverses an earlier version of this very test, and the reversal was forced
    /// by using the tool: "moving around the retouch patch is very laggy, as it adapts the
    /// retouching on the go".
    ///
    /// The reason is structural rather than a matter of taste. `SpotRemovalStage` lives in the
    /// **prefix**, so moving a spot drops the prefix cache and re-runs noise reduction and dehaze
    /// over the whole buffer — ~117 ms each at native resolution, every frame of the drag. A mask
    /// handle can render live because `MaskStage` is in the cheap tail; the M3.1a rule was right
    /// for masks and wrong here.
    ///
    /// ⚠ Both halves are asserted. "No render during the drag" alone would pass against a version
    /// that never rendered at all and left the patch permanently stale.
    #[test]
    fn dragging_a_placed_spot_renders_only_on_release() {
        let ctx = egui::Context::default();
        let mut app = retouch_app(&ctx);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        assert_eq!(spots(&app).len(), 1, "setup: the click placed nothing");
        let before = spots(&app)[0].centre;

        let counts = crate::develop::canvas::test_support::drag_through(
            &mut app,
            &ctx,
            centre,
            centre + egui::vec2(25.0, 15.0),
        );
        let after = spots(&app);
        assert_eq!(after.len(), 1, "the drag stacked a second spot");
        assert_ne!(after[0].centre, before, "the drag did not move the spot");
        assert_eq!(
            counts[crate::develop::canvas::test_support::DRAG_PASS],
            counts[crate::develop::canvas::test_support::DRAG_PASS - 1],
            "the move re-rendered mid-drag, which is what made it laggy: {counts:?}"
        );
        assert!(
            *counts.last().unwrap() > counts[crate::develop::canvas::test_support::DRAG_PASS],
            "the move never rendered at all, so the patch is left stale: {counts:?}"
        );
    }
}
