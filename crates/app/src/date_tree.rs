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

/// Group ascending day rows into years and months, preserving order and summing counts.
///
/// ⚠ Relies on `days` being **ascending**, which `Catalog::capture_days` guarantees with its
/// `ORDER BY`. The alternative is a `HashMap` plus a re-sort, for no gain.
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
}
