# Filtering on capture metadata — M4.3b, Plan 2 of 2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the capture metadata plan 1 collected usable — browse by date from the left panel, narrow by camera and lens from the Filter popup.

**Architecture:** A date tree in the panel, built from a `GROUP BY` over what is actually in the catalog, browsed like a keyword. Camera and lens join `LibraryFilter` as multi-select sets, matched against a per-`Library` capture map loaded the way `marks` already is.

**Tech Stack:** Rust, `rusqlite`, `egui`. **No new dependencies, no schema change** — plan 1 put everything in place.

**Spec:** `docs/superpowers/specs/2026-08-27-capture-metadata-design.md`

---

## The decision this plan rests on, and the data behind it

Measured on the author's real catalog after plan 1 filled it:

| | distinct values |
|---|---|
| **cameras** | **1** |
| lenses | 2 (824 / 6) |
| months | 2 — May 92, July 738 |
| **days** | **6** |

⚠⚠ **The camera filter — the criterion the roadmap named — has zero discriminating power on this
library**, and a *month* filter nearly none, since 738 of 830 photos fall in one month. **Date at
day granularity is the one that does real work here.** A ten-year archive inverts that, which is
why no single granularity can be chosen: the tree expands.

**Dates are BROWSED from the left panel, like keywords — not filtered.** Clicking `2026 › July ›
9` replaces the Library's contents with that day's photos from any folder.

⚠ **Why browse rather than filter, and the cost.** It needs **no date picker** — egui has none, and
a tree built from a `GROUP BY` shows the six days that exist instead of asking anyone to type a
range. It is also the idiom the panel already uses four times (All Photos, Folders, Collections,
Keywords: *a list with counts, click to browse*). **The cost: "in my Portfolio collection, show me
July" is not expressible** — exactly the limitation keywords already have, and exactly what
**M4.3c smart collections** exists to fix.

**Camera and lens go in the Filter popup** as multi-select sets, because they *are* narrowing
criteria and they are small lists.

---

## Ground rules for this repo

- **Branch:** `m4.3b-metadata-filtering`. **Never push** — the user pushes.
- **Commit messages carry no `Co-Authored-By` trailer.**
- **Build env** — every `cargo` command needs this first, in the same shell:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **The GUI binary is `Rustroom`**: bin tests are `cargo test -p app --bin Rustroom`.
- **⚠ In a binary crate an item lands with its first caller** — twelfth instance.
- **⚠ Check exit codes, never grep output.**
- **⚠ The sabotage compile check is "did a `test result:` line appear at all"**, not a grep for
  `^error:`.
- ⚠ `clippy --all-targets --all-features` and `test --all` each take **over two minutes** here.

---

## File Structure

| file | responsibility | new? |
|---|---|---|
| `crates/catalog/src/db.rs` | `DayRow`, `capture_days`, `photos_in_period`, `captures_in`, `distinct_cameras`, `distinct_lenses` | modify |
| `crates/app/src/date_tree.rs` | group flat day rows into a year › month › day tree | **create** |
| `crates/app/src/library_filter.rs` | `cameras`/`lenses` sets; `matches` takes a `Capture` | modify |
| `crates/app/src/main.rs` | `LibrarySource::Period`; `open_period`; `Library.captures`; `reload_captures` | modify |
| `crates/app/src/collections_panel.rs` | a Dates section | modify |
| `crates/app/src/shell.rs` | Camera and Lens sections in the Filter popup | modify |

---

## Task 1: Catalog queries

**Files:** Modify `crates/catalog/src/db.rs`

- [ ] **Step 1: Write the failing tests**

Use the `entries_in` helper and `Capture` already in that test module.

