//! The Develop view: toolbar, preset panel, adjustments panel, canvas.

pub mod canvas;
pub mod panel;
pub mod rail;

use crate::commands;
use crate::{filmstrip, theme, RustroomApp};
use eframe::egui;
use rustroom_app::action_log::ActionKind;
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
    if app.session.is_some() {
        if ui.button("Fit").clicked() {
            commands::execute(app, ctx, Command::FitView);
        }
        // ⚠ `toggle_value` needs the live flag, so this reads it and dispatches on a change
        // rather than writing it directly — the menu item and the button must not be two
        // implementations of the same toggle.
        let mut before_after = app.session.as_ref().is_some_and(|s| s.show_before);
        if ui.toggle_value(&mut before_after, "Before/After").clicked() {
            commands::execute(app, ctx, Command::ToggleBeforeAfter);
        }
        // ⚠ The filename, right-aligned. The row that used to hold fourteen controls now
        // carries the one piece of context worth always seeing.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(s) = &app.session {
                if let Some(name) = s.path.file_name() {
                    theme::caption(ui, &name.to_string_lossy());
                }
            }
        });
    }
}
