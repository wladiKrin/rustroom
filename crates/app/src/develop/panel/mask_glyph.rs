//! Hand-painted glyphs for the mask kinds, drawn into a small rect.
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
        // A head and shoulders: the subject.
        MaskKind::Subject => {
            painter.circle_filled(egui::pos2(c.x, r.top() + w * 0.3), w * 0.17, colour);
            // Shoulders: the top of a larger circle, clipped at the glyph's bottom edge.
            painter.with_clip_rect(r).circle_filled(
                egui::pos2(c.x, r.bottom() + w * 0.12),
                w * 0.4,
                colour,
            );
        }
        // A target: where the click lands.
        MaskKind::Select => {
            painter.circle_stroke(c, w * 0.4, stroke);
            painter.circle_filled(c, w * 0.12, colour);
        }
        // M10.16a: a road to the horizon — two lines converging on a faint horizon line: distance.
        MaskKind::Depth => {
            let horizon = r.top() + w * 0.25;
            for (from, to) in [
                (r.left_bottom(), egui::pos2(c.x - w * 0.08, horizon)),
                (r.right_bottom(), egui::pos2(c.x + w * 0.08, horizon)),
            ] {
                painter.line_segment([from, to], stroke);
            }
            painter.line_segment(
                [
                    egui::pos2(r.left(), horizon),
                    egui::pos2(r.right(), horizon),
                ],
                egui::Stroke::new(stroke.width, colour.gamma_multiply(0.5)),
            );
        }
    }
}

/// A retouch patch's mode: Heal is a cross over a dot, Clone two circles.
pub fn paint_spot(
    painter: &egui::Painter,
    rect: egui::Rect,
    mode: edit_model::SpotMode,
    colour: egui::Color32,
) {
    let r = rect.shrink(rect.width() * 0.12);
    let c = r.center();
    let w = r.width();
    let stroke = egui::Stroke::new((w / 9.0).max(1.0), colour);
    match mode {
        edit_model::SpotMode::Heal => {
            painter.line_segment(
                [egui::pos2(c.x, r.top()), egui::pos2(c.x, r.bottom())],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(r.left(), c.y), egui::pos2(r.right(), c.y)],
                stroke,
            );
            painter.circle_filled(c, w * 0.16, colour);
        }
        edit_model::SpotMode::Clone => {
            painter.circle_stroke(egui::pos2(c.x - w * 0.16, c.y - w * 0.16), w * 0.3, stroke);
            painter.circle_stroke(
                egui::pos2(c.x + w * 0.16, c.y + w * 0.16),
                w * 0.3,
                egui::Stroke::new(stroke.width, colour.gamma_multiply(0.55)),
            );
        }
    }
}

/// A red-eye row's glyph (M9.11): an iris ring round a pupil; a pet eye adds its catchlight.
/// ⚠ Not [`paint_eye`], which is the row's visibility toggle.
pub fn paint_pupil(
    painter: &egui::Painter,
    rect: egui::Rect,
    kind: edit_model::EyeKind,
    colour: egui::Color32,
) {
    let r = rect.shrink(rect.width() * 0.12);
    let c = r.center();
    let w = r.width();
    painter.circle_stroke(c, w * 0.38, egui::Stroke::new((w / 9.0).max(1.0), colour));
    painter.circle_filled(c, w * 0.16, colour);
    if kind == edit_model::EyeKind::Pet {
        painter.circle_filled(
            egui::pos2(c.x - w * 0.16, c.y - w * 0.16),
            w * 0.07,
            colour.gamma_multiply(0.55),
        );
    }
}

/// A preset's star.
pub fn paint_star(painter: &egui::Painter, rect: egui::Rect, colour: egui::Color32) {
    let r = rect.shrink(rect.width() * 0.1);
    let c = r.center();
    let outer = r.width() * 0.5;
    let inner = outer * 0.45;
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let rad = if i % 2 == 0 { outer } else { inner };
            egui::pos2(c.x + rad * a.cos(), c.y + rad * a.sin())
        })
        .collect();
    painter.add(egui::Shape::closed_line(
        pts,
        egui::Stroke::new((r.width() / 9.0).max(1.0), colour),
    ));
}

/// The hold-out toggle: a circle, struck through while `on` — the patch is held out of the
/// view so what it covers shows.
pub fn paint_hold(painter: &egui::Painter, rect: egui::Rect, on: bool, colour: egui::Color32) {
    let r = rect.shrink(rect.width() * 0.12);
    let c = r.center();
    let w = r.width();
    let stroke = egui::Stroke::new((w / 9.0).max(1.0), colour);
    painter.circle_stroke(c, w * 0.4, stroke);
    if on {
        painter.line_segment(
            [
                egui::pos2(c.x - w * 0.24, c.y),
                egui::pos2(c.x + w * 0.24, c.y),
            ],
            egui::Stroke::new(stroke.width * 1.3, colour),
        );
    }
}

/// An eye: open when `on`, a closed lid otherwise. The per-mask overlay toggle.
pub fn paint_eye(painter: &egui::Painter, rect: egui::Rect, on: bool, colour: egui::Color32) {
    let r = rect.shrink(rect.width() * 0.1);
    let c = r.center();
    let w = r.width();
    let stroke = egui::Stroke::new((w / 9.0).max(1.0), colour);
    const N: usize = 12;
    let arc = |sign: f32| -> Vec<egui::Pos2> {
        (0..=N)
            .map(|i| {
                let t = i as f32 / N as f32;
                egui::pos2(
                    r.left() + w * t,
                    c.y + sign * w * 0.32 * (std::f32::consts::PI * t).sin(),
                )
            })
            .collect()
    };
    if on {
        let mut pts = arc(-1.0);
        let mut lower = arc(1.0);
        lower.reverse();
        pts.extend(lower);
        painter.add(egui::Shape::closed_line(pts, stroke));
        painter.circle_filled(c, w * 0.14, colour);
    } else {
        painter.add(egui::Shape::line(arc(1.0), stroke));
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

    #[test]
    fn the_eye_paints_in_both_states_and_differently() {
        let shapes = |on: bool| {
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
                        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(16.0, 16.0));
                    paint_eye(ui.painter(), rect, on, egui::Color32::WHITE);
                },
            );
            format!("{:?}", out.shapes)
        };
        let (open, closed) = (shapes(true), shapes(false));
        assert!(open.contains("Circle"), "an open eye has a pupil");
        assert!(!closed.contains("Circle"), "a closed eye has none");
    }

    #[test]
    fn the_spot_glyphs_and_the_hold_toggle_paint_something() {
        let shapes = |f: &dyn Fn(&egui::Painter, egui::Rect)| {
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
                    f(ui.painter(), rect);
                },
            );
            format!("{:?}", out.shapes)
        };
        for mode in [edit_model::SpotMode::Heal, edit_model::SpotMode::Clone] {
            let s = shapes(&|p, r| paint_spot(p, r, mode, egui::Color32::WHITE));
            assert!(
                s.contains("Circle") || s.contains("LineSegment"),
                "{mode:?} painted nothing"
            );
        }
        let off = shapes(&|p, r| paint_hold(p, r, false, egui::Color32::WHITE));
        let on = shapes(&|p, r| paint_hold(p, r, true, egui::Color32::WHITE));
        assert!(off.contains("Circle"));
        assert!(
            on.contains("LineSegment") && !off.contains("LineSegment"),
            "the strike shows only when on"
        );
    }
}
