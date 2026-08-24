//! The Develop view: toolbar, preset panel, adjustments panel, canvas.

pub mod canvas;
pub mod panel;
pub mod rail;

use crate::commands;
use crate::{filmstrip, theme, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::edit_clipboard;
use rustroom_app::menu::Command;

/// The Develop editor body: preset panel, adjustments panel, filmstrip, canvas.
///
/// ⚠ `photo_edited` is collected from both halves and recorded **once**, after they have both
/// run. Each half sets it inside a closure that already holds `&mut app` through `app.session`,
/// so neither can touch the action log itself — which is why they return a `bool` instead.
///
/// ⚠ **Panel order is load-bearing.** egui allocates side panels in call order and the central
/// panel takes what is left, so rail (right, outermost) → adjustments (right) → filmstrip
/// (bottom) → canvas (central) must be preserved exactly. The rail coming *first* is what puts
/// it against the window edge rather than between the panel and the image.
pub fn body(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    app.handle_develop_arrows(ctx);
    let mut photo_edited = false;
    // ⚠ The rail writes into a local, which is then applied through `select_tool` — the one
    // place that owns the re-render. The 1–4 keys and the Photo menu arrive through the same
    // method, so none of the three can switch tools without re-rendering.
    let mut picked = app.tool;
    if app.session.is_some() {
        rail::show(ui, &mut picked);
    }
    app.select_tool(picked);
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
    // ⚠ The ⌘Z / ⇧⌘Z handling has moved to `shell::pressed_command`, which reads them from
    // the menu model. These buttons only dispatch.
    if undo_click {
        commands::execute(app, ctx, Command::Undo);
    }
    if redo_click {
        commands::execute(app, ctx, Command::Redo);
    }
    if let Some(s) = &mut app.session {
        if ui.button("Fit").clicked() {
            s.zoom = 1.0;
            s.pan = egui::Vec2::ZERO;
        }
        ui.toggle_value(&mut s.show_before, "Before/After");
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
