//! The capture-date tree and its periods, and the distinct cameras and lenses the filters offer.

use super::{Catalog, CatalogError};
use std::path::PathBuf;

/// One calendar day that has photos on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayRow {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub count: i64,
}

impl Catalog {
    /// Every calendar day with photos on it, ascending.
    ///
    /// ⚠⚠ Grouped by SQL's `strftime`, which works **because `taken` is naive wall-clock
    /// seconds**: the date that comes back is exactly the one the camera wrote, with no timezone
    /// arithmetic anywhere. That is the payoff of the storage decision in plan 1.
    ///
    /// ⚠ Day granularity, not month, because the useful granularity depends on the library —
    /// measured, 738 of 830 photos here fall in one month while six distinct days discriminate,
    /// and a ten-year archive is the other way round. The caller aggregates upward.
    ///
    /// ⚠ `WHERE taken IS NOT NULL` is **defence in depth, not the mechanism** — verified by
    /// removing it, which changed nothing: a NULL date makes `strftime` return NULL, the `i32`
    /// deserialization then fails, and `filter_map(Result::ok)` drops the row. It is kept because
    /// relying on an error being swallowed is not a thing to build on, and because forming the
    /// group at all is wasted work. **No test can distinguish the two**, and inventing one that
    /// appeared to would be worse than saying so.
    pub fn capture_days(&self) -> Result<Vec<DayRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(strftime('%Y', taken, 'unixepoch') AS INTEGER),
                    CAST(strftime('%m', taken, 'unixepoch') AS INTEGER),
                    CAST(strftime('%d', taken, 'unixepoch') AS INTEGER),
                    count(*)
             FROM photo_capture WHERE taken IS NOT NULL
             GROUP BY 1, 2, 3 ORDER BY 1 DESC, 2, 3",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(DayRow {
                year: r.get(0)?,
                month: r.get(1)?,
                day: r.get(2)?,
                count: r.get(3)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every photo taken in a period, as `(path, copy)`. `None` widens: a day, a month, a year.
    ///
    /// ⚠ Joins back to `photos` on `(folder_id, file_name)`, so **every copy** of a matching file
    /// comes back — capture is per file, a browsable source is per (file, copy). The rule
    /// `keyword_members` already follows.
    pub fn photos_in_period(
        &self,
        year: i32,
        month: Option<u32>,
        day: Option<u32>,
    ) -> Result<Vec<(PathBuf, u32)>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, p.file_name, p.copy
             FROM photo_capture c
             JOIN photos p ON p.folder_id = c.folder_id AND p.file_name = c.file_name
             JOIN folders f ON f.id = c.folder_id
             WHERE c.taken IS NOT NULL
               AND CAST(strftime('%Y', c.taken, 'unixepoch') AS INTEGER) = ?1
               AND (?2 IS NULL OR CAST(strftime('%m', c.taken, 'unixepoch') AS INTEGER) = ?2)
               AND (?3 IS NULL OR CAST(strftime('%d', c.taken, 'unixepoch') AS INTEGER) = ?3)
             ORDER BY f.path, p.file_name, p.copy",
        )?;
        let rows = stmt.query_map(rusqlite::params![year, month, day], |r| {
            let folder: String = r.get(0)?;
            let name: String = r.get(1)?;
            Ok((PathBuf::from(folder).join(name), r.get::<_, u32>(2)?))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Every photo taken in **any** of these periods, as `(path, copy)`, de-duplicated.
    ///
    /// ⚠ `photos_in_period` stays as the single-period case rather than being folded into this —
    /// it is what a plain click uses, and routing it through here would be the same query with
    /// more machinery.
    ///
    /// ⚠⚠ An **empty** slice matches nothing, not everything: it is the state the date panel is
    /// in before anything is clicked, and showing the whole library then would be surprising.
    ///
    /// ⚠ De-duplicated explicitly rather than by a `DISTINCT` over a union, because the periods
    /// can **overlap** — a day and the month containing it are both legitimate selections, and a
    /// photo in both must appear once.
    pub fn photos_in_periods(
        &self,
        periods: &[(i32, Option<u32>, Option<u32>)],
    ) -> Result<Vec<(PathBuf, u32)>, CatalogError> {
        let mut out: Vec<(PathBuf, u32)> = Vec::new();
        for &(year, month, day) in periods {
            for hit in self.photos_in_period(year, month, day)? {
                if !out.contains(&hit) {
                    out.push(hit);
                }
            }
        }
        Ok(out)
    }

    /// Every distinct camera the catalog has seen, sorted.
    pub fn distinct_cameras(&self) -> Result<Vec<String>, CatalogError> {
        self.distinct_capture_column("camera")
    }

    /// Every distinct lens the catalog has seen, sorted.
    pub fn distinct_lenses(&self) -> Result<Vec<String>, CatalogError> {
        self.distinct_capture_column("lens")
    }

    /// ⚠ The column name is interpolated, which is safe **only** because both callers pass a
    /// literal — SQLite cannot bind an identifier. Private for exactly that reason.
    fn distinct_capture_column(&self, col: &str) -> Result<Vec<String>, CatalogError> {
        let sql = format!(
            "SELECT DISTINCT {col} FROM photo_capture
             WHERE {col} IS NOT NULL AND {col} != '' ORDER BY lower({col}), {col}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::entries_in;
    use core_image::Capture;
    use std::path::Path;

    /// A capture at a given wall-clock date.
    ///
    /// ⚠ The stored value is naive wall-clock seconds, so building it from a civil date with no
    /// timezone step is not a bug — it is the convention both readers use. See `Capture::taken`.
    fn on_day(y: i32, m: u32, d: u32) -> Capture {
        Capture {
            taken: Some(days_from_civil(y, m, d) * 86_400),
            ..Default::default()
        }
    }

    /// Days since the epoch for a civil date — Howard Hinnant's `days_from_civil`.
    ///
    /// ⚠ Written out rather than pulling `chrono` into `catalog` for one test helper. The
    /// algorithm is standard and the test below pins it against known dates.
    fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as i64;
        let mp = ((m + 9) % 12) as i64;
        let doy = (153 * mp + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era as i64 * 146_097 + doe - 719_468
    }

    #[test]
    fn the_day_helper_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2026, 5, 17), 20590);
    }

    #[test]
    fn capture_days_groups_by_wall_clock_date() {
        // ⚠ The payoff of storing wall-clock: SQL's `strftime` gives back exactly the date the
        // camera wrote, with no timezone arithmetic anywhere.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            Path::new("/a"),
            &entries_in("/a", &["one.RAF", "two.RAF", "three.RAF"]),
        )
        .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        cat.set_capture(Path::new("/a/three.RAF"), &on_day(2026, 7, 9))
            .unwrap();
        // ⚠ Both rows are 2026, so month-ascending order puts May before July. The year ordering
        // is pinned separately by `capture_days_puts_the_newest_year_first`.
        assert_eq!(
            cat.capture_days().unwrap(),
            vec![
                DayRow {
                    year: 2026,
                    month: 5,
                    day: 17,
                    count: 2
                },
                DayRow {
                    year: 2026,
                    month: 7,
                    day: 9,
                    count: 1
                },
            ]
        );
    }

    /// ⚠⚠ **Years newest first, months chronological inside them** (2026-09-23, settled by use).
    /// You pick the year by recency, then read it forwards. ⚠ The test above cannot see this: both
    /// of its rows are the same year.
    #[test]
    fn capture_days_puts_the_newest_year_first_and_reads_its_months_forwards() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            Path::new("/a"),
            &entries_in("/a", &["old.RAF", "mid.RAF", "new.RAF"]),
        )
        .unwrap();
        cat.set_capture(Path::new("/a/old.RAF"), &on_day(2024, 8, 1))
            .unwrap();
        cat.set_capture(Path::new("/a/mid.RAF"), &on_day(2026, 3, 4))
            .unwrap();
        cat.set_capture(Path::new("/a/new.RAF"), &on_day(2026, 11, 2))
            .unwrap();
        let got: Vec<(i32, u32)> = cat
            .capture_days()
            .unwrap()
            .into_iter()
            .map(|r| (r.year, r.month))
            .collect();
        assert_eq!(got, vec![(2026, 3), (2026, 11), (2024, 8)]);
    }

    #[test]
    fn a_file_with_no_date_is_absent_from_the_tree() {
        // ⚠ A capture row exists for it (it was read) but it has nothing to place on a calendar.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &Capture::default())
            .unwrap();
        assert!(cat.capture_days().unwrap().is_empty());
    }

    #[test]
    fn a_period_narrows_from_a_year_down_to_a_day() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["three.RAF"]))
            .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &on_day(2026, 7, 9))
            .unwrap();
        cat.set_capture(Path::new("/b/three.RAF"), &on_day(2025, 7, 9))
            .unwrap();

        // ⚠ Spans folders, like every other browsable source.
        assert_eq!(cat.photos_in_period(2026, None, None).unwrap().len(), 2);
        assert_eq!(cat.photos_in_period(2026, Some(7), None).unwrap().len(), 1);
        assert_eq!(
            cat.photos_in_period(2026, Some(7), Some(9)).unwrap().len(),
            1
        );
        assert_eq!(
            cat.photos_in_period(2026, Some(5), Some(9)).unwrap().len(),
            0
        );
        assert_eq!(cat.photos_in_period(2025, None, None).unwrap().len(), 1);
    }

    #[test]
    fn a_period_returns_every_copy_of_a_matching_file() {
        // ⚠ Capture is per file; a browsable source is per (file, copy). Every copy of a photo
        // taken that day belongs in the result — the rule `keyword_members` already follows.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        let mut copies: Vec<u32> = cat
            .photos_in_period(2026, Some(5), Some(17))
            .unwrap()
            .into_iter()
            .map(|(_, c)| c)
            .collect();
        copies.sort();
        assert_eq!(copies, [0, 1]);
    }

    #[test]
    fn captures_in_returns_one_row_per_file_that_has_one() {
        // ⚠ One query per folder, never one per photo — the rule `marks_in` already follows.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        let got = cat.captures_in(Path::new("/a")).unwrap();
        // ⚠ Keyed by **path**, so a file with two copies is one entry — capture is per file.
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, PathBuf::from("/a/one.RAF"));
        assert_eq!(got[0].1.taken, on_day(2026, 5, 17).taken);
    }

    #[test]
    fn distinct_cameras_and_lenses_come_back_sorted_and_deduplicated() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            Path::new("/a"),
            &entries_in("/a", &["one.RAF", "two.RAF", "three.RAF"]),
        )
        .unwrap();
        let mk = |cam: &str, lens: &str| Capture {
            camera: Some(cam.into()),
            lens: Some(lens.into()),
            ..Default::default()
        };
        cat.set_capture(
            Path::new("/a/one.RAF"),
            &mk("FUJIFILM X-T4", "XF23mmF2 R WR"),
        )
        .unwrap();
        cat.set_capture(
            Path::new("/a/two.RAF"),
            &mk("FUJIFILM X-T4", "XF16-80mmF4 R OIS WR"),
        )
        .unwrap();
        cat.set_capture(Path::new("/a/three.RAF"), &Capture::default())
            .unwrap();
        assert_eq!(
            cat.distinct_cameras().unwrap(),
            vec!["FUJIFILM X-T4".to_string()]
        );
        assert_eq!(
            cat.distinct_lenses().unwrap(),
            vec![
                "XF16-80mmF4 R OIS WR".to_string(),
                "XF23mmF2 R WR".to_string()
            ]
        );
    }

    #[test]
    fn several_periods_come_back_as_one_set() {
        // ⚠ A union, not a repeated query: a photo in two of the requested periods (a day and the
        // month containing it) must appear **once**.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(
            Path::new("/a"),
            &entries_in("/a", &["one.RAF", "two.RAF", "three.RAF"]),
        )
        .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &on_day(2026, 7, 9))
            .unwrap();
        cat.set_capture(Path::new("/a/three.RAF"), &on_day(2025, 1, 1))
            .unwrap();

        let got = cat
            .photos_in_periods(&[(2026, Some(5), Some(17)), (2026, Some(7), Some(9))])
            .unwrap();
        assert_eq!(got.len(), 2);

        // ⚠ Overlapping periods must not double a photo.
        let overlapping = cat
            .photos_in_periods(&[(2026, Some(5), Some(17)), (2026, Some(5), None)])
            .unwrap();
        assert_eq!(
            overlapping.len(),
            1,
            "an overlapping period doubled a photo"
        );
    }

    #[test]
    fn an_empty_period_list_matches_nothing() {
        // ⚠ Not "everything": an empty selection is the state the panel is in before any date is
        // clicked, and showing the whole library then would be surprising.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17))
            .unwrap();
        assert!(cat.photos_in_periods(&[]).unwrap().is_empty());
    }
}
