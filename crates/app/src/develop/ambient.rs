//! The two overlays that sit over everything in Develop, Lightroom's ambient keys: lights-out
//! (`L`: the surround dimmed, then black, the picture alone) and the info card (`I`: what the
//! camera recorded, in the picture's corner). Both paint on the foreground layer after the
//! views, so panels and filmstrip go dark with the rest.

use crate::{AppView, RustroomApp};
use eframe::egui;

pub fn draw(app: &RustroomApp, ctx: &egui::Context) {
    if app.view != AppView::Develop {
        return;
    }
    let Some(s) = &app.session else { return };
    let Some(frame) = s.canvas_frame_rect else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("ambient_overlays"),
    ));
    if app.lights_out > 0 {
        let alpha = if app.lights_out == 1 { 190 } else { 255 };
        let hole = frame.intersect(ctx.viewport_rect());
        for r in crate::rects_around(ctx.viewport_rect(), hole) {
            painter.rect_filled(r, 0.0, egui::Color32::from_black_alpha(alpha));
        }
    }
    if app.show_info_overlay {
        let lines = info_lines(app, &s.path);
        let font = egui::FontId::proportional(14.0);
        let galleys: Vec<_> = lines
            .into_iter()
            .map(|l| painter.layout_no_wrap(l, font.clone(), egui::Color32::WHITE))
            .collect();
        let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max);
        let h: f32 = galleys.iter().map(|g| g.size().y + 3.0).sum();
        let origin = frame.left_top() + egui::vec2(16.0, 14.0);
        painter.rect_filled(
            egui::Rect::from_min_size(
                origin - egui::vec2(10.0, 8.0),
                egui::vec2(w + 20.0, h + 13.0),
            ),
            5.0,
            egui::Color32::from_black_alpha(170),
        );
        let mut y = origin.y;
        for g in galleys {
            let dy = g.size().y + 3.0;
            painter.galley(egui::pos2(origin.x, y), g, egui::Color32::WHITE);
            y += dy;
        }
    }
}

/// The card's lines: the file, the camera and lens, the exposure, the moment.
fn info_lines(app: &RustroomApp, path: &std::path::Path) -> Vec<String> {
    let mut lines = vec![path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()];
    let Some(c) = app.library.as_ref().and_then(|l| l.captures.get(path)) else {
        return lines;
    };
    let gear: Vec<&str> = [c.camera.as_deref(), c.lens.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if !gear.is_empty() {
        lines.push(gear.join(" · "));
    }
    let mut exposure: Vec<String> = Vec::new();
    if let Some(iso) = c.iso {
        exposure.push(format!("ISO {iso}"));
    }
    if let Some(sh) = c.shutter {
        exposure.push(crate::modals::info::shutter_text(sh));
    }
    if let Some(a) = c.aperture {
        exposure.push(format!("f/{a:.1}"));
    }
    if let Some(f) = c.focal_len {
        exposure.push(format!("{f:.0} mm"));
    }
    if !exposure.is_empty() {
        lines.push(exposure.join(" · "));
    }
    if c.taken.is_some() {
        lines.push(crate::modals::info::taken_text(c.taken));
    }
    lines
}
