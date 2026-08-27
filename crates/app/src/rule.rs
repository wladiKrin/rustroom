//! A saved rule: a boolean expression over everything Rustroom knows about a photo.
//!
//! ⚠⚠ **Evaluated in memory, not in SQL, and that is forced rather than chosen.** The `Text`
//! criterion matches `entries::shown_name`, which reads a copy's own name from the **sidecar** —
//! so a pure-SQL engine would need copy names duplicated into the catalog, which is two facts that
//! can disagree. Measured: reading every sidecar is 4.6 ms for 830 files, so ~60 ms at 10,000.
//!
//! ⚠ The payoff is that the evaluator is **one pure function**, testable exhaustively with no
//! database and no GUI — which is what a boolean engine needs most.
//!
//! ⚠ **A tree in storage, one level in the editor.** The stored shape never has to change when
//! nesting arrives; only the editor grows. A hand-written nested rule in the JSON already works.
//!
//! ⚠⚠ It exists because **`LibraryFilter` is not sufficient as a rule**, contrary to what was
//! claimed twice while M4.3a and M4.3b were built: `Collection`, `Keyword` and `Period` are
//! `LibrarySource` variants, not filter fields, so a saved filter cannot express *"in my Portfolio
//! collection, show me July"* — the very query this milestone exists for.

use crate::entries::{self, PhotoEntry};
use catalog::db::{Flag, Marks};
use core_image::Capture;
use serde::{Deserialize, Serialize};

/// How a numeric criterion compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cmp {
    AtLeast,
    Exactly,
    AtMost,
}

impl Cmp {
    pub fn holds(self, actual: f64, want: f64) -> bool {
        match self {
            Cmp::AtLeast => actual >= want,
            Cmp::Exactly => (actual - want).abs() < 1e-6,
            Cmp::AtMost => actual <= want,
        }
    }
}

/// One thing that can be true of a photo.
///
/// ⚠ The union of today's *sources* and today's *filters*.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Criterion {
    Folder(std::path::PathBuf),
    Collection(i64),
    /// ⚠ Matches the keyword **or any of its descendants** — searching `Europe` finds photos
    /// tagged `Paris`, the rule M4.2b established. The expansion happens where the map is built.
    Keyword(i64),
    Period {
        year: i32,
        month: Option<u32>,
        day: Option<u32>,
    },
    /// Case-insensitive substring of the row's **shown name**.
    Text(String),
    /// ⚠⚠ An operator, not a set, because that is what composes under `Any`. The filter bar keeps
    /// its checkboxes; this is a deliberate divergence, and the same one Lightroom has between its
    /// filter bar and its smart-collection editor.
    Rating {
        op: Cmp,
        value: u8,
    },
    Flag(Flag),
    Label(u8),
    Camera(String),
    Lens(String),
    /// Lower-case extension.
    Format(String),
    Iso {
        op: Cmp,
        value: u32,
    },
    Aperture {
        op: Cmp,
        value: f32,
    },
    Shutter {
        op: Cmp,
        value: f32,
    },
    FocalLength {
        op: Cmp,
        value: f32,
    },
    FileSize {
        op: Cmp,
        value: i64,
    },
}

/// A boolean expression over [`Criterion`]s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    All(Vec<Expr>),
    Any(Vec<Expr>),
    Not(Box<Expr>),
    Is(Criterion),
}

/// Everything the evaluator needs about one photo.
///
/// ⚠ Borrowed rather than owned: it is built once per entry inside a loop over ten thousand.
pub struct Facts<'a> {
    pub entry: &'a PhotoEntry,
    pub marks: Marks,
    pub capture: &'a Capture,
    /// Collection ids this photo belongs to.
    pub collections: &'a [i64],
    /// Keyword ids, **already expanded to include ancestors** — see [`Criterion::Keyword`].
    pub keywords: &'a [i64],
    /// File size in bytes, when the catalog recorded one.
    pub size: Option<i64>,
}

impl Expr {
    pub fn matches(&self, f: &Facts) -> bool {
        match self {
            // ⚠⚠ The identity elements, and the state a **half-built rule** is in: `All([])` is
            // true and `Any([])` is false. Getting these backwards makes an empty rule hide or
            // show everything, which reads as the feature being broken rather than empty.
            Expr::All(v) => v.iter().all(|e| e.matches(f)),
            Expr::Any(v) => v.iter().any(|e| e.matches(f)),
            Expr::Not(e) => !e.matches(f),
            Expr::Is(c) => c.matches(f),
        }
    }
}

