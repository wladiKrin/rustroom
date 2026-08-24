# Multi-selection & Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Select several photos in the grid or filmstrip and apply the open photo's settings to all of them, undoably.

**Architecture:** Three new `app` lib modules, all pure and `egui`-free: `selection` (the model), `sidecar` (extracted from `main.rs` so sync can reach it), and `sync` (the batch itself). Sync writes sidecars and decodes nothing. `ActionKind` gains a `Sync` variant so one Ctrl+Z reverses a whole batch. **No pipeline stage is touched — the 23 goldens must stay byte-identical.**

**Tech Stack:** Rust workspace, `serde_json`, `egui`/`eframe` 0.35.

**Spec:** `docs/superpowers/specs/2026-08-07-multi-select-sync-design.md` (approved — do not re-derive).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/app/src/selection.rs` | `Selection` model + invariant | **Create** |
| `crates/app/src/sidecar.rs` | `path` / `load` / `save` / `remove` | **Create** |
| `crates/app/src/sync.rs` | `apply` / `revert` / `SyncReport` | **Create** |
| `crates/app/src/action_log.rs` | `ActionKind::Sync`, drop `Eq` | Modify |
| `crates/app/src/lib.rs` | register the three modules | Modify |
| `crates/app/src/main.rs` | swap the field, gestures, sync UI, undo wiring | Modify |
| `CLAUDE.md` | status section | Modify |

Every command assumes:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Package names:** crate `app`, lib `rustroom_app`, binary `Rustroom`. Use `cargo test -p app`.

---

## Task 1: `app::selection`

**Files:** Create `crates/app/src/selection.rs`; modify `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/selection.rs` with **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ The invariant every method must preserve: an active photo is always selected, and
    /// no active photo means nothing is selected. Sync would otherwise have a source that
    /// is not part of what it is syncing, or a selection with no source at all.
    fn check(s: &Selection) {
        match s.active() {
            Some(a) => assert!(s.contains(a), "active {a} is not in the selection"),
            None => assert!(s.is_empty(), "no active photo but the selection is non-empty"),
        }
    }

    fn items(s: &Selection) -> Vec<usize> {
        s.iter().collect()
    }

    #[test]
    fn a_plain_click_replaces_everything() {
        let mut s = Selection::default();
        s.click(3);
        check(&s);
        assert_eq!(s.active(), Some(3));
        assert_eq!(items(&s), vec![3]);
        s.toggle(5);
        s.click(1);
        check(&s);
        assert_eq!(items(&s), vec![1], "a plain click must not keep the old selection");
    }

    #[test]
    fn toggle_adds_and_removes_but_never_the_active_photo() {
        let mut s = Selection::default();
        s.click(2);
        s.toggle(4);
        check(&s);
        assert_eq!(items(&s), vec![2, 4]);
        s.toggle(4);
        check(&s);
        assert_eq!(items(&s), vec![2]);
        // The source cannot be deselected — the price of "modifiers never move the active".
        s.toggle(2);
        check(&s);
        assert_eq!(items(&s), vec![2]);
        assert_eq!(s.active(), Some(2));
    }

    #[test]
    fn extend_works_in_both_directions_and_keeps_the_active_photo() {
        let mut s = Selection::default();
        s.click(5);
        s.extend_to(8);
        check(&s);
        assert_eq!(items(&s), vec![5, 6, 7, 8]);
        assert_eq!(s.active(), Some(5), "shift-click must not move the active photo");

        let mut back = Selection::default();
        back.click(5);
        back.extend_to(2);
        check(&back);
        assert_eq!(items(&back), vec![2, 3, 4, 5]);
    }

    #[test]
    fn repeated_shift_clicks_extend_from_the_same_anchor() {
        // Standard behaviour: the second shift-click replaces the range rather than
        // growing from wherever the last one landed.
        let mut s = Selection::default();
        s.click(4);
        s.extend_to(8);
        s.extend_to(6);
        check(&s);
        assert_eq!(items(&s), vec![4, 5, 6]);
    }

    #[test]
    fn extend_keeps_the_active_photo_when_the_anchor_moved_away() {
        // A Cmd-click moves the anchor without moving the active photo, so the range alone
        // would drop the source.
        let mut s = Selection::default();
        s.click(0);
        s.toggle(7); // anchor -> 7, active still 0
        s.extend_to(9);
        check(&s);
        assert_eq!(items(&s), vec![0, 7, 8, 9]);
        assert_eq!(s.active(), Some(0));
    }

    #[test]
    fn select_all_adopts_index_zero_only_when_there_is_no_active_photo() {
        let mut fresh = Selection::default();
        fresh.select_all(4);
        check(&fresh);
        assert_eq!(fresh.active(), Some(0));
        assert_eq!(items(&fresh), vec![0, 1, 2, 3]);

        let mut existing = Selection::default();
        existing.click(2);
        existing.select_all(4);
        check(&existing);
        assert_eq!(existing.active(), Some(2), "an existing source must be kept");

        let mut none = Selection::default();
        none.select_all(0);
        check(&none);
        assert_eq!(none.active(), None);
    }

    #[test]
    fn clear_leaves_exactly_the_active_photo() {
        let mut s = Selection::default();
        s.click(1);
        s.extend_to(5);
        s.clear();
        check(&s);
        assert_eq!(items(&s), vec![1]);

        let mut empty = Selection::default();
        empty.clear();
        check(&empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn clamp_drops_out_of_range_indices() {
        let mut s = Selection::default();
        s.click(1);
        s.extend_to(6);
        s.clamp(4);
        check(&s);
        assert_eq!(items(&s), vec![1, 2, 3]);
    }

    #[test]
    fn clamp_empties_the_selection_when_the_active_photo_is_gone() {
        // A folder that shrank out from under the selection: keeping targets with no
        // source would leave a sync with nothing to sync *from*.
        let mut s = Selection::default();
        s.click(9);
        s.toggle(2);
        s.clamp(4);
        check(&s);
        assert!(s.is_empty());
        assert_eq!(s.active(), None);
    }

    #[test]
    fn others_excludes_the_active_photo() {
        // Exactly a sync's target list.
        let mut s = Selection::default();
        s.click(2);
        s.extend_to(5);
        assert_eq!(s.others().collect::<Vec<_>>(), vec![3, 4, 5]);
        assert_eq!(s.len(), 4);
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/app/src/lib.rs`, add after `pub mod render_engine;`:

```rust
pub mod selection;
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p app --lib selection
```

Expected: FAIL to compile — `cannot find type 'Selection'`.

- [ ] **Step 4: Implement it**

Prepend to `crates/app/src/selection.rs`, above the test module:

```rust
//! Which photos are selected, and which one a sync copies *from*.
//!
//! Pure and `egui`-free so the range arithmetic and clamping are testable rather than only
//! visible as a wrong highlight — the same reasoning behind `crop_tool` and `view_region`.

use std::collections::BTreeSet;

/// A multi-selection with a distinguished **active** photo: the one open in Develop, and
/// the source for a sync.
///
/// ⚠ Invariant, asserted after every operation in the tests: `active == Some(i)` implies
/// `i` is selected, and `active == None` implies nothing is selected.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    active: Option<usize>,
    set: BTreeSet<usize>,
    /// Where a shift-click measures its range from. Moved by plain clicks and Cmd-clicks,
    /// *not* by shift-clicks, so repeated shift-clicks re-measure from the same origin.
    anchor: Option<usize>,
}

impl Selection {
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    pub fn contains(&self, i: usize) -> bool {
        self.set.contains(&i)
    }

    /// Selected indices, ascending.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.set.iter().copied()
    }

    /// Everything selected except the active photo — a sync's targets.
    pub fn others(&self) -> impl Iterator<Item = usize> + '_ {
        let active = self.active;
        self.set.iter().copied().filter(move |&i| Some(i) != active)
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Plain click: this becomes the active photo and the entire selection.
    pub fn click(&mut self, i: usize) {
        self.active = Some(i);
        self.set.clear();
        self.set.insert(i);
        self.anchor = Some(i);
    }

    /// Cmd/Ctrl-click: add or remove.
    ///
    /// ⚠ Never touches the active photo. Deselecting the source would break the invariant,
    /// and moving the active photo here would kick off a RAW decode mid-selection.
    pub fn toggle(&mut self, i: usize) {
        if self.active == Some(i) {
            return;
        }
        if !self.set.remove(&i) {
            self.set.insert(i);
        }
        self.anchor = Some(i);
    }

    /// Shift-click: the inclusive range from the anchor to `i`.
    ///
    /// The active photo is re-inserted afterwards because a preceding Cmd-click can have
    /// moved the anchor away from it, and the range alone would then drop the source.
    pub fn extend_to(&mut self, i: usize) {
        let from = self.anchor.or(self.active).unwrap_or(i);
        let (lo, hi) = (from.min(i), from.max(i));
        self.set.clear();
        self.set.extend(lo..=hi);
        match self.active {
            Some(a) => {
                self.set.insert(a);
            }
            None => self.active = Some(i),
        }
    }

    /// Select `0..n`. With no active photo yet, index 0 becomes active — otherwise this
    /// would produce a non-empty set with no source and break the invariant.
    pub fn select_all(&mut self, n: usize) {
        self.set = (0..n).collect();
        if n == 0 {
            self.active = None;
            self.anchor = None;
        } else if self.active.is_none_or(|a| a >= n) {
            self.active = Some(0);
            self.anchor = Some(0);
        }
    }

    /// Back to just the active photo; empty if there is none.
    pub fn clear(&mut self) {
        self.set.clear();
        if let Some(a) = self.active {
            self.set.insert(a);
        }
        self.anchor = self.active;
    }

    /// Drop indices past `n`, after the folder's contents changed.
    pub fn clamp(&mut self, n: usize) {
        self.set.retain(|&i| i < n);
        if self.active.is_some_and(|a| a >= n) {
            self.active = None;
        }
        if self.active.is_none() {
            // Targets with no source are not a usable state.
            self.set.clear();
        }
        self.anchor = self.anchor.filter(|&a| a < n);
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app --lib selection
cargo clippy -p app --all-targets -- -D warnings
```

