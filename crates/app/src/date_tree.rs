//! Grouping the catalog's flat day rows into a year › month › day tree.
//!
//! ⚠ Pure and egui-free, so the aggregation is testable without a window — the `view_region` and
//! `thumb_window` precedent.
//!
//! ⚠ The catalog returns **days** rather than a pre-built tree because the useful granularity
//! depends on the library: measured, 738 of 830 photos in the author's fall in one month while six
//! distinct days discriminate, and a ten-year archive is the other way round.

use catalog::db::DayRow;

pub struct Year {
    pub year: i32,
    pub count: i64,
    pub months: Vec<Month>,
}

pub struct Month {
    pub month: u32,
    pub count: i64,
    pub days: Vec<DayRow>,
}

/// The month's name.
///
/// ⚠ Out of range degrades rather than panicking. The catalog only ever yields 1..=12, but this is
/// called from a panel body, where a panic is a crash.
pub fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES
        .get(month.wrapping_sub(1) as usize)
        .copied()
        .unwrap_or("—")
}

/// Group day rows into years and months, preserving order and summing counts.
///
/// ⚠⚠ Relies on `days` being **grouped**, not on a direction: a new year starts whenever the year
/// differs from the last, so any ordering that keeps a year's rows together works. This said "relies
/// on ascending" until 2026-09-23, when `Catalog::capture_days` was flipped to newest-first and
/// nothing here had to change — the contract was narrower than the comment claimed. The alternative
/// is a `HashMap` plus a re-sort, for no gain.
pub fn group(days: &[DayRow]) -> Vec<Year> {
    let mut years: Vec<Year> = Vec::new();
    for &row in days {
        // ⚠ Matches on the **year**, not just the month — grouping on the month alone folds July
        // 2025 into July 2026.
        if years.last().map(|y| y.year) != Some(row.year) {
            years.push(Year {
                year: row.year,
                count: 0,
                months: Vec::new(),
            });
        }
        let y = years.last_mut().expect("just pushed");
        y.count += row.count;
        if y.months.last().map(|m| m.month) != Some(row.month) {
            y.months.push(Month {
                month: row.month,
                count: 0,
                days: Vec::new(),
            });
        }
        let m = y.months.last_mut().expect("just pushed");
        m.count += row.count;
        m.days.push(row);
    }
    years
}

/// One clickable node of the tree: a year, a month, or a day.
pub type Period = (i32, Option<u32>, Option<u32>);

/// Every clickable node, in **display order** — the sequence a Shift-range walks.
///
/// ⚠ Built here rather than in the panel so it is testable, and so "everything between" means what
/// the eye sees: a year, then each of its months, then that month's days.
pub fn flatten(years: &[Year]) -> Vec<Period> {
    let mut out = Vec::new();
    for y in years {
        out.push((y.year, None, None));
        for m in &y.months {
            out.push((y.year, Some(m.month), None));
            for d in &m.days {
                out.push((y.year, Some(m.month), Some(d.day)));
            }
        }
    }
    out
}

/// The inclusive range between two nodes, in display order.
///
/// ⚠ Order-independent: Shift-clicking upwards selects the same set as downwards, which is what
/// every list in every app does. Either end being absent yields just the other.
pub fn range(order: &[Period], a: Period, b: Period) -> Vec<Period> {
    let (Some(i), Some(j)) = (
        order.iter().position(|p| *p == a),
        order.iter().position(|p| *p == b),
    ) else {
        return vec![b];
    };
    let (lo, hi) = if i <= j { (i, j) } else { (j, i) };
    order[lo..=hi].to_vec()
}

