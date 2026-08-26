//! The one editable name a photo has, and what committing it means.
//!
//! ⚠⚠ **Which name it edits depends on the row, and that asymmetry is the feature.** On the
//! original it is the filename on disk; on a virtual copy it is that copy's own name. A copy has
//! no file of its own, so editing the filename from one renamed the shared file and dragged every
//! sibling copy along with it — reported as surprising, and the reason copy names exist.
//!
//! ⚠ This module exists so the Develop panel and the Library grid cannot drift apart. They had
//! one editable field between them and it lived in the panel; growing a second copy in the grid
//! is how "the same logic" becomes two subtly different logics.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::entries;
use rustroom_app::file_rename;
use rustroom_app::photo_id::PhotoId;
use std::path::PathBuf;

/// What the name field committed: a file on disk, or one copy's own name.
pub enum Rename {
    File(PathBuf),
    Copy(Option<String>),
}

/// An editable name, drawn as a text field.
///
/// ⚠ Edits a **scratch** draft, not the stored value. Writing through would rename the file on
/// every keystroke — creating a file per character — and would commit a half-typed name the moment
/// focus moved. Same shape the export dialog's `export_draft` uses.
pub fn field(
    ui: &mut egui::Ui,
    id: &PhotoId,
    file_name: &str,
    name: Option<&str>,
    draft: &mut Option<(PhotoId, String)>,
    font: egui::TextStyle,
) -> Option<Rename> {
    let on_copy = !id.is_original();
    let default = file_rename::default_copy_name(file_name, id.copy);
    // ⚠ The same function the Library caption uses, so the field can never show a name
    // different from the one beside it.
    let current = entries::shown_name(file_name, id.copy, name);

    // ⚠ A draft belonging to a *different* photo is discarded, not shown. Without this the last
    // name typed followed you to every photo you opened afterwards.
    if draft.as_ref().is_some_and(|(d, _)| d != id) {
        *draft = None;
    }
    let (_, text) = draft.get_or_insert_with(|| (id.clone(), current.clone()));

    let r = ui.add(
        egui::TextEdit::singleline(text)
            .font(font)
            .desired_width(f32::INFINITY),
    );
    // ⚠ Read the key press into a local *before* touching `r`. `Response` methods take the
    // context lock internally, and holding one across `ui.input(..)` panics inside epaint's
    // mutex — the re-entrant lock the canvas-navigation work already paid for.
    let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));

    if r.gained_focus() {
        // Clicking in selects the whole name, so typing replaces it.
        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id) {
            let end = text.chars().count();
            st.cursor
                .set_char_range(Some(egui::text_selection::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(end),
                )));
            st.store(ui.ctx(), r.id);
        }
    }

    if r.lost_focus() || (r.has_focus() && entered) {
        let typed = text.clone();
        *draft = None;
        if typed == current {
            // ⚠ Nothing typed, nothing recorded — or Ctrl+Z would appear to do nothing.
            return None;
        }
        if on_copy {
            let trimmed = typed.trim();
            // ⚠ Typing the default back, or clearing the field, stores `None` rather than that
            // literal string. `None` is what makes an unnamed copy *follow* a later rename of its
            // file; freezing the derived name would silently strand it on the old stem.
            let to = (!trimmed.is_empty() && trimmed != default).then(|| trimmed.to_string());
            return (to.as_deref() != name).then_some(Rename::Copy(to));
        }
        // ⚠ A refusal leaves the file alone and the field resets to the real name, so an empty
        // or unusable entry can never look like it worked.
        if let Ok(to) = file_rename::target(&id.path, &typed) {
            if to != id.path {
                return Some(Rename::File(to));
            }
        }
    }
    None
}

/// Apply what [`field`] committed, and record it so Ctrl+Z can take it back.
///
/// ⚠ Must be called **outside** whatever borrow drew the field: it writes the sidecar and rebuilds
/// the Library, both of which need `&mut RustroomApp`.
pub fn commit(app: &mut RustroomApp, ctx: &egui::Context, id: &PhotoId, rename: Rename) {
    match rename {
        Rename::File(to) => {
            let from = id.path.clone();
            if app.rename_file(ctx, &from, &to) {
                // ⚠ Pushed only when the rename actually happened — a refused one (name taken,
                // empty) must not leave an undo step that would move a file nobody moved.
                //
                // ⚠ Global rather than photo-scoped: after the rename the photo is at a
                // *different* path, so scoping it to the old one would make it unreachable the
                // moment it took effect.
                app.actions.push(ActionKind::Rename { from, to }, None);
            }
        }
        Rename::Copy(to) => {
            let from = app.copy_name_of(id);
            if app.set_copy_name(id, to.clone()) {
                // ⚠ Scoped to its copy, unlike a file rename: the photo still exists at the same
                // path, so `reachable` can compare against it.
                app.actions.push(
                    ActionKind::CopyRename {
                        id: id.clone(),
                        from,
                        to,
                    },
                    Some(id),
                );
            }
        }
    }
}
