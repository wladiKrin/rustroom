//! The export flow'''s dialogs.
//!
//! Four in one file because they are one flow: `save_preset` and `confirm_delete` both
//! **return to** `Modal::ExportSettings` when closed, so the export dialog does not vanish
//! underneath the user.

use super::shell::tabs_with_hover;
use super::{dialog, Footer};
use crate::{theme, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::batch_export::{banding_warning, can_export, Collision, Decision, ExportOptions};
use rustroom_app::batch_export::{Watermark, WatermarkCorner};
use rustroom_app::export_presets::ExportPreset;
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

/// A tab's hover: whether its section is doing anything, so it need not be opened to know.
///
/// ⚠ The collapsing headers used to carry this as a `·` suffix in their title; a tab's title
/// is its name, so the fact moved to the hover — pure, because a hover is invisible to the
/// harness and this is the one place the sentence is written.
fn tab_hover(name: &str, on: bool) -> String {
    format!("{name} is {}", if on { "on" } else { "off" })
}

/// The Metadata tab.
fn metadata_tab(ui: &mut egui::Ui, opts: &mut ExportOptions) {
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
                    ui.selectable_value(&mut opts.metadata.scope, s, s.label());
                }
            });
            ui.end_row();
        });
    theme::hint(
        ui,
        "Written into the exported file as XMP. The original is never modified. \
         A photo's own title, caption, copyright and creator (Information dialog) win \
         over the fields above; leave a field empty to write nothing for it. \
         \"All metadata\" also writes the camera, exposure, keywords and rating.",
    );
}

/// The Watermark tab.
fn watermark_tab(ui: &mut egui::Ui, opts: &mut ExportOptions) {
    egui::Grid::new("export_watermark_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Text");
            ui.text_edit_singleline(&mut opts.watermark.text);
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
    theme::hint(
        ui,
        "Scales with the exported image, so one setting suits every output size. \
         Skipped on a file too small to hold it.",
    );
}

/// The Output tab's sharpening block.
fn sharpening_block(ui: &mut egui::Ui, opts: &mut ExportOptions) {
    theme::sub_label(ui, "Output sharpening");
    ui.add(egui::Slider::new(&mut opts.output_sharpening.amount, 0..=100).text("Amount"));
    theme::hint(
        ui,
        "Applied to the finished file at a fixed radius, on top of the Detail \
         panel's sharpening — it puts back the edge a resize takes. Not shown in \
         the preview.",
    );
}

