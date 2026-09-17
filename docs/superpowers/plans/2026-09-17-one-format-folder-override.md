# A one-format folder shows its one format: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A folder whose entries have fewer than two formats always shows them all, overriding the stored hidden-set for that folder without editing it.

**Architecture:** One predicate in `format_filter` (lib), one `effective_for` on `LibraryFilter` (lib), applied at the single choke point `LibraryState::refilter` (bin) so all 12 call sites are covered, with `format_bullets` (bin) reading the same predicate.

**Tech Stack:** Rust, `crates/app`, `cargo test -p app`.

**Spec:** `docs/superpowers/specs/2026-09-17-one-format-folder-override.md`

⚠ Prefix every cargo command with:
```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```
⚠⚠ Never run two gates concurrently — 7 temp-dir test-isolation sites.

## File Structure

| File | Kind | Change |
|---|---|---|
| `crates/app/src/format_filter.rs` | lib | `folder_overrides_hidden` + 2 tests |
| `crates/app/src/library_filter.rs` | lib | `effective_for` + 3 tests |
| `crates/app/src/library_state.rs` | bin | one line in `refilter` |
| `crates/app/src/shell.rs` | bin | one line in `format_bullets` |

---

### Task 1: The predicate

**Files:** Modify `crates/app/src/format_filter.rs`

- [ ] **Step 1: Add the function** after `formats_present`

```rust
/// Whether a folder's own contents override the stored hidden-set.
///
/// ⚠⚠ **The ONE place this rule is written.** `shell.rs::format_bullets` draws no chips for such a
/// folder and `LibraryFilter::effective_for` hides nothing in it; before 2026-09-17 only the first
/// of those existed, and hiding `jpg` then opening a JPEG-only folder gave an empty grid **with no
/// control to undo it** — reported in use. Two callers reading one predicate is what stops them
/// drifting back apart into a chip that does nothing, or filtering with no chip.
///
/// ⚠ Fewer than *two* rather than exactly one: an empty folder, and a folder of extension-less
/// files, both have nothing to choose between either.
pub fn folder_overrides_hidden(entries: &[PhotoEntry]) -> bool {
    formats_present(entries).len() < 2
}
```

- [ ] **Step 2: Add the tests** in the tests module

```rust
    #[test]
    fn a_folder_with_one_format_overrides_the_hidden_set() {
        assert!(folder_overrides_hidden(&entries(&["a.jpg", "b.JPG", "c.jpg"])));
        assert!(folder_overrides_hidden(&entries(&[])));
        // ⚠ Extension-less files contribute no format, so a folder of them has nothing to choose.
        assert!(folder_overrides_hidden(&entries(&["notes", "readme"])));
    }

    #[test]
    fn a_mixed_folder_does_not_override() {
        assert!(!folder_overrides_hidden(&folder()));
        assert!(!folder_overrides_hidden(&entries(&["a.jpg", "b.raf"])));
        // ⚠ Case-folded, so `.JPG` and `.jpg` are ONE format and this folder still overrides.
        assert!(folder_overrides_hidden(&entries(&["a.JPG", "b.jpg"])));
    }
```

- [ ] **Step 3: Run**

`cargo test --release -q -p app format_filter 2>&1 | grep -E 'test result|error\['`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/format_filter.rs
git commit -m "feat(library): the predicate for a folder that overrides the hidden-set"
```

---

### Task 2: `effective_for`

**Files:** Modify `crates/app/src/library_filter.rs`

- [ ] **Step 1: Add the method** to `impl LibraryFilter`, beside `is_active`

```rust
    /// This filter as it actually applies to `entries`.
    ///
    /// ⚠⚠ **A folder with fewer than two formats shows them all** — the stored hidden-set is
    /// overridden for that folder and **never edited**, so returning to a mixed folder uses the
    /// remembered setting unchanged. Editing it instead would silently unhide the format
    /// everywhere, which is not what "this folder has only JPEGs" means.
    ///
    /// ⚠ Every other half of the filter is left alone. A one-format folder can still be filtered
    /// to empty by rating or flag, and that is recoverable because those controls are always drawn.
    pub fn effective_for(&self, entries: &[PhotoEntry]) -> Self {
        let mut out = self.clone();
        if crate::format_filter::folder_overrides_hidden(entries) {
            out.format = FormatFilter::default();
        }
        out
    }
```

- [ ] **Step 2: Add the tests**

```rust
    #[test]
    fn a_one_format_folder_shows_everything_it_holds() {
        // The reported trap: hide jpg, then open a folder of nothing but jpg.
        let mut f = LibraryFilter::default();
        f.format.toggle("jpg");
        let only_jpg = [entry("a.jpg", 0, None), entry("b.JPG", 0, None)];
        let eff = f.effective_for(&only_jpg);
        for e in &only_jpg {
            assert!(
                eff.matches(e, Marks::default(), &Default::default()),
                "a one-format folder must show its one format"
            );
        }
        // ⚠ And the stored set is untouched, so the setting survives the visit.
        assert!(f.format.is_hidden("jpg"), "the override must not EDIT the set");
    }

    #[test]
    fn a_mixed_folder_still_honours_the_hidden_set() {
        let mut f = LibraryFilter::default();
        f.format.toggle("jpg");
        let mixed = [entry("a.jpg", 0, None), entry("b.raf", 0, None)];
        let eff = f.effective_for(&mixed);
        assert!(!eff.matches(&mixed[0], Marks::default(), &Default::default()));
        assert!(eff.matches(&mixed[1], Marks::default(), &Default::default()));
    }

    #[test]
    fn an_overridden_filter_reports_itself_inactive() {
        // ⚠ `is_active` has no production caller today; this pins the semantics for its first one.
        // A filter hiding nothing must not claim to be on — the inverse of the invisible state the
        // crop aspect lock was fixed for.
        let mut f = LibraryFilter::default();
        f.format.toggle("jpg");
        assert!(f.is_active(), "the stored filter is on");
        assert!(!f
            .effective_for(&[entry("a.jpg", 0, None)])
            .is_active());
        assert!(f
            .effective_for(&[entry("a.jpg", 0, None), entry("b.raf", 0, None)])
            .is_active());
    }
