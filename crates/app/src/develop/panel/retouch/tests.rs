//! The Retouch panel's tests (M10.19d): the task switch, the brush, the one list and the selected retouch's settings
//! — every behaviour the three old lists' tests pinned, moved to where its control now lives. The Find card's own are
//! in `retouch_find`.

use super::*;
use crate::develop::panel::retouch_brush;
use edit_model::{EyeKind, SpotMode};
use rustroom_app::retouch_list::Brush;

pub(crate) fn a_spot() -> edit_model::Spot {
    edit_model::Spot {
        decode: false,
        centre: [0.5, 0.5],
        source: [0.2, 0.2],
        radius: 0.02,
        feather: 50.0,
        mode: SpotMode::Heal,
        dust: false,
    }
}

pub(crate) fn an_eye() -> edit_model::Eye {
    edit_model::Eye {
        decode: false,
        centre: [0.5, 0.5],
        radius: 0.02,
        kind: EyeKind::Red,
        pupil_size: 50.0,
        darken: 50.0,
        catchlight: None,
    }
}

pub(crate) fn a_removal() -> edit_model::Removal {
    edit_model::Removal {
        points: vec![[0.3, 0.5]],
        radius: 0.02,
        detect: false,
        variant: 0,
        opacity: 100.0,
        person: None,
    }
}

/// The panel with one context for its whole life, drawn through the real entry point with the app's own tools.
pub(crate) struct Panel {
    pub ctx: egui::Context,
    pub app: crate::RustroomApp,
}

impl Panel {
    pub fn new() -> Self {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        Self { ctx, app }
    }

    pub fn with(
        spots: Vec<edit_model::Spot>,
        removals: Vec<edit_model::Removal>,
        eyes: Vec<edit_model::Eye>,
    ) -> Self {
        let mut p = Self::new();
        let s = p.app.session.as_mut().unwrap();
        s.edits.spots = spots;
        s.edits.removals = removals;
        s.edits.eyes = eyes;
        p
    }

    /// One frame of the panel, reporting what it edited.
    pub fn frame(&mut self, ui: &mut egui::Ui) -> Edited {
        let mut ed = Edited::default();
        let (mut t, mut e, mut r, mut st) = (
            self.app.spot_tool,
            self.app.eye_tool,
            self.app.remove_tool,
            self.app.retouch,
        );
        show_with(
            ui,
            self.app.session.as_mut().unwrap(),
            &mut t,
            &mut e,
            &mut r,
            &mut st,
            &mut ed,
        );
        (
            self.app.spot_tool,
            self.app.eye_tool,
            self.app.remove_tool,
            self.app.retouch,
        ) = (t, e, r, st);
        ed
    }

    /// A click on `label`, and what the frames reported.
    pub fn click(&mut self, label: &str) -> Edited {
        let ctx = self.ctx.clone();
        let mut seen = Edited::default();
        crate::test_ui::click(&ctx, label, |ui| {
            let ed = self.frame(ui);
            seen.changed |= ed.changed;
            seen.settled |= ed.settled;
        });
        seen
    }

    /// Every drawn label.
    pub fn labels(&mut self) -> Vec<String> {
        let ctx = self.ctx.clone();
        crate::test_ui::nodes(&ctx, |ui| {
            self.frame(ui);
        })
        .into_iter()
        .map(|n| n.label)
        .collect()
    }

