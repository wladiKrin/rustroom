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

use crate::develop::panel::section::{header, Header};
use crate::{theme, RustroomApp};
use edit_model::EditGroups;
use eframe::egui;

pub mod shell;
pub use shell::{dialog, Footer};

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
    /// An upload did not confirm, and the file is at `at`. Raised for the **first** one in a run.
    ///
    /// ⚠ A notice, not a question: there is nothing to decide, and the export is still running.
    UploadUnconfirmed {
        at: std::path::PathBuf,
        why: String,
    },
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
        /// The editable description, read from the catalog on the **first frame** and cached
        /// here like `dims`, then committed when the dialog closes — see `info::show`.
        description: Option<rustroom_app::description::DescriptionDraft>,
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
    /// Confirming a keyword deletion that would take descendants or assignments with it.
    ///
    /// ⚠ Opened **only** when it would; a bare leaf is deleted outright. See `DeleteKeyword`.
    ConfirmDeleteKeyword {
        id: i64,
        name: String,
        descendants: usize,
        assignments: usize,
    },
    /// Confirming a keyword merge — a rename or move whose name a sibling already has (M9.8).
    ConfirmMergeKeyword {
        from: i64,
        into: i64,
        from_name: String,
        into_name: String,
        photos: usize,
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
        ConfirmDeleteKeyword,
        ConfirmMergeKeyword,
        PhotoInfo,
        EditRule,
        Settings,
        Shortcuts,
        About,
        UploadUnconfirmed,
    }
    let which = match &app.modal {
        Modal::None => Which::None,
        Modal::SavePreset { .. } => Which::Save,
        Modal::PasteSettings => Which::Paste,
        Modal::SyncSettings => Which::Sync,
        Modal::ExportSettings => Which::Export,
        Modal::UploadUnconfirmed { .. } => Which::UploadUnconfirmed,
        Modal::SaveExportPreset { .. } => Which::SaveExport,
        Modal::ConfirmDeleteExportPreset { .. } => Which::ConfirmDeleteExport,
        Modal::ConfirmDeletePreset { .. } => Which::ConfirmDelete,
        Modal::ConfirmForgetFolder { .. } => Which::ConfirmForget,
        Modal::ConfirmDeleteKeyword { .. } => Which::ConfirmDeleteKeyword,
        Modal::ConfirmMergeKeyword { .. } => Which::ConfirmMergeKeyword,
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
        Which::ConfirmDeleteKeyword => confirm_delete_keyword(app, ctx),
        Which::ConfirmMergeKeyword => confirm_merge_keyword(app, ctx),
        Which::PhotoInfo => info::show(app, ctx),
        Which::EditRule => rule_edit::show(app, ctx),
        Which::Settings => settings::show(app, ctx),
        Which::Shortcuts => shortcuts(app, ctx),
        Which::About => about(app, ctx),
        Which::UploadUnconfirmed => export::upload_unconfirmed(app, ctx),
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

    let out = dialog(
        ctx,
        &mut app.settings,
        "confirm_forget_folder",
        "Forget folder?",
        theme::DialogSize::Compact,
        Footer::confirm("Forget", true, "Cancel"),
        |ui| {
            ui.label(format!("Forget “{}”?", path.display()));
            crate::theme::hint(
                ui,
                &format!(
                    "Removes {count} photos from the catalog, with their ratings, flags, labels \
                     and keywords. No files are deleted and no edits are lost — re-opening the \
                     folder catalogues it again."
                ),
            );
        },
    );

    if out.primary {
        app.forget_folder(ctx, id, &path);
        app.modal = Modal::None;
    } else if out.dismissed {
        app.modal = Modal::None;
    }
}

