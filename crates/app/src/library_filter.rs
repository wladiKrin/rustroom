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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LibraryFilter {
    pub format: FormatFilter,
    /// Case-insensitive substring of the row's shown name. Empty matches everything.
    pub text: String,
    /// Minimum star rating. 0 matches everything.
    pub min_stars: u8,
    /// `None` matches any flag; `Some(Flag::None)` matches only *unflagged* photos.
    pub flag: Option<Flag>,
    /// `None` matches any label; `Some(0)` matches only *unlabelled* photos.
    pub label: Option<u8>,
}

impl LibraryFilter {
    /// Does this row survive the filter?
    ///
    /// ⚠ A conjunction: every criterion must pass. `marks` is the row's marks, or the default
    /// when it has none — "unrated, unflagged, unlabelled" is a real value, not a missing one,
    /// which is why `Marks` defaults to zero and no special case is needed here.
    pub fn matches(&self, entry: &PhotoEntry, marks: Marks) -> bool {
        if !self.format.is_visible(&entry.id.path) {
            return false;
        }
        if marks.rating < self.min_stars {
            return false;
        }
        if let Some(want) = self.flag {
            if marks.flag != want {
                return false;
            }
        }
        if let Some(want) = self.label {
            if marks.label != want {
                return false;
            }
        }
        if !self.text.is_empty() {
            // ⚠⚠ `shown_name`, not `file_name`. Every copy of a file shares one `file_name`, so
            // matching that would mean searching for what a copy is *called* and not finding it —
            // precisely the bug that made copy names invisible in the grid, where the Library was
            // the last consumer bypassing this shared rule.
            let shown = entries::shown_name(&entry.file_name, entry.id.copy, entry.name.as_deref());
            if !shown.to_lowercase().contains(&self.text.to_lowercase()) {
                return false;
            }
        }
        true
    }

    /// True when this filter hides anything at all.
    pub fn is_active(&self) -> bool {
        self.format.is_active()
            || !self.text.is_empty()
            || self.min_stars > 0
            || self.flag.is_some()
            || self.label.is_some()
    }

    /// Clear the marks and the text, leaving the format filter alone.
    ///
    /// ⚠ The format filter answers a different question and is reset by *loading a source*. A
    /// Clear button that silently unhid a format would be doing something nobody asked for.
    pub fn clear_marks_and_text(&mut self) {
        self.text.clear();
        self.min_stars = 0;
        self.flag = None;
        self.label = None;
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
            name: copy_name.map(str::to_string),
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
        assert!(f.matches(&entry("a.RAF", 0, None), Marks::default()));
        assert!(!f.is_active());
    }

    #[test]
    fn text_matches_case_insensitively_on_a_substring() {
        let mut f = LibraryFilter {
            text: "dscf".into(),
            ..Default::default()
        };
        assert!(f.matches(&entry("DSCF6223.RAF", 0, None), Marks::default()));
        assert!(!f.matches(&entry("IMG_0001.JPG", 0, None), Marks::default()));
        f.text = "6223".into();
        assert!(f.matches(&entry("DSCF6223.RAF", 0, None), Marks::default()));
    }

    #[test]
    fn text_matches_a_copys_own_name_not_the_files() {
        // ⚠⚠ Matches `entries::shown_name`, the one rule the Library caption, the rename field and
        // the default export name already share. Matching `file_name` instead would mean
        // searching for what a copy is *called* and not finding it.
        let warm = entry("DSCF6223.RAF", 2, Some("Warm.RAF"));
        let f = LibraryFilter {
            text: "warm".into(),
            ..Default::default()
        };
        assert!(
            f.matches(&warm, Marks::default()),
            "a copy's own name must match"
        );

        // And the file's name must NOT match a named copy, since that is not what it is called.
        let g = LibraryFilter {
            text: "dscf".into(),
            ..Default::default()
        };
        assert!(!g.matches(&warm, Marks::default()));
    }

    #[test]
    fn an_unnamed_copys_derived_name_matches() {
        // An unnamed copy is shown as `{stem}_{copy}.{ext}` — derived, never stored.
        let f = LibraryFilter {
            text: "_1".into(),
            ..Default::default()
        };
        assert!(f.matches(&entry("DSCF6223.RAF", 1, None), Marks::default()));
    }

    #[test]
    fn stars_filter_on_a_minimum() {
        let f = LibraryFilter {
            min_stars: 3,
            ..Default::default()
        };
        let e = entry("a.RAF", 0, None);
        assert!(!f.matches(&e, marks(2, Flag::None, 0)));
        assert!(f.matches(&e, marks(3, Flag::None, 0)));
        assert!(f.matches(&e, marks(5, Flag::None, 0)));
    }

    #[test]
    fn some_flag_none_means_unflagged_rather_than_any() {
        // ⚠ `Some(Flag::None)` is meaningful, not degenerate: "show me the ones I have not judged
        // yet" is a real culling question. `None` is what means "any".
        let e = entry("a.RAF", 0, None);
        let unflagged = LibraryFilter {
            flag: Some(Flag::None),
            ..Default::default()
        };
        assert!(unflagged.matches(&e, marks(0, Flag::None, 0)));
        assert!(!unflagged.matches(&e, marks(0, Flag::Pick, 0)));

        let any = LibraryFilter::default();
        assert!(any.matches(&e, marks(0, Flag::Pick, 0)));
        assert!(any.matches(&e, marks(0, Flag::None, 0)));
    }

    #[test]
    fn some_label_zero_means_unlabelled_rather_than_any() {
        let e = entry("a.RAF", 0, None);
        let unlabelled = LibraryFilter {
            label: Some(0),
            ..Default::default()
        };
        assert!(unlabelled.matches(&e, marks(0, Flag::None, 0)));
        assert!(!unlabelled.matches(&e, marks(0, Flag::None, 2)));

        let red = LibraryFilter {
            label: Some(1),
            ..Default::default()
        };
        assert!(red.matches(&e, marks(0, Flag::None, 1)));
        assert!(!red.matches(&e, marks(0, Flag::None, 2)));
    }

    #[test]
    fn the_format_filter_still_applies() {
        // ⚠ Contained, not absorbed — its hidden-by-exception rule is unchanged.
        let mut f = LibraryFilter::default();
        f.format.toggle("jpg");
        assert!(f.matches(&entry("a.RAF", 0, None), Marks::default()));
        assert!(!f.matches(&entry("a.jpg", 0, None), Marks::default()));
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
            min_stars: 3,
            flag: Some(Flag::Pick),
            label: Some(2),
            ..Default::default()
        };
        assert!(pass.matches(&e, m), "the fully-matching case must pass");

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
                    min_stars: 5,
                    ..pass.clone()
                },
            ),
            (
                "flag",
                LibraryFilter {
                    flag: Some(Flag::Reject),
                    ..pass.clone()
                },
            ),
            (
                "label",
                LibraryFilter {
                    label: Some(3),
                    ..pass.clone()
                },
            ),
        ] {
            assert!(!f.matches(&e, m), "{label} did not narrow");
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
            min_stars: 1,
            ..Default::default()
        }
        .is_active());
        assert!(LibraryFilter {
            flag: Some(Flag::None),
            ..Default::default()
        }
        .is_active());
        assert!(LibraryFilter {
            label: Some(0),
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
            min_stars: 3,
            ..Default::default()
        };
        f.format.toggle("jpg");
        f.clear_marks_and_text();
        assert!(
            f.format.is_hidden("jpg"),
            "the format filter was cleared too"
        );
        assert!(f.text.is_empty() && f.min_stars == 0);
    }
}