Expected: 10 tests PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/app/src/selection.rs crates/app/src/lib.rs
git commit -m "feat(app): add the Selection model with an active-photo invariant"
```

---

## Task 2: `app::sidecar`

**Files:** Create `crates/app/src/sidecar.rs`; modify `crates/app/src/lib.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/sidecar.rs` with **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp directory for one test, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rustroom_sidecar_{tag}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn photo(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_path_appends_json_to_the_whole_filename() {
        // Not "replace the extension": a RAW and a JPEG sharing a basename must get
        // distinct sidecars.
        assert_eq!(
            path(Path::new("/a/photo.RAF")),
            PathBuf::from("/a/photo.RAF.json")
        );
        assert_eq!(
            path(Path::new("/a/photo.jpg")),
            PathBuf::from("/a/photo.jpg.json")
        );
        assert_ne!(path(Path::new("/a/p.RAF")), path(Path::new("/a/p.jpg")));
    }

    #[test]
    fn save_then_load_round_trips() {
        let d = TempDir::new("roundtrip");
        let photo = d.photo("x.RAF");
        let e = Edits {
            exposure: 1.5,
            ..Edits::default()
        };
        save(&photo, &e).unwrap();
        assert_eq!(load(&photo), Some(e));
    }

    #[test]
    fn an_absent_or_malformed_sidecar_loads_as_none() {
        // The same forgiving posture the preset and settings stores take.
        let d = TempDir::new("absent");
        let photo = d.photo("x.RAF");
        assert_eq!(load(&photo), None);
        std::fs::write(path(&photo), "{ not json").unwrap();
        assert_eq!(load(&photo), None);
    }

    #[test]
    fn removing_an_absent_sidecar_is_not_an_error() {
        let d = TempDir::new("remove");
        let photo = d.photo("x.RAF");
        remove(&photo).unwrap();
        save(&photo, &Edits::default()).unwrap();
        assert!(path(&photo).exists());
        remove(&photo).unwrap();
        assert!(!path(&photo).exists());
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/app/src/lib.rs`, add after `pub mod selection;`:

```rust
pub mod sidecar;
```

- [ ] **Step 3: Run to verify it fails**

```bash
cargo test -p app --lib sidecar
```

Expected: FAIL to compile — `cannot find function 'path'`.

- [ ] **Step 4: Implement it**

Prepend to `crates/app/src/sidecar.rs`:

```rust
//! The `<file>.json` edit sidecar.
//!
//! Extracted from `main.rs` so `sync` can reach it and so it is testable at all.

use edit_model::{EditLoadError, Edits};
use std::path::{Path, PathBuf};

/// The sidecar path for a source file: the full filename with `.json` **appended**
/// (`photo.RAF` → `photo.RAF.json`), not the extension replaced — so a RAW and a JPEG
/// sharing a basename get distinct sidecars.
pub fn path(photo: &Path) -> PathBuf {
    let mut s = photo.as_os_str().to_os_string();
    s.push(".json");
    PathBuf::from(s)
}

/// A photo's stored edits. `None` when there is no sidecar **or it is malformed** — a
/// hand-edited file with a typo should read as "no edits", not take anything down.
pub fn load(photo: &Path) -> Option<Edits> {
    Edits::from_json_file(&path(photo)).ok()
}

pub fn save(photo: &Path, edits: &Edits) -> Result<(), EditLoadError> {
    edits.to_json_file(&path(photo))
}

/// Delete a photo's sidecar. Absent is not an error.
pub fn remove(photo: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path(photo)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
```

- [ ] **Step 5: Replace the `main.rs` copy**

In `crates/app/src/main.rs`, delete the whole `sidecar_path` function (the doc comment plus the four-line body starting `fn sidecar_path(`), then add to the `use` block:

```rust
use rustroom_app::sidecar;
```

Replace the three call sites:

- in `persist_current_session`: `s.edits.to_json_file(&sidecar_path(&s.path))` → `sidecar::save(&s.path, &s.edits)`
- in `finalize_session`, replace

```rust
        let sidecar = sidecar_path(&path);
        let edits = if sidecar.exists() {
            edit_model::Edits::from_json_file(&sidecar).unwrap_or_default()
        } else {
            Edits::default()
        };
```

with

```rust
        let edits = sidecar::load(&path).unwrap_or_default();
```

- in the export thread: `edits.to_json_file(&sidecar_path(&out))?` → `sidecar::save(&out, &edits)?`

- [ ] **Step 6: Run to verify**

```bash
cargo test -p app --lib sidecar
cargo build -p app
cargo clippy -p app --all-targets -- -D warnings
```

