//! What the Library rail reads from the catalog — re-read only when the catalog changes (M10.14e).
//!
//! ⚠⚠ **The rail draws every frame**, and it used to ask the catalog six questions each time — the
//! photo count, the folders with their counts, the collections, the smart collections, the keywords
//! with their counts and the capture days — several aggregating over every photo: **27.5 ms a frame
//! at 100,000 photos** (`catalog_scale`). Keyed on the catalog's identity and
//! [`Catalog::change_count`], which moves on every write through the app's one connection — the
//! signal `keyword_suggest` and `source_refresh` already use — so a change from any route shows on
//! the next frame without a list of writers to keep in step.

use catalog::db::{Catalog, CollectionRow, DayRow, FolderRow, KeywordRow, SmartRow};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

#[derive(Default)]
pub struct RailData {
    /// `(instance, change_count)` of the catalog last read, or `None` before the first read.
    key: Option<(u64, u64)>,
    /// The photos outside the Immich cache, copies included — every one counts in All Photos.
    local: i64,
    /// The Immich cache's rows by file name, each with its number of copies — one counts only once
    /// its file is downloaded, as the All Photos view shows it (`row_is_browsable`).
    immich: Arc<HashMap<OsString, i64>>,
    /// `(downloads, count)` of the last [`RailData::all`] — see there.
    all_memo: Cell<Option<(usize, i64)>>,
    /// ⚠ `Arc`s, so a frame that takes a list costs a reference count, not a copy of every row.
    pub folders: Arc<Vec<FolderRow>>,
    pub collections: Arc<Vec<CollectionRow>>,
    pub smarts: Arc<Vec<SmartRow>>,
    pub keywords: Arc<Vec<KeywordRow>>,
    pub days: Arc<Vec<DayRow>>,
    /// How many times the catalog was read — what the tests count.
    computed: usize,
}

impl RailData {
    /// Re-read everything when `cat` is another catalog or has changed since the last read.
    /// `immich_root` is the Immich cache folder, if a server is set.
    pub fn refresh(&mut self, cat: Option<&Catalog>, immich_root: Option<&Path>) {
        let Some(cat) = cat else {
            if self.key.is_some() {
                *self = Self {
                    computed: self.computed,
                    ..Default::default()
                };
            }
            return;
        };
        let key = (cat.instance(), cat.change_count());
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        let folders = cat.folders().unwrap_or_default();
        let mut immich: HashMap<OsString, i64> = HashMap::new();
        let mut in_cache = 0;
        for f in folders.iter().filter(|f| {
            immich_root.is_some_and(|r| crate::immich_source::is_immich_path(r, &f.path))
        }) {
            in_cache += f.count;
            for name in cat.photos_in(&f.path).unwrap_or_default() {
                *immich.entry(OsString::from(name)).or_default() += 1;
            }
        }
        self.local = cat.photo_count().unwrap_or(0) - in_cache;
        self.immich = Arc::new(immich);
        self.all_memo.set(None);
        self.folders = Arc::new(folders);
        self.collections = Arc::new(cat.collections().unwrap_or_default());
        self.smarts = Arc::new(cat.smart_collections().unwrap_or_default());
        self.keywords = Arc::new(cat.keywords().unwrap_or_default());
        self.days = Arc::new(cat.capture_days().unwrap_or_default());
        self.computed += 1;
    }

