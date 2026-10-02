//! Moving an `Edits` around: copy/paste, presets and sync.
//!
//! Split out of `main.rs` unchanged. ⚠ All three go through one partition — [`EditGroups`]
//! — and one application path, `apply_to_selection`, so a paste, a preset click and a sync
//! cannot come to disagree about what they transfer.

use crate::develop;
use crate::modals::Modal;
use crate::RustroomApp;
use edit_model::EditGroups;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::action_log::{ActionKind, SyncAction};
use rustroom_app::edit_clipboard;
use rustroom_app::history::History;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::sidecar;
use rustroom_app::sync;
use std::path::PathBuf;

/// How a paste reports what it did. Shared by the Ctrl+V path and the "Paste…" dialog so
/// the two cannot describe the same work differently.
pub(crate) fn paste_status(touched: usize, failed: usize) -> String {
    match (touched, failed) {
        (0, _) => "Nothing selected to paste into".to_string(),
        (1, 0) => "Settings pasted".to_string(),
        (n, 0) => format!("Pasted to {n} photos"),
        (n, f) => format!("Pasted to {n} photos, {f} failed"),
    }
}

/// The six group checkboxes, shared by the Save-preset and Paste-settings dialogs.
pub(crate) fn edit_group_checkboxes(ui: &mut egui::Ui, groups: &mut EditGroups) {
    ui.horizontal(|ui| {
        if ui.button("All").clicked() {
            *groups = EditGroups::ALL;
        }
        if ui.button("None").clicked() {
            *groups = EditGroups::NONE;
        }
    });
    for (i, name) in edit_model::EDIT_GROUP_NAMES.iter().enumerate() {
        ui.checkbox(groups.get_mut(i), *name);
    }
}

impl RustroomApp {
    /// Selected photos **other than the one currently open** — the ones a batch has to
    /// reach through their sidecars rather than through the live session.
    ///
    /// Keyed on the open *path*, not the active index: in Develop those are the same photo,
    /// but a paste triggered from the Library grid may have a selection that does not
    /// include whatever is open, and then every selected photo needs a sidecar write.
    pub(crate) fn sync_targets(&self) -> Vec<PhotoId> {
        let open = self.open_photo();
        let Some(lib) = &self.library else {
            return Vec::new();
        };
        lib.selection
            .iter()
            .filter_map(|i| lib.entries.get(i).map(|e| e.id.clone()))
            .filter(|id| Some(id) != open.as_ref())
            .collect()
    }

    /// Whether the open photo is part of the current selection, and so should be changed
    /// through its session. True in single-file mode, where there is no selection at all
    /// and the open photo is the only thing an apply could mean.
    pub(crate) fn open_photo_is_selected(&self) -> bool {
        let Some(s) = &self.session else { return false };
        let Some(lib) = &self.library else {
            return true;
        };
        // ⚠ The **whole** id, not just the path. Comparing paths would call the open photo
        // "selected" whenever any *other copy of the same file* was selected, and a paste would
        // then be applied through the live session to the wrong version.
        let open = PhotoId::new(s.path.clone(), s.copy);
        lib.selection
            .iter()
            .filter_map(|i| lib.entries.get(i))
            .any(|e| e.id == open)
    }

    /// Apply `src` to the whole selection — the open photo through its session, the rest
    /// through their sidecars — as **one** undo step.
    ///
    /// Two mechanisms, one gesture, so the resulting actions are wrapped in a `Batch`: one
    /// Ctrl+V, or one preset click, must stay one Ctrl+Z.
    ///
    /// Returns `(photos changed, photos that failed)`. The caller words the status line,
    /// because "pasted" and "applied preset" want different phrasing for the same work.
    pub(crate) fn apply_to_selection(&mut self, src: &Edits, groups: EditGroups) -> (usize, usize) {
        // The open photo, as the undo log scopes it. `sync_targets` already excludes it and
        // needs no path of its own.
        let open_id = self.open_photo();
        let targets = self.sync_targets();
        let into_session = self.open_photo_is_selected();
        let mut parts: Vec<ActionKind> = Vec::new();
        let mut touched = 0usize;
        let mut failed = 0usize;

        if into_session {
            touched += 1;
            if self.apply_edits_to_session(src, groups) {
                // `into_session` is only true when a session exists, so this `if let` always
                // takes its branch — it is here because the compiler cannot know that.
                if let Some(id) = open_id.clone() {
                    parts.push(ActionKind::PhotoEdit { id });
                }
            }
        }
        if !targets.is_empty() {
            let report = sync::apply(&targets, src, groups);
            touched += report.prior.len();
            failed = report.failed.len();
            // ⚠ The targets' rows cache their sidecars — see `refresh_entry_edits`. Without
            // this a synced photo's thumbnail keeps showing its pre-sync state.
            let paths: Vec<PathBuf> = targets.iter().map(|t| t.path.clone()).collect();
            self.refresh_entry_edits(&paths);
            if !report.prior.is_empty() {
                parts.push(ActionKind::Sync(Box::new(SyncAction {
                    source: src.clone(),
                    groups,
                    targets: targets.clone(),
                    prior: report.prior,
                })));
            }
        }

        // A lone action does not need a wrapper — keep the log readable.
        match parts.len() {
            0 => {}
            1 => self.actions.push(parts.remove(0), open_id.as_ref()),
            _ => self
                .actions
                .push(ActionKind::Batch(parts), open_id.as_ref()),
        }
        (touched, failed)
    }

