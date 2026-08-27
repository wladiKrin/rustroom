# Smart collections — M4.3c, Plan 1 of 2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A saved rule that auto-populates — created from whatever you are currently looking at, evaluated across the whole catalog.

**Architecture:** A boolean expression tree with a **pure** evaluator, stored as JSON in a new schema-v5 table, browsed as a fifth `LibrarySource` by evaluating the rule over All Photos.

**Tech Stack:** Rust, `serde_json`, `rusqlite`, `egui`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-27-smart-collections-design.md`

**Plan 2 (the rule editor, and multi-select dates) is a separate document.** Do not start it here.

---

## Ground rules for this repo

- **Branch:** `m4.3c-smart-collections`. **Never push** — the user pushes.
- **Commit messages carry no `Co-Authored-By` trailer.**
- **Build env** — every `cargo` command needs this first, in the same shell:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **The GUI binary is `Rustroom`**: bin tests are `cargo test -p app --bin Rustroom`.
- **⚠ In a binary crate an item lands with its first caller** — fourteenth instance.
- **⚠ Check exit codes, never grep output.** ⚠ And **run clippy before every commit** — three
  commits this session went in with it failing while `cargo test` was green.
- **⚠ The sabotage compile check is "did a `test result:` line appear at all"**, not a grep for
  `^error:`.
- ⚠ `clippy --all-targets --all-features` and `test --all` each take **over two minutes** here.

---

## File Structure

| file | responsibility | new? |
|---|---|---|
| `crates/app/src/rule.rs` | `Expr`, `Criterion`, `Facts`, and the pure evaluator | **create** |
| `crates/app/src/lib.rs` | declare it | modify |
| `crates/catalog/src/db.rs` | schema v5, `smart_collections` CRUD | modify |
| `crates/app/src/main.rs` | `LibrarySource::Smart`; `open_smart`; membership maps; `rule_from_current` | modify |
| `crates/app/src/collections_panel.rs` | a Smart section, and "Save as smart collection" | modify |

---

## Task 1: The rule and its evaluator

**Files:** Create `crates/app/src/rule.rs`; modify `crates/app/src/lib.rs`

- [ ] **Step 1: Write the module with `todo!()` and the tests**

```rust
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
        todo!()
    }
}

/// One thing that can be true of a photo.
///
/// ⚠ The union of today's *sources* and today's *filters*: `Collection`, `Keyword` and `Period`
/// are `LibrarySource` variants elsewhere, and it is exactly their absence from `LibraryFilter`
/// that made a saved filter insufficient as a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Criterion {
    Folder(std::path::PathBuf),
    Collection(i64),
    /// ⚠ Matches the keyword **or any of its descendants** — searching `Europe` finds photos
    /// tagged `Paris`, the rule M4.2b established.
    Keyword(i64),
    Period {
        year: i32,
        month: Option<u32>,
        day: Option<u32>,
    },
    /// Case-insensitive substring of the row's **shown name**.
    Text(String),
    /// ⚠⚠ An operator, not a set, because that is what composes under `Any`. The filter bar keeps
    /// its checkboxes; this is a deliberate divergence, and the same one Lightroom has.
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
    Iso { op: Cmp, value: u32 },
    Aperture { op: Cmp, value: f32 },
    Shutter { op: Cmp, value: f32 },
    FocalLength { op: Cmp, value: f32 },
    FileSize { op: Cmp, value: i64 },
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
    /// Keyword ids, **already expanded to include ancestors** — see `Criterion::Keyword`.
    pub keywords: &'a [i64],
    /// File size in bytes, when the catalog recorded one.
    pub size: Option<i64>,
}

impl Expr {
    pub fn matches(&self, f: &Facts) -> bool {
        todo!()
    }
}

