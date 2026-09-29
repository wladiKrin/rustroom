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

use crate::theme::{self, tokens};
use eframe::egui;

const HEIGHT: f32 = tokens::HEADER_ROW;
const CHEVRON_W: f32 = 14.0;
/// How far a card reaches past the content it holds, left and right.
const CARD_PAD_X: f32 = 8.0;
/// How far a card reaches above its header and below its last row.
const CARD_PAD_TOP: f32 = 4.0;
const CARD_PAD_BOTTOM: f32 = 6.0;
/// The space a card leaves under itself before the next header's own air.
const CARD_GAP: f32 = 10.0;

/// The text a header paints for `name`: the Library rail's group names in capitals, so the rail
/// reads as a table of contents; every other header as written, in sentence case —
/// `docs/DESIGN.md` §7.5. Pure, so the rule is testable without a window.
///
/// ⚠ Sentence case is applied here rather than by renaming: the names are also the AccessKit
/// labels the harness clicks by ("White Balance", "Color") and the Sync dialog's rows, so the
/// painted text changes and the name does not. A later word written in capitals ("HSL") stays.
pub fn header_text(name: &str, rail: bool) -> String {
    if rail {
        return name.to_uppercase();
    }
    let mut out = String::with_capacity(name.len());
    for (i, word) in name.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let acronym = word.len() > 1 && word.chars().all(|c| !c.is_lowercase());
        if i == 0 || acronym {
            out.push_str(word);
        } else {
            let mut cs = word.chars();
            if let Some(first) = cs.next() {
                out.extend(first.to_lowercase());
                out.push_str(cs.as_str());
            }
        }
    }
    out
}

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
    /// The Library rail's form: the name in capitals, in the caption face and the secondary grey.
    pub rail: bool,
}

impl<'a> Header<'a> {
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            modified: false,
            resettable: false,
            trailing: None,
            draggable: false,
            rail: false,
        }
    }
    pub fn rail(mut self, on: bool) -> Self {
        self.rail = on;
        self
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
    /// The shapes reserved behind and under the row: the card background and the hairline.
    card: CardShapes,
}

struct CardShapes {
    background: egui::layers::ShapeIdx,
    rule: egui::layers::ShapeIdx,
    /// The header row, which the card grows from to take in the body.
    row: egui::Rect,
}