    /// Apply the open photo's edits to the rest of the selection, as one undo step.
    pub(crate) fn run_sync(&mut self, groups: EditGroups) {
        let targets = self.sync_targets();
        let Some(source) = self.session.as_ref().map(|s| s.edits.clone()) else {
            return;
        };
        if targets.is_empty() {
            self.status = "Nothing to sync".to_string();
            return;
        }
        // The source is the session's *in-memory* edits, not its sidecar: you sync what you
        // are looking at, not what was last written.
        let report = sync::apply(&targets, &source, groups);
        self.status = if report.failed.is_empty() {
            format!("Synced {} photos", report.prior.len())
        } else {
            format!(
                "Synced {}, {} failed",
                report.prior.len(),
                report.failed.len()
            )
        };
        let open = self.open_photo();
        self.actions.push(
            ActionKind::Sync(Box::new(SyncAction {
                source,
                groups,
                targets,
                prior: report.prior,
            })),
            open.as_ref(),
        );
    }

    /// Reload the open photo's edits if its sidecar was just rewritten underneath it.
    ///
    /// ⚠ The active photo is never a sync *target*, so a sync cannot make the open session
    /// stale. But a user can sync, switch to a synced photo, and *then* undo — which
    /// rewrites that photo's sidecar while it is open with its own in-memory edits. Without
    /// this the canvas silently disagrees with the disk until you switch away and back.
    pub(crate) fn reload_if_affected(&mut self, affected: &[PathBuf]) {
        let Some(open) = self.session.as_ref().map(|s| s.path.clone()) else {
            return;
        };
        if !affected.contains(&open) {
            return;
        }
        // ⚠ `load_copy` with the session's own copy: a sync may have rewritten a *different*
        // copy of the same file, and reloading copy 0 would replace the open copy's edits with
        // the original's.
        let copy = self.session.as_ref().map_or(0, |s| s.copy);
        let edits = sidecar::load_copy(&open, copy).unwrap_or_default();
        let tool = self.tool;
        if let Some(s) = &mut self.session {
            s.edits = edits;
            // The old history described a state that is no longer on disk.
            s.history = History::new(s.edits.clone());
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        }
        // That photo's history was just replaced, so its entries point at steps it no longer
        // holds. Every other photo's stack is untouched and stays.
        // ⚠ The *copy* whose history was replaced, not the file: another copy of the same file
        // still holds a perfectly good stack, and sweeping it would be silent data loss.
        if let Some(id) = self.open_photo() {
            self.actions.forget_edits_of(&id);
        }
    }

    /// Cmd/Ctrl+C and Cmd/Ctrl+V for the edit clipboard.
    ///
    /// ⚠ These must be read as **`egui::Event::Copy` / `Event::Paste`, not key presses.**
    /// `egui-winit` matches the clipboard chords first, pushes the semantic event and
    /// *returns without emitting a `Key` event at all* — so `key_pressed(Key::C)` can never
    /// fire, which is exactly how the first version of this silently did nothing.
    ///
    /// The copy also writes to the **system** clipboard, and not only as a nicety: egui
    /// pushes `Event::Paste` only when the system clipboard holds non-empty text, so
    /// without that write Ctrl+V would produce no event on a freshly booted machine.
    ///
    /// Gated on nothing having keyboard focus, the way `handle_develop_arrows` already is,
    /// so Ctrl+C inside the preset-name field still copies text.
    pub(crate) fn handle_clipboard_keys(&mut self, ctx: &egui::Context) {
        // Deliberately not gated on a session: a copy needs one, but a paste only needs a
        // clipboard and a selection, and Ctrl+V has to work from the Library grid too.
        if self.modal != Modal::None {
            return;
        }
        if ctx.memory(|m| m.focused()).is_some() {
            return;
        }
        let (copy, pasted) = ctx.input(|i| {
            let mut copy = false;
            let mut pasted: Option<String> = None;
            for e in &i.events {
                match e {
                    egui::Event::Copy => copy = true,
                    egui::Event::Paste(text) => pasted = Some(text.clone()),
                    _ => {}
                }
            }
            (copy, pasted)
        });

        if copy {
            self.copy_settings(ctx);
        }

        if let Some(text) = pasted {
            // Prefer the clipboard's own payload — that is what makes copy/paste work
            // between two running instances — and fall back to this session's in-memory
            // copy when the system clipboard holds something else entirely.
            let src = edit_clipboard::decode(&text).or_else(|| self.clipboard.clone());
            if let Some(src) = src {
                // Ctrl+V uses the last-used group selection with no dialog; the "Paste…"
                // button opens the chooser. Either way it targets the whole selection.
                let (n, failed) = self.apply_to_selection(&src, self.paste_groups);
                self.status = paste_status(n, failed);
            }
        }
    }

