//! The quiet progress line a panel draws under the control whose job is running (`docs/DESIGN.md` §7.16):
//! AI denoise's "Full denoise" (M10.16b6) and Remove dust's "Looking for dust" (M10.16c2).

use crate::theme::tokens;
use crate::theme::Hover as _;
use eframe::egui;

/// `label` in Caption `text2` over a 3 px line across the section, its `done` part (0..=1) `text3` on
/// `line` — never the accent, no percentage. Both carry `hover`.
pub(super) fn show(ui: &mut egui::Ui, label: &str, done: f32, hover: &str) {
    let t = tokens::of(ui);
    ui.add(
        egui::Label::new(
            egui::RichText::new(label)
                .size(tokens::CAPTION)
                .color(t.text2),
        )
        .selectable(false),
    )
    .hover(hover);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), tokens::PROGRESS_LINE),
        egui::Sense::hover(),
    );
    let round = egui::CornerRadius::same((tokens::PROGRESS_LINE / 2.0) as u8);
    ui.painter().rect_filled(rect, round, t.line);
    let mut part = rect;
    part.set_width(rect.width() * done.clamp(0.0, 1.0));
    ui.painter().rect_filled(part, round, t.text3);
    response.hover(hover);
}
