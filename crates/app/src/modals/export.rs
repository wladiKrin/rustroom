//! The export flow'''s dialogs.
//!
//! Four in one file because they are one flow: `save_preset` and `confirm_delete` both
//! **return to** `Modal::ExportSettings` when closed, so the export dialog does not vanish
//! underneath the user.

use super::dialog;
use crate::{theme, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::batch_export::{
    banding_warning, destination_ready, Collision, Decision, ExportOptions,
};
use rustroom_app::export_presets::ExportPreset;
use rustroom_app::settings::ConfirmOutcome;

/// Where and how to export the selection.
pub fn settings(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut opts = app.export_draft.clone();
    let count = app.export_jobs().len();
    let mut close = false;
    let mut start = false;
    // egui has no folder field, so the picker writes into a local and the label shows it.
    let mut pick_dir = false;
    let mut picked: Option<(String, ExportOptions)> = None;
    let mut save_as = false;
    let mut delete_selected = false;
    // ⚠ Snapshots taken *before* the closure. `app.export_preset_list` cannot be read
    // inside it while `opts` also borrows `self` — the same borrow shape that has
    // already cost a compile error in this file twice.
    let presets = app.export_preset_list.clone();
    let selected = app.selected_export_preset.clone();

    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "export_settings",
        "Export",
        theme::DialogSize::Standard,
        |ui| {
            ui.label(format!("Export {count} photos as JPEG."));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("export_preset_pick")
                    .selected_text(selected.clone().unwrap_or_else(|| "Preset…".into()))
                    .show_ui(ui, |ui| {
                        for p in &presets {
                            let is_sel = selected.as_deref() == Some(p.name.as_str());
                            if ui.selectable_label(is_sel, &p.name).clicked() {
                                // Fills every field below, which is what keeps a preset's
                                // destination change visible rather than hidden.
                                picked = Some((p.name.clone(), p.options.clone()));
                            }
                        }
                    });
                if ui.button("Save as…").clicked() {
                    save_as = true;
                }
                let can_delete = selected.is_some();
                if ui
                    .add_enabled(can_delete, egui::Button::new("Delete"))
                    .clicked()
                {
                    delete_selected = true;
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Destination…").clicked() {
                    pick_dir = true;
                }
                let shown = if opts.dest_dir.as_os_str().is_empty() {
                    "(none chosen)".to_string()
                } else if opts.dest_dir.is_dir() {
                    opts.dest_dir.display().to_string()
                } else {
                    // A preset can carry a folder that has since gone — an unplugged drive,
                    // or a preset from another machine. Say so rather than failing per photo.
                    format!("{} (missing)", opts.dest_dir.display())
                };
                ui.label(shown);
            });
            ui.horizontal(|ui| {
                ui.label("Format");
                egui::ComboBox::from_id_salt("format_pick")
                    .selected_text(opts.format.label())
                    .show_ui(ui, |ui| {
                        for f in export::ExportFormat::ALL {
                            ui.selectable_value(&mut opts.format, f, f.label());
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Colour space");
                egui::ComboBox::from_id_salt("space_pick")
                    .selected_text(opts.space.label())
                    .show_ui(ui, |ui| {
                        for s in color::OutputSpace::ALL {
                            ui.selectable_value(&mut opts.space, s, s.label());
                        }
                    });
            });
            // ⚠ Warn, never block — see `banding_warning`. Placed under the two controls it is
            // about, so the fix ("use 16-bit") is one glance from the Format dropdown.
            if let Some(msg) = banding_warning(opts.space, opts.format) {
                theme::hint(ui, msg);
            }

            // Greyed out rather than hidden for the non-lossy formats: the value is still
            // there and returns when JPEG is reselected — the same treatment NR's Detail and
            // Sharpening's Masking get for being inert.
            ui.add_enabled(
                opts.format.is_lossy(),
                egui::Slider::new(&mut opts.quality, 1..=100).text("JPEG quality"),
            );

            let mut resize = opts.max_edge.is_some();
            ui.checkbox(&mut resize, "Fit within a long edge");
            if resize {
                let mut edge = opts.max_edge.unwrap_or(2048);
                ui.add(egui::Slider::new(&mut edge, 256..=8192).text("Long edge (px)"));
                opts.max_edge = Some(edge);
            } else {
                opts.max_edge = None;
            }

            ui.horizontal(|ui| {
                ui.label("Demosaic");
                egui::ComboBox::from_id_salt("demosaic_pick")
                    .selected_text(opts.demosaic.label())
                    .show_ui(ui, |ui| {
                        for d in raw_decode::Demosaic::ALL {
                            ui.selectable_value(&mut opts.demosaic, d, d.label());
                        }
                    });
            });
            // ⚠ Not decoration. This is the one export control whose effect cannot be
            // previewed, and the dialog is the only place a user would find that out.
            theme::hint(
                ui,
                "RAW only. Higher is slower — roughly 1.2 s to 11 s per photo. \
             Independent of the 1:1 view's own quality setting, so judging this \
             means exporting a frame at two settings and comparing crops.",
            );

            ui.separator();
            let ready = destination_ready(&opts.dest_dir);
            let (confirmed, dismissed) = theme::button_row(ui, "Export", ready, "Cancel");
            start |= confirmed;
            close |= dismissed;
        },
    );

    if pick_dir {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            opts.dest_dir = dir;
        }
    }
    if let Some((name, o)) = picked {
        opts = o;
        app.selected_export_preset = Some(name);
    }
    if save_as {
        app.modal = Modal::SaveExportPreset {
            name: String::new(),
        };
    }
    if delete_selected {
        // No trash — see `export_presets`' module doc. But deletion *is* permanent here,
        // which is precisely why it is guarded by its own confirmation.
        if let Some(name) = app.selected_export_preset.clone() {
            if app.settings.confirm_export_preset_delete {
                app.modal = Modal::ConfirmDeleteExportPreset {
                    name,
                    dont_ask_again: false,
                };
            } else {
                app.delete_selected_export_preset();
            }
        }
    }
    app.export_draft = opts;
    if start {
        app.start_export();
        close = true;
    }
    if close || dismissed {
        app.modal = Modal::None;
    }
}

