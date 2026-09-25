//! Which file formats the Library browser shows.
//!
//! Pure and `egui`-free for the same reason `view_region` and `selection` are: the index
//! bookkeeping is where an off-by-one hides, and a wrong answer here is a photo that silently
//! is not there.

use crate::entries::PhotoEntry;
use std::collections::BTreeSet;
use std::path::Path;

/// The one name a format goes by, for extensions that have more than one spelling.
///
/// ⚠⚠ `jpeg` and `jpg` are one format, and so are `tiff` and `tif` — `catalog::IMAGE_EXTENSIONS`
/// lists both spellings of each, so without this a folder of mixed spellings grew **two chips for
/// one format** and hiding one left the other visible. Reported in use, 2026-09-23.
///
/// ⚠ The RAW extensions are **not** aliases: `cr2` and `cr3` are different formats, as are `nef`,
/// `arw`, `raf`, `dng`, `orf` and `rw2`. Only these two pairs collapse.
pub fn canonical_ext(ext: &str) -> &str {
    match ext {
        "jpeg" => "jpg",
        "tiff" => "tif",
        other => other,
    }
}

/// The canonical lowercase extension of an entry, or `None` when it has none.
fn ext_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| canonical_ext(&e.to_ascii_lowercase()).to_string())
}

/// Extensions present in a folder, lowercased, each with how many entries it has.
///
/// Sorted by extension, so the toggles do not reorder between scans of the same folder.
/// ⚠ The count is of **rows**, not files, so a file with two copies counts twice. That is what
/// the chip label means once copies exist — how many entries the toggle would hide — and counting
/// files instead would put a number on screen that does not match what disappears.
pub fn formats_present(entries: &[PhotoEntry]) -> Vec<(String, usize)> {
    formats_of(entries.iter().map(|e| e.id.path.as_path()))
}

/// [`formats_present`] over bare paths — the folder's **own** contents.
///
/// ⚠⚠ **The chips must read this, not the grouped entry list.** With stacking on, a collapsed
/// RAW+JPEG pair leaves no JPEG row at all, so the JPEG chip **vanished and reappeared** as stacks
/// were expanded and collapsed — "a bit jittery", reported in use 2026-09-25. The user's point is
/// the right one: the JPEGs are still in the folder, so the control that hides them belongs on
/// screen. Which formats a folder holds is a fact about the folder, and stacking is a fact about
/// the view.
pub fn formats_of<'a>(paths: impl Iterator<Item = &'a Path>) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for p in paths {
        if let Some(ext) = ext_of(p) {
            *counts.entry(ext).or_default() += 1;
        }
    }
    counts.into_iter().collect()
}

/// Whether a folder's own contents override the stored hidden-set.
///
/// ⚠⚠ **The ONE place this rule is written.** `shell.rs::format_bullets` draws no chips for such a
/// folder and [`crate::library_filter::LibraryFilter::effective_for`] hides nothing in it; before
/// 2026-09-17 only the first of those existed, and hiding `jpg` then opening a JPEG-only folder
/// gave an empty grid **with no control to undo it** — reported in use. Two callers reading one
/// predicate is what stops them drifting back apart into a chip that does nothing, or filtering
/// with no chip.
///
/// ⚠ Fewer than *two* rather than exactly one: an empty folder, and a folder of extension-less
/// files, both have nothing to choose between either.
pub fn folder_overrides_hidden(formats: &[(String, usize)]) -> bool {
    formats.len() < 2
}

/// Which formats are hidden from the browser.
///
/// ⚠ It stores what is **hidden**, not what is shown. With a shown-set, a format that appeared
/// after a folder reload — a new file type dropped into the directory — would be absent from
/// the set and therefore silently invisible. Hidden-by-exception makes "everything is visible"
/// the default and needs no maintenance when the folder changes.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct FormatFilter {
    hidden: BTreeSet<String>,
}

