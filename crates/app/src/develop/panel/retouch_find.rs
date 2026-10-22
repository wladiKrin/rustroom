//! Retouch's Find card (M10.19d): **People · Dust**, the button that finds them, the quiet line while it looks, and
//! the review — the readout, **On the photo: Pick · Add more**, Everyone, **Remove N** and Cancel. People and dust
//! are reviewed the same way; dust's review also carries the dust view.
//!
//! ⚠ The searches are `people_flow`'s and `dust_flow`'s; this card only asks and shows. After Remove N the panel
//! goes back to Paint with the brush that made them, so what was done shows on the photo.

use super::{retouch_brush, section, Edited};
use crate::segmented::{Mark, Segment};
use crate::theme;
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::eye_tool::EyeTool;
use rustroom_app::people_tool::{self, Wait};
use rustroom_app::remove_tool::RemoveTool;
use rustroom_app::retouch_list::Brush;
use rustroom_app::retouch_task::{FindKind, Gesture, RetouchState, Task};
use rustroom_app::slider_help::{self, Scope};
use rustroom_app::spot_tool::SpotTool;
use rustroom_app::{dust_tool, icons};

const PEOPLE_HOVER: &str = "Looks for people in the photo";
const DUST_HOVER: &str = "Looks for sensor dust in the photo";
const FIND_PEOPLE_HOVER: &str = "Finds the people in the photo so you can pick the ones to remove";
const FIND_DUST_HOVER: &str = "Finds sensor dust in the photo so you can pick the spots to heal";
const LOOKING_HOVER: &str =
    "Each one found is shaded on the photo, the ones to remove in the accent";
const THERE_HOVER: &str = "Looks again where you drew, closer, and adds what it finds there";
const PICK_HOVER: &str = "A click on the photo picks or unpicks one of those found";
const ADD_HOVER: &str = "Drawing over something it missed looks for it again there, closer";
const EVERYONE_PEOPLE_HOVER: &str = "Picks every person found to remove, or none";
const EVERYONE_DUST_HOVER: &str = "Picks every spot found to heal, or none";
const REMOVE_PEOPLE_HOVER: &str = "Removes each person picked, filled from their surroundings";
const REMOVE_DUST_HOVER: &str = "Heals each spot picked with a patch";
const CANCEL_HOVER: &str = "Stops showing what was found without removing anything";
const DUST_VIEW_HOVER: &str =
    "Shows a high-contrast view that makes sensor dust stand out, in the view only";

/// The "Find" card.
pub(super) fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    tool: &mut SpotTool,
    eyes: &mut EyeTool,
    remove: &mut RemoveTool,
    state: &mut RetouchState,
    ed: &mut Edited,
) {
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("retouch", "Find")),
        true,
        section::Header::new("Find"),
    );
    out.show_body(ui, |ui| {
        theme::segmented(
            ui,
            &mut state.find,
            &[
                (FindKind::People, "People", PEOPLE_HOVER),
                (FindKind::Dust, "Dust", DUST_HOVER),
            ],
        );
        ui.add_space(4.0);
        let done = match state.find {
            FindKind::People => people(ui, s, state),
            FindKind::Dust => dust(ui, s, tool, state),
        };
        match done {
            Done::Nothing => {}
            Done::Removed(brush) => {
                ed.discrete();
                state.task = Task::Paint;
                retouch_brush::set(s, tool, eyes, remove, brush);
            }
        }
    });
}

/// What a review's buttons did.
enum Done {
    Nothing,
    /// Remove N was pressed and made retouches with this brush.
    Removed(Brush),
}

/// **On the photo: Pick · Add more** — what a click or drawing on the photo does while a review is on.
fn gesture(ui: &mut egui::Ui, state: &mut RetouchState) {
    theme::sub_label(ui, "On the photo");
    let font = egui::TextStyle::Body.resolve(ui.style());
    crate::segmented::segmented_marked(
        ui,
        &mut state.gesture,
        &[
            Segment {
                value: Gesture::Pick,
                label: "Pick",
                hover: PICK_HOVER,
                mark: Mark::Icon(icons::PICK),
            },
            Segment {
                value: Gesture::Add,
                label: "Add more",
                hover: ADD_HOVER,
                mark: Mark::Icon(icons::ADD_MORE),
            },
        ],
        28.0,
        font,
    );
}

