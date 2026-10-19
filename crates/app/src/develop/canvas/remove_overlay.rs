//! Retouch's Remove mode on the canvas (M10.16c1): the brush under the pointer, a stroke painted over
//! what to remove, a click to pick one, and every removal drawn — the selected one lit, the ones still
//! being filled shaded until their fill lands.
//!
//! ⚠ Removals live in DECODE space (`edit_model::Removal`), so the screen maps through
//! `ai_space::DecodeMap` both ways: `frame_to_decode` for the pointer, `decode_to_frame` for drawing.

use super::navigation::{tool_drag, tool_drag_started};
use crate::Session;
use eframe::egui;
use rustroom_app::remove_tool::{self, RemoveTool};
use rustroom_app::tool::Tool;

/// A press that moved less than this many points is a click, left to the click path — the eye
/// overlay's rule, for its reason: egui runs the whole drag lifecycle for a plain click.
const CLICK_SLOP: f32 = 3.0;

/// The Remove gesture across frames — on the session for `spot_drag`'s reason: `show` runs fresh every
/// frame. And which removal is selected — view state, never `Edits`.
#[derive(Debug, Default, Clone)]
pub(crate) struct RemovalUi {
    pub(crate) selected: Option<usize>,
    /// The stroke being painted, decode-normalised.
    pub(crate) stroke: Vec<[f32; 2]>,
    /// Where the press began, on screen — what tells a stroke from a click.
    pub(crate) press: Option<egui::Pos2>,
}

/// The screen ↔ decode mapping for this frame.
fn decode_map(s: &Session, frame_rect: egui::Rect) -> rustroom_app::ai_space::DecodeMap {
    let (dw, dh) = (s.buffer.width(), s.buffer.height());
    rustroom_app::ai_space::DecodeMap::new(
        &s.edits.geometry,
        s.edits.perspective,
        frame_rect.width() as usize,
        frame_rect.height() as usize,
        s.lens
            .shot
            .as_ref()
            .and_then(|l| rustroom_app::ai_space::corrector_for(l, &s.edits, dw, dh)),
        dw,
        dh,
    )
}

/// One removal as the panel and the canvas place it: the tool's brush and Detect, full Opacity.
fn removal(points: Vec<[f32; 2]>, tool: &RemoveTool) -> edit_model::Removal {
    edit_model::Removal {
        points,
        radius: tool.radius,
        detect: tool.detect,
        variant: 0,
        opacity: 100.0,
    }
}

