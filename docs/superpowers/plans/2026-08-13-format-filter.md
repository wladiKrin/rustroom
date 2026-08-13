# Filtering the Library by file format — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A toggle per file format present in the folder, so a RAW+JPEG shoot can be browsed one format at a time.

**Architecture:** `Library.entries` stays complete; a `FormatFilter` derives `visible: Vec<usize>` that the grid, filmstrip and arrow navigation iterate. Toggling recomputes a few hundred `usize`s — no thumbnail reload, no loader involvement. The selection is pruned to visible entries on every filter change.

**Tech Stack:** Rust, `egui`, `cargo test` / `cargo clippy --all-targets --all-features` / `cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-08-13-format-filter-design.md`

**Build setup — every `cargo` command below assumes this has been exported:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

---

## ⚠ One correction to the spec, found while reading `Selection`

The spec says a pruned active photo should move to another surviving member. **`Selection::clamp`
already handles the same situation differently** — it clears the whole set, on the stated
grounds that "targets with no source are not a usable state".

`retain_visible` follows `clamp`. Two different rules for "the active index went away" is a
smell, and silently *moving* the active photo changes which photo a subsequent sync copies
**from** — a change the user would have no reason to notice. Clearing is visible and recoverable.

The spec is amended to match in Task 4.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/app/src/format_filter.rs` | **new** — `formats_present`, `FormatFilter`, `step` | Create |
| `crates/app/src/lib.rs` | export the module | Modify (1 line) |
| `crates/app/src/selection.rs` | `retain_visible` | Modify |
| `crates/app/src/main.rs` | `Library.filter`/`visible`, toolbar chips, iterate `visible` | Modify |
| `CLAUDE.md` | durable status | Modify |

---

## Task 1: `app::format_filter`

**Files:**
- Create: `crates/app/src/format_filter.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Create the module with its tests**

Create `crates/app/src/format_filter.rs`:

```rust
//! Which file formats the Library browser shows.
//!
//! Pure and `egui`-free for the same reason `view_region` and `selection` are: the index
//! bookkeeping is where an off-by-one hides, and a wrong answer here is a photo that silently
//! is not there.

use catalog::CatalogEntry;
use std::collections::BTreeSet;

/// The lowercase extension of an entry, or `None` when it has none.
fn ext_of(entry: &CatalogEntry) -> Option<String> {
    entry
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Extensions present in a folder, lowercased, each with how many entries it has.
///
/// Sorted by extension, so the toggles do not reorder between scans of the same folder.
pub fn formats_present(entries: &[CatalogEntry]) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for e in entries {
        if let Some(ext) = ext_of(e) {
            *counts.entry(ext).or_default() += 1;
        }
    }
    counts.into_iter().collect()
}

/// Which formats are hidden from the browser.
///
/// ⚠ It stores what is **hidden**, not what is shown. With a shown-set, a format that appeared
/// after a folder reload — a new file type dropped into the directory — would be absent from
/// the set and therefore silently invisible. Hidden-by-exception makes "everything is visible"
/// the default and needs no maintenance when the folder changes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FormatFilter {
    hidden: BTreeSet<String>,
}

impl FormatFilter {
    pub fn is_hidden(&self, ext: &str) -> bool {
        self.hidden.contains(ext)
    }

    pub fn toggle(&mut self, ext: &str) {
        if !self.hidden.remove(ext) {
            self.hidden.insert(ext.to_string());
        }
    }

    /// An entry with no extension is always visible: it cannot be filtered by a control that
    /// does not exist for it.
    pub fn is_visible(&self, entry: &CatalogEntry) -> bool {
        match ext_of(entry) {
            Some(ext) => !self.hidden.contains(&ext),
            None => true,
        }
    }

    /// Indices of the visible entries, ascending — what the grid, the filmstrip and arrow
    /// navigation iterate in place of `0..entries.len()`.
    pub fn visible_indices(&self, entries: &[CatalogEntry]) -> Vec<usize> {
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.is_visible(e))
            .map(|(i, _)| i)
            .collect()
    }
}

/// The neighbour of `cur` within `visible`, one step in the given direction, clamped at both
/// ends. `None` only when nothing is visible.
///
/// ⚠ `cur` need not itself be visible. The photo open in Develop is not closed when its format
/// is hidden, so arrow navigation has to start from an index that may no longer be in the list
/// — from which the nearest visible one in the direction of travel is the useful answer.
pub fn step(visible: &[usize], cur: usize, forward: bool) -> Option<usize> {
    if visible.is_empty() {
        return None;
    }
    let last = visible.len() - 1;
    let pos = match visible.binary_search(&cur) {
        // `cur` is visible: move off it.
        Ok(p) if forward => (p + 1).min(last),
        Ok(p) => p.saturating_sub(1),
        // `cur` is hidden: `p` is how many visible indices sit below it, so `p` is already the
        // next one above and `p - 1` the previous one below.
        Err(p) if forward => p.min(last),
        Err(p) => p.saturating_sub(1),
    };
    Some(visible[pos])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entries(names: &[&str]) -> Vec<CatalogEntry> {
        names
            .iter()
            .map(|n| CatalogEntry {
                path: PathBuf::from(format!("/f/{n}")),
                file_name: n.to_string(),
            })
            .collect()
    }

    fn folder() -> Vec<CatalogEntry> {
        entries(&["a.RAF", "a.JPG", "b.raf", "b.jpg", "c.raf", "notes"])
    }

    #[test]
    fn formats_present_counts_each_extension() {
        // Lowercased and deduped, so `.RAF` and `.raf` are one toggle; sorted so the chips do
        // not reorder between scans; an extensionless file contributes no toggle.
        assert_eq!(
            formats_present(&folder()),
            vec![("jpg".to_string(), 2), ("raf".to_string(), 3)]
        );
    }

    #[test]
    fn hiding_a_format_removes_only_its_entries() {
        let f = folder();
        let mut filter = FormatFilter::default();
        filter.toggle("raf");
        // Indices into the *original* list, ascending — including the extensionless entry.
        assert_eq!(filter.visible_indices(&f), vec![1, 3, 5]);
        assert!(filter.is_hidden("raf"));
    }

    #[test]
    fn an_unknown_extension_defaults_to_visible() {
        // The hidden-not-shown choice, as a property: a format the filter has never seen — a
        // file type dropped into the folder after the scan — must still be listed.
        let f = entries(&["x.png", "y.raf"]);
        let mut filter = FormatFilter::default();
        filter.toggle("raf");
        assert_eq!(filter.visible_indices(&f), vec![0]);
        assert!(!filter.is_hidden("png"));
    }

    #[test]
    fn toggling_twice_restores_the_folder() {
        let f = folder();
        let all: Vec<usize> = (0..f.len()).collect();
        let mut filter = FormatFilter::default();
        assert_eq!(filter.visible_indices(&f), all);
        filter.toggle("jpg");
        assert_ne!(filter.visible_indices(&f), all);
        filter.toggle("jpg");
        assert_eq!(filter.visible_indices(&f), all);
    }

    #[test]
    fn stepping_moves_within_the_visible_list() {
        let visible = vec![1, 3, 5];
        assert_eq!(step(&visible, 1, true), Some(3));
        assert_eq!(step(&visible, 3, false), Some(1));
        // Clamped at both ends rather than wrapping.
        assert_eq!(step(&visible, 5, true), Some(5));
        assert_eq!(step(&visible, 1, false), Some(1));
        assert_eq!(step(&[], 0, true), None);
    }

    #[test]
    fn stepping_from_a_hidden_index_lands_on_a_neighbour() {
        // ⚠ The case that exists because Develop keeps the open photo when its format is
        // hidden: `cur` is not in `visible`.
        let visible = vec![1, 3, 5];
        assert_eq!(step(&visible, 2, true), Some(3), "next above");
        assert_eq!(step(&visible, 2, false), Some(1), "previous below");
        assert_eq!(step(&visible, 0, false), Some(1), "clamped at the start");
        assert_eq!(step(&visible, 9, true), Some(5), "clamped at the end");
    }
}
```

