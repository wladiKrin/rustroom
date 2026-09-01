//! Walking [`ActionLog`] backwards and forwards.
//!
//! Split out of `main.rs` unchanged. ⚠ `undo_one` and `redo_one` are exact inverses and
//! must stay so — several `ActionKind`s call one function with the arguments swapped
//! precisely so the two directions cannot drift.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::sidecar;
use rustroom_app::sync;
use std::path::PathBuf;

impl RustroomApp {
    /// Undo the most recent action, whichever kind it was.
    pub(crate) fn undo_action(&mut self, ctx: &egui::Context) {
        let open = self.open_photo();
        if let Some(kind) = self.actions.undo(open.as_ref()) {
            self.undo_one(ctx, kind);
        }
    }

    // ⚠ Takes `ctx` because restoring a deleted copy has to *reopen* it, not merely put it back
    // in the sidecar — see the `CopyDelete` arm.
    pub(crate) fn undo_one(&mut self, ctx: &egui::Context, kind: ActionKind) {
        match kind {
            ActionKind::Rename { from, to } => {
                // ⚠ Undo moves it back; redo moves it forward. One function, arguments swapped,
                // so the two directions cannot drift apart.
                self.rename_file(ctx, &to, &from);
            }
            ActionKind::CopyRename { id, from, to: _ } => {
                self.set_copy_name(&id, from);
            }
            ActionKind::CopyDelete { path, entry } => {
                // ⚠ Read-modify-write: other copies of this file are in the same sidecar and
                // must survive the restore.
                let mut sidecar = sidecar::load_all(&path);
                let n = entry.copy;
                // ⚠ Through `restore_copy`, not a bare `copies.push`: a direct push bypasses the
                // high-water mark, which is the one thing stopping this undo from landing a
                // second copy with a number a *newer* copy has since taken.
                sidecar.restore_copy(*entry);
                match sidecar::persist_all(&path, &sidecar) {
                    Ok(()) => self.status = format!("Restored copy {n}"),
                    Err(e) => self.status = format!("Could not restore the copy: {e}"),
                }
                // ⚠ Reopen it, don't merely restore it. Undo of a delete has to put you back
                // where you were: restoring the sidecar alone left the row in the filmstrip
                // while the canvas still showed the original, which reads as the undo half
                // working.
                let id = PhotoId::new(path, n);
                self.rebuild_entries(Some(&id));
                self.open_path(ctx, id);
            }
            ActionKind::SnapshotDelete { id, entry } => {
                self.put_back_snapshot(&id, *entry);
            }
            ActionKind::SnapshotRename {
                id: _,
                n,
                from,
                to: _,
            } => {
                // ⚠ No id check: both variants are photo-scoped, so `reachable` has already
                // guaranteed the open photo *is* the one they name. One function with the
                // arguments swapped, so undo and redo cannot drift — `set_copy_name`'s rule.
                self.set_snapshot_name(n, from);
            }
            ActionKind::Batch(parts) => {
                // Reverse order: the last effect a gesture had is the first one undone.
                for part in parts.into_iter().rev() {
                    self.undo_one(ctx, part);
                }
            }
            ActionKind::PhotoEdit { .. } => {
                if let Some(s) = &mut self.session {
                    s.history.undo();
                }
                self.rerender_current();
            }
            ActionKind::CatalogDelete { snapshot } => {
                if let Some(cat) = &self.catalog {
                    match cat.restore(&snapshot) {
                        Ok(()) => self.status = snapshot.label(),
                        Err(e) => self.status = format!("Could not restore: {e}"),
                    }
                }
                self.after_catalog_restore(ctx, &snapshot);
            }
            ActionKind::PresetDelete { name } => {
                match self.preset_store.restore(&name) {
                    Ok(Some(p)) => self.status = format!("Restored preset “{}”", p.name),
                    // Evicted past the trash cap, or removed by hand.
                    Ok(None) => self.status = format!("“{name}” is no longer in the trash"),
                    Err(e) => self.status = format!("Could not restore preset: {e}"),
                }
                self.reload_presets();
            }
            ActionKind::Sync(action) => {
                let failed = sync::revert(&action.prior);
                let total = action.prior.len();
                self.status = if failed.is_empty() {
                    format!("Reverted sync of {total} photos")
                } else {
                    format!("Reverted {}, {} failed", total - failed.len(), failed.len())
                };
                let affected: Vec<PathBuf> = action.prior.iter().map(|(p, _)| p.clone()).collect();
                self.reload_if_affected(&affected);
            }
        }
    }

    /// Redo the most recently undone action.
    pub(crate) fn redo_action(&mut self, ctx: &egui::Context) {
        let open = self.open_photo();
        if let Some(kind) = self.actions.redo(open.as_ref()) {
            self.redo_one(ctx, kind);
        }
    }

    pub(crate) fn redo_one(&mut self, ctx: &egui::Context, kind: ActionKind) {
        match kind {
            ActionKind::Rename { from, to } => {
                self.rename_file(ctx, &from, &to);
            }
            ActionKind::CopyRename { id, from: _, to } => {
                self.set_copy_name(&id, to);
            }
            ActionKind::CopyDelete { path, entry } => {
                let mut sidecar = sidecar::load_all(&path);
                sidecar.copies.retain(|c| c.copy != entry.copy);
                match sidecar::persist_all(&path, &sidecar) {
                    Ok(()) => self.status = format!("Deleted copy {}", entry.copy),
                    Err(e) => self.status = format!("Could not delete the copy: {e}"),
                }
                // The copy is gone, so whatever was showing it must move to the original.
                let id = PhotoId::original(path);
                self.rebuild_entries(Some(&id));
                self.open_path(ctx, id);
            }
            ActionKind::SnapshotDelete { id, entry } => {
                let mut sidecar = sidecar::load_all(&id.path);
                sidecar.remove_snapshot(id.copy, entry.n);
                match sidecar::persist_all(&id.path, &sidecar) {
                    Ok(()) => self.status = format!("Deleted {}", entry.label()),
                    Err(e) => self.status = format!("Could not delete the snapshot: {e}"),
                }
                if let Some(s) = self.session.as_mut() {
                    if s.path == id.path && s.copy == id.copy {
                        s.snapshots.retain(|x| x.n != entry.n);
                    }
                }
            }
            ActionKind::SnapshotRename {
                id: _,
                n,
                from: _,
                to,
            } => {
                // See the undo arm: same function, arguments swapped.
                self.set_snapshot_name(n, to);
            }
            ActionKind::Batch(parts) => {
                for part in parts {
                    self.redo_one(ctx, part);
                }
            }
            ActionKind::PhotoEdit { .. } => {
                if let Some(s) = &mut self.session {
                    s.history.redo();
                }
                self.rerender_current();
            }
            ActionKind::CatalogDelete { snapshot } => {
                self.redo_catalog_delete(ctx, &snapshot);
            }
            ActionKind::PresetDelete { name } => {
                match self.preset_store.delete(&name) {
                    Ok(()) => self.status = format!("Deleted preset “{name}”"),
                    Err(e) => self.status = format!("Could not delete preset: {e}"),
                }
                self.reload_presets();
            }
            ActionKind::Sync(action) => {
                let report = sync::apply(&action.targets, &action.source, action.groups);
                self.status = format!("Re-synced {} photos", report.prior.len());
                let paths: Vec<PathBuf> = action.targets.iter().map(|t| t.path.clone()).collect();
                self.reload_if_affected(&paths);
            }
        }
    }
}