impl HeaderOut {
    /// Draw the section's body, then turn the section into a **card** — `docs/DESIGN.md` §7.5:
    /// the `card` surface behind the header and the body, radius 10, and no hairline.
    ///
    /// ⚠⚠ **Painted behind, never wrapped.** The background was reserved as an empty shape before
    /// the header painted, and is filled in here once the body's height is known — so a card
    /// costs a caller one word (`show_body` for `state.show_body_unindented`) and changes no
    /// layout. A header whose body is shown the old way (the dialogs, the rail) keeps its
    /// hairline and gets no card.
    ///
    /// ⚠⚠ **Never for a body that fills its container** (`set_min_height(available_height)`): the
    /// gap left under the card makes the content taller than the container every frame, and a
    /// resizable panel then grows by that much per frame — the snapshot strip did, 2026-09-29.
    pub fn show_body<R>(
        &mut self,
        ui: &mut egui::Ui,
        add: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<R> {
        // ⚠⚠ Vertically the card is the header row **united with the body's own rect**, never
        // "from the header down to the cursor": the snapshot strip lays out bottom-up, and reading
        // the cursor there stretched its card to the bottom of the window (seen 2026-09-29).
        let (inner, body) = match self.state.show_body_unindented(ui, add) {
            Some(r) => (Some(r.inner), Some(r.response.rect)),
            None => (None, None),
        };
        let span = body.map_or(self.card.row, |b| self.card.row.union(b));
        let full = ui.max_rect();
        let rect = egui::Rect::from_min_max(
            egui::pos2(full.left() - CARD_PAD_X, span.top() - CARD_PAD_TOP),
            egui::pos2(full.right() + CARD_PAD_X, span.bottom() + CARD_PAD_BOTTOM),
        );
        let t = tokens::of(ui);
        ui.painter().set(
            self.card.background,
            egui::Shape::rect_filled(rect, tokens::RADIUS_CARD, t.card),
        );
        ui.painter().set(self.card.rule, egui::Shape::Noop);
        ui.add_space(CARD_GAP);
        inner
    }
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
    // ⚠ Reserved before anything paints, so a card filled in by `show_body` sits behind the row.
    let background = ui.painter().add(egui::Shape::Noop);
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

    let caption = theme::section_font(ui.style());
    // ⚠ A panel section's name in the medium face at body size (`docs/DESIGN.md` §7.5); the rail's
    // capitals stay regular caption — weight would make a table of contents shout.
    let font = if h.rail {
        caption.clone()
    } else {
        theme::medium_font(ui.ctx(), tokens::BODY)
    };
    let weak = ui.visuals().weak_text_color();
    // The actions fill in from the right edge; `right` is where the next one ends.
    let mut right = rect.right();
    let mut reset = false;
    if h.modified && h.resettable {
        let galley = ui
            .painter()
            .layout_no_wrap("reset".to_owned(), caption.clone(), weak);
        let cell = egui::Rect::from_min_max(
            egui::pos2(right - galley.size().x - 8.0, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let r = ui
            .interact(cell, id.with("reset"), egui::Sense::click())
            .on_hover_text(format!("Resets {} to its defaults", h.name.to_lowercase()));
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
            let galley = painter.layout_no_wrap(text.to_owned(), caption.clone(), weak);
            // ⚠ At the list rows' inset, so a rail header's count and its rows' counts are one
            // column (`list_row`, `tokens::TRAILING_INSET`).
            painter.galley(
                egui::pos2(
                    right - galley.size().x - tokens::TRAILING_INSET,
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
            header_text(h.name, h.rail),
            font,
            if h.rail {
                v.weak_text_color()
            } else {
                v.strong_text_color()
            },
        );
        if h.modified {
            painter.circle_filled(
                egui::pos2(text_rect.right() + 8.0, rect.center().y),
                2.5,
                v.selection.bg_fill,
            );
        }
    }
    // ⚠ The hairline is its own shape so `show_body` can take it away: a card needs no rule, a
    // dialog's plain header keeps one, and the rail's capitals carry the structure alone.
    let rule = if h.rail {
        ui.painter().add(egui::Shape::Noop)
    } else {
        ui.painter().add(egui::Shape::line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        ))
    };

    HeaderOut {
        state,
        reset,
        response: resp,
        card: CardShapes {
            background,
            rule,
            row: rect,
        },
    }
}

/// A collapsible block **inside** a section — the tone curve (2026-09-29): the chevron the section
/// headers draw, the name in the sub-label face (Caption medium, `text2`), an accent dot while the
/// block differs from neutral, and `trailing` — the block's own Reset — at the right **while it is
/// open**. Returns whether it is open.
///
/// ⚠⚠ **`open` is the caller's, never egui's collapsing state:** egui's memory does not survive a
/// restart here, and a block the author collapsed to keep it out of the way must stay collapsed
/// (`Settings::tone_curve_open`).
///
/// ⚠ The trailing controls act on what the block shows — the curve's selected channel — so they
/// go with it when it closes; the dot still says the block is doing something.
///
/// ⚠ The row's click is registered **before** the trailing controls, so they win the hit test
/// where they sit (egui takes the last widget registered at a point), and the rest of the row
/// toggles.
pub fn sub_header(
    ui: &mut egui::Ui,
    name: &str,
    open: &mut bool,
    modified: bool,
    trailing: impl FnOnce(&mut egui::Ui),
) -> bool {
    ui.add_space(8.0);
    let height = ui.spacing().interact_size.y;
    let row = egui::Rect::from_min_size(
        ui.next_widget_position(),
        egui::vec2(ui.available_width(), height),
    );
    let resp = ui.interact(
        row,
        ui.id().with(("sub_header", name)),
        egui::Sense::click(),
    );
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, enabled, name)
    });
    if resp.clicked() {
        *open = !*open;
    }
    // ⚠ The whole row's height is claimed even when nothing trails, or the next widget would be
    // placed over this row — the empty-scope trap `list_row` records.
    let shown = *open;
    ui.allocate_ui_with_layout(
        row.size(),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.set_min_height(height);
            if shown {
                trailing(ui);
            }
        },
    );
    if ui.is_rect_visible(row) {
        let v = ui.visuals().clone();
        let painter = ui.painter();
        let c = egui::pos2(row.left() + CHEVRON_W / 2.0, row.center().y);
        let rot = if shown {
            egui::emath::Rot2::from_angle(std::f32::consts::FRAC_PI_2)
        } else {
            egui::emath::Rot2::IDENTITY
        };
        let pts = [
            egui::vec2(-2.0, -3.5),
            egui::vec2(1.5, 0.0),
            egui::vec2(-2.0, 3.5),
        ]
        .map(|p| c + rot * p)
        .to_vec();
        let colour = if resp.hovered() {
            v.strong_text_color()
        } else {
            v.weak_text_color()
        };
        painter.add(egui::Shape::line(pts, egui::Stroke::new(1.4, colour)));
        let font = theme::medium_font(ui.ctx(), theme::section_font(ui.style()).size);
        let text_rect = painter.text(
            egui::pos2(row.left() + CHEVRON_W + 2.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            font,
            colour,
        );
        if modified {
            painter.circle_filled(
                egui::pos2(text_rect.right() + 8.0, row.center().y),
                2.5,
                v.selection.bg_fill,
            );
        }
    }
    shown
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
            rail,
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
                    rail,
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
    fn a_rail_header_is_in_capitals_and_every_other_header_is_as_written() {
        // `docs/DESIGN.md` §7.5: the rail reads as a table of contents, the panel and the dialogs
        // as sentence case. Both halves, or a function that always (or never) uppercased passes.
        assert_eq!(header_text("Smart collections", true), "SMART COLLECTIONS");
        assert_eq!(header_text("White Balance", false), "White balance");
        assert_eq!(header_text("Noise Reduction", false), "Noise reduction");
        // A later word in capitals is an acronym and keeps them; a one-word name is untouched.
        assert_eq!(header_text("Adjust HSL", false), "Adjust HSL");
        assert_eq!(header_text("Tone", false), "Tone");
    }

    #[test]
    fn a_card_body_is_drawn_and_its_value_returned() {
        // `show_body` replaces `state.show_body_unindented` at every card site; it must still run
        // the body when open and hand back what the body returned.
        let ctx = crate::test_ui::ctx();
        let mut got = None;
        let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            let mut out = header(ui, egui::Id::new("c"), true, Header::new("Tone"));
            got = out.show_body(ui, |ui| {
                let _ = ui.button("Auto");
                7
            });
        });
        assert_eq!(got, Some(7));
        assert!(found.iter().any(|(l, _)| l == "Auto"), "{found:?}");
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
        assert_eq!(
            open_after,
            Some(false),
            "the click did not close the section"
        );
    }
}