impl FormatFilter {
    /// Is this format hidden?
    ///
    /// ⚠⚠ Compares **canonically on both sides**. `hidden` is persisted in `Settings`, so a config
    /// written before 2026-09-23 can hold `"jpeg"`; matching raw strings would leave that entry
    /// matching nothing and **silently un-hide** the JPEGs somebody had chosen to hide. Canonicalising
    /// the stored value keeps it working, with no migration.
    ///
    /// ⚠ `toggle` canonicalises on insert, so only legacy values are ever non-canonical — the scan is
    /// over at most a handful of formats.
    pub fn is_hidden(&self, ext: &str) -> bool {
        let want = canonical_ext(ext);
        self.hidden.iter().any(|h| canonical_ext(h) == want)
    }

    /// True when at least one format is hidden.
    ///
    /// ⚠ Lands with its first caller (`LibraryFilter::is_active`), per this crate's rule that an
    /// item with no caller is dead code in the binary.
    pub fn is_active(&self) -> bool {
        !self.hidden.is_empty()
    }

    /// Hide or show a format.
    ///
    /// ⚠ Removes **every** spelling of it, so a legacy `"jpeg"` entry cannot survive a toggle of
    /// `"jpg"` and leave the format hidden by a value no chip corresponds to.
    pub fn toggle(&mut self, ext: &str) {
        let want = canonical_ext(ext);
        let had = self
            .hidden
            .iter()
            .filter(|h| canonical_ext(h) == want)
            .cloned()
            .collect::<Vec<_>>();
        if had.is_empty() {
            self.hidden.insert(want.to_string());
        } else {
            for h in had {
                self.hidden.remove(&h);
            }
        }
    }

    /// An entry with no extension is always visible: it cannot be filtered by a control that
    /// does not exist for it.
    pub fn is_visible(&self, path: &Path) -> bool {
        match ext_of(path) {
            Some(ext) => !self.is_hidden(&ext),
            None => true,
        }
    }

    /// Indices of the visible entries, ascending — what the grid, the filmstrip and arrow
    /// navigation iterate in place of `0..entries.len()`.
    /// ⚠ Every copy of a file shares its path, so a format toggle hides or shows **all** of a
    /// file's copies together. `hiding_a_format_hides_every_copy_of_those_files` pins that: a copy
    /// stranded in the grid after its original was hidden would be selectable, and a sync or a
    /// paste would then write to a photo that is not on screen.
    pub fn visible_indices(&self, entries: &[PhotoEntry]) -> Vec<usize> {
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.is_visible(&e.id.path))
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
    /// ⚠⚠ Two spellings, one chip. Before 2026-09-23 a folder of mixed spellings grew two chips for
    /// one format, and hiding one left the other visible.
    #[test]
    fn jpg_and_jpeg_are_one_format_and_so_are_tif_and_tiff() {
        assert_eq!(canonical_ext("jpeg"), "jpg");
        assert_eq!(canonical_ext("jpg"), "jpg");
        assert_eq!(canonical_ext("tiff"), "tif");
        assert_eq!(canonical_ext("tif"), "tif");
    }

    /// ⚠ The RAW extensions are different formats, not spellings of one.
    #[test]
    fn the_raw_extensions_are_not_aliased_together() {
        for e in [
            "cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2", "png",
        ] {
            assert_eq!(canonical_ext(e), e, "{e} must stand alone");
        }
    }

    /// ⚠⚠ Hiding the format hides **both** spellings — the bug this change exists for.
    #[test]
    fn hiding_jpg_also_hides_a_dot_jpeg_file() {
        let mut f = FormatFilter::default();
        f.toggle("jpg");
        assert!(!f.is_visible(Path::new("/a/one.jpg")));
        assert!(!f.is_visible(Path::new("/a/two.jpeg")));
        assert!(
            !f.is_visible(Path::new("/a/three.JPEG")),
            "case must not matter"
        );
        assert!(f.is_visible(Path::new("/a/four.png")));
    }

