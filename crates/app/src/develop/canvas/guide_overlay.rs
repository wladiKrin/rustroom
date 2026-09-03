//! The perspective-guide overlay: tracing lines onto the photograph, and the loupe that makes
//! placing an endpoint precise.

use super::loupe::draw_loupe;
use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::canvas_geom;
use crate::Session;
use eframe::egui;

/// How close, in normalised frame units, a press must be to grab a guide endpoint.
const GUIDE_GRAB: f32 = 0.03;

/// What a frame of guide interaction produced.
///
/// ⚠⚠ Only `Settled` re-solves. `Dragging` exists so the caller knows a gesture is in progress
/// and can leave the edits — and therefore the image — **frozen** until it ends.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum GuideDrag {
    None,
    Dragging,
    Settled,
}

/// Draw the perspective guides and apply any drag.
///
/// ⚠⚠ Guides are stored on the **raw** frame and drawn mapped through the live correction, so a
/// line stays on the feature it was traced onto while the photo is being corrected underneath it.
pub(super) fn draw_guide_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    frame_rect: egui::Rect,
    img_rect: egui::Rect,
) -> GuideDrag {
    use pipeline::perspective::guides::{self, Family, Guide};

    // ⚠⚠ **Every coordinate here is against `frame_rect`, never `img_rect`** — the rule the mask
    // overlay already carries, and for the same reason: once zoomed, `img_rect` covers only the
    // *rendered slice*, so guides placed against it drift with the view while panning. ⚠ And the
    // aspect must come from the frame too: **a region's aspect is not the frame's**, and taking it
    // from `img_rect` silently skews the solve at any zoom.
    let aspect = frame_rect.width() / frame_rect.height().max(1.0);
    // ⚠⚠ The transform the pixels are taking right now. Guides are stored on the **raw** frame, so
    // they must be mapped through it to stay on the features they were traced onto — that is what
    // makes a live solve usable rather than disorienting.
    // ⚠⚠ **`shown_geo`, not `s.edits`** — the transform the *visible* texture was rendered with.
    // Perspective is a prefix stage, so changing it drops the prefix cache: at native zoom the
    // re-render costs hundreds of milliseconds, and mapping through the requested edits put the
    // guides at their new positions over the previous image. Reported as the line jumping
    // slightly on release when fully zoomed in, and looking right again once zoomed out — the
    // same normalised error, magnified.
    //
    // ⚠ It also gives the frozen-during-a-drag behaviour for nothing: no request is issued while
    // dragging, so `shown_geo` cannot move.
    let (shown_persp, shown_angle) = s.shown_geo;
    let hom = pipeline::perspective::Homography::from_edits(&shown_persp, aspect);
    let angle = shown_angle.to_radians();
    let shown: Vec<([f32; 2], [f32; 2])> = s
        .guides
        .iter()
        .map(|g| {
            (
                guides::to_frame(g.from, &hom, angle, aspect),
                guides::to_frame(g.to, &hom, angle, aspect),
            )
        })
        .collect();

    let to_screen = |n: [f32; 2]| {
        egui::pos2(
            frame_rect.left() + n[0] * frame_rect.width(),
            frame_rect.top() + n[1] * frame_rect.height(),
        )
    };
    // ⚠ `img_rect` is still the right *clip*: there is no photograph outside it to annotate.
    let marks = ui.painter_at(canvas_geom::overlay_clip(img_rect));
    let halo = egui::Color32::from_black_alpha(180);
    for (g, (a, b)) in s.guides.iter().zip(&shown) {
        // ⚠ The inferred family is **shown**, so a misclassified 45° line is visible and fixed by
        // nudging an endpoint. A rule the user cannot see is one they cannot correct.
        let colour = match g.family() {
            Family::Vertical => egui::Color32::from_rgb(80, 200, 255),
            Family::Horizontal => egui::Color32::from_rgb(255, 190, 70),
        };
        let (a, b) = (to_screen(*a), to_screen(*b));
        marks.line_segment([a, b], egui::Stroke::new(5.0, halo));
        marks.line_segment([a, b], egui::Stroke::new(2.0, colour));
        for p in [a, b] {
            marks.circle_filled(p, 6.0, halo);
            marks.circle_filled(p, 4.0, colour);
        }
    }

    // ⚠ Clamped to the frame, so a drag that leaves the picture pins the endpoint to its edge
    // rather than placing a guide somewhere that does not exist.
    let norm = |p: egui::Pos2| {
        [
            ((p.x - frame_rect.left()) / frame_rect.width()).clamp(0.0, 1.0),
            ((p.y - frame_rect.top()) / frame_rect.height()).clamp(0.0, 1.0),
        ]
    };

    // ⚠ Right-click removes one guide, so a single bad line does not cost you the whole set.
    // Read before the drag handling: a secondary press is not a drag and would otherwise fall
    // through to creating a guide.
    if resp.secondary_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(i) = canvas_geom::guide_near(&shown, norm(p), GUIDE_GRAB) {
                s.guides.remove(i);
                s.active_guide = None;
                return GuideDrag::Settled;
            }
        }
        return GuideDrag::None;
    }

    // ⚠ The loupe: a magnified crop around the endpoint being placed, so an edge can be traced
    // precisely without the whole canvas being at 1:1. Drawn last, over everything.
    if let (Some((i, e)), Some(tex)) = (s.active_guide, s.texture.as_ref()) {
        if let Some(seg) = shown.get(i) {
            let at = to_screen(if e == 0 { seg.0 } else { seg.1 });
            draw_loupe(&marks, tex, at, img_rect, frame_rect);
        }
    }

    let mut out = GuideDrag::None;
    if let Some(p) = resp.interact_pointer_pos() {
        let screen = norm(p);
        // ⚠ The cursor is on the *corrected* frame; a guide lives on the raw one.
        let raw = guides::from_frame(screen, &hom, angle, aspect);
        if tool_drag_started(resp) {
            s.active_guide = canvas_geom::guide_endpoint_at_shown(&shown, screen, GUIDE_GRAB)
                .or_else(|| {
                    // ⚠ A new guide starts as a zero-length line whose *`to`* end follows the
                    // cursor, so the very first drag frame extends it — no separate "creating"
                    // state.
                    s.guides.push(Guide { from: raw, to: raw });
                    Some((s.guides.len() - 1, 1))
                });
        }
        if tool_drag(resp) {
            if let Some((i, e)) = s.active_guide {
                if let Some(g) = s.guides.get_mut(i) {
                    if e == 0 {
                        g.from = raw;
                    } else {
                        g.to = raw;
                    }
                }
                // ⚠⚠ **Deliberately NOT solved here.** Re-solving every frame was built and
                // rejected in use — reported as "jiggling too much": the whole photo shifts under
                // the cursor on every pixel of movement, and with two guides in play a small
                // nudge swings the correction hard. Leaving the edits alone freezes the image,
                // and because the guides are drawn through those same edits the frozen transform
                // holds them still too. The dragged endpoint follows the cursor exactly.
                out = GuideDrag::Dragging;
            }
        }
        if resp.drag_stopped() {
            // ⚠ A tap that never became a drag leaves a zero-length guide, which has no direction
            // and would classify arbitrarily. Drop it rather than keeping a line nobody can see.
            if let Some((i, _)) = s.active_guide {
                let dead = s.guides.get(i).is_some_and(|g| {
                    (g.to[0] - g.from[0]).abs() < 1e-4 && (g.to[1] - g.from[1]).abs() < 1e-4
                });
                if dead {
                    s.guides.remove(i);
                }
            }
            s.active_guide = None;
            out = GuideDrag::Settled;
        }
    }
    out
}
