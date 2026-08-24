//! Which file formats the Library browser shows.
//!
//! Pure and `egui`-free for the same reason `view_region` and `selection` are: the index
//! bookkeeping is where an off-by-one hides, and a wrong answer here is a photo that silently
//! is not there.

use catalog::CatalogEntry;
use std::collections::BTreeSet;

/// The lowercase extension of an entry, or `None` when it has none.
fn ext_of(entry: &CatalogEntry) -> Option<String> {
    entry
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Extensions present in a folder, lowercased, each with how many entries it has.
///
/// Sorted by extension, so the toggles do not reorder between scans of the same folder.
pub fn formats_present(entries: &[CatalogEntry]) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for e in entries {
        if let Some(ext) = ext_of(e) {
            *counts.entry(ext).or_default() += 1;
        }
    }
    counts.into_iter().collect()
}

/// Which formats are hidden from the browser.
///
/// ⚠ It stores what is **hidden**, not what is shown. With a shown-set, a format that appeared
/// after a folder reload — a new file type dropped into the directory — would be absent from
/// the set and therefore silently invisible. Hidden-by-exception makes "everything is visible"
/// the default and needs no maintenance when the folder changes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FormatFilter {
    hidden: BTreeSet<String>,
}

impl FormatFilter {
    pub fn is_hidden(&self, ext: &str) -> bool {
        self.hidden.contains(ext)
    }

    pub fn toggle(&mut self, ext: &str) {
        if !self.hidden.remove(ext) {
            self.hidden.insert(ext.to_string());
        }
    }

    /// An entry with no extension is always visible: it cannot be filtered by a control that
    /// does not exist for it.
    pub fn is_visible(&self, entry: &CatalogEntry) -> bool {
        match ext_of(entry) {
            Some(ext) => !self.hidden.contains(&ext),
            None => true,
        }
    }

    /// Indices of the visible entries, ascending — what the grid, the filmstrip and arrow
    /// navigation iterate in place of `0..entries.len()`.
    pub fn visible_indices(&self, entries: &[CatalogEntry]) -> Vec<usize> {
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.is_visible(e))
            .map(|(i, _)| i)
            .collect()
    }
}

/// The neighbour of `cur` within `visible`, one step in the given direction, clamped at both
/// ends. `None` only when nothing is visible.
///
/// ⚠ `cur` need not itself be visible. The photo open in Develop is not closed when its format
/// is hidden, so arrow navigation has to start from an index that may no longer be in the list
/// — from which the nearest visible one in the direction of travel is the useful answer.
pub fn step(visible: &[usize], cur: usize, forward: bool) -> Option<usize> {
    if visible.is_empty() {
        return None;
    }
    let last = visible.len() - 1;
    let pos = match visible.binary_search(&cur) {
        // `cur` is visible: move off it.
        Ok(p) if forward => (p + 1).min(last),
        Ok(p) => p.saturating_sub(1),
        // `cur` is hidden: `p` is how many visible indices sit below it, so `p` is already the
        // next one above and `p - 1` the previous one below.
        Err(p) if forward => p.min(last),
        Err(p) => p.saturating_sub(1),
    };
    Some(visible[pos])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entries(names: &[&str]) -> Vec<CatalogEntry> {
        names
            .iter()
            .map(|n| CatalogEntry {
                path: PathBuf::from(format!("/f/{n}")),
                file_name: n.to_string(),
            })
            .collect()
    }

    fn folder() -> Vec<CatalogEntry> {
        entries(&["a.RAF", "a.JPG", "b.raf", "b.jpg", "c.raf", "notes"])
    }

    #[test]
    fn formats_present_counts_each_extension() {
        // Lowercased and deduped, so `.RAF` and `.raf` are one toggle; sorted so the chips do
        // not reorder between scans; an extensionless file contributes no toggle.
        assert_eq!(
            formats_present(&folder()),
            vec![("jpg".to_string(), 2), ("raf".to_string(), 3)]
        );
    }

    #[test]
    fn hiding_a_format_removes_only_its_entries() {
        let f = folder();
        let mut filter = FormatFilter::default();
        filter.toggle("raf");
        // Indices into the *original* list, ascending — including the extensionless entry.
        assert_eq!(filter.visible_indices(&f), vec![1, 3, 5]);
        assert!(filter.is_hidden("raf"));
    }

    #[test]
    fn an_unknown_extension_defaults_to_visible() {
        // The hidden-not-shown choice, as a property: a format the filter has never seen — a
        // file type dropped into the folder after the scan — must still be listed.
        let f = entries(&["x.png", "y.raf"]);
        let mut filter = FormatFilter::default();
        filter.toggle("raf");
        assert_eq!(filter.visible_indices(&f), vec![0]);
        assert!(!filter.is_hidden("png"));
    }

    #[test]
    fn toggling_twice_restores_the_folder() {
        let f = folder();
        let all: Vec<usize> = (0..f.len()).collect();
        let mut filter = FormatFilter::default();
        assert_eq!(filter.visible_indices(&f), all);
        filter.toggle("jpg");
        assert_ne!(filter.visible_indices(&f), all);
        filter.toggle("jpg");
        assert_eq!(filter.visible_indices(&f), all);
    }

    #[test]
    fn stepping_moves_within_the_visible_list() {
        let visible = vec![1, 3, 5];
        assert_eq!(step(&visible, 1, true), Some(3));
        assert_eq!(step(&visible, 3, false), Some(1));
        // Clamped at both ends rather than wrapping.
        assert_eq!(step(&visible, 5, true), Some(5));
        assert_eq!(step(&visible, 1, false), Some(1));
        assert_eq!(step(&[], 0, true), None);
    }

    #[test]
    fn stepping_from_a_hidden_index_lands_on_a_neighbour() {
        // ⚠ The case that exists because Develop keeps the open photo when its format is
        // hidden: `cur` is not in `visible`.
        let visible = vec![1, 3, 5];
        assert_eq!(step(&visible, 2, true), Some(3), "next above");
        assert_eq!(step(&visible, 2, false), Some(1), "previous below");
        assert_eq!(step(&visible, 0, false), Some(1), "clamped at the start");
        assert_eq!(step(&visible, 9, true), Some(5), "clamped at the end");
    }
}
