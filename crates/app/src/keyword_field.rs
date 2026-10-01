//! The Keywords section's field — type to tag (M10.8a).
//!
//! With photos selected, typing lists the matching keywords and Enter (or a click) tags the
//! selection with the highlighted one; a name with no exact match ends the list with a *New
//! keyword* row that creates it and tags. With nothing selected the field creates: the *New
//! keyword* row leads, and picking an existing match opens it instead of making a duplicate.
//!
//! ⚠ Its own module: `collections_panel.rs` is over 600 lines of code and gains only the wiring.
//! The matching rule is the lib's `keyword_suggest::complete`; this is the gesture.

use crate::list_row::{self, RowSpec};
use crate::theme::Hover as _;
use catalog::db::KeywordRow;
use eframe::egui;
use rustroom_app::keyword_suggest::{self, Match};

/// What the field asked for, applied after the panel closes.
#[derive(Debug, PartialEq)]
pub(crate) enum FieldOut {
    /// Tag the selection with this keyword.
    Tag(i64),
    /// Create a keyword of this name — and tag the selection, if there is one.
    Create(String),
    /// Open this keyword, as a click on its row does.
    Open(i64, String),
}

/// The field's state between frames.
#[derive(Default)]
pub(crate) struct FieldState {
    /// The row ↑/↓ moved to; `None` is the default row (see [`default_row`]).
    highlight: Option<usize>,
    /// The text the highlight belongs to — typing resets it.
    typed: String,
}

/// The field's id, so a test can focus it.
fn field_id() -> egui::Id {
    egui::Id::new("keyword_field")
}

/// One row of the list under the field.
enum Choice {
    Existing(Match),
    New(String),
}

/// The list for `typed`: the matches, then *New keyword* unless a keyword of exactly that name
/// exists. ⚠ With nothing selected *New keyword* leads — the field creates there, and Enter on a
/// match would open what the user meant to make.
fn choices(typed: &str, kws: &[KeywordRow], picked: usize) -> Vec<Choice> {
    let name = typed.trim();
    if name.is_empty() {
        return Vec::new();
    }
    let matches = keyword_suggest::complete(name, kws, keyword_suggest::COMPLETE_LIMIT);
    let exact = matches.iter().any(|m| m.name.eq_ignore_ascii_case(name));
    let mut list: Vec<Choice> = matches.into_iter().map(Choice::Existing).collect();
    if !exact {
        let new = Choice::New(name.to_string());
        if picked > 0 {
            list.push(new);
        } else {
            list.insert(0, new);
        }
    }
    list
}

/// The field's hint: what Enter will do. ⚠⚠ The parent is the keyword last CLICKED — no
/// dropdown (the author's call, 2026-09-27): click one, type, and the new keyword goes under it.
fn hint(picked: usize, parent: Option<&str>) -> String {
    match (picked, parent) {
        (0, Some(p)) => format!("New keyword under {p}"),
        (0, None) => "New keyword".to_string(),
        _ => "Tag with a keyword".to_string(),
    }
}

