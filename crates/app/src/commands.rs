//! The one place a [`Command`] is performed.
//!
//! ⚠ Three input sources feed this — menu clicks, key presses and the rail — and each has a
//! different reason to exist. Routing them all through one exhaustive `match` is what stops a
//! menu item and its shortcut drifting apart, which is exactly what happened with the old Copy
//! button and Ctrl+C before they were made to share a code path.

use crate::{modals::Modal, AppView, RustroomApp, PREVIEW_MAX_DIM};
use edit_model::EditGroups;
use eframe::egui;
use rustroom_app::menu::Command;
use rustroom_app::view_region;

/// One click of Zoom in / Zoom out.
///
/// ⚠ A ratio, not an increment: zoom is multiplicative, so a fixed step would be enormous at
/// 0.2x and imperceptible at 8x.
const ZOOM_STEP: f32 = 1.25;

fn zoom_by(app: &mut RustroomApp, factor: f32) {
    if let Some(s) = &mut app.session {
        s.zoom = view_region::zoom_step(s.zoom, factor);
    }
}

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
        Command::CreateVirtualCopy => app.create_virtual_copy(ctx),
        Command::DeleteVirtualCopy => app.delete_virtual_copy(ctx),
        Command::Undo => app.undo_action(ctx),
        Command::Redo => app.redo_action(ctx),
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
        Command::ZoomIn => zoom_by(app, ZOOM_STEP),
        Command::ZoomOut => zoom_by(app, 1.0 / ZOOM_STEP),
        Command::FitView => {
            if let Some(s) = &mut app.session {
                s.zoom = 1.0;
                s.pan = egui::Vec2::ZERO;
            }
        }
        Command::ZoomActual => {
            if let Some(s) = &mut app.session {
                // ⚠ 1:1 against the **editing buffer**, which is what this app's "native" rung
                // has always meant. `fit_image_size` is measured at the fit cap, so the buffer
                // is `buffer_long / fit_cap` times larger; dividing by the canvas's fit scale
                // turns that into a zoom factor.
                let fit_cap = view_region::fit_cap(s.buffer_long, PREVIEW_MAX_DIM) as f32;
                if fit_cap > 0.0 && s.fit_scale > 0.0 {
                    let target = (s.buffer_long as f32 / fit_cap) / s.fit_scale;
                    // ⚠ Through `zoom_step` from 1.0 rather than a separate clamp helper, so
                    // the MIN/MAX rule lives in exactly one place.
                    s.zoom = view_region::zoom_step(1.0, target);
                    s.pan = egui::Vec2::ZERO;
                }
            }
        }
        Command::ToggleBeforeAfter => {
            // ⚠ Must re-render: before/after now changes *what is rendered*, not which of two
            // textures is shown. Without this the toggle would appear to do nothing until some
            // other edit happened to request a frame — the M3.1a `draw_mask_overlay` bug.
            let tool = app.tool;
            if let Some(s) = &mut app.session {
                s.show_before = !s.show_before;
                s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
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
