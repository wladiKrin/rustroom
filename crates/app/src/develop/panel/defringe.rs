//! The Lens section's Defringe block (M9.6): the eyedropper, and an amount and a hue range for each
//! of purple and green.
//!
//! ⚠ The bin half only: which hue belongs to which fringe, how a pick sets a range, how a handle or
//! the band drags — all in `rustroom_app::defringe_tool`, tested with no window.

use crate::develop::panel::Edited;
use crate::theme;
use eframe::egui;
use rustroom_app::defringe_tool as tool;

/// The hue-range widget's height, in points.
const STRIP_H: f32 = 18.0;
/// How close to a handle a press must land to grab it, in points.
const HANDLE_GRAB: f32 = 6.0;

pub fn show(
    ui: &mut egui::Ui,
    d: &mut edit_model::Defringe,
    picking: Option<&mut bool>,
    ed: &mut Edited,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Defringe").strong());
        if let Some(picking) = picking {
            // ⚠ The white-balance eyedropper's shape: a toggle that arms one canvas click.
            if ui
                .selectable_label(*picking, "Pick a fringe")
                .on_hover_text(
                    "Click a purple or green fringe on the photo: its hue sets that colour's range",
                )
                .clicked()
            {
                *picking = !*picking;
            }
        }
    });
    ed.slider(ui, &mut d.purple_amount, 0.0..=100.0, "Purple amount");
    if hue_range(ui, "defringe_purple", &mut d.purple_hue, ed) {
        ed.changed = true;
    }
    ed.slider(ui, &mut d.green_amount, 0.0..=100.0, "Green amount");
    if hue_range(ui, "defringe_green", &mut d.green_hue, ed) {
        ed.changed = true;
    }
}

/// What a drag on the strip holds.
#[derive(Clone, Copy, Debug)]
enum Grab {
    Handle(usize),
    /// The whole band, with the range and pointer position at the press.
    Band([f32; 2], f32),
}

/// A dual-handle hue range over an Oklab hue gradient. Returns whether it changed this frame.
fn hue_range(ui: &mut egui::Ui, id: &str, range: &mut [f32; 2], ed: &mut Edited) -> bool {
    let width = ui.available_width();
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, STRIP_H), egui::Sense::click_and_drag());
    let to_x = |deg: f32| rect.left() + deg / 360.0 * rect.width();
    let to_deg = |x: f32| ((x - rect.left()) / rect.width() * 360.0).clamp(0.0, 360.0);

    let grab_id = ui.id().with(id).with("grab");
    let mut grab: Option<Grab> = ui.data_mut(|m| m.get_temp(grab_id)).flatten();
    let mut changed = false;
    if resp.drag_started() {
        grab = ui.input(|i| i.pointer.press_origin()).map(|p| {
            let (lo, hi) = (to_x(range[0]), to_x(range[1]));
            if (p.x - lo).abs() <= HANDLE_GRAB {
                Grab::Handle(0)
            } else if (p.x - hi).abs() <= HANDLE_GRAB {
                Grab::Handle(1)
            } else {
                Grab::Band(*range, p.x)
            }
        });
    }
    if resp.dragged() {
        if let (Some(g), Some(p)) = (grab, resp.interact_pointer_pos()) {
            let next = match g {
                Grab::Handle(i) => tool::drag_handle(*range, i, to_deg(p.x)),
                Grab::Band(start, x0) => tool::drag_band(start, (p.x - x0) / rect.width() * 360.0),
            };
            changed = next != *range;
            *range = next;
        }
    }
    if resp.drag_stopped() {
        grab = None;
        ed.settled = true;
    }
    ui.data_mut(|m| m.insert_temp(grab_id, grab));

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        // The hue gradient, drawn from the real Oklab → sRGB conversion at a mid lightness and
        // moderate chroma, so the strip shows the hues the range selects.
        let steps = 72;
        for k in 0..steps {
            let h0 = k as f32 / steps as f32 * 360.0;
            let h = (h0 + 2.5).to_radians();
            let lin = color::oklab_to_linear_srgb([0.7, 0.12 * h.cos(), 0.12 * h.sin()]);
            let c = lin.map(|v| (color::linear_to_srgb(v) * 255.0).round() as u8);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(to_x(h0), rect.top() + 4.0),
                    egui::pos2(to_x(h0 + 360.0 / steps as f32) + 0.5, rect.bottom() - 4.0),
                ),
                0.0,
                egui::Color32::from_rgb(c[0], c[1], c[2]).gamma_multiply(0.35),
            );
        }
        let band = egui::Rect::from_min_max(
            egui::pos2(to_x(range[0]), rect.top() + 2.0),
            egui::pos2(to_x(range[1]), rect.bottom() - 2.0),
        );
        let v = ui.visuals();
        painter.rect_stroke(
            band,
            2.0,
            egui::Stroke::new(1.5, v.strong_text_color()),
            egui::StrokeKind::Inside,
        );
        for x in [band.left(), band.right()] {
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(2.5, v.strong_text_color()),
            );
        }
    }
    theme::hint(
        ui,
        &format!(
            "Hue {:.0}°–{:.0}° — drag an end to widen, the middle to move",
            range[0], range[1]
        ),
    );
    changed
}