/// What the status line calls a selection of periods.
///
/// ⚠ Names one period in full and counts the rest, rather than listing them: a weekend's worth of
/// days would otherwise run off the end of the status line.
pub fn label_for(periods: &[Period]) -> String {
    let Some(&(y, m, d)) = periods.first() else {
        return "No dates".to_string();
    };
    let one = match (m, d) {
        (Some(m), Some(d)) => format!("{d} {} {y}", month_name(m)),
        (Some(m), None) => format!("{} {y}", month_name(m)),
        _ => y.to_string(),
    };
    match periods.len() {
        1 => one,
        n => format!("{one} +{}", n - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(year: i32, month: u32, day: u32, count: i64) -> DayRow {
        DayRow {
            year,
            month,
            day,
            count,
        }
    }

    /// ⚠⚠ `capture_days` returns **newest first** since 2026-09-23. Grouping must hold either way,
    /// because it groups on change rather than on direction.
    #[test]
    fn grouping_holds_for_descending_rows() {
        let years = group(&[d(2026, 7, 9, 1), d(2026, 5, 17, 2), d(2025, 12, 1, 3)]);
        assert_eq!(years.len(), 2);
        assert_eq!(years[0].year, 2026, "newest year first");
        assert_eq!(years[0].count, 3);
        assert_eq!(
            years[0].months.iter().map(|m| m.month).collect::<Vec<_>>(),
            vec![7, 5]
        );
        assert_eq!(years[1].year, 2025);
    }

    /// ⚠⚠ **Shift-click range selection reads the same rows through `flatten` and `range`**, so the
    /// reversal could have changed it silently. The **days** it spans are the same set either way.
    ///
    /// ⚠⚠ What is *not* the same is the whole-month node the span picks up at its boundary, and that
    /// is a **pre-existing quirk made visible, not new fallout**: `flatten` interleaves year, month
    /// and day nodes, so a span between two days also contains whichever month headers sit between
    /// them — ascending picks up the later endpoint's month, descending the earlier one's. Either way
    /// shift-clicking two days already selects one entire intervening month. Narrowing `range` to
    /// comparable granularity would change what shift-click *means*, which is its own decision.
    #[test]
    fn a_shift_click_spans_the_same_days_whichever_way_the_tree_is_ordered() {
        let asc = [d(2026, 5, 17, 1), d(2026, 6, 1, 1), d(2026, 7, 9, 1)];
        let desc: Vec<DayRow> = asc.iter().rev().copied().collect();
        let a = (2026, Some(5), Some(17));
        let b = (2026, Some(7), Some(9));

        // Only the day-granularity periods, which is what "select these days" means.
        let days_of = |rows: &[DayRow]| {
            let mut v: Vec<Period> = range(&flatten(&group(rows)), a, b)
                .into_iter()
                .filter(|(_, m, dd)| m.is_some() && dd.is_some())
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            days_of(&asc),
            days_of(&desc),
            "the days between two clicks must not depend on the tree's direction"
        );
        assert_eq!(
            days_of(&asc).len(),
            3,
            "all three days, not a truncated span"
        );
    }

    #[test]
    fn days_group_into_years_and_months_with_summed_counts() {
        let tree = group(&[
            d(2025, 12, 31, 3),
            d(2026, 5, 17, 6),
            d(2026, 5, 19, 86),
            d(2026, 7, 9, 544),
        ]);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].year, 2025);
        assert_eq!(tree[0].count, 3);
        assert_eq!(tree[1].year, 2026);
        assert_eq!(tree[1].count, 636, "the year must sum its months");
        assert_eq!(tree[1].months.len(), 2);
        assert_eq!(tree[1].months[0].month, 5);
        assert_eq!(tree[1].months[0].count, 92, "the month must sum its days");
        assert_eq!(tree[1].months[0].days.len(), 2);
        assert_eq!(tree[1].months[1].days[0].day, 9);
    }

    #[test]
    fn an_empty_catalog_yields_an_empty_tree() {
        assert!(group(&[]).is_empty());
    }

    #[test]
    fn a_single_day_still_makes_a_year_and_a_month() {
        // ⚠ The degenerate case a new library is in, and the one most likely to be mishandled.
        let tree = group(&[d(2026, 7, 9, 1)]);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].months.len(), 1);
        assert_eq!(tree[0].months[0].days.len(), 1);
        assert_eq!(tree[0].count, 1);
    }

    #[test]
    fn the_same_month_number_in_two_years_does_not_merge() {
        // ⚠ Grouping on the month alone would fold July 2025 into July 2026 — the bug this
        // function exists to not have.
        let tree = group(&[d(2025, 7, 1, 1), d(2026, 7, 1, 2)]);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].count, 1);
        assert_eq!(tree[1].count, 2);
    }

    #[test]
    fn every_month_has_a_name_and_nothing_panics_out_of_range() {
        for m in 1..=12 {
            assert!(!month_name(m).is_empty(), "month {m} has no name");
        }
        assert_eq!(month_name(1), "January");
        assert_eq!(month_name(12), "December");
        // ⚠ Unreachable from the catalog, which only ever yields 1..=12 — but a panic in a panel
        // body is a crash, so it must degrade instead.
        assert!(!month_name(0).is_empty());
        assert!(!month_name(13).is_empty());
    }

    #[test]
    fn flatten_walks_a_year_then_its_months_then_their_days() {
        // ⚠ Display order, so a Shift-range means what the eye sees.
        let tree = group(&[d(2026, 5, 17, 1), d(2026, 5, 19, 1), d(2026, 7, 9, 1)]);
        assert_eq!(
            flatten(&tree),
            vec![
                (2026, None, None),
                (2026, Some(5), None),
                (2026, Some(5), Some(17)),
                (2026, Some(5), Some(19)),
                (2026, Some(7), None),
                (2026, Some(7), Some(9)),
            ]
        );
    }

    #[test]
    fn two_years_do_not_interleave() {
        let tree = group(&[d(2025, 7, 1, 1), d(2026, 7, 1, 1)]);
        let flat = flatten(&tree);
        let first_2026 = flat.iter().position(|p| p.0 == 2026).unwrap();
        assert!(
            flat[..first_2026].iter().all(|p| p.0 == 2025),
            "a 2026 node appeared among 2025's: {flat:?}"
        );
    }

    #[test]
    fn a_range_is_the_same_in_both_directions() {
        // ⚠ Shift-clicking upwards must select what downwards does — what every list does.
        let tree = group(&[d(2026, 5, 17, 1), d(2026, 5, 19, 1), d(2026, 7, 9, 1)]);
        let order = flatten(&tree);
        let down = range(&order, (2026, Some(5), None), (2026, Some(7), None));
        let up = range(&order, (2026, Some(7), None), (2026, Some(5), None));
        assert_eq!(down, up);
        assert_eq!(down.len(), 4, "month, two days, month: {down:?}");
    }

    #[test]
    fn a_range_from_an_unknown_anchor_selects_only_the_clicked_node() {
        // ⚠ The state before anything has been clicked, and after the tree has changed under a
        // stale anchor — it must not select nothing, and must not panic.
        let tree = group(&[d(2026, 5, 17, 1)]);
        let order = flatten(&tree);
        let target = (2026, Some(5), Some(17));
        assert_eq!(range(&order, (1999, None, None), target), vec![target]);
    }

    #[test]
    fn a_label_names_one_period_and_counts_the_rest() {
        // ⚠ Listing them all would run off the end of the status line for a weekend's worth.
        assert_eq!(label_for(&[(2026, Some(5), Some(17))]), "17 May 2026");
        assert_eq!(label_for(&[(2026, Some(5), None)]), "May 2026");
        assert_eq!(label_for(&[(2026, None, None)]), "2026");
        assert_eq!(
            label_for(&[(2026, Some(5), Some(17)), (2026, Some(7), Some(9))]),
            "17 May 2026 +1"
        );
        assert_eq!(label_for(&[]), "No dates");
    }
}
