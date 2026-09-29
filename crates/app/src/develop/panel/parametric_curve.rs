//! The tone curve's **Parametric** view (M5.1c): Lightroom's four region sliders over three splits.
//!
//! ⚠ The bin half only: every rule — which region a point is in, how a split moves, how a drag
//! scales — lives in `rustroom_app::tone_curve_tool`, tested with no window.
//!
//! ⚠⚠ **The drawn curve is the COMBINED one** (parametric, then the composite points), sampled through
//! the same constructor a render uses, so what you see is the function applied.

use super::tone_curve::{backdrop, CurveChannel};
use crate::develop::panel::Edited;
use crate::theme;
use eframe::egui;
use rustroom_app::settings::ParametricDrag;
use rustroom_app::tone_curve_tool::{self as tool, Region};
use crate::theme::Hover as _;

/// The widget's largest side, matching the point view so switching modes does not jump.
const MAX_SIZE: f32 = 220.0;
/// Samples along the plotted curve.
const SAMPLES: usize = 96;
/// The strip along the bottom edge that grabs a split handle, in points.
const HANDLE_STRIP: f32 = 14.0;
/// How close to a split the pointer must be to grab it, in points.
const HANDLE_GRAB: f32 = 8.0;

/// What a drag started on: a split handle, or a region's amount (with its value at the start).
#[derive(Clone, Copy, Debug)]
enum Grab {
    Split(usize),
    Region(Region, f32),
}

/// The Reset for the parametric curve: its four regions and three boundaries back to neutral, and
/// disabled while they are there. Drawn in the curve block's header (`tone_curve::header`).
pub fn reset_button(ui: &mut egui::Ui, param: &mut edit_model::ParametricCurve, ed: &mut Edited) {
    let r = ui
        .add_enabled(
            *param != edit_model::ParametricCurve::default(),
            egui::Button::new("Reset"),
        )
        .hover("Resets the four regions and their boundaries");
    if r.clicked() {
        *param = edit_model::ParametricCurve::default();
        ed.mark_changed();
    }
}