```rust
    /// A capture at a given wall-clock date, for the date-tree tests.
    ///
    /// ⚠ The stored value is naive wall-clock seconds, so building it with `and_utc()` is not a
    /// timezone bug — it is the same convention both readers use. See `Capture::taken`.
    fn on_day(y: i32, m: u32, d: u32) -> Capture {
        let secs = chrono_free_days_from_civil(y, m, d) * 86_400;
        Capture {
            taken: Some(secs),
            ..Default::default()
        }
    }

    /// Days since the epoch for a civil date — Howard Hinnant's `days_from_civil`.
    ///
    /// ⚠ Written out rather than pulling `chrono` into `catalog` for two test helpers. The
    /// algorithm is standard and its own test below pins it against known dates.
    fn chrono_free_days_from_civil(y: i32, m: u32, d: u32) -> i64 {
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
        assert_eq!(chrono_free_days_from_civil(1970, 1, 1), 0);
        assert_eq!(chrono_free_days_from_civil(2000, 3, 1), 11017);
        assert_eq!(chrono_free_days_from_civil(2026, 5, 17), 20590);
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
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17)).unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &on_day(2026, 5, 17)).unwrap();
        cat.set_capture(Path::new("/a/three.RAF"), &on_day(2026, 7, 9)).unwrap();
        assert_eq!(
            cat.capture_days().unwrap(),
            vec![
                DayRow { year: 2026, month: 5, day: 17, count: 2 },
                DayRow { year: 2026, month: 7, day: 9, count: 1 },
            ]
        );
    }

    #[test]
    fn a_file_with_no_date_is_absent_from_the_tree() {
        // ⚠ A capture row exists for it (it was read) but it has nothing to place on a calendar.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"])).unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &Capture::default()).unwrap();
        assert!(cat.capture_days().unwrap().is_empty());
    }

    #[test]
    fn a_period_narrows_from_a_year_down_to_a_day() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.sync_folder(Path::new("/b"), &entries_in("/b", &["three.RAF"])).unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17)).unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &on_day(2026, 7, 9)).unwrap();
        cat.set_capture(Path::new("/b/three.RAF"), &on_day(2025, 7, 9)).unwrap();

        // ⚠ Spans folders, like every other browsable source.
        assert_eq!(cat.photos_in_period(2026, None, None).unwrap().len(), 2);
        assert_eq!(cat.photos_in_period(2026, Some(7), None).unwrap().len(), 1);
        assert_eq!(cat.photos_in_period(2026, Some(7), Some(9)).unwrap().len(), 1);
        assert_eq!(cat.photos_in_period(2026, Some(5), Some(9)).unwrap().len(), 0);
        assert_eq!(cat.photos_in_period(2025, None, None).unwrap().len(), 1);
    }

    #[test]
    fn a_period_returns_every_copy_of_a_matching_file() {
        // ⚠ Capture is per file; a browsable source is per (file, copy). Every copy of a photo
        // taken that day belongs in the result — the rule `keyword_members` already follows.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"])).unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17)).unwrap();
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
        // ⚠ One query per folder, never one per photo — the rule `marks_in` already follows,
        // because the grid draws sixty times a second over hundreds of thumbnails.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17)).unwrap();
        let got = cat.captures_in(Path::new("/a")).unwrap();
        // ⚠ Keyed by **path**, so a file with two copies is one entry — capture is per file.
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, PathBuf::from("/a/one.RAF"));
        assert_eq!(got[0].1.taken, on_day(2026, 5, 17).taken);
    }

    #[test]
    fn distinct_cameras_and_lenses_come_back_sorted_and_deduplicated() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF", "three.RAF"]))
            .unwrap();
        let mk = |cam: &str, lens: &str| Capture {
            camera: Some(cam.into()),
            lens: Some(lens.into()),
            ..Default::default()
        };
        cat.set_capture(Path::new("/a/one.RAF"), &mk("FUJIFILM X-T4", "XF23mmF2 R WR")).unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &mk("FUJIFILM X-T4", "XF16-80mmF4 R OIS WR")).unwrap();
        cat.set_capture(Path::new("/a/three.RAF"), &Capture::default()).unwrap();
        assert_eq!(cat.distinct_cameras().unwrap(), vec!["FUJIFILM X-T4".to_string()]);
        assert_eq!(
            cat.distinct_lenses().unwrap(),
            vec!["XF16-80mmF4 R OIS WR".to_string(), "XF23mmF2 R WR".to_string()]
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p catalog 2>&1 | tail -15
```

Expected: compile errors — `DayRow`, `capture_days`, `photos_in_period`, `distinct_cameras`,
`distinct_lenses` do not exist.

- [ ] **Step 3: Write the implementation**

Beside `FolderRow`:

```rust
/// One calendar day that has photos on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayRow {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub count: i64,
}
```