    /// All Photos' count: the local photos and the Immich ones in `present`, the downloaded file
    /// names (`RustroomApp::immich_present`, read live so a download shows at once).
    ///
    /// ⚠ Called every frame, so the intersection is memoised on the number of downloads — it moves
    /// on every download and on Clear…, the ways the set changes; a re-read of the same size with
    /// other names would keep the old count until the next catalog change.
    pub fn all(&self, present: &HashSet<OsString>) -> i64 {
        if let Some((n, all)) = self.all_memo.get() {
            if n == present.len() {
                return all;
            }
        }
        let downloaded: i64 = if present.len() < self.immich.len() {
            present.iter().filter_map(|n| self.immich.get(n)).sum()
        } else {
            self.immich
                .iter()
                .filter(|(n, _)| present.contains(*n))
                .map(|(_, c)| c)
                .sum()
        };
        let all = self.local + downloaded;
        self.all_memo.set(Some((present.len(), all)));
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::path::Path;

    fn catalog_with_photos(n: usize) -> Catalog {
        catalog_in("/photos/2026-10-13", n)
    }

    fn catalog_in(folder: &str, n: usize) -> Catalog {
        let cat = Catalog::open_in_memory().unwrap();
        let dir = Path::new(folder);
        let entries: Vec<catalog::CatalogEntry> = (0..n)
            .map(|i| catalog::CatalogEntry {
                path: dir.join(format!("{i}.jpg")),
                file_name: format!("{i}.jpg"),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            })
            .collect();
        cat.sync_folder(dir, &entries).unwrap();
        cat
    }

    #[test]
    fn a_frame_without_a_change_reads_nothing_and_a_change_rereads() {
        let cat = catalog_with_photos(3);
        let mut d = RailData::default();
        d.refresh(Some(&cat), None);
        assert_eq!(
            (d.computed, d.all(&HashSet::new()), d.folders.len()),
            (1, 3, 1)
        );
        d.refresh(Some(&cat), None);
        d.refresh(Some(&cat), None);
        assert_eq!(d.computed, 1, "no write, no read");
        cat.create_keyword("Paris", None).unwrap();
        d.refresh(Some(&cat), None);
        assert_eq!(d.computed, 2);
        assert!(d.keywords.iter().any(|k| k.name == "Paris"));
    }

    /// ⚠ Two fresh catalogs sit at the same change count — the identity is what tells them apart.
    #[test]
    fn another_catalog_is_read_even_at_the_same_count() {
        let a = catalog_in("/photos/a", 3);
        let b = catalog_in("/photos/b", 3);
        assert_eq!(a.change_count(), b.change_count(), "fixture check");
        let mut d = RailData::default();
        d.refresh(Some(&a), None);
        d.refresh(Some(&b), None);
        assert_eq!(d.computed, 2);
        assert_eq!(d.folders[0].path, Path::new("/photos/b"));
    }

    /// The Immich folder of `catalog_in`'s shape, its rows named like `cache_path`'s.
    const ROOT: &str = "/cache/immich/https___host";

    fn present(names: &[&str]) -> HashSet<OsString> {
        names.iter().map(OsString::from).collect()
    }

    /// ⚠ Reported in use (2026-10-28): All Photos read 2,588 over the eight photos its view showed —
    /// every Immich row browsed but never downloaded was counted. The view hides them
    /// (`row_is_browsable`); the count must agree.
    #[test]
    fn all_photos_counts_only_the_immich_photos_on_disk() {
        let cat = catalog_in("/photos/local", 3);
        let root = Path::new(ROOT);
        let entries: Vec<catalog::CatalogEntry> = (0..4)
            .map(|i| catalog::CatalogEntry {
                path: root.join(format!("{i}.jpg")),
                file_name: format!("{i}.jpg"),
                modified: std::time::SystemTime::UNIX_EPOCH,
                edited: None,
            })
            .collect();
        cat.sync_folder(root, &entries).unwrap();
        let mut d = RailData::default();
        d.refresh(Some(&cat), Some(root));
        assert_eq!(
            d.all(&present(&[])),
            3,
            "no download, only the local photos"
        );
        assert_eq!(d.all(&present(&["1.jpg"])), 4);
        // A download lands without a catalog write: the count follows it on the next frame.
        assert_eq!(d.all(&present(&["1.jpg", "2.jpg"])), 5);
        // A file in the cache the catalog never saw is not a photo of it.
        assert_eq!(d.all(&present(&["1.jpg", "2.jpg", "stray.jpg"])), 5);
        assert_eq!(d.computed, 1, "no catalog read per frame");
    }

    #[test]
    fn without_immich_every_row_counts() {
        let cat = catalog_with_photos(3);
        let mut d = RailData::default();
        d.refresh(Some(&cat), None);
        assert_eq!(d.all(&present(&[])), 3);
    }

    #[test]
    fn no_catalog_shows_nothing() {
        let cat = catalog_with_photos(4);
        let mut d = RailData::default();
        d.refresh(Some(&cat), None);
        d.refresh(None, None);
        assert_eq!((d.all(&HashSet::new()), d.folders.len()), (0, 0));
    }
}
