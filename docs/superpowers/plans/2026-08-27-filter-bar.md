# The filter bar — M4.3a-ii, Plan 2 of 2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Narrow whatever the Library is showing by name, rating, flag and colour label — which, now that All Photos exists, is a library-wide search.

**Architecture:** One `LibraryFilter` in the `rustroom_app` lib holds every filter including the existing `FormatFilter`, with a single `matches`. `Library::refilter` becomes one call. The UI is a search field and one Filter popup.

**Tech Stack:** Rust, `egui`/`eframe` 0.35. No new dependencies, no schema change, no catalog change.

**Spec:** `docs/superpowers/specs/2026-08-27-all-photos-and-filtering-design.md`

**Plan 1 (sources) is complete** — All Photos, the Folders panel and Forget-folder shipped 2026-08-27.

---

## ⚠ A layout correction to the spec, made after reading the toolbar

The spec says the bar goes *"in the Library toolbar, right of the existing sort and grid-size
controls and left of the format chips."* **That was written before reading `shell.rs:143`**, which
is a single `Layout::right_to_left` cluster already carrying format bullets, the star combo, the
grid-size buttons, the sort combo, Reindex *and* the status line. A text field needs ~180 px there
and would squeeze the status out — the exact bug that cluster was created to fix, where the develop
toolbar right-aligned the filename and the status label was then added to a row with no space left.

So:

- **The search field goes in the LEFT part of the row**, after the view switch — the space the
  Develop toolbar occupies in Develop, and which is **empty in Library**. It has room there.
- **Stars, flag and label become one `Filter` popup** in the right cluster, *replacing* the star
  combo rather than joining it. The row's control count stays at five while gaining two
  capabilities, and the button's label reports the active state so the filter is never invisible.

⚠ The cost, stated: the star filter goes from one click to two. It is the most-used of the three,
so this is a real trade. It buys a row that does not overflow, and one home for "narrow what I am
looking at" rather than three controls scattered along a cluster.

---

## Ground rules for this repo

- **Branch:** `m4.3a-ii-filter-bar`. **Never push** — the user pushes.
- **Commit messages carry no `Co-Authored-By` trailer.**
- **Build env** — every `cargo` command needs this first, in the same shell:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **The GUI binary is `Rustroom`**: bin tests are `cargo test -p app --bin Rustroom`.
- **⚠ In a binary crate an item lands with its first caller** — tenth instance of that trap.
- **⚠ Check exit codes, never grep output.**
- **⚠ The sabotage compile check is "did a `test result:` line appear at all"** — not a grep for
  `^error:`, which matches cargo's ordinary test-failure line.
- ⚠ `cargo clippy --all-targets --all-features` and `cargo test --all` each take over two minutes
  here; give them a generous timeout rather than assuming a hang.

---

## File Structure

| file | responsibility | new? |
|---|---|---|
| `crates/app/src/library_filter.rs` | `LibraryFilter`: every filter in one place, one `matches` | **create** |
| `crates/app/src/lib.rs` | declare it | modify |
| `crates/app/src/main.rs` | `Library` swaps `filter` + `min_stars` for one `LibraryFilter`; `refilter` becomes one call | modify |
| `crates/app/src/shell.rs` | the search field and the Filter popup | modify |
| `crates/app/src/test_app.rs` | the `Library` fixture follows the field change | modify |

---

## Task 1: `LibraryFilter`

