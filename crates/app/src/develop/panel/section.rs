//! The section header: `› NAME •  …  reset`.
//!
//! A hand-drawn header over `CollapsingState`, not a `CollapsingHeader`: that widget draws
//! egui's triangle and indents its body, and has no slot for the two things this row carries —
//! an accent dot while the group differs from neutral, and a `reset` for exactly that group.
//! Callers show the body through `show_body_unindented`, so the left-hand gap under every
//! section is gone with the indent that made it.
//!
//! ⚠ **The chevron is on the left and the reset alone on the right** (2026-09-19). The first
//! draft put both at the right edge, where they read as one smudged control. Lightroom's layout
//! is the fix: the disclosure marker leads the name, and the row's one action has the corner to
//! itself.
//!
//! Two callers: `adjustments::section` for the six edit-group sections, and the Profile block in
//! `panel/mod.rs`, which is deliberately **not** a section (a reset there would flatten the
//! photo to a colorimetric render — see `edit_model::RENDERING_GROUP`) and so passes
//! `resettable: false`.

use crate::theme;
use eframe::egui;

const HEIGHT: f32 = 22.0;
const CHEVRON_W: f32 = 14.0;

/// What a header row carries besides its name.
pub struct Header<'a> {
    /// The AccessKit label is this text as written, not the uppercased text the row paints:
    /// the harnesses click sections by name ("Color"), and the name is what a screen reader
    /// wants.
    pub name: &'a str,
    /// Draw the accent dot after the name.
    pub modified: bool,
    /// Offer a `reset` at the right edge while `modified`.
    pub resettable: bool,
}

/// Draw the header row. Returns the collapsing state, for the caller's
/// `show_body_unindented`, and whether `reset` was clicked this frame.
///
/// ⚠ The reset is registered **after** the row, so it wins the hit-test tie — egui takes the
/// last widget registered at a point, the rule the Library's rubber band relies on. A click on
/// it does not toggle the section.
pub fn header(
    ui: &mut egui::Ui,
    id: egui::Id,
    default_open: bool,
    h: Header<'_>,
) -> (egui::collapsing_header::CollapsingState, bool) {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );

    ui.add_space(6.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEIGHT),
        egui::Sense::click(),
    );
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, enabled, h.name)
    });

    let font = theme::section_font(ui.style());
    let mut reset = false;
    if h.modified && h.resettable {
        let weak = ui.visuals().weak_text_color();
        let galley = ui
            .painter()
            .layout_no_wrap("reset".to_owned(), font.clone(), weak);
        let cell = egui::Rect::from_min_max(
            egui::pos2(rect.right() - galley.size().x - 8.0, rect.top()),
            egui::pos2(rect.right(), rect.bottom()),
        );
        let r = ui
            .interact(cell, id.with("reset"), egui::Sense::click())
            .on_hover_text(format!("Reset {} and nothing else", h.name));
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "reset"));
        reset = r.clicked();
        let colour = if r.hovered() {
            ui.visuals().strong_text_color()
        } else {
            weak
        };
        ui.painter().galley(
            egui::pos2(cell.left() + 4.0, cell.center().y - galley.size().y / 2.0),
            galley,
            colour,
        );
    }
    if resp.clicked() && !reset {
        state.toggle(ui);
    }

    if ui.is_rect_visible(rect) {
        let v = ui.visuals().clone();
        let painter = ui.painter_at(rect);
        // The chevron: '›' closed, rotated to '⌄' as the section opens.
        let openness = state.openness(ui.ctx());
        let rot = egui::emath::Rot2::from_angle(std::f32::consts::FRAC_PI_2 * openness);
        let c = egui::pos2(rect.left() + CHEVRON_W / 2.0, rect.center().y);
        let pts = [
            egui::vec2(-2.0, -3.5),
            egui::vec2(1.5, 0.0),
            egui::vec2(-2.0, 3.5),
        ]
        .map(|p| c + rot * p)
        .to_vec();
        painter.add(egui::Shape::line(
            pts,
            egui::Stroke::new(1.4, v.weak_text_color()),
        ));
        let text_rect = painter.text(
            egui::pos2(rect.left() + CHEVRON_W + 2.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            h.name.to_uppercase(),
            font,
            v.strong_text_color(),
        );
        if h.modified {
            painter.circle_filled(
                egui::pos2(text_rect.right() + 8.0, rect.center().y),
                2.5,
                v.selection.bg_fill,
            );
        }
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        );
    }

    (state, reset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(h: Header<'static>) -> Vec<String> {
        let ctx = crate::test_ui::ctx();
        let (modified, resettable) = (h.modified, h.resettable);
        crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            let _ = header(
                ui,
                egui::Id::new("t"),
                true,
                Header {
                    name: "Profile",
                    modified,
                    resettable,
                },
            );
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect()
    }

    #[test]
    fn a_header_that_is_not_resettable_offers_no_reset_even_when_modified() {
        // ⚠ The Profile block's contract: a reset there would flatten the photograph to a
        // colorimetric render, so the flag must actually suppress the button and not merely
        // grey it. The resettable half proves the harness can see a reset at all.
        let none = labels(Header {
            name: "Profile",
            modified: true,
            resettable: false,
        });
        assert!(none.iter().any(|l| l == "Profile"), "the row is unlabelled");
        assert!(
            !none.iter().any(|l| l == "reset"),
            "a non-resettable header offered a reset"
        );
        let some = labels(Header {
            name: "Profile",
            modified: true,
            resettable: true,
        });
        assert!(
            some.iter().any(|l| l == "reset"),
            "a modified, resettable header offered no reset"
        );
    }

    #[test]
    fn a_neutral_header_offers_no_reset() {
        let l = labels(Header {
            name: "Profile",
            modified: false,
            resettable: true,
        });
        assert!(!l.iter().any(|l| l == "reset"));
    }
}
