//! Folders marked in the Library's Folders rail, for an action on all of them at once (M10.19c).
//!
//! ⚠ Keyed by **path**, never by row index: rows move whenever a parent collapses or a folder is
//! added, and a mark must stay on the folder it was made on. The gestures are the Dates section's own
//! (`smart_sources::click_period`): ⌘ toggles, ⇧ extends a range anchored on the last non-⇧ click.
//!
//! ⚠ No egui — the rail (`folder_rows`) reads the modifiers and calls these.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The marks, the anchor a ⇧-click measures from, and the open folder they were last reset for.
#[derive(Debug, Default, Clone)]
pub struct FolderMarks {
    marks: BTreeSet<PathBuf>,
    anchor: Option<PathBuf>,
    /// The open folder the marks were last reset for — `None` until the first [`Self::follow`].
    followed: Option<Option<PathBuf>>,
}

impl FolderMarks {
    /// A plain click: only `path` is marked, and the anchor moves there.
    pub fn click(&mut self, path: &Path) {
        self.marks = [path.to_path_buf()].into();
        self.anchor = Some(path.to_path_buf());
    }

    /// ⌘-click: mark or unmark `path`; the anchor moves there.
    pub fn toggle(&mut self, path: &Path) {
        if !self.marks.remove(path) {
            self.marks.insert(path.to_path_buf());
        }
        self.anchor = Some(path.to_path_buf());
    }

    /// ⇧-click: every folder from the anchor to `path` inclusive, in `order` — the rail's visible
    /// order. The anchor stays, so a second ⇧-click re-measures from the same place.
    ///
    /// ⚠ With no anchor, or one no longer in `order` (forgotten, or hidden by a collapse), it is a
    /// plain click — never a range from nowhere.
    pub fn extend_to(&mut self, path: &Path, order: &[PathBuf]) {
        let at = |q: &Path| order.iter().position(|o| o == q);
        match (self.anchor.as_deref().and_then(at), at(path)) {
            (Some(a), Some(b)) => {
                self.marks = order[a.min(b)..=a.max(b)].iter().cloned().collect();
            }
            _ => self.click(path),
        }
    }

    /// Once per frame, with the folder the Library shows (`None` for anything else). **When that
    /// changes the marks reset to it**, so opening a folder marks just it, leaving for All Photos
    /// clears them, and a mark never outlives the context it was made in. ⌘ and ⇧ never open
    /// anything, so they never reset.
    pub fn follow(&mut self, open: Option<&Path>) {
        let open = open.map(Path::to_path_buf);
        if self.followed.as_ref() != Some(&open) {
            self.marks = open.iter().cloned().collect();
            self.anchor = open.clone();
            self.followed = Some(open);
        }
    }

    /// Drop the marks of folders that no longer exist — after a forget or a relink — so the menu's
    /// count is never inflated by a folder you cannot see.
    pub fn retain(&mut self, keep: impl Fn(&Path) -> bool) {
        self.marks.retain(|p| keep(p));
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.marks.contains(path)
    }

    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What a right-click on `row` acts on: **every mark** when `row` is marked and more than one
    /// is, else `row` alone. ⚠ A right-click on an unmarked row never acts on the marks.
    pub fn targets(&self, row: &Path) -> Vec<PathBuf> {
        if self.marks.len() > 1 && self.marks.contains(row) {
            self.marks.iter().cloned().collect()
        } else {
            vec![row.to_path_buf()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn order() -> Vec<PathBuf> {
        ["/a", "/a/x", "/b", "/c"].iter().map(|s| p(s)).collect()
    }

    /// Review Focus 5: a plain click on one of several marks leaves only it.
    #[test]
    fn a_plain_click_leaves_only_that_folder() {
        let mut m = FolderMarks::default();
        m.click(&p("/a"));
        m.toggle(&p("/b"));
        m.click(&p("/c"));
        assert_eq!(m.targets(&p("/c")), vec![p("/c")]);
        assert!(!m.contains(&p("/a")) && !m.contains(&p("/b")));
    }

    #[test]
    fn a_command_click_toggles_and_moves_the_anchor() {
        let mut m = FolderMarks::default();
        m.click(&p("/a"));
        m.toggle(&p("/c"));
        assert_eq!(m.len(), 2);
        m.extend_to(&p("/b"), &order());
        assert_eq!(
            m.targets(&p("/b")),
            vec![p("/b"), p("/c")],
            "the range runs from the ⌘-clicked /c"
        );
        m.toggle(&p("/c"));
        assert!(!m.contains(&p("/c")));
    }

    #[test]
    fn a_shift_click_marks_the_visible_range_and_keeps_the_anchor() {
        let mut m = FolderMarks::default();
        m.click(&p("/a"));
        m.extend_to(&p("/b"), &order());
        assert_eq!(m.targets(&p("/a")), vec![p("/a"), p("/a/x"), p("/b")]);
        m.extend_to(&p("/c"), &order());
        assert_eq!(
            m.len(),
            4,
            "a second ⇧-click re-measures from the same anchor"
        );
    }

    /// Review Focus 2.
    #[test]
    fn a_range_from_a_vanished_anchor_is_a_click() {
        let mut m = FolderMarks::default();
        m.click(&p("/gone"));
        m.extend_to(&p("/b"), &order());
        assert_eq!(m.targets(&p("/b")), vec![p("/b")]);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn follow_resets_only_when_the_open_folder_changes() {
        let mut m = FolderMarks::default();
        m.follow(None);
        m.toggle(&p("/a"));
        m.toggle(&p("/b"));
        m.follow(None);
        assert_eq!(m.len(), 2, "marking in All Photos survives the next frame");
        m.follow(Some(&p("/c")));
        assert_eq!(
            m.targets(&p("/c")),
            vec![p("/c")],
            "opening a folder marks just it"
        );
        m.toggle(&p("/a"));
        m.follow(Some(&p("/c")));
        assert_eq!(m.len(), 2, "the same open folder resets nothing");
        m.follow(None);
        assert_eq!(m.len(), 0, "leaving for All Photos clears the marks");
    }

    /// Review Focus 4.
    #[test]
    fn retain_drops_marks_for_missing_folders() {
        let mut m = FolderMarks::default();
        m.click(&p("/a"));
        m.toggle(&p("/b"));
        m.retain(|q| q != Path::new("/b"));
        assert_eq!(m.targets(&p("/a")), vec![p("/a")]);
    }

    #[test]
    fn targets_are_the_marks_only_for_a_marked_row_among_several() {
        let mut m = FolderMarks::default();
        m.click(&p("/a"));
        m.toggle(&p("/b"));
        assert_eq!(m.targets(&p("/b")), vec![p("/a"), p("/b")]);
        assert_eq!(
            m.targets(&p("/c")),
            vec![p("/c")],
            "an unmarked row acts alone"
        );
        m.toggle(&p("/b"));
        assert_eq!(m.targets(&p("/a")), vec![p("/a")]);
    }
}
