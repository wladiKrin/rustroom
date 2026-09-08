//! The export flow'''s dialogs.
//!
//! Four in one file because they are one flow: `save_preset` and `confirm_delete` both
//! **return to** `Modal::ExportSettings` when closed, so the export dialog does not vanish
//! underneath the user.

use super::dialog;
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

/// The three sections that turn an export into a deliverable.
///
/// ⚠ Extracted from [`settings`] rather than written inline: that function had reached ~360
/// lines, and this repository's record is that every oversized file got there one
/// reasonable-seeming addition at a time. ⚠ It takes `&mut ExportOptions` and no `app`, which
/// is what lets it sit outside the dialog closure's borrow — the shape this file's own
/// comments say has cost a compile error here twice.
fn delivery_sections(ui: &mut egui::Ui, opts: &mut ExportOptions) {
    ui.separator();
    // ⚠⚠ **Collapsed by default, all three.** They are the sections most exports never
    // touch, and the dialog was reported as cramped once already; five always-open
    // sections is how it got that way the first time. ⚠ Each header says whether it is
    // *doing* something, so a collapsed section can never be hidden state — the failure
    // the crop aspect lock and the `Filter:` button were each fixed for.
    let on = |b: bool| if b { " ·" } else { "" };

    egui::CollapsingHeader::new(format!("Metadata{}", on(opts.metadata.is_active())))
        .id_salt("export_metadata")
        .show(ui, |ui| {
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
            // ⚠ The load-bearing sentence in this whole dialog. Rustroom's promise is
            // that it never writes to the negative, and a field marked "Copyright" is
            // exactly where someone would expect it to break that promise.
            theme::hint(
                ui,
                "Written into the exported file as XMP. The original is never modified. \
                 A photo's own title, caption, copyright and creator (Information dialog) win \
                 over the fields above; leave a field empty to write nothing for it. \
                 \"All metadata\" also writes the camera, exposure, keywords and rating.",
            );
        });

    egui::CollapsingHeader::new(format!("Watermark{}", on(opts.watermark.is_active())))
        .id_salt("export_watermark")
        .show(ui, |ui| {
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
                        // ⚠ Built from `WatermarkCorner::ALL`, never written out here,
                        // so a fifth corner cannot land in a list this dialog does not
                        // show — the rule `Tool::ALL` and `FlagSet::ALL` already follow.
                        .show_ui(ui, |ui| {
                            for c in WatermarkCorner::ALL {
                                ui.selectable_value(&mut opts.watermark.corner, c, c.label());
                            }
                        });
                    ui.end_row();
                });
            let (lo, hi) = Watermark::SIZE_RANGE;
            ui.add(
                egui::Slider::new(&mut opts.watermark.size, lo..=hi).text("Size (% of long edge)"),
            );
            ui.add(egui::Slider::new(&mut opts.watermark.opacity, 0..=100).text("Opacity"));
            // ⚠ Not decoration: the size is a *fraction*, which is the only thing that
            // makes one preset usable at 800 px and at 6000 — and it is exactly the
            // behaviour someone would otherwise report as the slider being ignored.
            theme::hint(
                ui,
                "Scales with the exported image, so one setting suits every output size. \
                 Skipped on a file too small to hold it.",
            );
        });

    egui::CollapsingHeader::new(format!(
        "Output sharpening{}",
        on(opts.output_sharpening.is_active())
    ))
    .id_salt("export_sharpening")
    .show(ui, |ui| {
        ui.add(egui::Slider::new(&mut opts.output_sharpening.amount, 0..=100).text("Amount"));
        // ⚠ The one thing a user cannot work out from the slider: this is *not* the
        // Detail panel's Sharpening, it stacks on top of it, and it exists because the
        // resize happens before the pipeline. Same reason the Demosaic hint above says
        // what it says.
        theme::hint(
            ui,
            "Applied to the finished file at a fixed radius, on top of the Detail \
             panel's sharpening — it puts back the edge a resize takes. Not shown in \
             the preview.",
        );
    });
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

            // ─── M6.1 sections ───────────────────────────────────────────────────────────
            //
            // ⚠⚠ Naming is here, between the destination and the output format, because *where
            // the file goes* and *what it is called* are one question. The four sections below
            // it are the seam M6.1b–e drop into; their `ExportOptions` fields already exist,
            // defaulted and inert, so each can be built without touching this file's neighbours
            // or the struct every one of them would otherwise contend for.
            //
            // ⚠ Deliberately **no inert widgets** for the one not yet built: a copyright field
            // that silently writes nothing is worse than an absent one, because it looks like a
            // feature. A section appears when it works.
            //
            //   M6.1e — Delivery:  opts.delivery.{subfolder, then}
            ui.horizontal(|ui| {
                ui.label("Filename");
                ui.text_edit_singleline(&mut opts.filename_template);
            });
            // ⚠ Always shown, and built from `Token::NAMES` rather than written out here: a
            // reference listing a token the parser does not have is worse than none, the rule
            // the Keyboard-shortcuts dialog already follows.
            egui::Grid::new("export_token_help")
                .num_columns(2)
                .spacing([10.0, 2.0])
                .show(ui, |ui| {
                    // ⚠ A grid rather than one comma-separated line: the tokens are not
                    // self-explanatory — `{copy}` in particular was reported as unclear — and a
                    // list of names without meanings is a reference only for whoever wrote it.
                    for (tok, what) in rustroom_app::filename_template::token_help() {
                        theme::hint(ui, tok);
                        theme::hint(ui, what);
                        ui.end_row();
                    }
                });
            match Template::parse(&opts.filename_template) {
                // ⚠ The preview is what makes the absent-value rule visible: a template naming a
                // token this photo has nothing for otherwise reads as "why did my dash vanish".
                // It is also where `-N` and `_N` are visibly *not* part of the template.
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
                // ⚠⚠ Reported, and Export goes dark while it stands. A typo must not export a
                // folder of files all called the same thing — which is what dropping an unknown
                // token silently would produce, collided into one by `-N`.
                Err(e) => theme::error_hint(ui, &e.to_string()),
            }
            // ⚠⚠ **Only these two.** `Demosaic` further down is also a labelled control, but it
            // is separated from these by the banding hint, the quality slider, the long-edge
            // checkbox and its slider — folding it in here would **reorder the dialog** and
            // detach the banding hint from the two controls it is about, whose placement is
            // deliberate (see its comment). Alignment is not worth silently rearranging a dialog
            // someone reasoned about.
            //
            // ⚠ The two sliders carry their labels inside the widget (`Slider::text`), so they
            // are not `label: control` rows and do not belong in a column either.
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
            delivery_sections(ui, &mut opts);

            ui.separator();
            // ⚠ Both halves, checked before the run rather than per photo — the reason
            // `destination_ready` exists, applied to the other thing knowable up front.
            let (confirmed, dismissed) =
                theme::button_row(ui, "Export", can_export(&opts), "Cancel");
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
    fn the_three_delivery_sections_are_offered() {
        // ⚠ A `CollapsingHeader` *is* an interactive widget, so unlike `theme::hint` it does
        // produce an AccessKit node and can be read back. That is the whole of what is testable
        // here — the fields inside it are only drawn once it is opened.
        let drawn = with_app(|_| {});
        for want in ["Metadata", "Watermark", "Output sharpening"] {
            assert!(
                drawn.iter().any(|l| l.starts_with(want)),
                "{want} section missing: {drawn:?}"
            );
        }
    }

    #[test]
    fn a_section_that_is_doing_something_says_so_in_its_header() {
        // ⚠⚠ **The guard against invisible state**, and the reason the marker exists at all:
        // these sections are collapsed by default, so a watermark set once and forgotten would
        // otherwise appear on every export with nothing on screen to say so. Same failure the
        // crop aspect lock and the `Filter:` button were each fixed for.
        //
        // ⚠ Both directions are asserted. A marker that is always on and one that is never on
        // are equally useless, and only the pair distinguishes them.
        let idle = with_app(|_| {});
        assert!(
            idle.iter().any(|l| l == "Metadata"),
            "an untouched section must carry no marker: {idle:?}"
        );

        let busy = with_app(|app| {
            app.export_draft.metadata.copyright = "(c) 2026".into();
            app.export_draft.watermark.text = "rustroom".into();
            app.export_draft.output_sharpening.amount = 40;
        });
        for want in ["Metadata", "Watermark", "Output sharpening"] {
            assert!(
                busy.iter()
                    .any(|l| l.starts_with(want) && l.len() > want.len()),
                "{want} is active and its header does not say so: {busy:?}"
            );
        }
    }

    #[test]
    fn a_narrowed_scope_marks_the_metadata_header() {
        // ⚠ `All` is the default and therefore not hidden state; `CopyrightOnly` is a choice
        // someone made and forgot, which is exactly what the marker exists for.
        let narrowed = with_app(|app| {
            app.export_draft.metadata.scope = export::MetadataScope::CopyrightOnly;
        });
        assert!(
            narrowed
                .iter()
                .any(|l| l.starts_with("Metadata") && l.len() > "Metadata".len()),
            "a changed scope must show in the header: {narrowed:?}"
        );
        let idle = with_app(|_| {});
        assert!(
            idle.iter().any(|l| l == "Metadata"),
            "All is the default and must carry no marker: {idle:?}"
        );
    }
}