**Files:**
- Create: `crates/app/src/library_filter.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the file with `todo!()` bodies and the tests**

```rust
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
    pub fn matches(&self, entry: &PhotoEntry, marks: Marks) -> bool {
        todo!()
    }

    /// True when this filter hides anything at all.
    pub fn is_active(&self) -> bool {
        todo!()
    }

    /// Everything except the format filter, which is reset separately on a folder load.
    pub fn clear_marks_and_text(&mut self) {
        todo!()
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
        let mut f = LibraryFilter { text: "dscf".into(), ..Default::default() };
        assert!(f.matches(&entry("DSCF6223.RAF", 0, None), Marks::default()));
        assert!(!f.matches(&entry("IMG_0001.JPG", 0, None), Marks::default()));
        f.text = "6223".into();
        assert!(f.matches(&entry("DSCF6223.RAF", 0, None), Marks::default()));
    }

    #[test]
    fn text_matches_a_copys_own_name_not_the_files() {
        // ⚠⚠ Matches `entries::shown_name`, the one rule the Library caption, the rename field and
        // the default export name already share. Matching `file_name` instead would mean
        // searching for what a copy is *called* and not finding it — precisely the bug that made
        // copy names invisible in the grid, where the Library was the last consumer bypassing the
        // shared rule.
        let warm = entry("DSCF6223.RAF", 2, Some("Warm.RAF"));
        let f = LibraryFilter { text: "warm".into(), ..Default::default() };
        assert!(f.matches(&warm, Marks::default()), "a copy's own name must match");

        // And the file's name must NOT match a named copy, since that is not what it is called.
        let g = LibraryFilter { text: "dscf".into(), ..Default::default() };
        assert!(!g.matches(&warm, Marks::default()));
    }

    #[test]
    fn an_unnamed_copys_derived_name_matches() {
        // An unnamed copy is shown as `{stem}_{copy}.{ext}` — derived, never stored.
        let f = LibraryFilter { text: "_1".into(), ..Default::default() };
        assert!(f.matches(&entry("DSCF6223.RAF", 1, None), Marks::default()));
    }

    #[test]
    fn stars_filter_on_a_minimum() {
        let f = LibraryFilter { min_stars: 3, ..Default::default() };
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
        let unflagged = LibraryFilter { flag: Some(Flag::None), ..Default::default() };
        assert!(unflagged.matches(&e, marks(0, Flag::None, 0)));
        assert!(!unflagged.matches(&e, marks(0, Flag::Pick, 0)));

        let any = LibraryFilter::default();
        assert!(any.matches(&e, marks(0, Flag::Pick, 0)));
        assert!(any.matches(&e, marks(0, Flag::None, 0)));
    }

    #[test]
    fn some_label_zero_means_unlabelled_rather_than_any() {
        let e = entry("a.RAF", 0, None);
        let unlabelled = LibraryFilter { label: Some(0), ..Default::default() };
        assert!(unlabelled.matches(&e, marks(0, Flag::None, 0)));
        assert!(!unlabelled.matches(&e, marks(0, Flag::None, 2)));

        let red = LibraryFilter { label: Some(1), ..Default::default() };
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
            ("text", LibraryFilter { text: "nope".into(), ..pass.clone() }),
            ("stars", LibraryFilter { min_stars: 5, ..pass.clone() }),
            ("flag", LibraryFilter { flag: Some(Flag::Reject), ..pass.clone() }),
            ("label", LibraryFilter { label: Some(3), ..pass.clone() }),
        ] {
            assert!(!f.matches(&e, m), "{label} did not narrow");
        }
    }

    #[test]
    fn is_active_reports_every_field() {
        // A filter that hides something while reporting itself inactive would leave the UI with no
        // way to say so — the "invisible state" the crop aspect lock was fixed for.
        assert!(LibraryFilter { text: "a".into(), ..Default::default() }.is_active());
        assert!(LibraryFilter { min_stars: 1, ..Default::default() }.is_active());
        assert!(LibraryFilter { flag: Some(Flag::None), ..Default::default() }.is_active());
        assert!(LibraryFilter { label: Some(0), ..Default::default() }.is_active());
        assert!(!LibraryFilter::default().is_active());
    }

    #[test]
    fn clearing_leaves_the_format_filter_alone() {
        // ⚠ The format filter is reset by *loading a folder*, not by clearing the search — they
        // answer different questions and a Clear button must not silently unhide a format.
        let mut f = LibraryFilter { text: "a".into(), min_stars: 3, ..Default::default() };
        f.format.toggle("jpg");
        f.clear_marks_and_text();
        assert!(!f.is_active() || f.format.is_hidden("jpg"));
        assert!(f.format.is_hidden("jpg"), "the format filter was cleared too");
        assert!(f.text.is_empty() && f.min_stars == 0);
    }
}
```

⚠ **Check every borrowed name before running**: `PhotoEntry`'s fields, `PhotoId::new`, `Marks`'
fields, `Flag`'s variants, `FormatFilter::toggle`/`is_hidden`, and whether `Marks` is `Copy`. Fix
the *test* to match reality — do not change production types to suit a sketch.

Add to `crates/app/src/lib.rs` in alphabetical order:

```rust
pub mod library_filter;
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib library_filter 2>&1 | tail -20
```

Expected: every test panics at `not yet implemented` (after any name fixes compile).

- [ ] **Step 3: Write the implementation**

```rust
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
            // matching that would mean searching for what a copy is *called* and not finding it.
            let shown = entries::shown_name(&entry.file_name, entry.id.copy, entry.name.as_deref());
            if !shown.to_lowercase().contains(&self.text.to_lowercase()) {
                return false;
            }
        }
        true
    }

    pub fn is_active(&self) -> bool {
        self.format.is_active()
            || !self.text.is_empty()
            || self.min_stars > 0
            || self.flag.is_some()
            || self.label.is_some()
    }

    /// Clear the marks and the text, leaving the format filter alone.
    ///
    /// ⚠ The format filter answers a different question and is reset by *loading a folder*. A
    /// Clear button that silently unhid a format would be doing something the user did not ask
    /// for.
    pub fn clear_marks_and_text(&mut self) {
        self.text.clear();
        self.min_stars = 0;
        self.flag = None;
        self.label = None;
    }
