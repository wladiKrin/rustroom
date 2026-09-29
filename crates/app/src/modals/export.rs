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

/// What a DNG is — the Format combo's hover. ⚠ It was a paragraph under the Colour section,
/// shown whenever DNG was chosen (`docs/DESIGN.md` §8).
const DNG_NOTE: &str = "DNG holds the finished picture as linear ProPhoto, 16-bit, losslessly \
                        compressed; the application that opens it converts. ⚠ darktable adds its \
                        own exposure and tone mapping to raw files unless its workflow is set to \
                        \"none\".";

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

/// The destination-folder row: a Choose button and the path.
///
/// ⚠ Extracted when Location absorbed Delivery, so the one row is drawn identically whether or not
/// the Immich ticks are present.
fn folder_row(ui: &mut egui::Ui, opts: &ExportOptions, pick_dir: &mut bool) {
    ui.horizontal(|ui| {
        if ui.button("Choose…").clicked() {
            *pick_dir = true;
        }
        let shown = if opts.dest_dir.as_os_str().is_empty() {
            "(none chosen)".to_string()
        } else {
            opts.dest_dir.display().to_string()
        };
        ui.label(shown);
    });
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
    // ⚠⚠ **A destination naming a target that is not reachable is coerced back to Files.** Without
    // this, switching Immich off left a preset saying "Immich only" with **no visible control to
    // change it** — the ticks are not drawn — so Export stayed dark and the Location header still
    // read "Immich, original date", which is also the word leaking into a UI that is supposed to
    // have no mention of it. Found by use, 2026-09-24.
    //
    // ⚠ The **draft** is coerced, never a saved preset: the preset still says what it says, and
    // re-enabling Immich brings its meaning back.
    if upload_avail == rustroom_app::immich_upload::Availability::NotConfigured
        && opts.destination.target().is_some()
    {
        opts.destination = Destination::Files;
    }
    let upload_host = app.settings.immich_url.trim().to_string();
    // ⚠ The first photo of the run, matching how `sample` previews `{seq}` = 1.
    let already_uploaded = jobs.first().and_then(|j| {
        rustroom_app::immich_upload::uploaded_note(&j.id.path, j.id.copy, &upload_host)
    });
    // ⚠⚠ **Only for a single photo, and computed by the same `upload_seconds` the upload calls** — a
    // second formatting of "what the date will be" is a second implementation that can disagree with
    // the first, and the whole point of showing it is that it can be trusted. For a selection each
    // photo has its own capture and edit date, so one number would be a lie.
    let resolved_date = jobs.first().filter(|_| jobs.len() == 1).map(|j| {
        let edited = rustroom_app::sidecar::edited_at(&j.id.path).and_then(|s| {
            s.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs() as i64)
        });
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        rustroom_app::when::stamp(rustroom_app::immich_upload::upload_seconds(
            opts.upload_date,
            j.capture.taken,
            edited,
            now,
        ))
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
                            // ⚠⚠ **With Immich unconfigured this draws exactly what it drew before
                            // the whole delivery feature existed: the folder row and no ticks.** A
                            // lone "Local" tick with nowhere else to go is a control that cannot
                            // mean anything, and one unconditional shape is the easiest way to keep
                            // "no mention of immich at all" true.
                            let choosable = upload_avail
                                != rustroom_app::immich_upload::Availability::NotConfigured;
                            if choosable {
                                let mut local = opts.destination.keeps_files();
                                let mut immich = opts.destination.target().is_some();
                                let before = (local, immich);
                                ui.checkbox(&mut local, "Local");
                                ui.indent("export_local", |ui| {
                                    // ⚠ Enabled even when unticked, so the folder can be chosen
                                    // before ticking it — and because it is still where a failed
                                    // upload lands under Immich-only.
                                    folder_row(ui, &opts, &mut pick_dir);
                                });

                                let ready = upload_avail
                                    == rustroom_app::immich_upload::Availability::Ready;
                                let r = ui
                                    .add_enabled(ready, egui::Checkbox::new(&mut immich, "Immich"));
                                if !ready {
                                    r.on_disabled_hover_text(
                                        "Set an Upload key in Settings → Immich first. Browsing \
                                         uses a read-only key, which cannot upload.",
                                    );
                                }
                                ui.indent("export_immich", |ui| {
                                    ui.add_enabled_ui(immich, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label("Date");
                                            egui::ComboBox::from_id_salt("upload_date")
                                                .selected_text(opts.upload_date.label())
                                                .show_ui(ui, |ui| {
                                                    for d in
                                                        rustroom_app::batch_export::UploadDate::ALL
                                                    {
                                                        ui.selectable_value(
                                                            &mut opts.upload_date,
                                                            d,
                                                            d.label(),
                                                        );
                                                    }
                                                });
                                            // ⚠⚠ The **resolved** moment, for one photo only —
                                            // computed by the same `upload_seconds` the upload
                                            // calls, so the preview cannot disagree with what
                                            // actually happens. For a selection each photo has its
                                            // own date and one number would be a lie.
                                            if let Some(when) = &resolved_date {
                                                ui.label(
                                                    egui::RichText::new(format!("→ {when}")).weak(),
                                                )
                                                .on_hover_text(
                                                    "The date this photo will carry in Immich.",
                                                );
                                            }
                                        });
                                        if let Some(note) = &already_uploaded {
                                            // ⚠⚠ Informs, never blocks — a re-export after editing
                                            // IS a new picture, and replacing the old asset would
                                            // mean deleting one.
                                            ui.label(egui::RichText::new(note).weak())
                                                .on_hover_text(
                                                    "Uploading again adds a second photo to \
                                                     Immich. Immich recognises a byte-identical \
                                                     re-upload and keeps only one.",
                                                );
                                        }
                                    });
                                });

                                if (local, immich) != before {
                                    opts.destination = match (local, immich) {
                                        (true, false) => Destination::Files,
                                        (false, true) => Destination::Target(UploadTarget::Immich),
                                        (true, true) => Destination::Both(UploadTarget::Immich),
                                        // ⚠ Reachable on purpose: Export goes dark and the footer
                                        // note says why.
                                        (false, false) => Destination::Nowhere,
                                    };
                                }
                            } else {
                                folder_row(ui, &opts, &mut pick_dir);
                            }
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
                        )
                        .on_hover_text(rustroom_app::filename_template::token_hover());
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
                                    })
                                    .response
                                    .on_hover_text(DNG_NOTE);
                                ui.end_row();
                            });
                        // ⚠ Hidden, not disabled, for a lossless format: at this size a disabled
                        // slider reads the same as an enabled one (P-05).
                        if opts.format.is_lossy() {
                            ui.add(
                                egui::Slider::new(&mut opts.quality, 1..=100).text("JPEG quality"),
                            );
                        }
                    });
                    // ⚠ Its own collapsed section: colour space and rendering intent are the two
                    // terms a first export has no use for, and they sat between Format and the
                    // quality slider (A-25). sRGB and Clip are the silent defaults.
                    section(ui, "Colour", &export_summary::colour(&opts), false, |ui| {
                        egui::Grid::new("export_colour")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ui.label("Colour space");
                                // ⚠ Disabled for DNG, which is always linear ProPhoto — the
                                // opening application converts (`ExportFormat::fixed_space`).
                                let fixed = opts.format.fixed_space().is_some();
                                ui.add_enabled_ui(!fixed, |ui| egui::ComboBox::from_id_salt("space_pick")
                                    .selected_text(opts.space.label())
                                    .show_ui(ui, |ui| {
                                        for s in color::OutputSpace::ALL {
                                            ui.selectable_value(&mut opts.space, s, s.label());
                                        }
                                    }));
                                ui.end_row();
                                ui.label("Rendering intent");
                                // ⚠ Disabled, not hidden, for ProPhoto: the working space maps
                                // nothing, and a row that vanishes moves the grid under the pointer.
                                ui.add_enabled_ui(opts.space != color::OutputSpace::ProPhoto, |ui| {
                                    egui::ComboBox::from_id_salt("intent_pick")
                                        .selected_text(opts.intent.label())
                                        .show_ui(ui, |ui| {
                                            for i in color::RenderingIntent::ALL {
                                                ui.selectable_value(&mut opts.intent, i, i.label());
                                            }
                                        })
                                        .response
                                        .on_hover_text(
                                            "Clip clamps each channel: vivid, what Lightroom does, but \
                                             it can shift hue. Perceptual and Relative colorimetric keep \
                                             hue: perceptual compresses saturated colour to keep its \
                                             gradation, relative leaves in-gamut colour untouched and \
                                             cuts the rest at the gamut's edge.",
                                        );
                                });
                                ui.end_row();
                            });
                        if let Some(msg) = banding_warning(opts.space, opts.format) {
                            theme::hint(ui, msg);
                        }

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

