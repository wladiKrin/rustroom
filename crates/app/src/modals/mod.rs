//! The modal dialogs.
//!
//! Each dialog is a free function taking `&mut RustroomApp`. That is deliberately more
//! explicit than an `impl RustroomApp` block in another file, which would have been a pure
//! cut-and-paste with zero call-site churn — and would have kept every dialog silently able
//! to reach anything on `self`, teaching a reader nothing.
//!
//! ⚠ **No visibility changes were needed for this.** Rust privacy is "visible in the declaring
//! module *and its descendants*", and this module is a descendant of the crate root where
//! `RustroomApp` lives, so its private fields are reachable without a single `pub`.
//!
//! # The borrow idiom
//!
//! Every dialog follows the same shape, and it is load-bearing rather than incidental:
//!
//! ```ignore
//! let mut local = app.settings.field;   // copy out
//! let mut close = false;
//! let dismissed = dialog(ctx, "id", "Title", |ui| { /* mutate locals only */ });
//! close |= dismissed;
//! if local != app.settings.field { /* apply */ }
//! ```
//!
//! The closure cannot touch `app`, because `dialog` already holds it. Reaching for `app`
//! inside the closure has cost compile errors twice in this codebase's history.
//!
//! ⚠ **A dialog body runs every frame it is open.** Any side effect must therefore be guarded
//! by a change comparison — `if tier != app.settings.view_demosaic` — never fired
//! unconditionally. Without that guard, `apply_view_demosaic` would rebuild a decode worker
//! and start an 11-second decode sixty times a second.

pub mod edits;
pub mod export;
pub mod info;
pub mod presets;
pub mod rule_edit;
pub mod settings;

use crate::{theme, RustroomApp};
use edit_model::EditGroups;
use eframe::egui;

/// Show a centred modal with a heading and a separator. Returns whether the backdrop was
/// clicked, which every dialog treats as a dismissal.
///
/// ⚠ `egui::Modal`, not `egui::Window`: a Window opens at the top-left and is genuinely easy
/// to miss, while a Modal centres on the viewport, dims the backdrop and blocks clicks behind
/// it.
///
/// ⚠⚠ **This is the single place dialog sizing lives.** Fourteen dialogs each choosing their own
/// width is how they became inconsistent and cramped; a call site names a [`theme::DialogSize`]
/// and nothing else.
///
/// ⚠⚠ **The dialog stays centred as it grows**, because `Modal` anchors `Align2::CENTER_CENTER`.
/// The corner handle therefore drifts from the pointer at half rate. That is inherent to a centred
/// modal and cannot be fixed without giving up the centring, which was the deliberate choice over
/// `egui::Window`.
///
/// ⚠ The size is measured as the content `Ui`'s available size on its **first line** — the
/// `Resize`'s own `content_ui`, whose extent is the same quantity `default_size` sets. Feeding
/// back the same quantity is what makes the round trip unable to drift. It is measured rather than
/// read from `egui::Resize::State`, whose `desired_size` is `pub(crate)`.
pub fn dialog<R>(
    ctx: &egui::Context,
    settings: &mut rustroom_app::settings::Settings,
    id: &str,
    title: &str,
    size: theme::DialogSize,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> bool {
    let min = size.min();
    let seed = settings.dialog_size(id, min, size.default_size());
    // ⚠ Bounded by the live viewport, so a size chosen on a large monitor is clamped rather than
    // stranding the dialog off-screen on a small one.
    // ⚠ `viewport_rect`, not `screen_rect` — renamed in egui 0.35.
    let vp = ctx.viewport_rect().size();
    let max = (vp.x * 0.8, vp.y * 0.8);
    let mut measured = seed;

    let backdrop = egui::Modal::new(egui::Id::new(id))
        .frame(theme::dialog_frame(ctx))
        .show(ctx, |ui| {
            egui::Resize::default()
                .id_salt(id)
                .default_size(seed)
                .min_size(min)
                .max_size(max)
                .resizable(true)
                .show(ui, |ui| {
                    measured = (ui.available_width(), ui.available_height());
                    ui.heading(title);
                    ui.separator();
                    // ⚠ The content scrolls, so a long rule or preset list can never push the
                    // dialog past the bottom of the screen. ⚠ The buttons scroll **with** it:
                    // `dialog` does not own the button row, and pinning it means restructuring
                    // every call site. They stay reachable, which is the bug being fixed.
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            content(ui);
                        });
                });
        })
        .backdrop_response
        .clicked();

    settings.remember_dialog_size(id, measured);
    backdrop
}

