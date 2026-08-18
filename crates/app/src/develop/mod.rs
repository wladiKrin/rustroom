//! The Develop view: toolbar, preset panel, adjustments panel, canvas.

pub mod canvas;
pub mod panel;

use crate::{filmstrip, preview_edits, theme, Modal, RustroomApp};
use edit_model::EditGroups;
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::edit_clipboard;

/// The Develop editor body: preset panel, adjustments panel, filmstrip, canvas.
///
/// ⚠ `photo_edited` is collected from both halves and recorded **once**, after they have both
/// run. Each half sets it inside a closure that already holds `&mut app` through `app.session`,
/// so neither can touch the action log itself — which is why they return a `bool` instead.
///
/// ⚠ **Panel order is load-bearing.** egui allocates side panels in call order and the central
/// panel takes what is left, so preset (left) → adjustments (right) → filmstrip (bottom) →
/// canvas (central) must be preserved exactly.
pub fn body(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    app.handle_develop_arrows(ctx);
    let mut photo_edited = false;
    if app.show_presets && app.session.is_some() {
        preset_panel(app, ui);
    }
    photo_edited |= panel::show(app, ui);
    if app.show_filmstrip && app.library.is_some() {
        filmstrip::show(app, ui, ctx);
    }
    photo_edited |= canvas::show(app, ui, ctx);

    // Recorded in the same order as everything else, so "delete a preset, move a slider,
    // Ctrl+Z twice" undoes the slider and then restores the preset.
    if photo_edited {
        if let Some(path) = app.open_photo() {
            // ⚠ `Some(path.as_path())`, not `Some(&path)`: the parameter is `Option<&Path>`,
            // and deref coercion does not reach inside `Some`.
            app.actions.push(
                ActionKind::PhotoEdit { path: path.clone() },
                Some(path.as_path()),
            );
        }
    }
}

/// Develop-mode toolbar buttons (shown in the top panel when a photo is open).
pub fn toolbar(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // Undo/Redo drive the unified `ActionLog`, so one control walks back through photo
    // edits and preset deletions in the order they actually happened. The label says
    // which, so "Undo" never silently does the surprising one.
    let open = app.open_photo();
    let undo_label = match app.actions.peek_undo(open.as_deref()) {
        Some(ActionKind::PresetDelete { name }) => format!("Undo delete “{name}”"),
        _ => "Undo".to_string(),
    };
    let undo_click = ui
        .add_enabled(
            app.actions.can_undo(open.as_deref()),
            egui::Button::new(undo_label),
        )
        .clicked();
    let redo_click = ui
        .add_enabled(
            app.actions.can_redo(open.as_deref()),
            egui::Button::new("Redo"),
        )
        .clicked();
    let (kundo, kredo) = ctx.input(|i| {
        let z = i.key_pressed(egui::Key::Z) && i.modifiers.command;
        (z && !i.modifiers.shift, z && i.modifiers.shift)
    });
    if undo_click || kundo {
        app.undo_action();
    }
    if redo_click || kredo {
        app.redo_action();
    }
    if let Some(s) = &mut app.session {
        if ui.button("Fit").clicked() {
            s.zoom = 1.0;
            s.pan = egui::Vec2::ZERO;
        }
        ui.toggle_value(&mut s.show_before, "Before/After");
        if ui.toggle_value(&mut s.geometry_mode, "Geometry").clicked() {
            s.request(
                preview_edits(&s.edits, s.geometry_mode),
                s.render_max_dim(),
                s.requested_region,
            );
        }
    }
    if app.session.is_some() {
        ui.separator();
        if ui.button("Copy").clicked() {
            // Same borrow reason as in `handle_clipboard_keys`, and the same system
            // clipboard write, so the button and the shortcut cannot disagree.
            app.clipboard = app.session.as_ref().map(|s| s.edits.clone());
            if let Some(edits) = &app.clipboard {
                ui.ctx().copy_text(edit_clipboard::encode(edits));
            }
            app.status = "Settings copied".to_string();
        }
        if ui
            .add_enabled(app.clipboard.is_some(), egui::Button::new("Paste…"))
            .clicked()
        {
            app.modal = Modal::PasteSettings;
        }
        let selected = app.library.as_ref().map_or(0, |l| l.selection.len());
        if ui
            .add_enabled(
                selected > 1,
                egui::Button::new(format!("Sync {}…", selected.saturating_sub(1))),
            )
            .clicked()
        {
            app.modal = Modal::SyncSettings;
        }
        // One button for both: the count decides whether it exports the open photo or
        // the whole selection. There is no second code path — a single export is a
        // batch of one, through the same worker.
        let to_export = app.export_jobs().len();
        let label = if to_export > 1 {
            format!("Export {to_export}…")
        } else {
            "Export…".to_string()
        };
        if ui
            .add_enabled(to_export > 0, egui::Button::new(label))
            .clicked()
        {
            // Reset the working copy from the committed value, so a previously
            // cancelled edit does not linger.
            app.export_draft = app.settings.export.clone();
            app.modal = Modal::ExportSettings;
        }
        // Unlike the filmstrip, this does not need a Library: presets are just as
        // useful on a single-file open.
        ui.toggle_value(&mut app.show_presets, "Presets");
        if ui.button("Settings…").clicked() {
            app.modal = Modal::Settings;
        }
        // ⚠ The binding is named from the setting through `PanDrag::label()` rather than
        // hardcoded as "middle-drag", which would become a lie the moment someone changed it.
        theme::hint(
            ui,
            &format!(
                "Scroll or {} to pan · pinch or Ctrl+scroll to zoom",
                app.settings.pan_drag.label()
            ),
        );
    }
    if app.library.is_some() {
        ui.toggle_value(&mut app.show_filmstrip, "Filmstrip");
    }
}

