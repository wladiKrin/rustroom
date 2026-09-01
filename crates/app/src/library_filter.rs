//! Everything that narrows what the Library shows, in one place.
//!
//! ⚠⚠ **One derived `visible` list, never a second filtering mechanism.** `Library::refilter`
//! composes this into the single list the grid, the filmstrip, `Selection` and arrow navigation
//! all iterate. Sparse-versus-dense index confusion from a *second* mechanism is this project's
//! third-most-repeated bug — the grid row break, the crop feedback loop, `render_max_dim`.
//!
//! ⚠ `FormatFilter` is **contained, not absorbed**. That module is tested and its
//! *hidden-by-exception* rule is deliberate — it stores what is *hidden*, so a format appearing
//! later is visible by default and needs no maintenance. Merging it would churn a working thing.

use crate::entries::{self, PhotoEntry};
use crate::format_filter::FormatFilter;
use catalog::db::{Flag, Marks};
use core_image::Capture;
use std::collections::BTreeSet;

/// Which star ratings to show, as a bitmask over 0..=5.
///
/// ⚠⚠ **Exact ratings, not a minimum — and that is what makes stars consistent with the rest.**
/// Flag and label already match exactly (`Some(Flag::Pick)` means picked, not "picked or better"),
/// so a minimum made stars the odd one out *and* made "only the one-star ones" unexpressible.
/// A set subsumes a minimum: 3-or-more is 3, 4 and 5.
///
/// Empty matches everything, the same convention `flag: None` and `label: None` use.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RatingSet(u8);

impl RatingSet {
    pub const MAX: u8 = 5;

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, rating: u8) -> bool {
        rating <= Self::MAX && self.0 & (1 << rating) != 0
    }

    pub fn toggle(&mut self, rating: u8) {
        if rating <= Self::MAX {
            self.0 ^= 1 << rating;
        }
    }

    /// The selected ratings, ascending.
    pub fn ratings(self) -> Vec<u8> {
        (0..=Self::MAX).filter(|&r| self.contains(r)).collect()
    }
}

/// Which colour labels to show, as a bitmask over 0..=`Marks::MAX_LABEL`, where 0 is unlabelled.
///
/// ⚠ Same shape and same convention as [`RatingSet`]: empty means "any", and 0 is a real value
/// meaning *unlabelled*, exactly as 0 stars means *unrated*.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LabelSet(u8);

impl LabelSet {
    /// ⚠ Four colours, not five — keys `6`–`9` are what set a label, so a fifth would be
    /// unreachable. Taken from `Marks::MAX_LABEL` so the two cannot drift.
    pub const MAX: u8 = Marks::MAX_LABEL;

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, label: u8) -> bool {
        label <= Self::MAX && self.0 & (1 << label) != 0
    }

    pub fn toggle(&mut self, label: u8) {
        if label <= Self::MAX {
            self.0 ^= 1 << label;
        }
    }

    pub fn labels(self) -> Vec<u8> {
        (0..=Self::MAX).filter(|&l| self.contains(l)).collect()
    }
}

/// Which flags to show. ⚠ Empty means "any"; `Flag::None` is a real value meaning *unflagged*.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FlagSet(u8);

impl FlagSet {
    /// ⚠ **Unset first, then the values** — the one ordering every section of this filter uses:
    /// unflagged / unrated / unlabelled leads, then the marks themselves. Two sections ordering
    /// differently is what prompted this, and the ordering lives here so the UI cannot re-invent
    /// it.
    pub const ALL: [Flag; 3] = [Flag::None, Flag::Pick, Flag::Reject];

    fn bit(flag: Flag) -> u8 {
        match flag {
            Flag::None => 1,
            Flag::Pick => 2,
            Flag::Reject => 4,
        }
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, flag: Flag) -> bool {
        self.0 & Self::bit(flag) != 0
    }

    pub fn toggle(&mut self, flag: Flag) {
        self.0 ^= Self::bit(flag);
    }

