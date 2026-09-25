//! The loaded [`Library`]'s derived state: the visible list, the thumbnail window, and the
//! rebuilds that follow a sort, a rename or a catalog write.
//!
//! Split out of `main.rs` unchanged. ⚠ Sorting is applied to the **scan**, never to
//! `entries` — that is what keeps a file's copies adjacent to it under every ordering.
//! ⚠ `marks` is keyed by [`PhotoId`] and `captures` by path, because a rating is per copy
//! and capture metadata is per file; both are read once per rebuild, never per cell.

use crate::{Library, RustroomApp};
use edit_model::Edits;
use rustroom_app::entries;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::sidecar;
use rustroom_app::thumb_key::ThumbKey;
use rustroom_app::thumb_loader::ThumbRequest;
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

impl Library {
    /// Recompute `visible` and prune the selection to it.
    ///
    /// ⚠ Both halves, always. Recomputing the visible list without pruning leaves the
    /// selection holding photos that are not on screen, which a sync or a paste would then
    /// write to.
    pub(crate) fn refilter(&mut self, filter: &rustroom_app::library_filter::LibraryFilter) {
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
        let filter = filter.effective_for(&formats);
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
) -> Vec<entries::PhotoEntry> {
    rustroom_app::stack::group(entries::build(scan, sidecar::load_all), stacking, expanded)
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
        lib.entries = browser_entries(&lib.scan, &stacking, &lib.stack_expanded);

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
        let grouped = browser_entries(&pair, &on, &Default::default());
        assert_eq!(grouped.len(), 1, "the pair must collapse on the open path");
        assert!(grouped[0].stack.is_some());

        // ⚠ And the control: with stacking off the same scan yields both rows, so the assertion
        // above is about grouping rather than about the fixture.
        let off = rustroom_app::stack::Stacking::default();
        assert_eq!(browser_entries(&pair, &off, &Default::default()).len(), 2);
    }
}