impl Criterion {
    pub fn matches(&self, f: &Facts) -> bool {
        todo!()
    }
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
        // (text AND rating>=0) OR format=png  — the second arm false, the first true.
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
```

⚠ **`Flag` must derive `Serialize`/`Deserialize`** for this to compile — check whether it already
does in `catalog::db` and add it there if not.

Declare `pub mod rule;` in `crates/app/src/lib.rs`.

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib rule 2>&1 | tail -15
```

Expected: panics at `not yet implemented` once the types compile.

- [ ] **Step 3: Write the implementation**

```rust
impl Cmp {
    pub fn holds(self, actual: f64, want: f64) -> bool {
        match self {
            Cmp::AtLeast => actual >= want,
            Cmp::Exactly => (actual - want).abs() < 1e-6,
            Cmp::AtMost => actual <= want,
        }
    }
}

impl Expr {
    pub fn matches(&self, f: &Facts) -> bool {
        match self {
            // ⚠ The identity elements, and the state a half-built rule is in: `All([])` is true
            // and `Any([])` is false. Getting these backwards makes an empty rule hide or show
            // everything, which reads as the feature being broken rather than empty.
            Expr::All(v) => v.iter().all(|e| e.matches(f)),
            Expr::Any(v) => v.iter().any(|e| e.matches(f)),
            Expr::Not(e) => !e.matches(f),
            Expr::Is(c) => c.matches(f),
        }
    }
}

impl Criterion {
    pub fn matches(&self, f: &Facts) -> bool {
        // ⚠⚠ Every optional fact fails when absent rather than passing — the `is_some_and` rule
        // the camera filter already follows. "Show me the X-T4 shots" must not include a photo
        // whose camera is unknown.
        match self {
            // ⚠ The **parent**, not a string prefix: `/x` must not match `/xy/a.RAF`.
            Criterion::Folder(p) => f.entry.id.path.parent() == Some(p.as_path()),
            Criterion::Collection(id) => f.collections.contains(id),
            Criterion::Keyword(id) => f.keywords.contains(id),
            Criterion::Period { year, month, day } => {
                f.capture.taken.is_some_and(|t| in_period(t, *year, *month, *day))
            }
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
            Criterion::Iso { op, value } => {
                f.capture.iso.is_some_and(|v| op.holds(v as f64, *value as f64))
            }
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
    let Some(d) = chrono::DateTime::from_timestamp(secs, 0) else {
        return false;
    };
    use chrono::Datelike;
    let d = d.naive_utc().date();
    d.year() == year
        && month.is_none_or(|m| d.month() == m)
        && day.is_none_or(|dd| d.day() == dd)
}
```

⚠ `Option::is_none_or` is stable since 1.82; if clippy on the pinned toolchain prefers
`map_or(true, ..)`, follow clippy.

- [ ] **Step 4: Run, then sabotage**

| sabotage | expected |
|---|---|
| `All(v)` uses `any` and `Any(v)` uses `all` | `all_of_nothing_is_true_and_any_of_nothing_is_false` |
| `Camera` matches when the fact is `None` | `a_missing_fact_fails_rather_than_passes` |
| `Folder` uses `starts_with` on the path | `a_folder_criterion_matches_the_parent_not_a_prefix` |
| `Text` uses `file_name` instead of `shown_name` | `text_matches_the_shown_name_and_format_the_extension` |
| `Period` ignores `day` | `a_period_narrows_from_a_year_to_a_day` |

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A && git commit -m "feat(app): a boolean rule over everything known about a photo"
```

---

## Task 2: Schema v5 and storage

**Files:** Modify `crates/catalog/src/db.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn a_smart_collection_round_trips() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        let rows = cat.smart_collections().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].name, "Keepers");
        assert_eq!(rows[0].rule, r#"{"All":[]}"#);
    }

    #[test]
    fn a_duplicate_name_returns_the_existing_one() {
        // ⚠ The rule `create_collection` and `create_keyword` already follow: the name is the
        // identity a user thinks in, so failing the second time "Keepers" is used is a bug.
        let cat = Catalog::open_in_memory().unwrap();
        let a = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        let b = cat.create_smart("Keepers", r#"{"Any":[]}"#).unwrap();
        assert_eq!(a, b);
        assert_eq!(cat.smart_collections().unwrap().len(), 1);
    }

    #[test]
    fn a_rule_can_be_replaced_without_changing_the_row() {
        let cat = Catalog::open_in_memory().unwrap();
        let id = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        cat.set_smart_rule(id, r#"{"Any":[]}"#).unwrap();
        let rows = cat.smart_collections().unwrap();
        assert_eq!(rows[0].id, id, "editing must not create a second row");
        assert_eq!(rows[0].rule, r#"{"Any":[]}"#);
    }

    #[test]
    fn deleting_a_smart_collection_touches_no_photos() {
        // ⚠ The property people actually worry about — a smart collection is a *query*, and
        // nothing it matched is affected by removing it.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        let id = cat.create_smart("Keepers", r#"{"All":[]}"#).unwrap();
        cat.delete_smart(id).unwrap();
        assert!(cat.smart_collections().unwrap().is_empty());
        assert_eq!(cat.photo_count().unwrap(), 1);
    }
```

⚠ And **add `DROP TABLE smart_collections;` to BOTH migration-simulation drop lists** — there are
two (`a_version_one_…` and `a_version_two_…`), which the v4 sabotage revealed. Without it those
tests fail with "table already exists".

- [ ] **Step 2: Implement**

`SmartRow { id: i64, name: String, rule: String }` beside `CollectionRow`, and a v5 gate:

```rust
        if from < 5 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–4. Fourth time this rule meets a
            // database holding real user data.
            //
            // ⚠ The rule is stored as **JSON text**, not as columns. It is a tree whose shape the
            // editor will grow into; columns would need a migration every time a criterion is
            // added, and the evaluator is the single authority on what the text means.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE smart_collections (
                     id      INTEGER PRIMARY KEY,
                     name    TEXT NOT NULL UNIQUE,
                     rule    TEXT NOT NULL,
                     created INTEGER NOT NULL
                 );
                 PRAGMA user_version = 5;
                 COMMIT;",
            )?;
        }