    /// Where each widget labelled `label` sits, top to bottom.
    pub fn all(&mut self, label: &str) -> Vec<egui::Pos2> {
        let ctx = self.ctx.clone();
        let mut at: Vec<egui::Pos2> = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            self.frame(ui);
        })
        .into_iter()
        .filter(|(l, _)| l == label)
        .map(|(_, p)| p)
        .collect();
        at.sort_by(|a, b| a.y.total_cmp(&b.y));
        at
    }

    /// A click at a point.
    pub fn click_at(&mut self, at: egui::Pos2) -> Edited {
        let ctx = self.ctx.clone();
        let mut seen = Edited::default();
        crate::test_ui::click_at(&ctx, at, false, |ui| {
            let ed = self.frame(ui);
            seen.changed |= ed.changed;
            seen.settled |= ed.settled;
        });
        seen
    }

    /// A drag from `at` by `by` pixels across; `mid` sees the session halfway.
    pub fn drag(
        &mut self,
        at: egui::Pos2,
        by: f32,
        mut mid: impl FnMut(&crate::Session),
    ) -> Edited {
        let mut out = Edited::default();
        let ctx = self.ctx.clone();
        for pass in 0..4 {
            let pos = if pass <= 1 {
                at
            } else {
                egui::pos2(at.x + by, at.y)
            };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pass == 1 || pass == 3 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let mut ed = Edited::default();
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::test_ui::screen()),
                    events,
                    ..Default::default()
                },
                |ui| ed = self.frame(ui),
            );
            if pass == 2 {
                mid(self.s());
            }
            out.changed |= ed.changed;
            out.settled |= ed.settled;
        }
        out
    }

    pub fn s(&self) -> &crate::Session {
        self.app.session.as_ref().unwrap()
    }

    pub fn s_mut(&mut self) -> &mut crate::Session {
        self.app.session.as_mut().unwrap()
    }

    pub fn brush(&self) -> Brush {
        retouch_brush::current(
            &self.app.spot_tool,
            &self.app.eye_tool,
            &self.app.remove_tool,
        )
    }
}

/// ⚠ One click, one history entry — and it must clear every selection too, or the next keystroke would act on an
/// index into an empty list.
#[test]
fn the_reset_removes_every_retouch_and_marks_one_edit() {
    let mut p = Panel::with(vec![a_spot(), a_spot()], vec![a_removal()], vec![an_eye()]);
    p.s_mut().selected_spot = Some(1);
    let ed = p.click("reset");
    let s = p.s();
    assert!(s.edits.spots.is_empty() && s.edits.removals.is_empty() && s.edits.eyes.is_empty());
    assert_eq!(
        s.selected_spot, None,
        "the selection outlived the retouches"
    );
    assert!(
        ed.changed && ed.settled,
        "one click must be one history entry"
    );
}

/// ⚠ The discriminating other half: merely drawing the panel must change nothing.
#[test]
fn merely_drawing_the_panel_changes_nothing() {
    let mut p = Panel::with(vec![a_spot()], vec![a_removal()], vec![an_eye()]);
    for _ in 0..3 {
        let ctx = p.ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::test_ui::screen()),
                ..Default::default()
            },
            |ui| {
                let ed = p.frame(ui);
                assert!(!ed.changed, "drawing the panel reported an edit");
            },
        );
    }
    assert_eq!(
        p.s().edits.spots.len(),
        1,
        "drawing the panel removed a spot"
    );
}

#[test]
fn find_hides_the_brush_and_shows_the_find_card() {
    let mut p = Panel::new();
    let paint = p.labels();
    assert!(paint.iter().any(|l| l == "Whole object"), "{paint:?}");
    assert!(!paint.iter().any(|l| l == "Find people"));
    p.click("Find");
    let find = p.labels();
    assert!(find.iter().any(|l| l == "Find people"), "{find:?}");
    assert!(
        !find.iter().any(|l| l == "Whole object"),
        "the brush is hidden in Find"
    );
    assert!(
        find.iter().any(|l| l == "Retouches"),
        "the list stays in both"
    );
    p.click("Paint");
    assert!(p.labels().iter().any(|l| l == "Whole object"));
}

#[test]
fn remove_is_the_first_and_default_brush() {
    let p = Panel::new();
    assert_eq!(p.brush(), Brush::Remove);
    assert_eq!(p.app.retouch.task, Task::Paint);
}

/// Each brush shows its own settings: Remove its Whole object, Heal and Clone their Feather.
#[test]
fn the_brush_switches_between_remove_and_heal() {
    let mut p = Panel::new();
    p.click("Heal");
    assert_eq!(p.brush(), Brush::Heal);
    let labels = p.labels();
    assert!(labels.iter().any(|l| l == "Feather"), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "Whole object"));
    p.click("Clone");
    assert_eq!(p.app.spot_tool.mode, SpotMode::Clone);
    p.click("Remove");
    assert_eq!(p.brush(), Brush::Remove);
    assert!(p.labels().iter().any(|l| l == "Whole object"));
}