/// Telling the user an upload did not confirm, and where the file is.
///
/// ⚠⚠ **A notice, not a question.** There is nothing to decide: the file was **not** deleted, it is
/// on disk, and the export is very likely still running. So one button, and the text names the path.
///
/// ⚠ "Did not confirm" rather than "failed", and that wording is load-bearing: an SSL/EOF error means
/// the *response* was lost, not that the server rejected the file — 4 of 2,373 writes failed that way
/// on this network and **none had actually failed to apply**. Telling the user it failed could have
/// them upload a second copy.
pub fn upload_unconfirmed(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::UploadUnconfirmed { at, why } = &app.modal else {
        return;
    };
    let (at, why) = (at.clone(), why.clone());
    let out = dialog(
        ctx,
        &mut app.settings,
        "upload_unconfirmed",
        "Upload not confirmed",
        // ⚠ `Standard`, not `Compact`: this carries a full path and two hints, which `Compact` —
        // sized for "a sentence and two buttons" — wrapped too tightly to read (user's call, on use).
        theme::DialogSize::Standard,
        Footer::confirm("OK", true, ""),
        |ui| {
            ui.label("An upload did not confirm, so the file was kept rather than removed.");
            ui.add_space(4.0);
            ui.label(egui::RichText::new(at.display().to_string()).monospace());
            ui.add_space(4.0);
            theme::hint(ui, &format!("The server said: {why}"));
            theme::hint(
                ui,
                "It may still have arrived — Immich recognises a byte-identical re-upload and keeps \
                 only one, so exporting it again is safe.",
            );
        },
    );
    if out.primary || out.dismissed {
        app.modal = Modal::None;
    }
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

    /// ⚠⚠ **"No mention of immich at all" when the integration is off** (`docs/BACKLOG.md`) — and
    /// since Delivery folded into Location, that means Location must draw its folder row and **no
    /// ticks**: a lone "Local" tick with nowhere else to go is a control that cannot mean anything.
    #[test]
    fn without_immich_configured_location_has_no_ticks() {
        let drawn = with_app(|app| {
            app.settings.immich_url.clear();
            app.settings.immich_key.clear();
        });
        for absent in ["Local", "Immich", "Delivery", "Original date"] {
            assert!(
                !drawn.iter().any(|l| l == absent),
                "{absent:?} was drawn with no Immich configured: {drawn:?}"
            );
        }
        // ⚠ The sample assertions: Location drew, and its folder row is still there — so the
        // absences above mean something.
        assert!(
            drawn.iter().any(|l| l == "Location"),
            "the Location section is missing: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|l| l == "Choose…"),
            "the folder row must remain: {drawn:?}"
        );
    }

    /// ⚠ Configured: both ticks appear inside Location, along with the date dropdown. Location is
    /// **open by default**, so unlike the old Delivery section nothing has to be clicked first.
    #[test]
    fn with_immich_configured_location_offers_both_ticks_and_the_date() {
        let drawn = with_app(|app| {
            app.settings.immich_url = "https://immich.example".to_string();
            app.settings.immich_key = "browse-key".to_string();
            app.settings.immich_upload_key = "upload-key".to_string();
        });
        for want in [
            "Location",
            "Local",
            "Immich",
            "Date",
            "Original date",
            "Choose…",
        ] {
            assert!(
                drawn.iter().any(|l| l == want),
                "{want:?} missing: {drawn:?}"
            );
        }
        // ⚠ And the retired section name must be gone, not merely unused.
        assert!(!drawn.iter().any(|l| l == "Delivery"), "{drawn:?}");
    }

    /// ⚠⚠ **The bug the user reported: the Location header still read "Immich, original date" while
    /// the ticks were hidden.** A persisted `Target` destination with Immich unreachable left Export
    /// dark with **no visible control to change it**, and leaked the word into a UI that is supposed
    /// to have no mention of it.
    ///
    /// ⚠⚠ **Asserted on the DRAFT, not on the drawn labels** — and the first version of this test
    /// was vacuous for exactly that reason: it looked for "immich" among the labels, but a section's
    /// trailing summary is **painted**, which the harness cannot read (`export_summary`'s own module
    /// doc says so). It passed with the fix sabotaged. What is observable is `app.export_draft` after
    /// the dialog runs, which is where the coercion lands.
    #[test]
    fn an_unreachable_destination_is_coerced_back_to_files() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.library = Some(test_app::library_of(2));
        app.library.as_mut().unwrap().selection.select_indices(&[0]);
        app.settings.immich_url.clear();
        app.settings.immich_key.clear();
        // A preset saved while Immich worked, now unreachable.
        app.export_draft.destination = rustroom_app::batch_export::Destination::Target(
            rustroom_app::batch_export::UploadTarget::Immich,
        );

        let _ = test_ui::labelled_centres(&ctx, Vec::new(), |_| super::settings(&mut app, &ctx));

        assert_eq!(
            app.export_draft.destination,
            rustroom_app::batch_export::Destination::Files,
            "an unreachable target must be coerced, or Export stays dark with the cause hidden"
        );
    }

    /// ⚠ The control: with Immich reachable the destination is **left alone**, or the coercion would
    /// quietly undo the user's choice every time the dialog opened.
    #[test]
    fn a_reachable_destination_is_left_alone() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.library = Some(test_app::library_of(2));
        app.library.as_mut().unwrap().selection.select_indices(&[0]);
        app.settings.immich_url = "https://immich.example".to_string();
        app.settings.immich_key = "browse-key".to_string();
        app.settings.immich_upload_key = "upload-key".to_string();
        let wanted = rustroom_app::batch_export::Destination::Target(
            rustroom_app::batch_export::UploadTarget::Immich,
        );
        app.export_draft.destination = wanted;

        let _ = test_ui::labelled_centres(&ctx, Vec::new(), |_| super::settings(&mut app, &ctx));

        assert_eq!(
            app.export_draft.destination, wanted,
            "a reachable target must survive the dialog"
        );
    }
}