impl Criterion {
    pub fn matches(&self, f: &Facts) -> bool {
        // ⚠⚠ Every optional fact **fails when absent** rather than passing — the `is_some_and`
        // rule the camera filter already follows. "Show me the X-T4 shots" must not include a
        // photo whose camera is unknown.
        match self {
            // ⚠ The **parent**, not a string prefix: `/x` must not match `/xy/a.RAF`.
            Criterion::Folder(p) => f.entry.id.path.parent() == Some(p.as_path()),
            Criterion::Collection(id) => f.collections.contains(id),
            Criterion::Keyword(id) => f.keywords.contains(id),
            Criterion::Period { year, month, day } => f
                .capture
                .taken
                .is_some_and(|t| in_period(t, *year, *month, *day)),
            Criterion::Text(q) => {
                let shown = entries::shown_name(
                    &f.entry.file_name,
                    f.entry.id.copy,
                    f.entry.name.as_deref(),
                );
                shown.to_lowercase().contains(&q.to_lowercase())
            }
            Criterion::Rating { op, value } => op.holds(f.marks.rating as f64, *value as f64),
            Criterion::Flag(want) => f.marks.flag == *want,
            Criterion::Label(want) => f.marks.label == *want,
            Criterion::Camera(want) => f.capture.camera.as_deref() == Some(want.as_str()),
            Criterion::Lens(want) => f.capture.lens.as_deref() == Some(want.as_str()),
            Criterion::Format(want) => f
                .entry
                .id
                .path
                .extension()
                .is_some_and(|e| e.to_string_lossy().to_lowercase() == want.to_lowercase()),
            Criterion::Iso { op, value } => f
                .capture
                .iso
                .is_some_and(|v| op.holds(v as f64, *value as f64)),
            Criterion::Aperture { op, value } => f
                .capture
                .aperture
                .is_some_and(|v| op.holds(v as f64, *value as f64)),
            Criterion::Shutter { op, value } => f
                .capture
                .shutter
                .is_some_and(|v| op.holds(v as f64, *value as f64)),
            Criterion::FocalLength { op, value } => f
                .capture
                .focal_len
                .is_some_and(|v| op.holds(v as f64, *value as f64)),
            Criterion::FileSize { op, value } => {
                f.size.is_some_and(|v| op.holds(v as f64, *value as f64))
            }
        }
    }
}

