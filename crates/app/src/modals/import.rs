//! The Import dialog (M9.12): the source and its photos on the left, where and how on the right.
//!
//! ⚠ The export dialog's shape — everything that needs `&mut app` is recorded in a local during the
//! frame and done after `dialog` returns, because `dialog` holds `app.settings` mutably.

use super::{dialog, Footer};
use crate::import_flow::Tick;
use crate::theme;
use crate::RustroomApp;
use eframe::egui;
use rustroom_app::filename_template::{token_help, Template};
use rustroom_app::ingest::options::Layout;
use std::path::PathBuf;

/// The source column's width; the settings column takes the rest.
const GRID_W: f32 = 520.0;
/// A thumbnail cell's side.
const CELL: f32 = 96.0;

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Some(st) = app.import.as_mut() else {
        app.modal = super::Modal::None;
        return;
    };
    let mut new_source: Option<PathBuf> = None;
    let mut pick_source = false;
    let mut pick_dest = false;
    let mut pick_backup = false;
    let importing = st.importing();
    let ticked = st.ticked();
    let blocker = st.draft.blocker(st.source.as_deref(), ticked);
    let title = match st.items.len() {
        0 => "Import".to_string(),
        n => format!("Import — {n} photos found"),
    };
    let primary = if ticked == 1 {
        "Import 1 photo".to_string()
    } else {
        format!("Import {ticked} photos")
    };
    let note = if importing {
        st.progress.clone()
    } else {
        blocker.unwrap_or("").to_string()
    };
    let dismiss = if importing { "Cancel import" } else { "Cancel" };

    let out = dialog(
        ctx,
        &mut app.settings,
        "import",
        &title,
        theme::DialogSize::Wide,
        Footer::confirm(&primary, !importing && blocker.is_none(), dismiss).note(&note),
        |ui| {
            ui.add_enabled_ui(!importing, |ui| {
                ui.horizontal_top(|ui| {
                    // ---- the source and its photos ----
                    ui.allocate_ui_with_layout(
                        egui::vec2(GRID_W, ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            theme::sub_label(ui, "From");
                            ui.horizontal_wrapped(|ui| {
                                for c in &st.cards {
                                    let on = st.source.as_ref() == Some(&c.root);
                                    if ui
                                        .selectable_label(on, &c.name)
                                        .on_hover_text(c.root.display().to_string())
                                        .clicked()
                                        && !on
                                    {
                                        new_source = Some(c.root.clone());
                                    }
                                }
                                if ui.button("Choose folder…").clicked() {
                                    pick_source = true;
                                }
                            });
                            if st.cards.is_empty() {
                                theme::hint(
                                    ui,
                                    "No card found — a card is a drive with a DCIM folder. \
                                     Choose any folder instead.",
                                );
                            }
                            if let Some(src) = &st.source {
                                theme::hint(ui, &src.display().to_string());
                            }
                            ui.horizontal(|ui| {
                                if ui.small_button("All").clicked() {
                                    st.tick(Tick::All);
                                }
                                if ui.small_button("None").clicked() {
                                    st.tick(Tick::None);
                                }
                                if ui
                                    .small_button("New only")
                                    .on_hover_text("Untick the photos already in the catalog")
                                    .clicked()
                                {
                                    st.tick(Tick::New);
                                }
                                let gb = st.ticked_bytes() as f64 / 1e9;
                                theme::hint(
                                    ui,
                                    &format!("{ticked} of {} ticked · {gb:.1} GB", st.items.len()),
                                );
                            });
                            grid(ui, st);
                        },
                    );
                    ui.separator();
                    // ---- where and how ----
                    ui.vertical(|ui| {
                        let d = &mut st.draft;
                        theme::sub_label(ui, "To");
                        ui.horizontal(|ui| {
                            if ui.button("Choose…").clicked() {
                                pick_dest = true;
                            }
                            theme::hint(
                                ui,
                                &if d.destination.as_os_str().is_empty() {
                                    "No destination chosen".to_string()
                                } else {
                                    d.destination.display().to_string()
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Organise");
                            egui::ComboBox::from_id_salt("import_layout")
                                .selected_text(d.layout.label())
                                .show_ui(ui, |ui| {
                                    for l in Layout::ALL {
                                        ui.selectable_value(&mut d.layout, l, l.label());
                                    }
                                });
                        });
                        if d.layout == Layout::OneFolder {
                            ui.horizontal(|ui| {
                                ui.label("Subfolder");
                                ui.add(
                                    egui::TextEdit::singleline(&mut d.subfolder)
                                        .hint_text("none — straight into the destination"),
                                );
                            });
                        }
                        ui.add_space(6.0);
                        theme::sub_label(ui, "File names");
                        ui.add(
                            egui::TextEdit::singleline(&mut d.template)
                                .desired_width(f32::INFINITY),
                        );
                        let tokens: Vec<&str> = token_help().iter().map(|(t, _)| *t).collect();
                        theme::hint(ui, &tokens.join("  "));
                        if let Err(e) = Template::parse(&d.template) {
                            theme::error_hint(ui, &e.to_string());
                        }
                        ui.add_space(6.0);
                        theme::sub_label(ui, "Backup");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut d.backup_on, "Also copy to");
                            if ui.button("Choose…").clicked() {
                                pick_backup = true;
                            }
                        });
                        if !d.backup.as_os_str().is_empty() {
                            theme::hint(ui, &d.backup.display().to_string());
                        }
                        ui.add_space(6.0);
                        theme::sub_label(ui, "Metadata");
                        egui::Grid::new("import_meta")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("Copyright");
                                ui.text_edit_singleline(&mut d.copyright);
                                ui.end_row();
                                ui.label("Creator");
                                ui.text_edit_singleline(&mut d.creator);
                                ui.end_row();
                                ui.label("Keywords");
                                ui.text_edit_singleline(&mut d.keywords)
                                    .on_hover_text("Comma-separated; created if missing");
                                ui.end_row();
                            });
                        theme::hint(ui, "An empty field changes nothing.");
                        ui.add_space(6.0);
                        theme::sub_label(ui, "Develop preset");
                        let shown = d.preset.clone().unwrap_or_else(|| "None".into());
                        egui::ComboBox::from_id_salt("import_preset")
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut d.preset, None, "None");
                                for n in &st.preset_names {
                                    ui.selectable_value(&mut d.preset, Some(n.clone()), n);
                                }
                            });
                    });
                });
            });
            if let Some(p) = st.preview() {
                ui.add_space(4.0);
                theme::hint(ui, &format!("First file: {p}"));
            }
        },
    );

    if pick_source {
        new_source = rfd::FileDialog::new().pick_folder().or(new_source);
    }
    if let Some(dir) = new_source {
        app.set_import_source(dir);
    }
    for (want, backup) in [(pick_dest, false), (pick_backup, true)] {
        if !want {
            continue;
        }
        if let (Some(dir), Some(st)) = (rfd::FileDialog::new().pick_folder(), app.import.as_mut()) {
            if backup {
                st.draft.backup = dir;
                st.draft.backup_on = true;
            } else {
                st.draft.destination = dir;
            }
        }
    }
    if out.primary {
        app.start_import();
    } else if out.dismissed {
        app.close_import();
    }
}