/// Where and how to export the selection.
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

    let note = format!(
        "{count} photo{} to export",
        if count == 1 { "" } else { "s" }
    );
    // ⚠ Four tabs (2026-09-21): Output, Filename, Metadata, Watermark. The collapsing headers
    // and their `(on)` suffix are gone with the single page.
    let out = dialog(
        ctx,
        &mut app.settings,
        "export_settings",
        "Export",
        theme::DialogSize::Standard,
        Footer::confirm("Export", can_export(&opts), "Cancel").note(&note),
        |ui| {
            let meta = tab_hover("Metadata", opts.metadata.is_active());
            let wm = tab_hover("Watermark", opts.watermark.is_active());
            let tab = tabs_with_hover(
                ui,
                egui::Id::new("export_tab"),
                &[
                    ("Output", ""),
                    ("Filename", ""),
                    ("Metadata", &meta),
                    ("Watermark", &wm),
                ],
            );
            match tab {
                0 => {
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("export_preset_pick")
                            .selected_text(selected.clone().unwrap_or_else(|| "Preset…".into()))
                            .show_ui(ui, |ui| {
                                for p in &presets {
                                    let is_sel = selected.as_deref() == Some(p.name.as_str());
                                    if ui.selectable_label(is_sel, &p.name).clicked() {
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
                    theme::sub_label(ui, "Destination");
                    ui.horizontal(|ui| {
                        if ui.button("Destination…").clicked() {
                            pick_dir = true;
                        }
                        let shown = if opts.dest_dir.as_os_str().is_empty() {
                            "(none chosen)".to_string()
                        } else if opts.dest_dir.is_dir() {
                            opts.dest_dir.display().to_string()
                        } else {
                            format!("{} (missing)", opts.dest_dir.display())
                        };
                        ui.label(shown);
                    });
                    theme::sub_label(ui, "File");
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
                    let mut resize = opts.max_edge.is_some();
                    ui.checkbox(&mut resize, "Fit within a long edge");
                    if resize {
                        let mut edge = opts.max_edge.unwrap_or(2048);
                        ui.add(egui::Slider::new(&mut edge, 256..=8192).text("Long edge (px)"));
                        opts.max_edge = Some(edge);
                    } else {
                        opts.max_edge = None;
                    }
                    theme::sub_label(ui, "Demosaic");
                    egui::ComboBox::from_id_salt("demosaic_pick")
                        .selected_text(opts.demosaic.label())
                        .show_ui(ui, |ui| {
                            for d in raw_decode::Demosaic::ALL {
                                ui.selectable_value(&mut opts.demosaic, d, d.label());
                            }
                        });
                    theme::hint(
                        ui,
                        "RAW only. Higher is slower — roughly 1.2 s to 11 s per photo. \
                         Independent of the 1:1 view's own quality setting, so judging this \
                         means exporting a frame at two settings and comparing crops.",
                    );
                    sharpening_block(ui, &mut opts);
                }
                1 => {
                    ui.horizontal(|ui| {
                        ui.label("Filename");
                        ui.text_edit_singleline(&mut opts.filename_template);
                    });
                    egui::Grid::new("export_token_help")
                        .num_columns(2)
                        .spacing([10.0, 2.0])
                        .show(ui, |ui| {
                            for (tok, what) in rustroom_app::filename_template::token_help() {
                                theme::hint(ui, tok);
                                theme::hint(ui, what);
                                ui.end_row();
                            }
                        });
                    match Template::parse(&opts.filename_template) {
                        Ok(t) => {
                            if let Some(s) = &sample {
                                theme::hint(
                                    ui,
                                    &format!(
                                        "First file: {}.{}",
                                        t.render(&s.facts()),
                                        opts.format.extension()
                                    ),
                                );
                            }
                        }
                        Err(e) => theme::error_hint(ui, &e.to_string()),
                    }
                }
                2 => metadata_tab(ui, &mut opts),
                _ => watermark_tab(ui, &mut opts),
            }
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
    fn the_four_tabs_are_offered() {
        let drawn = with_app(|_| {});
        for want in ["Output", "Filename", "Metadata", "Watermark"] {
            assert!(
                drawn.iter().any(|l| l == want),
                "{want} tab missing: {drawn:?}"
            );
        }
    }

    /// ⚠ The footer is pinned, so the verb is there whichever tab is open — the reason the
    /// footer exists.
    #[test]
    fn export_is_in_the_footer_on_every_tab() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.library = Some(test_app::library_of(2));
        app.library.as_mut().unwrap().selection.select_indices(&[0]);
        for tab in ["Filename", "Metadata", "Watermark", "Output"] {
            test_ui::click(&ctx, tab, |_| super::settings(&mut app, &ctx));
            let drawn: Vec<String> =
                test_ui::labelled_centres(&ctx, Vec::new(), |_| super::settings(&mut app, &ctx))
                    .into_iter()
                    .map(|(l, _)| l)
                    .collect();
            assert!(
                drawn.iter().any(|l| l == "Export"),
                "Export missing on {tab}: {drawn:?}"
            );
            assert!(
                drawn.iter().any(|l| l == "Cancel"),
                "Cancel missing on {tab}"
            );
        }
    }

    #[test]
    fn a_tabs_hover_says_whether_its_section_is_doing_something() {
        assert_eq!(super::tab_hover("Metadata", true), "Metadata is on");
        assert_eq!(super::tab_hover("Watermark", false), "Watermark is off");
    }
}
