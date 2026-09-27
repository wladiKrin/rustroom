//! The red-eye overlay (M9.11): every eye's circle, and the gestures that place, move and resize
//! them.
//!
//! ⚠ `spot_overlay`'s rules, all of them, for its reasons: coordinates against `frame_rect`, never
//! `img_rect`; eyes live in spot space and are mapped forward through [`SpotMap`]; the click is a
//! separate click-sensing interaction registered after `resp`; a drag renders at its start and its
//! end only, because `RedEyeStage` is in the prefix; and **one history push per committed gesture,
//! made here**, with the edit reported only when that push recorded a step.

use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::canvas_geom;
use crate::Session;
use edit_model::EyeKind;
use eframe::egui;
use pipeline::brush::SpotMap;
use rustroom_app::eye_detect::{self, Detection};
use rustroom_app::eye_tool::{self, EyeTool, Grab};
use rustroom_app::spot_tool;
use rustroom_app::tool::Tool;

/// A drag shorter than this, in screen points, is a click that wandered (`spot_overlay`'s value).
const CLICK_SLOP: f32 = 3.0;
/// The smallest a rim may be to grab, in screen points (`spot_overlay`'s value).
const GRAB_POINTS: f32 = 7.0;
/// Segments in a drawn circle.
const CIRCLE_SEGMENTS: usize = 48;

/// What the pointer is doing with eyes, across frames.
#[derive(Clone, Copy, PartialEq, Default)]
pub(crate) enum EyeDrag {
    #[default]
    None,
    /// A new eye sized by the drag, without detection: press and latest position, in screen
    /// points — carried because egui often reports no pointer on the release frame.
    Sizing { at: egui::Pos2, to: egui::Pos2 },
    /// An existing eye, and whether the grab moves or resizes it.
    Editing(usize, Grab),
}

