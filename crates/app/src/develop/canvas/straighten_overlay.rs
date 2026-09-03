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
use pipeline::perspective::guides;
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

    // ⚠⚠ **`shown_geo`, not `s.edits`** — the transform the *visible* texture was rendered with.
    // Perspective and geometry are prefix stages, so changing them drops the prefix cache: at
    // native zoom the re-render costs hundreds of milliseconds, and mapping through the
    // *requested* edits put the guides at their new positions over the previous image. Reported
    // there as the line jumping on release when fully zoomed in.
    let (shown_persp, shown_angle) = s.shown_geo;
    let hom = pipeline::perspective::Homography::from_edits(&shown_persp, aspect);
    let angle_rad = shown_angle.to_radians();

    // ⚠⚠ **The cursor is on the *corrected* frame; the line is stored on the raw one.** That is
    // `guide_overlay.rs`'s exact pattern, and it is what makes the line read **level** after
    // release: the raw endpoints survive the rotation that is about to be applied, and drawing
    // them forward through the new angle puts them back on the feature they were traced onto.
    let raw_at = |p: egui::Pos2| {
        let screen = [
            ((p.x - frame_rect.left()) / frame_rect.width()).clamp(0.0, 1.0),
            ((p.y - frame_rect.top()) / frame_rect.height()).clamp(0.0, 1.0),
        ];
        guides::from_frame(screen, &hom, angle_rad, aspect)
    };
    let to_screen = |raw: [f32; 2]| {
        let n = guides::to_frame(raw, &hom, angle_rad, aspect);
        egui::pos2(
            frame_rect.left() + n[0] * frame_rect.width(),
            frame_rect.top() + n[1] * frame_rect.height(),
        )
    };

    if let Some(p) = resp.interact_pointer_pos() {
        let at = raw_at(p);
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
            // The measurement stays here — it needs the drawn endpoints and the live transform.
            // Everything that follows from the *answer* lives in `settle`, so the panel's tests
            // can reach it without a canvas.
            // ⚠ Measured on the **corrected** frame — that is the line the user drew. The stored
            // endpoints stay raw so they survive the rotation that is about to be applied.
            let sf = guides::to_frame(from, &hom, angle_rad, aspect);
            let st = guides::to_frame(to, &hom, angle_rad, aspect);
            if let Some(a) = rustroom_app::straighten_line::angle_for_line(sf, st, aspect) {
                // ⚠⚠ The new angle is **relative** to the one already applied: the line was drawn
                // on a frame that already carries `shown_angle`, so levelling it means rotating by
                // that much *more*. Without the addition, straightening a second time would undo
                // the first correction rather than refine it.
                s.straighten_line = Some((from, to));
                edited = settle(s, shown_angle + a, tool);
            }
            // ⚠ Disarmed after one line, the eyedropper's recorded rule: a mode that stayed armed
            // would turn every later drag on the canvas — including one meant to move the crop —
            // into a rotation. Disarmed even when the line was degenerate, so a stray click does
            // not leave the tool silently armed.
            s.straighten_armed = false;
        }
    }

    // Draw the band in progress, else the settled line mapped through the correction — which is
    // what makes it read **level** after release, the only feedback that says a subtle 0.4°
    // correction worked at all.
    //
    // ⚠ `img_rect` is still the right *clip*: there is no photograph outside it to annotate, and
    // `overlay_clip` widens it because `line_segment` centres its stroke on the path — a tight
    // clip shaves the outer half of a halo at the frame edge.
    let shown = s.straighten_drag.or(s.straighten_line);
    if let Some((from, to)) = shown {
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
        for q in [a, b] {
            p.circle_filled(q, 5.0, egui::Color32::from_black_alpha(180));
            p.circle_filled(q, 3.0, egui::Color32::from_rgb(255, 80, 80));
        }
    }
    edited
}

/// Apply a straighten angle the way a released line does: clamp it, request, push one history
/// entry, and clear the guides.
///
/// Returns whether a photo edit settled into a new history entry.
///
/// ⚠ Split out of the release path so the *rule* has one home and the panel's tests can reach it
/// without driving a canvas — the `apply_cell_action` / `name_edit` pattern, where two routes into
/// one act must not be able to drift.
pub(super) fn settle(s: &mut Session, angle: f32, tool: Tool) -> bool {
    // ⚠ Clamped to the Straighten slider's own range, which `angle_for_line` alone cannot do: it
    // bounds the *relative* angle, and a sum of two in-range values is not in range. A value
    // outside ±45 is one the slider could neither show nor return to.
    let want = angle.clamp(-45.0, 45.0);

    // ⚠⚠ **A straighten line is a hand edit through another affordance, so it clears the guides.**
    // `panel/geometry.rs` states the rule for the slider: "editing by hand throws the guides away,
    // because the solve no longer describes what you see." Straighten is one of the three values a
    // solve writes, so a line leaves any drawn guides describing a correction that no longer holds
    // — and a stale overlay is worse than none, because it gets trusted.
    //
    // ⚠ The panel's own hand-edit check structurally **cannot** do this for us: it captures
    // `solved_before` earlier in the same frame, so an angle written by the canvas on the
    // *previous* frame is already in `s.edits` and never reads as an edit.
    //
    // ⚠ The **sliders are deliberately left alone**, which is `Clear`'s own contract — "Remove all
    // guides. The sliders they produced are left alone." Clearing them here would throw away a
    // perspective correction the user never asked to undo.
    s.guides.clear();
    s.active_guide = None;

    if s.edits.geometry.angle == want {
        return false;
    }
    s.edits.geometry.angle = want;
    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    s.history.push(s.edits.clone());
    true
}