```

⚠ `FormatFilter` may expose `is_visible(&Path)` rather than `is_hidden(&str)`, and may have no
`is_active`. **Read the module and use what is there**, adding `is_active` to it only if it is
genuinely absent — in which case add it *with* this caller, per the dead-code rule.

- [ ] **Step 4: Run to verify they pass**

```bash
cargo test -p app --lib library_filter 2>&1 | grep -E "^test result:"
```

Expected: 10 pass.

- [ ] **Step 5: Sabotage each field and confirm one test fails per field**

For each of the four criteria in `matches`, delete its early return, run, and note which test
fails. Confirm a `test result:` line appears each time before believing the outcome.

| sabotage | expected |
|---|---|
| drop the `min_stars` check | `stars_filter_on_a_minimum` and `every_field_narrows…` |
| drop the `flag` check | `some_flag_none_means_unflagged…` and `every_field_narrows…` |
| drop the `label` check | `some_label_zero_means_unlabelled…` and `every_field_narrows…` |
| match on `entry.file_name` instead of `shown_name` | `text_matches_a_copys_own_name_not_the_files` |

Restore after each and confirm green.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/library_filter.rs crates/app/src/lib.rs
git commit -m "feat(app): one filter over name, rating, flag, label and format"
```

---

## Task 2: `Library` holds one filter

**Files:**
- Modify: `crates/app/src/main.rs`
- Modify: `crates/app/src/test_app.rs`

- [ ] **Step 1: Swap the fields**

In `struct Library`, replace:

```rust
    /// Which formats the browser hides. Reset when a folder is loaded.
    filter: FormatFilter,
```
…and…
```rust
    /// Minimum star rating shown. 0 shows everything.
    min_stars: u8,
```

with the single:

```rust
    /// Everything that narrows what is shown. Reset when a source is loaded.
    filter: rustroom_app::library_filter::LibraryFilter,
```

- [ ] **Step 2: Collapse `refilter`**

```rust
    /// Recompute `visible` and prune the selection to it.
    ///
    /// ⚠ Both halves, always. Recomputing the visible list without pruning leaves the selection
    /// holding photos that are not on screen, which a sync or a paste would then write to.
    fn refilter(&mut self) {
        // ⚠ One derived `visible` list, composed from one filter — **not** a second filtering
        // mechanism. Sparse-versus-dense index confusion is this project's third-most-repeated
        // bug, and `visible` staying the single derived list is the mitigation that has held.
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let m = self.marks.get(&e.id).copied().unwrap_or_default();
                self.filter.matches(e, m)
            })
            .map(|(i, _)| i)
            .collect();
        self.selection.retain_visible(&self.visible);
    }
```