/// Everyone over `ticked`: all, none, or mixed.
fn everyone(ui: &mut egui::Ui, ticked: &mut [bool], hover: &str) {
    let n = ticked.len();
    let k = ticked.iter().filter(|t| **t).count();
    let mut all = k == n;
    if ui
        .add(egui::Checkbox::new(&mut all, "Everyone").indeterminate(k > 0 && k < n))
        .hover(hover)
        .changed()
    {
        ticked.iter_mut().for_each(|t| *t = all);
    }
}

/// Remove N (the card's primary, off with none ticked) and Cancel, for `n` found. Returns (remove, cancel).
fn verbs(ui: &mut egui::Ui, n: usize, ticked: usize, label: &str, hover: &str) -> (bool, bool) {
    let (mut remove, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if n > 0 {
            remove = theme::primary_button(ui, label, ticked > 0)
                .hover(hover)
                .clicked();
        }
        cancel = ui.button("Cancel").hover(CANCEL_HOVER).clicked();
    });
    (remove, cancel)
}

fn people(ui: &mut egui::Ui, s: &mut Session, state: &mut RetouchState) -> Done {
    let looking = s.people.search.is_some();
    let label = if s.people.active {
        "Find again"
    } else {
        "Find people"
    };
    if ui
        .add_enabled(!looking, egui::Button::new(label))
        .hover(FIND_PEOPLE_HOVER)
        .clicked()
    {
        s.people.pressed = true;
    }
    // The whole search: its line alone. A look there keeps the review, its line under it.
    if let Some(search) = s.people.search.as_ref().filter(|_| !s.people.there) {
        super::progress_line::show(ui, "Looking for people", search.progress(), LOOKING_HOVER);
        return Done::Nothing;
    }
    match s.people.waiting {
        Some(Wait::Photo) => {
            theme::hint(ui, "Waits for the photo to load");
            return Done::Nothing;
        }
        Some(Wait::Models) => {
            theme::hint(ui, "Waits for its models to download");
            return Done::Nothing;
        }
        None => {}
    }
    if !s.people.active {
        if let Some(said) = &s.people.said {
            theme::hint(ui, said);
        }
        return Done::Nothing;
    }
    let n = s.people.people.len();
    s.people.ticked.resize(n, false);
    let k = s.people.ticked.iter().filter(|t| **t).count();
    theme::hint(ui, &people_tool::summary(n, k));
    if let Some(said) = &s.people.said {
        theme::hint(ui, said);
    }
    gesture(ui, state);
    if n > 0 {
        everyone(ui, &mut s.people.ticked, EVERYONE_PEOPLE_HOVER);
    }
    let (remove, cancel) = verbs(ui, n, k, &people_tool::remove_label(k), REMOVE_PEOPLE_HOVER);
    if let Some(search) = &s.people.search {
        super::progress_line::show(ui, "Looking there", search.progress(), THERE_HOVER);
    }
    let mut done = Done::Nothing;
    if remove {
        let new = people_tool::removals(&s.people.people, &s.people.ticked);
        s.edits.removals.extend(new);
        s.removal.sync(s.edits.removals.len());
        s.removal.selected = None;
        done = Done::Removed(Brush::Remove);
    }
    if remove || cancel {
        s.people.active = false;
        s.people.look = None;
        s.people.people.clear();
        s.people.ticked.clear();
        s.people.said = None;
    }
    done
}

