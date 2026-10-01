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
use rustroom_app::photo_id::PhotoId;
use rustroom_app::selection::Selection;
use rustroom_app::sidecar;
use rustroom_app::thumb_loader::ThumbLoader;
use std::path::PathBuf;

/// A tree scan and the `(from, to)` moves it recognised — what `scan_and_relink` returns.
type Scanned = (Vec<catalog::CatalogEntry>, Vec<(PathBuf, PathBuf)>);

impl RustroomApp {
    pub(crate) fn open_folder(&mut self, ctx: &egui::Context) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.open_folder_at(ctx, dir);
    }

    /// Scan `dir`'s tree into the catalog, relinking photos moved on disk — shared by opening a
    /// folder and Reindex, so the two cannot disagree about what is there (M9.9; Reindex joined it
    /// at the author's request). Returns the scan and the `(from, to)` moves, or `None` — with the
    /// status set — when `dir` cannot be read.
    ///
    /// Per folder, BEFORE its new rows are added: a file new to it that is a photo moved from
    /// another known folder is relinked (`Catalog::relink_moved`), its left-behind sidecars
    /// following it (`move_tracking::carry_sidecars`).
    pub(crate) fn scan_and_relink(&mut self, dir: &std::path::Path) -> Option<Scanned> {
        let tree = match catalog::scan_tree(dir) {
            Ok(t) => t,
            Err(e) => {
                self.status = format!("Could not read folder: {e}");
                return None;
            }
        };
        let mut all_moved = Vec::new();
        let mut scan = Vec::new();
        for (folder, entries) in tree {
            if let Some(cat) = &self.catalog {
                // ⚠ `gone` is the filesystem's answer: a surviving original makes the new file a
                // copy, and the catalog itself checks no files.
                match cat.relink_moved(&folder, &entries, |p| !p.exists()) {
                    Ok(moved) => {
                        for (from, to) in &moved {
                            rustroom_app::move_tracking::carry_sidecars(from, to);
                        }
                        all_moved.extend(moved);
                    }
                    Err(e) => eprintln!("catalog: {e}"),
                }
                // ⚠ The catalog records the *files*; copies are added when they are created
                // rather than scanned, because they live in the sidecar and not on disk.
                if let Err(e) = cat.sync_folder(&folder, &entries) {
                    eprintln!("catalog: {e}");
                }
            }
            scan.extend(entries);
        }
        Some((scan, all_moved))
    }

    /// Scan `dir` and everything beneath it, and show it all. The half of `open_folder` that is not
    /// the file dialog.
    ///
    /// ⚠⚠ M9.9: **the whole tree** (`catalog::scan_tree`) — each subfolder holding images becomes a
    /// catalog folder, and the view shows `dir` with everything beneath (the author's choice,
    /// Lightroom's default). And per folder, BEFORE its new rows are added, a file new to it that
    /// is a photo moved from another known folder is relinked (`Catalog::relink_moved`), its
    /// left-behind sidecars following it (`move_tracking::carry_sidecars`).
    pub(crate) fn open_folder_at(&mut self, ctx: &egui::Context, dir: PathBuf) {
        let Some((mut scan, moved)) = self.scan_and_relink(&dir) else {
            return;
        };
        let relinked = moved.len();
        // ⚠ Ordered **before** expanding into entries. Sorting the scan is what keeps a file's
        // copies adjacent to it and ascending under every ordering — sorting `entries` instead
        // would scatter them. After the sidecars moved, so "edited" reads where they are now.
        if self.settings.library_sort.needs_edited() {
            for e in scan.iter_mut() {
                e.edited = sidecar::edited_at(&e.path);
            }
        }
        catalog::sort(&mut scan, self.settings.library_sort);
        let files = scan.len();
        let n = self.open_scan(ctx, LibrarySource::Folder(dir.clone()), scan);
        // ⚠ Both numbers when stacking hides some: the rail counts files and the grid counts
        // stacks, and "Katzen 18" beside "9 photos" read as a disagreement (A-10).
        let mut status = if n == 0 {
            "No supported images in this folder.".to_string()
        } else if n == files {
            format!("{n} photos")
        } else {
            format!("{n} photos · {files} files")
        };
        status.push_str(&rustroom_app::move_tracking::recognised(relinked));
        self.status = status;
        self.settings.remember_folder(&dir);
        self.save_settings();
        #[cfg(target_os = "macos")]
        crate::native_menu::refresh(ctx, &self.settings.recent_folders);
    }

    /// Show every photo the catalog knows, from every folder ever opened.
    ///
    /// ⚠⚠ **This does not walk the filesystem.** The modification times come from the catalog,
    /// recorded when each folder was last scanned — which is what makes the view affordable at
    /// 10,000 photos, and is what `mtime` was captured for in M4.1 (nothing had read it until
    /// now). Consequence: a file changed outside Rustroom since its folder was last scanned shows
    /// a stale time here. Clicking that folder in the panel re-scans it and refreshes them.
    /// A drop on the window: the first folder opens as a folder; failing that, the first
    /// supported photo opens in Develop, as File → Open file… would.
    pub(crate) fn open_dropped(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if let Some(dir) = paths.iter().find(|p| p.is_dir()) {
            self.open_folder_at(ctx, dir.clone());
            return;
        }
        if let Some(file) = paths.iter().find(|p| catalog::is_supported(p)) {
            self.open_path(ctx, rustroom_app::photo_id::PhotoId::original(file.clone()));
            self.library = None;
            self.view = AppView::Develop;
        }
    }

    pub(crate) fn open_all_photos(&mut self, ctx: &egui::Context) {
        // ⚠ Read before `cat` borrows `self`.
        let immich_root = self.immich_cache_root();
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
        // ⚠⚠ An Immich row exists as soon as its album is browsed, downloaded or not. A
        // catalog-wide view shows the ones that are really here — the user's call, and the same line
        // he drew for slice 2: a photo downloaded from Immich becomes local and joins everything else.
        //
        // ⚠ Through `row_is_browsable`, shared with the other catalog-wide reader, so the two cannot
        // drift. ⚠ It checks **Immich** rows only: an absent local file stays visible, because the
        // unplugged-drive rule is a different question.
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|r| {
                rustroom_app::immich_catalog::row_is_browsable(&r.path, immich_root.as_deref())
            })
            .collect();
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
            // ⚠ M9.9: the folder AND every folder beneath it — the tree shows them as one, and
            // forgetting a parent while its children stayed would leave them re-rooted, surprising.
            // One undo step: a `Batch` of the deletions, the shape a paste already uses.
            let folders = cat.folders().unwrap_or_default();
            let mut ids: Vec<i64> = rustroom_app::folder_tree::subtree(&folders, path)
                .map(|f| f.id)
                .collect();
            if ids.is_empty() {
                ids.push(id);
            }
            let mut parts = Vec::new();
            for fid in ids {
                match cat.forget_folder(fid) {
                    // ⚠ Pushed so the deletion is undoable. This is the destructive one — it takes
                    // the ratings, flags, labels and keyword assignments with it.
                    Ok(snap) => parts.push(ActionKind::CatalogDelete {
                        snapshot: Box::new(snap),
                    }),
                    Err(e) => {
                        self.status = format!("Could not forget the folder: {e}");
                        break;
                    }
                }
            }
            let open = self.open_photo();
            match parts.len() {
                0 => return,
                1 => self.actions.push(parts.remove(0), open.as_ref()),
                _ => self.actions.push(ActionKind::Batch(parts), open.as_ref()),
            }
        }
        // ⚠ What is on screen may now be rows that exist nowhere. Two cases, and both need
        // handling: the folder itself, and All Photos, which was showing some of its photos.
        match self.library.as_ref().map(|l| l.source.clone()) {
            Some(LibrarySource::Folder(p)) if p.starts_with(path) => {
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
        self.open_scan_with(ctx, source, scan, None)
    }

    /// [`Self::open_scan`] for a source that holds copies rather than files: only `copies` are shown
    /// (`Library::copies`).
    pub(crate) fn open_scan_with(
        &mut self,
        ctx: &egui::Context,
        source: LibrarySource,
        scan: Vec<catalog::CatalogEntry>,
        copies: Option<std::collections::HashSet<PhotoId>>,
    ) -> usize {
        // ⚠⚠ **Through `browser_entries`, never a bare `entries::build`.** This is the
        // folder-open path, and building the rows here without grouping is exactly the bug that
        // shipped: stacking worked after a re-sort and did nothing when a folder was opened.
        let hand_stacks = crate::library_state::hand_stacks_of(self.catalog.as_ref(), &scan);
        let (entries, below) = crate::library_state::browser_entries(
            &scan,
            &self.settings.stacking,
            &Default::default(),
            &self.immich_stacks.of_asset,
            copies.as_ref(),
            &hand_stacks,
        );
        let n = entries.len();
        let ctx2 = ctx.clone();
        let space = self
            .settings
            .effective_display_space(self.os_manages_colour);
        let budget = self.settings.thumb_budget_mb as u64 * 1024 * 1024;
        let immich_budget = self.settings.immich_thumb_budget_mb as u64 * 1024 * 1024;
        let loader = ThumbLoader::new(space, budget, immich_budget, move || ctx2.request_repaint());
        // ⚠ Read here, before `import_xmp` below writes, so a keyword or rating it imports from
        // another program's sidecar reaches a view of it on the next frame — `source_refresh`.
        let read_at = self.catalog.as_ref().map_or(0, |c| c.change_count());
        self.library = Some(Library {
            source,
            scan,
            entries,
            stack_expanded: Default::default(),
            cache: rustroom_app::thumb_cache::ThumbCache::with_default_budget(),
            last_want: Vec::new(),
            selection: Selection::default(),
            loader,
            visible: (0..n).collect(),
            marks: std::collections::HashMap::new(),
            captures: std::collections::HashMap::new(),
            band: None,
            copies,
            hand_stacks,
            below,
            members_read_at: read_at,
            members_settling: None,
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
        let source = LibrarySource::Collection {
            id,
            name: name.to_string(),
        };
        let members = match self.source_members(&source) {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                self.status = format!("Could not read the collection: {e}");
                return;
            }
            None => {
                self.status = "No catalog".into();
                return;
            }
        };
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let copies = crate::source_refresh::copies_of(&source, &members);
        let n = self.open_scan_with(ctx, source, scan, copies);
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
        let source = LibrarySource::Keyword {
            id,
            name: name.to_string(),
        };
        let members = match self.source_members(&source) {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                self.status = format!("Could not read the keyword: {e}");
                return;
            }
            None => {
                self.status = "No catalog".into();
                return;
            }
        };
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(ctx, source, scan);
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

#[cfg(test)]
mod move_tests {
    use crate::test_app::{for_test, TestDir};

    /// ⚠⚠ **M9.9's done-when, end to end:** a RAW rated and edited in `root/a`, then moved on disk
    /// to `root/b` — the RAW alone, as a Finder drag of one file would — keeps its rating and its
    /// edits when the tree is scanned again, and the view shows both subfolders' photos.
    #[test]
    fn a_file_moved_on_disk_keeps_its_marks_and_edits() {
        let d = TestDir::new("m99_move");
        let root = d.0.join("shoot");
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        let x = root.join("a/x.RAF");
        std::fs::write(&x, b"some raw bytes").unwrap();
        std::fs::write(root.join("b/y.RAF"), b"another raw, different size").unwrap();
        rustroom_app::sidecar::save(
            &x,
            &edit_model::Edits {
                exposure: 0.7,
                ..Default::default()
            },
        )
        .unwrap();

        let ctx = egui::Context::default();
        let mut app = for_test();
        app.open_folder_at(&ctx, root.clone());
        assert_eq!(
            app.library.as_ref().unwrap().entries.len(),
            2,
            "both subfolders are shown"
        );
        let cat = app.catalog.as_ref().unwrap();
        cat.set_marks(
            &x,
            0,
            catalog::db::Marks {
                rating: 5,
                ..Default::default()
            },
        )
        .unwrap();

        let moved = root.join("b/x.RAF");
        std::fs::rename(&x, &moved).unwrap();
        app.open_folder_at(&ctx, root.clone());

        let cat = app.catalog.as_ref().unwrap();
        assert_eq!(
            cat.marks(&moved, 0).unwrap().rating,
            5,
            "the rating followed the file"
        );
        assert_eq!(
            rustroom_app::sidecar::load(&moved).map(|e| e.exposure),
            Some(0.7),
            "the edits followed it too"
        );
        assert!(
            app.status.contains("Recognised 1 moved photo"),
            "{}",
            app.status
        );
        // ⚠ The catalog's count, not the view's: the view is built from the disk scan and shows
        // two photos either way; the catalog would hold THREE rows had the move not relinked.
        assert_eq!(
            cat.photo_count().unwrap(),
            2,
            "one photo moved, not a new one beside a stale row"
        );
    }
}

#[cfg(test)]
mod reindex_tests {
    use crate::test_app::{for_test, TestDir};

    /// ⚠ Reindex recognises a move too (the author's request): the same fixture as opening, but
    /// the second scan is a Reindex of the open folder — and the photo OPEN at the time is the one
    /// that moved, so it must be reopened where it is now.
    #[test]
    fn reindex_recognises_a_move_and_reopens_the_moved_photo_where_it_is() {
        let d = TestDir::new("m99_reindex");
        let root = d.0.join("shoot");
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        let x = root.join("a/x.RAF");
        std::fs::write(&x, b"some raw bytes").unwrap();
        rustroom_app::sidecar::save(
            &x,
            &edit_model::Edits {
                exposure: 0.7,
                ..Default::default()
            },
        )
        .unwrap();
        let ctx = egui::Context::default();
        let mut app = for_test();
        app.open_folder_at(&ctx, root.clone());
        app.catalog
            .as_ref()
            .unwrap()
            .set_marks(
                &x,
                0,
                catalog::db::Marks {
                    rating: 3,
                    ..Default::default()
                },
            )
            .unwrap();

        // x is the photo OPEN when Reindex runs.
        app.finalize_session(
            &ctx,
            rustroom_app::photo_id::PhotoId::original(x.clone()),
            crate::test_app::fake_decode(),
        );

        let moved = root.join("b/x.RAF");
        std::fs::rename(&x, &moved).unwrap();
        app.reindex(&ctx);
        assert_eq!(
            app.pending.as_ref().map(|p| p.id.path.clone()),
            Some(moved.clone()),
            "the open photo must be reopened where it is now, not at its vanished path"
        );

        let cat = app.catalog.as_ref().unwrap();
        assert_eq!(
            cat.marks(&moved, 0).unwrap().rating,
            3,
            "the rating followed"
        );
        assert_eq!(
            rustroom_app::sidecar::load(&moved).map(|e| e.exposure),
            Some(0.7),
            "the edits followed"
        );
        assert!(
            app.status.contains("Recognised 1 moved photo"),
            "{}",
            app.status
        );
    }
}

#[cfg(test)]
mod forget_tests {
    use crate::test_app::{for_test, TestDir};

    /// Forgetting a parent forgets its subfolders too — ONE undo step that brings all back.
    #[test]
    fn forgetting_a_parent_forgets_its_subtree_as_one_undo_step() {
        let d = TestDir::new("m99_forget");
        let root = d.0.join("shoot");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.RAF"), b"one").unwrap();
        std::fs::write(root.join("sub/b.RAF"), b"two!").unwrap();
        let ctx = egui::Context::default();
        let mut app = for_test();
        app.open_folder_at(&ctx, root.clone());
        let n = |app: &crate::RustroomApp| app.catalog.as_ref().unwrap().folders().unwrap().len();
        assert_eq!(
            n(&app),
            2,
            "fixture: the parent and its subfolder are catalogued"
        );
        let id = app.catalog.as_ref().unwrap().folders().unwrap()[0].id;
        app.forget_folder(&ctx, id, &root);
        assert_eq!(n(&app), 0, "both forgotten");
        app.undo_action(&ctx);
        assert_eq!(n(&app), 2, "one undo brings both back");
    }
}