And in `impl Catalog`:

```rust
    /// Every calendar day with photos on it, ascending.
    ///
    /// ⚠⚠ Grouped by SQL's `strftime`, which works **because `taken` is naive wall-clock
    /// seconds**: the date that comes back is exactly the one the camera wrote, with no timezone
    /// arithmetic anywhere. That is the payoff of the storage decision in plan 1.
    ///
    /// ⚠ Day granularity, not month, because the right granularity depends on the library —
    /// measured, 738 of 830 photos here fall in one month while six distinct days discriminate.
    /// The caller aggregates upward.
    pub fn capture_days(&self) -> Result<Vec<DayRow>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(strftime('%Y', taken, 'unixepoch') AS INTEGER),
                    CAST(strftime('%m', taken, 'unixepoch') AS INTEGER),
                    CAST(strftime('%d', taken, 'unixepoch') AS INTEGER),
                    count(*)
             FROM photo_capture WHERE taken IS NOT NULL
             GROUP BY 1, 2, 3 ORDER BY 1, 2, 3",
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

    /// Every photo taken in a period, as `(path, copy)`. `None` widens: a year, a month, a day.
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

    /// Every capture row in a folder, as `(path, capture)`.
    ///
    /// ⚠⚠ One query per folder per rebuild, never one per photo — the rule `marks_in` follows.
    /// ⚠ Keyed by path, so a file with several copies appears once: capture is per file.
    /// ⚠ Only files that have been *read* come back; the rest are simply absent.
    pub fn captures_in(&self, folder: &Path) -> Result<Vec<(PathBuf, Capture)>, CatalogError> {
        let key = folder.to_string_lossy().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT c.file_name, c.taken, c.camera, c.lens, c.iso, c.shutter, c.aperture,
                    c.focal_len
             FROM photo_capture c JOIN folders f ON f.id = c.folder_id
             WHERE f.path = ?1",
        )?;
        let rows = stmt.query_map([&key], |r| {
            let name: String = r.get(0)?;
            Ok((
                PathBuf::from(&key).join(name),
                Capture {
                    taken: r.get(1)?,
                    camera: r.get(2)?,
                    lens: r.get(3)?,
                    iso: r.get::<_, Option<i64>>(4)?.map(|v| v as u32),
                    shutter: r.get::<_, Option<f64>>(5)?.map(|v| v as f32),
                    aperture: r.get::<_, Option<f64>>(6)?.map(|v| v as f32),
                    focal_len: r.get::<_, Option<f64>>(7)?.map(|v| v as f32),
                },
            ))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
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
    /// literal — SQLite cannot bind an identifier. It is private for that reason.
    fn distinct_capture_column(&self, col: &str) -> Result<Vec<String>, CatalogError> {
        let sql = format!(
            "SELECT DISTINCT {col} FROM photo_capture
             WHERE {col} IS NOT NULL AND {col} != '' ORDER BY lower({col}), {col}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).collect())
    }
```

- [ ] **Step 4: Run, then sabotage**

Each checked for a `test result:` line before believing it:

| sabotage | expected |
|---|---|
| `photos_in_period` drops its `?2 IS NULL OR` widening | `a_period_narrows_from_a_year_down_to_a_day` fails |
| its `JOIN photos` becomes a `photo_capture`-only select | `a_period_returns_every_copy_of_a_matching_file` fails |
| `capture_days` drops `WHERE taken IS NOT NULL` | `a_file_with_no_date_is_absent_from_the_tree` fails |
| `distinct_capture_column` drops `DISTINCT` | the cameras/lenses test fails |

- [ ] **Step 5: Commit**

```bash
git add crates/catalog/src/db.rs
git commit -m "feat(catalog): query photos by capture date, camera and lens"
```

---

## Task 2: `date_tree` — grouping flat days into a tree

**Files:** Create `crates/app/src/date_tree.rs`, modify `crates/app/src/lib.rs`

- [ ] **Step 1: Write the module with `todo!()` and the tests**

