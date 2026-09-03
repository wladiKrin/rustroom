//! The straighten-line overlay: drag along something that should be level, and on release the
//! photo rotates so it is.
//!
//! ⚠⚠ **Every coordinate here is against `frame_rect`, never `img_rect`** — the rule the mask and
//! guide overlays both carry, for the same reason: once a region render is on, `img_rect` covers
//! only the rendered *slice*, so a line placed against it drifts with the view while panning.
//! `img_rect` survives only as the clip, widened by `overlay_clip` because `line_segment` centres
//! its stroke on the path and a tight clip shaves the outer half of a halo at the frame edge.

use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::canvas_geom;
use crate::Session;
use eframe::egui;
use rustroom_app::tool::Tool;

/// Draw the straighten line and apply the angle it asks for on release.
///
/// Returns whether a photo edit settled into a new history entry — the same contract
/// `draw_crop_overlay` has, so `show` folds it into `photo_edited` and one gesture stays one
/// Ctrl+Z.
///
/// ⚠ Applies the edit itself rather than reporting it outward, which the guide overlay cannot do:
/// a guide *solve* needs `&mut RustroomApp`, where everything here — `edits`, `request`, `history`
/// — is on `Session`, which this already holds. `pick_white_balance` takes `tool` for the same
/// reason and does the same thing.
pub(super) fn draw_straighten_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    frame_rect: egui::Rect,
    img_rect: egui::Rect,
    tool: Tool,
) -> bool {
    // ⚠ The aspect must come from the FRAME, not from `img_rect`: a region's aspect is not the
    // frame's, and taking it from the painted rect silently skews the angle at any zoom. The
    // guide overlay records the identical trap.
    let aspect = frame_rect.width() / frame_rect.height().max(1.0);

    let norm = |p: egui::Pos2| {
        [
            ((p.x - frame_rect.left()) / frame_rect.width()).clamp(0.0, 1.0),
            ((p.y - frame_rect.top()) / frame_rect.height()).clamp(0.0, 1.0),
        ]
    };
    let to_screen = |n: [f32; 2]| {
        egui::pos2(
            frame_rect.left() + n[0] * frame_rect.width(),
            frame_rect.top() + n[1] * frame_rect.height(),
        )
    };

    if let Some(p) = resp.interact_pointer_pos() {
        let at = norm(p);
        if tool_drag_started(resp) {
            s.straighten_drag = Some((at, at));
        }
        if tool_drag(resp) {
            if let Some((_, to)) = s.straighten_drag.as_mut() {
                *to = at;
                // ⚠⚠ **Deliberately NOT applied here.** The guide overlay records that solving
                // live "was built and rejected in use — reported as 'jiggling too much': the whole
                // photo shifts under the cursor on every pixel of movement." The rubber band is
                // the feedback; the rotation lands on release, so the image is frozen for the
                // whole drag.
            }
        }
    }

    let mut edited = false;
    // ⚠⚠ **Read OUTSIDE the `interact_pointer_pos` block.** On the release frame egui often
    // reports no interact position, and the brush canvas records that nesting a `drag_stopped`
    // there "silently skipped both the render and the history push."
    if resp.drag_stopped() {
        if let Some((from, to)) = s.straighten_drag.take() {
            if let Some(a) = rustroom_app::straighten_line::angle_for_line(from, to, aspect) {
                if s.edits.geometry.angle != a {
                    s.edits.geometry.angle = a;
                    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
                    s.history.push(s.edits.clone());
                    edited = true;
                }
            }
            // ⚠ Disarmed after one line, the eyedropper's recorded rule: a mode that stayed armed
            // would turn every later drag on the canvas — including one meant to move the crop —
            // into a rotation. Disarmed even when the line was degenerate, so a stray click does
            // not leave the tool silently armed.
            s.straighten_armed = false;
        }
    }

    // Draw the band in progress. ⚠ `img_rect` is still the right *clip*: there is no photograph
    // outside it to annotate, and `overlay_clip` widens it because `line_segment` centres its
    // stroke on the path — a tight clip shaves the outer half of a halo at the frame edge.
    if let Some((from, to)) = s.straighten_drag {
        let p = ui.painter_at(canvas_geom::overlay_clip(img_rect));
        let (a, b) = (to_screen(from), to_screen(to));
        // ⚠ Halo then mark, the crop overlay's convention: "a white line is invisible against a
        // bright photograph."
        p.line_segment(
            [a, b],
            egui::Stroke::new(5.0, egui::Color32::from_black_alpha(180)),
        );
        p.line_segment(
            [a, b],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 80, 80)),
        );
    }
    edited
}