/// The photos as ticked thumbnails; an already-imported one dimmed, with the reason on hover.
fn grid(ui: &mut egui::Ui, st: &mut crate::import_flow::ImportState) {
    egui::ScrollArea::vertical()
        .id_salt("import_grid")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for it in &mut st.items {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(CELL, CELL), egui::Sense::click());
                    let dim = if it.ticked { 1.0 } else { 0.35 };
                    match &it.thumb {
                        Some(t) => {
                            let size = t.size_vec2();
                            let s = (CELL - 6.0) / size.x.max(size.y);
                            let r = egui::Rect::from_center_size(rect.center(), size * s);
                            ui.painter().image(
                                t.id(),
                                r,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE.gamma_multiply(dim),
                            );
                        }
                        None => {
                            ui.painter().rect_filled(
                                rect.shrink(3.0),
                                4.0,
                                ui.visuals().faint_bg_color,
                            );
                        }
                    }
                    // ⚠ Painted, not a ☑/☐ character: the bundled fonts' cmap is the authority,
                    // and `icons.rs` records five features that shipped grey squares by trusting a
                    // glyph instead.
                    tick_box(ui, rect.left_top() + egui::vec2(5.0, 5.0), it.ticked);
                    let name = it
                        .cand
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let hover = if it.known {
                        format!("{name}\nAlready in the catalog — same size and capture time")
                    } else {
                        name
                    };
                    if resp.on_hover_text(hover).clicked() {
                        it.ticked = !it.ticked;
                    }
                }
            });
        });
}

/// A 14 pt tick box, painted: an outlined square, filled with the accent and a check when on.
fn tick_box(ui: &egui::Ui, at: egui::Pos2, on: bool) {
    let r = egui::Rect::from_min_size(at, egui::vec2(14.0, 14.0));
    let v = ui.visuals();
    let p = ui.painter();
    p.rect_filled(
        r,
        3.0,
        if on {
            v.selection.bg_fill
        } else {
            v.extreme_bg_color
        },
    );
    p.rect_stroke(
        r,
        3.0,
        egui::Stroke::new(1.0, v.strong_text_color()),
        egui::StrokeKind::Inside,
    );
    if on {
        let s = egui::Stroke::new(2.0, v.selection.stroke.color);
        p.line_segment(
            [
                r.left_top() + egui::vec2(3.0, 7.0),
                r.left_top() + egui::vec2(6.0, 10.0),
            ],
            s,
        );
        p.line_segment(
            [
                r.left_top() + egui::vec2(6.0, 10.0),
                r.left_top() + egui::vec2(11.0, 4.0),
            ],
            s,
        );
    }
}
