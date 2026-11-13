//! The welcome's two small maps of the window (M11.4): blocks in the theme's surfaces, each part's number on an
//! accent disc. Proportions follow the mockup; they must read like the window, not match it to the pixel.

use crate::start_page::Map;
use eframe::egui;

/// A map's height for its width — the mockup's 270 × 128.
const ASPECT: f32 = 128.0 / 270.0;
/// The number discs' diameter.
const DISC: f32 = 16.0;

/// A number disc where the layout puts it.
pub(crate) fn disc(ui: &mut egui::Ui, n: u8) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(DISC, DISC), egui::Sense::hover());
    paint_disc(ui, rect.center(), n);
}

fn paint_disc(ui: &egui::Ui, at: egui::Pos2, n: u8) {
    let accent = ui.visuals().selection.bg_fill;
    let on = crate::theme::tokens::on_accent(ui.visuals().dark_mode);
    let p = ui.painter();
    p.circle_filled(at, DISC / 2.0, accent);
    p.text(
        at,
        egui::Align2::CENTER_CENTER,
        n.to_string(),
        egui::TextStyle::Small.resolve(ui.style()),
        on,
    );
}

/// Paint `map` at `width`.
pub(crate) fn paint(ui: &mut egui::Ui, map: Map, width: f32) {
    let size = egui::vec2(width, (width * ASPECT).round());
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let tok = crate::theme::tokens::of(ui);
    let p = ui.painter();
    p.rect_filled(rect, 8.0, tok.canvas);
    let r = rect.shrink(5.0);
    let gap = 2.0;
    let block = |x0: f32, y0: f32, x1: f32, y1: f32| {
        egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
    };
    let top = 20.0_f32.min(r.height() * 0.16);
    let status = 12.0_f32.min(r.height() * 0.1);
    let body_top = r.top() + top + gap;
    let fill = |rect: egui::Rect, c: egui::Color32| p.rect_filled(rect, 3.0, c);
    let bar = block(r.left(), r.top(), r.right(), r.top() + top);
    fill(bar, tok.raised);
    let mut discs: Vec<(egui::Pos2, u8)> = Vec::new();
    match map {
        Map::Library => {
            let rail_w = r.width() * 0.23;
            let rail = block(r.left(), body_top, r.left() + rail_w, r.bottom());
            let grid = block(
                rail.right() + gap,
                body_top,
                r.right(),
                r.bottom() - status - gap,
            );
            let st = block(grid.left(), grid.bottom() + gap, r.right(), r.bottom());
            fill(rail, tok.raised);
            fill(grid, tok.panel);
            fill(st, tok.raised);
            // The grid's tiles: four by two.
            let (cols, rows) = (4.0, 2.0);
            let pad = 6.0;
            let tw = (grid.width() - pad * 2.0 - 4.0 * (cols - 1.0)) / cols;
            let th = ((grid.height() - pad * 2.0 - 4.0 * (rows - 1.0)) / rows).min(tw * 0.8);
            for row in 0..2 {
                for col in 0..4 {
                    let x = grid.left() + pad + col as f32 * (tw + 4.0);
                    let y = grid.top() + pad + row as f32 * (th + 4.0);
                    p.rect_filled(block(x, y, x + tw, y + th), 2.0, tok.raised);
                }
            }
            discs.push((egui::pos2(bar.left() + 12.0, bar.center().y), 1));
            discs.push((rail.center(), 2));
            discs.push((egui::pos2(bar.right() - 12.0, bar.center().y), 3));
            discs.push((st.center(), 4));
        }
        Map::Develop => {
            let tools_w = r.width() * 0.06;
            let panel_w = r.width() * 0.24;
            let tools = block(r.right() - tools_w, body_top, r.right(), r.bottom());
            let panel = block(
                tools.left() - gap - panel_w,
                body_top,
                tools.left() - gap,
                r.bottom(),
            );
            let strip_h = (r.height() * 0.17).max(14.0);
            let strip = block(
                r.left(),
                r.bottom() - strip_h,
                panel.left() - gap,
                r.bottom(),
            );
            let st = block(
                strip.left(),
                strip.top() - gap - status,
                strip.right(),
                strip.top() - gap,
            );
            let photo = block(r.left(), body_top, panel.left() - gap, st.top() - gap);
            fill(photo, tok.panel);
            fill(panel, tok.raised);
            fill(tools, tok.raised);
            fill(st, tok.raised);
            fill(strip, tok.raised);
            // The photo: a lighter frame on the canvas, so the area reads as a picture.
            p.rect_filled(
                photo.shrink2(egui::vec2(photo.width() * 0.12, 6.0)),
                2.0,
                tok.hover,
            );
            discs.push((egui::pos2(bar.left() + 12.0, bar.center().y), 1));
            discs.push((egui::pos2(bar.right() - 12.0, bar.center().y), 8));
            discs.push((panel.center(), 5));
            discs.push((tools.center(), 6));
            discs.push((strip.center(), 7));
        }
    }
    for (at, n) in discs {
        paint_disc(ui, at, n);
    }
}