```

with `create_smart` (`ON CONFLICT(name) DO NOTHING` then select, mirroring `create_collection`),
`smart_collections`, `set_smart_rule` and `delete_smart`.

⚠ `create_smart`'s conflict clause must be **removable to make the insert error** if it is
sabotaged — the M4.1 lesson that `DO UPDATE SET created = created` is an *inert* sabotage and
proves nothing.

- [ ] **Step 3: Run, sabotage, commit**

Sabotages: drop the UNIQUE on `name` → the duplicate test fails; remove `smart_collections` from a
migration drop list → that test fails.

```bash
git add -A && git commit -m "feat(catalog): schema v5 stores smart collections"
```

---

## Task 3: Browsing a smart collection

**Files:** Modify `crates/app/src/main.rs`

- [ ] **Step 1: The membership maps**

`open_smart` needs, for every photo, its collection ids and its **ancestor-expanded** keyword ids.

⚠ Build them with **one query per collection and one per keyword**, reusing `collection_members`
and `keyword_members` — the latter already expands descendants via its recursive CTE, so iterating
keywords and inserting the keyword's own id for each member yields exactly the "matches an
ancestor" semantics `Criterion::Keyword` documents.

```rust
    /// `(path, copy)` → the collection ids it belongs to, and the keyword ids that match it
    /// **including via descendants**.
    ///
    /// ⚠ One query per collection and per keyword, not per photo. A catalog has a handful of
    /// each and ten thousand photos.
    fn memberships(&self) -> (HashMap<PhotoId, Vec<i64>>, HashMap<PathBuf, Vec<i64>>);
```

⚠ Collections are per `(file, copy)`; keywords are per **file**. That asymmetry is M4.2b's, and the
two maps are keyed differently for exactly that reason — **do not unify them.**

- [ ] **Step 2: `LibrarySource::Smart` and `open_smart`**

```rust
    Smart { id: i64, name: String },
