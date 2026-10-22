//! The Retouch panel's Find people (M10.16c3), in Remove mode: the button, the line while it looks, then the
//! people found — how many, how many go, Everyone, Remove N people and Cancel. The search is `people_flow`'s; the
//! people are picked on the photo (`people_overlay`).
//!
//! ⚠ Its own file, `retouch_eyes`'s reason: `retouch.rs` sits at the line ceiling.

use super::Edited;
use crate::theme;
use crate::theme::tokens;
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::people_tool::{self, Wait};

const BUTTON_HOVER: &str = "Finds the people in the photo so you can pick the ones to remove";
const LOOKING_HOVER: &str =
    "Each person found is shaded on the photo, the ones to remove in the accent";
const EVERYONE_HOVER: &str = "Picks every person found to remove, or none";
const REMOVE_HOVER: &str = "Removes each person picked, filled from their surroundings";
const CANCEL_HOVER: &str = "Stops showing the people found without removing anyone";
const SUMMARY_HOVER: &str = "Draw over someone it missed to look for them there";
const THERE_HOVER: &str = "Looks again where you drew, closer, and adds who it finds there";

pub(super) fn show(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited) {
    ui.add_space(6.0);
    let looking = s.people.search.is_some();
    if ui
        .add_enabled(!looking, egui::Button::new("Find people"))
        .hover(BUTTON_HOVER)
        .clicked()
    {
        s.people.pressed = true;
    }
    // The whole search: its line alone. A look there keeps the people view, its line under it.
    if let Some(search) = s.people.search.as_ref().filter(|_| !s.people.there) {
        super::progress_line::show(ui, "Looking for people", search.progress(), LOOKING_HOVER);
        return;
    }
    match s.people.waiting {
        Some(Wait::Photo) => return theme::hint(ui, "Waits for the photo to load"),
        Some(Wait::Models) => return theme::hint(ui, "Waits for its models to download"),
        None => {}
    }
    if !s.people.active {
        if let Some(said) = &s.people.said {
            theme::hint(ui, said);
        }
        return;
    }
    let n = s.people.people.len();
    s.people.ticked.resize(n, false);
    let ticked = s.people.ticked.iter().filter(|t| **t).count();
    // ⚠ The one line saying a drawing looks again: its hover, the panel's only word on the gesture.
    let t = tokens::of(ui);
    ui.add(
        egui::Label::new(
            egui::RichText::new(people_tool::summary(n, ticked))
                .size(tokens::CAPTION)
                .color(t.text2),
        )
        .selectable(false),
    )
    .hover(SUMMARY_HOVER);
    if let Some(said) = &s.people.said {
        theme::hint(ui, said);
    }
    if n > 0 {
        let mut all = ticked == n;
        if ui
            .add(egui::Checkbox::new(&mut all, "Everyone").indeterminate(ticked > 0 && ticked < n))
            .hover(EVERYONE_HOVER)
            .changed()
        {
            s.people.ticked.iter_mut().for_each(|t| *t = all);
        }
    }
    let (mut remove, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if n > 0 {
            remove = ui
                .add_enabled(
                    ticked > 0,
                    egui::Button::new(people_tool::remove_label(ticked)),
                )
                .hover(REMOVE_HOVER)
                .clicked();
        }
        cancel = ui.button("Cancel").hover(CANCEL_HOVER).clicked();
    });
    if let Some(search) = &s.people.search {
        super::progress_line::show(ui, "Looking there", search.progress(), THERE_HOVER);
    }
    if remove {
        let new = people_tool::removals(&s.people.people, &s.people.ticked);
        s.edits.removals.extend(new);
        s.removal.sync(s.edits.removals.len());
        s.removal.selected = None;
        ed.discrete();
    }
    if remove || cancel {
        s.people.active = false;
        s.people.look = None;
        s.people.people.clear();
        s.people.ticked.clear();
        s.people.said = None;
    }
}

#[cfg(test)]
mod tests {
    use crate::develop::panel::{retouch, Edited};
    use rustroom_app::people_tool::{Person, Search};
    use std::sync::Arc;

    fn person(x: f32, tall: f32) -> Person {
        Person {
            bbox: [x, 0.1, x + 0.05, 0.1 + tall],
            score: 0.9,
            tall,
            area: Arc::new(rustroom_app::remove_tool::Area {
                rect: [0.0; 4],
                side: 1,
                alpha: vec![0.0],
            }),
            outline: Vec::new(),
        }
    }

    struct Panel {
        ctx: eframe::egui::Context,
        app: crate::RustroomApp,
    }

    impl Panel {
        /// Retouch in Remove, its default mode, with `people` found and `ticked`.
        fn new(people: Vec<Person>, ticked: Vec<bool>) -> Self {
            let ctx = crate::test_ui::ctx();
            let mut app = crate::test_app::for_test();
            crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
            assert!(app.remove_tool.active, "the fixture: Remove");
            let s = app.session.as_mut().unwrap();
            s.people.active = !people.is_empty();
            s.people.people = people;
            s.people.ticked = ticked;
            Self { ctx, app }
        }