- [ ] **Step 2: Export it**

In `crates/app/src/lib.rs`, add in alphabetical position:

```rust
pub mod format_filter;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p app format_filter 2>&1 | grep -E "^test |^test result" | head -10`

Expected: PASS, 6 tests.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/format_filter.rs crates/app/src/lib.rs
git commit -m "feat(app): a file-format filter for the Library browser"
```

---

## Task 2: `Selection::retain_visible`

**Files:**
- Modify: `crates/app/src/selection.rs`

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/app/src/selection.rs`:

```rust
    #[test]
    fn pruning_drops_hidden_entries_from_the_selection() {
        // ⚠ The data-loss guard. Select photos, hide their format, and without this the
        // selection still holds them — so a sync or a paste writes to photos that are not on
        // screen. Silent, and found as data loss rather than as a bug.
        let mut s = Selection::default();
        s.click(1);
        s.toggle(3);
        s.toggle(5);
        s.retain_visible(&[1, 3]);
        check(&s);
        assert_eq!(items(&s), vec![1, 3]);
        assert_eq!(s.active(), Some(1), "a surviving active photo stays put");
    }

    #[test]
    fn pruning_the_active_photo_clears_the_selection() {
        // Follows `clamp`: an active photo that goes away takes the selection with it, rather
        // than silently moving to a different photo — which would change which photo a
        // subsequent sync copies *from*, with nothing on screen to say so.
        let mut s = Selection::default();
        s.click(5);
        s.toggle(1);
        s.retain_visible(&[1, 3]);
        check(&s);
        assert_eq!(s.active(), None);
        assert!(s.is_empty(), "targets with no source are not a usable state");
    }

    #[test]
    fn pruning_everything_empties_the_selection() {
        let mut s = Selection::default();
        s.click(2);
        s.retain_visible(&[]);
        check(&s);
        assert_eq!(s.active(), None);
        assert!(s.is_empty());
    }

    #[test]
    fn pruning_nothing_is_a_no_op() {
        let mut s = Selection::default();
        s.click(1);
        s.toggle(2);
        let before = s.clone();
        s.retain_visible(&[0, 1, 2, 3]);
        check(&s);
        assert_eq!(s, before);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app pruning_ 2>&1 | grep -E "^error" | head -3`

Expected: FAIL to compile — `no method named `retain_visible` found`.

- [ ] **Step 3: Implement**

In `crates/app/src/selection.rs`, add after `clamp`:

```rust
    /// Drop every selected index that is not in `visible`, after the format filter changed.
    ///
    /// ⚠ Deliberately the same rule as [`Selection::clamp`]: if the *active* photo is dropped,
    /// the whole selection goes. Two different rules for "the active index went away" would be
    /// a smell, and silently moving the active photo elsewhere changes which photo a sync
    /// copies **from** — a change with nothing on screen to announce it.
    pub fn retain_visible(&mut self, visible: &[usize]) {
        self.set.retain(|i| visible.binary_search(i).is_ok());
        if self
            .active
            .is_some_and(|a| visible.binary_search(&a).is_err())
        {
            self.active = None;
        }
        if self.active.is_none() {
            self.set.clear();
        }
        self.anchor = self
            .anchor
            .filter(|a| visible.binary_search(a).is_ok());
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p app selection 2>&1 | grep -E "^test result|FAILED" | head -3`

