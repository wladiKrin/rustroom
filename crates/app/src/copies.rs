//! Virtual copies, and the naming that follows from them — including renaming the file on disk
//! and `Reindex`.
//!
//! Split out of `main.rs` unchanged. ⚠ Copy numbers come from a **high-water mark** rather than
//! from the live maximum, because deleting the newest copy would otherwise free its number while
//! the undo log still holds it — see `Sidecar::copy_hwm`. Snapshots number the same way and live
//! in [`crate::snapshots`].

use crate::{AppView, LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::file_rename;
use rustroom_app::immich_source;
use rustroom_app::immich_worker::Job;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::sidecar;
use std::path::Path;

impl RustroomApp {
    /// Photo → Create virtual copy: of the open photo in Develop, of the selected one in the Library.
    pub(crate) fn create_virtual_copy(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.copy_subjects().into_iter().next() {
            self.create_copy_of(ctx, &id);
        }
    }

    /// Duplicate `id` as a new virtual copy, selected — and opened only when Develop is showing.
    ///
    /// ⚠ Copies the photo's **current** edits rather than starting neutral — "try a variation"
    /// should be one keystroke, not one keystroke plus redoing your work. The open photo's come from
    /// its live session; any other's from its sidecar.
    ///
    /// ⚠⚠ **The Library stays the Library** (the author, 2026-10-01): the right-click used to open
    /// the photo in Develop first and then the new copy. In the Library the copy is made and
    /// selected, and nothing is decoded.
    pub(crate) fn create_copy_of(&mut self, ctx: &egui::Context, id: &PhotoId) {
        let edits = if self.open_photo().as_ref() == Some(id) {
            // ⚠ Persist the *current* copy first, or its unsaved edits are lost when the session
            // is replaced by the new one below.
            self.persist_current_session();
            self.session.as_ref().map(|s| s.edits.clone())
        } else {
            None
        };
        let path = id.path.clone();
        let mut sidecar = sidecar::load_all(&path);
        let edits = edits.unwrap_or_else(|| {
            sidecar
                .get(id.copy)
                .map(|c| c.edits.clone())
                .unwrap_or_default()
        });
        let n = sidecar.next_copy();
        sidecar.set_edits(n, edits);
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not create a copy: {e}");
            return;
        }
        let viewing = self.selected_collection();
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.add_copy(&path, n) {
                eprintln!("catalog: {e}");
            }
            // ⚠⚠ A copy made while a collection is on screen joins it — Lightroom's rule, and the
            // only way it can stay visible, since that view shows its member copies alone.
            if let Some(c) = viewing {
                if let Err(e) = cat.add_to_collection(c, &path, n) {
                    eprintln!("catalog: {e}");
                }
            }
        }
        let new = PhotoId::new(path, n);
        if let Some(copies) = self.library.as_mut().and_then(|l| l.copies.as_mut()) {
            copies.insert(new.clone());
        }
        self.rebuild_entries(Some(&new));
        self.status = format!("Created copy {n}");
        if self.view == AppView::Develop {
            self.open_path(ctx, new);
        }
    }

    /// Photo → Delete virtual copy: the open photo in Develop, the selected copies in the Library.
    pub(crate) fn delete_virtual_copy(&mut self, ctx: &egui::Context) {
        let ids = self.copy_subjects();
        self.delete_copies(ctx, &ids);
    }

    /// What the Photo menu's copy commands act on: the open photo in Develop, the selection in the
    /// Library (its active photo first).
    fn copy_subjects(&self) -> Vec<PhotoId> {
        if self.view == AppView::Develop {
            return self.open_photo().into_iter().collect();
        }
        let Some(lib) = self.library.as_ref() else {
            return Vec::new();
        };
        let active = lib
            .selection
            .active()
            .and_then(|i| lib.entries.get(i))
            .map(|e| e.id.clone());
        let mut ids: Vec<PhotoId> = active.iter().cloned().collect();
        ids.extend(
            lib.selected_ids()
                .into_iter()
                .filter(|id| Some(id) != active.as_ref()),
        );
        ids
    }

    /// Delete the virtual copies among `ids` as **one** undo step. An original is the file itself
    /// and is skipped.
    ///
    /// ⚠ The catalog rows stay, so an undo brings a copy back with its rating, flags and
    /// collections — `CopyDelete` restores the sidecar entry only.
    pub(crate) fn delete_copies(&mut self, ctx: &egui::Context, ids: &[PhotoId]) {
        let copies: Vec<&PhotoId> = ids.iter().filter(|id| id.copy != 0).collect();
        if copies.is_empty() {
            self.status = "The original cannot be deleted".to_string();
            return;
        }
        let open = self.open_photo();
        let mut steps = Vec::new();
        let mut gone: Vec<PhotoId> = Vec::new();
        for id in copies {
            let mut sidecar = sidecar::load_all(&id.path);
            let Some(entry) = sidecar.get(id.copy).cloned() else {
                continue;
            };
            sidecar.copies.retain(|c| c.copy != id.copy);
            if let Err(e) = sidecar::persist_all(&id.path, &sidecar) {
                self.status = format!("Could not delete the copy: {e}");
                break;
            }
            // ⚠ Drop this copy's stack *and* its log entries: the copy no longer exists, so an
            // undo reaching it would pop steps for a photo that is not there.
            self.actions.forget_edits_of(id);
            steps.push(ActionKind::CopyDelete {
                path: id.path.clone(),
                entry: Box::new(entry),
            });
            gone.push(id.clone());
        }
        let Some(first) = gone.first() else {
            return;
        };
        let back = PhotoId::original(first.path.clone());
        self.status = match gone.len() {
            1 => format!("Deleted copy {} — Ctrl+Z to restore", first.copy),
            n => format!("Deleted {n} virtual copies — Ctrl+Z to restore"),
        };
        match steps.len() {
            1 => self.actions.push(steps.remove(0), None),
            _ => self.actions.push(ActionKind::Batch(steps), None),
        }
        self.rebuild_entries(Some(&back));
        if let Some(open) = open.filter(|o| gone.contains(o)) {
            // The session is being replaced, so its edits must not be written back to a copy
            // that no longer exists.
            self.session = None;
            if self.view == AppView::Develop {
                self.open_path(ctx, PhotoId::original(open.path));
            }
        }
    }

    /// What a copy is currently called, or `None` if it has never been named.
    ///
    /// ⚠ From the Library entry when there is one, so a rename made this session is seen without
    /// re-reading the file — and from the sidecar otherwise, which is the single-file case.
    pub(crate) fn copy_name_of(&self, id: &PhotoId) -> Option<String> {
        if let Some(lib) = &self.library {
            if let Some(e) = lib.entries.iter().find(|e| e.id == *id) {
                return e.label.clone();
            }
        }
        sidecar::load_all(&id.path)
            .label_of(id.copy)
            .map(str::to_string)
    }

    /// Rename a **virtual copy** — its own label, which is not a file.
    ///
    /// ⚠ Writes only this copy's `label`; the file, the other copies and every `Edits` are left
    /// alone. Undo and redo both come through here with the two labels swapped, so the directions
    /// cannot drift apart — the same shape `rename_file` uses.
    ///
    /// ⚠⚠ **Copy 0 is allowed**, unlike the `set_copy_name` this replaces. That refusal existed
    /// because a copy-0 "name" would have been a second filename beside the real one; a label is
    /// not a filename, so the premise is gone and "Colour" beside "B&W" is expressible.
    pub(crate) fn set_label(&mut self, id: &PhotoId, label: Option<String>) -> bool {
        // ⚠ Read-modify-write of the whole envelope: the file's other copies live in the same
        // sidecar and must survive.
        let mut sidecar = sidecar::load_all(&id.path);
        sidecar.set_label(id.copy, label.clone());
        if let Err(e) = sidecar::persist_all(&id.path, &sidecar) {
            self.status = format!("Could not label the copy: {e}");
            return false;
        }
        if let Some(s) = self.session.as_mut() {
            if s.path == id.path && s.copy == id.copy {
                s.name = label.clone();
            }
        }
        // ⚠ The draft is dropped whatever happens: undo writes a label the field is not showing,
        // and a stale draft would paint over it.
        self.label_draft = None;
        self.snapshot_rename = None;
        if let Some(lib) = self.library.as_mut() {
            for e in lib.entries.iter_mut() {
                if e.id == *id {
                    e.label = label.clone();
                }
            }
        }
        self.status = match &label {
            Some(n) => format!("Labelled copy {} \"{n}\"", id.copy),
            None => format!("Cleared the label of copy {}", id.copy),
        };
        true
    }

    /// Rename a photo **on disk**, moving its sidecar with it.
    ///
    /// ⚠ Every copy of the file follows, because copies are versions *of* that file and it has
    /// one name. That is exactly why this is reachable **only from the original**: renaming from
    /// a copy would drag every sibling copy along with it, which is how it was first built and
    /// was reported as surprising. A copy carries a label instead — `set_label`.
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

        // ⚠ Through Move to folder's `move_file`: the JSON sidecar carries every edit ever made to
        // this photo and its copies, and a RAW's `stem.xmp` its marks — both go with it, or the
        // photo goes back (M10.8d; the XMP used to stay behind under the old name).
        let planned = rustroom_app::photo_move::Planned {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        };
        if let Err(e) = rustroom_app::photo_move::move_file(&planned) {
            self.status = format!("Could not rename: {e}");
            return false;
        }

        // ⚠⚠ In the same operation as the file move. A catalog row and the file it names are two
        // facts that must move as one — the rule the history stash and the action log needed
        // twice. A failure is reported, never fatal: the file has already moved.
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.move_photo(from, to) {
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
        // ⚠ The catalog moved the stack row with the file; the view's map must follow it.
        self.reload_hand_stacks();
        self.rebuild_entries(reopen.as_ref());
        if let Some(id) = reopen {
            self.open_path(ctx, id);
        }
        self.status = format!("Renamed to {}", file_rename::field_text(to));
        true
    }

    /// Close gaps in the copy numbering of every photo in `scan`, and let go of the undo state and
    /// catalog rows of every copy that moved. Returns how many copies were renumbered.
    ///
    /// ⚠ Shared by the folder and the Immich branches of [`Self::reindex`]. Extracted verbatim; the
    /// three characterisation tests were written **first**, because `reindex` had none at all and
    /// "the existing tests still pass" would otherwise have proved nothing.
    ///
    /// ⚠⚠ The caller must have called `persist_current_session` first — a compaction rewrites the
    /// sidecar the open photo's edits belong in, so an unsaved change would otherwise be lost or land
    /// on a renumbered copy.
    fn compact_copy_numbering(&mut self, scan: &[catalog::CatalogEntry]) -> usize {
        let mut renumbered = 0usize;
        for e in scan {
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

        renumbered
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
        match self.library.as_ref().map(|l| l.source.clone()) {
            Some(LibrarySource::Folder(dir)) => self.reindex_folder(ctx, dir),
            // ⚠ The rail list requested is the **matching** one only: an album's validator comes
            // from the album list and a month's from the bucket list, so the other is irrelevant.
            Some(LibrarySource::ImmichAlbum { id, name }) => self.reindex_immich(
                Job::Album {
                    id: id.clone(),
                    name,
                },
                id,
                Job::Albums,
            ),
            Some(LibrarySource::ImmichBucket { bucket, label }) => self.reindex_immich(
                Job::Bucket {
                    bucket: bucket.clone(),
                    label,
                },
                bucket,
                Job::Buckets,
            ),
            // ⚠ A collection, a date or All Photos has no single source to re-read, and renumbering
            // photos you are merely viewing would be surprising.
            _ => {
                self.status = "Reindex works on a folder or an Immich album, not a collection, a \
                               date or All Photos"
                    .into()
            }
        }
    }

    /// Fetch an Immich album's or month's asset list again, so a change made in Immich appears.
    ///
    /// ⚠⚠ **`key` leaving `immich_assets` is the feature.** A click resolves an album from that map
    /// *before* the stored list's validator is consulted, and that map is never invalidated — so a
    /// refetch on its own would land and the next click would still serve the frozen list.
    ///
    /// ⚠ `list_job` refreshes the rail list so the refetched asset list can be stamped with a current
    /// validator. The two replies race; the rail list is a handful of rows against up to a thousand
    /// assets, so it lands first in practice, and if it loses, the stamp is stale and costs exactly
    /// **one** extra fetch on the next launch. Never wrong data, so not worth sequencing.
    ///
    /// ⚠⚠ Both requests would have been pointless before `pending_albums`/`pending_buckets` were
    /// split apart: one shared guard dropped whichever list came second, silently.
    fn reindex_immich(&mut self, fetch: Job, key: String, list_job: Job) {
        if immich_source::connection(&self.settings).is_none() {
            self.status = "No Immich server is configured — set one in Settings".into();
            return;
        }
        // ⚠ First, and the reason dropping the session below is safe.
        self.persist_current_session();
        let scan = self
            .library
            .as_ref()
            .map(|l| l.scan.clone())
            .unwrap_or_default();
        let renumbered = self.compact_copy_numbering(&scan);

        // ⚠ The open photo is **dropped rather than reopened**, unlike the folder branch: it may no
        // longer exist on the server, and re-downloading it is not what Reindex is for. You land in
        // the grid. Safe only because the session was persisted just above.
        self.session = None;
        self.rename_draft = None;
        self.snapshot_rename = None;

        self.immich_assets.remove(&key);
        // ⚠⚠ **Reindex is the ONLY way the stack projection refreshes**, because
        // `StackResponseDto` carries no `assetCount`/`lastModified` to validate against — stated as a
        // limitation in `immich_stack_cache`, not hidden. Dropping the file makes
        // `load_immich_stacks` refetch it.
        if let Some(p) = self.immich_stacks_path() {
            rustroom_app::immich_stack_cache::clear(&p);
        }
        self.load_immich_stacks();
        self.immich.request(list_job);
        // ⚠ Set before the request so the rail lights up on the press, not on the reply.
        self.immich_opening = Some(key);
        self.immich.request(fetch);
        self.status = if renumbered == 0 {
            "Fetching again from Immich…".to_string()
        } else {
            format!("Fetching again from Immich… (renumbered {renumbered} copies)")
        };
    }

    /// The folder branch: re-read the directory, compact, and reopen.
    fn reindex_folder(&mut self, ctx: &egui::Context, dir: std::path::PathBuf) {
        // ⚠ Written first: a compaction rewrites the sidecar this photo's edits belong in, so an
        // unsaved change would otherwise be lost or land on a renumbered copy.
        self.persist_current_session();

        // ⚠ Re-scanning is what picks up files added or removed outside the app; the ordering
        // itself is re-applied by `rebuild_entries`, from the chosen sort order.
        //
        // ⚠⚠ Through `scan_and_relink`, the function opening a folder uses (M9.9, at the author's
        // request): the whole tree, and a photo moved on disk relinked with its sidecars — so
        // Reindex and opening cannot disagree about what is there.
        let Some((scan, moved)) = self.scan_and_relink(&dir) else {
            return;
        };

        let renumbered = self.compact_copy_numbering(&scan);

        // ⚠ The open photo may itself have been renumbered, so it is reopened by *path* at copy
        // 0 rather than by an id that might no longer exist. ⚠ And it may have MOVED, in which
        // case it is reopened where it is now — its old path no longer exists.
        let reopen = self.session.as_ref().map(|s| {
            moved
                .iter()
                .find(|(from, _)| *from == s.path)
                .map(|(_, to)| to.clone())
                .unwrap_or_else(|| s.path.clone())
        });
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
        let mut status = if renumbered == 0 {
            "Reindexed — the copy numbering was already tidy".to_string()
        } else {
            format!("Reindexed — renumbered {renumbered} copies")
        };
        status.push_str(&rustroom_app::move_tracking::recognised(moved.len()));
        self.status = status;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::Edits;
    use std::path::PathBuf;

    /// A folder with `n` files that `catalog::scan_folder` will report.
    ///
    /// ⚠ The bytes are not images. `scan_folder` classifies by **extension**, and nothing in
    /// Reindex decodes anything, so a real JPEG would only make the test slower.
    fn folder_of(n: usize) -> crate::test_app::TestDir {
        let d = crate::test_app::TestDir::new("reindex");
        for i in 0..n {
            std::fs::write(d.0.join(format!("shot{i}.jpg")), b"not an image").unwrap();
        }
        d
    }

    /// ⚠⚠ M10.8d: renaming a RAW takes its XMP along — it used to stay behind under the old name —
    /// and a photo in a hand stack stays in it, in the view as well as in the catalog.
    #[test]
    fn a_renamed_raw_takes_its_xmp_and_keeps_its_stack() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = crate::test_app::TestDir::new("rename_xmp");
        for n in ["a.RAF", "b.RAF"] {
            std::fs::write(d.0.join(n), b"not a raw").unwrap();
        }
        std::fs::write(d.0.join("a.xmp"), b"<xmp/>").unwrap();
        let scan = catalog::scan_folder(&d.0).unwrap();
        // ⚠ What opening a folder does through `scan_and_relink`: the stack table keys on its row.
        app.catalog
            .as_ref()
            .unwrap()
            .sync_folder(&d.0, &scan)
            .unwrap();
        app.open_scan(&ctx, LibrarySource::Folder(d.0.clone()), scan);
        let lib = app.library.as_mut().unwrap();
        lib.selection.click(0);
        lib.selection.toggle(1);
        crate::commands::execute(&mut app, &ctx, rustroom_app::menu::Command::Stack);
        assert_eq!(
            app.library.as_ref().unwrap().entries.len(),
            1,
            "the sample: one stack — {}",
            app.status
        );

        let (from, to) = (d.0.join("a.RAF"), d.0.join("c.RAF"));
        assert!(app.rename_file(&ctx, &from, &to), "{}", app.status);

        assert!(
            d.0.join("c.xmp").exists() && !d.0.join("a.xmp").exists(),
            "the XMP followed"
        );
        let lib = app.library.as_ref().unwrap();
        assert!(
            lib.hand_stacks.contains_key(&to),
            "the view's stacks know the new name"
        );
        assert_eq!(lib.entries.len(), 1, "still one stack");
    }

    fn edited(v: f32) -> Edits {
        Edits {
            exposure: v,
            ..Edits::default()
        }
    }

    /// ⚠⚠ **A characterisation test written before the compaction loop was extracted, because
    /// `reindex` had NO tests at all** — so "the existing tests still pass" could not have shown the
    /// extraction was a pure move. This is what shows it.
    #[test]
    fn reindexing_a_folder_closes_a_gap_in_the_copy_numbering() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let dir = folder_of(1);
        let file = dir.0.join("shot0.jpg");

        // A gap: copy 1 was deleted, leaving copy 2 with no copy 1 beside it.
        let mut s = sidecar::load_all(&file);
        s.set_edits(0, edited(1.0));
        s.set_edits(2, edited(3.0));
        sidecar::persist_all(&file, &s).unwrap();

        let scan = catalog::scan_folder(&dir.0).unwrap();
        app.open_scan(&ctx, LibrarySource::Folder(dir.0.clone()), scan);

        app.reindex(&ctx);

        let after = sidecar::load_all(&file);
        assert!(
            after.get(1).is_some(),
            "copy 2 should have become copy 1: {}",
            app.status
        );
        assert!(after.get(2).is_none(), "and vacated its old number");
        assert_eq!(
            after.get(1).unwrap().edits,
            edited(3.0),
            "carrying its own edits, not the original's"
        );
        assert!(
            app.status.contains("renumbered"),
            "and it must say what it did: {}",
            app.status
        );
    }

    /// ⚠ The no-op case has its own message, because "Reindexed" with no further word reads as a
    /// failure when nothing visibly changed.
    #[test]
    fn reindexing_a_tidy_folder_says_it_was_already_tidy() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let dir = folder_of(2);
        let scan = catalog::scan_folder(&dir.0).unwrap();
        app.open_scan(&ctx, LibrarySource::Folder(dir.0.clone()), scan);

        app.reindex(&ctx);

        assert!(
            app.status.contains("already tidy"),
            "status was: {}",
            app.status
        );
    }

    /// A configured server, so the Immich branch is reachable at all.
    fn with_server(app: &mut RustroomApp) {
        app.settings.immich_url = "https://example.invalid".to_string();
        app.settings.immich_key = "k".to_string();
    }

    fn one_asset(id: &str) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            original_file_name: format!("{id}.jpg"),
            original_mime_type: None,
            width: Some(1),
            height: Some(1),
            thumbhash: None,
            file_modified_at: "2025-01-01T00:00:00.000Z".into(),
            updated_at: "2025-01-02T00:00:00.000Z".into(),
            is_favorite: false,
            exif_info: None,
        }
    }

    /// ⚠⚠ **The one line that makes the whole feature work.** A click resolves an album from
    /// `immich_assets` *before* the stored list's validator is consulted, and that map is never
    /// invalidated — so without this removal the refetch would land and the next click would still
    /// serve the frozen list.
    #[test]
    fn reindexing_an_immich_album_drops_its_frozen_asset_list() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        with_server(&mut app);
        app.immich_assets
            .insert("mine".to_string(), vec![one_asset("a")]);
        app.immich_assets
            .insert("other".to_string(), vec![one_asset("b")]);
        app.open_immich_album(
            &ctx,
            "mine".to_string(),
            "Mine".to_string(),
            &[one_asset("a")],
        );

        app.reindex(&ctx);

        assert!(
            !app.immich_assets.contains_key("mine"),
            "the reindexed album's cached list must go, or the refetch changes nothing"
        );
        assert!(
            app.immich_assets.contains_key("other"),
            "another album's list must be left alone — the open album, not all of them"
        );
    }

    #[test]
    fn reindexing_an_immich_month_drops_that_months_list() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        with_server(&mut app);
        app.immich_assets
            .insert("2026-08-01".to_string(), vec![one_asset("a")]);
        app.open_immich_bucket(
            &ctx,
            "2026-08-01".to_string(),
            "August 2026".to_string(),
            &[one_asset("a")],
        );

        app.reindex(&ctx);

        assert!(!app.immich_assets.contains_key("2026-08-01"));
    }

    /// ⚠ Feedback at the press, not when the reply lands: the server takes a couple of seconds over a
    /// list, and a press with no visible effect reads as a press that did nothing — which is exactly
    /// what was reported in use for album clicks.
    #[test]
    fn reindexing_an_immich_album_says_so_at_once() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        with_server(&mut app);
        app.open_immich_album(
            &ctx,
            "mine".to_string(),
            "Mine".to_string(),
            &[one_asset("a")],
        );

        app.reindex(&ctx);

        assert!(
            app.status.contains("Immich"),
            "the status must name where it is fetching from: {}",
            app.status
        );
        assert_eq!(app.immich_opening.as_deref(), Some("mine"));
    }

    /// ⚠ No server, no fetch — and it must say why rather than appearing to do nothing, **and** it
    /// must not throw away a list it cannot replace.
    #[test]
    fn reindexing_an_immich_album_with_no_server_keeps_the_list_and_says_why() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        with_server(&mut app);
        app.open_immich_album(
            &ctx,
            "mine".to_string(),
            "Mine".to_string(),
            &[one_asset("a")],
        );
        app.settings.immich_url = String::new();
        app.settings.immich_key = String::new();
        app.immich_assets
            .insert("mine".to_string(), vec![one_asset("a")]);

        app.reindex(&ctx);

        assert!(
            app.status.contains("Immich"),
            "it must say the server is missing: {}",
            app.status
        );
        assert!(
            app.immich_assets.contains_key("mine"),
            "and it must not throw the list away when it cannot refetch it"
        );
    }

    /// ⚠ The refusal used to name three unsupported sources and implied Immich was a fourth. It is
    /// supported now, so the message must not say otherwise.
    #[test]
    fn reindexing_something_unsupported_still_refuses_and_names_immich_as_working() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        app.library = Some(crate::test_app::library_of(2));
        app.library.as_mut().unwrap().source = LibrarySource::AllPhotos;

        app.reindex(&ctx);

        assert!(
            app.status.contains("Immich"),
            "the refusal must list Immich among what works: {}",
            app.status
        );
    }

    /// ⚠ Re-scanning is what picks up a file that appeared outside Rustroom — the reason Reindex
    /// re-reads the folder at all rather than only rewriting sidecars.
    #[test]
    fn reindexing_a_folder_picks_up_a_file_added_outside_rustroom() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let dir = folder_of(1);
        let scan = catalog::scan_folder(&dir.0).unwrap();
        app.open_scan(&ctx, LibrarySource::Folder(dir.0.clone()), scan);
        assert_eq!(app.library.as_ref().unwrap().entries.len(), 1);

        std::fs::write(dir.0.join("arrived.jpg"), b"not an image").unwrap();
        app.reindex(&ctx);

        assert_eq!(
            app.library.as_ref().unwrap().entries.len(),
            2,
            "the new file must appear"
        );
    }

    /// ⚠⚠ A copy made while a collection is on screen joins it (2026-10-01, Lightroom's rule): the
    /// view shows member copies only, so otherwise the new copy would vanish the moment it was made.
    #[test]
    fn a_copy_made_in_a_collection_view_joins_the_collection() {
        let ctx = egui::Context::default();
        let dir = crate::test_app::TestDir::new("copy_joins");
        let a = dir.0.join("a.RAF");
        std::fs::write(&a, b"not a real raw").unwrap();
        let mut app = crate::test_app::for_test();
        let cat = app.catalog.as_ref().unwrap();
        let entry = catalog::CatalogEntry {
            path: a.clone(),
            file_name: "a.RAF".into(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            edited: None,
        };
        cat.sync_folder(&dir.0, &[entry]).unwrap();
        let bw = cat.create_collection("BW").unwrap();
        cat.add_to_collection(bw, &a, 0).unwrap();
        app.open_collection(&ctx, bw, "BW");
        app.library.as_mut().unwrap().selection.click(0);

        app.create_virtual_copy(&ctx);

        let members = app
            .catalog
            .as_ref()
            .unwrap()
            .collection_members(bw)
            .unwrap();
        assert!(
            members.contains(&(a.clone(), 1)),
            "{members:?} — {}",
            app.status
        );
        let shown: Vec<u32> = app
            .library
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.id.copy)
            .collect();
        assert_eq!(shown, vec![0, 1], "the new copy is on screen");
    }

    /// A real RAW whose sidecar holds copies `1..=n` (copy k at exposure k), open as a folder in the
    /// Library.
    fn with_copies(tag: &str, n: u32) -> (crate::test_app::TestDir, crate::RustroomApp, PathBuf) {
        let ctx = egui::Context::default();
        let dir = crate::test_app::TestDir::new(tag);
        let a = dir.0.join("a.RAF");
        std::fs::write(&a, b"not a real raw").unwrap();
        for k in 1..=n {
            sidecar::persist_copy(&a, k, &edited(k as f32)).unwrap();
        }
        let mut app = crate::test_app::for_test();
        let scan = catalog::scan_folder(&dir.0).unwrap();
        app.open_scan(&ctx, LibrarySource::Folder(dir.0.clone()), scan);
        (dir, app, a)
    }

    fn index_of(app: &crate::RustroomApp, copy: u32) -> usize {
        let lib = app.library.as_ref().unwrap();
        lib.entries.iter().position(|e| e.id.copy == copy).unwrap()
    }

    fn active(app: &crate::RustroomApp) -> Option<PhotoId> {
        let lib = app.library.as_ref().unwrap();
        lib.selection
            .active()
            .and_then(|i| lib.entries.get(i))
            .map(|e| e.id.clone())
    }

    /// ⚠⚠ The author's report (2026-10-01): a copy made from the Library's right-click opened
    /// Develop. It stays in the Library now, the new copy selected and nothing decoded — carrying
    /// the clicked copy's edits, not the original's.
    #[test]
    fn a_copy_made_in_the_library_stays_in_the_library() {
        let (_d, mut app, a) = with_copies("copy_in_library", 1);
        let ctx = egui::Context::default();
        assert!(app.view == crate::AppView::Library, "the sample");
        let i = index_of(&app, 1);
        crate::library::apply_cell_action(
            &mut app,
            &ctx,
            i,
            crate::library::CellAction::VirtualCopy,
        );
        assert!(app.view == crate::AppView::Library, "it went to Develop");
        // ⚠ `pending` as well as `session`: opening starts a decode and the session arrives only
        // when it finishes, so a test reading `session` alone stayed green while the copy opened.
        assert!(
            app.session.is_none() && app.pending.is_none(),
            "it opened the photo"
        );
        let s = sidecar::load_all(&a);
        assert_eq!(
            s.get(2).map(|c| c.edits.clone()),
            Some(edited(1.0)),
            "copy 1's edits"
        );
        assert_eq!(
            active(&app),
            Some(PhotoId::new(a, 2)),
            "the new copy is selected"
        );
    }

    /// Photo → Create virtual copy in the Library copies the selected photo, and stays.
    #[test]
    fn the_menu_command_in_the_library_copies_the_selection() {
        let (_d, mut app, a) = with_copies("copy_cmd_library", 0);
        app.library.as_mut().unwrap().selection.click(0);
        app.create_virtual_copy(&egui::Context::default());
        assert!(app.view == crate::AppView::Library);
        assert!(app.session.is_none() && app.pending.is_none());
        assert!(sidecar::load_all(&a).get(1).is_some(), "copy 1 made");
    }

    /// The right-click offers to delete only a virtual copy, and counts several.
    #[test]
    fn the_menu_offers_delete_only_for_a_virtual_copy() {
        let a = PathBuf::from("/x/a.RAF");
        let source = LibrarySource::Folder(PathBuf::from("/x"));
        let labels = |targets: &[PhotoId]| -> Vec<String> {
            crate::test_ui::nodes(&crate::test_ui::ctx(), |ui| {
                let _ = crate::library::cell_menu(ui, None, &source, &Default::default(), targets);
            })
            .into_iter()
            .map(|n| n.label)
            .collect()
        };
        let original = labels(&[PhotoId::original(a.clone())]);
        assert!(
            original.iter().any(|l| l == "Edit"),
            "the sample: {original:?}"
        );
        assert!(
            !original.iter().any(|l| l.starts_with("Delete")),
            "{original:?}"
        );
        let one = labels(&[PhotoId::original(a.clone()), PhotoId::new(a.clone(), 1)]);
        assert!(one.iter().any(|l| l == "Delete virtual copy"), "{one:?}");
        let two = labels(&[PhotoId::new(a.clone(), 1), PhotoId::new(a, 2)]);
        assert!(
            two.iter().any(|l| l == "Delete 2 virtual copies"),
            "{two:?}"
        );
    }

    /// Deleting two selected copies from the Library is one undo step, keeps the original, and never
    /// leaves the Library — not on the delete and not on its undo.
    #[test]
    fn deleting_copies_in_the_library_is_one_step_and_stays_there() {
        let (_d, mut app, a) = with_copies("copy_delete_library", 2);
        let ctx = egui::Context::default();
        let lib = app.library.as_mut().unwrap();
        let (one, two) = (
            lib.entries.iter().position(|e| e.id.copy == 1).unwrap(),
            lib.entries.iter().position(|e| e.id.copy == 2).unwrap(),
        );
        lib.selection.click(one);
        lib.selection.toggle(two);
        crate::library::apply_cell_action(
            &mut app,
            &ctx,
            one,
            crate::library::CellAction::DeleteVirtualCopies,
        );
        let s = sidecar::load_all(&a);
        assert!(s.get(1).is_none() && s.get(2).is_none(), "both deleted");
        let copies: Vec<u32> = app
            .library
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.id.copy)
            .collect();
        assert_eq!(copies, vec![0], "the original stays");
        assert!(
            app.view == crate::AppView::Library && app.session.is_none() && app.pending.is_none()
        );
        app.undo_action(&ctx);
        let s = sidecar::load_all(&a);
        assert!(
            s.get(1).is_some() && s.get(2).is_some(),
            "one undo brings both back"
        );
        assert!(
            app.view == crate::AppView::Library && app.session.is_none() && app.pending.is_none(),
            "undo opened it"
        );
        assert!(!app.actions.can_undo(None), "one step");
    }
}
