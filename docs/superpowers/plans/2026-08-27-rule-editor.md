# The rule editor and multi-select dates — M4.3c, Plan 2 of 2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close M4.3 — make a rule editable so criteria can be *composed*, and let several dates be browsed at once.

**Architecture:** A flat editor model (`Match all|any|none` over rows of *kind + values*) with a pure, round-trip-tested conversion to and from `Expr`; a modal that edits it; and `LibrarySource::Periods` holding a set.

**Tech Stack:** Rust, `egui`, `rusqlite`. No new dependencies, **no schema change** — plan 1's `smart_collections.rule` is JSON text.

**Spec:** `docs/superpowers/specs/2026-08-27-smart-collections-design.md`

---

## ⚠⚠ A finding that changes the editor's model

Plan 1's `rule_from_current` maps a star **set** to `Any([Is, Is, …])`, so **every rule it saves is
nested**. The real one saved during testing is literally:

```json
{"All":[{"Any":[{"Is":{"Rating":{"op":"Exactly","value":3}}}]}]}
```

A naïvely flat editor — one `Is` per row — would therefore **refuse to open its own output**.

**So a row is (kind, one-or-more values)**, serialising to `Is(c)` for a single value and
`Any([Is…])` for several. That matches the filter bar's multi-select model, makes plan 1's rules
editable, and keeps the stored shape unchanged.

⚠ The editor still expresses a strict subset of `Expr`. A rule with nesting it cannot represent —
hand-written, or from a future editor — **must be refused rather than flattened lossily**: silently
rewriting someone's rule is worse than declining to edit it.

---

## Ground rules for this repo

- **Branch:** `m4.3c-rule-editor`. **Never push** — the user pushes.
- **Commit messages carry no `Co-Authored-By` trailer.**
- **Build env** — every `cargo` command needs this first, in the same shell:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **The GUI binary is `Rustroom`**: bin tests are `cargo test -p app --bin Rustroom`.
- **⚠ In a binary crate an item lands with its first caller** — fifteenth instance.
- **⚠ Run clippy before every commit.** Three commits this session went in with it failing while
  `cargo test` was green; one of them had shipped a feature that could never work.
- **⚠ The sabotage compile check is "did a `test result:` line appear at all"**, not a grep for
  `^error:`.
- ⚠ `clippy --all-targets --all-features` and `test --all` each take **over two minutes** here.

---

## File Structure

| file | responsibility | new? |
|---|---|---|
| `crates/catalog/src/db.rs` | `photos_in_periods` | modify |
| `crates/app/src/rule_form.rs` | the flat editor model, and its conversion to/from `Expr` | **create** |
| `crates/app/src/modals/rule_edit.rs` | the editor dialog | **create** |
| `crates/app/src/main.rs` | `LibrarySource::Periods`; `open_periods`; date selection state | modify |
| `crates/app/src/collections_panel.rs` | shift/⌘-click dates; an edit button on smart rows | modify |

---

## Task 1: Several dates at once

**Files:** Modify `crates/catalog/src/db.rs`

- [ ] **Step 1: Write the failing test**

```rust
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
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17)).unwrap();
        cat.set_capture(Path::new("/a/two.RAF"), &on_day(2026, 7, 9)).unwrap();
        cat.set_capture(Path::new("/a/three.RAF"), &on_day(2025, 1, 1)).unwrap();

        let got = cat
            .photos_in_periods(&[(2026, Some(5), Some(17)), (2026, Some(7), Some(9))])
            .unwrap();
        assert_eq!(got.len(), 2);

        // ⚠ Overlapping periods must not double a photo.
        let overlapping = cat
            .photos_in_periods(&[(2026, Some(5), Some(17)), (2026, Some(5), None)])
            .unwrap();
        assert_eq!(overlapping.len(), 1, "an overlapping period doubled a photo");
    }

    #[test]
    fn an_empty_period_list_matches_nothing() {
        // ⚠ Not "everything": an empty selection is the state the panel is in before any date is
        // clicked, and showing the whole library then would be surprising.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"])).unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &on_day(2026, 5, 17)).unwrap();
        assert!(cat.photos_in_periods(&[]).unwrap().is_empty());
    }
```

- [ ] **Step 2: Implement**