/// Which modal dialog is open. At most one at a time, so this is an enum rather than a
/// pile of booleans that could contradict each other.
#[derive(Default, PartialEq)]
pub enum Modal {
    #[default]
    None,
    /// Naming a new preset and choosing the groups it will carry.
    SavePreset {
        name: String,
        groups: EditGroups,
    },
    /// Choosing which groups a paste applies.
    PasteSettings,
    /// Choosing which groups a sync applies to the rest of the selection.
    SyncSettings,
    /// Choosing where and how to export the selection.
    ExportSettings,
    /// Naming a new export preset. Returns to `ExportSettings` when closed, so the export
    /// dialog does not vanish underneath the user.
    SaveExportPreset {
        name: String,
    },
    /// Confirming an export-preset deletion. `dont_ask_again` is scratch state, committed to
    /// `Settings` only on the Delete path. Returns to `ExportSettings` when closed.
    ConfirmDeleteExportPreset {
        name: String,
        dont_ask_again: bool,
    },
    /// Editing a smart collection's rule.
    ///
    /// ⚠ The form and the dropdown lists are loaded **once** when the dialog opens: rebuilding
    /// them per frame would discard every keystroke and run six catalog queries at 60 Hz.
    EditRule {
        id: i64,
        name: String,
        form: rustroom_app::rule_form::Form,
        lists: rule_edit::Lists,
        /// The stored rule uses nesting the flat form cannot show, so it is shown read-only.
        unsupported: bool,
    },
    /// Everything known about one photo.
    ///
    /// ⚠ `dims` is filled on the **first frame** and cached here, because reading them costs a
    /// whole-file read (~15 ms for a RAW) and a dialog body runs every frame.
    PhotoInfo {
        id: crate::PhotoId,
        dims: Option<(u32, u32)>,
    },
    /// Confirming that a folder is removed from the catalog.
    ///
    /// ⚠ **No `dont_ask_again`, deliberately.** A deleted preset goes to a 20-entry trash and is
    /// restorable by Ctrl+Z; this has neither. Suppressing a guard on the *irrecoverable* action
    /// is exactly the distinction `the_two_delete_guards_are_independent` exists to protect.
    ConfirmForgetFolder {
        path: std::path::PathBuf,
        id: i64,
        count: i64,
    },
    /// Confirming a preset deletion. `dont_ask_again` is scratch state — it is committed to
    /// `Settings` only on the Delete path, never on Cancel.
    ConfirmDeletePreset {
        name: String,
        dont_ask_again: bool,
    },
    Settings,
    /// The shortcut reference, generated from the menu model.
    KeyboardShortcuts,
    /// The about box.
    About,
}