/// Confirming a keyword deletion that takes more than the keyword itself.
///
/// ⚠⚠ **Undo does not help if you never notice.** Deleting a keyword removes its whole subtree
/// and every assignment of any of them from one click, with nothing said — so this names what is
/// about to go, in the shape `confirm_forget_folder` already uses.
fn confirm_delete_keyword(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::ConfirmDeleteKeyword {
        id,
        name,
        descendants,
        assignments,
    } = &app.modal
    else {
        return;
    };
    let (id, name, descendants, assignments) = (*id, name.clone(), *descendants, *assignments);

    let out = dialog(
        ctx,
        &mut app.settings,
        "confirm_delete_keyword",
        "Delete keyword?",
        theme::DialogSize::Compact,
        Footer::confirm("Delete", true, "Cancel"),
        |ui| {
            ui.label(format!("Delete “{name}”?"));
            let mut what = Vec::new();
            if descendants > 0 {
                what.push(format!(
                    "{descendants} keyword{} beneath it",
                    if descendants == 1 { "" } else { "s" }
                ));
            }
            if assignments > 0 {
                what.push(format!(
                    "the keyword from {assignments} photo{}",
                    if assignments == 1 { "" } else { "s" }
                ));
            }
            crate::theme::hint(
                ui,
                &format!(
                    "This also deletes {}. Ctrl+Z restores it.",
                    what.join(" and ")
                ),
            );
        },
    );

    if out.primary {
        app.delete_keyword_now(id);
        app.modal = Modal::None;
    } else if out.dismissed {
        app.modal = Modal::None;
    }
}

/// Confirming a keyword merge (M9.8): the author's rule for a rename or move onto a name a sibling
/// already has. It says what joins what and how many photos gain the survivor.
fn confirm_merge_keyword(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::ConfirmMergeKeyword {
        from,
        into,
        from_name,
        into_name,
        photos,
    } = &app.modal
    else {
        return;
    };
    let (from, into, from_name, into_name, photos) =
        (*from, *into, from_name.clone(), into_name.clone(), *photos);
    let out = dialog(
        ctx,
        &mut app.settings,
        "confirm_merge_keyword",
        "Merge keywords?",
        theme::DialogSize::Compact,
        Footer::confirm("Merge", false, "Cancel"),
        |ui| {
            ui.label(format!(
                "“{into_name}” already exists here. Merge “{from_name}” into it?"
            ));
            crate::theme::hint(
                ui,
                &format!(
                    "{photos} photo{} tagged “{from_name}” will carry “{into_name}”; keywords \
                     beneath it join those beneath “{into_name}”. Ctrl+Z undoes it.",
                    if photos == 1 { "" } else { "s" }
                ),
            );
        },
    );
    if out.primary {
        app.modal = Modal::None;
        app.merge_keyword_now(from, into);
    } else if out.dismissed {
        app.modal = Modal::None;
    }
}

/// The keyboard reference.
///
/// ⚠ **Generated from `menu::model()`, never hand-written.** A shortcuts dialog listing a
/// chord the app does not have is worse than no dialog at all, and a hand-written one drifts
/// the first time a shortcut moves.
fn shortcuts(app: &mut RustroomApp, ctx: &egui::Context) {
    // ⚠ Through the shared funnel like every other dialog. It built its own `egui::Modal` before,
    // which meant it missed the frame padding and the sizing entirely — and it is one of the
    // dialogs that most wants to be Wide and scrollable.
    let out = dialog(
        ctx,
        &mut app.settings,
        "shortcuts",
        "Keyboard shortcuts",
        theme::DialogSize::Wide,
        Footer::close("Close"),
        |ui| {
            // One section per menu section, under the panels' header. ⚠ The inner
            // `ScrollArea` is gone: `dialog` provides one, and nesting two vertical scroll
            // areas makes them fight over the wheel.
            for section in rustroom_app::menu::model() {
                let mut h = header(
                    ui,
                    egui::Id::new(("shortcuts", section.title)),
                    true,
                    Header::new(section.title),
                );
                h.state.show_body_unindented(ui, |ui| {
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
                });
            }
        },
    );
    if out.dismissed {
        app.modal = Modal::None;
    }
}