```rust
    /// Every photo taken in **any** of these periods, as `(path, copy)`, de-duplicated.
    ///
    /// ⚠ `photos_in_period` stays as the single-period case rather than being deleted — it is what
    /// a plain click uses, and a one-element slice through this would be the same query with more
    /// machinery.
    ///
    /// ⚠ An **empty** slice matches nothing, not everything: it is the state the panel is in
    /// before any date is clicked.
    pub fn photos_in_periods(
        &self,
        periods: &[(i32, Option<u32>, Option<u32>)],
    ) -> Result<Vec<(PathBuf, u32)>, CatalogError>;
```

Implement by running the existing per-period query once per period and de-duplicating, **or** by
building an `OR`-ed `WHERE`. ⚠ Prefer the loop: a handful of periods times one indexed-ish query is
simpler than assembling SQL, and the de-duplication is explicit rather than resting on `DISTINCT`
over a join.

- [ ] **Step 3: Run, sabotage, commit**

Sabotage: skip the de-duplication → `several_periods_come_back_as_one_set` fails on the overlapping
case.

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A && git commit -m "feat(catalog): query photos across several periods at once"
```

---

## Task 2: Multi-select in the date tree

**Files:** Modify `crates/app/src/main.rs`, `crates/app/src/collections_panel.rs`

- [ ] **Step 1: `LibrarySource::Periods`**

Replace the single `Period` variant:

```rust
    /// Photos taken in any of these calendar periods.
    ///
    /// ⚠ A **set**, because ⌘-clicking several days is how a shoot spanning a weekend is browsed.
    /// A single click is the one-element case, so there is no second code path.
    Periods {
        periods: Vec<(i32, Option<u32>, Option<u32>)>,
        label: String,
    },
```

The compiler names every site: `open_period`, `rule_from_current`, the panel's selection
highlighting, and `reindex`'s message.

⚠ `rule_from_current` maps it to `Any([Period, Period, …])` — which is exactly the shape a
multi-value row takes in Task 3, so the two agree by construction.

- [ ] **Step 2: The panel interaction**

⚠ **egui gives modifiers on the click response**, so read them there rather than from global input:
`resp.clicked()` plus `ui.input(|i| i.modifiers)` in the same frame, the shape `apply_click` uses.

Rules, matching `apply_click`'s so the app has **one** multi-select idiom:

- **plain click** — replace the selection with this period.
- **⌘/Ctrl-click** — toggle this period.
- **Shift-click** — extend from the last-clicked period to this one, **in tree order**.

⚠ Shift-range needs a flattened ordering of the tree. Build it from `date_tree::group` as
`Vec<(i32, Option<u32>, Option<u32>)>` in display order — year row, then each month row, then its
days — so "everything between" means what the eye sees. ⚠ Put that flattening in `date_tree` beside
`group`, **not in the panel**, so it is testable:

```rust
/// Every clickable node of the tree, in display order — the sequence a Shift-range walks.
pub fn flatten(years: &[Year]) -> Vec<(i32, Option<u32>, Option<u32>)>;
```

with a test that a year is followed by its months and each month by its days, and that two years do
not interleave.

- [ ] **Step 3: Highlight every selected period**

The row's `selectable_label` takes `selected` from whether that period is **in the set**, not from
equality with a single value.

- [ ] **Step 4: Verify, sabotage, commit**

Sabotage: make Shift-click behave like a plain click → the range test fails.

```bash
git add -A && git commit -m "feat(app): select several dates at once"
```

---

## Task 3: The editor model

**Files:** Create `crates/app/src/rule_form.rs`; modify `crates/app/src/lib.rs`

- [ ] **Step 1: The types, with `todo!()`, and the tests**

```rust
//! The flat form a rule is edited in, and its conversion to and from [`Expr`].
//!
//! ⚠⚠ **A row is a *kind* plus one-or-more values**, not a single criterion — because
//! `rule_from_current` maps a star set to `Any([Is, Is, …])`, so **every rule plan 1 saves is
//! nested**. A one-criterion-per-row editor would refuse to open its own output.
//!
//! ⚠ The form expresses a strict **subset** of `Expr`. A rule with nesting it cannot represent is
//! **refused**, never flattened lossily: silently rewriting someone's rule is worse than declining
//! to edit it.

use crate::rule::{Criterion, Expr};

/// How the rows combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Match {
    #[default]
    All,
    Any,
    /// `Not(Any([…]))` — "none of the following".
    None,
}

/// One row: a criterion kind, with one or more values of that kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub values: Vec<Criterion>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Form {
    pub match_: Match,
    pub rows: Vec<Row>,
}

impl Form {
    /// Build the expression this form describes.
    pub fn to_expr(&self) -> Expr {
        todo!()
    }