```rust
//! Grouping the catalog's flat day rows into a year › month › day tree.
//!
//! ⚠ Pure, and egui-free, so the aggregation is testable without a window — the `view_region` and
//! `thumb_window` precedent.
//!
//! ⚠ The catalog returns **days** rather than a pre-built tree because the useful granularity
//! depends on the library: measured, 738 of 830 photos here fall in one month while six distinct
//! days discriminate, and a ten-year archive is the other way round.

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

/// Month names for the tree. ⚠ Indexed 1..=12; `name(0)` is unreachable from real data but must
/// not panic.
pub fn month_name(month: u32) -> &'static str {
    todo!()
}

/// Group ascending day rows into years and months, preserving order and summing counts.
pub fn group(days: &[DayRow]) -> Vec<Year> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(year: i32, month: u32, day: u32, count: i64) -> DayRow {
        DayRow { year, month, day, count }
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
```

Declare `pub mod date_tree;` in `crates/app/src/lib.rs`.

- [ ] **Step 2: Run to verify they fail**, then implement:

```rust
pub fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January", "February", "March", "April", "May", "June",
        "July", "August", "September", "October", "November", "December",
    ];
    NAMES.get(month.wrapping_sub(1) as usize).copied().unwrap_or("—")
}

pub fn group(days: &[DayRow]) -> Vec<Year> {
    let mut years: Vec<Year> = Vec::new();
    for &row in days {
        // ⚠ Matches on the **year**, not just the month — grouping on the month alone folds July
        // 2025 into July 2026.
        if years.last().map(|y| y.year) != Some(row.year) {
            years.push(Year { year: row.year, count: 0, months: Vec::new() });
        }
        let y = years.last_mut().expect("just pushed");
        y.count += row.count;
        if y.months.last().map(|m| m.month) != Some(row.month) {
            y.months.push(Month { month: row.month, count: 0, days: Vec::new() });
        }
        let m = y.months.last_mut().expect("just pushed");
        m.count += row.count;
        m.days.push(row);
    }
    years
}
```

⚠ This relies on the input being **ascending**, which `capture_days` guarantees with its
`ORDER BY`. Say so in the doc comment: the alternative is a `HashMap` and a re-sort for no gain.

- [ ] **Step 3: Verify and commit**

```bash
cargo test -p app --lib date_tree 2>&1 | grep -E "^test result:"
git add -A && git commit -m "feat(app): group capture days into a year-month-day tree"
```

---

## Task 3: Camera and lens in `LibraryFilter`

**Files:** Modify `crates/app/src/library_filter.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Extend the filter**

```rust
    /// Which cameras to show. Empty matches everything.
    pub cameras: std::collections::BTreeSet<String>,
    /// Which lenses to show. Empty matches everything.
    pub lenses: std::collections::BTreeSet<String>,
