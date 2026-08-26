//! The Presets tool: save the current edits as a preset, apply one, or delete one.
//!
//! ⚠ Was a dedicated **left panel** plus a toolbar toggle — the only feature in the app with
//! two homes. It is a rail tool now, with exactly one.
//!
//! ⚠ Drawn *outside* the panel's mutable `Session` borrow, because it is the one tool that
//! edits neither `Session` nor `Edits`: applying a preset targets the whole *selection*
//! through `app.apply_to_selection`. That is why it needs `&mut RustroomApp` and cannot sit
//! inside the tool `match` with the others.

use crate::{Modal, RustroomApp};
use edit_model::EditGroups;
use eframe::egui;
use rustroom_app::action_log::ActionKind;

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ⚠ egui borrows `app` through the closure, so a click is recorded here and acted on
    // after the panel closes. Keep this indirection — it is not incidental.
    enum Action {
        None,
        Apply(usize),
        Delete(String),
        Save,
        UndoDelete,
    }
    let mut action = Action::None;
    // Shown whenever the next undo would restore a preset, so the restore is
    // discoverable rather than only a keystroke. It is the same action the toolbar's
    // Undo performs — not a second, competing one.
    let open = app.open_photo();
    let undo_name = match app.actions.peek_undo(open.as_ref()) {
        Some(ActionKind::PresetDelete { name }) => Some(name.clone()),
        _ => None,
    };

    if ui.button("Save current as preset…").clicked() {
        action = Action::Save;
    }
    if let Some(name) = &undo_name {
        if ui.button(format!("↩ Undo delete “{name}”")).clicked() {
            action = Action::UndoDelete;
        }
    }
    ui.separator();
    if app.preset_list.is_empty() {
        ui.label("No presets yet.");
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, p) in app.preset_list.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.button("×").on_hover_text("Delete").clicked() {
                    action = Action::Delete(p.name.clone());
                }
                if ui.button(&p.name).clicked() {
                    action = Action::Apply(i);
                }
            });
        }
    });

    match action {
        Action::None => {}
        Action::Save => {
            app.modal = Modal::SavePreset {
                name: String::new(),
                groups: EditGroups::default(),
            };
        }
        Action::Apply(i) => {
            if let Some(p) = app.preset_list.get(i).cloned() {
                // Applying a preset targets the selection, exactly as a paste does.
                let (n, failed) = app.apply_to_selection(&p.edits, p.groups);
                app.status = match (n, failed) {
                    (0, _) => "Nothing selected".to_string(),
                    (1, 0) => format!("Applied preset “{}”", p.name),
                    (n, 0) => format!("Applied “{}” to {n} photos", p.name),
                    (n, f) => format!("Applied “{}” to {n} photos, {f} failed", p.name),
                };
            }
        }
        Action::UndoDelete => app.undo_action(ctx),
        Action::Delete(name) => {
            if app.settings.confirm_preset_delete {
                app.modal = Modal::ConfirmDeletePreset {
                    name,
                    dont_ask_again: false,
                };
            } else {
                app.delete_preset(&name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn app_with(presets: &[&str]) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.preset_list = presets
            .iter()
            .map(|n| rustroom_app::presets::Preset {
                name: n.to_string(),
                groups: EditGroups::default(),
                edits: edit_model::Edits::default(),
            })
            .collect();
        app
    }

    fn click_in_panel(label: &str, app: &mut RustroomApp) {
        let ctx = test_ui::ctx();
        let c = ctx.clone();
        test_ui::click(&ctx, label, |ui| show(app, ui, &c));
    }

    #[test]
    fn saving_opens_the_naming_dialog_rather_than_writing_straight_away() {
        // ⚠ A preset needs a name and a group choice before it can be written, so the button
        // opens a modal. One that saved immediately would produce unnamed presets carrying
        // every group — the two decisions the dialog exists to ask.
        let mut app = app_with(&[]);
        assert!(matches!(app.modal, Modal::None));
        click_in_panel("Save current as preset…", &mut app);
        assert!(
            matches!(app.modal, Modal::SavePreset { .. }),
            "expected the save dialog, got {:?}",
            std::mem::discriminant(&app.modal)
        );
    }

    #[test]
    fn deleting_asks_first_while_the_guard_is_armed() {
        // ⚠⚠ Deleting a preset is the *recoverable* one — it goes to a trash and Ctrl+Z restores
        // it — and it still confirms while the setting says to. The guard and the deletion are
        // separate flags on purpose: disarming the confirm on this one must not disarm it on the
        // irrecoverable export-preset deletion.
        let mut app = app_with(&["Warm"]);
        app.settings.confirm_preset_delete = true;
        click_in_panel("×", &mut app);
        assert!(
            matches!(app.modal, Modal::ConfirmDeletePreset { .. }),
            "expected the confirm dialog"
        );
        assert_eq!(app.preset_list.len(), 1, "and nothing deleted yet");
    }

    #[test]
    fn deleting_with_the_guard_disarmed_does_not_ask() {
        let mut app = app_with(&["Warm"]);
        app.settings.confirm_preset_delete = false;
        click_in_panel("×", &mut app);
        assert!(
            matches!(app.modal, Modal::None),
            "no dialog when the guard is off"
        );
    }

    #[test]
    fn applying_a_preset_with_nothing_selected_says_so() {
        // ⚠ A preset applies to the *selection*, exactly as a paste does. With nothing selected
        // the honest outcome is a status line, not a silent no-op.
        let mut app = app_with(&["Warm"]);
        click_in_panel("Warm", &mut app);
        assert_eq!(app.status, "Nothing selected");
    }

    #[test]
    fn an_empty_list_offers_nothing_to_delete_or_apply() {
        // ⚠ Asserted on the *absence* of the row buttons, not on the "No presets yet." label:
        // plain `ui.label` text produces no AccessKit node, so `test_ui` cannot see it. Only
        // interactive widgets are findable — the module's stated limit, and the point at which
        // `egui_kittest` would be the thing to adopt.
        let ctx = test_ui::ctx();
        let mut app = app_with(&[]);
        let c = ctx.clone();
        let labels: Vec<String> =
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| show(&mut app, ui, &c))
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        assert!(
            !labels.iter().any(|l| l == "×"),
            "an empty list must offer nothing to delete, got {labels:?}"
        );
        assert!(
            labels.iter().any(|l| l == "Save current as preset…"),
            "but saving must still be offered, got {labels:?}"
        );
    }

    #[test]
    fn the_undo_delete_button_appears_only_when_that_is_what_undo_would_do() {
        // ⚠ It is the *same* action the toolbar's Undo performs, surfaced so the restore is
        // discoverable rather than only a keystroke — not a second, competing one. Showing it
        // when undo would do something else would make it lie about what it does.
        let ctx = test_ui::ctx();
        let shown = |app: &mut RustroomApp| {
            let c = ctx.clone();
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| show(app, ui, &c))
                .into_iter()
                .any(|(l, _)| l.starts_with("↩ Undo delete"))
        };

        let mut app = app_with(&["Warm"]);
        assert!(!shown(&mut app), "nothing deleted yet");

        app.actions.push(
            ActionKind::PresetDelete {
                name: "Warm".to_string(),
            },
            None,
        );
        assert!(
            shown(&mut app),
            "a preset deletion should surface the restore"
        );
    }
}