Expected: 4 tests PASS, builds clean, no warnings.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/app/src/sidecar.rs crates/app/src/lib.rs crates/app/src/main.rs
git commit -m "refactor(app): extract the sidecar module out of main.rs"
```

---

## Task 3: `app::sync`

**Files:** Create `crates/app/src/sync.rs`; modify `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/sync.rs` with **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp directory for one test, removed on drop.
    ///
    /// The photo files themselves are never created: `sidecar::path` only appends `.json`,
    /// so a sync touches nothing but sidecars and needs no image fixtures at all.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rustroom_sync_{tag}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn photo(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn source() -> Edits {
        Edits {
            exposure: 1.0,
            vignette: edit_model::Vignette {
                amount: -50.0,
                ..Default::default()
            },
            ..Edits::default()
        }
    }

    #[test]
    fn a_sync_applies_only_the_selected_groups() {
        let d = TempDir::new("groups");
        let a = d.photo("a.RAF");
        let report = apply(&[a.clone()], &source(), EditGroups::only(1)); // Tone
        assert!(report.failed.is_empty());

        let got = sidecar::load(&a).expect("a sidecar should exist now");
        assert_eq!(got.exposure, 1.0, "tone should transfer");
        assert_eq!(
            got.vignette,
            edit_model::Vignette::default(),
            "effects were not selected"
        );
    }

    #[test]
    fn prior_records_none_for_a_photo_that_had_no_sidecar() {
        // ⚠ The load-bearing distinction: undoing must *delete* a sidecar the sync created,
        // not write a neutral one and leave litter behind.
        let d = TempDir::new("prior_none");
        let fresh = d.photo("fresh.RAF");
        let existing = d.photo("existing.RAF");
        let before = Edits {
            contrast: 25.0,
            ..Edits::default()
        };
        sidecar::save(&existing, &before).unwrap();

        let report = apply(
            &[fresh.clone(), existing.clone()],
            &source(),
            EditGroups::ALL,
        );
        assert_eq!(report.prior.len(), 2);
        assert_eq!(report.prior[0], (fresh, None));
        assert_eq!(report.prior[1], (existing, Some(before)));
    }

    #[test]
    fn revert_restores_edited_sidecars_and_deletes_created_ones() {
        let d = TempDir::new("revert");
        let fresh = d.photo("fresh.RAF");
        let existing = d.photo("existing.RAF");
        let before = Edits {
            contrast: 25.0,
            ..Edits::default()
        };
        sidecar::save(&existing, &before).unwrap();

        let report = apply(
            &[fresh.clone(), existing.clone()],
            &source(),
            EditGroups::ALL,
        );
        assert!(sidecar::path(&fresh).exists());

        assert!(revert(&report.prior).is_empty());
        assert!(
            !sidecar::path(&fresh).exists(),
            "a sidecar the sync created must be removed, not neutralised"
        );
        assert_eq!(sidecar::load(&existing), Some(before));
    }

    #[test]
    fn a_failed_target_is_reported_without_aborting_the_batch() {
        // A locked or unreachable file should not cost the other photos their sync.
        let d = TempDir::new("failure");
        let good = d.photo("good.RAF");
        let bad = d.0.join("no_such_dir").join("bad.RAF");

        let report = apply(&[bad.clone(), good.clone()], &source(), EditGroups::ALL);
        assert_eq!(report.failed, vec![bad]);
        assert_eq!(report.prior.len(), 1, "the good target still synced");
        assert!(sidecar::load(&good).is_some());
    }

    #[test]
    fn syncing_an_empty_target_list_does_nothing() {
        let report = apply(&[], &source(), EditGroups::ALL);
        assert!(report.prior.is_empty() && report.failed.is_empty());
        assert!(revert(&report.prior).is_empty());
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/app/src/lib.rs`, add after `pub mod sidecar;`:

```rust
pub mod sync;
```

- [ ] **Step 3: Run to verify it fails**

```bash
cargo test -p app --lib sync
```

Expected: FAIL to compile — `cannot find function 'apply'`.

- [ ] **Step 4: Implement it**

Prepend to `crates/app/src/sync.rs`:

```rust
//! Applying one photo's edits to many others.
//!
//! **No decoding happens here.** Every target's edits live in a JSON sidecar, so a sync is
//! `load → merge_from → save`. Syncing 200 photos is 200 tiny file operations rather than
//! 200 RAW decodes (~11 s each) — which is what makes a progress bar, a worker thread and
//! cancellation all unnecessary.

use crate::sidecar;
use edit_model::{EditGroups, Edits};
use std::path::PathBuf;

/// What a sync changed, and what it could not.
pub struct SyncReport {
    /// Each target's state *before* the sync, for undo.
    ///
    /// ⚠ `None` means the photo had **no sidecar at all**, so undoing must delete the file
    /// the sync created rather than writing a neutral one and leaving litter behind.
    pub prior: Vec<(PathBuf, Option<Edits>)>,
    /// Targets whose sidecar could not be written.
    pub failed: Vec<PathBuf>,
}

/// Apply `source`'s selected groups to every target's sidecar.
///
/// A failure on one photo does not abort the batch: a locked or unreachable file should not
/// cost the other targets their sync.
pub fn apply(targets: &[PathBuf], source: &Edits, groups: EditGroups) -> SyncReport {
    let mut prior = Vec::with_capacity(targets.len());
    let mut failed = Vec::new();
    for target in targets {
        let before = sidecar::load(target);
        let mut edits = before.clone().unwrap_or_default();
        edits.merge_from(source, groups);
        match sidecar::save(target, &edits) {
            Ok(()) => prior.push((target.clone(), before)),
            Err(_) => failed.push(target.clone()),
        }
    }
    SyncReport { prior, failed }
}

/// Reverse a sync, restoring each target's prior state. Returns the paths that failed.
pub fn revert(prior: &[(PathBuf, Option<Edits>)]) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for (target, before) in prior {
        let ok = match before {
            Some(edits) => sidecar::save(target, edits).is_ok(),
            None => sidecar::remove(target).is_ok(),
        };
        if !ok {
            failed.push(target.clone());
        }
    }
    failed
}
```

- [ ] **Step 5: Run to verify**

```bash
cargo test -p app --lib sync
cargo clippy -p app --all-targets -- -D warnings
```