/// The Parametric view. ⚠ Its header, Reset and the selectors above it are the caller's — see
/// `tone_curve::selectors`.
pub fn show(
    ui: &mut egui::Ui,
    param: &mut edit_model::ParametricCurve,
    points: &[[f32; 2]],
    ed: &mut Edited,
    hist: Option<&[[u32; 256]; 3]>,
    binding: ParametricDrag,
) -> egui::Rect {
    let full = ui.available_width();
    let size = full.min(MAX_SIZE);
    let pad = ((full - size) * 0.5).max(0.0);
    let (rect, resp) = {
        let mut allocated = None;
        ui.horizontal(|ui| {
            ui.add_space(pad);
            allocated =
                Some(ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag()));
        });
        allocated.expect("the horizontal closure always runs")
    };
    // ⚠ How to work it is the hover (`docs/DESIGN.md` §8); the line under it carries only a value.
    let resp = resp.hover(match binding {
        ParametricDrag::Left => {
            "Drag the handles below to move the region boundaries, or drag on the curve to adjust a region"
        }
        ParametricDrag::Off => "Drag the handles below to move the region boundaries",
    });
    let norm_x = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let at = |n: [f32; 2]| {
        egui::pos2(
            rect.left() + n[0] * rect.width(),
            rect.bottom() - n[1] * rect.height(),
        )
    };

    // ---- the gestures -------------------------------------------------------------------------
    let grab_id = resp.id.with("parametric-grab");
    let mut grab: Option<Grab> = ui.data_mut(|d| d.get_temp(grab_id)).flatten();
    let press = ui.input(|i| i.pointer.press_origin());
    if resp.drag_started_by(egui::PointerButton::Primary) {
        grab = press.and_then(|p| {
            // ⚠ A split handle wins when the press is on the bottom strip near one; otherwise the
            // region under the pointer, if the binding allows the on-curve drag at all.
            let near = (0..3).min_by(|&a, &b| {
                let da = (at([param.splits[a], 0.0]).x - p.x).abs();
                let db = (at([param.splits[b], 0.0]).x - p.x).abs();
                da.total_cmp(&db)
            })?;
            let on_strip = p.y >= rect.bottom() - HANDLE_STRIP;
            if on_strip && (at([param.splits[near], 0.0]).x - p.x).abs() <= HANDLE_GRAB {
                Some(Grab::Split(near))
            } else if binding == ParametricDrag::Left {
                let r = tool::region_at(param.splits, norm_x(p.x));
                Some(Grab::Region(r, *r.amount(param)))
            } else {
                None
            }
        });
    }
    if resp.dragged_by(egui::PointerButton::Primary) {
        if let (Some(g), Some(p), Some(origin)) = (grab, resp.interact_pointer_pos(), press) {
            match g {
                Grab::Split(i) => tool::drag_split(&mut param.splits, i, norm_x(p.x)),
                Grab::Region(r, start) => {
                    *r.amount(param) = tool::drag_amount(start, p.y - origin.y, rect.height());
                }
            }
            ed.changed = true;
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        grab = None;
        ed.settled = true;
    }
    ui.data_mut(|d| d.insert_temp(grab_id, grab));

    // ---- the picture --------------------------------------------------------------------------
    let hovered = resp
        .hover_pos()
        .map(|p| tool::region_at(param.splits, norm_x(p.x)));
    let active = match grab {
        Some(Grab::Region(r, _)) => Some(r),
        _ => hovered,
    };
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let painter = ui.painter_at(rect);
        backdrop(&painter, rect, v, hist, CurveChannel::Rgb);
        // The region under the pointer (or being dragged), shaded so the drag's target is visible.
        if let Some(r) = active {
            let s = pipeline::parametric::sanitised_splits(param.splits);
            let bounds = [0.0, s[0], s[1], s[2], 1.0];
            let k = Region::ALL.iter().position(|&x| x == r).unwrap_or(0);
            let band = egui::Rect::from_min_max(
                egui::pos2(at([bounds[k], 0.0]).x, rect.top()),
                egui::pos2(at([bounds[k + 1], 0.0]).x, rect.bottom()),
            );
            painter.rect_filled(band, 0.0, v.selection.bg_fill.gamma_multiply(0.18));
        }
        let plotted: Vec<egui::Pos2> = tool::combined_points(points, param, SAMPLES)
            .into_iter()
            .map(at)
            .collect();
        painter.add(egui::Shape::line(
            plotted,
            egui::Stroke::new(1.5, v.strong_text_color()),
        ));
        // The three split handles: small triangles standing on the bottom edge.
        for (i, &sx) in param.splits.iter().enumerate() {
            let x = at([sx, 0.0]).x;
            let lit = matches!(grab, Some(Grab::Split(j)) if j == i);
            let colour = if lit {
                v.selection.bg_fill
            } else {
                v.strong_text_color()
            };
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(x, rect.bottom() - HANDLE_STRIP + 4.0),
                    egui::pos2(x - 5.0, rect.bottom() - 1.0),
                    egui::pos2(x + 5.0, rect.bottom() - 1.0),
                ],
                colour,
                egui::Stroke::NONE,
            ));
        }
    }

    // ---- the four sliders ---------------------------------------------------------------------
    for r in Region::ALL {
        ed.help_scope = rustroom_app::slider_help::Scope::Parametric;
        ed.slider(ui, r.amount(param), -100.0..=100.0, r.label(), 0.0);
        ed.help_scope = rustroom_app::slider_help::Scope::Global;
    }
    theme::hint(
        ui,
        &match (active, binding) {
            (Some(r), _) => format!("{}: {:+.0}", r.label(), {
                let mut p = *param;
                *r.amount(&mut p)
            }),
            (None, _) => String::new(),
        },
    );
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drag from `from` to `to` (normalised widget space, y up) over the Parametric view — the
    /// point view's five-pass harness, for its reason: egui resolves a press against the widgets
    /// laid out on the PREVIOUS pass.
    fn gesture(
        from: [f32; 2],
        to: [f32; 2],
        binding: ParametricDrag,
    ) -> (edit_model::ParametricCurve, Edited) {
        let ctx = egui::Context::default();
        let mut param = edit_model::ParametricCurve::default();
        let mut ed = Edited::default();
        let mut rect = egui::Rect::NOTHING;
        let at = |r: egui::Rect, n: [f32; 2]| {
            egui::pos2(r.left() + n[0] * r.width(), r.bottom() - n[1] * r.height())
        };
        for pass in 0..5 {
            let (a, b) = (at(rect, from), at(rect, to));
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 700.0),
                )),
                ..Default::default()
            };
            let press = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            match pass {
                1 => input.events.push(egui::Event::PointerMoved(a)),
                2 => input.events.push(press(a, true)),
                3 => input.events.push(egui::Event::PointerMoved(b)),
                4 => input.events.push(press(b, false)),
                _ => {}
            }
            let _ = ctx.run_ui(input, |ui| {
                rect = show(ui, &mut param, &[], &mut ed, None, binding);
            });
        }
        (param, ed)
    }

    /// ⚠⚠ Both directions, in two different regions, so a flipped sign or a drag that always
    /// lands on one region cannot pass.
    #[test]
    fn a_drag_on_the_curve_moves_the_region_under_the_pointer() {
        let (up, ed) = gesture([0.12, 0.5], [0.12, 0.8], ParametricDrag::Left);
        assert!(up.shadows > 20.0, "a drag up in the shadows gave {up:?}");
        assert_eq!((up.darks, up.lights, up.highlights), (0.0, 0.0, 0.0));
        assert!(ed.changed && ed.settled, "one settled edit");

        let (down, _) = gesture([0.9, 0.5], [0.9, 0.3], ParametricDrag::Left);
        assert!(
            down.highlights < -10.0,
            "a drag down in the highlights gave {down:?}"
        );
        assert_eq!((down.shadows, down.darks, down.lights), (0.0, 0.0, 0.0));
    }

    #[test]
    fn with_the_binding_off_a_drag_on_the_curve_does_nothing() {
        let (p, _) = gesture([0.12, 0.5], [0.12, 0.8], ParametricDrag::Off);
        assert_eq!(p, edit_model::ParametricCurve::default());
    }

    /// A press on the bottom strip at a split grabs the split, not the region — and the split moves.
    #[test]
    fn a_drag_on_a_split_handle_moves_the_split() {
        let (p, _) = gesture([0.5, 0.01], [0.62, 0.01], ParametricDrag::Left);
        assert!((p.splits[1] - 0.62).abs() < 0.03, "{:?}", p.splits);
        assert_eq!(
            (p.shadows, p.darks, p.lights, p.highlights),
            (0.0, 0.0, 0.0, 0.0)
        );
    }
}
