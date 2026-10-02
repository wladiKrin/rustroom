//! Photo → Rename… ⌘R and the right-click's Rename… (M10.8d): the selection's files renamed from a
//! template, in grid order, as one undo step.
//!
//! The names are the lib's `batch_rename`; the moving is Move to folder's — `move_planned` (two passes
//! when names chain) and `after_move` — so the JSON, a RAW's XMP, every catalog table, the selection
//! and the stacks follow, and Ctrl+Z is a `PhotoMove`.

use crate::modals::Modal;
use crate::RustroomApp;
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::batch_rename::{self, Photo};
use rustroom_app::filename_template::Template;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::status_text::photos;
use std::path::PathBuf;

impl RustroomApp {
    /// The photos behind `ids` in grid order, and how many were left out — Immich photos, which live
    /// in the cache under the server's names, and files that are not there.
    pub(crate) fn rename_targets(&self, ids: &[PhotoId]) -> (Vec<Photo>, usize) {
        let root = self.immich_cache_root();
        self.rename_targets_under(ids, root.as_deref())
    }

    /// [`Self::rename_targets`] with the Immich cache root given — a test's own directory rather than
    /// the user's cache (`move_files_under`'s shape).
    pub(crate) fn rename_targets_under(
        &self,
        ids: &[PhotoId],
        immich_root: Option<&std::path::Path>,
    ) -> (Vec<Photo>, usize) {
        let Some(lib) = &self.library else {
            return (Vec::new(), 0);
        };
        let row = |id: &PhotoId| {
            lib.entries
                .iter()
                .position(|e| e.id == *id)
                .unwrap_or(usize::MAX)
        };
        let mut ids = ids.to_vec();
        ids.sort_by_key(row);
        let in_cache = |f: &PathBuf| {
            immich_root.is_some_and(|r| rustroom_app::immich_source::is_immich_path(r, f))
        };
        let (keep, left): (Vec<PathBuf>, Vec<PathBuf>) = self
            .files_of(&ids)
            .into_iter()
            .partition(|f| f.exists() && !in_cache(f));
        let photos = batch_rename::photos_of(&keep)
            .into_iter()
            .map(|files| {
                let capture = self.capture_of_any(&files);
                Photo {
                    files,
                    taken: capture.taken,
                    camera: capture.camera,
                }
            })
            .collect();
        (photos, batch_rename::photos_of(&left).len())
    }

    /// What the camera recorded for a photo: the catalog's row of the first file that has one, else
    /// read from the first file (the catalog's background pass may not have reached it yet).
    fn capture_of_any(&self, files: &[PathBuf]) -> core_image::Capture {
        let known = self
            .catalog
            .as_ref()
            .and_then(|cat| files.iter().find_map(|f| cat.capture_of(f).ok().flatten()));
        known
            .or_else(|| {
                files
                    .first()
                    .map(|f| rustroom_app::metadata_scanner::read_capture(f))
            })
            .unwrap_or_default()
    }

    /// Photo → Rename… (⌘R): the dialog, on the selection.
    pub(crate) fn rename_selection(&mut self) {
        let ids = self.selection_ids();
        self.open_rename(&ids);
    }

    /// The Rename dialog on `ids` — or, with nothing that can be renamed, a word in the status row.
    pub(crate) fn open_rename(&mut self, ids: &[PhotoId]) {
        let (photos, left_out) = self.rename_targets(ids);
        self.open_rename_on(photos, left_out);
    }

    /// The Rename dialog on these photos, or the status row's word when there are none.
    fn open_rename_on(&mut self, photos: Vec<Photo>, left_out: usize) {
        if photos.is_empty() {
            self.status = if left_out > 0 {
                "Immich photos keep the server's names — move them to a folder to rename them"
                    .to_string()
            } else {
                "Select photos to rename".to_string()
            };
            return;
        }
        self.modal = Modal::Rename {
            photos,
            left_out,
            template: self.settings.rename_template.clone(),
            start: 1,
            planned: None,
        };
    }