        fn draw(&mut self) -> impl FnMut(&mut eframe::egui::Ui) + '_ {
            |ui| {
                let mut ed = Edited::default();
                self.frame(ui, &mut ed);
            }
        }

        fn frame(&mut self, ui: &mut eframe::egui::Ui, ed: &mut Edited) {
            let (mut t, mut e, mut r) =
                (self.app.spot_tool, self.app.eye_tool, self.app.remove_tool);
            retouch::show_with(
                ui,
                self.app.session.as_mut().unwrap(),
                &mut t,
                &mut e,
                &mut r,
                ed,
            );
            (self.app.spot_tool, self.app.eye_tool, self.app.remove_tool) = (t, e, r);
        }

        /// A click on `label`, and whether the frame settled one history step.
        fn click(&mut self, label: &str) -> bool {
            let ctx = self.ctx.clone();
            let mut settled = false;
            crate::test_ui::click(&ctx, label, |ui| {
                let mut ed = Edited::default();
                self.frame(ui, &mut ed);
                settled |= ed.settled;
            });
            settled
        }

        fn labels(&mut self) -> Vec<String> {
            let ctx = self.ctx.clone();
            crate::test_ui::nodes(&ctx, self.draw())
                .into_iter()
                .map(|n| n.label)
                .collect()
        }

        fn s(&self) -> &crate::Session {
            self.app.session.as_ref().unwrap()
        }
    }

    fn three() -> Panel {
        Panel::new(
            vec![person(0.1, 0.62), person(0.4, 0.10), person(0.7, 0.20)],
            vec![false, true, true],
        )
    }

    #[test]
    fn the_people_found_are_counted() {
        let mut p = three();
        let labels = p.labels();
        assert!(
            labels.iter().any(|l| l == "3 people · 2 to remove"),
            "{labels:?}"
        );
        assert!(labels.iter().any(|l| l == "Remove 2 people"), "{labels:?}");
    }

    /// Review Focus 4: Remove 2 people adds two removals of their boxes, as one step.
    #[test]
    fn remove_people_is_one_undo_step() {
        let mut p = three();
        assert!(p.click("Remove 2 people"), "one history step");
        let rs = &p.s().edits.removals;
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].person, Some(person(0.4, 0.10).bbox));
        assert_eq!(rs[1].person, Some(person(0.7, 0.20).bbox));
        assert!(p.s().people.people.is_empty(), "the people found are done");
        assert!(!p.s().people.active, "the view ends");
    }

    #[test]
    fn cancel_removes_nobody() {
        let mut p = three();
        assert!(!p.click("Cancel"), "no step");
        assert!(p.s().edits.removals.is_empty());
        assert!(p.s().people.people.is_empty());
        assert!(!p.s().people.active, "the view ends");
    }

    #[test]
    fn everyone_ticks_all_and_none() {
        let mut p = three();
        p.click("Everyone");
        assert_eq!(p.s().people.ticked, vec![true, true, true]);
        p.click("Everyone");
        assert_eq!(p.s().people.ticked, vec![false, false, false]);
        let labels = p.labels();
        assert!(
            labels.iter().any(|l| l == "3 people · 0 to remove"),
            "{labels:?}"
        );
    }

    #[test]
    fn no_people_found_stays_under_the_button() {
        // The view stays on with nobody found — so one can draw over someone it missed, or Cancel.
        let mut p = Panel::new(Vec::new(), Vec::new());
        p.app.session.as_mut().unwrap().people.active = true;
        let labels = p.labels();
        assert!(labels.iter().any(|l| l == "No people found"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "Cancel"), "{labels:?}");
        // ⚠ "Remove dust" also starts so — the people's button ends in "people" or "person".
        assert!(
            !labels
                .iter()
                .any(|l| l.starts_with("Remove ")
                    && (l.ends_with("people") || l.ends_with("person"))),
            "nothing to remove: {labels:?}"
        );
        p.click("Cancel");
        assert!(!p.s().people.active);
    }

    /// A look there keeps the people and their buttons, and says what it is doing under them.
    #[test]
    fn a_look_there_says_so_under_the_people() {
        let mut p = three();
        let (search, _answer) = Search::answered_by(0.4);
        let s = p.app.session.as_mut().unwrap();
        s.people.search = Some(search);
        s.people.there = true;
        let labels = p.labels();
        assert!(labels.iter().any(|l| l == "Looking there"), "{labels:?}");
        assert!(
            labels.iter().any(|l| l == "3 people · 2 to remove"),
            "{labels:?}"
        );
    }

    /// While it looks, the line says so — and the button waits, so a second press cannot start a second search.
    #[test]
    fn while_it_looks_the_line_says_so_and_the_button_waits() {
        let mut p = Panel::new(Vec::new(), Vec::new());
        let (search, _answer) = Search::answered_by(0.4);
        p.app.session.as_mut().unwrap().people.search = Some(search);
        assert!(p.labels().iter().any(|l| l == "Looking for people"));
        p.click("Find people");
        assert!(!p.s().people.pressed, "a press while it looks");
    }

    #[test]
    fn find_people_is_pressed() {
        let mut p = Panel::new(Vec::new(), Vec::new());
        p.click("Find people");
        assert!(p.s().people.pressed);
    }
}
