//! The Retouch panel's Remove half (M10.16c1): the Removals list, the brush's Size and Detect objects, and
//! the selected removal's Opacity and Refresh.
//!
//! ⚠ Its own file, `retouch_eyes`'s reason: `retouch.rs` sits at the line ceiling.

use super::Edited;
use crate::develop::panel::{mask_components, section};
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::remove_tool::RemoveTool;
use rustroom_app::slider_help::{self, Scope};
use rustroom_app::spot_tool::{self, MAX_RADIUS, MIN_RADIUS};

pub(super) const REMOVE_HINT: &str =
    "Paints over something to remove, filled from its surroundings";

/// Whether removal `i` has its fill — until then the object still shows under its stroke.
fn filled(s: &Session, i: usize) -> bool {
    let Some(r) = s.edits.removals.get(i) else {
        return false;
    };
    let maps = s.engine.ai_maps();
    let got = maps.read().is_ok_and(|m| m.fill(r).is_some());
    got
}

/// The Removals list: one row per removal, a click selects it, `×` deletes it.
pub(super) fn list(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited) {
    let n = s.edits.removals.len();
    let count = match n {
        0 => "none".to_owned(),
        1 => "1 removal".to_owned(),
        n => format!("{n} removals"),
    };
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("retouch", "Removals")),
        true,
        section::Header::new("Removals")
            .modified(n > 0)
            .resettable(true)
            .trailing(&count),
    );
    if out.reset {
        s.edits.removals.clear();
        s.removal.selected = None;
        ed.discrete();
    }
    let (mut pick, mut delete) = (None, None);
    let selected = s.removal.selected;
    let shown: Vec<bool> = (0..s.edits.removals.len()).map(|i| filled(s, i)).collect();
    out.show_body(ui, |ui| {
        for (i, done) in shown.iter().enumerate() {
            let name = format!("Removal {}", i + 1);
            let meta = if *done { "" } else { "filling" };
            let row = mask_components::row(
                ui,
                mask_components::RowSpec {
                    glyph: mask_components::RowGlyph::Spot(edit_model::SpotMode::Heal),
                    op: None,
                    name: &name,
                    meta,
                    accessible: &name,
                    selected: selected == Some(i),
                    current: selected == Some(i),
                    deletable: true,
                    delete_hover: "Deletes this removal",
                    indent: 0.0,
                    eye: None,
                    hold: None,
                    hover: "Selects this removal, to change its Opacity or fill it again",
                    draggable: false,
                },
            );
            if row.delete {
                delete = Some(i);
            } else if row.clicked {
                pick = Some(i);
            }
        }
    });
    if let Some(i) = delete {
        s.edits.removals.remove(i);
        s.removal.selected = match s.removal.selected {
            Some(j) if j == i => None,
            Some(j) if j > i => Some(j - 1),
            other => other,
        };
        ed.discrete();
    } else if let Some(i) = pick {
        s.removal.selected = Some(i);
        s.selected_spot = None;
        s.selected_eye = None;
    }
}