    /// Rename `photos` by `template`, `{seq}` from `start` — one undo step. Returns how many photos
    /// were renamed; the template is remembered when one is.
    pub(crate) fn rename_photos(
        &mut self,
        ctx: &egui::Context,
        photo_list: &[Photo],
        template: &str,
        start: usize,
        left_out: usize,
    ) -> usize {
        let Ok(parsed) = Template::parse_for_rename(template) else {
            return 0;
        };
        self.persist_current_session();
        let plan = {
            let cat = self.catalog.as_ref();
            batch_rename::plan(photo_list, &parsed, start, |p| {
                crate::move_flow::path_taken(cat, p)
            })
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
        let n = batch_rename::photos_of(&done.iter().map(|(_, to)| to.clone()).collect::<Vec<_>>())
            .len();
        if n > 0 && self.settings.rename_template != template {
            self.settings.rename_template = template.to_string();
            self.save_settings();
        }
        let mut s = match n {
            0 => "Nothing renamed — every name is already what the template gives".to_string(),
            n => format!("Renamed {} — Ctrl+Z to undo", photos(n)),
        };
        if left_out > 0 {
            s.push_str(&format!(" · {} left out", photos(left_out)));
        }
        if let Some(f) = failed {
            s.push_str(&format!(" · could not rename {f}"));
        }
        self.status = s;
        n
    }
}

#[cfg(test)]
mod tests {
    use crate::test_app::{self, TestDir};
    use crate::LibrarySource;
    use rustroom_app::menu::Command;
    use std::path::{Path, PathBuf};