    /// Read an expression into the form, or `None` if it uses nesting the form cannot express.
    pub fn from_expr(e: &Expr) -> Option<Self> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::Cmp;

    fn rating(v: u8) -> Criterion {
        Criterion::Rating {
            op: Cmp::Exactly,
            value: v,
        }
    }

    #[test]
    fn a_rule_saved_from_the_current_view_can_be_edited() {
        // ⚠⚠ The finding this model exists for. `rule_from_current` maps a star **set** to
        // `Any([…])`, so its output is nested — this is the real rule saved during plan 1's
        // testing, and a one-criterion-per-row editor would refuse to open it.
        let saved = Expr::All(vec![Expr::Any(vec![Expr::Is(rating(3))])]);
        let form = Form::from_expr(&saved).expect("plan 1's own output must be editable");
        assert_eq!(form.match_, Match::All);
        assert_eq!(form.rows.len(), 1);
        assert_eq!(form.rows[0].values, vec![rating(3)]);
    }

    #[test]
    fn a_form_round_trips_through_an_expression() {
        let form = Form {
            match_: Match::Any,
            rows: vec![
                Row {
                    values: vec![rating(4), rating(5)],
                },
                Row {
                    values: vec![Criterion::Text("paris".into())],
                },
            ],
        };
        let back = Form::from_expr(&form.to_expr()).expect("must round trip");
        assert_eq!(back, form);
    }

    #[test]
    fn none_becomes_a_negated_any_and_back() {
        let form = Form {
            match_: Match::None,
            rows: vec![Row {
                values: vec![rating(1)],
            }],
        };
        let e = form.to_expr();
        assert!(matches!(e, Expr::Not(_)), "None must negate: {e:?}");
        assert_eq!(Form::from_expr(&e).unwrap(), form);
    }

    #[test]
    fn a_single_value_row_is_a_bare_is_rather_than_a_one_element_any() {
        // ⚠ Keeps the stored JSON readable, and keeps a hand-written `{"Is":…}` editable.
        let form = Form {
            match_: Match::All,
            rows: vec![Row {
                values: vec![rating(3)],
            }],
        };
        assert_eq!(form.to_expr(), Expr::All(vec![Expr::Is(rating(3))]));
    }

    #[test]
    fn nesting_the_form_cannot_express_is_refused_rather_than_flattened() {
        // ⚠⚠ Silently rewriting someone's rule is worse than declining to edit it. A hand-written
        // `All([Any([All([…])])])` has no flat equivalent, and losing it on save would be
        // invisible until the collection stopped matching what it used to.
        let deep = Expr::All(vec![Expr::Any(vec![Expr::All(vec![Expr::Is(rating(3))])])]);
        assert!(Form::from_expr(&deep).is_none());

        let negated_inside = Expr::All(vec![Expr::Not(Box::new(Expr::Is(rating(3))))]);
        assert!(Form::from_expr(&negated_inside).is_none());
    }

    #[test]
    fn an_empty_form_is_all_of_nothing() {
        // ⚠ Which matches **everything**, per `Expr`'s identity elements — the state a new rule
        // starts in, and it must not be an error.
        assert_eq!(Form::default().to_expr(), Expr::All(vec![]));
        assert_eq!(Form::from_expr(&Expr::All(vec![])).unwrap(), Form::default());
    }

    #[test]
    fn a_row_of_mixed_kinds_is_refused() {
        // ⚠ A row is "one kind, several values", so `Any([Rating, Text])` is not a row — it is
        // nesting the form cannot show, and must be refused rather than silently split.
        let mixed = Expr::All(vec![Expr::Any(vec![
            Expr::Is(rating(3)),
            Expr::Is(Criterion::Text("x".into())),
        ])]);
        assert!(Form::from_expr(&mixed).is_none());
    }
}
```

⚠ `a_row_of_mixed_kinds_is_refused` needs a way to compare *kinds* without comparing values. Add
`Criterion::kind(&self) -> &'static str` (or a small `Kind` enum) — ⚠ it is also what the editor's
type dropdown needs, so it lands with a second caller and is not speculative.

- [ ] **Step 2: Run to verify they fail, then implement**

`to_expr`: each row becomes `Is` (one value) or `Any` (several); the list becomes `All`, `Any`, or
`Not(Any(..))`.

`from_expr`: match the top level for `All` / `Any` / `Not(Any(..))`; each child must be `Is(c)` or
`Any([Is, Is, …])` **all of one kind**; anything else returns `None`.

