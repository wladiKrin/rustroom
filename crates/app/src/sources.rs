//! What the Library is showing: folders, All Photos, collections, keywords, dates and
//! smart collections — and the rebuild, sort and filter that follow a source change.
//!
//! Split out of `main.rs` unchanged. ⚠ Browsing **replaces** the source; it never narrows
//! the current one. `open_scan` is the single place a [`Library`] is constructed, which is
//! what stops the loaders drifting apart.

use crate::*;

/// The modification time `collections::scan_of` used to read for itself.
///
/// ⚠ `UNIX_EPOCH` when the metadata cannot be read, so an unreadable file sorts to the oldest end
/// rather than failing the whole load — the rule `CatalogEntry::modified` already documents. It
/// lives here rather than in `scan_of` because **All Photos must not walk the filesystem**, and a
/// function that stats internally cannot offer that.
pub(crate) fn fs_modified(path: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}

impl Library {
    /// Recompute `visible` and prune the selection to it.
    ///
    /// ⚠ Both halves, always. Recomputing the visible list without pruning leaves the
    /// selection holding photos that are not on screen, which a sync or a paste would then
    /// write to.
    pub(crate) fn refilter(&mut self, filter: &rustroom_app::library_filter::LibraryFilter) {
        // ⚠ One derived `visible` list, composed from one filter — **not** a second filtering
        // mechanism. Sparse-versus-dense index confusion is this project's third-most-repeated
        // bug, and `visible` staying the single derived list is the mitigation that has held.
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let m = self.marks.get(&e.id).copied().unwrap_or_default();
                let c = self.captures.get(&e.id.path).cloned().unwrap_or_default();
                filter.matches(e, m, &c)
            })
            .map(|(i, _)| i)
            .collect();
        self.selection.retain_visible(&self.visible);
    }

    /// The thumbnail request for row `i`, using `live` in place of the stored edits when that
    /// row is the photo currently open.
    ///
    /// ⚠ `live` is passed in rather than read from `app`: the callers hold `&mut Library` through
    /// a closure and cannot reach `app.session` from there. Hoisting it out is the same shape the
    /// grid's context menu already uses for the catalog.
    pub(crate) fn thumb_request(
        &self,
        i: usize,
        live: Option<&(PathBuf, u32, Edits)>,
    ) -> ThumbRequest {
        let e = &self.entries[i];
        let edits = match live {
            Some((p, c, ed)) if *p == e.id.path && *c == e.id.copy => ed.clone(),
            _ => e.edits.clone(),
        };
        ThumbRequest {
            id: e.id.clone(),
            edits,
        }
    }

    /// Tell the cache and the loader what is on screen.
    ///
    /// `window` is every thumbnail the view wants resident — the drawn cells plus the prefetch
    /// margin, one request per row rather than per file.
    /// Pins them in the cache, evicts down to the budget, then asks the loader for whatever is
    /// still missing.
    pub(crate) fn sync_thumb_window(&mut self, window: &[ThumbRequest]) {
        let keys: Vec<ThumbKey> = window.iter().map(|r| r.key()).collect();
        self.cache.retain_window(&keys);
        let missing: Vec<ThumbRequest> = window
            .iter()
            .filter(|r| !self.cache.contains(&r.key()))
            .cloned()
            .collect();
        // ⚠ Only when it changes. See `last_want`.
        if missing != self.last_want {
            self.loader.want(&missing);
            self.last_want = missing;
        }
    }
}

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

    pub(crate) fn open_periods(
        &mut self,
        ctx: &egui::Context,
        periods: &[rustroom_app::date_tree::Period],
        label: &str,
    ) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let members = match cat.photos_in_periods(periods) {
            Ok(m) => m,
            Err(e) => {
                self.status = format!("Could not read the catalog: {e}");
                return;
            }
        };
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::Periods {
                periods: periods.to_vec(),
                label: label.to_string(),
            },
            scan,
        );
        self.status = if n == 0 {
            format!("Nothing taken in {label}")
        } else {
            format!("{label} — {n} photos")
        };
    }

    /// Handle a click on a date row, honouring the modifiers.
    ///
    /// ⚠⚠ **The same three rules `apply_click` uses for photos**, so the app has *one*
    /// multi-select idiom rather than two: a plain click replaces, ⌘/Ctrl toggles, Shift extends
    /// from the last click.
    ///
    /// ⚠ Shift extends **in tree order**, which `date_tree::flatten` defines — so "everything
    /// between" means what the eye sees rather than what a tuple comparison would say.
    pub(crate) fn click_period(
        &mut self,
        ctx: &egui::Context,
        period: rustroom_app::date_tree::Period,
        mods: egui::Modifiers,
    ) {
        use rustroom_app::date_tree;
        let current: Vec<date_tree::Period> = match self.library.as_ref().map(|l| &l.source) {
            Some(LibrarySource::Periods { periods, .. }) => periods.clone(),
            _ => Vec::new(),
        };
        let next = if mods.command {
            let mut v = current.clone();
            if let Some(i) = v.iter().position(|p| *p == period) {
                v.remove(i);
            } else {
                v.push(period);
            }
            v
        } else if mods.shift {
            let days = self
                .catalog
                .as_ref()
                .and_then(|c| c.capture_days().ok())
                .unwrap_or_default();
            let order = date_tree::flatten(&date_tree::group(&days));
            // ⚠ Anchored on the *last* click, which is the previous selection's tail — the same
            // thing `Selection::extend_to` anchors on.
            let anchor = self.last_period.unwrap_or(period);
            date_tree::range(&order, anchor, period)
        } else {
            vec![period]
        };

        // ⚠ Only a plain or ⌘ click moves the anchor; a Shift-click extends from where it was, or
        // shift-clicking twice would walk the range along instead of growing it.
        if !mods.shift {
            self.last_period = Some(period);
        }

        if next.is_empty() {
            // ⚠ Deselecting the last date closes the view rather than showing an empty grid with
            // no way back — `photos_in_periods(&[])` matches nothing by design.
            self.library = None;
            self.status = "No dates selected".into();
            return;
        }
        let label = date_tree::label_for(&next);
        self.open_periods(ctx, &next, &label);
    }

    /// Show the photos a saved rule matches.
    ///
    /// ⚠⚠ **Re-evaluated on every open, never cached as a member list.** A photo rated *into* the
    /// rule must appear without a restart — the same reason `open_collection` re-reads its members
    /// rather than caching them.
    ///
    /// ⚠ Evaluated in memory over every photo the catalog knows, which is what the `Text`
    /// criterion forces: it reads a copy's own name from the **sidecar**, which SQL cannot see.
    /// Measured at 4.6 ms per 830 sidecars, so ~60 ms at 10,000.
    pub(crate) fn open_smart(&mut self, ctx: &egui::Context, id: i64, name: &str) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let Some(row) = cat
            .smart_collections()
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.id == id)
        else {
            self.status = format!("“{name}” is gone");
            return;
        };
        // ⚠ A rule that will not parse yields an **empty** collection and a message, never a
        // panic: the text is user-editable, in a file, by hand.
        let rule: rustroom_app::rule::Expr = match serde_json::from_str(&row.rule) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("“{name}” has an unreadable rule: {e}");
                return;
            }
        };

        let rows = cat.all_photos().unwrap_or_default();
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
        let sizes: std::collections::HashMap<PathBuf, i64> = Default::default();
        let members: Vec<(PathBuf, u32)> = rows.into_iter().map(|r| (r.path, r.copy)).collect();
        let scan = rustroom_app::collections::scan_of(&members, sidecar::edited_at, |p| {
            mtimes
                .get(p)
                .copied()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        let all = entries::build(&scan, sidecar::load_all);

        // Facts the evaluator needs, gathered once rather than per entry.
        let (colls, keys) = self.memberships();
        let Some(cat) = &self.catalog else { return };
        let mut marks: std::collections::HashMap<PhotoId, catalog::db::Marks> = Default::default();
        let mut captures: std::collections::HashMap<PathBuf, core_image::Capture> =
            Default::default();
        let mut folders: Vec<PathBuf> = Vec::new();
        for e in &all {
            if let Some(dir) = e.id.path.parent() {
                if !folders.iter().any(|f| f == dir) {
                    folders.push(dir.to_path_buf());
                }
            }
        }
        for dir in &folders {
            for (path, copy, m) in cat.marks_in(dir).unwrap_or_default() {
                marks.insert(PhotoId::new(path, copy), m);
            }
            for (path, c) in cat.captures_in(dir).unwrap_or_default() {
                captures.insert(path, c);
            }
        }

        let empty: Vec<i64> = Vec::new();
        let keep: Vec<PathBuf> = all
            .iter()
            .filter(|e| {
                let capture = captures.get(&e.id.path).cloned().unwrap_or_default();
                let facts = rustroom_app::rule::Facts {
                    entry: e,
                    marks: marks.get(&e.id).copied().unwrap_or_default(),
                    capture: &capture,
                    collections: colls.get(&e.id).unwrap_or(&empty),
                    keywords: keys.get(&e.id.path).unwrap_or(&empty),
                    size: sizes.get(&e.id.path).copied(),
                };
                rule.matches(&facts)
            })
            .map(|e| e.id.path.clone())
            .collect();

        // ⚠ One entry per **file**: `entries::build` expands copies itself, so passing copies
        // through `scan_of` would double every one of them.
        let mut kept: Vec<(PathBuf, u32)> = Vec::new();
        for p in keep {
            if !kept.iter().any(|(q, _)| q == &p) {
                kept.push((p, 0));
            }
        }
        let mut scan = rustroom_app::collections::scan_of(&kept, sidecar::edited_at, |p| {
            mtimes
                .get(p)
                .copied()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::Smart {
                id,
                name: name.to_string(),
            },
            scan,
        );
        self.status = if n == 0 {
            format!("“{name}” matches nothing")
        } else {
            format!("“{name}” — {n} photos")
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
        let loader = ThumbLoader::new(space, move || ctx2.request_repaint());
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

    /// Change how the browser orders a folder, keeping the photo you were looking at selected.
    ///
    /// ⚠ Guarded on an actual change, because the combo's body runs every frame — the
    /// `apply_view_demosaic` rule. Without it a folder of hundreds would be re-sorted and its
    /// entries rebuilt sixty times a second.
    pub(crate) fn apply_sort(&mut self, order: catalog::SortOrder) {
        if self.settings.library_sort == order {
            return;
        }
        self.settings.library_sort = order;
        self.save_settings();
        // ⚠ Sorting renumbers every index, and `Selection` holds indices. `rebuild_entries`
        // re-derives the selection from a `PhotoId`, which survives a reorder; keeping the old
        // indices would leave a different photo selected.
        let keep = self.open_photo().or_else(|| {
            let lib = self.library.as_ref()?;
            lib.entries
                .get(lib.selection.active()?)
                .map(|e| e.id.clone())
        });
        self.rebuild_entries(keep.as_ref());
    }

    /// Re-expand the Library's rows after a copy was added or removed.
    ///
    /// ⚠ Touches neither the thumbnail cache nor the loader: both are keyed by **path**, so a
    /// rebuild that only shifts indices invalidates nothing. This used to have to carry the
    /// textures across by hand, and getting it wrong greyed out the whole folder.
    pub(crate) fn rebuild_entries(&mut self, select: Option<&PhotoId>) {
        // ⚠ Read before the borrow: `self.settings` and `self.library` are disjoint fields, but
        // the compiler only sees that when the read happens first.
        let sort = self.settings.library_sort;
        // ⚠ Read before the borrow: `self.filter` and `self.library` are disjoint fields, but the
        // compiler only sees that when the read happens first — the rule this file already states
        // for `self.settings`.
        let filter = self.filter.clone();
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        // ⚠ Re-sorted every rebuild rather than only when the order changes: a rename moves a
        // photo's place in a name ordering, and a rebuild is exactly when that has to be seen.
        //
        // ⚠ The edit times are refreshed here too, and **only for the orders that use them** —
        // the other four would be paying a stat per file for nothing.
        if sort.needs_edited() {
            for e in lib.scan.iter_mut() {
                e.edited = sidecar::edited_at(&e.path);
            }
        }
        catalog::sort(&mut lib.scan, sort);
        lib.entries = entries::build(&lib.scan, sidecar::load_all);

        // ⚠ The selection is by *index*, and the indices have just shifted — so it is re-derived
        // from the photo we mean rather than kept. Keeping it would silently select whichever
        // row happened to land on the old number.
        lib.selection.clear();
        if let Some(id) = select {
            if let Some(i) = lib.entries.iter().position(|e| &e.id == id) {
                lib.selection.click(i);
            }
        }
        lib.refilter(&filter);
        // ⚠ The indices have just shifted, so the marks map must be re-read against the new
        // entries — and `reload_marks` refilters, which the rebuild needs anyway.
        self.reload_marks();
    }

    /// Re-read the marks for whatever the Library is showing.
    ///
    /// ⚠ Reads the marks of every *folder* represented, because a collection spans folders. One
    /// query per folder, never per photo.
    pub(crate) fn reload_marks(&mut self) {
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
        let mut map = std::collections::HashMap::new();
        for dir in &folders {
            if let Ok(rows) = cat.marks_in(dir) {
                for (path, copy, m) in rows {
                    map.insert(PhotoId::new(path, copy), m);
                }
            }
        }
        let filter = self.filter.clone();
        if let Some(lib) = self.library.as_mut() {
            lib.marks = map;
            // ⚠ Refilter here, or a newly-rated photo would not appear or disappear under an
            // active star filter until some unrelated action happened to rebuild.
            lib.refilter(&filter);
        }
    }

    /// Re-read the capture metadata for whatever the Library is showing.
    ///
    /// ⚠ One query per **folder** represented, never one per photo — the shape `reload_marks`
    /// uses, and for the same reason.
    ///
    /// ⚠⚠ Must run after the metadata scanner drains as well as wherever `reload_marks` does, or
    /// a newly-scanned folder does not become filterable until something unrelated rebuilds. The
    /// `reload_marks` refilter rule, one layer along.
    pub(crate) fn reload_captures(&mut self) {
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
        let mut map = std::collections::HashMap::new();
        for dir in &folders {
            if let Ok(rows) = cat.captures_in(dir) {
                for (path, c) in rows {
                    map.insert(path, c);
                }
            }
        }
        let filter = self.filter.clone();
        if let Some(lib) = self.library.as_mut() {
            lib.captures = map;
            lib.refilter(&filter);
        }
    }

    /// Collection ids per photo, and keyword ids per **file**.
    ///
    /// ⚠ One query per collection and per keyword, never one per photo: a catalog has a handful
    /// of each and may have ten thousand photos.
    ///
    /// ⚠⚠ Two maps keyed differently, and that is M4.2b's asymmetry rather than an oversight:
    /// **collections are per (file, copy)** — a rating describes the image you made — while
    /// **keywords are per file**, because a keyword describes what is *in* the picture, which
    /// every copy shares. **Do not unify them.**
    ///
    /// ⚠ `keyword_members` already expands *descendants* through its recursive CTE, so inserting
    /// the queried keyword's own id for each member is exactly the "matches an ancestor"
    /// semantics `Criterion::Keyword` documents.
    #[allow(clippy::type_complexity)]
    pub(crate) fn memberships(
        &self,
    ) -> (
        std::collections::HashMap<PhotoId, Vec<i64>>,
        std::collections::HashMap<PathBuf, Vec<i64>>,
    ) {
        let mut colls: std::collections::HashMap<PhotoId, Vec<i64>> = Default::default();
        let mut keys: std::collections::HashMap<PathBuf, Vec<i64>> = Default::default();
        let Some(cat) = &self.catalog else {
            return (colls, keys);
        };
        for c in cat.collections().unwrap_or_default() {
            for (path, copy) in cat.collection_members(c.id).unwrap_or_default() {
                colls
                    .entry(PhotoId::new(path, copy))
                    .or_default()
                    .push(c.id);
            }
        }
        for k in cat.keywords().unwrap_or_default() {
            for (path, _copy) in cat.keyword_members(k.id).unwrap_or_default() {
                let e = keys.entry(path).or_default();
                if !e.contains(&k.id) {
                    e.push(k.id);
                }
            }
        }
        (colls, keys)
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
