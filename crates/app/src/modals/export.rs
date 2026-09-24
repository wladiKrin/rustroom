//! The export flow'''s dialogs.
//!
//! Four in one file because they are one flow: `save_preset` and `confirm_delete` both
//! **return to** `Modal::ExportSettings` when closed, so the export dialog does not vanish
//! underneath the user.

use super::{dialog, Footer};
use crate::list_row::{self, RowAction, RowSpec};
use crate::{theme, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::batch_export::{banding_warning, can_export, Collision, Decision, ExportOptions};
use rustroom_app::batch_export::{Destination, UploadTarget};
use rustroom_app::batch_export::{Watermark, WatermarkCorner};
use rustroom_app::export_presets::ExportPreset;
use rustroom_app::export_summary;
use rustroom_app::filename_template::{NameFacts, Template};
use rustroom_app::settings::ConfirmOutcome;

/// The first photo of the run, as the filename preview needs it.
///
/// ⚠ Owned strings rather than borrows: it is read *inside* the dialog closure, which cannot
/// hold a borrow of `app` — see the note where it is built.
struct SampleName {
    file_stem: String,
    copy: u32,
    copy_name: Option<String>,
    taken: Option<i64>,
    camera: Option<String>,
}

impl SampleName {
    /// ⚠ `seq: 1` because this *is* the first photo, so the preview is the real answer rather
    /// than an illustration.
    fn facts(&self) -> NameFacts<'_> {
        NameFacts {
            file_stem: &self.file_stem,
            copy: self.copy,
            copy_label: self.copy_name.as_deref(),
            seq: 1,
            taken: self.taken,
            camera: self.camera.as_deref(),
        }
    }
}

/// The Metadata section's body.
fn metadata_body(ui: &mut egui::Ui, opts: &mut ExportOptions) {
    const HOVER: &str = "Written into the exported file as XMP. The original is never modified. \
                         A photo's own title, caption, copyright and creator (Information dialog) \
                         win over the fields above; leave a field empty to write nothing for it. \
                         \"All metadata\" also writes the camera, exposure, keywords and rating.";
    egui::Grid::new("export_metadata_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Copyright");
            ui.text_edit_singleline(&mut opts.metadata.copyright);
            ui.end_row();
            ui.label("Creator");
            ui.text_edit_singleline(&mut opts.metadata.creator);
            ui.end_row();
            ui.label("Caption");
            ui.text_edit_singleline(&mut opts.metadata.caption);
            ui.end_row();
            ui.label("Include");
            ui.horizontal(|ui| {
                for s in [
                    export::MetadataScope::All,
                    export::MetadataScope::CopyrightOnly,
                ] {
                    ui.selectable_value(&mut opts.metadata.scope, s, s.label())
                        .on_hover_text(HOVER);
                }
            });
            ui.end_row();
        });
}

/// The Watermark section's body.
fn watermark_body(ui: &mut egui::Ui, opts: &mut ExportOptions) {
    egui::Grid::new("export_watermark_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Text");
            ui.text_edit_singleline(&mut opts.watermark.text)
                .on_hover_text(
                    "Scales with the exported image, so one setting suits every output size. \
                 Skipped on a file too small to hold it.",
                );
            ui.end_row();
            ui.label("Corner");
            egui::ComboBox::from_id_salt("watermark_corner")
                .selected_text(opts.watermark.corner.label())
                .show_ui(ui, |ui| {
                    for c in WatermarkCorner::ALL {
                        ui.selectable_value(&mut opts.watermark.corner, c, c.label());
                    }
                });
            ui.end_row();
        });
    let (lo, hi) = Watermark::SIZE_RANGE;
    ui.add(egui::Slider::new(&mut opts.watermark.size, lo..=hi).text("Size (% of long edge)"));
    ui.add(egui::Slider::new(&mut opts.watermark.opacity, 0..=100).text("Opacity"));
}

