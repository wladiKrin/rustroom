//! The loaded [`Library`]'s derived state: the visible list, the thumbnail window, and the
//! rebuilds that follow a sort, a rename or a catalog write.
//!
//! Split out of `main.rs` unchanged. ⚠ Sorting is applied to the **scan**, never to
//! `entries` — that is what keeps a file's copies adjacent to it under every ordering.
//! ⚠ `marks` is keyed by [`PhotoId`] and `captures` by path, because a rating is per copy
//! and capture metadata is per file; both are read once per rebuild, never per cell.

use crate::RustroomApp;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::entries;
use rustroom_app::entries::PhotoEntry;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::selection::Selection;
use rustroom_app::sidecar;
use rustroom_app::thumb_key::ThumbKey;
use rustroom_app::thumb_loader::{ThumbLoader, ThumbRequest};
use std::path::PathBuf;

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

/// What the Library is currently showing.
///
/// ⚠ A collection is a *source*, not a filter, so the format filter and the six sort orders
/// compose with it unchanged — they operate on whatever scan is loaded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LibrarySource {
    Folder(PathBuf),
    /// Every photo the catalog knows, from every folder ever opened.
    AllPhotos,
    /// The photos a saved rule matches, re-evaluated every time it is opened.
    Smart {
        id: i64,
        name: String,
    },
    /// Photos taken in any of these calendar periods.
    ///
    /// ⚠ A **set**, because ⌘-clicking several days is how a shoot spanning a weekend is browsed.
    /// A single click is the one-element case, so there is no second code path.
    Periods {
        periods: Vec<rustroom_app::date_tree::Period>,
        label: String,
    },
    Collection {
        id: i64,
        name: String,
    },
    Keyword {
        id: i64,
        name: String,
    },
    /// One Immich album, its photos cached locally on demand.
    ///
    /// ⚠ The name is carried rather than looked up: the rail may not have fetched the album list
    /// when the status line wants to say where these photos came from.
    ImmichAlbum {
        id: String,
        name: String,
    },
    /// One month of the Immich timeline.
    ///
    /// ⚠ The bucket identifier the server gave, plus the label to show. The label is carried rather
    /// than formatted on demand so the status line and the rail always agree.
    ImmichBucket {
        bucket: String,
        label: String,
    },
}

/// Browsing state for an opened folder.
pub(crate) struct Library {
    /// Where these photos came from.
    ///
    /// ⚠ Kept rather than derived from `scan[0].path.parent()`, which says nothing at all about
    /// an *empty* folder — and Reindex on an empty folder is exactly when you would want it,
    /// because the files may have arrived since.
    pub(crate) source: LibrarySource,
    /// The folder scan this Library was built from.
    ///
    /// ⚠ Kept so `entries` can be re-expanded when a copy is created or deleted, without a
    /// second directory read — and so the rebuild sees exactly the same files, rather than
    /// whatever the folder happens to hold at that moment.
    pub(crate) scan: Vec<catalog::CatalogEntry>,
    pub(crate) entries: Vec<PhotoEntry>,
    /// Which stacks the user has expanded, by `stack::StackKey`.
    ///
    /// ⚠ **Session state, deliberately not persisted.** Expanding a pair is a momentary "let me see
    /// the JPEG", not a preference. ⚠ It lives here rather than on `RustroomApp` so opening another
    /// folder starts collapsed — `Library` is rebuilt, and the keys carry the folder anyway.
    ///
    /// ⚠⚠ Changing it must go through `rebuild_entries`, never patch `entries` in place: the grid,
    /// the filmstrip, `selection` and `view_region` all index into that one list.
    pub(crate) stack_expanded: std::collections::HashSet<rustroom_app::stack::StackKey>,
    /// Loaded thumbnails, keyed by path and bounded by a byte budget.
    ///
    /// ⚠ Keyed by **path**, so copies of one file share a texture and a re-sort or a rebuild
    /// invalidates nothing. `entries` indices never reach it.
    pub(crate) cache: rustroom_app::thumb_cache::ThumbCache<egui::TextureHandle>,
    /// The last want-list handed to the loader.
    ///
    /// ⚠ Kept so `sync_thumb_window` can call `want` **only when the list changes**. Its body
    /// runs every frame; without the comparison it would rewrite the queue sixty times a second.
    /// The same guard `apply_view_demosaic` needs, for the same reason.
    pub(crate) last_want: Vec<ThumbRequest>,
    pub(crate) selection: Selection,
    pub(crate) loader: ThumbLoader,
    /// Indices into `entries` that pass `filter`, ascending. Derived — recomputed by
    /// `refilter`, never edited directly.
    pub(crate) visible: Vec<usize>,
    /// Marks for every marked photo on show, keyed by identity.
    ///
    /// ⚠⚠ Read **once per rebuild**, not per frame and never per cell. The grid draws sixty
    /// times a second over hundreds of thumbnails; a query per cell would be a database round
    /// trip per photo per frame. Only marked photos are in here, so an unmarked folder costs
    /// nothing.
    pub(crate) marks: std::collections::HashMap<PhotoId, catalog::db::Marks>,
    /// Capture metadata for everything on show, keyed by **path**.
    ///
    /// ⚠ By path, not `PhotoId`: capture is per *file* and every copy shares it, while `marks` is
    /// keyed by `PhotoId` for the opposite reason. Both carry a comment saying so, because it
    /// looks like an inconsistency and is not.
    ///
    /// ⚠⚠ Read **once per rebuild**, never per frame and never per cell — the rule `marks`
    /// already follows, and for the same reason: the grid draws sixty times a second.
    pub(crate) captures: std::collections::HashMap<PathBuf, core_image::Capture>,
    /// An in-progress rubber-band selection, as `(start, current)` in **screen** coordinates.
    ///
    /// ⚠ Screen coordinates rather than content coordinates, because the cell rects it is tested
    /// against come from `Response::rect`, which is also screen. Both are recomputed every frame,
    /// so scrolling mid-drag cannot leave them describing different things — the mistake the
    /// M3.1a mask overlay made by caching a derived screen position.
    pub(crate) band: Option<(egui::Pos2, egui::Pos2)>,
}