```

`open_smart` loads `all_photos`, builds entries, evaluates the rule per entry, and hands the
survivors to `open_scan`.

⚠ **Re-evaluated on every open, never cached as a member list** — the rule `open_collection`
already follows for a different reason: a photo rated into the rule must appear without a restart.

⚠ A rule that fails to parse yields an **empty** collection and a status message, never a panic:
`serde_json::from_str` on user-editable text must degrade.

⚠ `Reindex`'s message widens again.

- [ ] **Step 3: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A && git commit -m "feat(app): browse a smart collection by evaluating its rule"
```

---

## Task 4: The panel, and "Save as smart collection"

**Files:** Modify `crates/app/src/collections_panel.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: `rule_from_current`**

Turn the open source and the live filter into an `Expr::All`:

```rust
    /// The rule that reproduces what is on screen right now.
    ///
    /// ⚠⚠ This is what makes plan 1 usable **without an editor**: browse, filter, save. Its limit
    /// is equally structural — the UI can only be in *one* source at a time, so the rule it
    /// captures has at most one source criterion. Composing two ("Portfolio ∩ July") needs the
    /// editor, which is plan 2 and the actual point of the milestone.
    fn rule_from_current(&self) -> Expr;
```

Mapping: `Folder`→`Criterion::Folder`, `Collection`→`Collection`, `Keyword`→`Keyword`,
`Period`→`Period`, `AllPhotos`→ no source criterion, `Smart`→ ⚠ **no source criterion either**, and
say so: saving a smart collection's own view would otherwise nest a rule inside itself by id, which
a later delete would dangle.

Then each active filter field becomes a criterion. ⚠ `stars`, `flags`, `labels` are **sets**, so
each becomes an `Any([...])` of the selected values — which is precisely why the rule needed `Any`
and the filter bar's checkbox model maps onto it cleanly.

- [ ] **Step 2: The panel section**

A **Smart** section beside Collections, following that file's idiom exactly (heading, separator,
rows with a count, `×` to delete). ⚠ Its rows show **no count**: computing one means evaluating the
rule over every photo, per frame. That is a real difference from Collections and Keywords, and the
row should not pretend otherwise.

A "Save as smart collection" control: a name field plus `+`, enabled only when a Library is open,
storing `rule_from_current`.

⚠ Deleting always confirms? **No** — a smart collection is a query with nothing behind it, so it
follows `delete_collection`'s precedent, not `forget_folder`'s. Say so in a comment.

- [ ] **Step 3: Verify, run and check by hand**

```bash
cargo run --release -p app
```

1. Filter to 3★+ in All Photos, save as "Keepers", and it appears in the panel.
2. Clicking it shows exactly those photos, **from every folder**.
3. Rate a fourth photo up, reopen the smart collection, and it is now included.
4. Browse a date, save it, and the saved rule reproduces that day.
5. Delete a smart collection — the photos are untouched.
6. **Check the real catalog** reaches schema v5 with every row intact.

- [ ] **Step 4: Commit**

---

## Task 5: Docs

- [ ] `CLAUDE.md` — a section covering: **the correction that `LibraryFilter` was not the rule
  representation** and why; that `text` cannot be evaluated in SQL and what that forced; the
  identity elements; absent facts failing; rating being an operator here and a set in the bar; the
  tree-in-storage / flat-in-editor split; and the caveats. Update the test count; next action
  **plan 2 — the editor and multi-select dates**.
- [ ] The Obsidian roadmap — M4.3c plan 1 done.
- [ ] Final `rustup update stable`, verification by **exit code**, commit. **Do not push.**

---

## Definition of done

- `fmt`, `clippy --all-targets --all-features -D warnings`, `test --all` all exit 0.
- **All 47 goldens byte-identical.**
- A smart collection saved from the current view reproduces it, spans folders, and re-evaluates on
  open.
- The real catalog reaches schema v5 with every existing row intact.