/// The Remove gesture and drawing. Returns whether a removal was added and recorded as one undo step.
pub(super) fn draw_remove_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    tool: &RemoveTool,
) -> bool {
    if frame_rect.width() <= f32::EPSILON || frame_rect.height() <= f32::EPSILON {
        return false;
    }
    let (dw, dh) = (s.buffer.width(), s.buffer.height());
    let dm = decode_map(s, frame_rect);
    let to_decode =
        |p: egui::Pos2| dm.frame_to_decode(p.x - frame_rect.left(), p.y - frame_rect.top());
    let to_screen = |d: [f32; 2]| {
        let (x, y) = dm.decode_to_frame(d);
        egui::pos2(frame_rect.left() + x, frame_rect.top() + y)
    };
    // A decode radius on screen, measured at `at` — the lens and the zoom both scale it.
    let screen_radius = |at: [f32; 2], radius: f32| {
        let step = radius * dw.max(dh) as f32 / dw.max(1) as f32;
        (to_screen([at[0] + step, at[1]]) - to_screen(at)).length()
    };
    let mut changed = false;

    // ⚠ A click-sensing interaction over the drag-sensing canvas — the spot overlay's arrangement and
    // its reason: the canvas's own `resp` never reports `clicked()`.
    let click = ui
        .interact(img_rect, ui.id().with("remove_click"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::Crosshair);
    if click.clicked() {
        if let Some(p) = click.interact_pointer_pos() {
            let at = to_decode(p);
            match remove_tool::hit_test(&s.edits.removals, at, dw, dh) {
                Some(i) => s.removal.selected = Some(i),
                None if at.iter().all(|v| (0.0..=1.0).contains(v)) => {
                    s.edits.removals.push(removal(vec![at], tool));
                    s.removal.selected = Some(s.edits.removals.len() - 1);
                    changed = true;
                }
                None => {}
            }
            s.selected_spot = None;
            s.selected_eye = None;
        }
    }

    if tool_drag_started(resp) {
        s.removal.stroke.clear();
        s.removal.press = resp.interact_pointer_pos();
    }
    if tool_drag(resp) {
        if let Some(p) = resp.interact_pointer_pos() {
            let at = to_decode(p);
            remove_tool::add_point(&mut s.removal.stroke, at, tool.radius, dw, dh);
        }
    }
    if resp.drag_stopped() {
        let points = std::mem::take(&mut s.removal.stroke);
        let moved = match (s.removal.press.take(), resp.interact_pointer_pos()) {
            (Some(a), Some(b)) => (b - a).length() > CLICK_SLOP,
            (Some(_), None) => points.len() > 1,
            _ => false,
        };
        // ⚠ A press that never moved is the click path's; anything further is one stroke, one removal.
        if moved && !points.is_empty() {
            s.edits.removals.push(removal(points, tool));
            s.removal.selected = Some(s.edits.removals.len() - 1);
            s.selected_spot = None;
            s.selected_eye = None;
            changed = true;
        }
    }
    if changed {
        s.request(
            s.view_edits(Tool::Retouch),
            s.render_max_dim(),
            s.requested_region,
        );
    }
    // ⚠ The overlay records its own history step and reports the edit only if one was recorded — the
    // spot overlay's rule.
    let changed = changed && s.history.push(s.edits.clone());

    // ---- the overlay ------------------------------------------------------------------------
    let painter = ui.painter_at(crate::develop::canvas_geom::overlay_clip(img_rect));
    let filled: Vec<bool> = {
        let maps = s.engine.ai_maps();
        let maps = maps.read().expect("ai maps");
        s.edits
            .removals
            .iter()
            .map(|r| maps.fill(r).is_some())
            .collect()
    };
    let selected = s.removal.selected.filter(|&i| i < s.edits.removals.len());
    for (i, r) in s.edits.removals.iter().enumerate() {
        let Some(&first) = r.points.first() else {
            continue;
        };
        let rad = screen_radius(first, r.radius);
        let pts: Vec<egui::Pos2> = r.points.iter().map(|&p| to_screen(p)).collect();
        // ⚠ A removal still being filled is shaded — the object is still in the picture under it; a
        // filled one is only outlined, faintly unless selected (the spot overlay's dimming).
        let fill = if !filled[i] {
            Some(egui::Color32::from_white_alpha(70))
        } else if selected == Some(i) {
            Some(egui::Color32::from_white_alpha(35))
        } else {
            None
        };
        band(&painter, &pts, rad, fill, selected == Some(i));
    }
    if !s.removal.stroke.is_empty() {
        let pts: Vec<egui::Pos2> = s.removal.stroke.iter().map(|&p| to_screen(p)).collect();
        let rad = screen_radius(s.removal.stroke[0], tool.radius);
        band(
            &painter,
            &pts,
            rad,
            Some(egui::Color32::from_white_alpha(70)),
            true,
        );
    }
    // The brush under the pointer.
    if let Some(p) = click.hover_pos().or_else(|| resp.hover_pos()) {
        let rad = screen_radius(to_decode(p), tool.radius);
        for (w, col) in [
            (3.0, egui::Color32::from_black_alpha(140)),
            (1.0, egui::Color32::from_white_alpha(255)),
        ] {
            painter.circle_stroke(p, rad, egui::Stroke::new(w, col));
        }
    }
    changed
}

/// A stroke's band on screen: `fill` laid along it at its radius, and its outline — bright when lit.
fn band(
    painter: &egui::Painter,
    pts: &[egui::Pos2],
    rad: f32,
    fill: Option<egui::Color32>,
    lit: bool,
) {
    if let Some(col) = fill {
        if pts.len() == 1 {
            painter.circle_filled(pts[0], rad, col);
        } else {
            painter.add(egui::Shape::line(
                pts.to_vec(),
                egui::Stroke::new(2.0 * rad, col),
            ));
        }
    }
    let (halo, mark) = if lit { (140, 255) } else { (70, 90) };
    for (w, col) in [
        (3.0, egui::Color32::from_black_alpha(halo)),
        (1.0, egui::Color32::from_white_alpha(mark)),
    ] {
        let stroke = egui::Stroke::new(w, col);
        if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
            painter.circle_stroke(*a, rad, stroke);
            if pts.len() > 1 {
                painter.circle_stroke(*b, rad, stroke);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::develop::canvas::test_support::{canvas_centre, drag_through, retouch_app};
    use eframe::egui;

    /// Retouch open in Remove, its default mode.
    fn remove_app(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = retouch_app(ctx);
        app.remove_tool = Default::default();
        assert!(app.remove_tool.active, "Remove is the default mode");
        app
    }

    /// One drag across the photo: one removal, its points along the drag, one undo step, selected.
    #[test]
    fn a_stroke_makes_one_removal_and_one_undo_step() {
        let ctx = egui::Context::default();
        let mut app = remove_app(&ctx);
        app.remove_tool.radius = 0.02;
        let c = canvas_centre(&mut app, &ctx);
        let depth = app.session.as_ref().unwrap().history.undo_depth();
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        let s = app.session.as_ref().unwrap();
        assert_eq!(s.edits.removals.len(), 1, "one stroke, one removal");
        let r = &s.edits.removals[0];
        assert!(
            r.points.len() >= 2,
            "the stroke followed the drag: {:?}",
            r.points
        );
        assert!(
            r.points[1][0] > r.points[0][0],
            "left to right: {:?}",
            r.points
        );
        assert_eq!(r.radius, 0.02);
        assert_eq!(s.history.undo_depth(), depth + 1, "one undo step");
        assert_eq!(s.removal.selected, Some(0));
        assert!(s.edits.spots.is_empty(), "no patch placed");
    }

    /// A drag in Heal mode places no removal — the mode decides.
    #[test]
    fn a_drag_in_heal_places_no_removal() {
        let ctx = egui::Context::default();
        let mut app = remove_app(&ctx);
        app.remove_tool.active = false;
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        assert!(app.session.as_ref().unwrap().edits.removals.is_empty());
    }
}