/// Draw whichever dialog is open.
///
/// ⚠ The local `Which` enum is not ceremony: matching directly on `app.modal` keeps the
/// scrutinee borrowed across the `&mut app` call in each arm, which does not compile. Decide
/// *which* dialog into a plain value first, then dispatch.
/// Draw whichever modal is open. Called once per frame from `ui`.
pub fn draw(app: &mut RustroomApp, ctx: &egui::Context) {
    // ⚠⚠ **The one place a dialog's size is written to disk.** `dialog` keeps `Settings` current
    // in memory every frame, which is cheap; the *file* is written only here, on the frame a
    // dialog closes. A resize runs at 60 Hz and the settings file is not a place to write sixty
    // times a second — the `apply_view_demosaic` rule.
    //
    // ⚠ Detected centrally rather than in each dialog's close path: there are fourteen of those —
    // a button, the backdrop, Escape — and every one would have to remember. This catches all of
    // them, including any dialog added later.
    //
    // ⚠ There is deliberately **no headless test for this hook**, and a fake one would be worse
    // than none: it needs a live `egui::Context`, a real modal frame and a real settings file.
    // It is verified by hand — resize, close, reopen, and again after a restart.
    let was_open = !matches!(app.modal, Modal::None) || app.export_collision.is_some();

    // Decide *which* modal first, then dispatch. Matching on `app.modal` directly and
    // calling a `&mut self` method from an arm keeps the scrutinee's borrow alive across
    // the call, which does not compile.
    enum Which {
        None,
        Save,
        Paste,
        Sync,
        Export,
        SaveExport,
        ConfirmDeleteExport,
        ConfirmDelete,
        ConfirmForget,
        PhotoInfo,
        EditRule,
        Settings,
        Shortcuts,
        About,
    }
    let which = match &app.modal {
        Modal::None => Which::None,
        Modal::SavePreset { .. } => Which::Save,
        Modal::PasteSettings => Which::Paste,
        Modal::SyncSettings => Which::Sync,
        Modal::ExportSettings => Which::Export,
        Modal::SaveExportPreset { .. } => Which::SaveExport,
        Modal::ConfirmDeleteExportPreset { .. } => Which::ConfirmDeleteExport,
        Modal::ConfirmDeletePreset { .. } => Which::ConfirmDelete,
        Modal::ConfirmForgetFolder { .. } => Which::ConfirmForget,
        Modal::PhotoInfo { .. } => Which::PhotoInfo,
        Modal::EditRule { .. } => Which::EditRule,
        Modal::Settings => Which::Settings,
        Modal::KeyboardShortcuts => Which::Shortcuts,
        Modal::About => Which::About,
    };
    // Independent of `app.modal`: a collision arrives while no dialog is open.
    if app.export_collision.is_some() {
        export::collision(app, ctx);
    }
    match which {
        Which::None => {}
        Which::Save => presets::save(app, ctx),
        Which::Paste => edits::paste(app, ctx),
        Which::Sync => edits::sync(app, ctx),
        Which::Export => export::settings(app, ctx),
        Which::SaveExport => export::save_preset(app, ctx),
        Which::ConfirmDeleteExport => export::confirm_delete(app, ctx),
        Which::ConfirmDelete => presets::confirm_delete(app, ctx),
        Which::ConfirmForget => confirm_forget_folder(app, ctx),
        Which::PhotoInfo => info::show(app, ctx),
        Which::EditRule => rule_edit::show(app, ctx),
        Which::Settings => settings::show(app, ctx),
        Which::Shortcuts => shortcuts(app, ctx),
        Which::About => about(app, ctx),
    }

    let still_open = !matches!(app.modal, Modal::None) || app.export_collision.is_some();
    if was_open && !still_open {
        app.save_settings();
    }
}

/// Confirming that a folder is removed from the catalog.
///
/// ⚠⚠ **No "don't ask again".** A deleted preset goes to a 20-entry trash and Ctrl+Z restores it;
/// this has neither, so the guard on the irrecoverable action is never suppressible.
fn confirm_forget_folder(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::ConfirmForgetFolder { path, id, count } = &app.modal else {
        return;
    };
    let (path, id, count) = (path.clone(), *id, *count);
    let mut confirmed = false;
    let mut cancelled = false;

    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "confirm_forget_folder",
        "Forget folder",
        theme::DialogSize::Compact,
        |ui| {
            ui.label(format!("Forget “{}”?", path.display()));
            crate::theme::hint(
                ui,
                &format!(
                "Removes {count} photos from the catalog, with their ratings, flags, labels and \
                 keywords. No files are deleted and no edits are lost — re-opening the folder \
                 catalogues it again."
            ),
            );
            ui.separator();
            let (yes, no) = crate::theme::button_row(ui, "Forget", true, "Cancel");
            confirmed = yes;
            cancelled = no;
        },
    );

    // Clicking outside is a cancel, as in every other confirmation here.
    if confirmed {
        app.forget_folder(ctx, id, &path);
        app.modal = Modal::None;
    } else if cancelled || dismissed {
        app.modal = Modal::None;
    }
}