- [ ] **Step 3: Fix what the compiler names**

```bash
cargo build -p app 2>&1 | grep -E "^error" -A 6 | head -40
```

Expected sites: the `Library` literal in `open_scan`, the fixture in `test_app.rs`, and
`shell.rs`'s `star_filter` and `format_bullets`. In `open_scan` the literal becomes
`filter: Default::default(),` and the `min_stars: 0,` line goes.

⚠ In `shell.rs`, `format_bullets` reaches `lib.filter.is_hidden(ext)` / `lib.filter.toggle(&ext)`
— those become `lib.filter.format.…`. Leave its behaviour otherwise untouched; Task 3 handles
`star_filter`.

- [ ] **Step 4: Verify**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: green, and **the test count is unchanged from Task 1** — this is a representation
change with no new behaviour. Commit:

```bash
git add -A
git commit -m "refactor(app): the Library carries one filter instead of two"
```

---

## Task 3: The search field and the Filter popup

**Files:**
- Modify: `crates/app/src/shell.rs`

- [ ] **Step 1: Replace `star_filter` with the Filter popup**

Delete `fn star_filter` and add, keeping its doc comment's reasoning about composing through
`refilter`:

```rust
/// Rating, flag and colour-label filtering, in one popup.
///
/// ⚠ **One control rather than three.** The row it sits in is a right-to-left cluster already
/// carrying the format bullets, the grid-size buttons, the sort combo, Reindex and the status
/// line; three more controls would squeeze the status out — the exact bug that cluster was
/// created to fix.
///
/// ⚠ The button reports the active state, so a filter is never invisible. A control that hides
/// rows while looking idle is the "invisible state" problem the crop aspect lock was fixed for.
///
/// ⚠ Composes with the format filter and the search text through `Library::refilter`, the single
/// derived `visible` list every consumer iterates — not a second filtering mechanism.
fn mark_filter(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let Some(before) = app.library.as_ref().map(|l| l.filter.clone()) else {
        return;
    };
    let mut f = before.clone();

    let label = mark_filter_label(&f);
    ui.menu_button(label, |ui| {
        ui.label("Rating");
        for v in 0..=5u8 {
            let text = if v == 0 { "All".to_string() } else { format!("{v}★+") };
            ui.selectable_value(&mut f.min_stars, v, text);
        }
        ui.separator();
        ui.label("Flag");
        ui.selectable_value(&mut f.flag, None, "Any");
        ui.selectable_value(&mut f.flag, Some(catalog::db::Flag::Pick), "⚑ Picked");
        ui.selectable_value(&mut f.flag, Some(catalog::db::Flag::Reject), "× Rejected");
        ui.selectable_value(&mut f.flag, Some(catalog::db::Flag::None), "Unflagged");
        ui.separator();
        ui.label("Colour");
        ui.selectable_value(&mut f.label, None, "Any");
        ui.selectable_value(&mut f.label, Some(0), "Unlabelled");
        for (n, name) in [(1u8, "Red"), (2, "Blue"), (3, "Yellow"), (4, "Green")] {
            ui.selectable_value(&mut f.label, Some(n), name);
        }
        ui.separator();
        if ui.button("Clear").clicked() {
            f.clear_marks_and_text();
        }
    });

    // ⚠ Guarded on an actual change: this body runs every frame, and refiltering a folder of
    // hundreds per frame is the `apply_view_demosaic` mistake in another costume.
    if f != before {
        if let Some(lib) = app.library.as_mut() {
            lib.filter = f;
            lib.refilter();
        }
    }
}

/// What the Filter button says. ⚠ Pure, so the "an active filter is visible" rule is testable
/// without a window.
fn mark_filter_label(f: &rustroom_app::library_filter::LibraryFilter) -> String {
    let mut parts: Vec<String> = Vec::new();
    if f.min_stars > 0 {
        parts.push(format!("{}★+", f.min_stars));
    }
    match f.flag {
        Some(catalog::db::Flag::Pick) => parts.push("⚑".into()),
        Some(catalog::db::Flag::Reject) => parts.push("×".into()),
        Some(catalog::db::Flag::None) => parts.push("unflagged".into()),
        None => {}
    }
    if let Some(n) = f.label {
        parts.push(if n == 0 {
            "unlabelled".into()
        } else {
            format!("colour {n}")
        });
    }
    if parts.is_empty() {
        "Filter".to_string()
    } else {
        parts.join(" ")
    }
}
```

