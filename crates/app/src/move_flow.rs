//! Right-click → Move to folder… (2026-10-01): the picker, the catalog, the Library, the undo.
//!
//! The names and the bytes are the lib's `photo_move`; this is what the app does around them. ⚠ The
//! open photo's edits are saved **before** anything moves — afterwards they would land beside the old
//! name and be orphaned (`rename_file`'s rule).

use crate::library_state::fs_modified;
use crate::{AppView, LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::photo_move::{self, Planned};
use rustroom_app::sidecar;
use rustroom_app::status_text::photos;
use std::path::{Path, PathBuf};

impl RustroomApp {
    /// The right-click's Move to folder…: the files the targets stand for, the system folder picker
    /// (which can make a folder), then the move.
    pub(crate) fn move_targets_to_folder(&mut self, ctx: &egui::Context, clicked: usize) {
        let files = self.files_to_move(clicked);
        if files.is_empty() {
            return;
        }
        let Some(dest) = rfd::FileDialog::new()
            .set_title("Move to folder")
            .pick_folder()
        else {
            return;
        };
        self.move_files(ctx, &files, &dest);
    }

    /// The files a right-click's targets stand for — `files_of` (`stack_ops`).
    pub(crate) fn files_to_move(&self, clicked: usize) -> Vec<PathBuf> {
        let targets = self
            .library
            .as_ref()
            .map(|l| crate::library::menu_targets(l, clicked))
            .unwrap_or_default();
        self.files_of(&targets)
    }

    /// Move `files` into `dest`, as one undo step. Returns how many photos moved.
    pub(crate) fn move_files(
        &mut self,
        ctx: &egui::Context,
        files: &[PathBuf],
        dest: &Path,
    ) -> usize {
        let root = self.immich_cache_root();
        self.move_files_under(ctx, files, dest, root.as_deref())
    }

    /// [`Self::move_files`] with the Immich cache root given — a test's own directory rather than the
    /// user's cache.
    pub(crate) fn move_files_under(
        &mut self,
        ctx: &egui::Context,
        files: &[PathBuf],
        dest: &Path,
        immich_root: Option<&Path>,
    ) -> usize {
        self.persist_current_session();
        // ⚠ An Immich asset not downloaded yet has no file to move: skipped, and counted.
        let (present, missing): (Vec<PathBuf>, Vec<PathBuf>) =
            files.iter().cloned().partition(|f| f.exists());
        let plan = {
            let cat = self.catalog.as_ref();
            photo_move::plan(
                &present,
                dest,
                |p| wanted_name(p, immich_root, cat),
                |p| path_taken(cat, p),
            )
        };
        let (done, failed) = self.move_planned(&plan);
        if !done.is_empty() {
            self.actions.push(
                ActionKind::PhotoMove {
                    moves: done.clone(),
                },
                None,
            );
        }
        self.after_move(ctx, &done);
        let n = photo_count(&done);
        self.status = status(n, dest, missing.len(), failed);
        n
    }

    /// Move the planned files — two passes when names chain (`photo_move::move_all`) — and the
    /// catalog's rows of those that moved, in one transaction. Returns the moves made and the first
    /// failure, said for the status row.
    ///
    /// ⚠ Shared by Move to folder, Rename and the undo of both (M10.8d), so a swap undoes too.
    pub(crate) fn move_planned(
        &self,
        plan: &[Planned],
    ) -> (Vec<(PathBuf, PathBuf)>, Option<String>) {
        let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut failed: Option<String> = None;
        for (p, r) in plan.iter().zip(photo_move::move_all(plan)) {
            match r {
                Ok(()) => done.push((p.from.clone(), p.to.clone())),
                Err(e) => {
                    failed.get_or_insert_with(|| format!("{}: {e}", shown(&p.from)));
                }
            }
        }
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.move_photos(&done) {
                eprintln!("catalog: {e}");
            }
        }
        (done, failed)
    }

    /// Everything a move leaves stale, put right: the decode cache, the undo state of the old paths,
    /// the Library and the open photo.
    pub(crate) fn after_move(&mut self, ctx: &egui::Context, moves: &[(PathBuf, PathBuf)]) {
        if moves.is_empty() {
            return;
        }
        for (from, to) in moves {
            self.cache.rekey(from, to);
            // ⚠ Every copy of a moved file is a different `PhotoId` now. Its in-memory history and
            // log entries name the old path — dropped, since the persisted history moved with the
            // catalog row and is read again when the photo opens.
            let mut copies: Vec<u32> = sidecar::load_all(to)
                .copies
                .iter()
                .map(|c| c.copy)
                .collect();
            copies.push(0);
            for n in copies {
                let old = PhotoId::new(from.clone(), n);
                self.histories.forget(&old);
                self.actions.forget_edits_of(&old);
            }
        }
        let moved_to = |p: &Path| moves.iter().find(|(f, _)| f == p).map(|(_, t)| t.clone());
        if let Some(lib) = self.library.as_mut() {
            // ⚠ The rows first, in every view: the rebuild below restores the selection BY ID, so
            // a renamed photo stays selected only if its row already carries the new path (M10.8d).
            for e in lib.entries.iter_mut() {
                if let Some(to) = moved_to(&e.id.path) {
                    e.id.path = to;
                }
            }
            match lib.source.clone() {
                // ⚠ A folder is its directory: what left it goes, what an undo brings back comes.
                LibrarySource::Folder(dir) => {
                    lib.scan.retain(|e| moved_to(&e.path).is_none());
                    for (_, to) in moves {
                        if to.parent() == Some(dir.as_path())
                            && !lib.scan.iter().any(|e| &e.path == to)
                        {
                            lib.scan.extend(rustroom_app::collections::scan_of(
                                &[(to.clone(), 0)],
                                sidecar::edited_at,
                                fs_modified,
                            ));
                        }
                    }
                }
                // ⚠ The album still lists the asset, under its cache path: it shows as not
                // downloaded, and opening it there downloads a fresh copy.
                LibrarySource::ImmichAlbum { .. } | LibrarySource::ImmichBucket { .. } => {}
                // Every catalog view shows the photos where they are now, selection kept.
                _ => {
                    for e in lib.scan.iter_mut() {
                        if let Some(to) = moved_to(&e.path) {
                            e.file_name = shown(&to);
                            e.path = to;
                        }
                    }
                    if let Some(copies) = lib.copies.as_mut() {
                        *copies = copies
                            .drain()
                            .map(|id| match moved_to(&id.path) {
                                Some(to) => PhotoId::new(to, id.copy),
                                None => id,
                            })
                            .collect();
                    }
                }
            }
        }
        self.rename_draft = None;
        // ⚠ The catalog moved the stack rows with the files; the view's map is keyed by path.
        self.reload_hand_stacks();
        if let Some(open) = self.open_photo() {
            if let Some(to) = moved_to(&open.path) {
                // Saved before the move; reopened where it is now, and only where it is being looked at.
                self.session = None;
                if self.view == AppView::Develop {
                    self.open_path(ctx, PhotoId::new(to, open.copy));
                }
            }
        }
        self.rebuild_entries(None);
    }

    /// Undo (`forward: false`) or redo a [`ActionKind::PhotoMove`]: every file back where it was, or
    /// where it went. A way back that has been taken since is skipped and said.
    pub(crate) fn replay_photo_move(
        &mut self,
        ctx: &egui::Context,
        kind: &ActionKind,
        forward: bool,
    ) {
        let ActionKind::PhotoMove { moves } = kind else {
            return;
        };
        self.persist_current_session();
        let plan: Vec<Planned> = moves
            .iter()
            .map(|(a, b)| {
                let (from, to) = if forward { (a, b) } else { (b, a) };
                Planned {
                    from: from.clone(),
                    to: to.clone(),
                }
            })
            .collect();
        let (done, _) = self.move_planned(&plan);
        let stuck = plan.len() - done.len();
        self.after_move(ctx, &done);
        self.status = match (stuck, forward) {
            (0, true) => "Redone".to_string(),
            (0, false) => "Undone".to_string(),
            (n, _) => format!("{n} could not be moved — the place is taken"),
        };
    }
}