    /// A folder of `a.RAF a.JPG b.JPG c.JPG`, catalogued and open; `a.RAF` has an XMP and a JSON
    /// sidecar, and a title in the catalog; every photo is selected.
    fn folder(tag: &str) -> (TestDir, crate::RustroomApp) {
        let d = TestDir::new(tag);
        for n in ["a.RAF", "a.JPG", "b.JPG", "c.JPG"] {
            std::fs::write(d.0.join(n), n).unwrap();
        }
        let raw = d.0.join("a.RAF");
        std::fs::write(rustroom_app::xmp_sync::sidecar_path(&raw), "xmp").unwrap();
        let edits = edit_model::Edits {
            exposure: 1.0,
            ..Default::default()
        };
        rustroom_app::sidecar::persist_copy(&raw, 0, &edits).unwrap();
        let mut app = test_app::for_test();
        let scan = catalog::scan_folder(&d.0).unwrap();
        let cat = app.catalog.as_ref().unwrap();
        cat.sync_folder(&d.0, &scan).unwrap();
        app.open_scan(
            &egui::Context::default(),
            LibrarySource::Folder(d.0.clone()),
            scan,
        );
        // ⚠ After the open: an XMP never seen is imported then, and its empty description would
        // clear one set before — `xmp_sync::apply`'s rule, by design.
        app.catalog
            .as_ref()
            .unwrap()
            .set_description(
                &raw,
                &catalog::db::Description {
                    title: "Harbour".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let lib = app.library.as_mut().unwrap();
        let n = lib.entries.len();
        lib.selection.click(0);
        for i in 1..n {
            lib.selection.toggle(i);
        }
        (d, app)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn rename_all(app: &mut crate::RustroomApp, template: &str) -> usize {
        let ids = app.selection_ids();
        let (photos, left) = app.rename_targets(&ids);
        app.rename_photos(&egui::Context::default(), &photos, template, 1, left)
    }

    /// ⚠⚠ The selection renamed in GRID order — the pair as one photo — as one undo step, and undo
    /// puts every name, sidecar, XMP and catalog row back.
    #[test]
    fn the_selection_is_renamed_in_grid_order_and_undone_in_one_step() {
        let (d, mut app) = folder("rename_order");
        let cat = app.catalog.as_ref().unwrap();
        assert!(
            cat.description_of(&d.0.join("a.RAF")).unwrap().is_some(),
            "the sample: a title"
        );
        assert_eq!(rename_all(&mut app, "T-{seq}"), 3, "{}", app.status);
        assert_eq!(
            names(&d.0),
            [
                "T-1.JPG",
                "T-1.RAF",
                "T-1.RAF.json",
                "T-1.xmp",
                "T-2.JPG",
                "T-3.JPG"
            ]
        );
        assert_eq!(
            std::fs::read_to_string(d.0.join("T-2.JPG")).unwrap(),
            "b.JPG"
        );
        let cat = app.catalog.as_ref().unwrap();
        assert_eq!(
            cat.description_of(&d.0.join("T-1.RAF"))
                .unwrap()
                .map(|x| x.title)
                .as_deref(),
            Some("Harbour"),
            "the catalog followed"
        );
        assert!(app.status.starts_with("Renamed 3 photos"), "{}", app.status);

        crate::commands::execute(&mut app, &egui::Context::default(), Command::Undo);
        assert_eq!(
            names(&d.0),
            ["a.JPG", "a.RAF", "a.RAF.json", "a.xmp", "b.JPG", "c.JPG"],
            "one step undoes all of it"
        );
        let cat = app.catalog.as_ref().unwrap();
        assert!(cat.description_of(&d.0.join("a.RAF")).unwrap().is_some());
    }

    /// The renamed photos stay selected.
    #[test]
    fn the_selection_follows_the_rename() {
        let (d, mut app) = folder("rename_sel");
        rename_all(&mut app, "S-{seq}");
        let lib = app.library.as_ref().unwrap();
        let mut sel: Vec<PathBuf> = lib.selected_ids().into_iter().map(|i| i.path).collect();
        sel.sort();
        assert_eq!(sel.len(), 3, "{sel:?}");
        assert!(sel
            .iter()
            .all(|p| p.starts_with(&d.0)
                && p.file_name().unwrap().to_string_lossy().starts_with("S-")));
    }

    /// ⚠ A hand stack renamed stays a stack — the view's map is keyed by path.
    #[test]
    fn a_renamed_hand_stack_stays_a_stack() {
        let (_d, mut app) = folder("rename_stack");
        let lib = app.library.as_mut().unwrap();
        lib.selection = Default::default();
        lib.selection.click(1);
        lib.selection.toggle(2);
        crate::commands::execute(&mut app, &egui::Context::default(), Command::Stack);
        let rows = |app: &crate::RustroomApp| app.library.as_ref().unwrap().entries.len();
        assert_eq!(rows(&app), 2, "the sample: the pair, and b+c stacked");
        let lib = app.library.as_mut().unwrap();
        lib.selection.click(0);
        lib.selection.toggle(1);
        rename_all(&mut app, "K-{seq}");
        assert_eq!(rows(&app), 2, "still a stack after the rename");
    }

    /// ⚠ The drawn dialog: ⌘R and the right-click open it on the selection in grid order; on `{copy}`
    /// Rename is dark and a click changes nothing; on a valid template it renames and closes.
    #[test]
    fn the_dialog_renames_and_refuses_copy() {
        use crate::modals::Modal;
        let (d, mut app) = folder("rename_dialog");
        let ctx = crate::test_ui::ctx();
        crate::commands::execute(&mut app, &ctx, Command::RenamePhotos);
        let Modal::Rename { photos, .. } = &app.modal else {
            panic!("⌘R opens the dialog: {}", app.status);
        };
        assert_eq!(photos.len(), 3);
        assert_eq!(
            photos[0].files.len(),
            2,
            "the pair first, as the grid shows it"
        );
        app.modal = Modal::None;
        crate::library::apply_cell_action(
            &mut app,
            &ctx,
            0,
            crate::library::CellAction::RenameFiles,
        );
        let set = |app: &mut crate::RustroomApp, t: &str| {
            let Modal::Rename { template, .. } = &mut app.modal else {
                panic!("the right-click opens the dialog");
            };
            *template = t.to_string();
        };

        set(&mut app, "{name}-{copy}");
        crate::test_ui::click(&ctx, "Rename", |_| {
            crate::modals::rename::show(&mut app, &ctx)
        });
        assert!(matches!(app.modal, Modal::Rename { .. }), "still open");
        assert!(d.0.join("a.RAF").exists(), "nothing renamed");

        set(&mut app, "D-{seq}");
        crate::test_ui::click(&ctx, "Rename", |_| {
            crate::modals::rename::show(&mut app, &ctx)
        });
        assert!(matches!(app.modal, Modal::None), "closed");
        assert!(d.0.join("D-1.RAF").exists() && d.0.join("D-3.JPG").exists());
        assert_eq!(app.settings.rename_template, "D-{seq}", "remembered");
    }

    /// Immich photos are left out and counted; with nothing else, no dialog.
    #[test]
    fn immich_photos_are_left_out() {
        let (d, mut app) = folder("rename_immich");
        let ids = app.selection_ids();
        let (photos, left) = app.rename_targets_under(&ids, Some(&d.0));
        assert_eq!(
            (photos.len(), left),
            (0, 3),
            "everything here is in the cache"
        );
        app.open_rename_on(photos, left);
        assert!(matches!(app.modal, crate::modals::Modal::None));
        assert!(app.status.contains("Immich"), "{}", app.status);
    }
}
