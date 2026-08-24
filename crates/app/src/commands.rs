//! The one place a [`Command`] is performed.
//!
//! ⚠ Three input sources feed this — menu clicks, key presses and the rail — and each has a
//! different reason to exist. Routing them all through one exhaustive `match` is what stops a
//! menu item and its shortcut drifting apart, which is exactly what happened with the old Copy
//! button and Ctrl+C before they were made to share a code path.

use crate::{modals::Modal, AppView, RustroomApp};
use edit_model::EditGroups;
use eframe::egui;
use rustroom_app::menu::Command;

pub fn execute(app: &mut RustroomApp, ctx: &egui::Context, cmd: Command) {
    match cmd {
        Command::OpenFile => app.open_dialog(ctx),
        Command::OpenFolder => app.open_folder(ctx),
        Command::Export => {
            if !app.export_jobs().is_empty() {
                // Reset the working copy from the committed value, so a previously cancelled
                // edit does not linger.
                app.export_draft = app.settings.export.clone();
                app.modal = Modal::ExportSettings;
            }
        }
        Command::Settings => app.modal = Modal::Settings,
        Command::Undo => app.undo_action(),
        Command::Redo => app.redo_action(),
        Command::CopySettings => app.copy_settings(ctx),
        Command::PasteSettings => app.paste_last_groups(),
        Command::PasteSettingsWith => {
            if app.clipboard.is_some() {
                app.modal = Modal::PasteSettings;
            }
        }
        Command::SyncSelection => {
            let selected = app.library.as_ref().map_or(0, |l| l.selection.len());
            if selected > 1 {
                app.modal = Modal::SyncSettings;
            }
        }
        Command::ResetAdjustments => app.reset_adjustments(),
        Command::ShowLibrary => app.view = AppView::Library,
        Command::ShowDevelop => app.view = AppView::Develop,
        Command::FitView => {
            if let Some(s) = &mut app.session {
                s.zoom = 1.0;
                s.pan = egui::Vec2::ZERO;
            }
        }
        Command::ToggleBeforeAfter => {
            if let Some(s) = &mut app.session {
                s.show_before = !s.show_before;
            }
        }
        Command::ToggleFilmstrip => app.show_filmstrip = !app.show_filmstrip,
        Command::ToggleMaskOverlay => app.show_mask_tint = !app.show_mask_tint,
        // ⚠ Delegates rather than setting `app.tool` here: `select_tool` owns the re-render
        // decision, and duplicating it in this arm is how the rail and the menu drift apart.
        Command::SelectTool(t) => app.select_tool(t),
        Command::RotateLeft => app.rotate(false),
        Command::RotateRight => app.rotate(true),
        Command::SavePreset => {
            app.modal = Modal::SavePreset {
                name: String::new(),
                groups: EditGroups::default(),
            }
        }
        Command::KeyboardShortcuts => app.modal = Modal::KeyboardShortcuts,
        Command::About => app.modal = Modal::About,
    }
    // ⚠ No catch-all arm above. A new `Command` must fail to compile here rather than becoming
    // a menu item that silently does nothing — that is the whole reason this enum exists.
}