impl Library {
    /// `filter` as it applies to this folder — the per-folder format override included. `refilter`
    /// and `why_empty` both read it here, so the grid can never explain a filter it did not apply.
    fn effective(
        &self,
        filter: &rustroom_app::library_filter::LibraryFilter,
    ) -> rustroom_app::library_filter::LibraryFilter {
        // ⚠⚠ Applied HERE rather than at the callers: `refilter` has 12 call sites across `shell`,
        // `library`, `library_state`, `sources`, `multi::compare` and `test_app`, and the override
        // has to hold at every one. One choke point cannot be bypassed; twelve are twelve chances
        // to forget — the same argument the comment below makes for `visible` being one list.
        // ⚠⚠ The **folder's** formats, from the scan — not the grouped entries. With stacking on,
        // an all-paired folder's rows are all RAW, so reading `entries` made the folder look
        // single-format and the override fired: hiding a format then silently stopped working. The
        // question "what does this folder hold" is about the folder, and stacking is about the view.
        let formats =
            rustroom_app::format_filter::formats_of(self.scan.iter().map(|e| e.path.as_path()));
        filter.effective_for(&formats)
    }

    /// Why nothing is visible under `filter` — for the empty grid to say.
    pub(crate) fn why_empty(
        &self,
        filter: &rustroom_app::library_filter::LibraryFilter,
    ) -> rustroom_app::library_filter::EmptyGrid {
        self.effective(filter).why_empty(
            &self.entries,
            |e| self.marks.get(&e.id).copied().unwrap_or_default(),
            |e| self.captures.get(&e.id.path).cloned().unwrap_or_default(),
        )
    }