/// Whether a photo may not move onto `p`: a file is there, or its JSON sidecar, or — for a RAW — its
/// XMP (the move would refuse it and put the photo back), or a catalog row of a vanished file.
///
/// ⚠ Shared by Move to folder and Rename (M10.8d), so the two cannot disagree about what is free.
pub(crate) fn path_taken(cat: Option<&catalog::db::Catalog>, p: &Path) -> bool {
    p.exists()
        || sidecar::path(p).exists()
        || (rustroom_app::xmp_sync::takes_sidecar(p)
            && rustroom_app::xmp_sync::sidecar_path(p).exists())
        || cat.is_some_and(|c| c.knows(p).unwrap_or(false))
}

/// The name a file wants in its new folder: an Immich download's original, else its own.
fn wanted_name(
    path: &Path,
    immich_root: Option<&Path>,
    cat: Option<&catalog::db::Catalog>,
) -> String {
    let in_cache =
        immich_root.is_some_and(|r| rustroom_app::immich_source::is_immich_path(r, path));
    let original = if in_cache {
        cat.and_then(|c| c.original_name(path).ok().flatten())
    } else {
        None
    };
    original.unwrap_or_else(|| shown(path))
}

fn shown(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Photos, not files: a RAW+JPEG pair that moved together is one.
fn photo_count(moves: &[(PathBuf, PathBuf)]) -> usize {
    // ⚠ A hash set, not a `Vec` searched per move — a move can be the whole selection (M10.14e).
    moves
        .iter()
        .map(|(_, to)| {
            (
                to.parent(),
                to.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        })
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// What a move says in the status row.
fn status(n: usize, dest: &Path, missing: usize, failed: Option<String>) -> String {
    let folder = shown(dest);
    let mut s = match n {
        0 => "Nothing moved".to_string(),
        n => format!("Moved {} to “{folder}” — Ctrl+Z to undo", photos(n)),
    };
    match missing {
        0 => {}
        1 => s.push_str(" · 1 isn't downloaded yet"),
        m => s.push_str(&format!(" · {m} aren't downloaded yet")),
    }
    if let Some(f) = failed {
        s.push_str(&format!(" · could not move {f}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use crate::test_app::{self, TestDir};
    use crate::LibrarySource;
    use rustroom_app::photo_id::PhotoId;
    use rustroom_app::sidecar;
    use std::path::{Path, PathBuf};

    /// `a/` holding `x.RAF` (rated 4, edited, with a copy) and `y.RAF`, open as a folder; `b/` empty.
    fn two_folders(tag: &str) -> (TestDir, crate::RustroomApp, PathBuf, PathBuf) {
        let dir = TestDir::new(tag);
        let (a, b) = (dir.0.join("a"), dir.0.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        for n in ["x.RAF", "y.RAF"] {
            std::fs::write(a.join(n), b"not a real raw").unwrap();
        }
        let x = a.join("x.RAF");
        let edits = edit_model::Edits {
            exposure: 1.0,
            ..Default::default()
        };
        sidecar::persist_copy(&x, 1, &edits).unwrap();
        let mut app = test_app::for_test();
        let scan = catalog::scan_folder(&a).unwrap();
        let cat = app.catalog.as_ref().unwrap();
        cat.sync_folder(&a, &scan).unwrap();
        let marks = catalog::db::Marks {
            rating: 4,
            ..Default::default()
        };
        cat.set_marks(&x, 0, marks).unwrap();
        app.open_scan(&egui::Context::default(), LibrarySource::Folder(a), scan);
        (dir, app, x, b)
    }

    fn shown_paths(app: &crate::RustroomApp) -> Vec<PathBuf> {
        let lib = app.library.as_ref().unwrap();
        let mut v: Vec<PathBuf> = lib.entries.iter().map(|e| e.id.path.clone()).collect();
        v.dedup();
        v
    }

    /// ⚠⚠ The whole move from a folder view: the file and its sidecar go, the rating follows in the
    /// catalog, the photo leaves the view — and one undo brings all of it back.
    #[test]
    fn a_move_takes_the_file_its_edits_and_its_marks_and_undoes() {
        let (_d, mut app, x, b) = two_folders("move_folder");
        let ctx = egui::Context::default();
        assert!(shown_paths(&app).contains(&x), "the sample");
        let n = app.move_files(&ctx, std::slice::from_ref(&x), &b);
        let to = b.join("x.RAF");
        assert_eq!(n, 1, "{}", app.status);
        assert!(to.exists() && !x.exists());
        assert!(
            sidecar::load_all(&to).get(1).is_some(),
            "the copy went with it"
        );
        let cat = app.catalog.as_ref().unwrap();
        assert_eq!(cat.marks(&to, 0).unwrap().rating, 4, "the rating followed");
        assert!(!shown_paths(&app).contains(&x), "it left the folder view");
        assert!(
            app.status.starts_with("Moved 1 photo to “b”"),
            "{}",
            app.status
        );

        app.undo_action(&ctx);
        assert!(x.exists() && !to.exists(), "{}", app.status);
        assert!(
            sidecar::load_all(&x).get(1).is_some(),
            "the sidecar came back"
        );
        let cat = app.catalog.as_ref().unwrap();
        assert_eq!(cat.marks(&x, 0).unwrap().rating, 4);
        assert!(
            shown_paths(&app).contains(&x),
            "the undo brought it back into view"
        );
    }

    /// An Immich download leaves the cache under the name it had on the phone; one not downloaded is
    /// skipped and counted.
    #[test]
    fn an_immich_download_takes_its_original_name() {
        let dir = TestDir::new("move_immich");
        let root = dir.0.join("immich");
        let dest = dir.0.join("photos");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        let mut app = test_app::for_test();
        let assets = [asset("3f2a", "IMG_1.jpg"), asset("9c1b", "IMG_2.jpg")];
        let scan = rustroom_app::immich_source::entries_from_assets(&assets, &root);
        rustroom_app::immich_catalog::record_assets(app.catalog.as_ref().unwrap(), &root, &scan)
            .unwrap();
        let (got, absent) = (scan[0].path.clone(), scan[1].path.clone());
        std::fs::write(&got, b"jpeg bytes").unwrap();
        let ctx = egui::Context::default();
        let n = app.move_files_under(&ctx, &[got.clone(), absent], &dest, Some(&root));
        assert_eq!(n, 1);
        assert!(dest.join("IMG_1.jpg").exists(), "{}", app.status);
        assert!(!got.exists(), "it left the cache");
        assert!(
            app.status.contains("1 isn't downloaded yet"),
            "{}",
            app.status
        );
    }

    fn asset(id: &str, name: &str) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            original_file_name: name.to_string(),
            original_mime_type: None,
            width: Some(1),
            height: Some(1),
            thumbhash: None,
            file_modified_at: "2025-01-01T00:00:00.000Z".into(),
            updated_at: "2025-01-02T00:00:00.000Z".into(),
        }
    }

    /// A collapsed RAW+JPEG stack is one row and moves as both files.
    #[test]
    fn a_collapsed_pair_moves_together() {
        let dir = TestDir::new("move_pair");
        let (a, b) = (dir.0.join("a"), dir.0.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        for n in ["p.RAF", "p.JPG"] {
            std::fs::write(a.join(n), b"bytes").unwrap();
        }
        let mut app = test_app::for_test();
        app.settings.stacking.enabled = true;
        let scan = catalog::scan_folder(&a).unwrap();
        let ctx = egui::Context::default();
        app.open_scan(&ctx, LibrarySource::Folder(a.clone()), scan);
        assert_eq!(
            app.library.as_ref().unwrap().entries.len(),
            1,
            "the sample: one row"
        );
        let files = app.files_to_move(0);
        assert_eq!(files.len(), 2, "{files:?}");
        app.move_files(&ctx, &files, &b);
        assert!(b.join("p.RAF").exists() && b.join("p.JPG").exists());
        assert!(
            app.status.starts_with("Moved 1 photo"),
            "a pair is one photo: {}",
            app.status
        );
    }

    /// The open photo moved from the Library: its session (saved first) closes, and nothing opens.
    #[test]
    fn the_open_photo_closes_in_the_library() {
        let (_d, mut app, x, b) = two_folders("move_open");
        let ctx = egui::Context::default();
        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        app.session.as_mut().unwrap().path = x.clone();
        app.view = crate::AppView::Library;
        assert_eq!(
            app.open_photo(),
            Some(PhotoId::original(x.clone())),
            "the sample"
        );
        app.move_files(&ctx, std::slice::from_ref(&x), &b);
        assert!(app.session.is_none() && app.pending.is_none());
        assert!(Path::new(&b.join("x.RAF")).exists());
    }

    #[test]
    fn the_menu_offers_move_to_folder() {
        let lib = test_app::library_of(1);
        let targets = vec![lib.entries[0].id.clone()];
        let labels: Vec<String> = crate::test_ui::nodes(&crate::test_ui::ctx(), |ui| {
            let _ = crate::library::cell_menu(ui, None, &lib.source, &Default::default(), &targets);
        })
        .into_iter()
        .map(|n| n.label)
        .collect();
        assert!(labels.iter().any(|l| l == "Move to folder…"), "{labels:?}");
    }
}
