//! Which photos are selected, and which one a sync copies *from*.
//!
//! Pure and `egui`-free so the range arithmetic and clamping are testable rather than only
//! visible as a wrong highlight — the same reasoning behind `crop_tool` and `view_region`.

use std::collections::BTreeSet;

/// A multi-selection with a distinguished **active** photo: the one open in Develop, and
/// the source for a sync.
///
/// ⚠ Invariant, asserted after every operation in the tests: `active == Some(i)` implies
/// `i` is selected, and `active == None` implies nothing is selected.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    active: Option<usize>,
    set: BTreeSet<usize>,
    /// Where a shift-click measures its range from. Moved by plain clicks and Cmd-clicks,
    /// *not* by shift-clicks, so repeated shift-clicks re-measure from the same origin.
    anchor: Option<usize>,
}

impl Selection {
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    pub fn contains(&self, i: usize) -> bool {
        self.set.contains(&i)
    }

    /// Selected indices, ascending.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.set.iter().copied()
    }

    /// Everything selected except the active photo — a sync's targets.
    pub fn others(&self) -> impl Iterator<Item = usize> + '_ {
        let active = self.active;
        self.set.iter().copied().filter(move |&i| Some(i) != active)
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Plain click: this becomes the active photo and the entire selection.
    pub fn click(&mut self, i: usize) {
        self.active = Some(i);
        self.set.clear();
        self.set.insert(i);
        self.anchor = Some(i);
    }

    /// Cmd/Ctrl-click: add or remove.
    ///
    /// ⚠ Never touches the active photo. Deselecting the source would break the invariant,
    /// and moving the active photo here would kick off a RAW decode mid-selection.
    pub fn toggle(&mut self, i: usize) {
        if self.active == Some(i) {
            return;
        }
        if !self.set.remove(&i) {
            self.set.insert(i);
        }
        self.anchor = Some(i);
    }

    /// Make an **already-selected** photo the active one, leaving the set alone.
    ///
    /// ⚠⚠ This exists for Survey, where the selection is the *content of the view* rather than a
    /// target set: clicking a tile there must move the focus without collapsing the survey to
    /// one photo, which is what [`Selection::click`] would do. Returns whether it moved.
    ///
    /// ⚠ It refuses an unselected index rather than selecting it, which is what keeps the
    /// invariant — an active photo is always selected — true by construction rather than by the
    /// caller remembering.
    pub fn focus(&mut self, i: usize) -> bool {
        if !self.set.contains(&i) {
            return false;
        }
        self.active = Some(i);
        self.anchor = Some(i);
        true
    }

    /// Shift-click: the inclusive range from the anchor to `i`.
    ///
    /// The active photo is re-inserted afterwards because a preceding Cmd-click can have
    /// moved the anchor away from it, and the range alone would then drop the source.
    pub fn extend_to(&mut self, i: usize) {
        let from = self.anchor.or(self.active).unwrap_or(i);
        let (lo, hi) = (from.min(i), from.max(i));
        self.set.clear();
        self.set.extend(lo..=hi);
        match self.active {
            Some(a) => {
                self.set.insert(a);
            }
            None => self.active = Some(i),
        }
    }

    /// Select exactly `indices`. With no active photo among them, the first becomes active —
    /// otherwise this would produce a non-empty set with no source and break the invariant.
    ///
    /// ⚠ Unlike [`Selection::retain_visible`], this *does* move the active photo when the
    /// current one is not in the set — and the difference is deliberate. Ctrl+A is an explicit
    /// "select these" from the user, so taking a source from what they asked for is expected;
    /// a filter change is a side effect, where silently moving which photo a sync copies
    /// **from** would go unnoticed.
    pub fn select_indices(&mut self, indices: &[usize]) {
        self.set = indices.iter().copied().collect();
        if self.active.is_none_or(|a| !self.set.contains(&a)) {
            self.active = self.set.iter().next().copied();
            self.anchor = self.active;
        }
        if self.active.is_none() {
            self.set.clear();
        }
    }

    /// Back to just the active photo; empty if there is none.
    pub fn clear(&mut self) {
        self.set.clear();
        if let Some(a) = self.active {
            self.set.insert(a);
        }
        self.anchor = self.active;
    }

    /// Drop indices past `n`, after the folder's contents changed.
    pub fn clamp(&mut self, n: usize) {
        self.set.retain(|&i| i < n);
        if self.active.is_some_and(|a| a >= n) {
            self.active = None;
        }
        if self.active.is_none() {
            // Targets with no source are not a usable state.
            self.set.clear();
        }
        self.anchor = self.anchor.filter(|&a| a < n);
    }

    /// Drop every selected index that is not in `visible`, after the format filter changed.
    ///
    /// ⚠ Deliberately the same rule as [`Selection::clamp`]: if the *active* photo is dropped,
    /// the whole selection goes. Two different rules for "the active index went away" would be
    /// a smell, and silently moving the active photo elsewhere changes which photo a sync
    /// copies **from** — a change with nothing on screen to announce it.
    ///
    /// `visible` must be ascending, which is what [`crate::format_filter::FormatFilter`]
    /// produces.
    pub fn retain_visible(&mut self, visible: &[usize]) {
        self.set.retain(|i| visible.binary_search(i).is_ok());
        if self
            .active
            .is_some_and(|a| visible.binary_search(&a).is_err())
        {
            self.active = None;
        }
        if self.active.is_none() {
            // Targets with no source are not a usable state.
            self.set.clear();
        }
        self.anchor = self.anchor.filter(|a| visible.binary_search(a).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ The invariant every method must preserve: an active photo is always selected, and
    /// no active photo means nothing is selected. Sync would otherwise have a source that
    /// is not part of what it is syncing, or a selection with no source at all.
    fn check(s: &Selection) {
        match s.active() {
            Some(a) => assert!(s.contains(a), "active {a} is not in the selection"),
            None => assert!(
                s.is_empty(),
                "no active photo but the selection is non-empty"
            ),
        }
    }

    fn items(s: &Selection) -> Vec<usize> {
        s.iter().collect()
    }

    #[test]
    fn focusing_a_selected_photo_moves_the_active_one_and_keeps_the_set() {
        // ⚠⚠ The whole reason `focus` is not `click`: in Survey the selection *is* what the view
        // is showing, so moving the focus must not collapse it to one photo.
        let mut s = Selection::default();
        s.select_indices(&[1, 3, 5]);
        assert!(s.focus(5));
        assert_eq!(s.active(), Some(5));
        assert_eq!(items(&s), vec![1, 3, 5], "the survey lost its other photos");
        check(&s);
    }

    #[test]
    fn focusing_an_unselected_photo_is_refused() {
        // ⚠ Refused rather than selecting it, which is what makes the invariant hold by
        // construction. Accepting it would leave an active photo outside the set — the state
        // every other method in this file exists to prevent.
        let mut s = Selection::default();
        s.select_indices(&[1, 3]);
        assert!(!s.focus(9));
        assert_eq!(s.active(), Some(1));
        assert_eq!(items(&s), vec![1, 3]);
        check(&s);
    }

    #[test]
    fn pruning_drops_hidden_entries_from_the_selection() {
        // ⚠ The data-loss guard. Select photos, hide their format, and without this the
        // selection still holds them — so a sync or a paste writes to photos that are not on
        // screen. Silent, and found as data loss rather than as a bug.
        let mut s = Selection::default();
        s.click(1);
        s.toggle(3);
        s.toggle(5);
        s.retain_visible(&[1, 3]);
        check(&s);
        assert_eq!(items(&s), vec![1, 3]);
        assert_eq!(s.active(), Some(1), "a surviving active photo stays put");
    }

    #[test]
    fn pruning_the_active_photo_clears_the_selection() {
        // Follows `clamp`: an active photo that goes away takes the selection with it, rather
        // than silently moving to a different photo — which would change which photo a
        // subsequent sync copies *from*, with nothing on screen to say so.
        let mut s = Selection::default();
        s.click(5);
        s.toggle(1);
        s.retain_visible(&[1, 3]);
        check(&s);
        assert_eq!(s.active(), None);
        assert!(
            s.is_empty(),
            "targets with no source are not a usable state"
        );
    }

    #[test]
    fn pruning_everything_empties_the_selection() {
        let mut s = Selection::default();
        s.click(2);
        s.retain_visible(&[]);
        check(&s);
        assert_eq!(s.active(), None);
        assert!(s.is_empty());
    }

    #[test]
    fn pruning_nothing_is_a_no_op() {
        let mut s = Selection::default();
        s.click(1);
        s.toggle(2);
        let before = s.clone();
        s.retain_visible(&[0, 1, 2, 3]);
        check(&s);
        assert_eq!(s, before);
    }

    #[test]
    fn a_plain_click_replaces_everything() {
        let mut s = Selection::default();
        s.click(3);
        check(&s);
        assert_eq!(s.active(), Some(3));
        assert_eq!(items(&s), vec![3]);
        s.toggle(5);
        s.click(1);
        check(&s);
        assert_eq!(
            items(&s),
            vec![1],
            "a plain click must not keep the old selection"
        );
    }

    #[test]
    fn toggle_adds_and_removes_but_never_the_active_photo() {
        let mut s = Selection::default();
        s.click(2);
        s.toggle(4);
        check(&s);
        assert_eq!(items(&s), vec![2, 4]);
        s.toggle(4);
        check(&s);
        assert_eq!(items(&s), vec![2]);
        // The source cannot be deselected — the price of "modifiers never move the active".
        s.toggle(2);
        check(&s);
        assert_eq!(items(&s), vec![2]);
        assert_eq!(s.active(), Some(2));
    }

    #[test]
    fn extend_works_in_both_directions_and_keeps_the_active_photo() {
        let mut s = Selection::default();
        s.click(5);
        s.extend_to(8);
        check(&s);
        assert_eq!(items(&s), vec![5, 6, 7, 8]);
        assert_eq!(
            s.active(),
            Some(5),
            "shift-click must not move the active photo"
        );

        let mut back = Selection::default();
        back.click(5);
        back.extend_to(2);
        check(&back);
        assert_eq!(items(&back), vec![2, 3, 4, 5]);
    }

    #[test]
    fn repeated_shift_clicks_extend_from_the_same_anchor() {
        // Standard behaviour: the second shift-click replaces the range rather than
        // growing from wherever the last one landed.
        let mut s = Selection::default();
        s.click(4);
        s.extend_to(8);
        s.extend_to(6);
        check(&s);
        assert_eq!(items(&s), vec![4, 5, 6]);
    }

    #[test]
    fn extend_keeps_the_active_photo_when_the_anchor_moved_away() {
        // A Cmd-click moves the anchor without moving the active photo, so the range alone
        // would drop the source.
        let mut s = Selection::default();
        s.click(0);
        s.toggle(7); // anchor -> 7, active still 0
        s.extend_to(9);
        check(&s);
        assert_eq!(items(&s), vec![0, 7, 8, 9]);
        assert_eq!(s.active(), Some(0));
    }

    #[test]
    fn select_indices_adopts_the_first_only_when_there_is_no_active_photo() {
        let mut fresh = Selection::default();
        fresh.select_indices(&[0, 1, 2, 3]);
        check(&fresh);
        assert_eq!(fresh.active(), Some(0));
        assert_eq!(items(&fresh), vec![0, 1, 2, 3]);

        let mut existing = Selection::default();
        existing.click(2);
        existing.select_indices(&[0, 1, 2, 3]);
        check(&existing);
        assert_eq!(
            existing.active(),
            Some(2),
            "an existing source must be kept"
        );

        let mut none = Selection::default();
        none.select_indices(&[]);
        check(&none);
        assert_eq!(none.active(), None);
    }

    #[test]
    fn select_indices_takes_a_source_from_a_sparse_set() {
        // Ctrl+A over a filtered folder: the visible list is sparse, and an active photo
        // outside it must be replaced rather than left dangling. ⚠ This is the deliberate
        // difference from `retain_visible`, which clears instead — see the doc comment.
        let mut s = Selection::default();
        s.click(0);
        s.select_indices(&[1, 3, 5]);
        check(&s);
        assert_eq!(items(&s), vec![1, 3, 5]);
        assert_eq!(s.active(), Some(1), "the first visible becomes the source");
    }

    #[test]
    fn clear_leaves_exactly_the_active_photo() {
        let mut s = Selection::default();
        s.click(1);
        s.extend_to(5);
        s.clear();
        check(&s);
        assert_eq!(items(&s), vec![1]);

        let mut empty = Selection::default();
        empty.clear();
        check(&empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn clamp_drops_out_of_range_indices() {
        let mut s = Selection::default();
        s.click(1);
        s.extend_to(6);
        s.clamp(4);
        check(&s);
        assert_eq!(items(&s), vec![1, 2, 3]);
    }

    #[test]
    fn clamp_empties_the_selection_when_the_active_photo_is_gone() {
        // A folder that shrank out from under the selection: keeping targets with no
        // source would leave a sync with nothing to sync *from*.
        let mut s = Selection::default();
        s.click(9);
        s.toggle(2);
        s.clamp(4);
        check(&s);
        assert!(s.is_empty());
        assert_eq!(s.active(), None);
    }

    #[test]
    fn others_excludes_the_active_photo() {
        // Exactly a sync's target list.
        let mut s = Selection::default();
        s.click(2);
        s.extend_to(5);
        assert_eq!(s.others().collect::<Vec<_>>(), vec![3, 4, 5]);
        assert_eq!(s.len(), 4);
    }
}