pub(crate) fn field(
    ui: &mut egui::Ui,
    text: &mut String,
    state: &mut FieldState,
    kws: &[KeywordRow],
    picked: usize,
    parent: Option<&str>,
) -> Option<FieldOut> {
    if state.typed != *text {
        state.highlight = None;
        state.typed = text.clone();
    }
    let hint = hint(picked, parent);
    let named = !text.trim().is_empty();
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    let (resp, plus) = ui
        .allocate_ui_with_layout(
            size,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let plus = ui
                    .add_enabled(named, egui::Button::new("+"))
                    .hover(if picked > 0 {
                        "Create a keyword with this name and tag the selected photos with it"
                    } else {
                        "Create a keyword with this name"
                    })
                    .clicked();
                let resp = ui.add_sized(
                    egui::vec2(ui.available_width(), size.y),
                    egui::TextEdit::singleline(text)
                        .id(field_id())
                        .hint_text(hint),
                );
                (resp, plus)
            },
        )
        .inner;
    if plus {
        return finish(text, state, FieldOut::Create(text.trim().to_string()));
    }
    let list = choices(text, kws, picked);
    if list.is_empty() {
        return None;
    }
    let last = list.len() - 1;
    let mut at = state.highlight.unwrap_or(0).min(last);
    let focused = resp.has_focus() || resp.lost_focus();
    if focused {
        let (down, up, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            text.clear();
            *state = FieldState::default();
            return None;
        }
        if down {
            at = (at + 1).min(last);
            state.highlight = Some(at);
        }
        if up {
            at = at.saturating_sub(1);
            state.highlight = Some(at);
        }
    }
    let enter = crate::theme::committed(ui, &resp);
    let mut picked_row = enter.then_some(at);
    for (i, c) in list.iter().enumerate() {
        let (name, meta, hover) = match c {
            Choice::Existing(m) => (
                m.name.clone(),
                m.path.as_str(),
                if picked > 0 {
                    "Tag the selected photos with this keyword"
                } else {
                    "Open this keyword"
                },
            ),
            Choice::New(n) => (
                format!("New keyword “{n}”"),
                parent.unwrap_or(""),
                if picked > 0 {
                    "Create this keyword and tag the selected photos with it"
                } else {
                    "Create this keyword"
                },
            ),
        };
        let r = list_row::row(
            ui,
            RowSpec {
                name: &name,
                count: None,
                meta,
                selected: i == at,
                indent: 0.0,
                hover,
                actions: &[],
            },
        );
        if r.clicked {
            picked_row = Some(i);
        }
    }
    let i = picked_row?;
    let out = match &list[i] {
        Choice::Existing(m) if picked > 0 => FieldOut::Tag(m.id),
        Choice::Existing(m) => FieldOut::Open(m.id, m.name.clone()),
        Choice::New(n) => FieldOut::Create(n.clone()),
    };
    // ⚠ Enter keeps the field focused for the next keyword — tagging is often several in a row.
    if enter {
        resp.request_focus();
    }
    finish(text, state, out)
}