/// Review Focus 2: a row picked selects that retouch alone, and Paint and its brush come with it — from Find too.
#[test]
fn picking_a_row_selects_it_alone_and_shows_its_brush() {
    let mut p = Panel::with(vec![a_spot()], vec![a_removal()], vec![an_eye()]);
    p.app.retouch.task = Task::Find;
    p.click("Heal 1");
    assert_eq!(
        (
            p.s().selected_spot,
            p.s().removal.selected,
            p.s().selected_eye
        ),
        (Some(0), None, None)
    );
    assert_eq!(p.app.retouch.task, Task::Paint);
    assert_eq!(p.brush(), Brush::Heal);
    p.click("Removal 1");
    assert_eq!(
        (
            p.s().selected_spot,
            p.s().removal.selected,
            p.s().selected_eye
        ),
        (None, Some(0), None)
    );
    assert_eq!(p.brush(), Brush::Remove);
    p.click("Red eye 1");
    assert_eq!(
        (
            p.s().selected_spot,
            p.s().removal.selected,
            p.s().selected_eye
        ),
        (None, None, Some(0))
    );
    assert_eq!(p.brush(), Brush::Eye);
    assert_eq!(p.app.eye_tool.active, Some(EyeKind::Red));
}

#[test]
fn clicking_the_selected_row_deselects_it() {
    let mut p = Panel::with(vec![a_spot()], Vec::new(), Vec::new());
    p.click("Heal 1");
    assert_eq!(p.s().selected_spot, Some(0));
    // ⚠ The selected settings' heading reads "Heal 1" too: the row is the upper one.
    let rows = p.all("Heal 1");
    assert_eq!(rows.len(), 2, "the row and its settings' heading: {rows:?}");
    p.click_at(rows[0]);
    assert_eq!(p.s().selected_spot, None);
}

/// Review Focus 1: with a patch selected, the brush's Size moves the brush and never the patch.
#[test]
fn the_brush_never_edits_a_selected_retouch() {
    let mut p = Panel::with(vec![a_spot()], Vec::new(), Vec::new());
    p.click("Heal 1");
    let sizes = p.all("Size");
    assert_eq!(sizes.len(), 2, "the brush's and the patch's: {sizes:?}");
    let (before, tool) = (p.s().edits.spots[0], p.app.spot_tool.radius);
    let ed = p.drag(sizes[0], 200.0, |_| {});
    assert_eq!(
        p.s().edits.spots[0],
        before,
        "the brush edited the selected patch"
    );
    assert_ne!(p.app.spot_tool.radius, tool, "the drag did nothing at all");
    assert!(!ed.settled, "a brush setting pushed a history entry");
}

/// The selected patch's own Size edits it and nothing else — and renders once, on release, with the patch out of the
/// view while it is dragged (the rule moving one follows).
#[test]
fn the_selected_patch_s_settings_edit_it_only() {
    let mut p = Panel::with(vec![a_spot()], Vec::new(), Vec::new());
    p.click("Heal 1");
    let sizes = p.all("Size");
    let (before, tool) = (p.s().edits.spots[0].radius, p.app.spot_tool.radius);
    let mut mid = None;
    let ed = p.drag(sizes[1], 200.0, |s| {
        mid = Some((s.suppressed_spot, s.edits.spots[0].radius));
    });
    let (suppressed, radius_mid) = mid.expect("the drag never reached its middle");
    assert_ne!(
        radius_mid, before,
        "the stored patch did not follow the slider"
    );
    assert_eq!(
        suppressed,
        Some(0),
        "the patch stayed in the view while dragged"
    );
    assert_eq!(p.s().suppressed_spot, None, "the patch never came back");
    assert!(ed.settled, "no history entry on release");
    assert_eq!(
        p.app.spot_tool.radius, tool,
        "the selected patch moved the brush"
    );
}

/// ⚠⚠ A suppression the CANVAS owns must survive the panel being drawn (a heal flickered away during a move).
#[test]
fn the_panel_leaves_a_suppression_it_did_not_start_alone() {
    let mut p = Panel::with(vec![a_spot()], Vec::new(), Vec::new());
    p.s_mut().selected_spot = Some(0);
    p.s_mut().suppressed_spot = Some(0);
    p.app.remove_tool.active = false;
    for _ in 0..3 {
        let ctx = p.ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::test_ui::screen()),
                ..Default::default()
            },
            |ui| {
                let ed = p.frame(ui);
                assert!(
                    !ed.changed,
                    "the panel requested a render during a canvas move"
                );
            },
        );
    }
    assert_eq!(
        p.s().suppressed_spot,
        Some(0),
        "the panel released a suppression the canvas owns"
    );
}

