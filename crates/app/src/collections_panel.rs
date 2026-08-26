//! The Library's collections list.
//!
//! ⚠ A **left** panel, in the Library view only — Lightroom's placement, and it competes with
//! nothing. The Presets panel was removed for having two homes; this has one.

use crate::RustroomApp;
use eframe::egui;

/// Draw the collections panel. Every action is applied here.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ⚠ Collected and applied after the list closes, the shape every panel in this app uses:
    // egui borrows `app` through the closure, so a click cannot act immediately.
    enum Action {
        None,
        Open(i64, String),
        Delete(i64),
        AddSelection(i64),
    }
    let mut action = Action::None;

    egui::Panel::left("collections")
        .default_size(190.0)
        .show(ui, |ui| {
            ui.heading("Collections");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut app.new_collection)
                        .hint_text("New collection")
                        .desired_width(110.0),
                );
                // ⚠ Guarded on a non-empty name: the store keys on the name, and a nameless
                // collection cannot be told from another nameless one.
                let named = !app.new_collection.trim().is_empty();
                if ui.add_enabled(named, egui::Button::new("+")).clicked() {
                    if let Some(cat) = &app.catalog {
                        let _ = cat.create_collection(app.new_collection.trim());
                    }
                    app.new_collection.clear();
                }
            });
            ui.separator();

            let rows = app
                .catalog
                .as_ref()
                .and_then(|c| c.collections().ok())
                .unwrap_or_default();
            if rows.is_empty() {
                crate::theme::hint(ui, "No collections yet.");
            }
            let selected = app.selected_collection();
            let picked = app.library.as_ref().map(|l| l.selection.len()).unwrap_or(0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for c in &rows {
                    ui.horizontal(|ui| {
                        if ui.button("×").on_hover_text("Delete").clicked() {
                            action = Action::Delete(c.id);
                        }
                        // ⚠ Enabled only with a selection, and the hover names the count, so it
                        // can never be ambiguous about what it is about to do.
                        if ui
                            .add_enabled(picked > 0, egui::Button::new("+"))
                            .on_hover_text(format!("Add {picked} selected"))
                            .clicked()
                        {
                            action = Action::AddSelection(c.id);
                        }
                        let label = format!("{} ({})", c.name, c.count);
                        if ui.selectable_label(selected == Some(c.id), label).clicked() {
                            action = Action::Open(c.id, c.name.clone());
                        }
                    });
                }
            });
        });

    match action {
        Action::None => {}
        Action::Open(id, name) => app.open_collection(ctx, id, &name),
        Action::Delete(id) => {
            if let Some(cat) = &app.catalog {
                let _ = cat.delete_collection(id);
            }
            // ⚠ If the deleted collection was the one on screen, there is nothing left to show.
            if app.selected_collection() == Some(id) {
                app.library = None;
                app.status = "Collection deleted".into();
            }
        }
        Action::AddSelection(id) => {
            let chosen: Vec<_> = app
                .library
                .as_ref()
                .map(|l| {
                    l.selection
                        .iter()
                        .filter_map(|i| l.entries.get(i))
                        .map(|e| e.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            let mut n = 0;
            if let Some(cat) = &app.catalog {
                for pid in &chosen {
                    if cat.add_to_collection(id, &pid.path, pid.copy).is_ok() {
                        n += 1;
                    }
                }
            }
            app.status = format!("Added {n} to the collection");
        }
    }
}