/// Draw every eye, and run the gestures when the tool is in an eye mode. Returns `true` only when
/// a history step was recorded; `status` receives the "nothing detected" message.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_eye_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    tool: &mut EyeTool,
    sample: &dyn Fn([f32; 2]) -> [f32; 3],
    status: &mut Option<String>,
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
    let min_grab = if map.radius_scale > 0.0 {
        GRAB_POINTS / map.radius_scale
    } else {
        0.0
    };
    // The stage's input frame in pixels: the decode's size, which lens correction keeps.
    let dims = (s.buffer.width(), s.buffer.height());
    let to_source = |p: egui::Pos2| -> [f32; 2] {
        let (nx, ny) = canvas_geom::normalized_point(p, frame_rect);
        map.inverse_point(nx * frame_rect.width(), ny * frame_rect.height())
    };
    let to_screen = |p: [f32; 2]| -> egui::Pos2 {
        let (x, y) = map.point(p);
        frame_rect.min + egui::vec2(x, y)
    };
    let mut changed = false;

    if let Some(kind) = tool.active {
        // ---- the click: select an eye, or detect and place one ----
        let click = ui
            .interact(
                img_rect,
                ui.id().with("red_eye_click"),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::Crosshair);
        if click.clicked() {
            if let Some(p) = click.interact_pointer_pos() {
                let at = to_source(p);
                // ⚠ Hit-test BEFORE placement, so a click on an eye selects it rather than
                // stacking a second one on top.
                match eye_tool::hit_test(&s.edits.eyes, at, aspect, min_grab) {
                    Some((i, _)) => select(s, tool, i),
                    None => {
                        let found = eye_detect::detect(at, kind, dims, sample);
                        if found == Detection::NotFound {
                            *status = Some(not_found_message(kind));
                        }
                        let light = catchlight_for(tool, at, p, &to_source, dims);
                        s.edits
                            .eyes
                            .push(eye_tool::place(found, at, kind, tool, light));
                        select(s, tool, s.edits.eyes.len() - 1);
                        changed = true;
                    }
                }
            }
        }

        // ---- the drags ----
        if tool_drag_started(resp) {
            if let Some(p) = resp.interact_pointer_pos() {
                s.eye_drag = match eye_tool::hit_test(&s.edits.eyes, to_source(p), aspect, min_grab)
                {
                    Some((i, g)) => {
                        select(s, tool, i);
                        // The eye leaves the view for the gesture, so the pupil shows.
                        s.suppressed_eye = Some(i);
                        s.request(
                            s.view_edits(Tool::Retouch),
                            s.render_max_dim(),
                            s.requested_region,
                        );
                        EyeDrag::Editing(i, g)
                    }
                    None => EyeDrag::Sizing { at: p, to: p },
                };
            }
        }
        if tool_drag(resp) {
            if let Some(p) = resp.interact_pointer_pos() {
                match s.eye_drag {
                    EyeDrag::Sizing { at, .. } => s.eye_drag = EyeDrag::Sizing { at, to: p },
                    EyeDrag::Editing(i, g) => {
                        // ⚠ Both ends of the delta through the same inverse — a cropped or
                        // straightened frame is not a plain scale (`spot_overlay`'s note).
                        let now = to_source(p);
                        let was = to_source(p - resp.drag_delta());
                        if let Some(e) = s.edits.eyes.get_mut(i) {
                            match g {
                                Grab::Move => {
                                    e.centre[0] += now[0] - was[0];
                                    e.centre[1] += now[1] - was[1];
                                }
                                Grab::Resize => {
                                    e.radius = spot_tool::drag_radius(e.centre, now, aspect)
                                        .clamp(spot_tool::MIN_RADIUS, spot_tool::MAX_RADIUS);
                                    tool.radius = e.radius;
                                }
                            }
                        }
                    }
                    EyeDrag::None => {}
                }
            }
        }
        if resp.drag_stopped() {
            let live = resp.interact_pointer_pos();
            match s.eye_drag {
                // ⚠ A drag that never moved is left to the click path — egui runs the whole drag
                // lifecycle for a plain click, and without this one click placed two eyes.
                EyeDrag::Sizing { at, to } if (live.unwrap_or(to) - at).length() > CLICK_SLOP => {
                    let c = to_source(at);
                    tool.radius = spot_tool::drag_radius(c, to_source(live.unwrap_or(to)), aspect)
                        .clamp(spot_tool::MIN_RADIUS, spot_tool::MAX_RADIUS);
                    let light = catchlight_for(tool, c, at, &to_source, dims);
                    s.edits
                        .eyes
                        .push(eye_tool::place(Detection::NotFound, c, kind, tool, light));
                    select(s, tool, s.edits.eyes.len() - 1);
                    changed = true;
                }
                EyeDrag::Editing(..) => changed = true,
                _ => {}
            }
            if s.eye_drag != EyeDrag::None {
                // Cleared before the request, or the render would still leave the eye out.
                s.suppressed_eye = None;
                s.request(
                    s.view_edits(Tool::Retouch),
                    s.render_max_dim(),
                    s.requested_region,
                );
                s.eye_drag = EyeDrag::None;
            }
        }
    }
    let changed = changed && s.history.push(s.edits.clone());

    // ---- the overlay ----
    let painter = ui.painter_at(canvas_geom::overlay_clip(img_rect));
    for (i, e) in s.edits.eyes.iter().enumerate() {
        let (halo, mark) = if s.selected_eye == Some(i) {
            (140, 255)
        } else {
            (70, 110)
        };
        let c = to_screen(e.centre);
        let r = e.radius * map.radius_scale_at(e.centre);
        for (w, col) in [
            (3.0, egui::Color32::from_black_alpha(halo)),
            (1.0, egui::Color32::from_white_alpha(mark)),
        ] {
            circle(&painter, c, r, egui::Stroke::new(w, col));
        }
        // A centre dot tells an eye from a patch outline at a glance.
        painter.circle_filled(c, 1.5, egui::Color32::from_white_alpha(mark));
    }
    if let EyeDrag::Sizing { at, to } = s.eye_drag {
        if (to - at).length() > CLICK_SLOP {
            let c = to_source(at);
            let r = spot_tool::drag_radius(c, to_source(to), aspect) * map.radius_scale_at(c);
            for (w, col) in [
                (3.0, egui::Color32::from_black_alpha(140)),
                (1.5, egui::Color32::from_white_alpha(230)),
            ] {
                circle(&painter, at, r, egui::Stroke::new(w, col));
            }
        }
    }
    changed
}

/// Select eye `i`, drop the patch selection, and let the tool show that eye's kind.
fn select(s: &mut Session, tool: &mut EyeTool, i: usize) {
    s.selected_eye = Some(i);
    s.selected_spot = None;
    if let Some(e) = s.edits.eyes.get(i) {
        tool.active = Some(e.kind);
    }
}

/// A new pet eye's catchlight: toward the point 10 screen points up-left of `screen`, mapped into
/// source space — so it sits up-left **as displayed**, whatever the orientation.
fn catchlight_for(
    tool: &EyeTool,
    centre: [f32; 2],
    screen: egui::Pos2,
    to_source: &dyn Fn(egui::Pos2) -> [f32; 2],
    dims: (usize, usize),
) -> Option<[f32; 2]> {
    tool.catchlight.then(|| {
        eye_tool::catchlight_offset(centre, to_source(screen + egui::vec2(-10.0, -10.0)), dims)
    })
}

fn not_found_message(kind: EyeKind) -> String {
    let what = match kind {
        EyeKind::Red => "red pupil",
        EyeKind::Pet => "bright eyeshine",
    };
    format!("No {what} found near the click — placed a circle; drag its edge to resize.")
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
