//! Virtual copies, and the naming that follows from them — including renaming the file on disk
//! and `Reindex`.
//!
//! Split out of `main.rs` unchanged. ⚠ Copy numbers come from a **high-water mark** rather than
//! from the live maximum, because deleting the newest copy would otherwise free its number while
//! the undo log still holds it — see `Sidecar::copy_hwm`. Snapshots number the same way and live
//! in [`crate::snapshots`].

use crate::{LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::file_rename;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::sidecar;
use std::path::Path;

impl RustroomApp {
    /// Duplicate the open photo as a new virtual copy, and open it.
    ///
    /// ⚠ Copies the photo's **current** edits rather than starting neutral — "try a variation"
    /// should be one keystroke, not one keystroke plus redoing your work.
    pub(crate) fn create_virtual_copy(&mut self, ctx: &egui::Context) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let (path, edits) = (s.path.clone(), s.edits.clone());
        // ⚠ Persist the *current* copy first, or its unsaved edits are lost when the session is
        // replaced by the new one below.
        self.persist_current_session();

        let mut sidecar = sidecar::load_all(&path);
        let n = sidecar.next_copy();
        sidecar.set_edits(n, edits);
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not create a copy: {e}");
            return;
        }
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.add_copy(&path, n) {
                eprintln!("catalog: {e}");
            }
        }
        let id = PhotoId::new(path, n);
        self.rebuild_entries(Some(&id));
        self.status = format!("Created copy {n}");
        self.open_path(ctx, id);
    }

    /// Delete the open virtual copy. The original cannot be deleted — it *is* the file.
    pub(crate) fn delete_virtual_copy(&mut self, ctx: &egui::Context) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        if s.copy == 0 {
            self.status = "The original cannot be deleted".to_string();
            return;
        }
        let (path, copy) = (s.path.clone(), s.copy);

        let mut sidecar = sidecar::load_all(&path);
        let Some(entry) = sidecar.get(copy).cloned() else {
            return;
        };
        sidecar.copies.retain(|c| c.copy != copy);
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not delete the copy: {e}");
            return;
        }
        // ⚠ Drop this copy's stack *and* its log entries before the session goes: the copy no
        // longer exists, so an undo reaching it would pop steps for a photo that is not there.
        self.actions
            .forget_edits_of(&PhotoId::new(path.clone(), copy));
        self.actions.push(
            ActionKind::CopyDelete {
                path: path.clone(),
                entry: Box::new(entry),
            },
            None,
        );
        let back = PhotoId::original(path.clone());
        self.rebuild_entries(Some(&back));
        self.status = format!("Deleted copy {copy} — Ctrl+Z to restore");
        // The session is being replaced, so its edits must not be written back to a copy that
        // no longer exists.
        self.session = None;
        self.open_path(ctx, PhotoId::original(path));
    }

    /// What a copy is currently called, or `None` if it has never been named.
    ///
    /// ⚠ From the Library entry when there is one, so a rename made this session is seen without
    /// re-reading the file — and from the sidecar otherwise, which is the single-file case.
    pub(crate) fn copy_name_of(&self, id: &PhotoId) -> Option<String> {
        if let Some(lib) = &self.library {
            if let Some(e) = lib.entries.iter().find(|e| e.id == *id) {
                return e.name.clone();
            }
        }
        sidecar::load_all(&id.path)
            .label_of(id.copy)
            .map(str::to_string)
    }

    /// Rename a **virtual copy** — its own label, which is not a file.
    ///
    /// ⚠ Writes only this copy's `name`; the file, the other copies and every `Edits` are left
    /// alone. Undo and redo both come through here with the two names swapped, so the directions
    /// cannot drift apart — the same shape `rename_file` uses.
    pub(crate) fn set_copy_name(&mut self, id: &PhotoId, name: Option<String>) -> bool {
        // ⚠ Copy 0 *is* the file; its name is the filename, and storing a second one beside it
        // would be two names for one photo.
        if id.is_original() {
            return false;
        }
        // ⚠ Read-modify-write of the whole envelope: the file's other copies live in the same
        // sidecar and must survive.
        let mut sidecar = sidecar::load_all(&id.path);
        sidecar.set_label(id.copy, name.clone());
        if let Err(e) = sidecar::persist_all(&id.path, &sidecar) {
            self.status = format!("Could not rename the copy: {e}");
            return false;
        }
        if let Some(s) = self.session.as_mut() {
            if s.path == id.path && s.copy == id.copy {
                s.name = name.clone();
            }
        }
        // ⚠ The draft is dropped whatever happens: undo writes a name the field is not showing,
        // and a stale draft would paint over it.
        self.rename_draft = None;
        self.snapshot_rename = None;
        if let Some(lib) = self.library.as_mut() {
            for e in lib.entries.iter_mut() {
                if e.id == *id {
                    e.name = name.clone();
                }
            }
        }
        self.status = match &name {
            Some(n) => format!("Renamed copy {} to {n}", id.copy),
            None => format!("Cleared the name of copy {}", id.copy),
        };
        true
    }

    /// Rename a photo **on disk**, moving its sidecar with it.
    ///
    /// ⚠ Every copy of the file follows, because copies are versions *of* that file and it has
    /// one name. That is exactly why this is reachable **only from the original**: renaming from
    /// a copy would drag every sibling copy along with it, which is how it was first built and
    /// was reported as surprising. A copy renames itself instead — `set_copy_name`.
    pub(crate) fn rename_file(&mut self, ctx: &egui::Context, from: &Path, to: &Path) -> bool {
        if from == to {
            return false;
        }
        // ⚠ Never overwrite. A photo lost this way is not recoverable by undo.
        if to.exists() {
            self.status = format!(
                "There is already a {} here",
                to.file_name().unwrap_or_default().to_string_lossy()
            );
            return false;
        }
        // ⚠ Write the open photo's edits *before* moving the file, or they would be persisted to
        // the old path afterwards and orphaned.
        self.persist_current_session();

        if let Err(e) = std::fs::rename(from, to) {
            self.status = format!("Could not rename: {e}");
            return false;
        }
        // ⚠ The sidecar carries every edit ever made to this photo and all its copies. Leaving
        // it behind would make the file reopen neutral, with the old sidecar lingering as litter.
        let (sc_from, sc_to) = (file_rename::sidecar_of(from), file_rename::sidecar_of(to));
        if sc_from.exists() {
            let _ = std::fs::rename(&sc_from, &sc_to);
        }

        // ⚠⚠ In the same operation as the file move. A catalog row and the file it names are two
        // facts that must move as one — the rule the history stash and the action log needed
        // twice. A failure is reported, never fatal: the file has already moved.
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.rename_photo(from, to) {
                eprintln!("catalog: {e}");
            }
        }

        // ⚠ Every path-keyed cache now holds a path that no longer exists. The decode is
        // expensive, so it is re-keyed rather than dropped.
        self.cache.rekey(from, to);
        if let Some(lib) = self.library.as_mut() {
            for e in lib.scan.iter_mut() {
                if e.path == from {
                    e.path = to.to_path_buf();
                    e.file_name = file_rename::field_text(to);
                }
            }
            // ⚠ The *entries* must move too, not just the scan. Every thumbnail lookup goes
            // through `entries[i].id.path`, so leaving them on the old path greyed out exactly
            // the renamed photo and its copies while the rest of the folder stayed.
            for e in lib.entries.iter_mut() {
                if e.id.path == from {
                    e.id.path = to.to_path_buf();
                    e.file_name = file_rename::field_text(to);
                }
            }
        }
        let reopen = self
            .session
            .as_ref()
            .and_then(|s| (s.path == from).then(|| PhotoId::new(to.to_path_buf(), s.copy)));
        self.rename_draft = None;
        self.snapshot_rename = None;
        self.rebuild_entries(reopen.as_ref());
        if let Some(id) = reopen {
            self.open_path(ctx, id);
        }
        self.status = format!("Renamed to {}", file_rename::field_text(to));
        true
    }

    /// Re-read the folder from disk, restore alphabetical order, and close gaps in the copy
    /// numbering.
    ///
    /// ⚠⚠ **This is the one thing allowed to renumber copies, and it is a button for that
    /// reason.** Numbers are stable everywhere else precisely so a copy's export filename and its
    /// undo stack cannot shift under it. The price of that stability is `_1, _3, _7` after some
    /// deleting — which is the mess this exists to tidy, on request rather than behind the user's
    /// back.
    ///
    /// ⚠ Three consequences, all deliberate:
    /// - **Unnamed copies get new export filenames**, because their name is derived from the
    ///   number. A *named* copy keeps its name, so its export filename does not move.
    /// - **Undo stacks for renumbered copies are dropped.** They are keyed by `PhotoId`, and a
    ///   renumbered copy is a different `PhotoId`; keeping them would hand old copy 1's stack to
    ///   whatever becomes the new copy 1, which is worse than losing it.
    /// - It re-reads every sidecar and re-scans the directory, so files added or deleted outside
    ///   Rustroom appear here.
    pub(crate) fn reindex(&mut self, ctx: &egui::Context) {
        let Some(LibrarySource::Folder(dir)) = self.library.as_ref().map(|l| l.source.clone())
        else {
            // ⚠ Reindex renumbers copies in a *folder*. A collection has no single folder to
            // re-read, and renumbering photos you are merely viewing would be surprising.
            self.status =
                "Reindex works on a folder, not a collection, a date or All Photos".into();
            return;
        };
        // ⚠ Written first: a compaction rewrites the sidecar this photo's edits belong in, so an
        // unsaved change would otherwise be lost or land on a renumbered copy.
        self.persist_current_session();

        // ⚠ Re-scanning is what picks up files added or removed outside the app; the ordering
        // itself is re-applied by `rebuild_entries`, from the chosen sort order.
        let Ok(scan) = catalog::scan_folder(&dir) else {
            self.status = "Could not read the folder".into();
            return;
        };

        let mut renumbered = 0usize;
        for e in &scan {
            let mut sidecar = sidecar::load_all(&e.path);
            let moves = sidecar.compact();
            if moves.is_empty() {
                continue;
            }
            if sidecar::persist_all(&e.path, &sidecar).is_err() {
                continue;
            }
            renumbered += moves.len();
            // ⚠ Every `PhotoId` that moved is now a different photo as far as the undo machinery
            // is concerned. Both the stash and the log have to let go, together — a stash kept
            // without its log entries is unreachable, and log entries kept without their stash
            // pop nothing.
            for (from, to) in moves {
                let old = PhotoId::new(e.path.clone(), from);
                self.histories.forget(&old);
                self.actions.forget_edits_of(&old);
                // ⚠ The catalog follows the same renumbering, or its row for the old number
                // becomes unreachable while the new one looks empty.
                if let Some(cat) = &self.catalog {
                    if let Err(err) = cat.renumber_copy(&e.path, from, to) {
                        eprintln!("catalog: {err}");
                    }
                }
            }
        }

        // ⚠ The open photo may itself have been renumbered, so it is reopened by *path* at copy
        // 0 rather than by an id that might no longer exist.
        let reopen = self.session.as_ref().map(|s| s.path.clone());
        if let Some(lib) = self.library.as_mut() {
            lib.scan = scan;
        }
        self.rename_draft = None;
        self.snapshot_rename = None;
        let select = reopen.map(PhotoId::original);
        self.rebuild_entries(select.as_ref());
        if let Some(id) = select {
            self.session = None;
            self.open_path(ctx, id);
        }
        self.status = if renumbered == 0 {
            "Reindexed — the copy numbering was already tidy".to_string()
        } else {
            format!("Reindexed — renumbered {renumbered} copies")
        };
    }
}