Expected: 5 tests PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/app/src/sync.rs crates/app/src/lib.rs
git commit -m "feat(app): add sync, applying one photo's edits through sidecars"
```

---

## Task 4: `ActionKind::Sync`

**Files:** Modify `crates/app/src/action_log.rs`

- [ ] **Step 1: Write the failing test**

Append inside `#[cfg(test)] mod tests` in `crates/app/src/action_log.rs`:

```rust
    fn sync_entry() -> ActionKind {
        ActionKind::Sync {
            source: edit_model::Edits {
                exposure: 1.0,
                ..Default::default()
            },
            groups: edit_model::EditGroups::default(),
            prior: vec![(std::path::PathBuf::from("/a/b.RAF"), None)],
        }
    }

    #[test]
    fn a_sync_survives_a_photo_switch() {
        // A sync touches other photos' sidecars, never the live session, so it is global
        // in exactly the way a preset deletion is.
        let mut log = ActionLog::default();
        log.push(sync_entry());
        log.push(ActionKind::PhotoEdit);
        log.forget_photo_edits();
        assert_eq!(log.undo(), Some(sync_entry()));
        assert!(!log.can_undo());
    }

    #[test]
    fn a_sync_orders_with_everything_else() {
        let mut log = ActionLog::default();
        log.push(sync_entry());
        log.push(ActionKind::PhotoEdit);
        assert_eq!(log.undo(), Some(ActionKind::PhotoEdit));
        assert_eq!(log.undo(), Some(sync_entry()));
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --lib action_log
```

Expected: FAIL to compile — no variant `Sync`.

- [ ] **Step 3: Add the variant**

In `crates/app/src/action_log.rs`, add the imports at the top:

```rust
use edit_model::{EditGroups, Edits};
use std::path::PathBuf;
```

Change the derive on `ActionKind` from

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
```

to

```rust
// ⚠ Not `Eq`: `Edits` holds `f32`s, so it is `PartialEq` only. That is all the tests need.
#[derive(Debug, Clone, PartialEq)]
```

and add the variant after `PresetDelete`:

```rust
    /// One photo's settings were applied to several others' sidecars.
    ///
    /// Carries everything needed to reverse *and* replay the batch: `prior` restores each
    /// target (`None` = delete the sidecar the sync created), while `source` and `groups`
    /// re-apply it. Global, like `PresetDelete` — it never touches the live session.
    Sync {
        source: Edits,
        groups: EditGroups,
        prior: Vec<(PathBuf, Option<Edits>)>,
    },
```

- [ ] **Step 4: Run to verify**

```bash
cargo test -p app --lib action_log
cargo clippy -p app --all-targets -- -D warnings
```

Expected: 10 tests PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/app/src/action_log.rs
git commit -m "feat(app): add a Sync action so a whole batch is one undo step"
```

---

## Task 5: Swap `Library.selected` for `Selection`

Mechanical, but it touches every selection call site. No behaviour change yet — multi-select gestures land in Task 6.

**Files:** Modify `crates/app/src/main.rs`

- [ ] **Step 1: Change the field**

Add to the `use` block:

```rust
use rustroom_app::selection::Selection;
```

In `struct Library`, replace `selected: Option<usize>,` with:

```rust
    selection: Selection,
```

- [ ] **Step 2: Update every call site**

Where the `Library` is constructed (`selected: None,`):

```rust
                    selection: Selection::default(),
```

In `open_entry`, `lib.selected = Some(index);` →

```rust
                    lib.selection.click(index);
```

In `handle_develop_arrows`, `let cur = lib.selected.unwrap_or(0);` →

```rust
            let cur = lib.selection.active().unwrap_or(0);
```

…and, a few lines below, `self.library.as_ref().and_then(|l| l.selected) != Some(i)` →

```rust
            if self.library.as_ref().and_then(|l| l.selection.active()) != Some(i) {
```

In `filmstrip`, `if lib.selected == Some(i) {` →

```rust
                            if lib.selection.active() == Some(i) {
```

In `library_view`'s keyboard block, replace

```rust
                let cur = lib.selected.unwrap_or(0);
                if i.key_pressed(egui::Key::ArrowRight) {
                    lib.selected = Some((cur + 1).min(n - 1));
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    lib.selected = Some(cur.saturating_sub(1));
                }
                if i.key_pressed(egui::Key::Enter) {
                    open_index = lib.selected;
                }
```

with

```rust
                let cur = lib.selection.active().unwrap_or(0);
                if i.key_pressed(egui::Key::ArrowRight) {
                    lib.selection.click((cur + 1).min(n - 1));
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    lib.selection.click(cur.saturating_sub(1));
                }
                if i.key_pressed(egui::Key::Enter) {
                    open_index = lib.selection.active();
                }
```

In the grid cell loop, `if lib.selected == Some(i) {` →

```rust
                        if lib.selection.active() == Some(i) {
```

…and `lib.selected = Some(i);` →

```rust
                            lib.selection.click(i);
```

- [ ] **Step 3: Clamp when the folder reloads**

In `open_folder`, immediately after the new `Library` is stored, the selection is fresh, so
nothing is needed there. But `library_view` must tolerate a shrunken list; add this as the
first statement inside the `egui::CentralPanel::default().show(ui, |ui| {` closure, right
after the `let Some(lib) = &mut self.library else { ... }` block:

```rust
            // A folder can be reloaded with fewer photos than the selection remembers.
            lib.selection.clamp(lib.entries.len());
```

- [ ] **Step 4: Build and verify**

```bash
cargo build -p app
cargo clippy -p app --all-targets -- -D warnings
cargo test --all
```

Expected: builds clean, no warnings, all tests pass. Behaviour is unchanged so far.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/app/src/main.rs
git commit -m "refactor(app): hold a Selection instead of a single index"
```

---

## Task 6: Multi-select gestures

**Files:** Modify `crates/app/src/main.rs`

- [ ] **Step 1: Add the shared gesture helper**

Add this free function next to `edit_group_checkboxes` at the bottom of `main.rs`:

```rust
/// Apply a click to the selection according to its modifiers. Returns whether this was a
/// plain click, which is the only gesture that moves the active photo — and therefore the
/// only one the filmstrip turns into an open.
fn apply_click(sel: &mut Selection, i: usize, mods: egui::Modifiers) -> bool {
    if mods.command {
        sel.toggle(i);
        false
    } else if mods.shift {
        sel.extend_to(i);
        false
    } else {
        sel.click(i);
        true
    }
}

/// Outline a selected cell: bright for the active photo, dim for the rest of the selection.
/// Being able to tell the source from the targets is what makes a sync predictable.
fn paint_selection_border(ui: &egui::Ui, rect: egui::Rect, active: bool, selected: bool) {
    if !selected {
        return;
    }
    let color = if active {
        egui::Color32::from_rgb(120, 160, 240)
    } else {
        egui::Color32::from_rgb(90, 100, 120)
    };
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(3),
        egui::Stroke::new(2.0, color),
        egui::StrokeKind::Inside,
    );
}
```

- [ ] **Step 2: Wire the grid**

In `library_view`'s cell loop, replace the border block

```rust
                        if lib.selection.active() == Some(i) {
```

…through its closing brace with:

```rust
                        paint_selection_border(
                            ui,
                            resp.rect,
                            lib.selection.active() == Some(i),
                            lib.selection.contains(i),
                        );
```

and replace the click handler

```rust
                        if resp.clicked() {
                            lib.selection.click(i);
                        }
```

with:

```rust
                        if resp.clicked() {
                            let mods = ui.input(|inp| inp.modifiers);
                            apply_click(&mut lib.selection, i, mods);
                        }
```

In the same function's keyboard block, add after the Enter handler:

```rust
                if i.key_pressed(egui::Key::A) && i.modifiers.command {
                    lib.selection.select_all(n);
                }
                if i.key_pressed(egui::Key::Escape) {
                    lib.selection.clear();
                }
```

- [ ] **Step 3: Wire the filmstrip**

In `filmstrip`, change `let Some(lib) = &self.library else { return };` to:

```rust
                let Some(lib) = &mut self.library else { return };
```

Replace the border block

```rust
                            if lib.selection.active() == Some(i) {
```

…through its closing brace with:

```rust
                            paint_selection_border(
                                ui,
                                resp.rect,
                                lib.selection.active() == Some(i),
                                lib.selection.contains(i),
                            );
```

and the click handler

```rust
                            if resp.clicked() {
                                open_index = Some(i);
                            }
```

with:

```rust
                            if resp.clicked() {
                                let mods = ui.input(|inp| inp.modifiers);
                                // Only a plain click moves the active photo, so only a
                                // plain click opens — a Cmd- or Shift-click builds the
                                // target set without triggering a decode.
                                if apply_click(&mut lib.selection, i, mods) {
                                    open_index = Some(i);
                                }
                            }
```

- [ ] **Step 4: Build and check by eye**

```bash
cargo build -p app --release
cargo clippy -p app --all-targets -- -D warnings
cargo run --release -p app
```

Open a folder and confirm: Cmd-click adds cells with a dim border, Shift-click fills a range,
the active cell keeps the bright border, Cmd+A selects all in the grid, Esc collapses back to
one, and a Cmd-click in the filmstrip does **not** switch photos.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/app/src/main.rs
git commit -m "feat(app): multi-select with Cmd/Shift-click in the grid and filmstrip"
```

---

## Task 7: The Sync button, dialog and undo wiring

**Files:** Modify `crates/app/src/main.rs`

- [ ] **Step 1: Add the modal variant and its group state**

Add to the `use` block:

```rust
use rustroom_app::sync;
```

In `enum Modal`, add before `Settings`:

```rust
    /// Choosing which groups a sync applies to the rest of the selection.
    SyncSettings,
```

In `struct RustroomApp`, add after `paste_groups`:

```rust
    /// Group selection last used for a sync, kept separate from `paste_groups` so the two
    /// dialogs do not silently reconfigure each other.
    sync_groups: EditGroups,
```

and in `RustroomApp::new`, after `paste_groups: EditGroups::default(),`:

```rust
            sync_groups: EditGroups::default(),
```

- [ ] **Step 2: Add the sync executor and the session reload**

Add these to the `impl RustroomApp` block containing `apply_edits_from`:

```rust
    /// Paths of the selected photos other than the active one — a sync's targets.
    fn sync_targets(&self) -> Vec<PathBuf> {
        let Some(lib) = &self.library else {
            return Vec::new();
        };
        lib.selection
            .others()
            .filter_map(|i| lib.entries.get(i).map(|e| e.path.clone()))
            .collect()
    }

    /// Apply the open photo's edits to the rest of the selection, as one undo step.
    fn run_sync(&mut self, groups: EditGroups) {
        let targets = self.sync_targets();
        let Some(source) = self.session.as_ref().map(|s| s.edits.clone()) else {
            return;
        };
        if targets.is_empty() {
            self.status = "Nothing to sync".to_string();
            return;
        }
        // The source is the session's *in-memory* edits, not its sidecar: you sync what you
        // are looking at, not what was last written.
        let report = sync::apply(&targets, &source, groups);
        self.status = if report.failed.is_empty() {
            format!("Synced {} photos", report.prior.len())
        } else {
            format!(
                "Synced {}, {} failed",
                report.prior.len(),
                report.failed.len()
            )
        };
        self.actions.push(ActionKind::Sync {
            source,
            groups,
            prior: report.prior,
        });
    }

    /// Reload the open photo's edits if its sidecar was just rewritten underneath it.
    ///
    /// ⚠ The active photo is never a sync *target*, so a sync cannot make the open session
    /// stale. But a user can sync, switch to a synced photo, and *then* undo — which
    /// rewrites that photo's sidecar while it is open with its own in-memory edits. Without
    /// this the canvas silently disagrees with the disk until you switch away and back.
    fn reload_if_affected(&mut self, affected: &[PathBuf]) {
        let open = self.session.as_ref().map(|s| s.path.clone());
        let Some(open) = open else { return };
        if !affected.iter().any(|p| *p == open) {
            return;
        }
        let edits = sidecar::load(&open).unwrap_or_default();
        if let Some(s) = &mut self.session {
            s.edits = edits;
            // The old history described a state that is no longer on disk.
            s.history = History::new(s.edits.clone());
            s.engine.request(
                preview_edits(&s.edits, s.crop_mode),
                s.render_max_dim(),
                s.requested_region,
            );
        }
        // Those history entries just became unreachable.
        self.actions.forget_photo_edits();
    }
```

- [ ] **Step 3: Handle the new action in undo and redo**

In `undo_action`, add a third arm before the closing brace of the `match`:

```rust
            Some(ActionKind::Sync { prior, .. }) => {
                let failed = sync::revert(&prior);
                self.status = if failed.is_empty() {
                    format!("Reverted sync of {} photos", prior.len())
                } else {
                    format!(
                        "Reverted {}, {} failed",
                        prior.len() - failed.len(),
                        failed.len()
                    )
                };
                let affected: Vec<PathBuf> = prior.into_iter().map(|(p, _)| p).collect();
                self.reload_if_affected(&affected);
            }
```

In `redo_action`, likewise:

```rust
            Some(ActionKind::Sync {
                source,
                groups,
                prior,
            }) => {
                let targets: Vec<PathBuf> = prior.iter().map(|(p, _)| p.clone()).collect();
                let report = sync::apply(&targets, &source, groups);
                self.status = format!("Re-synced {} photos", report.prior.len());
                self.reload_if_affected(&targets);
            }
```

- [ ] **Step 4: Add the toolbar button**

In `develop_toolbar`, add immediately after the `Paste…` button block:

```rust
            let targets = self.library.as_ref().map_or(0, |l| l.selection.len());
            if ui
                .add_enabled(
                    targets > 1,
                    egui::Button::new(format!("Sync {}…", targets.saturating_sub(1))),
                )
                .clicked()
            {
                self.modal = Modal::SyncSettings;
            }
```

- [ ] **Step 5: Add the dialog**

Add this method next to `paste_modal`:

```rust
    /// The "Sync…" dialog: which groups this sync applies to the rest of the selection.
    fn sync_modal(&mut self, ctx: &egui::Context) {
        let mut groups = self.sync_groups;
        let count = self.sync_targets().len();
        let mut close = false;
        let mut run = false;

        egui::Window::new("Sync settings")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Apply to {count} other selected photos:"));
                edit_group_checkboxes(ui, &mut groups);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Sync").clicked() {
                        run = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });

        self.sync_groups = groups;
        if run {
            self.run_sync(groups);
            close = true;
        }
        if close {
            self.modal = Modal::None;
        }
    }
```

Register it in `draw_modals`: add `Sync` to the local `Which` enum, map
`Modal::SyncSettings => Which::Sync` in the first match, and `Which::Sync => self.sync_modal(ctx)`
in the second.

- [ ] **Step 6: Build and verify**

```bash
cargo build -p app --release
cargo clippy -p app --all-targets -- -D warnings
cargo test --all
```

Expected: builds clean, no warnings, all tests pass.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/app/src/main.rs
git commit -m "feat(app): sync the open photo's settings to the rest of the selection"
```

---

## Task 8: Verify, then check it by hand

**Files:** none (verification only)

- [ ] **Step 1: Full workspace verification**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --all
```

Expected: all three green. Check exit codes, not just tail output.

- [ ] **Step 2: Confirm the goldens are untouched**

```bash
cargo test -p pipeline --test golden
git status --short crates/pipeline/tests/golden
```

Expected: **23 pass**, `git status` prints nothing.

- [ ] **Step 3: Launch the GUI and exercise it**

```bash
cargo run --release -p app
```

Report to the user what to look at:

1. Open a folder, open a photo, adjust some sliders.
2. In the filmstrip, **Cmd-click** two or three other photos — they gain a dim border and the
   photo on screen does **not** change.
3. **Sync 3…** → tick only Tone → Sync. The status line reports the count.
4. Open one of the targets: it has the tone, and not the other groups.
5. **Ctrl+Z** — the whole batch reverts in one step.
6. Sync onto a photo that had *no* sidecar, then undo, and confirm **no `.json` is left
   behind** next to it.
7. Sync, switch to a synced photo, then Ctrl+Z — the canvas must update rather than keep
   showing the synced look.
8. In the Library grid, **Shift-click** a range and **Cmd+A** / **Esc**.

- [ ] **Step 4: Confirm the sidecars on disk**

```bash
ls <the folder you synced> | head -20
```

Expected: a `.json` beside each synced photo.

---

## Task 9: Update `CLAUDE.md`

**Files:** Modify `CLAUDE.md`

- [ ] **Step 1: Update the crate layout**

Add the three modules to the `app/` line's lib list: `selection`, `sidecar`, `sync`.

- [ ] **Step 2: Rewrite the RESUME HERE next action**

Multi-selection and sync are done, so the next action becomes **batch export**, which reuses
`Selection` directly. Renumber the remaining Phase 2 gaps: batch export (1), texture /
clarity / dehaze (2), cross-photo undo history (3, deferred add-on).

- [ ] **Step 3: Add the status section**

Insert above **"Presets & copy/paste done"**, in the house style (what landed, the ⚠ traps,
accepted caveats):

```markdown
**Multi-selection & sync done.** App-layer only; the 23 goldens stayed byte-identical.

- **`app::selection`** — `Selection { active, set, anchor }` replaces `Library.selected:
  Option<usize>`. **⚠ The invariant is the whole design:** `active == Some(i)` implies `i` is
  selected, `active == None` implies nothing is. A sync would otherwise have a source outside
  its own selection, or none at all. A test helper asserts it after every operation.
- **⚠ Cmd-click and Shift-click never move the active photo** — only a plain click or arrow
  navigation does. Lightroom moves it on shift-click, but here that would fire a RAW decode
  mid-range-select, and it fights the order of the task: pick the source, *then* extend the
  targets. Consequences, both accepted: Cmd-clicking the active photo is a no-op, and
  `extend_to` re-inserts the active index because a preceding Cmd-click can have moved the
  anchor away from it.
- **`app::sync` decodes nothing.** Every target's edits live in a JSON sidecar, so a sync is
  `load → merge_from → save` — 200 photos is 200 tiny file writes, not 200 × ~11 s of RAW
  decoding. That single fact is why there is no progress bar, no worker thread and no
  cancellation. A failed write is collected and reported rather than aborting the batch.
- **`app::sidecar`** was extracted from `main.rs` so `sync` could reach it and so it could be
  tested at all. Its tests need **no image fixtures**: `path` only appends `.json`, so a sync
  touches nothing but sidecars.
- **⚠ `prior: Vec<(PathBuf, Option<Edits>)>` — the `Option` is load-bearing.** `None` records
  "this photo had no sidecar", so undo *deletes* the file the sync created instead of writing
  a neutral one and littering the folder.
- **⚠ `ActionKind` lost its `Eq` derive** when it gained an `Edits`: `f32` is `PartialEq`
  only. `Sync` entries are global like `PresetDelete`, so `forget_photo_edits` leaves them be.
- **⚠ The live-session hazard.** The active photo is never a sync target, so a sync cannot
  make the open session stale — but you can sync, *switch* to a synced photo, then Ctrl+Z,
  which rewrites its sidecar underneath an open session. `reload_if_affected` reloads the
  edits and resets that session's history; without it the canvas silently disagrees with disk.
- **`EditGroups` is now doing its third job** — copy/paste, presets, and sync all share one
  partition and one checkbox dialog. `sync_groups` is kept separate from `paste_groups` so
  the dialogs do not reconfigure each other.
- **Accepted caveats:** (1) Cmd-clicking the active photo does nothing. (2) No Auto-Sync.
  (3) Sync only from Develop — the source must be the open photo. (4) No progress UI or
  cancellation, justified above. (5) One undo step for the whole batch, not per photo.
  (6) Selection resets on plain navigation and on folder reload. (7) Sync writes sidecars for
  photos that had none, which is visible in the folder.
```

- [ ] **Step 4: Update the docs index**

Add `2026-08-07-multi-select-sync-design.md` to the `specs/` list and
`2026-08-07-multi-select-sync.md` to the `plans/` list.

- [ ] **Step 5: Final verification**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --all
```

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-07-multi-select-sync.md
git commit -m "docs: record multi-selection and sync"
```

---

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- The 23 golden references are byte-identical.
- In the GUI: Cmd/Shift-click build a selection in both grid and filmstrip with the active
  photo distinguishable; `Sync N…` applies the chosen groups; Ctrl+Z reverses the whole batch
  including deleting sidecars it created; undoing a sync while one of its targets is open
  updates the canvas.
- `CLAUDE.md` updated, with batch export named as the next thing to reuse `Selection`.
