//! The two sources that are a **query** rather than a named place: capture dates and smart
//! collections.
//!
//! Split out of `main.rs` unchanged. ⚠⚠ A smart collection is re-evaluated on every open,
//! never cached as a member list — a photo rated *into* the rule must appear without a
//! restart, which is what makes it smart rather than a snapshot. ⚠ Evaluation is in memory
//! rather than in SQL, because `text` matches `entries::shown_name`, which reads a copy's
//! own name from the sidecar.

use crate::library_state::fs_modified;
use crate::{LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::entries;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::sidecar;
use std::path::PathBuf;

/// What a smart collection matches: one member per **file**, and the catalog's modification times
/// so opening it stats nothing.
pub(crate) struct SmartMembers {
    pub(crate) members: Vec<(PathBuf, u32)>,
    pub(crate) mtimes: std::collections::HashMap<PathBuf, std::time::SystemTime>,
}

impl RustroomApp {
    pub(crate) fn open_periods(
        &mut self,
        ctx: &egui::Context,
        periods: &[rustroom_app::date_tree::Period],
        label: &str,
    ) {
        let source = LibrarySource::Periods {
            periods: periods.to_vec(),
            label: label.to_string(),
        };
        let members = match self.source_members(&source) {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                self.status = format!("Could not read the catalog: {e}");
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
        if self.catalog.is_none() {
            self.status = "No catalog".into();
            return;
        }
        let SmartMembers { members, mtimes } = match self.smart_members(id) {
            Ok(found) => found,
            Err(why) => {
                self.status = format!("“{name}” {why}");
                return;
            }
        };
        let mut scan = rustroom_app::collections::scan_of(&members, sidecar::edited_at, |p| {
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

    /// The files a saved rule matches now, with the catalog's modification times — what
    /// [`Self::open_smart`] shows and what `source_refresh` re-reads, evaluated in one place so the
    /// two cannot disagree. ⚠ The error completes a sentence that starts with the collection's name.
    pub(crate) fn smart_members(&self, id: i64) -> Result<SmartMembers, String> {
        // ⚠ Read before `cat` borrows `self`.
        let immich_root = self.immich_cache_root();
        let Some(cat) = &self.catalog else {
            return Err("has no catalog".into());
        };
        let Some(row) = cat
            .smart_collections()
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.id == id)
        else {
            return Err("is gone".into());
        };
        // ⚠ A rule that will not parse yields an **empty** collection and a message, never a
        // panic: the text is user-editable, in a file, by hand.
        let rule: rustroom_app::rule::Expr =
            serde_json::from_str(&row.rule).map_err(|e| format!("has an unreadable rule: {e}"))?;

        let rows = cat.all_photos().unwrap_or_default();
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
        let Some(cat) = &self.catalog else {
            return Err("has no catalog".into());
        };
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
        Ok(SmartMembers {
            members: kept,
            mtimes,
        })
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
}