/// The brush's Size and Detect objects, then the selected removal's Opacity and Refresh.
pub(super) fn settings(ui: &mut egui::Ui, s: &mut Session, tool: &mut RemoveTool, ed: &mut Edited) {
    let long = s.full_long;
    let mut px = spot_tool::radius_px(tool.radius, long);
    let r = ui.add(
        super::slider::Slider::new(
            &mut px,
            spot_tool::radius_px(MIN_RADIUS, long)..=spot_tool::radius_px(MAX_RADIUS, long),
        )
        .logarithmic(true)
        .text("Size")
        .help(slider_help::text(Scope::Remove, "Size"))
        .reset_to(spot_tool::radius_px(RemoveTool::default().radius, long))
        .custom_formatter(|v| format!("{v:.0} px")),
    );
    if r.changed() {
        tool.radius = spot_tool::radius_from_px(px, long);
    }
    ui.checkbox(&mut tool.detect, "Detect objects")
        .hover("Grows each new stroke to the whole object under it");
    let Some(i) = s.removal.selected.filter(|&i| i < s.edits.removals.len()) else {
        return;
    };
    ui.add_space(6.0);
    let mut opacity = s.edits.removals[i].opacity;
    let r = ui.add(
        super::slider::Slider::new(&mut opacity, 0.0..=100.0)
            .text("Opacity")
            .help(slider_help::text(Scope::Remove, "Opacity"))
            .reset_to(100.0),
    );
    if r.changed() {
        s.edits.removals[i].opacity = opacity;
    }
    ed.absorb(&r);
    if ui
        .button("Refresh")
        .hover("Fills this removal again from another part of its surroundings")
        .clicked()
    {
        s.edits.removals[i].variant += 1;
        ed.discrete();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::panel::retouch;
    use rustroom_app::eye_tool::EyeTool;

    fn removal(x: f32) -> edit_model::Removal {
        edit_model::Removal {
            points: vec![[x, 0.5]],
            radius: 0.02,
            detect: false,
            variant: 0,
            opacity: 100.0,
        }
    }

    /// An app with a photo open, `removals` on it, and the panel drawn through the real entry point with
    /// the app's own tools.
    struct Panel {
        ctx: egui::Context,
        app: crate::RustroomApp,
    }

    impl Panel {
        fn new(removals: Vec<edit_model::Removal>) -> Self {
            let ctx = crate::test_ui::ctx();
            let mut app = crate::test_app::for_test();
            crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
            app.session.as_mut().unwrap().edits.removals = removals;
            Self { ctx, app }
        }

        fn draw(&mut self) -> impl FnMut(&mut egui::Ui) + '_ {
            |ui| {
                let (mut t, mut e, mut r) =
                    (self.app.spot_tool, self.app.eye_tool, self.app.remove_tool);
                let mut ed = Edited::default();
                retouch::show_with(
                    ui,
                    self.app.session.as_mut().unwrap(),
                    &mut t,
                    &mut e,
                    &mut r,
                    &mut ed,
                );
                self.app.remove_tool = r;
            }
        }

        fn click(&mut self, label: &str) -> Edited {
            let ctx = self.ctx.clone();
            let mut seen = Edited::default();
            let (mut t, mut e, mut r) =
                (self.app.spot_tool, self.app.eye_tool, self.app.remove_tool);
            crate::test_ui::click(&ctx, label, |ui| {
                let mut ed = Edited::default();
                retouch::show_with(
                    ui,
                    self.app.session.as_mut().unwrap(),
                    &mut t,
                    &mut e,
                    &mut r,
                    &mut ed,
                );
                if ed.changed || ed.settled {
                    seen = ed;
                }
            });
            self.app.remove_tool = r;
            seen
        }

        fn s(&self) -> &Session {
            self.app.session.as_ref().unwrap()
        }
    }

    #[test]
    fn remove_is_the_first_and_default_mode() {
        use super::super::retouch_eyes::Mode;
        assert_eq!(
            Mode::current(
                edit_model::SpotMode::Heal,
                None,
                RemoveTool::default().active
            ),
            Mode::Remove
        );
        let mut p = Panel::new(Vec::new());
        assert!(p.app.remove_tool.active, "the app starts in Remove");
        let ctx = p.ctx.clone();
        let at = crate::test_ui::labelled_centres(&ctx, Vec::new(), p.draw());
        let x = |l: &str| at.iter().find(|(n, _)| n == l).map(|(_, c)| c.x);
        let remove = x("Remove").expect("a Remove mode");
        for other in ["Heal", "Clone", "Red eye", "Pet eye"] {
            assert!(remove < x(other).expect(other), "Remove is left of {other}");
        }
        assert!(x("Detect objects").is_some(), "Remove's own settings show");
        assert!(x("Feather").is_none(), "not a patch's");
    }

    /// Heal from Remove returns to patches; Remove from Heal takes the patch selection away.
    #[test]
    fn the_mode_switches_between_remove_and_heal() {
        let mut p = Panel::new(vec![removal(0.3)]);
        p.app.session.as_mut().unwrap().removal.selected = Some(0);
        p.click("Heal");
        assert!(!p.app.remove_tool.active);
        assert_eq!(
            p.s().removal.selected,
            None,
            "the removal selection went with the mode"
        );
        p.click("Remove");
        assert!(p.app.remove_tool.active);
        assert_eq!(p.app.eye_tool, EyeTool::default());
    }

    /// Refresh bumps the selected removal's variant, as one step; its row's × deletes it.
    #[test]
    fn refresh_and_delete_act_on_the_selected_removal() {
        let mut p = Panel::new(vec![removal(0.3), removal(0.7)]);
        p.app.session.as_mut().unwrap().removal.selected = Some(1);
        let ed = p.click("Refresh");
        let v: Vec<u32> = p.s().edits.removals.iter().map(|r| r.variant).collect();
        assert_eq!(v, vec![0, 1], "the selected one refilled");
        assert!(ed.changed && ed.settled, "one history step");
        let ed = p.click("Delete Removal 2");
        assert_eq!(p.s().edits.removals, vec![removal(0.3)]);
        assert_eq!(p.s().removal.selected, None);
        assert!(ed.settled, "a delete is one history step");
    }

    /// A row selects its removal and lights Opacity for it; with none selected there is no Opacity.
    #[test]
    fn a_row_selects_its_removal_and_shows_its_opacity() {
        let mut p = Panel::new(vec![removal(0.3), removal(0.7)]);
        let ctx = p.ctx.clone();
        assert!(crate::test_ui::find(&ctx, "Opacity", p.draw()).is_none());
        p.click("Removal 1");
        assert_eq!(p.s().removal.selected, Some(0));
        assert!(crate::test_ui::find(&ctx, "Opacity", p.draw()).is_some());
    }
}
