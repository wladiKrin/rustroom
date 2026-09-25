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
use rustroom_app::immich_source;
use rustroom_app::immich_worker::Job;
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
        let Ok(scan) = catalog::scan_folder(&dir) else {
            self.status = "Could not read the folder".into();
            return;
        };

        let renumbered = self.compact_copy_numbering(&scan);

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

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::Edits;

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
}