/// The about box.
fn about(app: &mut RustroomApp, ctx: &egui::Context) {
    let out = dialog(
        ctx,
        &mut app.settings,
        "about",
        "Rustroom",
        theme::DialogSize::Compact,
        Footer::close("Close"),
        |ui| {
            theme::hint(ui, "An open-source RAW photo editor.");
        },
    );
    if out.dismissed {
        app.modal = Modal::None;
    }
}

#[cfg(test)]
mod tests {
    use crate::theme::DialogSize;
    use crate::{test_app, test_ui, Modal};

    /// Draw the app's current modal once and return its accessible labels.
    fn labels(ctx: &egui::Context, app: &mut crate::RustroomApp) -> Vec<String> {
        test_ui::labelled_centres(ctx, Vec::new(), |ui| super::draw(app, ui.ctx()))
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    }

    /// ⚠ The tabs must not lose a setting: every control the single page had is reachable on
    /// one of the five, and this walks them all.
    #[test]
    fn every_setting_is_reachable_across_the_five_tabs() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        // ⚠ Pre-filled, so the Performance tab does not size the real user cache on disk.
        app.thumb_cache_bytes = Some(0);
        app.modal = Modal::Settings;
        let mut seen: Vec<String> = Vec::new();
        for tab in ["Behaviour", "Appearance", "Display", "Performance", "Input"] {
            test_ui::click(&ctx, tab, |ui| super::draw(&mut app, ui.ctx()));
            seen.extend(labels(&ctx, &mut app));
        }
        for want in [
            "Confirm before deleting a preset",
            "Open new photos with the camera rendering",
            "Apply lens corrections to new photos",
            "Write XMP sidecars beside RAW files",
            "Move to the next photo after rating",
            "Use the GPU",
            "Clear",
            "Apply",
            "Cancel",
            "Close",
        ] {
            assert!(
                seen.iter().any(|l| l == want),
                "{want:?} missing in {seen:?}"
            );
        }
    }

    #[test]
    fn the_information_dialog_offers_its_five_tabs() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.modal = Modal::PhotoInfo {
            id: rustroom_app::photo_id::PhotoId::new("/nonexistent/a.RAF".into(), 0),
            dims: Some((6240, 4160)),
            description: None,
        };
        let seen = labels(&ctx, &mut app);
        for want in ["File", "Image", "Capture", "Description", "Marks", "Close"] {
            assert!(
                seen.iter().any(|l| l == want),
                "{want:?} missing in {seen:?}"
            );
        }
    }

    #[test]
    fn the_collision_dialog_names_all_four_answers() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.export_collision = Some("/nonexistent/out/a.jpg".into());
        let seen = labels(&ctx, &mut app);
        for want in ["Keep both", "Skip", "Overwrite", "Cancel export"] {
            assert!(
                seen.iter().any(|l| l == want),
                "{want:?} missing in {seen:?}"
            );
        }
    }

    /// Every dialog id in the app, with the class it uses.
    ///
    /// ⚠ **A documentation test, and it is worth saying so rather than overselling it.** It pins
    /// the roster and that the ids are unique — it does **not** verify the wiring, which is
    /// appearance and is checked by hand with the app running. What it does catch is a dialog
    /// added with a duplicate id, which would make two dialogs share a persisted size and an
    /// `egui::Resize` state.
    #[test]
    fn every_dialog_has_a_class_and_the_ids_are_unique() {
        let dialogs: [(&str, DialogSize); 15] = [
            ("save_preset", DialogSize::Standard),
            ("confirm_delete", DialogSize::Compact),
            ("paste_settings", DialogSize::Standard),
            ("sync_settings", DialogSize::Standard),
            ("export_settings", DialogSize::Standard),
            ("save_export_preset", DialogSize::Standard),
            ("confirm_delete_export", DialogSize::Compact),
            ("export_collision", DialogSize::Standard),
            ("confirm_forget_folder", DialogSize::Compact),
            ("confirm_delete_keyword", DialogSize::Compact),
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
