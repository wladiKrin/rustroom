//! The Develop snapshot strip: a photo's named saved states, pinned below the adjustments.
//!
//! ⚠⚠ **A deliberate departure from the exclusive-panel rule.** Every other control in this panel
//! belongs to exactly one rail tool; this one is drawn under all of them. A snapshot list is
//! *navigation*, not a tool — the whole point is that the other states of the photo you are
//! working on are always in reach. Behind a fifth rail tool it would be hidden precisely when it
//! is wanted.
//!
//! ⚠ `ui.set_min_height(ui.available_height())` is load-bearing: a resizable egui panel stores its
//! *content's* rendered rect as its size each frame, so content smaller than the panel makes it
//! shrink back every frame. The filmstrip already carries this note.
//!
//! ⚠⚠ **The scroll area here is a SIBLING of the panel's own, not nested inside it.** The strip is
//! allocated before the right panel's `ScrollArea`, so the two occupy disjoint rects and the wheel
//! is never ambiguous. This looks exactly like the nesting the Library panel's three inner scroll
//! areas were removed for on 2026-08-27 — and it is not that. Do not "fix" it.

use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::settings::STRIP_HEIGHT_RANGE;

/// What a click in the strip asked for, applied after the panel closes.
///
/// ⚠ Collected rather than acted on immediately: egui borrows `app` through the closure, so a
/// click cannot call a `&mut self` method from inside it. The shape every panel here uses.
enum Action {
    None,
    Take,
    Restore(u32),
    Delete(u32),
    Rename(u32, Option<String>),
    MakeCopy(u32),
}

/// The strip's id, and so — via `.with("__resize")` — its resize handle's.
const PANEL_ID: &str = "snapshot_strip";

/// Draw the strip. Returns whether the photo's edits changed, which the caller records as one
/// history entry — the contract `panel::show` already has.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    if app.session.is_none() {
        return false;
    }
    let mut action = Action::None;
    // ⚠ Capped at half the *live* panel height rather than only at the stored value, so a height
    // chosen in a large window is clamped rather than obeyed in a small one. egui clamps the
    // persisted size into this range itself — see `Panel::outer_size`.
    let max = (ui.available_height() * 0.5).max(STRIP_HEIGHT_RANGE.0 as f32);
    let stored = app.settings.clamped_strip_height();

    let panel = egui::Panel::bottom(PANEL_ID)
        .resizable(true)
        .default_size(stored)
        .min_size(STRIP_HEIGHT_RANGE.0 as f32)
        .max_size(max)
        .show(ui, |ui| {
            // Fill the panel's height so egui stores the panel size, not the smaller content
            // size — otherwise a resizable panel shrinks back each frame.
            ui.set_min_height(ui.available_height());

            let snaps: Vec<(u32, String, i64)> = app
                .session
                .as_ref()
                .map(|s| {
                    s.snapshots
                        .iter()
                        .map(|x| (x.n, x.label(), x.taken))
                        .collect()
                })
                .unwrap_or_default();

            ui.horizontal(|ui| {
                ui.label(if snaps.is_empty() {
                    "Snapshots".to_string()
                } else {
                    format!("Snapshots ({})", snaps.len())
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("+")
                        .on_hover_text("Save the current settings as a snapshot")
                        .clicked()
                    {
                        action = Action::Take;
                    }
                });
            });

            if snaps.is_empty() {
                theme::hint(ui, "Press + to save the current settings.");
                return;
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (n, label, taken) in snaps {
                        // ⚠ egui's auto-ids shift with what was drawn before them, and this list
                        // changes as snapshots are taken and deleted — the hazard that broke the
                        // rename field under virtualisation.
                        ui.push_id(n, |ui| row(app, ui, n, &label, taken, &mut action));
                    }
                });
        });

    // ⚠ Persisted on the **release** frame only, never per frame: a resize runs at 60 Hz — the
    // `apply_view_demosaic` rule. egui deliberately recomputes the panel size on `drag_stopped`
    // so the released size is the one stored, and `InnerResponse::response.rect` is the panel's
    // *outer* rect — the same quantity `default_size` takes, so feeding it back cannot drift.
    let resize_id = egui::Id::new(PANEL_ID).with("__resize");
    if ui
        .read_response(resize_id)
        .is_some_and(|r| r.drag_stopped())
    {
        let h = panel.response.rect.height().round() as u32;
        if h != app.settings.snapshot_strip_height {
            app.settings.snapshot_strip_height = h;
            app.save_settings();
        }
    }

    apply(app, ctx, action)
}