    /// Recompute `visible` and prune the selection to it.
    ///
    /// ⚠ Both halves, always. Recomputing the visible list without pruning leaves the
    /// selection holding photos that are not on screen, which a sync or a paste would then
    /// write to.
    pub(crate) fn refilter(&mut self, filter: &rustroom_app::library_filter::LibraryFilter) {
        let filter = self.effective(filter);
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

    /// Every selected row's [`PhotoId`].
    ///
    /// ⚠⚠ **The selection, not the active photo** — the rule a paste, a preset and a sync already
    /// follow, and culling is where it matters most: select a run of frames and press `X`.
    ///
    /// ⚠ It exists as a method rather than being read inline **so the targets can be captured
    /// before auto-advance moves the selection.** `library.rs` records that ordering; this is the
    /// one place that decides which photos a culling key acts on.
    pub(crate) fn selected_ids(&self) -> Vec<PhotoId> {
        self.selection
            .iter()
            .filter_map(|i| self.entries.get(i))
            .map(|e| e.id.clone())
            .collect()
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

/// Expand a scan into the browser's rows, **grouped into stacks**.
///
/// ⚠⚠ **THE one place browser entries are made, and it exists because there were two.** The
/// folder-open path in `sources` built them with a bare `entries::build` while `rebuild_entries`
/// grouped them — so stacking worked after a re-sort and did nothing when a folder was opened,
/// reported as *"they are not stacked"*, 2026-09-25. ⚠ A comment claiming a single site is **not** a
/// single site; a function both callers must go through is.
///
/// ⚠ A free function taking `&Stacking` rather than a method on `&self`: `rebuild_entries` holds
/// `self.library` mutably while it needs this, and that function's own rule is "read before the
/// borrow", because the compiler cannot see that `settings` and `library` are disjoint fields.
///
/// ⚠ `smart_sources` deliberately does **not** use this: it evaluates every file against a rule, and
/// a collapsed pair would silently skip the JPEG.
pub(crate) fn browser_entries(
    scan: &[catalog::CatalogEntry],
    stacking: &rustroom_app::stack::Stacking,
    expanded: &std::collections::HashSet<rustroom_app::stack::StackKey>,
    // ⚠ The server's asset→stack map, empty for every non-Immich folder. See `stack::key_of_with`.
    remote: &std::collections::HashMap<String, String>,
) -> Vec<entries::PhotoEntry> {
    rustroom_app::stack::group(
        entries::build(scan, sidecar::load_all),
        stacking,
        expanded,
        remote,
    )
}

impl RustroomApp {
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
        // ⚠ Read before the borrow too, for the reason stated just above.
        let stacking = self.settings.stacking.clone();
        // ⚠ Read before the borrow, the rule stated just above. Cloned rather than borrowed for
        // the same reason: `self.immich_stacks` and `self.library` are disjoint but the compiler
        // cannot see it while one is held mutably.
        let remote = self.immich_stacks.of_asset.clone();
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
        lib.entries = browser_entries(&lib.scan, &stacking, &lib.stack_expanded, &remote);

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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ **The bug the user reported as "they are not stacked", pinned at the function both paths
    /// now go through.** Stacking was wired into `rebuild_entries` only, so it worked after a
    /// re-sort and did nothing when a folder was opened — and the comment there *claimed* it was the
    /// one place entries are built, which was simply false. A comment is not a guard.
    #[test]
    fn browser_entries_groups_a_pair_on_every_path() {
        let scan = |names: &[&str]| -> Vec<catalog::CatalogEntry> {
            names
                .iter()
                .map(|n| catalog::CatalogEntry {
                    path: std::path::PathBuf::from(format!("/f/{n}")),
                    file_name: (*n).to_string(),
                    modified: std::time::SystemTime::UNIX_EPOCH,
                    edited: None,
                })
                .collect()
        };
        let on = rustroom_app::stack::Stacking {
            enabled: true,
            ..Default::default()
        };
        let pair = scan(&["a.JPG", "a.RAF"]);

        // ⚠ The sample assertion: the scan really holds two files, or "grouped to one" is vacuous.
        assert_eq!(pair.len(), 2);
        let grouped = browser_entries(&pair, &on, &Default::default(), &Default::default());
        assert_eq!(grouped.len(), 1, "the pair must collapse on the open path");
        assert!(grouped[0].stack.is_some());

        // ⚠ And the control: with stacking off the same scan yields both rows, so the assertion
        // above is about grouping rather than about the fixture.
        let off = rustroom_app::stack::Stacking::default();
        assert_eq!(
            browser_entries(&pair, &off, &Default::default(), &Default::default()).len(),
            2
        );
    }

    #[test]
    fn a_filter_that_hides_a_selected_photo_prunes_the_selection() {
        // ⚠⚠ The hazard `retain_visible` exists for, and the reason `refilter` always does BOTH
        // halves. Select photos, then filter them away, and the selection would still hold them —
        // so a sync or a paste would write to photos that are not on screen. Silent, and
        // discovered as data loss rather than as a bug.
        let mut lib = crate::test_app::library_of(4);
        lib.selection.select_indices(&[0, 1, 2, 3]);
        assert_eq!(lib.selection.len(), 4);
        let filter = rustroom_app::library_filter::LibraryFilter {
            text: "nothing matches this".into(),
            ..Default::default()
        };
        lib.refilter(&filter);
        assert!(lib.visible.is_empty(), "the filter did not hide anything");
        assert_eq!(
            lib.selection.len(),
            0,
            "the selection still holds photos that are not on screen"
        );
    }

    #[test]
    fn the_search_text_narrows_without_touching_the_others() {
        // ⚠ Composition through the one `visible` list, asserted end to end on a real `Library`
        // rather than only on `LibraryFilter` in isolation.
        let mut lib = crate::test_app::library_of(20);
        assert_eq!(lib.visible.len(), 20);
        // ⚠ "0001" would match eleven rows — 00001 and 00010..00019 — which is *correct* and was
        // the first version of this assertion, wrong about right code. A substring that occurs in
        // exactly one name is what discriminates.
        let filter = rustroom_app::library_filter::LibraryFilter {
            text: "00007".into(),
            ..Default::default()
        };
        lib.refilter(&filter);
        assert_eq!(lib.visible.len(), 1, "expected exactly rustroom_00007.RAF");
    }
}