/// Empty the field after a pick.
fn finish(text: &mut String, state: &mut FieldState, out: FieldOut) -> Option<FieldOut> {
    text.clear();
    *state = FieldState::default();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn kw(id: i64, name: &str, parent: Option<i64>) -> KeywordRow {
        KeywordRow {
            id,
            name: name.to_string(),
            parent_id: parent,
            depth: 0,
            count: 0,
        }
    }

    /// Europe(1) > {Paris(2), Parma(3)}; Asia(4).
    fn tree() -> Vec<KeywordRow> {
        vec![
            kw(1, "Europe", None),
            kw(2, "Paris", Some(1)),
            kw(3, "Parma", Some(1)),
            kw(4, "Asia", None),
        ]
    }

    struct Harness {
        ctx: egui::Context,
        text: String,
        state: FieldState,
        picked: usize,
        outs: Vec<FieldOut>,
        t: f64,
    }

    impl Harness {
        fn new(picked: usize) -> Self {
            Self {
                ctx: test_ui::ctx(),
                text: String::new(),
                state: FieldState::default(),
                picked,
                outs: Vec::new(),
                t: 1.0,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let kws = tree();
            self.t += 0.05;
            let (text, state, picked, outs) =
                (&mut self.text, &mut self.state, self.picked, &mut self.outs);
            let _ = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(test_ui::screen()),
                    time: Some(self.t),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if let Some(o) = field(ui, text, state, &kws, picked, Some("Europe")) {
                        outs.push(o);
                    }
                },
            );
        }

        /// Focus the field and type `s`.
        fn type_text(&mut self, s: &str) {
            self.ctx.memory_mut(|m| m.request_focus(field_id()));
            self.frame(vec![]);
            self.frame(vec![egui::Event::Text(s.to_string())]);
            self.frame(vec![]);
        }

        fn press(&mut self, k: egui::Key) {
            self.frame(vec![egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }]);
            self.frame(vec![]);
        }

        /// The labels drawn this frame.
        fn labels(&mut self) -> Vec<String> {
            let kws = tree();
            let mut text = self.text.clone();
            let mut state = FieldState {
                highlight: self.state.highlight,
                typed: self.state.typed.clone(),
            };
            let picked = self.picked;
            test_ui::nodes(&self.ctx, |ui| {
                let _ = field(ui, &mut text, &mut state, &kws, picked, Some("Europe"));
            })
            .into_iter()
            .map(|n| n.label)
            .collect()
        }
    }

    #[test]
    fn typing_lists_the_matches_and_enter_tags_with_the_first() {
        let mut h = Harness::new(2);
        h.type_text("par");
        assert_eq!(h.text, "par", "the sample: the typing reached the field");
        let labels = h.labels();
        assert!(labels.iter().any(|l| l == "Paris"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "Parma"), "{labels:?}");
        assert!(!labels.iter().any(|l| l == "Asia"), "{labels:?}");
        h.press(egui::Key::Enter);
        assert_eq!(h.outs, vec![FieldOut::Tag(2)]);
        assert!(h.text.is_empty(), "the field empties after a pick");
    }

    #[test]
    fn arrow_down_then_enter_takes_the_second() {
        let mut h = Harness::new(1);
        h.type_text("par");
        h.press(egui::Key::ArrowDown);
        h.press(egui::Key::Enter);
        assert_eq!(h.outs, vec![FieldOut::Tag(3)]);
    }

    /// A name nothing matches: Enter creates it — and, with a selection, the caller tags.
    #[test]
    fn a_new_name_and_enter_creates() {
        let mut h = Harness::new(1);
        h.type_text("Berlin");
        assert!(
            h.labels().iter().any(|l| l == "New keyword “Berlin”"),
            "{:?}",
            h.labels()
        );
        h.press(egui::Key::Enter);
        assert_eq!(h.outs, vec![FieldOut::Create("Berlin".to_string())]);
    }

    /// ⚠ With a selection the matches come first and *New keyword* last; without one it leads, so
    /// Enter creates rather than opening "Paris" when "Par" was meant.
    #[test]
    fn with_nothing_selected_enter_creates_and_a_match_opens() {
        let mut h = Harness::new(0);
        h.type_text("Par");
        h.press(egui::Key::Enter);
        assert_eq!(h.outs, vec![FieldOut::Create("Par".to_string())]);
        h.type_text("Par");
        h.press(egui::Key::ArrowDown);
        h.press(egui::Key::Enter);
        assert_eq!(h.outs[1], FieldOut::Open(2, "Paris".to_string()));
    }

    /// An exact name offers no *New keyword* — picking it opens the one that exists.
    #[test]
    fn an_exact_name_opens_rather_than_duplicating() {
        let mut h = Harness::new(0);
        h.type_text("paris");
        assert!(
            !h.labels().iter().any(|l| l.starts_with("New keyword “")),
            "{:?}",
            h.labels()
        );
        h.press(egui::Key::Enter);
        assert_eq!(h.outs, vec![FieldOut::Open(2, "Paris".to_string())]);
    }

    #[test]
    fn escape_closes_the_list() {
        let mut h = Harness::new(1);
        h.type_text("par");
        h.press(egui::Key::Escape);
        assert!(h.text.is_empty());
        assert!(h.outs.is_empty());
        assert!(!h.labels().iter().any(|l| l == "Paris"));
    }

    /// ⚠ Asserted on the function: AccessKit reports a text field's value, not its hint.
    #[test]
    fn the_hint_follows_the_selection() {
        assert_eq!(hint(2, Some("Europe")), "Tag with a keyword");
        assert_eq!(hint(0, Some("Europe")), "New keyword under Europe");
        assert_eq!(hint(0, None), "New keyword");
    }
}