    /// Copy the open photo's edits to the clipboard, in memory and on the system clipboard.
    ///
    /// ⚠ One implementation for the menu item, the toolbar button and Ctrl+C. They used to be
    /// two, which is how a button and its shortcut drift apart.
    ///
    /// ⚠ `self.session.as_ref().map(..)` rather than `if let Some(s) = &self.session`: the
    /// latter holds an immutable borrow of `self` across the assignment and does not compile.
    pub(crate) fn copy_settings(&mut self, ctx: &egui::Context) {
        if self.session.is_none() {
            return;
        }
        self.clipboard = self.session.as_ref().map(|s| s.edits.clone());
        if let Some(edits) = &self.clipboard {
            ctx.copy_text(edit_clipboard::encode(edits));
        }
        self.status = "Settings copied".to_string();
    }

    /// Apply the in-memory clipboard to the selection with the last-used groups, no dialog.
    pub(crate) fn paste_last_groups(&mut self) {
        let Some(src) = self.clipboard.clone() else {
            return;
        };
        let (n, failed) = self.apply_to_selection(&src, self.paste_groups);
        self.status = paste_status(n, failed);
    }

    /// Reset the five adjustment groups the Edit tool shows, leaving geometry and masks alone.
    ///
    /// ⚠ Scoped, for the reason the button's own comment gives: under an exclusive panel each
    /// tool resets its own domain and must not discard a crop or a mask the user cannot see.
    ///
    /// ⚠⚠ To how a new photo opens (`Settings::initial_edits(None)`), never all-off: with "Apply
    /// lens correction to new photos" ticked the corrections are the default, not an edit, and a
    /// reset that turned them off undid a preference (reported in use 2026-09-29).
    pub(crate) fn reset_adjustments(&mut self) {
        let tool = self.tool;
        let fresh = self.settings.initial_edits(None);
        let Some(s) = &mut self.session else { return };
        s.edits
            .merge_from(&fresh, develop::panel::adjustments::edit_groups());
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        if s.history.push(s.edits.clone()) {
            if let Some(path) = self.open_photo() {
                self.actions
                    .push(ActionKind::PhotoEdit { id: path.clone() }, Some(&path));
            }
        }
    }

    /// Re-read the presets directory. Called after a save or a delete, not every frame.
    pub(crate) fn reload_presets(&mut self) {
        self.preset_list = self.preset_store.list();
        self.preset_folders = self.preset_store.folders();
    }

    /// Move a preset to the trash and refresh the cached list.
    pub(crate) fn delete_preset(&mut self, folder: Option<&str>, name: &str) {
        match self.preset_store.delete(folder, name) {
            Ok(()) => {
                self.status = format!("Deleted preset “{name}” — Ctrl+Z to restore");
                let open = self.open_photo();
                self.actions.push(
                    ActionKind::PresetDelete {
                        folder: folder.map(str::to_string),
                        name: name.to_string(),
                    },
                    open.as_ref(),
                );
            }
            Err(e) => self.status = format!("Could not delete preset: {e}"),
        }
        self.reload_presets();
    }

    /// Delete whatever preset the export dialog has selected, and clear the selection.
    pub(crate) fn delete_selected_export_preset(&mut self) {
        if let Some(name) = self.selected_export_preset.take() {
            if let Err(e) = self.export_preset_store.delete(&name) {
                self.status = format!("Could not delete preset: {e}");
            }
            self.export_preset_list = self.export_preset_store.list();
        }
    }
}
