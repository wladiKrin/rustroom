//! What the Library is showing: folders, All Photos, collections and keywords.
//!
//! Split out of `main.rs` unchanged. ⚠ Browsing **replaces** the source; it never narrows
//! the current one. `open_scan` is the single place a [`Library`] is constructed, which is
//! what stops the loaders drifting apart.
//!
//! The two query-driven sources — dates and smart collections — live in
//! [`crate::smart_sources`], and the derived state a loaded library carries in
//! [`crate::library_state`].

use crate::library_state::fs_modified;
use crate::{AppView, Library, LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::entries;
use rustroom_app::selection::Selection;
use rustroom_app::sidecar;
use rustroom_app::thumb_loader::ThumbLoader;
use std::path::PathBuf;

impl RustroomApp {
    pub(crate) fn open_folder(&mut self, ctx: &egui::Context) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.open_folder_at(ctx, dir);
    }

    /// Scan `dir` and show it. The half of `open_folder` that is not the file dialog.
    pub(crate) fn open_folder_at(&mut self, ctx: &egui::Context, dir: PathBuf) {
        match catalog::scan_folder(&dir) {
            Ok(mut scan) => {
                // ⚠ Ordered **before** expanding into entries. Sorting the scan is what keeps a
                // file's copies adjacent to it and ascending under every ordering — sorting
                // `entries` instead would scatter them.
                if self.settings.library_sort.needs_edited() {
                    for e in scan.iter_mut() {
                        e.edited = sidecar::edited_at(&e.path);
                    }
                }
                catalog::sort(&mut scan, self.settings.library_sort);
                // ⚠ The catalog records the *files*; copies are added when they are created
                // rather than scanned, because they live in the sidecar and not on disk.
                if let Some(cat) = &self.catalog {
                    if let Err(e) = cat.sync_folder(&dir, &scan) {
                        eprintln!("catalog: {e}");
                    }
                }
                let n = self.open_scan(ctx, LibrarySource::Folder(dir.clone()), scan);
                self.status = if n == 0 {
                    "No supported images in this folder.".into()
                } else {
                    format!("{n} photos")
                };
            }
            Err(e) => self.status = format!("Could not read folder: {e}"),
        }
    }

    /// Show every photo the catalog knows, from every folder ever opened.
    ///
    /// ⚠⚠ **This does not walk the filesystem.** The modification times come from the catalog,
    /// recorded when each folder was last scanned — which is what makes the view affordable at
    /// 10,000 photos, and is what `mtime` was captured for in M4.1 (nothing had read it until
    /// now). Consequence: a file changed outside Rustroom since its folder was last scanned shows
    /// a stale time here. Clicking that folder in the panel re-scans it and refreshes them.
    pub(crate) fn open_all_photos(&mut self, ctx: &egui::Context) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let rows = match cat.all_photos() {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("Could not read the catalog: {e}");
                return;
            }
        };
        // ⚠ A lookup built once, rather than a stat per photo. `s.max(0)` guards a negative unix
        // time, which `SystemTime` addition cannot represent — it would panic rather than sort
        // oddly.
        let mtimes: std::collections::HashMap<PathBuf, std::time::SystemTime> = rows
            .iter()
            .filter_map(|r| {
                r.mtime.map(|s| {
                    (
                        r.path.clone(),
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(s.max(0) as u64),
                    )
                })
            })
            .collect();
        let members: Vec<(PathBuf, u32)> = rows.into_iter().map(|r| (r.path, r.copy)).collect();
        let mut scan = rustroom_app::collections::scan_of(&members, sidecar::edited_at, |p| {
            mtimes
                .get(p)
                .copied()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(ctx, LibrarySource::AllPhotos, scan);
        self.status = if n == 0 {
            "The catalog is empty — open a folder to add photos.".into()
        } else {
            format!("All photos — {n}")
        };
    }

    /// Open a folder named by the catalog, re-scanning it.
    ///
    /// ⚠⚠ **A folder row re-scans; it does not browse from the catalog.** "One idiom for the whole
    /// panel" was the first instinct and it is wrong here: collections and keywords *span* folders
    /// and genuinely cannot scan, while a folder row names one real directory and can. So there is
    /// no such thing as a stale panel-browsed folder, refreshing needs no new button (click the
    /// row again), and `Reindex` keeps working because the source is a `Folder`.
    ///
    /// ⚠ An unreadable directory falls back to the catalog's last-seen rows rather than showing an
    /// empty folder. An unplugged drive must not look like the folder emptied itself — the rule
    /// `sync_folder` already follows in never deleting a photo row.
    pub(crate) fn open_catalog_folder(&mut self, ctx: &egui::Context, dir: &std::path::Path) {
        if catalog::scan_folder(dir).is_ok() {
            self.open_folder_at(ctx, dir.to_path_buf());
            return;
        }
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let names = cat.photos_in(dir).unwrap_or_default();
        let members: Vec<(PathBuf, u32)> = names.into_iter().map(|n| (dir.join(n), 0)).collect();
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(ctx, LibrarySource::Folder(dir.to_path_buf()), scan);
        self.status = format!(
            "{} is unreadable — showing {n} photos last seen",
            dir.display()
        );
    }

    /// Remove a folder from the catalog.
    ///
    /// ⚠ Deletes catalog rows only — the photos, their sidecars and every edit in them are
    /// untouched. Re-opening the folder catalogues it again, minus the library metadata.
    pub(crate) fn forget_folder(&mut self, ctx: &egui::Context, id: i64, path: &std::path::Path) {
        if let Some(cat) = &self.catalog {
            match cat.forget_folder(id) {
                // ⚠ Pushed so the deletion is undoable. This is the destructive one — it takes
                // the ratings, flags, labels and keyword assignments with it.
                Ok(snap) => {
                    let open = self.open_photo();
                    self.actions.push(
                        ActionKind::CatalogDelete {
                            snapshot: Box::new(snap),
                        },
                        open.as_ref(),
                    );
                }
                Err(e) => {
                    self.status = format!("Could not forget the folder: {e}");
                    return;
                }
            }
        }
        // ⚠ What is on screen may now be rows that exist nowhere. Two cases, and both need
        // handling: the folder itself, and All Photos, which was showing some of its photos.
        match self.library.as_ref().map(|l| l.source.clone()) {
            Some(LibrarySource::Folder(p)) if p == path => {
                // ⚠ Closed rather than left showing rows the catalog no longer has — the rule the
                // collection and keyword deletions already follow.
                self.library = None;
            }
            Some(LibrarySource::AllPhotos) => self.open_all_photos(ctx),
            _ => {}
        }
        self.status = format!("Forgot “{}”", path.display());
    }

    /// Build and show a `Library` from an already-sorted scan, returning how many rows it holds.
    ///
    /// ⚠⚠ **The one place a `Library` is constructed.** Three loaders had grown three copies of
    /// these fifteen lines between them, and a fourth was the point at which they became one — a
    /// new field on `Library` had to be added in three places, which is how two of them drift.
    ///
    /// ⚠ One row per (file, copy). Reading every sidecar costs 4.6 ms for 830 files, measured —
    /// small enough to stay inline rather than moving to the loader thread.
    pub(crate) fn open_scan(
        &mut self,
        ctx: &egui::Context,
        source: LibrarySource,
        scan: Vec<catalog::CatalogEntry>,
    ) -> usize {
        let entries = entries::build(&scan, sidecar::load_all);
        let n = entries.len();
        let ctx2 = ctx.clone();
        let space = self
            .settings
            .effective_display_space(self.os_manages_colour);
        let budget = self.settings.thumb_budget_mb as u64 * 1024 * 1024;
        let immich_budget = self.settings.immich_thumb_budget_mb as u64 * 1024 * 1024;
        let loader = ThumbLoader::new(space, budget, immich_budget, move || ctx2.request_repaint());
        self.library = Some(Library {
            source,
            scan,
            entries,
            cache: rustroom_app::thumb_cache::ThumbCache::with_default_budget(),
            last_want: Vec::new(),
            selection: Selection::default(),
            loader,
            visible: (0..n).collect(),
            marks: std::collections::HashMap::new(),
            captures: std::collections::HashMap::new(),
            band: None,
        });
        // ⚠ Applied explicitly rather than left to `reload_marks`, which early-returns when there
        // is no catalog — the filter must hold across a source switch whether or not one is open.
        let filter = self.filter.clone();
        if let Some(lib) = self.library.as_mut() {
            lib.refilter(&filter);
        }
        // ⚠⚠ **Before `reload_marks`.** A rating or keyword somebody set in Lightroom has to be
        // in the catalog before the marks map is read, or it would not show until something
        // unrelated rebuilt the Library. One `stat` per RAW, the cost the "last edited" sort
        // already pays for the JSON sidecar.
        self.import_xmp();
        self.reload_marks();
        self.reload_captures();
        self.queue_metadata();
        self.view = AppView::Library;
        n
    }

    /// Show a collection's photos, from wherever they live.
    ///
    /// ⚠⚠ The members are re-read from the catalog every time rather than cached: a photo added
    /// from another view must appear here without a restart.
    pub(crate) fn open_collection(&mut self, ctx: &egui::Context, id: i64, name: &str) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let members = match cat.collection_members(id) {
            Ok(m) => m,
            Err(e) => {
                self.status = format!("Could not read the collection: {e}");
                return;
            }
        };
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::Collection {
                id,
                name: name.to_string(),
            },
            scan,
        );
        self.status = if n == 0 {
            format!("“{name}” is empty")
        } else {
            format!("“{name}” — {n} photos")
        };
    }

    /// Show every photo carrying a keyword or any of its descendants.
    ///
    /// ⚠ The same shape as `open_collection`, deliberately: both are "a set of photos from
    /// anywhere", and giving them two different loaders is how two idioms grow for one concept.
    pub(crate) fn open_keyword(&mut self, ctx: &egui::Context, id: i64, name: &str) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let members = match cat.keyword_members(id) {
            Ok(m) => m,
            Err(e) => {
                self.status = format!("Could not read the keyword: {e}");
                return;
            }
        };
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::Keyword {
                id,
                name: name.to_string(),
            },
            scan,
        );
        self.status = if n == 0 {
            format!("Nothing tagged “{name}”")
        } else {
            format!("“{name}” — {n} photos")
        };
    }

    /// Queue whatever this source's folders have not had their capture metadata read yet.
    ///
    /// ⚠ Called on load only, never per frame — the `apply_view_demosaic` rule. The scanner
    /// de-duplicates anyway, but the query would not.
    ///
    /// ⚠ One query per **folder**, because a collection or All Photos spans several — the same
    /// shape `reload_marks` uses, and for the same reason.
    pub(crate) fn queue_metadata(&mut self) {
        let Some(cat) = &self.catalog else { return };
        let Some(lib) = self.library.as_ref() else {
            return;
        };
        let mut folders: Vec<PathBuf> = Vec::new();
        for e in &lib.entries {
            if let Some(dir) = e.id.path.parent() {
                if !folders.iter().any(|f| f == dir) {
                    folders.push(dir.to_path_buf());
                }
            }
        }
        let mut want: Vec<PathBuf> = Vec::new();
        for dir in &folders {
            if let Ok(names) = cat.files_without_capture(dir) {
                want.extend(names.into_iter().map(|n| dir.join(n)));
            }
        }
        if !want.is_empty() {
            self.metadata.scan(&want);
        }
    }

    /// Which keyword the Library is showing, if it is showing one.
    pub(crate) fn selected_keyword(&self) -> Option<i64> {
        match self.library.as_ref().map(|l| &l.source) {
            Some(LibrarySource::Keyword { id, .. }) => Some(*id),
            _ => None,
        }
    }

    /// Which collection the Library is showing, if it is showing one.
    pub(crate) fn selected_collection(&self) -> Option<i64> {
        match self.library.as_ref().map(|l| &l.source) {
            Some(LibrarySource::Collection { id, .. }) => Some(*id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};

    #[test]
    fn the_filter_survives_a_source_switch() {
        // ⚠⚠ Browsing **replaces** the source, so filter-then-pick-a-date is the only way to say
        // "five-star picks from the 9th". Rebuilding the filter on every switch destroyed exactly
        // the composition that browse-replaces-source otherwise denies — which is why the original
        // reset, argued for on the grounds that a stale filter "reads as breakage", was wrong in
        // use.
        let dir = test_app::TestDir::new("filterkeep");
        std::fs::write(dir.0.join("a.RAF"), b"x").unwrap();
        std::fs::write(dir.0.join("b.RAF"), b"x").unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();

        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(app.library.as_ref().unwrap().visible.len(), 2);

        app.filter.text = "a.RAF".into();
        let f = app.filter.clone();
        app.library.as_mut().unwrap().refilter(&f);
        assert_eq!(app.library.as_ref().unwrap().visible.len(), 1);

        // Re-open the same folder — a source switch by any other name.
        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(
            app.filter.text, "a.RAF",
            "the search text was reset by a source switch"
        );
        assert_eq!(
            app.library.as_ref().unwrap().visible.len(),
            1,
            "the filter was not applied to the new source"
        );
    }

    /// Write `stem.xmp` beside `photo`, carrying `rating`.
    fn put_xmp(photo: &std::path::Path, rating: u8) {
        let text = xmp::write(
            &xmp::Sidecar {
                rating: Some(rating),
                ..xmp::Sidecar::default()
            },
            None,
        )
        .unwrap();
        std::fs::write(rustroom_app::xmp_sync::sidecar_path(photo), text).unwrap();
    }

    #[test]
    fn a_raw_rated_elsewhere_imports_when_the_folder_opens() {
        // ⚠⚠ The milestone's own "done when": a folder rated in Lightroom opens here with its
        // stars. End to end through the real `open_scan`, with files on disk.
        let dir = test_app::TestDir::new("xmpopen");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"x").unwrap();
        put_xmp(&photo, 4);
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();

        app.open_catalog_folder(&ctx, &dir.0);

        let id = rustroom_app::photo_id::PhotoId::original(photo);
        assert_eq!(
            app.library
                .as_ref()
                .unwrap()
                .marks
                .get(&id)
                .map(|m| m.rating),
            Some(4),
            "the sidecar's rating did not reach the Library"
        );
    }

    #[test]
    fn rating_a_raw_writes_its_sidecar_and_does_not_re_import_on_the_next_open() {
        // ⚠ Both halves in one test on purpose: a write that did not record its own mtime would
        // still produce a file, so "a sidecar appeared" alone cannot see the bug.
        let dir = test_app::TestDir::new("xmpwrite");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"x").unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.open_catalog_folder(&ctx, &dir.0);

        let id = rustroom_app::photo_id::PhotoId::original(photo.clone());
        app.apply_mark_to(
            std::slice::from_ref(&id),
            rustroom_app::marks::MarkAction::Rating(3),
        );

        let side = rustroom_app::xmp_sync::sidecar_path(&photo);
        let text = std::fs::read_to_string(&side).expect("no sidecar was written");
        assert_eq!(xmp::read(&text).unwrap().rating, Some(3));

        // Re-opening must not treat our own write as a foreign edit.
        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(
            app.library
                .as_ref()
                .unwrap()
                .marks
                .get(&id)
                .map(|m| m.rating),
            Some(3)
        );
    }

    #[test]
    fn a_jpeg_gets_no_sidecar_when_rated() {
        let dir = test_app::TestDir::new("xmpjpeg");
        let photo = dir.0.join("a.jpg");
        std::fs::write(&photo, b"x").unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.open_catalog_folder(&ctx, &dir.0);

        app.apply_mark_to(
            &[rustroom_app::photo_id::PhotoId::original(photo.clone())],
            rustroom_app::marks::MarkAction::Rating(5),
        );
        assert!(
            !rustroom_app::xmp_sync::sidecar_path(&photo).exists(),
            "a JPEG got a sidecar Lightroom would never look for"
        );
    }

    #[test]
    fn the_write_setting_is_honoured() {
        let dir = test_app::TestDir::new("xmpoff");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"x").unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.settings.write_xmp = false;
        app.open_catalog_folder(&ctx, &dir.0);

        app.apply_mark_to(
            &[rustroom_app::photo_id::PhotoId::original(photo.clone())],
            rustroom_app::marks::MarkAction::Rating(5),
        );
        assert!(
            !rustroom_app::xmp_sync::sidecar_path(&photo).exists(),
            "a sidecar was written with the setting off"
        );
        // ⚠ And the mark still reached the catalog: the setting governs the sidecar, never the
        // rating itself.
        let id = rustroom_app::photo_id::PhotoId::original(photo);
        assert_eq!(
            app.library
                .as_ref()
                .unwrap()
                .marks
                .get(&id)
                .map(|m| m.rating),
            Some(5)
        );
    }

    #[test]
    fn reading_still_happens_with_writing_turned_off() {
        // ⚠ The asymmetry, stated as a test: turning writing off must not blind Rustroom to
        // what another application did — reading can destroy nothing.
        let dir = test_app::TestDir::new("xmpreadonly");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"x").unwrap();
        put_xmp(&photo, 2);
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.settings.write_xmp = false;

        app.open_catalog_folder(&ctx, &dir.0);

        let id = rustroom_app::photo_id::PhotoId::original(photo);
        assert_eq!(
            app.library
                .as_ref()
                .unwrap()
                .marks
                .get(&id)
                .map(|m| m.rating),
            Some(2)
        );
    }

    #[test]
    fn a_catalog_folder_row_rescans_the_directory() {
        // ⚠⚠ The whole reason a folder row differs from a collection or keyword row, and the
        // reason there is no "Refresh" button: browsing a folder from the panel *is* scanning it.
        // Without this the row would show whatever the catalog last recorded, and a file added
        // outside Rustroom would never appear — which reads as a bug.
        //
        // ⚠ Written because the sabotage step found nothing covered it: forcing
        // `open_catalog_folder` down its catalog-fallback path left the whole suite green.
        let dir = test_app::TestDir::new("rescan");
        std::fs::write(dir.0.join("a.RAF"), b"x").unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();

        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(
            app.library.as_ref().unwrap().entries.len(),
            1,
            "the folder did not load"
        );

        std::fs::write(dir.0.join("b.RAF"), b"x").unwrap();
        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(
            app.library.as_ref().unwrap().entries.len(),
            2,
            "the row did not re-scan — a file added outside Rustroom stayed invisible"
        );
        // ⚠ And the source must stay a `Folder`, or `Reindex` would start refusing on it.
        assert!(matches!(
            app.library.as_ref().unwrap().source,
            LibrarySource::Folder(_)
        ));
    }
}