#[test]
fn choosing_eye_switches_the_tool_to_eyes_and_deselects_the_patch() {
    let mut p = Panel::with(vec![a_spot()], Vec::new(), Vec::new());
    p.click("Heal 1");
    let before = p.s().edits.clone();
    p.click("Eye");
    assert_eq!(p.app.eye_tool.active, Some(EyeKind::Red));
    assert_eq!(p.s().selected_spot, None);
    assert_eq!(
        p.s().edits,
        before,
        "switching the brush edited the photograph"
    );
}

/// A selected eye's own Red eye · Pet eye changes it, one step — the brush's leaves it alone.
#[test]
fn a_selected_eye_s_kind_switch_changes_it_as_one_step() {
    let mut p = Panel::with(Vec::new(), Vec::new(), vec![an_eye()]);
    p.click("Red eye 1");
    let pets = p.all("Pet eye");
    assert_eq!(pets.len(), 2, "the brush's and the eye's: {pets:?}");
    p.click_at(pets[0]);
    assert_eq!(
        p.s().edits.eyes[0].kind,
        EyeKind::Red,
        "the brush's switch edited the eye"
    );
    let at = p.all("Pet eye")[1];
    let ed = p.click_at(at);
    let e = p.s().edits.eyes[0];
    assert_eq!(e.kind, EyeKind::Pet);
    assert!(
        e.catchlight.is_some(),
        "the default catchlight did not come with it"
    );
    assert!(ed.settled, "not an undo step");
}

#[test]
fn heal_from_eye_returns_to_patches() {
    let mut p = Panel::with(Vec::new(), Vec::new(), vec![an_eye()]);
    p.click("Red eye 1");
    p.click("Heal");
    assert_eq!(p.app.eye_tool.active, None);
    assert_eq!(p.s().selected_eye, None);
    assert_eq!(p.s().edits.eyes[0].kind, EyeKind::Red, "the eye was edited");
}

#[test]
fn an_eye_row_selects_it_and_its_delete_removes_it_as_one_step() {
    let mut p = Panel::with(Vec::new(), Vec::new(), vec![an_eye(), an_eye()]);
    p.click("Red eye 2");
    assert_eq!(p.s().selected_eye, Some(1));
    let ed = p.click("Delete Red eye 1");
    assert_eq!(p.s().edits.eyes.len(), 1);
    assert_eq!(
        p.s().selected_eye,
        Some(0),
        "the selection followed the survivor"
    );
    assert!(ed.settled, "a delete is one history entry");
}

#[test]
fn the_eye_hides_a_patchs_outline_and_edits_nothing() {
    let mut p = Panel::with(vec![a_spot()], Vec::new(), Vec::new());
    let before = p.s().edits.spots.clone();
    let ed = p.click("Show Heal 1");
    assert_eq!(
        p.s().spot_shown,
        vec![false],
        "the eye did not hide the outline"
    );
    assert_eq!(
        p.s().edits.spots,
        before,
        "hiding an outline edited a patch"
    );
    assert!(!ed.settled, "a view toggle pushed a history entry");
    p.click("Show Heal 1");
    assert_eq!(p.s().spot_shown, vec![true], "nor showed it again");
}

#[test]
fn holding_a_patch_out_re_renders_and_edits_nothing() {
    let mut p = Panel::with(vec![a_spot()], Vec::new(), Vec::new());
    let before = p.s().edits.clone();
    let ed = p.click("Hold out Heal 1");
    assert_eq!(p.s().spot_held, vec![true]);
    assert!(
        ed.changed,
        "holding a patch out changes the picture and asked for no render"
    );
    assert!(!ed.settled, "a view toggle pushed a history entry");
    assert_eq!(p.s().edits, before, "a view toggle edited the photograph");
    assert!(
        p.s()
            .view_edits(rustroom_app::tool::Tool::Retouch)
            .spots
            .is_empty(),
        "the held patch is still in the view"
    );
}

#[test]
fn hold_out_shows_what_a_removal_covers() {
    let mut p = Panel::with(Vec::new(), vec![a_removal()], Vec::new());
    let ed = p.click("Hold out Removal 1");
    assert!(p.s().removal.is_held(0));
    assert!(ed.changed && !ed.settled);
    assert!(p
        .s()
        .view_edits(rustroom_app::tool::Tool::Retouch)
        .removals
        .is_empty());
}

