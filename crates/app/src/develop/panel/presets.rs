//! The Presets drawer: save the current edits as a preset, apply one, or delete one.
//!
//! ⚠ Was a dedicated **left panel** plus a toolbar toggle — the only feature in the app with
//! two homes. It is a rail tool now, with exactly one.
//!
//! ⚠ Drawn *outside* the panel's mutable `Session` borrow, because it is the one drawer that
//! edits neither `Session` nor `Edits`: applying a preset targets the whole *selection*
//! through `app.apply_to_selection`. That is why it needs `&mut RustroomApp` and cannot sit
//! inside the drawer `match` with the others.

use crate::{Modal, RustroomApp};
use edit_model::EditGroups;
use eframe::egui;
use rustroom_app::action_log::ActionKind;

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui) {
    // ⚠ egui borrows `app` through the closure, so a click is recorded here and acted on
    // after the drawer closes. Keep this indirection — it is not incidental.
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
    let undo_name = match app.actions.peek_undo(open.as_deref()) {
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
        Action::UndoDelete => app.undo_action(),
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