⚠ **`⚑` and `×` are the glyphs M4.2a already probed and shipped** — CLAUDE.md records that `✓` and
`✕` are in *no* bundled font and would have shipped as tofu. Do not substitute others without
probing. `★` is likewise already in use.

⚠ Update the call site at `shell.rs:150` from `star_filter(self, ui);` to `mark_filter(self, ui);`.

- [ ] **Step 2: Add the search field on the left of the row**

In the `ui.horizontal(|ui| {` block, after the `view_switch` / Develop-toolbar branch and
**before** the `with_layout(right_to_left…)` cluster:

```rust
                // ⚠ On the LEFT, not in the right-aligned cluster. A text field needs ~180 px and
                // that cluster already carries five controls plus the status line; adding it there
                // squeezes the status out, which is the exact bug the cluster was created to fix.
                // This space is where the Develop toolbar sits, and it is empty in Library.
                if self.view == AppView::Library && self.library.is_some() {
                    ui.separator();
                    search_field(self, ui);
                }
```

and the function:

```rust
/// Narrow the Library by name.
///
/// ⚠ Matches `entries::shown_name`, so what you search is what the caption shows — including a
/// copy's own name.
///
/// ⚠ **Needs no new keyboard work, and that is checked rather than assumed.** The Library's bare
/// culling keys (`0`–`9`, `p`, `x`, `u`), its arrow navigation and `pressed_command` are all
/// already gated on `memory().focused()`, which is exactly the gate a text field needs. Escape is
/// added so the keyboard is recoverable without the mouse.
fn search_field(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let Some(before) = app.library.as_ref().map(|l| l.filter.text.clone()) else {
        return;
    };
    let mut text = before.clone();
    let r = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text("Search")
            .desired_width(170.0),
    );
    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        r.surrender_focus();
    }
    // ⚠ Guarded on a change: the body runs every frame.
    if text != before {
        if let Some(lib) = app.library.as_mut() {
            lib.filter.text = text;
            lib.refilter();
        }
    }
}
```

⚠ **Read the key press into a local *before* touching `r`** if the borrow checker or a panic
demands it — `Response` methods take the context lock internally, and holding one across
`ui.input(..)` panics inside epaint's mutex. This project has paid for that once already, in
`name_edit::field`. If `surrender_focus` misbehaves, follow that file's pattern exactly.

- [ ] **Step 3: Write the label test**

In `shell.rs`'s test module (create one if absent, following `library.rs`'s):

```rust
    #[test]
    fn the_filter_button_reports_an_active_filter() {
        // ⚠ A control that hides rows while looking idle is invisible state — the problem the crop
        // aspect lock was fixed for. The label is the only thing that says a filter is on.
        use rustroom_app::library_filter::LibraryFilter;
        assert_eq!(mark_filter_label(&LibraryFilter::default()), "Filter");
        assert_ne!(
            mark_filter_label(&LibraryFilter { min_stars: 3, ..Default::default() }),
            "Filter"
        );
        assert_ne!(
            mark_filter_label(&LibraryFilter {
                flag: Some(catalog::db::Flag::None),
                ..Default::default()
            }),
            "Filter"
        );
        assert_ne!(
            mark_filter_label(&LibraryFilter { label: Some(0), ..Default::default() }),
            "Filter"
        );
    }

    #[test]
    fn every_filter_glyph_is_renderable() {
        // ⚠⚠ M4.2a found `✓` and `✕` are in NO bundled font and would have shipped as tofu boxes;
        // the rail's four glyphs were all absent too. Probe, never assume.
        let ctx = crate::test_ui::ctx();
        let _ = ctx.run_ui(Default::default(), |_| {});
        for c in ['★', '⚑', '×'] {
            assert!(
                ctx.fonts(|f| f.has_glyph(c)),
                "{c} would render as a tofu box"
            );
        }
    }
```