/// Show all / Hide all reaches every kind that has an outline — patches and removals.
#[test]
fn show_all_flips_every_kind() {
    let mut p = Panel::with(vec![a_spot(), a_spot()], vec![a_removal()], Vec::new());
    p.click("Show Heal 1");
    p.click("Show all");
    assert_eq!(p.s().spot_shown, vec![true, true]);
    assert!(p.s().removal.is_shown(0));
    p.click("Hide all");
    assert_eq!(p.s().spot_shown, vec![false, false]);
    assert!(
        !p.s().removal.is_shown(0),
        "Hide all left the removal's outline"
    );
}

#[test]
fn deleting_a_patch_from_its_row_drops_its_flags_with_it() {
    let mut p = Panel::with(vec![a_spot(), a_spot()], Vec::new(), Vec::new());
    p.click("Show Heal 1");
    p.click("Hold out Heal 2");
    p.s_mut().selected_spot = Some(1);
    let ed = p.click("Delete Heal 1");
    let s = p.s();
    assert_eq!(s.edits.spots.len(), 1);
    assert_eq!(s.spot_shown, vec![true], "the survivor keeps its own eye");
    assert_eq!(s.spot_held, vec![true], "and its own hold-out");
    assert_eq!(
        s.selected_spot,
        Some(0),
        "the selection followed the survivor"
    );
    assert!(ed.settled, "a delete is one history entry");
}

#[test]
fn a_selected_removal_shows_its_opacity_and_tries_another_fill() {
    let mut p = Panel::with(Vec::new(), vec![a_removal(), a_removal()], Vec::new());
    assert!(
        !p.labels().iter().any(|l| l == "Opacity"),
        "no Opacity before a selection"
    );
    p.click("Removal 2");
    assert!(p.labels().iter().any(|l| l == "Opacity"));
    let ed = p.click("Try another fill");
    assert_eq!(p.s().edits.removals[1].variant, 1);
    assert_eq!(
        p.s().edits.removals[0].variant,
        0,
        "another removal changed"
    );
    assert!(ed.settled);
    let ed = p.click("Delete Removal 2");
    assert_eq!(p.s().edits.removals.len(), 1);
    assert_eq!(p.s().removal.selected, None);
    assert!(ed.settled);
}

#[test]
fn a_person_s_and_a_dust_patch_s_rows_say_what_they_are() {
    let person = edit_model::Removal {
        points: Vec::new(),
        person: Some([0.1, 0.1, 0.2, 0.5]),
        ..a_removal()
    };
    let dust = edit_model::Spot {
        dust: true,
        ..a_spot()
    };
    let mut p = Panel::with(vec![a_spot(), dust], vec![a_removal(), person], Vec::new());
    let labels = p.labels();
    for name in ["Removal 1", "Person 1", "Heal 1", "Dust 1"] {
        assert!(labels.iter().any(|l| l == name), "{name}: {labels:?}");
    }
}

/// ⚠ An empty list is still drawn — its header is where Reset and the count live — with no rows.
#[test]
fn the_list_is_drawn_when_empty() {
    let mut p = Panel::new();
    let labels = p.labels();
    assert!(labels.iter().any(|l| l == "Retouches"), "{labels:?}");
    assert!(
        !labels.iter().any(|l| l.starts_with("Delete ")),
        "{labels:?}"
    );
}

/// ⚠⚠ The dust view must not survive leaving the tool — a canvas stuck in the grey edge view reads as broken.
#[test]
fn visualize_turns_itself_off_when_the_tool_changes() {
    let mut p = Panel::new();
    p.app.select_tool(rustroom_app::tool::Tool::Retouch);
    p.s_mut().visualize = Some(12.0);
    p.app.select_tool(rustroom_app::tool::Tool::Edit);
    assert_eq!(p.s().visualize, None, "the view mode outlived its tool");
}

/// ⚠ The default radius is generous on purpose: the radius must exceed the blemish, or heal measures a ring that
/// still lies on it.
#[test]
fn the_default_tool_is_usable() {
    let t = SpotTool::default();
    assert!(
        t.radius >= rustroom_app::spot_tool::MIN_RADIUS * 4.0,
        "got {}",
        t.radius
    );
    assert_eq!(t.mode, SpotMode::Heal, "Heal is the useful default");
}