fn dust(ui: &mut egui::Ui, s: &mut Session, tool: &mut SpotTool, state: &mut RetouchState) -> Done {
    ui.add(
        super::slider::Slider::new(&mut tool.dust, 0.0..=100.0)
            .text("Sensitivity")
            .help(slider_help::text(Scope::Patch, "Sensitivity"))
            .reset_to(dust_tool::DEFAULT_SENSITIVITY),
    );
    let looking = s.dust.search.is_some();
    let label = if s.dust.active {
        "Find again"
    } else {
        "Find dust"
    };
    if ui
        .add_enabled(!looking, egui::Button::new(label))
        .hover(FIND_DUST_HOVER)
        .clicked()
    {
        s.press_find_dust(tool.dust);
    }
    if let Some(search) = s.dust.search.as_ref().filter(|_| s.dust.there.is_none()) {
        super::progress_line::show(ui, "Looking for dust", search.progress(), LOOKING_HOVER);
        return Done::Nothing;
    }
    if s.dust.queued.is_some() {
        theme::hint(ui, "Waits for the photo to load");
        return Done::Nothing;
    }
    if !s.dust.active {
        if let Some(said) = &s.dust.said {
            theme::hint(ui, said);
        }
        return Done::Nothing;
    }
    let n = s.dust.finds.len();
    s.dust.ticked.resize(n, true);
    let k = s.dust.ticked.iter().filter(|t| **t).count();
    theme::hint(ui, &dust_tool::summary(n, k));
    if let Some(said) = &s.dust.said {
        theme::hint(ui, said);
    }
    gesture(ui, state);
    ui.checkbox(&mut s.dust.view, "Dust view")
        .hover(DUST_VIEW_HOVER);
    if s.dust.view {
        ui.add(
            super::slider::Slider::new(
                &mut s.dust.contrast,
                rustroom_app::visualize::MIN_THRESHOLD..=rustroom_app::visualize::MAX_THRESHOLD,
            )
            .text("Contrast")
            .help(Some(
                "Raises the dust view's gain, so fainter dust shows".to_string(),
            )),
        );
    }
    if n > 0 {
        everyone(ui, &mut s.dust.ticked, EVERYONE_DUST_HOVER);
    }
    let (remove, cancel) = verbs(ui, n, k, &dust_tool::remove_label(k), REMOVE_DUST_HOVER);
    if let Some(search) = &s.dust.search {
        super::progress_line::show(ui, "Looking there", search.progress(), THERE_HOVER);
    }
    if remove {
        // ⚠ Spots already under a patch make none: no patch, no undo step, the review over all the same.
        return match s.remove_dust(*tool) {
            0 => Done::Nothing,
            _ => Done::Removed(Brush::Heal),
        };
    }
    if cancel {
        s.dust.active = false;
        s.dust.look = None;
        s.dust.finds.clear();
        s.dust.ticked.clear();
        s.dust.said = None;
    }
    Done::Nothing
}

#[cfg(test)]
mod tests {
    use crate::develop::panel::retouch::tests::Panel;
    use rustroom_app::dust_tool::{self, Find};
    use rustroom_app::people_tool::{Person, Search};
    use rustroom_app::retouch_list::Brush;
    use rustroom_app::retouch_task::{FindKind, Gesture, Task};
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

    fn a_find(x: f32) -> Find {
        Find {
            centre: [x, 0.5],
            radius: 0.01,
            contrast: 0.3,
        }
    }

    /// The panel in Find on `kind`.
    fn finding(kind: FindKind) -> Panel {
        let mut p = Panel::new();
        p.app.retouch.task = Task::Find;
        p.app.retouch.find = kind;
        p
    }

    /// Three people found, the two small ones ticked.
    fn three() -> Panel {
        let mut p = finding(FindKind::People);
        let s = p.s_mut();
        s.people.active = true;
        s.people.people = vec![person(0.1, 0.62), person(0.4, 0.10), person(0.7, 0.20)];
        s.people.ticked = vec![false, true, true];
        p
    }

    /// Two spots of dust found, both ticked.
    fn two_spots() -> Panel {
        let mut p = finding(FindKind::Dust);
        let s = p.s_mut();
        s.dust.active = true;
        s.dust.finds = vec![a_find(0.3), a_find(0.7)];
        s.dust.ticked = vec![true, true];
        p
    }