⚠ `Fonts::has_glyph` needs a frame to have run — a fresh context has not built its atlas, so the
empty `run_ui` above is load-bearing. ⚠ And the real app installs an icon font via
`theme::install_fonts` (or similar); if the test context does not, install it here the way
`icons`' own coverage tests do — **read them and copy the setup**, since a test asking egui's bare
defaults answers about a font stack production does not use.

- [ ] **Step 4: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A
git commit -m "feat(app): search and mark filtering in the Library toolbar"
```

---

## Task 4: Headless check, sabotage and docs

**Files:**
- Modify: `crates/app/src/library.rs` (test module), `CLAUDE.md`, the spec, the Obsidian roadmap

- [ ] **Step 1: Write the selection-pruning test**

```rust
    #[test]
    fn a_filter_that_hides_a_selected_photo_prunes_the_selection() {
        // ⚠⚠ The hazard `retain_visible` exists for, and the reason `refilter` always does BOTH
        // halves. Select photos, then filter them away, and the selection would still hold them —
        // so a sync or a paste would write to photos that are not on screen. Silent, and
        // discovered as data loss rather than as a bug.
        let mut lib = crate::test_app::library_of(4);
        lib.selection.select_indices(&[0, 1, 2, 3]);
        assert_eq!(lib.selection.len(), 4);
        lib.filter.text = "nothing matches this".into();
        lib.refilter();
        assert!(lib.visible.is_empty(), "the filter did not hide anything");
        assert_eq!(
            lib.selection.len(),
            0,
            "the selection still holds photos that are not on screen"
        );
    }
```

⚠ Check `Selection`'s API — `select_indices` and `len` are the names used elsewhere in this
codebase, but confirm rather than assume. `library_of` builds entries named
`rustroom_00000.RAF`, so choose search text that genuinely matches none of them.

- [ ] **Step 2: Sabotage the pruning**

Remove `self.selection.retain_visible(&self.visible);` from `refilter`. Confirm a `test result:`
line appears, then that the test above fails. Restore.

- [ ] **Step 3: Verify, run the app, and record**

```bash
cargo run --release -p app
```

Check by hand — the layout is not test-covered:

1. The **Search** field appears left of the status area in Library only, and typing narrows the
   grid as you type.
2. Searching finds a **named copy by its own name**, and does not find it by the file's name.
3. The **Filter** button opens the popup; picking 3★+ narrows the grid and **the button's label
   changes to say so**.
4. **Clear** empties the search and the marks but **leaves the format bullets alone**.
5. Typing in the search field does **not** rate photos or jump to Develop — the culling keys and
   `pressed_command` stay gated on focus. **Escape leaves the field.**
6. Switching source (a folder, then All Photos) **resets the filter**.

⚠ Item 6 is worth a specific look: `open_scan` builds a fresh `Library`, so the reset is by
construction — confirm it actually holds rather than trusting the comment.

- [ ] **Step 4: Update the docs**

- Spec header → `**Status:** built 2026-08-27`.
- `CLAUDE.md` — a section in house style covering: one filter instead of two fields; `shown_name`
  matching and why; `Some(Flag::None)`/`Some(0)` being meaningful; the layout correction and its
  cost (the star filter going from one click to two); the glyph probe; the focus gate needing no
  new work; and the caveats. Update the test count and set the next action to **M4.3b — capture
  metadata**, noting M4.3a is then complete.
- The Obsidian roadmap — mark M4.3a-ii complete.

- [ ] **Step 5: Final verification and commit**

```bash
rustup update stable
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
echo "exit=$?"
```

⚠ Check the exit code, not the output. Commit. **Do not push.**

---

## Definition of done

- `fmt`, `clippy --all-targets --all-features -D warnings`, `test --all` all exit 0.
- **All 47 goldens byte-identical** — nothing here touches `pipeline`.
- Searching narrows the grid, finds a named copy by its own name, and composes with the format
  bullets and the mark filters through one `visible` list.
- A filter that hides a selected photo prunes the selection, proved by the Task 4 sabotage.
- The Filter button never looks idle while a filter is on.