/// The left preset panel: save the current edits as a preset, apply one, or delete one.
pub fn preset_panel(app: &mut RustroomApp, ui: &mut egui::Ui) {
    // egui borrows `self` through the closure, so the click is recorded here and acted
    // on after the panel closes.
    enum Action {
        None,
        Apply(usize),
        Delete(String),
        Save,
        UndoDelete,
    }
    let mut action = Action::None;
    // Shown whenever the next undo would restore a preset, so the restore is
    // discoverable rather than only a keystroke. It is the same action the toolbar's
    // Undo performs — not a second, competing one.
    let open = app.open_photo();
    let undo_name = match app.actions.peek_undo(open.as_deref()) {
        Some(ActionKind::PresetDelete { name }) => Some(name.clone()),
        _ => None,
    };

    egui::Panel::left("presets")
        .default_size(180.0)
        .show(ui, |ui| {
            ui.heading("Presets");
            if ui.button("Save current as preset…").clicked() {
                action = Action::Save;
            }
            if let Some(name) = &undo_name {
                if ui.button(format!("↩ Undo delete “{name}”")).clicked() {
                    action = Action::UndoDelete;
                }
            }
            ui.separator();
            if app.preset_list.is_empty() {
                ui.label("No presets yet.");
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, p) in app.preset_list.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.button("×").on_hover_text("Delete").clicked() {
                            action = Action::Delete(p.name.clone());
                        }
                        if ui.button(&p.name).clicked() {
                            action = Action::Apply(i);
                        }
                    });
                }
            });
        });

    match action {
        Action::None => {}
        Action::Save => {
            app.modal = Modal::SavePreset {
                name: String::new(),
                groups: EditGroups::default(),
            };
        }
        Action::Apply(i) => {
            if let Some(p) = app.preset_list.get(i).cloned() {
                // Applying a preset targets the selection, exactly as a paste does.
                let (n, failed) = app.apply_to_selection(&p.edits, p.groups);
                app.status = match (n, failed) {
                    (0, _) => "Nothing selected".to_string(),
                    (1, 0) => format!("Applied preset “{}”", p.name),
                    (n, 0) => format!("Applied “{}” to {n} photos", p.name),
                    (n, f) => format!("Applied “{}” to {n} photos, {f} failed", p.name),
                };
            }
        }
        Action::UndoDelete => app.undo_action(),
        Action::Delete(name) => {
            if app.settings.confirm_preset_delete {
                app.modal = Modal::ConfirmDeletePreset {
                    name,
                    dont_ask_again: false,
                };
            } else {
                app.delete_preset(&name);
            }
        }
    }
}
