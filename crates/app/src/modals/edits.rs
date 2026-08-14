//! Dialogs that move edits between photos: paste, and sync.

use super::dialog;
use crate::{edit_group_checkboxes, paste_status, theme, Modal, RustroomApp};
use eframe::egui;

/// The "Paste…" dialog: which groups this paste applies.
///
/// The chosen selection is remembered in `paste_groups`, which is also what a bare
/// Ctrl+V uses.
pub fn paste(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut groups = app.paste_groups;
    let mut close = false;
    let mut paste = false;

    let dismissed = dialog(ctx, "paste_settings", "Paste settings", |ui| {
        ui.label("Apply:");
        edit_group_checkboxes(ui, &mut groups);
        ui.separator();
        let (confirmed, dismissed) = theme::button_row(ui, "Paste", true, "Cancel");
        paste |= confirmed;
        close |= dismissed;
    });
    close |= dismissed;

    app.paste_groups = groups;
    if paste {
        if let Some(src) = app.clipboard.clone() {
            let (n, failed) = app.apply_to_selection(&src, groups);
            app.status = paste_status(n, failed);
        }
        close = true;
    }
    if close {
        app.modal = Modal::None;
    }
}

/// The "Sync…" dialog: which groups this sync applies to the rest of the selection.
pub fn sync(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut groups = app.sync_groups;
    let count = app.sync_targets().len();
    let mut close = false;
    let mut run = false;

    let dismissed = dialog(ctx, "sync_settings", "Sync settings", |ui| {
        ui.label(format!("Apply to {count} other selected photos:"));
        edit_group_checkboxes(ui, &mut groups);
        ui.separator();
        let (confirmed, dismissed) = theme::button_row(ui, "Sync", true, "Cancel");
        run |= confirmed;
        close |= dismissed;
    });
    close |= dismissed;

    app.sync_groups = groups;
    if run {
        app.run_sync(groups);
        close = true;
    }
    if close {
        app.modal = Modal::None;
    }
}