```

- [ ] **Step 3: Run**

`cargo test --release -q -p app library_filter 2>&1 | grep -E 'test result|error\['`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/library_filter.rs
git commit -m "feat(library): a filter's effective form for a given folder"
```

---

### Task 3: Apply it at the choke point

**Files:** Modify `crates/app/src/library_state.rs:36-52` and `crates/app/src/shell.rs:795`

- [ ] **Step 1: `refilter` uses the effective filter**

In `library_state.rs`, immediately after the `fn refilter` signature line, insert:

```rust
        // ⚠⚠ Applied HERE rather than at the callers: `refilter` has 12 call sites across `shell`,
        // `library`, `library_state`, `sources`, `multi::compare` and `test_app`, and the override
        // has to hold at every one. One choke point cannot be bypassed; twelve are twelve chances
        // to forget — the same argument this function's own comment makes for `visible`.
        let filter = &filter.effective_for(&self.entries);
```

- [ ] **Step 2: `format_bullets` reads the same predicate**

In `shell.rs`, replace:

```rust
    if formats.len() < 2 {
        return;
    }
```

with:

```rust
    // ⚠⚠ The SAME predicate the filtering uses, not a second copy of the rule. When these were two
    // independent conditions, hiding `jpg` and opening a JPEG-only folder drew no chips AND hid
    // every photo — an empty grid with no way back.
    let overridden = app
        .library
        .as_ref()
        .is_some_and(|lib| format_filter::folder_overrides_hidden(&lib.entries));
    if overridden {
        return;
    }
```

- [ ] **Step 3: Build and run the app crate's tests**

`cargo test --release -q -p app 2>&1 | grep -E 'test result|error\[' | head`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/library_state.rs crates/app/src/shell.rs
git commit -m "fix(library): a one-format folder shows its one format"
```

---

### Task 4: Sabotage — prove the tests have teeth

- [ ] **Step 1: Disable the override**

In `format_filter.rs`, change `folder_overrides_hidden`'s body to:

```rust
    let _ = entries;
    false
```

- [ ] **Step 2: Run and confirm FAIL**

`cargo test --release -q -p app 2>&1 | grep -E 'test result|panicked' | head -4`
Expected: **FAIL** on `a_one_format_folder_shows_everything_it_holds`,
`an_overridden_filter_reports_itself_inactive` and `a_folder_with_one_format_overrides_the_hidden_set`.

⚠⚠ **If they PASS, stop.** The tests are not testing the override.

- [ ] **Step 3: Revert**

```bash
git checkout crates/app/src/format_filter.rs
```

- [ ] **Step 4: Confirm PASS restored**

`cargo test --release -q -p app format_filter 2>&1 | grep 'test result'`

---

### Task 5: The gate, each exit code from its own command

- [ ] **Step 1**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --check >/dev/null 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings >/dev/null 2>&1; echo "CLIPPY=$?"
cargo test --workspace >/tmp/ofo_test.txt 2>&1; echo "TEST=$?"
awk '/Running tests\/golden.rs/{f=1} f&&/test result/{print "golden: " $0; exit}' /tmp/ofo_test.txt
```
Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`, 74 goldens, 0 filtered.
⚠ If `FMT=1`, run `cargo fmt` and **re-read all three**.

- [ ] **Step 2: Confirm the gated tree IS the tip**

```bash
git diff HEAD --stat; git status --porcelain
```
Both empty. ⚠ If `cargo fmt` changed anything, commit it and **re-gate**.

---

### Task 6: Launch the GUI and record

- [ ] **Step 1: Launch it**, unprompted, per this repo's standing instruction

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```
Say what to look at: hide JPG in a **mixed** folder; open a **JPEG-only** folder — photos present,
no chips; return to the mixed folder — JPG hidden again.

- [ ] **Step 2: Record**

`CLAUDE.md` state + next action (⚠ **overwrite in place**, move the replaced block to
`docs/JOURNAL.md`), and a journal section covering the two-reasonable-decisions shape and the
false Filter-button premise the spec records.

- [ ] **Step 3: Commit, do NOT merge, do NOT push**

## Self-Review

**Spec coverage.** Done-when 1 → Task 1. 2 → Task 2. 3 → Task 3 step 1. 4 → Task 3 step 2.
5 → Tasks 1–2 tests. 6 → Task 4. 7 → Task 5. 8 → Task 6 step 1, performed by the user.

**Placeholders.** None; every step carries its full code or exact command.

**Type consistency.** `folder_overrides_hidden(&[PhotoEntry]) -> bool` is defined in Task 1 and used
in Tasks 2 and 3 with that exact signature. `effective_for(&self, &[PhotoEntry]) -> Self` is defined
in Task 2 and used in Task 3. `FormatFilter::default()`, `LibraryFilter::is_active`,
`FormatFilter::is_hidden` and the `entry` / `entries` / `folder` fixtures all match the tree.
