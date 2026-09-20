//! The section header: `› NAME •  …  trailing  reset`.
//!
//! A hand-drawn header over `CollapsingState`, not a `CollapsingHeader`: that widget draws
//! egui's triangle and indents its body, and has no slot for the things this row carries —
//! an accent dot while the group differs from neutral, a `reset` for exactly that group, and
//! optionally a word of trailing text. Callers show the body through
//! `show_body_unindented`, so the left-hand gap under every section is gone with the indent
//! that made it.
//!
//! ⚠ **The chevron is on the left and the actions on the right** (2026-09-19). The first
//! draft put the chevron and the reset together at the right edge, where they read as one
//! smudged control. Lightroom's layout is the fix: the disclosure marker leads the name, and
//! the row's actions have the corner to themselves, reset outermost.
//!
//! Callers: `adjustments::section` for the six edit-group sections, the Profile block in
//! `panel/mod.rs` (deliberately **not** resettable — a reset there would flatten the photo to a
//! colorimetric render, see `edit_model::RENDERING_GROUP`), and the Masks panel's MASKS, SHAPE
//! (trailing = the shape's kind) and adjustment sections.

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
    /// A word of weak text left of the reset — the Shape section names its kind here. Not a
    /// widget.
    pub trailing: Option<&'a str>,
    /// Sense a drag on the row as well as a click — the Library panel reorders its sections by
    /// dragging their headers. ⚠ egui reports `clicked()` only for a press-release that did not
    /// become a drag, so the two gestures exclude each other by construction.
    pub draggable: bool,
}

impl<'a> Header<'a> {
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            modified: false,
            resettable: false,
            trailing: None,
            draggable: false,
        }
    }
    pub fn modified(mut self, on: bool) -> Self {
        self.modified = on;
        self
    }
    pub fn resettable(mut self, on: bool) -> Self {
        self.resettable = on;
        self
    }
    pub fn trailing(mut self, text: &'a str) -> Self {
        self.trailing = Some(text);
        self
    }
    pub fn draggable(mut self, on: bool) -> Self {
        self.draggable = on;
        self
    }
}

/// What a header row reported.
pub struct HeaderOut {
    /// The collapsing state, for the caller's `show_body_unindented`.
    pub state: egui::collapsing_header::CollapsingState,
    /// `reset` was clicked this frame.
    pub reset: bool,
    /// The row's own response — a draggable header's drag is read from it.
    pub response: egui::Response,
}

/// Draw the header row.
///
/// ⚠ The reset is registered **after** the row, so it wins the hit-test tie — egui takes the
/// last widget registered at a point, the rule the Library's rubber band relies on. A click on
/// it does not toggle the section.
pub fn header(ui: &mut egui::Ui, id: egui::Id, default_open: bool, h: Header<'_>) -> HeaderOut {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );

    ui.add_space(6.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEIGHT),
        if h.draggable {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::click()
        },
    );
    let resp = if h.draggable {
        resp.on_hover_cursor(egui::CursorIcon::Grab)
    } else {
        resp
    };
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, enabled, h.name)
    });

    let font = theme::section_font(ui.style());
    let weak = ui.visuals().weak_text_color();
    // The actions fill in from the right edge; `right` is where the next one ends.
    let mut right = rect.right();
    let mut reset = false;
    if h.modified && h.resettable {
        let galley = ui
            .painter()
            .layout_no_wrap("reset".to_owned(), font.clone(), weak);
        let cell = egui::Rect::from_min_max(
            egui::pos2(right - galley.size().x - 8.0, rect.top()),
            egui::pos2(right, rect.bottom()),
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
        right = cell.left() - 6.0;
    }
    if resp.clicked() && !reset {
        state.toggle(ui);
    }

    if ui.is_rect_visible(rect) {
        let v = ui.visuals().clone();
        let painter = ui.painter_at(rect);
        if let Some(text) = h.trailing {
            let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), weak);
            painter.galley(
                egui::pos2(
                    right - galley.size().x - 4.0,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                weak,
            );
        }
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

    HeaderOut {
        state,
        reset,
        response: resp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(h: Header<'static>) -> Vec<String> {
        let ctx = crate::test_ui::ctx();
        let Header {
            name,
            modified,
            resettable,
            trailing,
            draggable,
        } = h;
        crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            let _ = header(
                ui,
                egui::Id::new("t"),
                true,
                Header {
                    name,
                    modified,
                    resettable,
                    trailing,
                    draggable,
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
        let none = labels(Header::new("Profile").modified(true));
        assert!(none.iter().any(|l| l == "Profile"), "the row is unlabelled");
        assert!(
            !none.iter().any(|l| l == "reset"),
            "a non-resettable header offered a reset"
        );
        let some = labels(Header::new("Profile").modified(true).resettable(true));
        assert!(
            some.iter().any(|l| l == "reset"),
            "a modified, resettable header offered no reset"
        );
    }

    #[test]
    fn a_neutral_header_offers_no_reset() {
        let l = labels(Header::new("Profile").resettable(true));
        assert!(!l.iter().any(|l| l == "reset"));
    }

    #[test]
    fn trailing_text_is_not_a_widget() {
        let l = labels(Header::new("Shape").trailing("Radial"));
        assert!(l.iter().any(|l| l == "Shape"));
        assert!(
            !l.iter().any(|l| l == "Radial"),
            "trailing text became a widget"
        );
    }

    #[test]
    fn a_draggable_header_still_toggles_on_a_click() {
        // ⚠ `Sense::click_and_drag` must not cost the click: the Library panel's sections
        // collapse by a click on the same row that drags them.
        let ctx = crate::test_ui::ctx();
        let mut open_after = None;
        crate::test_ui::click(&ctx, "Folders", |ui| {
            let out = header(
                ui,
                egui::Id::new("d"),
                true,
                Header::new("Folders").draggable(true),
            );
            open_after = Some(out.state.is_open());
        });
        assert_eq!(open_after, Some(false), "the click did not close the section");
    }
}
