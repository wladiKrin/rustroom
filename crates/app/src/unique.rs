//! De-duplication in first-seen order, through a hash set (M10.14e).
//!
//! ⚠⚠ **Never a `Vec` searched once per item.** Eight places did that over a whole view or a whole
//! selection — paths in `collections::scan_of`, folders in the marks, captures and metadata reloads,
//! in `smart_members` and in the XMP import, files in the XMP import and writer — and at 100,000
//! photos opening All Photos took **204 s** and a smart collection ~190 s (`catalog_scale`). One
//! helper, so the next caller has no reason to write the quadratic one.

use std::collections::HashSet;
use std::hash::Hash;
use std::path::{Path, PathBuf};

/// The folders holding `paths`, each once, in the order first seen. A path with no parent adds none.
pub fn folders_of<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<PathBuf> {
    first_of_each(paths.into_iter().filter_map(Path::parent))
        .into_iter()
        .map(Path::to_path_buf)
        .collect()
}

/// `items` with repeats dropped — the first of each kept, in order.
pub fn first_of_each<T: Eq + Hash + Clone>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_once_each_in_first_seen_order() {
        let paths = ["/b/2.jpg", "/a/1.jpg", "/b/3.jpg", "/c/4.jpg", "/a/5.jpg"].map(PathBuf::from);
        assert_eq!(
            folders_of(paths.iter().map(|p| p.as_path())),
            ["/b", "/a", "/c"].map(PathBuf::from).to_vec()
        );
    }

    #[test]
    fn a_path_without_a_parent_adds_no_folder() {
        assert!(folders_of([Path::new("")]).is_empty());
    }

    #[test]
    fn the_first_of_each_is_kept_in_order() {
        assert_eq!(first_of_each([3, 1, 3, 2, 1, 4]), vec![3, 1, 2, 4]);
        assert_eq!(first_of_each(Vec::<u8>::new()), Vec::<u8>::new());
    }

    /// ⚠ The regression guard: 200,000 items, every one distinct, in a bound a hash set clears by
    /// two orders of magnitude (tens of ms) and a per-item `Vec` search never could — that shape
    /// needs ~2·10¹⁰ comparisons here, minutes rather than seconds.
    #[test]
    fn two_hundred_thousand_distinct_items_take_well_under_seconds() {
        let paths: Vec<PathBuf> = (0..200_000)
            .map(|i| PathBuf::from(format!("/photos/{}/{i}.jpg", i / 70)))
            .collect();
        let t = std::time::Instant::now();
        let files = first_of_each(paths.iter().map(|p| p.as_path()));
        let folders = folders_of(paths.iter().map(|p| p.as_path()));
        let took = t.elapsed();
        assert_eq!(files.len(), 200_000);
        assert_eq!(folders.len(), 200_000 / 70 + 1);
        assert!(took.as_secs_f64() < 5.0, "took {took:?}");
    }
}