/// One summarised, collapsible section of the dialog.
fn section(
    ui: &mut egui::Ui,
    name: &str,
    trailing: &str,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    use crate::develop::panel::section::{header, Header};
    let mut out = header(
        ui,
        egui::Id::new(("export", name)),
        default_open,
        Header::new(name).trailing(trailing),
    );
    out.state.show_body_unindented(ui, body);
}

/// The preset column's width.
const PRESETS_W: f32 = 170.0;

pub fn settings(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut opts = app.export_draft.clone();
    let jobs = app.export_jobs();
    let count = jobs.len();
    // ⚠ Taken **before** the closure, which already borrows `app` through `dialog` — the borrow
    // shape this file's own comment says has cost a compile error here twice. It is the first
    // photo of the run, so its `{seq}` is 1 and the preview says what will actually be written.
    let sample = jobs.first().map(|j| SampleName {
        file_stem: j
            .id
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("export")
            .to_string(),
        copy: j.id.copy,
        copy_name: j.name.clone(),
        taken: j.capture.taken,
        camera: j.capture.camera.clone(),
    });
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

    let title = match count {
        0 => "Export".to_string(),
        1 => "Export 1 photo".to_string(),
        n => format!("Export {n} photos"),
    };
    let note = export_summary::footer_note(&presets, selected.as_deref(), &opts);
    // ⚠⚠ Both read **before** the closure, which borrows `app.settings` mutably through `dialog`
    // — the borrow shape this file's own comment says has cost a compile error here twice.
    let upload_avail = rustroom_app::immich_upload::availability(&app.settings);
    let upload_host = app.settings.immich_url.trim().to_string();
    // ⚠ The first photo of the run, matching how `sample` previews `{seq}` = 1.
    let already_uploaded = jobs.first().and_then(|j| {
        rustroom_app::immich_upload::uploaded_note(&j.id.path, j.id.copy, &upload_host)
    });
    let mut delete_named: Option<String> = None;
    // ⚠ Option C of the canvas "Rustroom Export" (2026-09-22): the presets as a list on the
    // left, the settings as summarised, collapsible sections on the right — a collapsed
    // section's header says what it is set to, so the closed list is the export in eight
    // lines. The four tabs it replaces had the same controls and no overview.
    let out = dialog(
        ctx,
        &mut app.settings,
        "export_settings",
        &title,
        theme::DialogSize::Wide,
        Footer::confirm(
            "Export",
            can_export(
                &opts,
                upload_avail == rustroom_app::immich_upload::Availability::Ready,
            ),
            "Cancel",
        )
        .note(&note),
        |ui| {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(PRESETS_W, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_height(ui.available_height());
                        theme::sub_label(ui, "Presets");
                        if presets.is_empty() {
                            theme::hint(ui, "No presets yet.");
                        }
                        for p in &presets {
                            let r = list_row::row(
                                ui,
                                RowSpec {
                                    name: &p.name,
                                    count: None,
                                    meta: "",
                                    selected: p.options == opts,
                                    indent: 0.0,
                                    hover: "",
                                    actions: &[RowAction {
                                        glyph: '×',
                                        verb: "Delete",
                                        hover: "Delete this preset",
                                        enabled: true,
                                    }],
                                },
                            );
                            if r.action == Some(0) {
                                delete_named = Some(p.name.clone());
                            } else if r.clicked {
                                picked = Some((p.name.clone(), p.options.clone()));
                            }
                        }
                        ui.add_space(6.0);
                        let save = ui
                            .button("+ Save current…")
                            .on_hover_text("Save these settings as a preset");
                        // ⚠ The accessible name is the sentence, as the Presets panel's.
                        save.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                "Save current as preset…",
                            )
                        });
                        if save.clicked() {
                            save_as = true;
                        }
                    },
                );
                ui.separator();
                ui.vertical(|ui| {
                    section(
                        ui,
                        "Location",
                        &export_summary::location(&opts),
                        true,
                        |ui| {
                            ui.horizontal(|ui| {
                                if ui.button("Choose…").clicked() {
                                    pick_dir = true;
                                }
                                let shown = if opts.dest_dir.as_os_str().is_empty() {
                                    "(none chosen)".to_string()
                                } else {
                                    opts.dest_dir.display().to_string()
                                };
                                ui.label(shown);
                            });
                        },
                    );
                    let first = match Template::parse(&opts.filename_template) {
                        Ok(t) => sample
                            .as_ref()
                            .map(|s| {
                                format!("{}.{}", t.render(&s.facts()), opts.format.extension())
                            })
                            .unwrap_or_default(),
                        Err(_) => "invalid".to_string(),
                    };
                    section(ui, "Naming", &first, true, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut opts.filename_template)
                                .desired_width(f32::INFINITY),
                        );
                        let tokens: Vec<String> = rustroom_app::filename_template::token_help()
                            .iter()
                            .map(|(tok, _)| tok.to_string())
                            .collect();
                        theme::hint(ui, &tokens.join("  "));
                        if let Err(e) = Template::parse(&opts.filename_template) {
                            theme::error_hint(ui, &e.to_string());
                        }
                    });
                    section(ui, "File", &export_summary::file(&opts), true, |ui| {
                        egui::Grid::new("export_output")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ui.label("Format");
                                egui::ComboBox::from_id_salt("format_pick")
                                    .selected_text(opts.format.label())
                                    .show_ui(ui, |ui| {
                                        for f in export::ExportFormat::ALL {
                                            ui.selectable_value(&mut opts.format, f, f.label());
                                        }
                                    });
                                ui.end_row();
                                ui.label("Colour space");
                                egui::ComboBox::from_id_salt("space_pick")
                                    .selected_text(opts.space.label())
                                    .show_ui(ui, |ui| {
                                        for s in color::OutputSpace::ALL {
                                            ui.selectable_value(&mut opts.space, s, s.label());
                                        }
                                    });
                                ui.end_row();
                            });
                        if let Some(msg) = banding_warning(opts.space, opts.format) {
                            theme::hint(ui, msg);
                        }
                        ui.add_enabled(
                            opts.format.is_lossy(),
                            egui::Slider::new(&mut opts.quality, 1..=100).text("JPEG quality"),
                        );
                    });
                    section(ui, "Size", &export_summary::size(&opts), false, |ui| {
                        let mut resize = opts.max_edge.is_some();
                        ui.checkbox(&mut resize, "Fit within a long edge");
                        if resize {
                            let mut edge = opts.max_edge.unwrap_or(2048);
                            ui.add(egui::Slider::new(&mut edge, 256..=8192).text("Long edge (px)"));
                            opts.max_edge = Some(edge);
                        } else {
                            opts.max_edge = None;
                        }
                    });
                    section(
                        ui,
                        "Sharpening",
                        &export_summary::sharpening(&opts),
                        false,
                        |ui| {
                            ui.add(
                                egui::Slider::new(&mut opts.output_sharpening.amount, 0..=100)
                                    .text("Amount"),
                            )
                            .on_hover_text(
                                "Applied to the finished file at a fixed radius, on top of the \
                             Detail panel's sharpening — it puts back the edge a resize takes. \
                             Not shown in the preview.",
                            );
                        },
                    );
                    section(
                        ui,
                        "Demosaic",
                        &export_summary::demosaic(&opts),
                        false,
                        |ui| {
                            egui::ComboBox::from_id_salt("demosaic_pick")
                            .selected_text(opts.demosaic.label())
                            .show_ui(ui, |ui| {
                                for d in raw_decode::Demosaic::ALL {
                                    ui.selectable_value(&mut opts.demosaic, d, d.label());
                                }
                            })
                            .response
                            .on_hover_text(
                                "RAW only. Higher is slower — roughly 1.2 s to 11 s per photo. \
                                 Independent of the 1:1 view's own quality setting, so judging \
                                 this means exporting a frame at two settings and comparing crops.",
                            );
                        },
                    );
                    section(
                        ui,
                        "Metadata",
                        &export_summary::metadata(&opts),
                        false,
                        |ui| metadata_body(ui, &mut opts),
                    );
                    section(
                        ui,
                        "Watermark",
                        &export_summary::watermark(&opts),
                        false,
                        |ui| watermark_body(ui, &mut opts),
                    );
                    // ⚠⚠ Drawn only when Immich is configured, and it holds **every** Immich
                    // mention in this dialog — which is what keeps "switch it off and no mention
                    // remains" a single condition rather than a list of places to remember.
                    if upload_avail != rustroom_app::immich_upload::Availability::NotConfigured {
                        section(
                            ui,
                            "Delivery",
                            &export_summary::delivery(&opts),
                            false,
                            |ui| {
                                let ready = upload_avail
                                    == rustroom_app::immich_upload::Availability::Ready;
                                let mut mode = opts.destination;
                                let r = ui.add_enabled_ui(ready, |ui| {
                                    crate::segmented::segmented(
                                        ui,
                                        &mut mode,
                                        &[
                                            (Destination::Files, "Files only"),
                                            (
                                                Destination::Target(UploadTarget::Immich),
                                                "Immich only",
                                            ),
                                            (
                                                Destination::Both(UploadTarget::Immich),
                                                "Files and Immich",
                                            ),
                                        ],
                                    )
                                });
                                if ready {
                                    opts.destination = mode;
                                } else {
                                    r.response.on_disabled_hover_text(
                                        "Set an Upload key in Settings → Immich first. Browsing \
                                         uses a read-only key, which cannot upload.",
                                    );
                                }
                                // ⚠ Disabled rather than hidden when nothing is uploaded: it has
                                // nothing to act on, and a control that vanishes reads as a bug.
                                ui.add_enabled_ui(opts.destination.target().is_some(), |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label("Date");
                                        egui::ComboBox::from_id_salt("upload_date")
                                            .selected_text(opts.upload_date.label())
                                            .show_ui(ui, |ui| {
                                                for d in rustroom_app::batch_export::UploadDate::ALL
                                                {
                                                    ui.selectable_value(
                                                        &mut opts.upload_date,
                                                        d,
                                                        d.label(),
                                                    );
                                                }
                                            });
                                    });
                                });
                                if let Some(note) = &already_uploaded {
                                    // ⚠⚠ Informs, never blocks — a re-export after editing IS a new
                                    // picture, and replacing the old asset would mean deleting one.
                                    ui.label(egui::RichText::new(note).weak()).on_hover_text(
                                        "Uploading again adds a second photo to Immich. Immich \
                                         recognises a byte-identical re-upload and keeps only one.",
                                    );
                                }
                            },
                        );
                    }
                });
            });
        },
    );
    if out.primary {
        start = true;
    }
    if out.dismissed {
        close = true;
    }

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
    if let Some(name) = delete_named {
        app.selected_export_preset = Some(name);
        delete_selected = true;
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
    if close {
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
    let out = dialog(
        ctx,
        &mut app.settings,
        "save_export_preset",
        "Save export preset",
        theme::DialogSize::Standard,
        Footer::confirm("Save", !name.trim().is_empty(), "Cancel"),
        |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut name);
            });
        },
    );
    save |= out.primary;
    close |= out.dismissed;
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
    app.modal = if close {
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

    let out = dialog(
        ctx,
        &mut app.settings,
        "confirm_delete_export",
        "Delete export preset?",
        theme::DialogSize::Compact,
        Footer::confirm("Delete", true, "Cancel"),
        |ui| {
            ui.label(format!("Delete the export preset \u{201c}{name}\u{201d}?"));
            theme::hint(ui, "This cannot be undone — export presets have no trash.");
            ui.checkbox(&mut dont_ask, "Don't ask again");
        },
    );
    // Clicking outside is a cancel — and therefore discards the tick too.
    if out.primary {
        outcome = Some(ConfirmOutcome::Confirmed {
            suppress_future: dont_ask,
        });
    } else if out.dismissed {
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

    // ⚠ "Keep both" is the primary: it is the one answer that loses nothing. Skip and
    // Overwrite are the extras, in that order; dismissing — the ×, the backdrop, "Cancel
    // export" — cancels the export, as it always did, and `Decision::Cancel` carries no
    // `apply_to_all`, so the tick cannot leak out of a cancel by construction.
    let out = dialog(
        ctx,
        &mut app.settings,
        "export_collision",
        "File already exists",
        theme::DialogSize::Standard,
        Footer::confirm("Keep both", true, "Cancel export").extra(&["Skip", "Overwrite"]),
        |ui| {
            ui.label(format!("“{name}” is already in the destination folder."));
            ui.checkbox(&mut apply_all, "Apply to all remaining");
        },
    );
    app.export_apply_all = apply_all;
    let resolve = |collision: Collision| Decision::Resolve {
        collision,
        apply_to_all: apply_all,
    };
    if out.primary {
        decision = Some(resolve(Collision::KeepBoth));
    } else if out.extra == Some(0) {
        decision = Some(resolve(Collision::Skip));
    } else if out.extra == Some(1) {
        decision = Some(resolve(Collision::Overwrite));
    } else if out.dismissed {
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

#[cfg(test)]
mod tests {
    use super::ExportPreset;
    use crate::{test_app, test_ui};

    /// Draw the export dialog once with `template` set, and report every widget label it drew.
    fn labels(template: &str) -> Vec<String> {
        with_app(|app| app.export_draft.filename_template = template.to_string())
    }

    /// Draw the dialog once after `setup` has adjusted the app, and report every widget label.
    fn with_app(setup: impl FnOnce(&mut crate::RustroomApp)) -> Vec<String> {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        // Selecting a photo gives the preview a sample to render, which is the line that would
        // panic if `NameFacts` were built from a borrow the closure cannot hold.
        app.library = Some(test_app::library_of(2));
        app.library.as_mut().unwrap().selection.select_indices(&[0]);
        setup(&mut app);
        test_ui::labelled_centres(&ctx, Vec::new(), |_| super::settings(&mut app, &ctx))
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    }

    #[test]
    fn an_invalid_template_leaves_export_on_screen_rather_than_hiding_it() {
        // ⚠ This project's convention: a control that cannot be used goes **dark, not away** — a
        // menu or a dialog that changes shape is harder to learn than one that greys out. So the
        // guard against a typo must not be "the button vanished", which reads as a crash.
        //
        // ⚠ It also exercises the whole dialog body under both branches, which is the only
        // headless coverage available here: `theme::hint` is a plain `ui.label`, and a plain
        // label produces **no AccessKit node**, so neither the preview nor the error message can
        // be read back. Stated rather than papered over with a test that appears to check them.
        for t in ["{name}", "{camrea}", "{seq", "{name:2}"] {
            let drawn = labels(t);
            assert!(
                drawn.iter().any(|l| l == "Export"),
                "Export missing for template {t:?}: {drawn:?}"
            );
            assert!(drawn.iter().any(|l| l == "Cancel"), "{t:?}");
        }
    }

    #[test]
    fn the_sections_the_presets_and_the_footer_are_offered() {
        let drawn = with_app(|app| {
            app.export_preset_list = vec![ExportPreset {
                name: "Web".into(),
                options: app.export_draft.clone(),
            }];
        });
        for want in [
            "Location",
            "Naming",
            "File",
            "Size",
            "Sharpening",
            "Demosaic",
            "Metadata",
            "Watermark",
            "Web",
            "Delete Web",
            "Save current as preset…",
            "Export",
            "Cancel",
        ] {
            assert!(
                drawn.iter().any(|l| l == want),
                "{want:?} missing: {drawn:?}"
            );
        }
    }

    /// A preset row is lit when the options equal its own, and not after a change.
    #[test]
    fn the_matching_preset_row_is_lit() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.library = Some(test_app::library_of(2));
        app.library.as_mut().unwrap().selection.select_indices(&[0]);
        app.export_preset_list = vec![ExportPreset {
            name: "Web".into(),
            options: app.export_draft.clone(),
        }];
        let lit = |app: &mut crate::RustroomApp| -> Option<bool> {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(test_ui::screen()),
                    ..Default::default()
                },
                |_| super::settings(app, &ctx),
            );
            let update = out.platform_output.accesskit_update?;
            update.nodes.iter().find_map(|(_, n)| {
                (n.label() == Some("Web"))
                    .then(|| n.toggled() == Some(egui::accesskit::Toggled::True))
            })
        };
        assert_eq!(lit(&mut app), Some(true));
        app.export_draft.quality = app.export_draft.quality.wrapping_add(1);
        assert_eq!(lit(&mut app), Some(false));
    }

    /// ⚠⚠ **"No mention of immich at all" when the integration is off** (`docs/BACKLOG.md`) — so an
    /// unconfigured Immich must draw **no** Upload section, not a disabled one. The whole point of
    /// the requirement is that a user who does not use Immich never sees the word.
    #[test]
    fn without_immich_configured_the_delivery_section_is_absent() {
        let drawn = with_app(|app| {
            app.settings.immich_url.clear();
            app.settings.immich_key.clear();
        });
        assert!(
            !drawn.iter().any(|l| l == "Delivery"),
            "a Delivery section was drawn with no Immich configured: {drawn:?}"
        );
        // ⚠ The sample assertion: the dialog really did draw, so the absence above means something.
        assert!(
            drawn.iter().any(|l| l == "Watermark"),
            "the dialog did not draw at all: {drawn:?}"
        );
    }

    /// ⚠ Configured but with no upload key: the section appears and the control is **disabled**,
    /// because a control that vanishes reads as a bug — this file's own convention, three tests up.
    #[test]
    fn with_immich_but_no_upload_key_the_delivery_section_appears() {
        let drawn = with_app(|app| {
            app.settings.immich_url = "https://immich.example".to_string();
            app.settings.immich_key = "browse-key".to_string();
            app.settings.immich_upload_key.clear();
        });
        assert!(
            drawn.iter().any(|l| l == "Delivery"),
            "the Delivery section should appear once Immich is configured: {drawn:?}"
        );
    }

    /// ⚠⚠ **The section is COLLAPSED by default, so its body is not laid out** — a first version of
    /// this test looked for "Files only" without opening it and failed against correct code. That is
    /// the same vacuous-guard shape as the Color section's: a query that finds nothing passes
    /// silently, so here it would have *failed* silently for the wrong reason. It is opened the way a
    /// user opens it: by clicking the header.
    #[test]
    fn the_three_delivery_modes_and_the_date_are_offered() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.library = Some(test_app::library_of(2));
        app.library.as_mut().unwrap().selection.select_indices(&[0]);
        app.settings.immich_url = "https://immich.example".to_string();
        app.settings.immich_key = "browse-key".to_string();
        app.settings.immich_upload_key = "upload-key".to_string();

        // Closed first: the body is genuinely absent, which is what makes the click meaningful.
        let closed: Vec<String> =
            test_ui::labelled_centres(&ctx, Vec::new(), |_| super::settings(&mut app, &ctx))
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        assert!(
            closed.iter().any(|l| l == "Delivery"),
            "the section header must be there to click: {closed:?}"
        );
        assert!(
            !closed.iter().any(|l| l == "Files only"),
            "the body must start collapsed, or this test proves nothing"
        );

        test_ui::click(&ctx, "Delivery", |_| super::settings(&mut app, &ctx));
        let open: Vec<String> =
            test_ui::labelled_centres(&ctx, Vec::new(), |_| super::settings(&mut app, &ctx))
                .into_iter()
                .map(|(l, _)| l)
                .collect();

        for want in [
            "Files only",
            "Immich only",
            "Files and Immich",
            "Original date",
        ] {
            assert!(open.iter().any(|l| l == want), "{want:?} missing: {open:?}");
        }
    }
}