/// Naming a new export preset.
pub fn save_preset(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::SaveExportPreset { name } = &app.modal else {
        return;
    };
    let mut name = name.clone();
    let mut save = false;
    let mut close = false;
    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "save_export_preset",
        "Save export preset",
        theme::DialogSize::Standard,
        |ui| {
            ui.text_edit_singleline(&mut name);
            let (confirmed, dismissed) =
                theme::button_row(ui, "Save", !name.trim().is_empty(), "Cancel");
            save |= confirmed;
            close |= dismissed;
        },
    );
    if save {
        let preset = ExportPreset {
            name: name.clone(),
            options: app.export_draft.clone(),
        };
        match app.export_preset_store.save(&preset) {
            Ok(()) => app.selected_export_preset = Some(preset.name),
            Err(e) => app.status = format!("Could not save preset: {e}"),
        }
        app.export_preset_list = app.export_preset_store.list();
        close = true;
    }
    app.modal = if close || dismissed {
        // Back to the export dialog rather than to nothing, so it does not vanish
        // underneath the user.
        Modal::ExportSettings
    } else {
        Modal::SaveExportPreset { name }
    };
}

/// Confirming an export-preset deletion.
///
/// Mirrors `confirm_delete_modal`, but folds into its **own** settings flag: an export
/// preset has no trash, so disarming the guard on the recoverable develop-preset deletion
/// must not disarm it here.
pub fn confirm_delete(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::ConfirmDeleteExportPreset {
        name,
        dont_ask_again,
    } = &mut app.modal
    else {
        return;
    };
    let name = std::mem::take(name);
    let mut dont_ask = *dont_ask_again;
    let mut outcome: Option<ConfirmOutcome> = None;

    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "confirm_delete_export",
        "Delete export preset",
        theme::DialogSize::Compact,
        |ui| {
            ui.label(format!("Delete the export preset \u{201c}{name}\u{201d}?"));
            theme::hint(ui, "This cannot be undone — export presets have no trash.");
            ui.checkbox(&mut dont_ask, "Don't ask again");
            ui.separator();
            let (confirmed, dismissed) = theme::button_row(ui, "Delete", true, "Cancel");
            if confirmed {
                outcome = Some(ConfirmOutcome::Confirmed {
                    suppress_future: dont_ask,
                });
            }
            if dismissed {
                outcome = Some(ConfirmOutcome::Cancelled);
            }
        },
    );
    // Clicking outside is a cancel — and therefore discards the tick too.
    if dismissed {
        outcome = Some(ConfirmOutcome::Cancelled);
    }

    match outcome {
        None => {
            app.modal = Modal::ConfirmDeleteExportPreset {
                name,
                dont_ask_again: dont_ask,
            };
        }
        Some(o) => {
            if app.settings.apply_export_delete_confirm(o) {
                app.delete_selected_export_preset();
            }
            app.save_settings();
            // Back to the export dialog rather than to nothing.
            app.modal = Modal::ExportSettings;
        }
    }
}

/// One destination already exists: what should happen to it?
///
/// Drawn whenever `export_collision` is set, which means the worker is **blocked**
/// waiting. Not a `Modal` variant, because an export can be started and then collide
/// long after the export dialog closed.
pub fn collision(app: &mut RustroomApp, ctx: &egui::Context) {
    let Some(dest) = app.export_collision.clone() else {
        return;
    };
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut apply_all = app.export_apply_all;
    let mut decision: Option<Decision> = None;

    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "export_collision",
        "File already exists",
        theme::DialogSize::Standard,
        |ui| {
            ui.label(format!("“{name}” is already in the destination folder."));
            ui.checkbox(&mut apply_all, "Apply to all remaining");
            ui.separator();
            ui.horizontal(|ui| {
                for (label, collision) in [
                    ("Skip", Collision::Skip),
                    ("Overwrite", Collision::Overwrite),
                    ("Keep both", Collision::KeepBoth),
                ] {
                    if ui.button(label).clicked() {
                        decision = Some(Decision::Resolve {
                            collision,
                            apply_to_all: apply_all,
                        });
                    }
                }
                if ui.button("Cancel export").clicked() {
                    decision = Some(Decision::Cancel);
                }
            });
        },
    );

    app.export_apply_all = apply_all;
    // ⚠ Dismissing by clicking outside cancels the run — and `Decision::Cancel` carries
    // no `apply_to_all`, so the tick cannot leak out of a cancel by construction.
    if dismissed {
        decision = Some(Decision::Cancel);
    }
    if let Some(d) = decision {
        if let Some(w) = &app.exporter {
            w.decide(d);
        }
        app.export_collision = None;
        // Cancelling from the prompt ends the run just as the toolbar's Cancel does,
        // rather than leaving a progress line up until the worker's Finished arrives.
        if d == Decision::Cancel {
            app.cancel_export();
        }
    }
}