Expected: PASS — the four new tests plus every existing `selection` test, which the `check`
helper guards.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/selection.rs
git commit -m "feat(app): prune the selection to visible entries"
```

---

## Task 3: Wire it into the browser

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the fields**

Add to `struct Library` (around line 62):

```rust
    /// Which formats the browser hides. Reset when a folder is loaded.
    filter: FormatFilter,
    /// Indices into `entries` that pass `filter`, ascending. Derived — recomputed by
    /// `refilter`, never edited directly.
    visible: Vec<usize>,
```

And the import, beside the other `rustroom_app` uses:

```rust
use rustroom_app::format_filter::{self, FormatFilter};
```

- [ ] **Step 2: Add the recompute helper**

Add an `impl Library` block next to the struct:

```rust
impl Library {
    /// Recompute `visible` and prune the selection to it.
    ///
    /// ⚠ Both halves, always. Recomputing the visible list without pruning leaves the
    /// selection holding photos that are not on screen, which a sync or a paste would then
    /// write to.
    fn refilter(&mut self) {
        self.visible = self.filter.visible_indices(&self.entries);
        self.selection.retain_visible(&self.visible);
    }
}
```

- [ ] **Step 3: Initialise on folder load**

Find where `Library { entries, thumbs, selection, loader }` is constructed (search for
`loader: ThumbLoader::new` or `Library {`). Add:

```rust
            filter: FormatFilter::default(),
            visible: (0..entries.len()).collect(),
```

⚠ The filter resets here by construction — a fresh `Library` per folder load, per the spec.
Make sure `visible` is seeded to all indices rather than left empty, or the grid opens blank.

- [ ] **Step 4: The toggle chips**

In the Library view's toolbar (near the `Open folder…` button, around line 2428), add:

```rust
                // Only when there is a choice to make: a lone toggle can only blank the grid.
                let formats = self
                    .library
                    .as_ref()
                    .map(|lib| format_filter::formats_present(&lib.entries))
                    .unwrap_or_default();
                if formats.len() >= 2 {
                    if let Some(lib) = &mut self.library {
                        ui.separator();
                        for (ext, count) in &formats {
                            let mut shown = !lib.filter.is_hidden(ext);
                            let label = format!("{} ({count})", ext.to_uppercase());
                            if ui.toggle_value(&mut shown, label).changed() {
                                lib.filter.toggle(ext);
                                lib.refilter();
                            }
                        }
                    }
                }
```

- [ ] **Step 5: Iterate `visible` in the grid**

In the grid loop (around line 2302), replace:

```rust
                    for i in 0..lib.entries.len() {
```

with:

```rust
                    for &i in &lib.visible {
```

⚠ `i` stays an index into `entries`, so `lib.thumbs[i]`, `lib.entries[i]` and every
`lib.selection` call inside the loop are unchanged — that is the point of the derived list.

If the borrow checker objects to iterating `lib.visible` while the body mutates
`lib.selection`, clone the list first: `let visible = lib.visible.clone();` before the
`ScrollArea`, then `for &i in &visible`. A few hundred `usize` per frame is nothing.

- [ ] **Step 6: Iterate `visible` in the filmstrip**

In the filmstrip loop (around line 2224), make the same replacement:

```rust
                        for &i in &visible {
```

hoisting `let visible = lib.visible.clone();` above the `ScrollArea` for the same borrow reason.

- [ ] **Step 7: Arrow navigation**

The Library grid's arrows (around line 2280) currently do `(cur + 1).min(n - 1)` and
`cur.saturating_sub(1)` in `entries` space. Replace with:

```rust
                let cur = lib.selection.active().unwrap_or(0);
                if i.key_pressed(egui::Key::ArrowRight) {
                    if let Some(n) = format_filter::step(&lib.visible, cur, true) {
                        lib.selection.click(n);
                    }
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    if let Some(n) = format_filter::step(&lib.visible, cur, false) {
                        lib.selection.click(n);
                    }
                }
```

And in `handle_develop_arrows` (around line 2178), replace the `n = lib.entries.len()` guard
and its ±1 arithmetic with the same `format_filter::step` call, keeping the existing
`ctx.memory(|m| m.focused().is_some())` guard untouched.

- [ ] **Step 8: Empty-state message**

The grid's `if lib.entries.is_empty()` check (around line 2270) should also cover "everything
is filtered out". Change it to test `lib.visible.is_empty()` and word the message for both
cases:

```rust
            if lib.visible.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(if lib.entries.is_empty() {
                        "No supported images in this folder."
                    } else {
                        "Every format is hidden — re-enable one above."
                    });
                });
                return;
            }
```

⚠ Keep the `lib.selection.clamp(lib.entries.len())` call above it: that guards a folder
reloaded with fewer photos, which is a different failure from filtering.

- [ ] **Step 9: Build and check**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features 2>&1 | grep -E "^warning|^error" | head -10`

Expected: clean.

- [ ] **Step 10: Full suite**

Run: `cargo test --all 2>&1 | grep -E "^error|FAILED" | head; cargo test --all 2>&1 | grep -c "test result: ok"`

Expected: no failures, 21 groups, and the 29 goldens byte-identical — this feature touches no
pixels.

- [ ] **Step 11: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): filter the Library and filmstrip by file format"
```

---

## Task 4: Verify and document

- [ ] **Step 1: Launch**

Run: `cargo run --release -p app`

- [ ] **Step 2: Check by hand**

Open `~/Desktop/fuji/`, which holds RAF and JPG.

1. **Two chips appear**, `RAF (n)` and `JPG (n)`, with the right counts.
2. **Untick JPG** — only RAFs remain, in both the grid and the filmstrip, instantly and with no
   thumbnail reload flicker.
3. **Arrow through the grid** — navigation skips the hidden format rather than landing on gaps.
4. **Select several photos, then hide their format** — the selection indicator should clear
   rather than leaving invisible photos selected. This is the one that matters.
5. **Open a RAW in Develop, then hide RAF** — the photo stays open and editable; the filmstrip
   shows only JPGs; arrows move to a JPG.
6. **Untick both** — the "every format is hidden" message, and re-ticking restores the grid.
7. **Open a folder with one format** — no chips at all.

- [ ] **Step 3: Fix or continue**

If anything misbehaves, reproduce it with a test before fixing.

- [ ] **Step 4: Amend the spec's selection rule**

In `docs/superpowers/specs/2026-08-13-format-filter-design.md`, §3, replace the paragraph that
says the active photo moves to a surviving member with the rule actually implemented — the
whole selection clears, matching `clamp` — and the reasoning from the correction note at the top
of this plan.

- [ ] **Step 5: Update `CLAUDE.md`**

Add a short feature section in the house style, above the bug-fix section, covering:

- The derived index list rather than a filtered `entries`, and why — `entries`, `thumbs`,
  `Selection` and `ThumbLoader` are all index-parallel, so filtering the list itself would mean
  a thumbnail reload per toggle.
- ⚠ The selection prune as the feature's real content, and that `retain_visible` follows
  `clamp`'s rule rather than inventing a second one.
- ⚠ Hidden-not-shown, so a newly appearing format defaults to visible.
- ⚠ `format_filter::step` existing because Develop keeps the open photo when its format is
  hidden, so `cur` may not be in the visible list.
- The three UI calls: chips only at ≥2 formats, the filmstrip filtered too, the filter resetting
  on folder load.
- Add `format_filter` to the `app` crate-layout line, and both docs to the index.

- [ ] **Step 6: Final check and commit**

Run: `cargo fmt --all --check && cargo clippy --all-targets --all-features 2>&1 | grep -E "^warning|^error" | head; cargo test --all 2>&1 | grep -E "^error|FAILED" | head`

```bash
git add CLAUDE.md docs/superpowers/specs/2026-08-13-format-filter-design.md
git commit -m "docs: record the Library format filter"
```