- [ ] **Step 3: Sabotage**

| sabotage | expected |
|---|---|
| `from_expr` flattens unknown nesting instead of returning `None` | the refusal test |
| a one-value row emits `Any([Is])` | the bare-`Is` test |
| `from_expr` ignores the kind check on a multi-value row | the mixed-kinds test |
| `Match::None` maps to `Any` without the `Not` | the none test |

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat(app): a flat, round-trip-safe form for editing a rule"
```

---

## Task 4: The editor dialog

**Files:** Create `crates/app/src/modals/rule_edit.rs`; modify `crates/app/src/modals/mod.rs`, `crates/app/src/collections_panel.rs`

- [ ] **Step 1: The dialog**

`Modal::EditRule { id: i64, name: String, form: Form, error: Option<String> }`, opened from a
pencil button on a smart-collection row.

⚠ The form is **loaded once when the dialog opens**, like the Information dialog's dimensions and
the export dialog's draft — not rebuilt per frame, or every keystroke would be discarded.

⚠ **If `Form::from_expr` returns `None`, the dialog says so and offers no editing** rather than
showing an empty form: an empty form that silently replaced a working rule on Save is the exact
data loss the refusal exists to prevent.

Layout: `Match [All|Any|None] of:` then one row per criterion, each with a **kind dropdown**, a
value widget, and a `×`; then `+ Add criterion`, `Save`, `Cancel`.

- [ ] **Step 2: Six value widgets, not fifteen**

⚠ The fifteen criterion kinds need only **six** widgets, which is what makes this task finite:

| widget | criteria |
|---|---|
| **operator + number** | `Rating`, `Iso`, `Aperture`, `Shutter`, `FocalLength`, `FileSize` |
| **dropdown of known strings** | `Camera`, `Lens`, `Format` |
| **dropdown of named ids** | `Collection`, `Keyword` |
| **dropdown of catalogued folders** | `Folder` |
| **free text** | `Text` |
| **enum dropdown** | `Flag`, `Label` |
| **year / month / day** | `Period` |

⚠ The string and id dropdowns are built from the catalog (`distinct_cameras`, `distinct_lenses`,
`collections`, `keywords`, `folders`) — the same lists the Filter popup and panel already use, so a
value that cannot exist cannot be chosen.

⚠ **A multi-value row is the same widget repeated**, with its own `+`. That is what keeps
`Any([Is, Is])` reachable from the UI rather than only from plan 1's capture.

- [ ] **Step 3: Save**

`form.to_expr()` → `serde_json::to_string` → `Catalog::set_smart_rule`. ⚠ Then **reopen the
collection if it is the one on screen**, or the grid keeps showing the old rule's results with no
sign that anything changed.

- [ ] **Step 4: Verify, run, and check by hand**

```bash
cargo run --release -p app
```

1. Edit **Keepers** (the real saved rule) — it opens without complaint, showing one Rating row.
   ⚠ **This is the finding's acceptance test**; if it refuses, the row model is wrong.
2. Add a `Collection` row, save, and the collection now matches the *intersection* — **the
   composition M4.3 exists for**.
3. Add a second value to a row and confirm it behaves as "either".
4. `Match: none` inverts the result.
5. Editing the open smart collection refreshes the grid.
6. Hand-edit a rule in the database into something nested, reopen the editor, and it **declines**
   rather than showing an empty form.

- [ ] **Step 5: Commit**

---

## Task 5: Docs

- [ ] `CLAUDE.md` — a section covering: **why a row is kind-plus-values** (plan 1's own output is
  nested); refusing rather than flattening; six widgets for fifteen criteria; the shared
  multi-select idiom with `apply_click`; and the caveats. Update the test count. **Record that
  M4.3 — and with it Phase 4's catalog half — is complete**, and set the next action to the agreed
  item 3: **persist undo history**.
- [ ] The Obsidian roadmap — M4.3 ✅ and Phase 4 ✅.
- [ ] Final `rustup update stable`, verification by **exit code**, commit. **Do not push.**

---

## Definition of done

- `fmt`, `clippy --all-targets --all-features -D warnings`, `test --all` all exit 0.
- **All 47 goldens byte-identical.**
- The rule saved during plan 1's testing opens in the editor unchanged.
- A rule combining a collection *and* a date matches their intersection — **the composition the
  milestone exists for**.
- ⌘- and Shift-clicking dates browses several at once, with every selected row highlighted.