    pub fn flags(self) -> Vec<Flag> {
        Self::ALL
            .into_iter()
            .filter(|&f| self.contains(f))
            .collect()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LibraryFilter {
    pub format: FormatFilter,
    /// Case-insensitive substring of the row's shown name. Empty matches everything.
    pub text: String,
    /// Which star ratings to show. Empty matches everything.
    pub stars: RatingSet,
    /// Which flags to show. Empty matches everything; `Flag::None` means *unflagged*.
    pub flags: FlagSet,
    /// Which colour labels to show. Empty matches everything; 0 means *unlabelled*.
    pub labels: LabelSet,
    /// Which cameras to show. Empty matches everything.
    ///
    /// ⚠ A `BTreeSet<String>` rather than a bitmask, unlike the mark filters: these values are
    /// open-ended strings discovered from the catalog, not a fixed small enum.
    pub cameras: BTreeSet<String>,
    /// Which lenses to show. Empty matches everything.
    pub lenses: BTreeSet<String>,
}

impl LibraryFilter {
    /// Does this row survive the filter?
    ///
    /// ⚠ A conjunction: every criterion must pass. `marks` is the row's marks, or the default
    /// when it has none — "unrated, unflagged, unlabelled" is a real value, not a missing one,
    /// which is why `Marks` defaults to zero and no special case is needed here.
    pub fn matches(&self, entry: &PhotoEntry, marks: Marks, capture: &Capture) -> bool {
        if !self.format.is_visible(&entry.id.path) {
            return false;
        }
        if !self.stars.is_empty() && !self.stars.contains(marks.rating) {
            return false;
        }
        if !self.flags.is_empty() && !self.flags.contains(marks.flag) {
            return false;
        }
        if !self.labels.is_empty() && !self.labels.contains(marks.label) {
            return false;
        }
        // ⚠ A photo with **no camera recorded fails a non-empty camera filter**, and that is
        // right: "show me the X-T4 shots" must not include a file whose camera is unknown.
        // `is_some_and` says exactly that.
        if !self.cameras.is_empty()
            && !capture
                .camera
                .as_deref()
                .is_some_and(|c| self.cameras.contains(c))
        {
            return false;
        }
        if !self.lenses.is_empty()
            && !capture
                .lens
                .as_deref()
                .is_some_and(|l| self.lenses.contains(l))
        {
            return false;
        }
        if !self.text.is_empty() {
            // ⚠⚠ `shown_name`, not `file_name`. Every copy of a file shares one `file_name`, so
            // matching that would mean searching for what a copy is *called* and not finding it —
            // precisely the bug that made copy names invisible in the grid, where the Library was
            // the last consumer bypassing this shared rule.
            // ⚠ Caption **or** label: M6.0 made them two strings, and a photo is findable by
            // whichever one the user remembers. Matching only the caption would make a copy
            // labelled "B&W" unsearchable by that name.
            let needle = self.text.to_lowercase();
            let caption = entries::caption(&entry.file_name, entry.id.copy).to_lowercase();
            let label = entry.label.as_deref().unwrap_or_default().to_lowercase();
            if !caption.contains(&needle) && !label.contains(&needle) {
                return false;
            }
        }
        true
    }

    /// True when this filter hides anything at all.
    pub fn is_active(&self) -> bool {
        self.format.is_active()
            || !self.text.is_empty()
            || !self.stars.is_empty()
            || !self.flags.is_empty()
            || !self.labels.is_empty()
            || !self.cameras.is_empty()
            || !self.lenses.is_empty()
    }

    /// Clear the marks and the text, leaving the format filter alone.
    ///
    /// ⚠ The format filter answers a different question and is reset by *loading a source*. A
    /// Clear button that silently unhid a format would be doing something nobody asked for.
    pub fn clear_marks_and_text(&mut self) {
        self.text.clear();
        self.stars = RatingSet::default();
        self.flags = FlagSet::default();
        self.labels = LabelSet::default();
        self.cameras.clear();
        self.lenses.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo_id::PhotoId;
    use std::path::PathBuf;

    fn entry(name: &str, copy: u32, copy_name: Option<&str>) -> PhotoEntry {
        PhotoEntry {
            id: PhotoId::new(PathBuf::from(format!("/x/{name}")), copy),
            file_name: name.to_string(),
            label: copy_name.map(str::to_string),
            edits: Default::default(),
        }
    }

    fn marks(rating: u8, flag: Flag, label: u8) -> Marks {
        Marks {
            rating,
            flag,
            label,
        }
    }

    #[test]
    fn an_empty_filter_matches_everything() {
        let f = LibraryFilter::default();
        assert!(f.matches(
            &entry("a.RAF", 0, None),
            Marks::default(),
            &Capture::default()
        ));
        assert!(!f.is_active());
    }

    #[test]
    fn text_matches_case_insensitively_on_a_substring() {
        let mut f = LibraryFilter {
            text: "dscf".into(),
            ..Default::default()
        };
        assert!(f.matches(
            &entry("DSCF6223.RAF", 0, None),
            Marks::default(),
            &Capture::default()
        ));
        assert!(!f.matches(
            &entry("IMG_0001.JPG", 0, None),
            Marks::default(),
            &Capture::default()
        ));
        f.text = "6223".into();
        assert!(f.matches(
            &entry("DSCF6223.RAF", 0, None),
            Marks::default(),
            &Capture::default()
        ));
    }

    #[test]
    fn text_matches_a_copys_own_name_not_the_files() {
        // ⚠⚠ Caption **or** label. Before M6.0 a copy's own name *replaced* the caption, so the
        // file's name deliberately did not match it; now they are two strings and either one
        // finds the photo. That is the point — a copy is findable by what it is called *and* by
        // which photograph it came from.
        let warm = entry("DSCF6223.RAF", 2, Some("Warm"));
        let f = LibraryFilter {
            text: "warm".into(),
            ..Default::default()
        };
        assert!(
            f.matches(&warm, Marks::default(), &Capture::default()),
            "a copy's label must match"
        );

        let g = LibraryFilter {
            text: "dscf".into(),
            ..Default::default()
        };
        assert!(
            g.matches(&warm, Marks::default(), &Capture::default()),
            "and so must the file's name it is captioned with"
        );

        let h = LibraryFilter {
            text: "sepia".into(),
            ..Default::default()
        };
        assert!(
            !h.matches(&warm, Marks::default(), &Capture::default()),
            "an unrelated term must still not match"
        );
    }

    #[test]
    fn an_unlabelled_copy_matches_its_file_name_but_not_its_number() {
        // ⚠⚠ Replaces `an_unnamed_copys_derived_name_matches`. That test searched `_1`, which
        // worked because an unnamed copy was *shown* as `DSCF6223_1.RAF`. M6.0 removes the
        // derived filename, so `_1` matches nothing.
        //
        // ⚠ And the copy **number** is deliberately not searchable either: the filter matches the
        // stored label, not the rendered badge. The badge shows "1" so you can see which copy it
        // is; searching "1" would match every first copy in the catalogue, which is noise. Only a
        // label someone actually chose is a search term.
        let e = entry("DSCF6223.RAF", 1, None);
        let by_file = LibraryFilter {
            text: "dscf6223".into(),
            ..Default::default()
        };
        assert!(by_file.matches(&e, Marks::default(), &Capture::default()));
        let by_number = LibraryFilter {
            text: "1".into(),
            ..Default::default()
        };
        assert!(
            !by_number.matches(&e, Marks::default(), &Capture::default()),
            "the badge number is shown, not searched"
        );
    }

    fn stars(list: &[u8]) -> RatingSet {
        let mut s = RatingSet::default();
        for &r in list {
            s.toggle(r);
        }
        s
    }

    #[test]
    fn stars_match_exactly_rather_than_as_a_minimum() {
        // ⚠⚠ The thing a minimum could not express: *only* the one-star photos. Reported in use —
        // "what if I want to only see the photos with one star" — and it is also what makes stars
        // consistent with flag and label, which already match exactly.
        let f = LibraryFilter {
            stars: stars(&[1]),
            ..Default::default()
        };
        let e = entry("a.RAF", 0, None);
        assert!(f.matches(&e, marks(1, Flag::None, 0), &Capture::default()));
        assert!(
            !f.matches(&e, marks(2, Flag::None, 0), &Capture::default()),
            "2★ is not 1★"
        );
        assert!(!f.matches(&e, marks(0, Flag::None, 0), &Capture::default()));
    }

    #[test]
    fn a_set_of_stars_subsumes_a_minimum() {
        // "3 or more" is still expressible — it is 3, 4 and 5.
        let f = LibraryFilter {
            stars: stars(&[3, 4, 5]),
            ..Default::default()
        };
        let e = entry("a.RAF", 0, None);
        assert!(!f.matches(&e, marks(2, Flag::None, 0), &Capture::default()));
        for r in [3, 4, 5] {
            assert!(
                f.matches(&e, marks(r, Flag::None, 0), &Capture::default()),
                "{r}★ should match"
            );
        }
    }

    #[test]
    fn an_empty_star_set_matches_every_rating() {
        // Empty means "any", the convention `flag: None` and `label: None` already use.
        let f = LibraryFilter::default();
        let e = entry("a.RAF", 0, None);
        for r in 0..=5 {
            assert!(f.matches(&e, marks(r, Flag::None, 0), &Capture::default()));
        }
    }

    #[test]
    fn zero_stars_selects_the_unrated() {
        // ⚠ 0 is a real value — "show me what I have not rated yet" — exactly as `Some(Flag::None)`
        // selects the unflagged.
        let f = LibraryFilter {
            stars: stars(&[0]),
            ..Default::default()
        };
        let e = entry("a.RAF", 0, None);
        assert!(f.matches(&e, marks(0, Flag::None, 0), &Capture::default()));
        assert!(!f.matches(&e, marks(1, Flag::None, 0), &Capture::default()));
    }

    #[test]
    fn toggling_a_rating_twice_clears_it_and_out_of_range_is_ignored() {
        let mut s = RatingSet::default();
        s.toggle(3);
        assert!(s.contains(3));
        s.toggle(3);
        assert!(s.is_empty());
        // ⚠ Out of range is ignored rather than shifting into another rating's bit, which is what
        // `1 << 9` on a `u8` would do.
        s.toggle(9);
        assert!(s.is_empty(), "an out-of-range rating set a bit");
        assert!(!s.contains(9));
    }

    #[test]
    fn ratings_come_back_ascending() {
        assert_eq!(stars(&[5, 1, 3]).ratings(), vec![1, 3, 5]);
        assert!(RatingSet::default().ratings().is_empty());
    }

    fn flags(list: &[Flag]) -> FlagSet {
        let mut s = FlagSet::default();
        for &f in list {
            s.toggle(f);
        }
        s
    }

    fn labels(list: &[u8]) -> LabelSet {
        let mut s = LabelSet::default();
        for &l in list {
            s.toggle(l);
        }
        s
    }

    #[test]
    fn flag_none_means_unflagged_rather_than_any() {
        // ⚠ `Flag::None` is meaningful, not degenerate: "show me the ones I have not judged yet"
        // is a real culling question. An *empty set* is what means "any".
        let e = entry("a.RAF", 0, None);
        let unflagged = LibraryFilter {
            flags: flags(&[Flag::None]),
            ..Default::default()
        };
        assert!(unflagged.matches(&e, marks(0, Flag::None, 0), &Capture::default()));
        assert!(!unflagged.matches(&e, marks(0, Flag::Pick, 0), &Capture::default()));

        let any = LibraryFilter::default();
        assert!(any.matches(&e, marks(0, Flag::Pick, 0), &Capture::default()));
        assert!(any.matches(&e, marks(0, Flag::None, 0), &Capture::default()));
    }

    #[test]
    fn several_flags_can_be_selected_at_once() {
        // ⚠ The point of a set: "picked or rejected, but not the ones I have not looked at".
        let e = entry("a.RAF", 0, None);
        let f = LibraryFilter {
            flags: flags(&[Flag::Pick, Flag::Reject]),
            ..Default::default()
        };
        assert!(f.matches(&e, marks(0, Flag::Pick, 0), &Capture::default()));
        assert!(f.matches(&e, marks(0, Flag::Reject, 0), &Capture::default()));
        assert!(!f.matches(&e, marks(0, Flag::None, 0), &Capture::default()));
    }

    #[test]
    fn label_zero_means_unlabelled_rather_than_any() {
        let e = entry("a.RAF", 0, None);
        let unlabelled = LibraryFilter {
            labels: labels(&[0]),
            ..Default::default()
        };
        assert!(unlabelled.matches(&e, marks(0, Flag::None, 0), &Capture::default()));
        assert!(!unlabelled.matches(&e, marks(0, Flag::None, 2), &Capture::default()));

        let red = LibraryFilter {
            labels: labels(&[1]),
            ..Default::default()
        };
        assert!(red.matches(&e, marks(0, Flag::None, 1), &Capture::default()));
        assert!(!red.matches(&e, marks(0, Flag::None, 2), &Capture::default()));
    }

    #[test]
    fn several_labels_can_be_selected_at_once() {
        let e = entry("a.RAF", 0, None);
        let f = LibraryFilter {
            labels: labels(&[1, 3]),
            ..Default::default()
        };
        assert!(f.matches(&e, marks(0, Flag::None, 1), &Capture::default()));
        assert!(f.matches(&e, marks(0, Flag::None, 3), &Capture::default()));
        assert!(!f.matches(&e, marks(0, Flag::None, 2), &Capture::default()));
    }

    #[test]
    fn every_set_leads_with_its_unset_value() {
        // ⚠⚠ **One ordering across every section**, which is the whole reason `FlagSet::ALL`
        // exists rather than the UI listing the variants itself: unset first, then the values.
        // Flag listed "Any, picked, rejected, unflagged" while colour listed "Any, unlabelled,
        // colours" — two orderings for one idea, reported in use.
        assert_eq!(FlagSet::ALL[0], Flag::None);
        assert_eq!(RatingSet::default().ratings().first(), None);
        let all_stars = stars(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(all_stars.ratings()[0], 0, "unrated must lead");
        let all_labels = labels(&[0, 1, 2, 3, 4]);
        assert_eq!(all_labels.labels()[0], 0, "unlabelled must lead");
    }

    #[test]
    fn out_of_range_flags_and_labels_are_ignored() {
        // ⚠ `1 << 9` on a `u8` wraps into another value's bit; the guard is what stops a label of
        // 9 silently selecting label 1.
        let mut l = LabelSet::default();
        l.toggle(9);
        assert!(l.is_empty(), "an out-of-range label set a bit");
        assert!(!l.contains(1));
    }

    #[test]
    fn the_format_filter_still_applies() {
        // ⚠ Contained, not absorbed — its hidden-by-exception rule is unchanged.
        let mut f = LibraryFilter::default();
        f.format.toggle("jpg");
        assert!(f.matches(
            &entry("a.RAF", 0, None),
            Marks::default(),
            &Capture::default()
        ));
        assert!(!f.matches(
            &entry("a.jpg", 0, None),
            Marks::default(),
            &Capture::default()
        ));
        assert!(f.is_active());
    }

    #[test]
    fn every_field_narrows_and_all_of_them_compose() {
        // ⚠ The conjunction, tested as a conjunction: a photo passing three criteria and failing
        // the fourth must be hidden, whichever the fourth is. Testing each field alone cannot see
        // an `||` written where an `&&` belongs.
        let e = entry("DSCF6223.RAF", 0, None);
        let m = marks(4, Flag::Pick, 2);
        let pass = LibraryFilter {
            text: "dscf".into(),
            stars: stars(&[4]),
            flags: flags(&[Flag::Pick]),
            labels: labels(&[2]),
            ..Default::default()
        };
        assert!(
            pass.matches(&e, m, &Capture::default()),
            "the fully-matching case must pass"
        );

        for (label, f) in [
            (
                "text",
                LibraryFilter {
                    text: "nope".into(),
                    ..pass.clone()
                },
            ),
            (
                "stars",
                LibraryFilter {
                    stars: stars(&[5]),
                    ..pass.clone()
                },
            ),
            (
                "flag",
                LibraryFilter {
                    flags: flags(&[Flag::Reject]),
                    ..pass.clone()
                },
            ),
            (
                "label",
                LibraryFilter {
                    labels: labels(&[3]),
                    ..pass.clone()
                },
            ),
        ] {
            assert!(
                !f.matches(&e, m, &Capture::default()),
                "{label} did not narrow"
            );
        }
    }

    #[test]
    fn is_active_reports_every_field() {
        // A filter that hides something while reporting itself inactive would leave the UI with no
        // way to say so — the "invisible state" the crop aspect lock was fixed for.
        assert!(LibraryFilter {
            text: "a".into(),
            ..Default::default()
        }
        .is_active());
        assert!(LibraryFilter {
            stars: stars(&[1]),
            ..Default::default()
        }
        .is_active());
        assert!(LibraryFilter {
            flags: flags(&[Flag::None]),
            ..Default::default()
        }
        .is_active());
        assert!(LibraryFilter {
            labels: labels(&[0]),
            ..Default::default()
        }
        .is_active());
        assert!(!LibraryFilter::default().is_active());
    }

    #[test]
    fn clearing_leaves_the_format_filter_alone() {
        // ⚠ The format filter is reset by *loading a source*, not by clearing the search — they
        // answer different questions and a Clear button must not silently unhide a format.
        let mut f = LibraryFilter {
            text: "a".into(),
            stars: stars(&[3]),
            ..Default::default()
        };
        f.format.toggle("jpg");
        f.clear_marks_and_text();
        assert!(
            f.format.is_hidden("jpg"),
            "the format filter was cleared too"
        );
        assert!(f.text.is_empty() && f.stars.is_empty());
    }

    fn cap(camera: &str, lens: &str) -> Capture {
        Capture {
            camera: (!camera.is_empty()).then(|| camera.to_string()),
            lens: (!lens.is_empty()).then(|| lens.to_string()),
            ..Default::default()
        }
    }

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_camera_set_narrows_and_an_empty_one_does_not() {
        let e = entry("a.RAF", 0, None);
        let f = LibraryFilter {
            cameras: names(&["FUJIFILM X-T4"]),
            ..Default::default()
        };
        assert!(f.matches(&e, Marks::default(), &cap("FUJIFILM X-T4", "")));
        assert!(!f.matches(&e, Marks::default(), &cap("CANON EOS R5", "")));
        assert!(LibraryFilter::default().matches(&e, Marks::default(), &cap("CANON EOS R5", "")));
    }

    #[test]
    fn several_cameras_can_be_selected_at_once() {
        let e = entry("a.RAF", 0, None);
        let f = LibraryFilter {
            cameras: names(&["FUJIFILM X-T4", "CANON EOS R5"]),
            ..Default::default()
        };
        assert!(f.matches(&e, Marks::default(), &cap("FUJIFILM X-T4", "")));
        assert!(f.matches(&e, Marks::default(), &cap("CANON EOS R5", "")));
        assert!(!f.matches(&e, Marks::default(), &cap("NIKON Z8", "")));
    }

    #[test]
    fn a_photo_with_no_camera_fails_a_camera_filter() {
        // ⚠ "Show me the X-T4 shots" must not include a file whose camera is unknown.
        let e = entry("a.RAF", 0, None);
        let f = LibraryFilter {
            cameras: names(&["FUJIFILM X-T4"]),
            ..Default::default()
        };
        assert!(!f.matches(&e, Marks::default(), &Capture::default()));
        // …but with no camera filter it is shown like anything else.
        assert!(LibraryFilter::default().matches(&e, Marks::default(), &Capture::default()));
    }

    #[test]
    fn the_lens_set_narrows_independently_of_the_camera_set() {
        let e = entry("a.RAF", 0, None);
        let f = LibraryFilter {
            lenses: names(&["XF23mmF2 R WR"]),
            ..Default::default()
        };
        assert!(f.matches(&e, Marks::default(), &cap("FUJIFILM X-T4", "XF23mmF2 R WR")));
        assert!(!f.matches(
            &e,
            Marks::default(),
            &cap("FUJIFILM X-T4", "XF16-80mmF4 R OIS WR")
        ));
    }

    #[test]
    fn is_active_reports_the_capture_sets_too() {
        assert!(LibraryFilter {
            cameras: names(&["X"]),
            ..Default::default()
        }
        .is_active());
        assert!(LibraryFilter {
            lenses: names(&["Y"]),
            ..Default::default()
        }
        .is_active());
    }

    #[test]
    fn clearing_empties_the_capture_sets() {
        let mut f = LibraryFilter {
            cameras: names(&["X"]),
            lenses: names(&["Y"]),
            ..Default::default()
        };
        f.clear_marks_and_text();
        assert!(f.cameras.is_empty() && f.lenses.is_empty());
    }
}
