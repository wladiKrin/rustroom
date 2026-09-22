//! Paste settings and Sync settings: which edit groups to carry across.

use super::{dialog, Footer};
use crate::transfer::{edit_group_checkboxes, paste_status};
use crate::{theme, Modal, RustroomApp};
use eframe::egui;

pub fn paste(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut groups = app.paste_groups;

    let out = dialog(
        ctx,
        &mut app.settings,
        "paste_settings",
        "Paste settings",
        theme::DialogSize::Standard,
        Footer::confirm("Paste", true, "Cancel"),
        |ui| {
            apply_section(ui, "paste", "", &mut groups);
        },
    );

    app.paste_groups = groups;
    if out.primary {
        if let Some(src) = app.clipboard.clone() {
            let (n, failed) = app.apply_to_selection(&src, groups);
            app.status = paste_status(n, failed);
        }
    }
    if out.primary || out.dismissed {
        app.modal = Modal::None;
    }
}

pub fn sync(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut groups = app.sync_groups;
    let count = app.sync_targets().len();
    let trailing = format!(
        "to {count} other selected photo{}",
        if count == 1 { "" } else { "s" }
    );

    let out = dialog(
        ctx,
        &mut app.settings,
        "sync_settings",
        "Sync settings",
        theme::DialogSize::Standard,
        Footer::confirm("Sync", true, "Cancel"),
        |ui| {
            apply_section(ui, "sync", &trailing, &mut groups);
        },
    );

    app.sync_groups = groups;
    if out.primary {
        app.run_sync(groups);
    }
    if out.primary || out.dismissed {
        app.modal = Modal::None;
    }
}

/// The **Apply** section: the edit-group checkboxes under the panels' section header.
fn apply_section(ui: &mut egui::Ui, id: &str, trailing: &str, groups: &mut edit_model::EditGroups) {
    use crate::develop::panel::section::{header, Header};
    let mut h = Header::new("Apply");
    if !trailing.is_empty() {
        h = h.trailing(trailing);
    }
    let mut out = header(ui, egui::Id::new((id, "Apply")), true, h);
    out.state
        .show_body_unindented(ui, |ui| edit_group_checkboxes(ui, groups));
}