```

⚠ `BTreeSet<String>` rather than a bitmask, unlike the mark filters: the values are open-ended
strings discovered from the catalog, not a fixed small enum.

`matches` gains a third argument:

```rust
    pub fn matches(&self, entry: &PhotoEntry, marks: Marks, capture: &Capture) -> bool {
```

with, after the existing criteria:

```rust
        if !self.cameras.is_empty()
            && !capture
                .camera
                .as_deref()
                .is_some_and(|c| self.cameras.contains(c))
        {
            return false;
        }
        if !self.lenses.is_empty()
            && !capture.lens.as_deref().is_some_and(|l| self.lenses.contains(l))
        {
            return false;
        }
```

⚠ **A photo with no camera recorded fails a non-empty camera filter**, and that is right: "show me
the X-T4 shots" should not include a file whose camera is unknown. `is_some_and` says so directly.

Extend `is_active` and `clear_marks_and_text` with both sets.

- [ ] **Step 2: Tests**

```rust
    fn cap(camera: &str, lens: &str) -> Capture {
        Capture {
            camera: (!camera.is_empty()).then(|| camera.to_string()),
            lens: (!lens.is_empty()).then(|| lens.to_string()),
            ..Default::default()
        }
    }

    fn set(items: &[&str]) -> std::collections::BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_camera_set_narrows_and_an_empty_one_does_not() {
        let e = entry("a.RAF", 0, None);
        let f = LibraryFilter { cameras: set(&["FUJIFILM X-T4"]), ..Default::default() };
        assert!(f.matches(&e, Marks::default(), &cap("FUJIFILM X-T4", "")));
        assert!(!f.matches(&e, Marks::default(), &cap("CANON EOS R5", "")));
        assert!(LibraryFilter::default().matches(&e, Marks::default(), &cap("CANON EOS R5", "")));
    }

    #[test]
    fn several_cameras_can_be_selected_at_once() {
        let e = entry("a.RAF", 0, None);
        let f = LibraryFilter {
            cameras: set(&["FUJIFILM X-T4", "CANON EOS R5"]),
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
        let f = LibraryFilter { cameras: set(&["FUJIFILM X-T4"]), ..Default::default() };
        assert!(!f.matches(&e, Marks::default(), &Capture::default()));
        // …but with no camera filter it is shown like anything else.
        assert!(LibraryFilter::default().matches(&e, Marks::default(), &Capture::default()));
    }

    #[test]
    fn the_lens_set_narrows_independently_of_the_camera_set() {
        let e = entry("a.RAF", 0, None);
        let f = LibraryFilter { lenses: set(&["XF23mmF2 R WR"]), ..Default::default() };
        assert!(f.matches(&e, Marks::default(), &cap("FUJIFILM X-T4", "XF23mmF2 R WR")));
        assert!(!f.matches(&e, Marks::default(), &cap("FUJIFILM X-T4", "XF16-80mmF4 R OIS WR")));
    }

    #[test]
    fn is_active_reports_the_capture_sets_too() {
        assert!(LibraryFilter { cameras: set(&["X"]), ..Default::default() }.is_active());
        assert!(LibraryFilter { lenses: set(&["Y"]), ..Default::default() }.is_active());
    }

    #[test]
    fn clearing_empties_the_capture_sets() {
        let mut f = LibraryFilter {
            cameras: set(&["X"]),
            lenses: set(&["Y"]),
            ..Default::default()
        };
        f.clear_marks_and_text();
        assert!(f.cameras.is_empty() && f.lenses.is_empty());
    }
```

⚠ **Every existing `matches` call in the tests gains a third argument.** Pass `&Capture::default()`
and **change nothing else about their assertions** — their continuing to pass is the evidence this
extension moved no behaviour.

- [ ] **Step 3: `Library` carries a capture map**

In `main.rs`, beside `marks`:

```rust
    /// Capture metadata for everything on show, keyed by **path**.
    ///
    /// ⚠ By path, not `PhotoId`: capture is per file, and every copy shares it. The `marks` map is
    /// keyed by `PhotoId` for the opposite reason, and both carry a comment saying so.
    ///
    /// ⚠⚠ Read **once per rebuild**, never per frame and never per cell — the rule `marks`
    /// already follows, and for the same reason: the grid draws sixty times a second.
    captures: std::collections::HashMap<PathBuf, core_image::Capture>,
```

`reload_captures` uses `captures_in` from Task 1 — one query per folder represented, never one per
photo — and mirrors `reload_marks` exactly (one query per folder represented, then
`refilter`). `refilter` passes `self.captures.get(&e.id.path).cloned().unwrap_or_default()`.

⚠ **`reload_captures` must run wherever `reload_marks` does**, plus after the metadata scanner
drains — otherwise a newly-scanned folder does not become filterable until something else rebuilds.
That is the `reload_marks` refilter rule, one layer along.

- [ ] **Step 4: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A && git commit -m "feat(app): filter by camera and lens"
```

---

## Task 4: The UI

**Files:** Modify `crates/app/src/collections_panel.rs`, `crates/app/src/shell.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: `LibrarySource::Period`**

```rust
    /// Photos taken in a calendar period — a year, a month, or a day.
    Period {
        year: i32,
        month: Option<u32>,
        day: Option<u32>,
        label: String,
    },
```

Build, and fix what the compiler names. ⚠ `reindex`'s message widens again; the `let … else` on
`LibrarySource::Folder` already refuses correctly.

`open_period` mirrors `open_keyword`: `photos_in_period` → `collections::scan_of(&members,
sidecar::edited_at, fs_modified)` → `catalog::sort` → `open_scan`.

- [ ] **Step 2: The Dates section**

In `collections_panel.rs`, below Keywords, following that file's existing idiom exactly (heading,
separator, `ScrollArea`, `selectable_label` rows). Years and months are `CollapsingHeader`s; a
click on any level browses that period.

```rust
            ui.heading("Dates");
            let days = app
                .catalog
                .as_ref()
                .and_then(|c| c.capture_days().ok())
                .unwrap_or_default();
            if days.is_empty() {
                crate::theme::hint(ui, "No capture dates yet.");
            }
            for y in rustroom_app::date_tree::group(&days) {
                egui::CollapsingHeader::new(format!("{} ({})", y.year, y.count))
                    .id_salt(("year", y.year))
                    .show(ui, |ui| {
                        if ui.selectable_label(false, format!("All of {}", y.year)).clicked() {
                            action = Action::OpenPeriod(y.year, None, None);
                        }
                        for m in &y.months {
                            let name = rustroom_app::date_tree::month_name(m.month);
                            egui::CollapsingHeader::new(format!("{name} ({})", m.count))
                                .id_salt(("month", y.year, m.month))
                                .show(ui, |ui| {
                                    if ui.selectable_label(false, format!("All of {name}")).clicked()
                                    {
                                        action = Action::OpenPeriod(y.year, Some(m.month), None);
                                    }
                                    for d in &m.days {
                                        let label = format!("{} {} ({})", d.day, name, d.count);
                                        if ui.selectable_label(false, label).clicked() {
                                            action = Action::OpenPeriod(
                                                y.year,
                                                Some(m.month),
                                                Some(d.day),
                                            );
                                        }
                                    }
                                });
                        }
                    });
            }
```

⚠ **`id_salt` on every header.** egui's auto-ids shift with what was drawn before them — the same
hazard the grid's `push_id` fixed. Two years with the same month number would otherwise collapse
together.

⚠ **The open period is highlighted**, like a selected collection or keyword — the rows above show
`false` only to keep the sketch short. Match on `LibrarySource::Period { year, month, day, .. }`
and pass the comparison as `selectable_label`'s first argument. Consistency matters more here than
the three lines it costs: every other row in this panel shows what is open.

- [ ] **Step 3: Camera and Lens in the Filter popup**

Two more sections in `mark_filter`, after Colour, following the checkbox idiom exactly and built
from `distinct_cameras()`/`distinct_lenses()`.

⚠ **Show a section only when it has more than one value.** Measured: this library has **one**
camera, so a camera section would be a single checkbox that can only hide everything — the same
reasoning that hides the format chips below two formats.

⚠ The popup is now tall enough to need `egui::ScrollArea::vertical().max_height(…)`.

⚠ `mark_filter_label` gains both, and the label must stay `Filter: …` prefixed.

- [ ] **Step 4: Verify, then run and check by hand**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
cargo run --release -p app
```

None of this is test-covered:

1. The panel shows **Dates** with `2026 (830)` → `May (92)` / `July (738)` → six days.
2. Clicking `9 July (544)` shows 544 photos; clicking `All of July` shows 738.
3. A day spanning two folders shows photos from both.
4. The **Lens** section appears in the popup (2 values) and the **Camera** section does **not**
   (1 value). ⚠ If camera does appear, the >1 guard is wrong.
5. Ticking a lens narrows the grid, and the button reads `Filter: …`.
6. `Reindex` says something sensible on a Period source.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(app): browse by capture date, filter by camera and lens"
```

---

## Task 5: Docs

- [ ] Spec header → `**Status:** built 2026-08-27 (both plans)`, with the real distribution added.
- `CLAUDE.md` — a section: the measurement that chose browse-over-filter (1 camera, 6 days), the
  no-date-picker consequence, why capture is keyed by path where marks are keyed by `PhotoId`, the
  `is_some_and` rule for photos with no camera, `id_salt` on the tree, the >1 guard, and the
  caveats. Update the test count; next action **M4.3c — smart collections, which closes M4.3**.
- The Obsidian roadmap — mark M4.3b complete.
- [ ] Final `rustup update stable`, full verification by **exit code**, commit. **Do not push.**

---

## Definition of done

- `fmt`, `clippy --all-targets --all-features -D warnings`, `test --all` all exit 0.
- **All 47 goldens byte-identical.**
- The date tree on the real catalog shows 2026 › May/July › six days with the counts above, and
  clicking a day browses exactly those photos.
- The camera section is hidden (one value) and the lens section works.