    fn has(p: &mut Panel, label: &str) -> bool {
        p.labels().iter().any(|l| l == label)
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

    /// Remove 2 people adds two removals of their boxes as one step, then Paint shows them with the brush that made
    /// them.
    #[test]
    fn remove_people_is_one_undo_step_and_returns_to_paint() {
        let mut p = three();
        assert!(p.click("Remove 2 people").settled, "one history step");
        let rs = &p.s().edits.removals;
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].person, Some(person(0.4, 0.10).bbox));
        assert_eq!(rs[1].person, Some(person(0.7, 0.20).bbox));
        assert!(p.s().people.people.is_empty(), "the people found are done");
        assert!(!p.s().people.active, "the review ends");
        assert_eq!(p.app.retouch.task, Task::Paint);
        assert_eq!(p.brush(), Brush::Remove);
    }

    #[test]
    fn cancel_removes_nobody() {
        let mut p = three();
        assert!(!p.click("Cancel").settled, "no step");
        assert!(p.s().edits.removals.is_empty());
        assert!(p.s().people.people.is_empty());
        assert!(!p.s().people.active, "the review ends");
        assert_eq!(p.app.retouch.task, Task::Find, "Cancel stays in Find");
    }

    #[test]
    fn everyone_ticks_all_and_none() {
        let mut p = three();
        p.click("Everyone");
        assert_eq!(p.s().people.ticked, vec![true, true, true]);
        p.click("Everyone");
        assert_eq!(p.s().people.ticked, vec![false, false, false]);
        assert!(has(&mut p, "3 people · 0 to remove"));
    }

    #[test]
    fn no_people_found_stays_in_the_review() {
        // The review stays on with nobody found — so one can draw over someone it missed, or Cancel.
        let mut p = finding(FindKind::People);
        p.s_mut().people.active = true;
        let labels = p.labels();
        assert!(labels.iter().any(|l| l == "No people found"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "Add more"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "Cancel"), "{labels:?}");
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
        let s = p.s_mut();
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
        let mut p = finding(FindKind::People);
        let (search, _answer) = Search::answered_by(0.4);
        p.s_mut().people.search = Some(search);
        assert!(has(&mut p, "Looking for people"));
        p.click("Find people");
        assert!(!p.s().people.pressed, "a press while it looks");
    }

    #[test]
    fn find_people_is_pressed() {
        let mut p = finding(FindKind::People);
        p.click("Find people");
        assert!(p.s().people.pressed);
    }

    /// ⚠ Before a review there is nothing on the photo to pick, so the gesture is not offered.
    #[test]
    fn the_gesture_shows_only_in_a_review_and_switches() {
        let mut p = finding(FindKind::People);
        assert!(
            !has(&mut p, "Add more"),
            "a gesture before anything was found"
        );
        let mut p = three();
        assert_eq!(p.app.retouch.gesture, Gesture::Pick, "Pick is the default");
        p.click("Add more");
        assert_eq!(p.app.retouch.gesture, Gesture::Add);
        p.click("Pick");
        assert_eq!(p.app.retouch.gesture, Gesture::Pick);
    }

    #[test]
    fn people_and_dust_switch_the_card() {
        let mut p = finding(FindKind::People);
        assert!(has(&mut p, "Find people"));
        p.click("Dust");
        assert_eq!(p.app.retouch.find, FindKind::Dust);
        let labels = p.labels();
        assert!(labels.iter().any(|l| l == "Find dust"), "{labels:?}");
        assert!(labels.iter().any(|l| l == "Sensitivity"), "{labels:?}");
        assert!(!labels.iter().any(|l| l == "Find people"));
    }

    #[test]
    fn the_dust_found_is_counted() {
        let mut p = two_spots();
        let labels = p.labels();
        assert!(
            labels.iter().any(|l| l == &dust_tool::summary(2, 2)),
            "{labels:?}"
        );
        assert!(labels.iter().any(|l| l == "Remove 2 spots"), "{labels:?}");
    }

    /// Review Focus 4: Remove N spots heals each ticked one with a patch MARKED dust, as one step, then Paint shows
    /// them with the Heal brush.
    #[test]
    fn removing_dust_spots_makes_marked_heal_patches() {
        let mut p = two_spots();
        p.s_mut().dust.ticked = vec![true, false];
        assert!(p.click("Remove 1 spot").settled, "one history step");
        let spots = &p.s().edits.spots;
        assert_eq!(spots.len(), 1, "only the ticked spot");
        assert!(spots[0].dust, "the patch lost its dust mark");
        assert!(!p.s().dust.active, "the review ends");
        assert_eq!(p.app.retouch.task, Task::Paint);
        assert_eq!(p.brush(), Brush::Heal);
    }

    #[test]
    fn cancelling_dust_heals_nothing() {
        let mut p = two_spots();
        assert!(!p.click("Cancel").settled);
        assert!(p.s().edits.spots.is_empty());
        assert!(!p.s().dust.active);
        assert!(p.s().dust.finds.is_empty());
    }

    #[test]
    fn none_ticked_removes_nothing() {
        let mut p = two_spots();
        p.click("Everyone");
        assert_eq!(p.s().dust.ticked, vec![false, false]);
        let ed = p.click("Remove 0 spots");
        assert!(!ed.settled, "a press with none ticked");
        assert!(p.s().edits.spots.is_empty());
        assert!(p.s().dust.active, "the review went on");
    }

    /// The dust view's switch and its Contrast belong to the review; off, Contrast goes.
    #[test]
    fn the_dust_view_switch_hides_its_contrast() {
        let mut p = two_spots();
        assert!(has(&mut p, "Contrast"));
        p.click("Dust view");
        assert!(!p.s().dust.view);
        assert!(!has(&mut p, "Contrast"), "Contrast without the view");
        assert!(
            p.s().edits.spots.is_empty(),
            "a view switch healed something"
        );
    }

    #[test]
    fn find_dust_starts_a_search() {
        let mut p = finding(FindKind::Dust);
        p.click("Find dust");
        let d = &p.s().dust;
        assert!(
            d.search.is_some() || d.queued.is_some(),
            "the press did nothing"
        );
    }
}
