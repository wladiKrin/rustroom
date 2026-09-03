//! Snapshots: named saved states within one virtual copy.
//!
//! Split out of `main.rs` unchanged. ⚠⚠ Snapshots and [`History<Edits>`] are deliberately
//! **orthogonal**: taking one pushes no undo entry (it changes no `Edits`), undoing past one
//! cannot touch it (it was never in the stack), and *restoring* one does push, because that
//! genuinely changes the edits. That independence is what stops a snapshot vanishing
//! silently.

use crate::RustroomApp;
use rustroom_app::action_log::ActionKind;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::sidecar;

impl RustroomApp {
    /// Save the open copy's current edits as a new snapshot.
    ///
    /// ⚠⚠ **Pushes no undo action, deliberately.** It changes no `Edits`, so `History<Edits>` has
    /// nothing to hold; and it follows `create_virtual_copy`, which likewise pushes nothing while
    /// `delete_virtual_copy` does. Creation is additive and reversed by deleting; deletion
    /// destroys the only record of a state.
    pub(crate) fn take_snapshot(&mut self) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let (path, copy, edits) = (s.path.clone(), s.copy, s.edits.clone());
        let mut sidecar = sidecar::load_all(&path);
        let n = sidecar.next_snapshot(copy);
        let snapshot = rustroom_app::sidecar::Snapshot {
            n,
            name: None,
            // ⚠ Local wall-clock as naive seconds — `Capture::taken`'s convention, so
            // `modals::info::taken_text` reads it back with no timezone anywhere.
            taken: chrono::Local::now().naive_local().and_utc().timestamp(),
            edits,
        };
        sidecar.add_snapshot(copy, snapshot.clone());
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not save the snapshot: {e}");
            return;
        }
        let label = snapshot.label();
        if let Some(s) = self.session.as_mut() {
            s.snapshots.push(snapshot);
        }
        self.status = format!("Saved {label}");
    }

    /// Make snapshot `n` the open copy's live edits. Reports whether a history step was recorded.
    ///
    /// ⚠⚠ **The one place snapshots and `History<Edits>` meet, and it pushes no `ActionKind`
    /// itself.** `develop::body` already records exactly one `ActionKind::PhotoEdit` from the
    /// `photo_edited` flag both panel halves return, so pushing here too would make one restore
    /// cost two presses of Ctrl+Z. This reports like a slider and lets the existing path log it.
    pub(crate) fn restore_snapshot(&mut self, n: u32) -> bool {
        let tool = self.tool;
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        let Some(snap) = s.snapshots.iter().find(|x| x.n == n) else {
            return false;
        };
        let label = snap.label();
        s.edits = snap.edits.clone();
        // ⚠ An index into the *old* mask list. Every read site guards with `.get(i)`, so a stale
        // one is harmless today — but a mask added later would land at that index and appear
        // selected without anyone having selected it. A photo switch clears it for this reason.
        s.selected_mask = None;
        // ⚠ Same reason as `selected_mask`: a restored state has a different spot list, so a
        // stale index would leave a spot selected that nobody selected.
        s.selected_spot = None;
        s.active_mask_handle = None;
        let pushed = s.history.push(s.edits.clone());
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        self.status = format!("Restored {label}");
        pushed
    }

    /// Delete snapshot `n`, keeping it in the undo log so Ctrl+Z can put it back.
    pub(crate) fn delete_snapshot(&mut self, n: u32) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let (path, copy) = (s.path.clone(), s.copy);
        let mut sidecar = sidecar::load_all(&path);
        let Some(entry) = sidecar.remove_snapshot(copy, n) else {
            return;
        };
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not delete the snapshot: {e}");
            return;
        }
        let label = entry.label();
        if let Some(s) = self.session.as_mut() {
            s.snapshots.retain(|x| x.n != n);
        }
        let id = PhotoId::new(path, copy);
        self.actions.push(
            ActionKind::SnapshotDelete {
                id: id.clone(),
                entry: Box::new(entry),
            },
            Some(&id),
        );
        self.status = format!("Deleted {label} — Ctrl+Z to restore");
    }

    /// Rename snapshot `n`, recording it so the change can be undone.
    ///
    /// ⚠ Undo and redo both come through [`Self::set_snapshot_name`] with the two names swapped,
    /// so the directions cannot drift apart — `set_copy_name`'s rule.
    pub(crate) fn rename_snapshot(&mut self, n: u32, name: Option<String>) {
        let from = self
            .session
            .as_ref()
            .and_then(|s| s.snapshots.iter().find(|x| x.n == n))
            .and_then(|x| x.name.clone());
        if from == name || !self.set_snapshot_name(n, name.clone()) {
            return;
        }
        if let Some(id) = self.open_photo() {
            self.actions.push(
                ActionKind::SnapshotRename {
                    id: id.clone(),
                    n,
                    from,
                    to: name,
                },
                Some(&id),
            );
        }
    }

    /// Write a snapshot's name to the sidecar and to the session cache — the half both
    /// directions of the rename share.
    pub(crate) fn set_snapshot_name(&mut self, n: u32, name: Option<String>) -> bool {
        let Some(s) = self.session.as_ref() else {
            return false;
        };
        let (path, copy) = (s.path.clone(), s.copy);
        let mut sidecar = sidecar::load_all(&path);
        if !sidecar.set_snapshot_name(copy, n, name.clone()) {
            return false;
        }
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not rename the snapshot: {e}");
            return false;
        }
        if let Some(s) = self.session.as_mut() {
            if let Some(x) = s.snapshots.iter_mut().find(|x| x.n == n) {
                x.name = name;
            }
        }
        true
    }

    /// Put a deleted snapshot back, from the undo log's own copy of it.
    ///
    /// ⚠ Re-sorted by number rather than left where `add_snapshot` appended it: a restored
    /// snapshot must return to the position it had, or the strip reorders under an undo.
    pub(crate) fn put_back_snapshot(
        &mut self,
        id: &PhotoId,
        entry: rustroom_app::sidecar::Snapshot,
    ) {
        let mut sidecar = sidecar::load_all(&id.path);
        let label = entry.label();
        sidecar.add_snapshot(id.copy, entry.clone());
        if let Some(c) = sidecar.copies.iter_mut().find(|c| c.copy == id.copy) {
            c.snapshots.sort_by_key(|s| s.n);
        }
        match sidecar::persist_all(&id.path, &sidecar) {
            Ok(()) => self.status = format!("Restored {label}"),
            Err(e) => self.status = format!("Could not restore the snapshot: {e}"),
        }
        if let Some(s) = self.session.as_mut() {
            if s.path == id.path && s.copy == id.copy {
                s.snapshots.push(entry);
                s.snapshots.sort_by_key(|x| x.n);
            }
        }
    }

    /// Promote a snapshot to its own Library row, without disturbing the open photo's edits.
    ///
    /// ⚠ Seeds the new copy's name from the snapshot's **only when it has one** — an unnamed
    /// snapshot gives an unnamed copy, which then follows the file's name as any unnamed copy
    /// does. Freezing a derived label into a stored one would strand the copy on it.
    ///
    /// ⚠ The new copy inherits **no** snapshots: `set_edits`/`set_name` build a fresh entry.
    /// That is a decision, not an accident — inheriting would duplicate every saved state on
    /// every copy made.
    pub(crate) fn copy_from_snapshot(&mut self, ctx: &egui::Context, n: u32) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let Some(snap) = s.snapshots.iter().find(|x| x.n == n) else {
            return;
        };
        let (path, edits, name) = (s.path.clone(), snap.edits.clone(), snap.name.clone());
        // ⚠ Persist the current copy first, or its unsaved edits are lost when the session is
        // replaced below — `create_virtual_copy`'s rule.
        self.persist_current_session();

        let mut sidecar = sidecar::load_all(&path);
        let copy = sidecar.next_copy();
        sidecar.set_edits(copy, edits);
        sidecar.set_label(copy, name);
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not create a copy: {e}");
            return;
        }
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.add_copy(&path, copy) {
                eprintln!("catalog: {e}");
            }
        }
        let id = PhotoId::new(path, copy);
        self.rebuild_entries(Some(&id));
        self.status = format!("Created copy {copy}");
        self.open_path(ctx, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};
    use std::path::{Path, PathBuf};

    /// An app with a real file and a real sidecar behind the open session, so the snapshot
    /// methods have something to write to.
    fn app_on_a_real_photo(dir: &test_app::TestDir, ctx: &egui::Context) -> (RustroomApp, PathBuf) {
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();
        let mut app = test_app::for_test();
        app.finalize_session(
            ctx,
            PhotoId::original(photo.clone()),
            test_app::fake_decode(),
        );
        (app, photo)
    }

    fn stored(photo: &Path) -> Vec<rustroom_app::sidecar::Snapshot> {
        rustroom_app::sidecar::load_all(photo)
            .snapshots_of(0)
            .to_vec()
    }

    #[test]
    fn taking_a_snapshot_stores_the_current_edits_and_caches_them() {
        let dir = test_app::TestDir::new("takesnap");
        let ctx = test_ui::ctx();
        let (mut app, photo) = app_on_a_real_photo(&dir, &ctx);
        app.session.as_mut().unwrap().edits.exposure = 1.25;

        app.take_snapshot();

        let s = app.session.as_ref().unwrap();
        assert_eq!(s.snapshots.len(), 1, "the cache must see it immediately");
        assert_eq!(s.snapshots[0].edits.exposure, 1.25);
        assert_eq!(s.snapshots[0].label(), "Snapshot 1");
        assert_eq!(stored(&photo).len(), 1, "and it must be on disk too");
        assert_eq!(stored(&photo)[0].edits.exposure, 1.25);
    }

    /// ⚠ Asserted on `can_undo`, **not** on `edits`. The edits change for reasons other than the
    /// push, so asserting on them cannot see a missing `history.push` — the correction the
    /// headless canvas tests already had to make.
    #[test]
    fn restoring_a_snapshot_is_one_undo_step() {
        let dir = test_app::TestDir::new("restoresnap");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        app.session.as_mut().unwrap().edits.exposure = 1.25;
        app.take_snapshot();
        app.session.as_mut().unwrap().edits.exposure = -2.0;

        assert!(app.restore_snapshot(1), "a real change reports a step");

        let s = app.session.as_ref().unwrap();
        assert_eq!(s.edits.exposure, 1.25, "the snapshot's edits are now live");
        assert!(s.history.can_undo(), "and Ctrl+Z can take it back");
    }

    /// ⚠ `selected_mask` is an index into `edits.masks`; a restored state has a different list.
    /// Every read site guards with `.get(i)`, so a stale index is harmless *today* — but a mask
    /// added later would land at that index and appear selected without anyone selecting it.
    #[test]
    fn restoring_a_snapshot_clears_the_mask_selection() {
        let dir = test_app::TestDir::new("restoremask");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();
        app.session.as_mut().unwrap().selected_mask = Some(0);
        app.session.as_mut().unwrap().edits.exposure = 1.0;

        app.restore_snapshot(1);

        assert_eq!(app.session.as_ref().unwrap().selected_mask, None);
    }

    #[test]
    fn deleting_a_snapshot_is_undoable() {
        let dir = test_app::TestDir::new("delsnap");
        let ctx = test_ui::ctx();
        let (mut app, photo) = app_on_a_real_photo(&dir, &ctx);
        app.session.as_mut().unwrap().edits.exposure = 0.5;
        app.take_snapshot();

        app.delete_snapshot(1);
        assert!(app.session.as_ref().unwrap().snapshots.is_empty());
        assert!(stored(&photo).is_empty());

        app.undo_action(&ctx);
        assert_eq!(
            app.session.as_ref().unwrap().snapshots.len(),
            1,
            "undo must put it back in the cache"
        );
        assert_eq!(
            stored(&photo)[0].edits.exposure,
            0.5,
            "with its edits, from the log's own copy"
        );
    }

    /// ⚠ A restored snapshot returns to the position it had, rather than to the end — otherwise
    /// the strip silently reorders under an undo.
    #[test]
    fn an_undone_deletion_puts_the_snapshot_back_in_order() {
        let dir = test_app::TestDir::new("delorder");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        for _ in 0..3 {
            app.take_snapshot();
            app.session.as_mut().unwrap().edits.exposure += 1.0;
        }
        app.delete_snapshot(2);
        app.undo_action(&ctx);

        let ns: Vec<u32> = app
            .session
            .as_ref()
            .unwrap()
            .snapshots
            .iter()
            .map(|s| s.n)
            .collect();
        assert_eq!(ns, vec![1, 2, 3]);
    }

    #[test]
    fn renaming_a_snapshot_is_undoable_in_both_directions() {
        let dir = test_app::TestDir::new("renamesnap");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();

        app.rename_snapshot(1, Some("Warm".to_string()));
        assert_eq!(app.session.as_ref().unwrap().snapshots[0].label(), "Warm");

        app.undo_action(&ctx);
        assert_eq!(
            app.session.as_ref().unwrap().snapshots[0].label(),
            "Snapshot 1",
            "undo restores the derived label"
        );

        app.redo_action(&ctx);
        assert_eq!(
            app.session.as_ref().unwrap().snapshots[0].label(),
            "Warm",
            "redo puts the name back"
        );
    }

    /// ⚠ `max + 1` end to end, through the app rather than only through `Sidecar`.
    #[test]
    fn a_snapshot_number_is_not_reused_after_a_delete() {
        let dir = test_app::TestDir::new("renumbersnap");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();
        app.session.as_mut().unwrap().edits.exposure = 1.0;
        app.take_snapshot();
        app.delete_snapshot(2);
        app.session.as_mut().unwrap().edits.exposure = 2.0;
        app.take_snapshot();

        let labels: Vec<String> = app
            .session
            .as_ref()
            .unwrap()
            .snapshots
            .iter()
            .map(|s| s.label())
            .collect();
        assert_eq!(labels, vec!["Snapshot 1", "Snapshot 3"]);
    }

    /// ⚠⚠ **The collision the high-water mark exists to prevent, end to end.** Take two, delete
    /// the newest, take another, then undo the deletion: without the mark the new snapshot takes
    /// number 2 and the undo puts a *second* number 2 beside it, after which a rename or a delete
    /// hits whichever comes first.
    #[test]
    fn undoing_a_deletion_after_taking_another_cannot_duplicate_a_number() {
        let dir = test_app::TestDir::new("snapcollide");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();
        app.session.as_mut().unwrap().edits.exposure = 1.0;
        app.take_snapshot();
        app.delete_snapshot(2);
        app.session.as_mut().unwrap().edits.exposure = 2.0;
        app.take_snapshot();
        app.undo_action(&ctx);

        let mut ns: Vec<u32> = app
            .session
            .as_ref()
            .unwrap()
            .snapshots
            .iter()
            .map(|s| s.n)
            .collect();
        let before = ns.len();
        ns.sort_unstable();
        ns.dedup();
        assert_eq!(ns.len(), before, "every snapshot must have its own number");
        assert_eq!(ns, vec![1, 2, 3]);
    }

    /// ⚠ Spec point 2 of "what snapshots deliberately do not do". An inherited list would
    /// duplicate every saved state on every copy made.
    #[test]
    fn a_copy_made_from_a_snapshot_does_not_inherit_the_snapshots() {
        let dir = test_app::TestDir::new("copyfromsnap");
        let ctx = test_ui::ctx();
        let (mut app, photo) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();
        app.session.as_mut().unwrap().edits.exposure = 1.0;
        app.take_snapshot();

        app.copy_from_snapshot(&ctx, 1);

        let sc = rustroom_app::sidecar::load_all(&photo);
        assert_eq!(sc.snapshots_of(0).len(), 2, "the original keeps both");
        assert!(
            sc.snapshots_of(1).is_empty(),
            "the new copy starts with none of its own"
        );
    }

    /// ⚠ A **named** snapshot seeds the copy's name; an unnamed one leaves it deriving from the
    /// file, so the copy is not stranded on a label frozen at creation.
    #[test]
    fn a_copy_takes_the_snapshots_name_only_when_it_has_one() {
        let dir = test_app::TestDir::new("copynamesnap");
        let ctx = test_ui::ctx();
        let (mut app, photo) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();
        app.rename_snapshot(1, Some("Warm".to_string()));
        app.session.as_mut().unwrap().edits.exposure = 1.0;
        app.take_snapshot();

        app.copy_from_snapshot(&ctx, 1);
        app.copy_from_snapshot(&ctx, 2);

        let sc = rustroom_app::sidecar::load_all(&photo);
        assert_eq!(sc.label_of(1), Some("Warm"));
        assert_eq!(sc.label_of(2), None, "an unnamed snapshot names nothing");
    }
}
