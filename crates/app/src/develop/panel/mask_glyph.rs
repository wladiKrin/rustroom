//! Hand-painted glyphs for the five mask kinds, drawn into a small rect.
//!
//! ⚠ Not the icon font. The Phosphor codepoints for these five are not known here, and a
//! guessed one ships a tofu box — four features in this project have done exactly that. Five
//! primitives-only pictures are cheap, deterministic and testable.

use eframe::egui;
use rustroom_app::mask_kind::MaskKind;

/// Paint `kind`'s glyph inside `rect`, in `colour`.
pub fn paint(painter: &egui::Painter, rect: egui::Rect, kind: MaskKind, colour: egui::Color32) {
    let r = rect.shrink(rect.width() * 0.12);
    let c = r.center();
    let w = r.width();
    let stroke = egui::Stroke::new((w / 9.0).max(1.0), colour);
    match kind {
        // Three lines fading downward: the ramp.
        MaskKind::Gradient => {
            for (i, a) in [1.0, 0.6, 0.3].into_iter().enumerate() {
                let y = r.top() + w * (0.2 + 0.3 * i as f32);
                painter.line_segment(
                    [egui::pos2(r.left(), y), egui::pos2(r.right(), y)],
                    egui::Stroke::new(stroke.width, colour.gamma_multiply(a)),
                );
            }
        }
        // An ellipse inside an ellipse.
        MaskKind::Radial => {
            ellipse(painter, c, w * 0.5, w * 0.36, stroke);
            ellipse(
                painter,
                c,
                w * 0.22,
                w * 0.15,
                egui::Stroke::new(stroke.width, colour.gamma_multiply(0.5)),
            );
        }
        // A short thick diagonal stroke with a dot at its tip.
        MaskKind::Brush => {
            painter.line_segment(
                [
                    egui::pos2(r.left() + w * 0.2, r.bottom() - w * 0.2),
                    egui::pos2(r.right() - w * 0.25, r.top() + w * 0.25),
                ],
                egui::Stroke::new(stroke.width * 2.2, colour),
            );
            painter.circle_filled(
                egui::pos2(r.left() + w * 0.16, r.bottom() - w * 0.16),
                stroke.width * 1.4,
                colour,
            );
        }
        // A circle with its left half filled: dark to light.
        MaskKind::Luminance => {
            let radius = w * 0.45;
            let left = egui::Rect::from_min_max(r.min, egui::pos2(c.x, r.max.y));
            painter
                .with_clip_rect(left)
                .circle_filled(c, radius, colour);
            painter.circle_stroke(c, radius, stroke);
        }
        // A drop: a circle with a point on top.
        MaskKind::Colour => {
            let radius = w * 0.3;
            let cc = egui::pos2(c.x, r.bottom() - radius - stroke.width);
            painter.circle_filled(cc, radius, colour);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(c.x, r.top() + stroke.width * 0.5),
                    egui::pos2(cc.x - radius * 0.95, cc.y - radius * 0.25),
                    egui::pos2(cc.x + radius * 0.95, cc.y - radius * 0.25),
                ],
                colour,
                egui::Stroke::NONE,
            ));
        }
    }
}

/// An ellipse as a closed polyline.
fn ellipse(painter: &egui::Painter, c: egui::Pos2, rx: f32, ry: f32, stroke: egui::Stroke) {
    const N: usize = 24;
    let pts: Vec<egui::Pos2> = (0..N)
        .map(|i| {
            let t = i as f32 / N as f32 * std::f32::consts::TAU;
            egui::pos2(c.x + rx * t.cos(), c.y + ry * t.sin())
        })
        .collect();
    painter.add(egui::Shape::closed_line(pts, stroke));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_paints_something() {
        // ⚠ A glyph that painted nothing would leave a blank cell beside a name and nothing
        // else would notice. Counted from the frame's shapes, kind by kind.
        for k in MaskKind::ALL {
            let ctx = egui::Context::default();
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(100.0, 100.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let rect =
                        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(14.0, 14.0));
                    paint(ui.painter(), rect, k, egui::Color32::WHITE);
                },
            );
            fn count(s: &egui::Shape) -> usize {
                match s {
                    egui::Shape::Vec(v) => v.iter().map(count).sum(),
                    egui::Shape::Noop => 0,
                    _ => 1,
                }
            }
            let n: usize = out.shapes.iter().map(|c| count(&c.shape)).sum();
            assert!(n > 0, "{k:?} painted nothing");
        }
    }
}
