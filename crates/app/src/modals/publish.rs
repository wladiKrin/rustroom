//! A publish service's New / Edit dialog and its deletion's confirmation (M10.18d).

use super::{dialog, Footer, Modal};
use crate::publish_flow::SettingsFrom;
use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::immich_upload::{availability, Availability};

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Some(editing) = app.publish.dialog.as_ref().map(|d| d.editing) else {
        app.modal = Modal::None;
        return;
    };
    let immich_ready = availability(&app.settings) == Availability::Ready;
    let albums: Vec<(String, String)> = match &app.immich_albums {
        Some(Ok(a)) => a
            .iter()
            .map(|a| (a.id.clone(), a.album_name.clone()))
            .collect(),
        _ => Vec::new(),
    };
    let presets: Vec<String> = app
        .export_preset_store
        .list()
        .into_iter()
        .map(|p| p.name)
        .collect();
    let mut pick = false;
    let out = dialog(
        ctx,
        &mut app.settings,
        "publish_service",
        if editing.is_some() {
            "Edit publish service"
        } else {
            "New publish service"
        },
        theme::DialogSize::Standard,
        Footer::confirm(
            if editing.is_some() { "Save" } else { "Create" },
            "Saves the service; nothing is published until you press Publish",
            true,
            "Cancel",
        ),
        |ui| {
            let d = app.publish.dialog.as_mut().expect("checked above");
            egui::Grid::new("publish_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut d.name)
                            .hint_text("Phone")
                            .desired_width(f32::INFINITY),
                    )
                    .hover("Names the service in the rail");
                    ui.end_row();
                    ui.label("Publish to");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.to_immich, false, "Folder")
                            .hover("Exports into a folder on this computer");
                        ui.add_enabled_ui(immich_ready, |ui| {
                            ui.radio_value(&mut d.to_immich, true, "Immich album")
                                .hover(if immich_ready {
                                    "Uploads to an Immich album, never deleting an asset there"
                                } else {
                                    "Needs Immich's Write key in Settings → Immich"
                                });
                        });
                    });
                    ui.end_row();
                    if d.to_immich {
                        ui.label("Album");
                        let shown = d
                            .album
                            .as_ref()
                            .map_or_else(|| "New album".to_string(), |(_, n)| n.clone());
                        egui::ComboBox::from_id_salt("publish_album")
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut d.album, None, "New album");
                                for a in &albums {
                                    ui.selectable_value(&mut d.album, Some(a.clone()), &a.1);
                                }
                            })
                            .response
                            .hover("Chooses the album the photos go into");
                        ui.end_row();
                        if d.album.is_none() {
                            ui.label("New album");
                            ui.add(
                                egui::TextEdit::singleline(&mut d.new_album)
                                    .hint_text("Album name")
                                    .desired_width(f32::INFINITY),
                            )
                            .hover("Names the album made at the first Publish");
                            ui.end_row();
                        }
                    } else {
                        ui.label("Folder");
                        ui.horizontal(|ui| {
                            if ui
                                .button("Choose…")
                                .hover("Chooses the folder the photos are exported into")
                                .clicked()
                            {
                                pick = true;
                            }
                            let f = d
                                .folder
                                .as_ref()
                                .map_or_else(|| "None chosen".to_string(), |f| f.display().to_string());
                            theme::hint(ui, &f);
                        });
                        ui.end_row();
                    }
                    ui.label("Export settings");
                    let shown = match &d.settings {
                        SettingsFrom::Keep => "Unchanged".to_string(),
                        SettingsFrom::LastExport => "Last export settings".to_string(),
                        SettingsFrom::Preset(p) => p.clone(),
                    };
                    egui::ComboBox::from_id_salt("publish_settings")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            if d.editing.is_some() {
                                ui.selectable_value(&mut d.settings, SettingsFrom::Keep, "Unchanged");
                            }
                            ui.selectable_value(
                                &mut d.settings,
                                SettingsFrom::LastExport,
                                "Last export settings",
                            );
                            for p in &presets {
                                ui.selectable_value(
                                    &mut d.settings,
                                    SettingsFrom::Preset(p.clone()),
                                    p,
                                );
                            }
                        })
                        .response
                        .hover("Copies these settings into the service, so changing them marks every photo modified");
                    ui.end_row();
                });
            if let Some(e) = &d.error {
                theme::error_hint(ui, e);
            }
        },
    );
    if pick {
        if let Some(f) = rfd::FileDialog::new()
            .set_title("Choose the folder this service publishes into")
            .pick_folder()
        {
            if let Some(d) = app.publish.dialog.as_mut() {
                d.folder = Some(f);
            }
        }
    }
    if out.primary {
        if app.save_service_dialog() {
            app.modal = Modal::None;
        }
    } else if out.dismissed {
        app.publish.dialog = None;
        app.modal = Modal::None;
    }
}

pub fn confirm_delete(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::ConfirmDeleteService { id, name } = &app.modal else {
        return;
    };
    let (id, name) = (*id, name.clone());
    let out = dialog(
        ctx,
        &mut app.settings,
        "confirm_delete_service",
        "Delete publish service?",
        theme::DialogSize::Compact,
        Footer::confirm(
            "Delete",
            "Deletes the service; the photos it published stay where they are",
            true,
            "Cancel",
        ),
        |ui| {
            ui.label(format!("Delete the service “{name}”?"));
            theme::hint(ui, "The exported photos stay where they are.");
        },
    );
    if out.primary {
        app.delete_service(id);
        app.modal = Modal::None;
    } else if out.dismissed {
        app.modal = Modal::None;
    }
}