/// One row: its label, its timestamp, a click to restore and a menu to manage it.
fn row(
    app: &mut RustroomApp,
    ui: &mut egui::Ui,
    n: u32,
    label: &str,
    taken: i64,
    action: &mut Action,
) {
    // While this row owns the draft, the label is replaced by an editable field.
    if app.snapshot_rename.as_ref().is_some_and(|(m, _)| *m == n) {
        let mut text = app
            .snapshot_rename
            .as_ref()
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        let resp = ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
        if let Some((_, t)) = app.snapshot_rename.as_mut() {
            *t = text.clone();
        }
        // ⚠ Focus is taken once, when the field first appears — not every frame, which would
        // fight anything else the user clicked.
        if ui.memory(|m| m.focused()).is_none() {
            resp.request_focus();
        }
        if theme::committed(ui, &resp) {
            let typed = text.trim().to_string();
            // ⚠ Typing the default label back in stores `None`, so the snapshot keeps following
            // its number — the rule an unnamed virtual copy already follows. Freezing the derived
            // string would strand it on today's number.
            *action = Action::Rename(
                n,
                (!typed.is_empty() && typed != format!("Snapshot {n}")).then_some(typed),
            );
        } else if resp.lost_focus() {
            // ⚠ Clicking away abandons. Only Enter commits.
            app.snapshot_rename = None;
        }
        return;
    }

    let resp = ui
        .horizontal(|ui| {
            let r = ui.selectable_label(false, label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::caption(ui, &crate::modals::info::taken_text(Some(taken)));
            });
            r
        })
        .inner;

    if resp.clicked() {
        *action = Action::Restore(n);
    }
    resp.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            app.snapshot_rename = Some((n, label.to_string()));
            ui.close();
        }
        if ui.button("Make virtual copy").clicked() {
            *action = Action::MakeCopy(n);
            ui.close();
        }
        if ui.button("Delete").clicked() {
            *action = Action::Delete(n);
            ui.close();
        }
    });
}

/// Apply what the frame asked for. Returns whether the photo's edits changed.
fn apply(app: &mut RustroomApp, ctx: &egui::Context, action: Action) -> bool {
    match action {
        Action::None => false,
        Action::Take => {
            app.take_snapshot();
            false
        }
        Action::Restore(n) => app.restore_snapshot(n),
        Action::Delete(n) => {
            app.delete_snapshot(n);
            false
        }
        Action::Rename(n, name) => {
            app.rename_snapshot(n, name);
            app.snapshot_rename = None;
            false
        }
        Action::MakeCopy(n) => {
            app.copy_from_snapshot(ctx, n);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{test_app, test_ui};
    use rustroom_app::tool::Tool;

    /// ⚠⚠ **The requirement the strip exists to meet, and the only test that can see it.**
    /// "Always active" is otherwise a visual claim: a strip drawn under `Tool::Edit` alone looks
    /// perfectly correct in any screenshot of the Edit panel.
    #[test]
    fn the_strip_is_present_under_every_tool() {
        for tool in Tool::ALL {
            let ctx = test_ui::ctx();
            let mut app = test_app::for_test();
            test_app::open_test_photo(&mut app, &ctx, 8, 4);
            app.tool = tool;

            let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                crate::develop::panel::show(&mut app, ui);
            });
            assert!(
                labels.iter().any(|(l, _)| l == "+"),
                "the Take-snapshot button must be reachable under {tool:?}, got {:?}",
                labels.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
    }

    /// The strip draws nothing when no photo is open — the panel says "No image open." instead.
    #[test]
    fn the_strip_draws_nothing_without_a_session() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            crate::develop::panel::show(&mut app, ui);
        });
        assert!(!labels.iter().any(|(l, _)| l == "+"));
    }

    /// A row is labelled by `Snapshot::label`, so a named one shows its name rather than its
    /// number — the one rule every consumer of a snapshot's name goes through.
    #[test]
    fn a_named_snapshot_shows_its_name_in_the_strip() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        app.session
            .as_mut()
            .unwrap()
            .snapshots
            .push(rustroom_app::sidecar::Snapshot {
                n: 1,
                name: Some("Before the crop".to_string()),
                taken: 0,
                edits: Default::default(),
            });

        let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            crate::develop::panel::show(&mut app, ui);
        });
        assert!(
            labels.iter().any(|(l, _)| l == "Before the crop"),
            "got {:?}",
            labels.iter().map(|(l, _)| l).collect::<Vec<_>>()
        );
    }

    /// ⚠ The height is app-wide, not per photo — it lives in `Settings`, not on `Session`. Stated
    /// as a property so a future move onto `Session` breaks a test rather than a habit.
    #[test]
    fn the_strip_height_is_app_wide_not_per_photo() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.settings.snapshot_strip_height = 200;
        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        test_app::open_test_photo(&mut app, &ctx, 16, 8);
        assert_eq!(
            app.settings.snapshot_strip_height, 200,
            "opening another photo must not reset the strip"
        );
    }
}