/// The keyboard reference.
///
/// ⚠ **Generated from `menu::model()`, never hand-written.** A shortcuts dialog listing a
/// chord the app does not have is worse than no dialog at all, and a hand-written one drifts
/// the first time a shortcut moves.
fn shortcuts(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut close = false;
    // ⚠ Through the shared funnel like every other dialog. It built its own `egui::Modal` before,
    // which meant it missed the frame padding and the sizing entirely — and it is one of the
    // dialogs that most wants to be Wide and scrollable.
    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "shortcuts",
        "Keyboard shortcuts",
        theme::DialogSize::Wide,
        |ui| {
            // ⚠ The inner `ScrollArea` is gone: `dialog` provides one, and nesting two vertical
            // scroll areas makes them fight over the wheel.
            for section in rustroom_app::menu::model() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(section.title).strong());
                for item in section.items {
                    if let rustroom_app::menu::Item::Action {
                        label,
                        shortcut: Some(sc),
                        ..
                    } = item
                    {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            theme::hint(ui, &sc.label());
                        });
                    }
                }
            }
            ui.separator();
            if ui.button("Close").clicked() {
                close = true;
            }
        },
    );
    if close || dismissed {
        app.modal = Modal::None;
    }
}

/// The about box.
fn about(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut close = false;
    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "about",
        "Rustroom",
        // ⚠ `Compact` despite being a "form-ish" dialog: it is two lines and a Close button, and
        // `Standard` would be mostly empty.
        theme::DialogSize::Compact,
        |ui| {
            theme::hint(ui, "An open-source RAW photo editor.");
            ui.separator();
            if ui.button("Close").clicked() {
                close = true;
            }
        },
    );
    if close || dismissed {
        app.modal = Modal::None;
    }
}

#[cfg(test)]
mod tests {
    use crate::theme::DialogSize;

    /// Every dialog id in the app, with the class it uses.
    ///
    /// ⚠ **A documentation test, and it is worth saying so rather than overselling it.** It pins
    /// the roster and that the ids are unique — it does **not** verify the wiring, which is
    /// appearance and is checked by hand with the app running. What it does catch is a dialog
    /// added with a duplicate id, which would make two dialogs share a persisted size and an
    /// `egui::Resize` state.
    #[test]
    fn every_dialog_has_a_class_and_the_ids_are_unique() {
        let dialogs: [(&str, DialogSize); 14] = [
            ("save_preset", DialogSize::Standard),
            ("confirm_delete", DialogSize::Compact),
            ("paste_settings", DialogSize::Standard),
            ("sync_settings", DialogSize::Standard),
            ("export_settings", DialogSize::Standard),
            ("save_export_preset", DialogSize::Standard),
            ("confirm_delete_export", DialogSize::Compact),
            ("export_collision", DialogSize::Standard),
            ("confirm_forget_folder", DialogSize::Compact),
            ("photo_info", DialogSize::Wide),
            ("edit_rule", DialogSize::Wide),
            ("settings", DialogSize::Standard),
            ("shortcuts", DialogSize::Wide),
            ("about", DialogSize::Compact),
        ];
        let mut ids: Vec<&str> = dialogs.iter().map(|(i, _)| *i).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "every dialog id must be unique");
    }
}
