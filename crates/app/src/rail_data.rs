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
use std::sync::Arc;

#[derive(Default)]
pub struct RailData {
    /// `(instance, change_count)` of the catalog last read, or `None` before the first read.
    key: Option<(u64, u64)>,
    pub all: i64,
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
    pub fn refresh(&mut self, cat: Option<&Catalog>) {
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
        self.all = cat.photo_count().unwrap_or(0);
        self.folders = Arc::new(cat.folders().unwrap_or_default());
        self.collections = Arc::new(cat.collections().unwrap_or_default());
        self.smarts = Arc::new(cat.smart_collections().unwrap_or_default());
        self.keywords = Arc::new(cat.keywords().unwrap_or_default());
        self.days = Arc::new(cat.capture_days().unwrap_or_default());
        self.computed += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        d.refresh(Some(&cat));
        assert_eq!((d.computed, d.all, d.folders.len()), (1, 3, 1));
        d.refresh(Some(&cat));
        d.refresh(Some(&cat));
        assert_eq!(d.computed, 1, "no write, no read");
        cat.create_keyword("Paris", None).unwrap();
        d.refresh(Some(&cat));
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
        d.refresh(Some(&a));
        d.refresh(Some(&b));
        assert_eq!(d.computed, 2);
        assert_eq!(d.folders[0].path, Path::new("/photos/b"));
    }

    #[test]
    fn no_catalog_shows_nothing() {
        let cat = catalog_with_photos(4);
        let mut d = RailData::default();
        d.refresh(Some(&cat));
        d.refresh(None);
        assert_eq!((d.all, d.folders.len()), (0, 0));
    }
}