    /// ⚠⚠ **A config written before this change can hold `"jpeg"`.** Matching raw strings would leave
    /// that entry matching nothing and silently un-hide the JPEGs somebody chose to hide.
    #[test]
    fn a_legacy_jpeg_entry_still_hides_jpgs() {
        let legacy: FormatFilter =
            serde_json::from_str(r#"["jpeg"]"#).expect("the stored shape is a bare array");
        assert!(!legacy.is_visible(Path::new("/a/one.jpg")));
        assert!(!legacy.is_visible(Path::new("/a/two.jpeg")));
        assert!(legacy.is_hidden("jpg"), "the chip must read as hidden");
    }

    /// ⚠ And toggling it off must clear the legacy spelling too, or the format would stay hidden by a
    /// value no chip corresponds to.
    #[test]
    fn toggling_off_clears_a_legacy_spelling() {
        let mut legacy: FormatFilter = serde_json::from_str(r#"["jpeg"]"#).unwrap();
        legacy.toggle("jpg");
        assert!(legacy.is_visible(Path::new("/a/one.jpeg")));
        assert!(!legacy.is_active(), "nothing should remain hidden");
    }

    /// ⚠ One chip, one count: a folder of both spellings reports them together.
    #[test]
    fn formats_present_merges_the_two_spellings_into_one_row() {
        let entries = vec![
            entry("/a/one.jpg", 0),
            entry("/a/two.jpeg", 0),
            entry("/a/three.png", 0),
        ];
        let present = formats_present(&entries);
        assert_eq!(
            present,
            vec![("jpg".to_string(), 2), ("png".to_string(), 1)]
        );
    }

    use super::*;
    use std::path::PathBuf;

    /// ⚠ One row per name, all originals — the pre-existing tests below keep their exact
    /// assertions through the widening to `PhotoEntry`, which is the evidence that adding copies
    /// did not change how the filter behaves for ordinary photos.
    fn entries(names: &[&str]) -> Vec<PhotoEntry> {
        names.iter().map(|n| entry(n, 0)).collect()
    }

    fn entry(name: &str, copy: u32) -> PhotoEntry {
        PhotoEntry {
            label: None,
            id: crate::photo_id::PhotoId::new(PathBuf::from(format!("/f/{name}")), copy),
            file_name: name.to_string(),
            edits: Default::default(),
            stack: None,
        }
    }

    fn folder() -> Vec<PhotoEntry> {
        entries(&["a.RAF", "a.JPG", "b.raf", "b.jpg", "c.raf", "notes"])
    }

    #[test]
    fn a_folder_with_one_format_overrides_the_hidden_set() {
        assert!(folder_overrides_hidden(&formats_present(&entries(&[
            "a.jpg", "b.JPG", "c.jpg"
        ]))));
        assert!(folder_overrides_hidden(&formats_present(&entries(&[]))));
        // ⚠ Extension-less files contribute no format, so a folder of them has nothing to choose.
        assert!(folder_overrides_hidden(&formats_present(&entries(&[
            "notes", "readme"
        ]))));
    }

    #[test]
    fn a_mixed_folder_does_not_override() {
        assert!(!folder_overrides_hidden(&formats_present(&folder())));
        assert!(!folder_overrides_hidden(&formats_present(&entries(&[
            "a.jpg", "b.raf"
        ]))));
        // ⚠ Case-folded, so `.JPG` and `.jpg` are ONE format and this folder still overrides.
        assert!(folder_overrides_hidden(&formats_present(&entries(&[
            "a.JPG", "b.jpg"
        ]))));
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

    #[test]
    fn hiding_a_format_hides_every_copy_of_those_files() {
        // ⚠ Copies share a path, so they share a format. A copy left in the grid after its
        // original was hidden would still be selectable, and a sync or a paste would then write
        // to a photo that is not on screen — the same hazard `retain_visible` exists for.
        let entries = vec![entry("a.RAF", 0), entry("a.RAF", 1), entry("b.jpg", 0)];
        let mut f = FormatFilter::default();
        f.toggle("raf");
        assert_eq!(f.visible_indices(&entries), vec![2]);
    }

    #[test]
    fn the_format_counts_are_of_rows_not_files() {
        // The chip says how many entries the toggle would hide; a file with two copies is two.
        let entries = vec![entry("a.RAF", 0), entry("a.RAF", 1), entry("b.jpg", 0)];
        assert_eq!(
            formats_present(&entries),
            vec![("jpg".to_string(), 1), ("raf".to_string(), 2)]
        );
    }
}