/// Is a wall-clock timestamp inside a calendar period?
///
/// ⚠ Reads the value back under the convention it was stored with — see `Capture::taken`. No
/// timezone step in either direction.
fn in_period(secs: i64, year: i32, month: Option<u32>, day: Option<u32>) -> bool {
    use chrono::Datelike;
    let Some(d) = chrono::DateTime::from_timestamp(secs, 0) else {
        return false;
    };
    let d = d.naive_utc().date();
    d.year() == year && month.is_none_or(|m| d.month() == m) && day.is_none_or(|dd| d.day() == dd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo_id::PhotoId;
    use std::path::PathBuf;

    fn entry(name: &str) -> PhotoEntry {
        PhotoEntry {
            id: PhotoId::new(PathBuf::from(format!("/x/{name}")), 0),
            file_name: name.to_string(),
            name: None,
        }
    }

    fn facts<'a>(e: &'a PhotoEntry, c: &'a Capture) -> Facts<'a> {
        Facts {
            entry: e,
            marks: Marks::default(),
            capture: c,
            collections: &[],
            keywords: &[],
            size: None,
        }
    }

    #[test]
    fn all_of_nothing_is_true_and_any_of_nothing_is_false() {
        // ⚠⚠ The identity elements — and the state a **half-built rule** is in, which is why this
        // is the first test rather than a curiosity. `All([])` must not hide every photo, and
        // `Any([])` must not show every photo.
        let e = entry("a.RAF");
        let c = Capture::default();
        let f = facts(&e, &c);
        assert!(Expr::All(vec![]).matches(&f));
        assert!(!Expr::Any(vec![]).matches(&f));
    }

    #[test]
    fn not_inverts() {
        let e = entry("a.RAF");
        let c = Capture::default();
        let f = facts(&e, &c);
        let yes = Expr::Is(Criterion::Text("a".into()));
        assert!(yes.matches(&f));
        assert!(!Expr::Not(Box::new(yes)).matches(&f));
    }

    #[test]
    fn nesting_composes() {
        // (text AND rating>=0) OR format=png — the second arm false, the first true.
        let e = entry("a.RAF");
        let c = Capture::default();
        let f = facts(&e, &c);
        let rule = Expr::Any(vec![
            Expr::All(vec![
                Expr::Is(Criterion::Text("a".into())),
                Expr::Is(Criterion::Rating {
                    op: Cmp::AtLeast,
                    value: 0,
                }),
            ]),
            Expr::Is(Criterion::Format("png".into())),
        ]);
        assert!(rule.matches(&f));
    }

    #[test]
    fn a_missing_fact_fails_rather_than_passes() {
        // ⚠⚠ "Show me the X-T4 shots" must not include a photo whose camera is unknown — the
        // `is_some_and` rule the camera filter already follows, applied to every optional fact.
        let e = entry("a.RAF");
        let c = Capture::default();
        let f = facts(&e, &c);
        for crit in [
            Criterion::Camera("FUJIFILM X-T4".into()),
            Criterion::Lens("XF23mmF2 R WR".into()),
            Criterion::Period {
                year: 2026,
                month: None,
                day: None,
            },
            Criterion::Iso {
                op: Cmp::AtLeast,
                value: 100,
            },
            Criterion::FileSize {
                op: Cmp::AtLeast,
                value: 1,
            },
        ] {
            assert!(!crit.matches(&f), "{crit:?} passed with no fact behind it");
        }
    }

    #[test]
    fn a_period_narrows_from_a_year_to_a_day() {
        let e = entry("a.RAF");
        // 2026-05-17, wall-clock seconds — the convention `Capture::taken` documents.
        let c = Capture {
            taken: Some(1_779_046_001),
            ..Default::default()
        };
        let f = facts(&e, &c);
        let p = |y, m, d| Criterion::Period {
            year: y,
            month: m,
            day: d,
        };
        assert!(p(2026, None, None).matches(&f));
        assert!(p(2026, Some(5), None).matches(&f));
        assert!(p(2026, Some(5), Some(17)).matches(&f));
        assert!(!p(2026, Some(5), Some(18)).matches(&f));
        assert!(!p(2026, Some(6), None).matches(&f));
        assert!(!p(2025, None, None).matches(&f));
    }

    #[test]
    fn a_rating_operator_compares_rather_than_matching_a_set() {
        let e = entry("a.RAF");
        let c = Capture::default();
        let mut f = facts(&e, &c);
        f.marks = Marks {
            rating: 3,
            ..Default::default()
        };
        let r = |op, value| Criterion::Rating { op, value };
        assert!(r(Cmp::AtLeast, 3).matches(&f));
        assert!(r(Cmp::AtLeast, 2).matches(&f));
        assert!(!r(Cmp::AtLeast, 4).matches(&f));
        assert!(r(Cmp::Exactly, 3).matches(&f));
        assert!(!r(Cmp::Exactly, 2).matches(&f));
        assert!(r(Cmp::AtMost, 3).matches(&f));
        assert!(!r(Cmp::AtMost, 2).matches(&f));
    }

    #[test]
    fn membership_criteria_read_their_lists() {
        let e = entry("a.RAF");
        let c = Capture::default();
        let mut f = facts(&e, &c);
        f.collections = &[7, 9];
        f.keywords = &[3];
        assert!(Criterion::Collection(7).matches(&f));
        assert!(!Criterion::Collection(8).matches(&f));
        assert!(Criterion::Keyword(3).matches(&f));
        assert!(!Criterion::Keyword(4).matches(&f));
    }

    #[test]
    fn text_matches_the_shown_name_and_format_the_extension() {
        // ⚠ `shown_name`, not `file_name` — a named copy must be findable by what it is called.
        let mut e = entry("DSCF6223.RAF");
        e.id = PhotoId::new(PathBuf::from("/x/DSCF6223.RAF"), 2);
        e.name = Some("Warm.RAF".into());
        let c = Capture::default();
        let f = facts(&e, &c);
        assert!(Criterion::Text("warm".into()).matches(&f));
        assert!(!Criterion::Text("dscf".into()).matches(&f));
        assert!(Criterion::Format("raf".into()).matches(&f));
        assert!(!Criterion::Format("jpg".into()).matches(&f));
    }

    #[test]
    fn a_folder_criterion_matches_the_parent_not_a_prefix() {
        // ⚠ `/x` must not match `/xy/a.RAF`. A `starts_with` on the string would.
        let e = entry("a.RAF");
        let c = Capture::default();
        let f = facts(&e, &c);
        assert!(Criterion::Folder(PathBuf::from("/x")).matches(&f));
        assert!(!Criterion::Folder(PathBuf::from("/xy")).matches(&f));
        assert!(!Criterion::Folder(PathBuf::from("/")).matches(&f));
    }

    #[test]
    fn a_rule_round_trips_through_json() {
        let rule = Expr::All(vec![
            Expr::Is(Criterion::Rating {
                op: Cmp::AtLeast,
                value: 4,
            }),
            Expr::Not(Box::new(Expr::Is(Criterion::Flag(Flag::Reject)))),
        ]);
        let json = serde_json::to_string(&rule).unwrap();
        assert_eq!(serde_json::from_str::<Expr>(&json).unwrap(), rule);
    }
}
