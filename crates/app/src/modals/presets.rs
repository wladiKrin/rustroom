//! Develop-preset dialogs: saving one, and confirming a deletion.

use super::{dialog, Footer};
use crate::develop::panel::section::{header, Header};
use crate::transfer::edit_group_checkboxes;
use crate::{theme, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::presets::Preset;
use rustroom_app::settings::ConfirmOutcome;

/// The "Save current as preset…" dialog.
pub fn save(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::SavePreset { name, groups } = &mut app.modal else {
        return;
    };
    let mut name = std::mem::take(name);
    let mut groups = *groups;
    let mut close = false;
    let mut save = false;

    let note = "Saved presets appear in the Presets panel.";
    let out = dialog(
        ctx,
        &mut app.settings,
        "save_preset",
        "Save preset",
        theme::DialogSize::Standard,
        Footer::confirm("Save", !name.trim().is_empty(), "Cancel").note(note),
        |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut name);
            });
            let summary = rustroom_app::presets::groups_summary(groups);
            let mut h = header(
                ui,
                egui::Id::new(("save_preset", "Include")),
                true,
                Header::new("Include").trailing(&summary),
            );
            h.state
                .show_body_unindented(ui, |ui| edit_group_checkboxes(ui, &mut groups));
        },
    );
    save |= out.primary;
    close |= out.dismissed;

    if save {
        // Clone the edits *out* first: holding `&app.session` across the assignments
        // to `app.status` below would be an immutable borrow crossing a mutable one.
        if let Some(edits) = app.session.as_ref().map(|s| s.edits.clone()) {
            let preset = Preset {
                name: name.trim().to_string(),
                groups,
                edits,
            };
            match app.preset_store.save(&preset) {
                Ok(()) => app.status = format!("Saved preset “{}”", preset.name),
                Err(e) => app.status = format!("Could not save preset: {e}"),
            }
            app.reload_presets();
        }
        close = true;
    }

    if close {
        app.modal = Modal::None;
    } else {
        app.modal = Modal::SavePreset { name, groups };
    }
}

/// The preset-delete confirmation.
///
/// ⚠ "Don't ask again" is scratch state until Delete is clicked. Ticking it and then
/// cancelling must leave the confirmation armed — see
/// `settings::cancelling_never_suppresses_the_confirmation`, which is where that rule
/// is actually tested.
pub fn confirm_delete(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::ConfirmDeletePreset {
        name,
        dont_ask_again,
    } = &mut app.modal
    else {
        return;
    };
    let name = std::mem::take(name);
    let mut dont_ask = *dont_ask_again;
    let mut outcome: Option<ConfirmOutcome> = None;

    let out = dialog(
        ctx,
        &mut app.settings,
        "confirm_delete",
        "Delete preset?",
        theme::DialogSize::Compact,
        Footer::confirm("Delete", true, "Cancel"),
        |ui| {
            ui.label(format!("Delete the preset “{name}”?"));
            ui.checkbox(&mut dont_ask, "Don't ask again");
        },
    );
    if out.primary {
        outcome = Some(ConfirmOutcome::Confirmed {
            suppress_future: dont_ask,
        });
    } else if out.dismissed {
        outcome = Some(ConfirmOutcome::Cancelled);
    }

    match outcome {
        None => {
            // Still open — carry the scratch state to the next frame.
            app.modal = Modal::ConfirmDeletePreset {
                name,
                dont_ask_again: dont_ask,
            };
        }
        Some(o) => {
            let confirm_before = app.settings.confirm_preset_delete;
            if app.settings.apply_delete_confirm(o) {
                app.delete_preset(&name);
            }
            if app.settings.confirm_preset_delete != confirm_before {
                app.save_settings();
            }
            app.modal = Modal::None;
        }
    }
}
