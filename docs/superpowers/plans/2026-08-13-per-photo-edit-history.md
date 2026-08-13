# Per-Photo Edit History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A photo's undo stack survives switching to another photo and back, and Ctrl+Z only ever acts on the photo currently on screen.

**Architecture:** `Session` keeps owning its `History<Edits>`; a new `HistoryStash` holds histories between sessions, keyed by path, handing one back only if it still matches the sidecar. `ActionLog` stops being two anonymous stacks and becomes one list of entries with an `undone` flag, where each `PhotoEdit` names its photo and undo/redo skip entries belonging to photos that are not open.

**Tech Stack:** Rust, no new dependencies. `crates/app` (lib + bin). Tests are plain `#[cfg(test)]` modules — no image fixtures, no egui.

**Spec:** `docs/superpowers/specs/2026-08-13-per-photo-edit-history-design.md`

**Build note:** every `cargo` command in this plan needs libraw and lcms2 on the pkg-config path. Export this once per shell before starting:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** (explicit user preference). Never `git push` — the user pushes themselves.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/app/src/edit_history.rs` | **New.** `HistoryStash`: keeps a `History<Edits>` per path between sessions, and refuses to hand back one that no longer matches the sidecar. | 1 |
| `crates/app/src/lib.rs` | Add `pub mod edit_history;`. | 1 |
| `crates/app/src/action_log.rs` | `ActionKind::PhotoEdit` gains a path (task 2); the two stacks become one `Vec<Entry>` with reachability filtering (task 3); `forget_photo_edits` becomes `forget_edits_of` (task 4). | 2, 3, 4 |
| `crates/app/src/main.rs` | Construct `PhotoEdit` with a path (task 2); pass the open photo to every log operation (task 3); stash and restore histories in `finalize_session` (task 4). | 2, 3, 4 |

The tasks are ordered so **every commit compiles and every test passes**. Tasks 1–3 are new machinery and behaviour-preserving refactors; the feature turns on in task 4.

---

## Task 1: `HistoryStash` — where a history waits between sessions

Self-contained: nothing depends on this module until task 4, so it can be written and tested in isolation.

**Files:**
- Create: `crates/app/src/edit_history.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Register the module**

In `crates/app/src/lib.rs`, add the line in alphabetical position — it goes between `edit_clipboard` and `export_presets`:

```rust
pub mod edit_clipboard;
pub mod edit_history;
pub mod export_presets;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/app/src/edit_history.rs` containing **only** the imports and the test module for now, so the tests fail to compile against a type that does not exist yet:

```rust
//! Per-photo undo histories that outlive their sessions.

use crate::history::History;
use edit_model::Edits;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;

    fn edits(exposure: f32) -> Edits {
        Edits {
            exposure,
            ..Default::default()
        }
    }

    /// A history holding one recorded step: from `edits(0.0)` to `edits(1.0)`.
    fn edited() -> History<Edits> {
        let mut h = History::new(edits(0.0));
        assert!(h.push(edits(1.0)), "the fixture must record a real step");
        h
    }

    #[test]
    fn a_stashed_history_comes_back_intact() {
        let mut stash = HistoryStash::default();
        stash.stash(PathBuf::from("/a.RAF"), edited());

        let mut back = stash
            .take_matching(Path::new("/a.RAF"), &edits(1.0))
            .expect("the sidecar still matches, so the history must come back");
        assert!(back.undo(), "the undo stack must have survived the round trip");
        assert_eq!(*back.get(), edits(0.0));
        assert!(back.can_redo(), "and so must the redo stack");
    }

    #[test]
    fn a_history_whose_photo_changed_underneath_it_is_discarded() {
        // ⚠ The property the module hangs on. A sync or a paste into a multi-selection
        // rewrites the sidecars of photos that are *not open*, so the stack left behind
        // describes a state that is no longer on disk.
        let mut stash = HistoryStash::default();
        stash.stash(PathBuf::from("/a.RAF"), edited());
        assert!(stash
            .take_matching(Path::new("/a.RAF"), &edits(9.0))
            .is_none());
    }

    #[test]
    fn a_mismatched_history_is_consumed_rather_than_left_behind() {
        let mut stash = HistoryStash::default();
        stash.stash(PathBuf::from("/a.RAF"), edited());
        stash.take_matching(Path::new("/a.RAF"), &edits(9.0));
        assert!(stash.is_empty(), "it will not match later either");
    }

    #[test]
    fn an_unknown_photo_has_no_history() {
        let mut stash = HistoryStash::default();
        assert!(stash
            .take_matching(Path::new("/never-seen.RAF"), &edits(0.0))
            .is_none());
    }

    #[test]
    fn a_history_with_nothing_to_undo_is_not_stashed() {
        let mut stash = HistoryStash::default();
        stash.stash(PathBuf::from("/a.RAF"), History::new(edits(0.0)));
        assert!(
            stash.is_empty(),
            "it carries nothing a fresh History::new would not"
        );
    }

    #[test]
    fn stashing_an_empty_history_replaces_an_older_one() {
        // Otherwise "Reset all" on a photo visited earlier would resurrect the old stack
        // on return, with steps that no longer lead anywhere.
        let mut stash = HistoryStash::default();
        stash.stash(PathBuf::from("/a.RAF"), edited());
        stash.stash(PathBuf::from("/a.RAF"), History::new(edits(1.0)));
        assert!(stash.is_empty());
    }

    #[test]
    fn photos_keep_separate_histories() {
        let mut stash = HistoryStash::default();
        stash.stash(PathBuf::from("/a.RAF"), edited());
        stash.stash(PathBuf::from("/b.RAF"), edited());
        assert_eq!(stash.len(), 2);
        assert!(stash
            .take_matching(Path::new("/a.RAF"), &edits(1.0))
            .is_some());
        assert!(stash
            .take_matching(Path::new("/b.RAF"), &edits(1.0))
            .is_some());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p app --lib edit_history
```

Expected: **compile error**, `cannot find type HistoryStash in this scope` (repeated for each test).

- [ ] **Step 4: Write the implementation**

Insert this into `crates/app/src/edit_history.rs` **between the `use` lines and the `#[cfg(test)]` module**, and extend the module doc comment at the top of the file so it reads:

```rust
//! Per-photo undo histories that outlive their sessions.
//!
//! `History<Edits>` lives on `Session`, and a `Session` dies the moment another photo is
//! opened — so without this, editing photo A, moving to B and coming back leaves A with no
//! undo stack, even though its *edits* were restored faithfully from the sidecar. This is
//! where a history waits in between.
//!
//! Deliberately a **stash, not a store**: while a photo is open its `Session` still owns the
//! authoritative history. Making this authoritative instead would force every
//! `s.history.push` call site — each already holding `&mut self` through `self.session` — to
//! take a second simultaneous `&mut self` borrow, for no behavioural difference.
```

```rust
/// Undo histories for photos that are not currently open, keyed by source path.
///
/// Unbounded on purpose. A `History<Edits>` is a `Vec` of a few-hundred-byte struct — 500
/// photos at 20 steps each is roughly 4 MB — so `BufferCache`'s cap of 3, which exists
/// because a decoded buffer is 78–313 MB, has no bearing here.
#[derive(Default)]
pub struct HistoryStash {
    map: HashMap<PathBuf, History<Edits>>,
}

impl HistoryStash {
    /// Keep `history` for `path`.
    ///
    /// A history with nothing to undo or redo is *removed* rather than stored: it carries
    /// nothing a fresh `History::new` would not, and dropping it keeps the map growing per
    /// photo actually edited rather than per photo visited. Removing rather than ignoring
    /// matters — otherwise resetting a photo whose stack was stashed earlier would bring the
    /// stale stack back on return.
    pub fn stash(&mut self, path: PathBuf, history: History<Edits>) {
        if !history.can_undo() && !history.can_redo() {
            self.map.remove(&path);
            return;
        }
        self.map.insert(path, history);
    }

    /// Take `path`'s history back, but **only if it still describes `on_disk`**.
    ///
    /// ⚠ This one check is what makes the feature safe against every writer of another
    /// photo's sidecar — a sync, a paste into a multi-selection, a preset click, and the undo
    /// or redo of any of them. Rather than obliging each of those, and every future one, to
    /// remember to invalidate, a stashed history that no longer matches disk is discarded
    /// here, at the single moment it would be used.
    ///
    /// `Edits` is `PartialEq` over `f32`, so a `NaN` compares unequal and this fails toward
    /// "discard and start fresh" — the safe direction, exactly as `PrefixKey` fails toward a
    /// cache miss.
    ///
    /// The entry is consumed either way: a history that did not match will not match later.
    pub fn take_matching(&mut self, path: &Path, on_disk: &Edits) -> Option<History<Edits>> {
        let history = self.map.remove(path)?;
        (history.get() == on_disk).then_some(history)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app --lib edit_history
```

Expected: `test result: ok. 7 passed`.

- [ ] **Step 6: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/edit_history.rs crates/app/src/lib.rs
git commit -m "feat(app): a stash for per-photo edit histories"
```

---

## Task 2: `PhotoEdit` names its photo

Pure refactor — behaviour is unchanged, because `forget_photo_edits()` still sweeps every photo edit on every switch. The point is to give the entry the identity it needs in task 3.

**Files:**
- Modify: `crates/app/src/action_log.rs`
- Modify: `crates/app/src/main.rs:483`, `main.rs:2253`

- [ ] **Step 1: Change the variant**

In `crates/app/src/action_log.rs`, replace the `PhotoEdit` variant and its doc comment:

```rust
    /// A step was recorded in the history of the photo at `path`.
    ///
    /// ⚠ The path is what makes the entry mean anything after a photo switch. It used to be
    /// a unit variant, and an anonymous entry can only say "pop whatever history is current"
    /// — which after a switch is a *different photo's*. That is the sole reason every such
    /// entry had to be destroyed whenever the session changed.
    PhotoEdit { path: PathBuf },
```

`PathBuf` is already imported in this file (`use std::path::PathBuf;`).

- [ ] **Step 2: Update the sweep to match the new shape**

In the same file, `fn sweep` matches the variant twice. Change both to the struct-variant form:

```rust
    fn sweep(stack: &mut Vec<ActionKind>) {
        for kind in stack.iter_mut() {
            if let ActionKind::Batch(parts) = kind {
                parts.retain(|p| !matches!(p, ActionKind::PhotoEdit { .. }));
            }
        }
        stack.retain(|kind| match kind {
            ActionKind::PhotoEdit { .. } => false,
            ActionKind::Batch(parts) => !parts.is_empty(),
            _ => true,
        });
    }
```

- [ ] **Step 3: Add a test helper and update the existing tests**

In `action_log.rs`'s `mod tests`, add this helper next to the existing `fn del`:

```rust
    fn edit(path: &str) -> ActionKind {
        ActionKind::PhotoEdit {
            path: PathBuf::from(path),
        }
    }
```

Then replace every bare `ActionKind::PhotoEdit` in the test module with `edit("/a.RAF")`. The occurrences are in: `undo_returns_actions_most_recent_first`, `a_preset_delete_survives_an_intervening_photo_edit`, `redo_replays_in_the_original_order`, `a_new_action_clears_the_redo_stack`, `a_sync_survives_a_photo_switch`, `a_batch_is_one_step`, `forgetting_photo_edits_reaches_inside_a_batch`, `a_batch_of_only_photo_edits_is_dropped_entirely`, `a_sync_orders_with_everything_else`, `forgetting_photo_edits_keeps_deletions_in_order`, and `forgetting_photo_edits_cleans_the_redo_stack_too`.

- [ ] **Step 4: Run the action_log tests to verify they pass**

```bash
cargo test -p app --lib action_log
```

Expected: all pass. These are the pre-existing tests — the point of this step is that a refactor did not change their outcomes.

- [ ] **Step 5: Update the two construction sites in `main.rs`**

In `apply_to_selection` (around `main.rs:473`), capture the open path before the block that needs it and use it. Replace:

```rust
        let targets = self.sync_targets();
        let into_session = self.open_photo_is_selected();
        let mut parts: Vec<ActionKind> = Vec::new();
        let mut touched = 0usize;
        let mut failed = 0usize;

        if into_session {
            touched += 1;
            if self.apply_edits_to_session(src, groups) {
                parts.push(ActionKind::PhotoEdit);
            }
        }
```

with:

```rust
        let targets = self.sync_targets();
        let into_session = self.open_photo_is_selected();
        let open = self.session.as_ref().map(|s| s.path.clone());
        let mut parts: Vec<ActionKind> = Vec::new();
        let mut touched = 0usize;
        let mut failed = 0usize;

        if into_session {
            touched += 1;
            if self.apply_edits_to_session(src, groups) {
                // `into_session` is only true when a session exists, so the `if let` always
                // takes its branch — it is here because the compiler cannot know that.
                if let Some(path) = open.clone() {
                    parts.push(ActionKind::PhotoEdit { path });
                }
            }
        }
```

In `develop_body` (around `main.rs:2252`), replace:

```rust
        if photo_edited {
            self.actions.push(ActionKind::PhotoEdit);
        }
```

with:

```rust
        if photo_edited {
            if let Some(path) = self.session.as_ref().map(|s| s.path.clone()) {
                self.actions.push(ActionKind::PhotoEdit { path });
            }
        }
```

- [ ] **Step 6: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/action_log.rs crates/app/src/main.rs
git commit -m "refactor(app): a photo edit records which photo it belongs to"
```

---

## Task 3: One list with an `undone` flag, filtered by the open photo

This is the substantive change to `ActionLog`. `forget_photo_edits()` is deliberately left in place and still sweeps everything, so the *observable* behaviour after this task is still today's — the reachability machinery is dormant until task 4 stops the sweeping. That is what makes this task safe to land on its own.

**Files:**
- Modify: `crates/app/src/action_log.rs`
- Modify: `crates/app/src/main.rs` — the log's six call sites

- [ ] **Step 1: Write the failing tests**

Add these to `action_log.rs`'s `mod tests`, after the existing helpers. Add the helper functions first:

```rust
    fn a() -> Option<&'static Path> {
        Some(Path::new("/a.RAF"))
    }

    fn b() -> Option<&'static Path> {
        Some(Path::new("/b.RAF"))
    }
```

and add `use std::path::Path;` to the test module's imports (`super::*` already brings in `PathBuf`).

```rust
    #[test]
    fn an_edit_to_another_photo_is_not_undoable_here() {
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        assert!(!log.can_undo(b()));
        assert_eq!(log.undo(b()), None);
    }

    #[test]
    fn returning_to_a_photo_makes_its_edits_undoable_again() {
        // The headline property, and the one that fails against the old anonymous entries:
        // A's step is merely out of reach from B, not destroyed.
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        assert!(!log.can_undo(b()), "not reachable from B");
        assert_eq!(
            log.undo(a()),
            Some(edit("/a.RAF")),
            "but still there on return"
        );
    }

    #[test]
    fn a_new_action_clears_only_the_redo_it_can_reach() {
        // Photo A has an undone step waiting. Editing B invalidates nothing about A.
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.undo(a());
        log.push(edit("/b.RAF"), b());
        assert_eq!(
            log.redo(a()),
            Some(edit("/a.RAF")),
            "B's edit cannot invalidate A's redo path"
        );
    }

    #[test]
    fn a_new_action_clears_its_own_photos_redo() {
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.undo(a());
        log.push(edit("/a.RAF"), a());
        assert!(!log.can_redo(a()), "the classic rule still holds within a photo");
    }

    #[test]
    fn a_new_action_clears_the_global_redo_from_any_photo() {
        // A preset deletion is reachable everywhere, so an edit to any photo invalidates it.
        let mut log = ActionLog::default();
        log.push(del("P"), a());
        log.undo(a());
        log.push(edit("/b.RAF"), b());
        assert!(!log.can_redo(a()));
        assert_eq!(log.redo(b()), None);
    }

    #[test]
    fn a_batch_waits_until_every_member_is_reachable() {
        let batch = ActionKind::Batch(vec![edit("/a.RAF"), sync_entry()]);
        let mut log = ActionLog::default();
        log.push(batch.clone(), a());
        assert!(!log.can_undo(b()), "half of it belongs to A");
        assert_eq!(log.undo(a()), Some(batch));
    }

    #[test]
    fn a_batch_survives_a_visit_to_another_photo_intact() {
        // ⚠ The wart this rework removes. The old sweep tore the `PhotoEdit` member out of
        // every batch on every switch, permanently stranding the sidecar half of a paste.
        let batch = ActionKind::Batch(vec![edit("/a.RAF"), sync_entry()]);
        let mut log = ActionLog::default();
        log.push(batch.clone(), a());
        assert!(!log.can_undo(b()));
        assert_eq!(log.undo(a()), Some(batch), "intact, not half destroyed");
    }

    #[test]
    fn undo_and_redo_are_inverses_with_two_photos_interleaved() {
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.push(del("P"), a());
        log.push(edit("/b.RAF"), b());

        // On B: B's own edit is newest, then the global deletion. A's edit is out of reach.
        assert_eq!(log.undo(b()), Some(edit("/b.RAF")));
        assert_eq!(log.undo(b()), Some(del("P")));
        assert_eq!(log.undo(b()), None, "A's edit is not reachable from B");

        // Redo replays in the order the entries were undone.
        assert_eq!(log.redo(b()), Some(del("P")));
        assert_eq!(log.redo(b()), Some(edit("/b.RAF")));
        assert_eq!(log.redo(b()), None);
    }

    #[test]
    fn with_no_photo_open_only_global_actions_are_reachable() {
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.push(del("P"), a());
        assert_eq!(log.undo(None), Some(del("P")));
        assert_eq!(log.undo(None), None);
    }
```

Every pre-existing test in this module also needs its calls updated to the new signatures — `log.push(k)` becomes `log.push(k, a())`, and `log.undo()` / `log.redo()` / `log.can_undo()` / `log.can_redo()` / `log.peek_undo()` each take `a()`. Do that as part of this step; the assertions themselves do not change, which is the regression evidence that a single open photo behaves exactly as before.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --lib action_log
```

Expected: **compile errors** — `this method takes 2 arguments but 1 argument was supplied` for `push`, and `this method takes 1 argument but 0 arguments were supplied` for `undo`/`redo`/`can_undo`/`can_redo`/`peek_undo`.

- [ ] **Step 3: Rewrite `ActionLog`**

In `crates/app/src/action_log.rs`, add `Path` to the imports:

```rust
use std::path::{Path, PathBuf};
```

Replace everything from the `/// Undo/redo ordering over [`ActionKind`]` doc comment down to the closing brace before `#[cfg(test)]` — that is, the `ActionLog` struct and its whole `impl` block, including `forget_photo_edits` and the private `sweep` — with the following. `forget_photo_edits` survives with the same behaviour but is inlined over the single list; `sweep` disappears, because with one list there is no second stack to sweep.

```rust
/// One entry in the log, plus whether it is currently undone.
///
/// ⚠ A flag rather than two stacks. Filtering two stacks by reachability means taking an
/// entry out of the *middle* of one and pushing it back on the *top* of the other, which
/// scrambles chronological order across photos — observable as a wrong undo order the next
/// time you switch, and repairable only with a sequence number on every entry. Marking in
/// place makes the ordering intrinsic instead.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    kind: ActionKind,
    undone: bool,
}

/// Whether `kind` can be undone or redone while `open` is the photo on screen.
///
/// A preset deletion or a sync never touches a live session, so it is reachable from
/// anywhere. A photo edit is reachable only from its own photo: undoing it from elsewhere
/// would either change an image the user cannot see or yank the view to it, and both were
/// rejected.
///
/// ⚠ A batch is reachable only when **every** member is, so one Ctrl+V stays one Ctrl+Z. It
/// waits, intact, until you return to the photo it touched.
fn reachable(kind: &ActionKind, open: Option<&Path>) -> bool {
    match kind {
        ActionKind::PhotoEdit { path } => open == Some(path.as_path()),
        ActionKind::PresetDelete { .. } | ActionKind::Sync(_) => true,
        ActionKind::Batch(parts) => parts.iter().all(|p| reachable(p, open)),
    }
}

/// Undo/redo ordering over [`ActionKind`], scoped to the photo on screen.
#[derive(Debug, Default)]
pub struct ActionLog {
    entries: Vec<Entry>,
}

impl ActionLog {
    /// Record a new action.
    ///
    /// Clears the redo path **this action can reach** — edits to the open photo, and global
    /// actions. Another photo's undone step is left alone: nothing done here invalidates it,
    /// and discarding it would reintroduce "your stack is gone" one level down.
    pub fn push(&mut self, kind: ActionKind, open: Option<&Path>) {
        self.entries
            .retain(|e| !(e.undone && reachable(&e.kind, open)));
        self.entries.push(Entry { kind, undone: false });
    }

    pub fn can_undo(&self, open: Option<&Path>) -> bool {
        self.undo_index(open).is_some()
    }

    pub fn can_redo(&self, open: Option<&Path>) -> bool {
        self.redo_index(open).is_some()
    }

    /// What the next undo would reverse, without performing it — for a button label.
    pub fn peek_undo(&self, open: Option<&Path>) -> Option<&ActionKind> {
        Some(&self.entries[self.undo_index(open)?].kind)
    }

    /// Take the most recent reachable action for undoing. The caller performs the reversal.
    pub fn undo(&mut self, open: Option<&Path>) -> Option<ActionKind> {
        let i = self.undo_index(open)?;
        self.entries[i].undone = true;
        Some(self.entries[i].kind.clone())
    }

    /// Take the most recently undone reachable action for redoing.
    pub fn redo(&mut self, open: Option<&Path>) -> Option<ActionKind> {
        let i = self.redo_index(open)?;
        self.entries[i].undone = false;
        Some(self.entries[i].kind.clone())
    }

    /// The last reachable entry still in force.
    fn undo_index(&self, open: Option<&Path>) -> Option<usize> {
        self.entries
            .iter()
            .rposition(|e| !e.undone && reachable(&e.kind, open))
    }

    /// The first reachable entry that has been undone.
    ///
    /// ⚠ *First*, not last, and that is what makes redo the exact inverse of undo. Undo
    /// always takes the last reachable entry in force, so within any one photo's subsequence
    /// the entries in force form a prefix and the undone ones a suffix — the first undone one
    /// is the boundary, which is the one undone most recently.
    fn redo_index(&self, open: Option<&Path>) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.undone && reachable(&e.kind, open))
    }

    /// Drop every photo edit, keeping global actions in their relative order.
    ///
    /// ⚠ Called when the session changes. `Session::history` is discarded on a photo switch,
    /// so a `PhotoEdit` entry that outlived its session would make an undo pop a *different*
    /// photo's history — silently reverting an edit the user never made here.
    pub fn forget_photo_edits(&mut self) {
        for e in self.entries.iter_mut() {
            if let ActionKind::Batch(parts) = &mut e.kind {
                parts.retain(|p| !matches!(p, ActionKind::PhotoEdit { .. }));
            }
        }
        self.entries.retain(|e| match &e.kind {
            ActionKind::PhotoEdit { .. } => false,
            ActionKind::Batch(parts) => !parts.is_empty(),
            _ => true,
        });
    }
}
```

- [ ] **Step 4: Update the seven call sites in `main.rs`**

Add this helper method to `impl RustroomApp`, directly above `fn persist_current_session` (around `main.rs:404`):

```rust
    /// The open photo, which is what the action log's photo-edit entries must match to be
    /// undoable right now.
    ///
    /// ⚠ Returns an owned `PathBuf` rather than a borrow: every caller passes it *into* a
    /// `&mut self.actions` method, and a borrow of `self` living across that call does not
    /// compile. One clone per user gesture is not worth fighting the borrow checker over.
    fn open_photo(&self) -> Option<PathBuf> {
        self.session.as_ref().map(|s| s.path.clone())
    }
```

Then, one site at a time:

**(a) `undo_action` / `redo_action`** (around `main.rs:692` and `main.rs:736`):

```rust
    /// Undo the most recent action, whichever kind it was.
    fn undo_action(&mut self) {
        let open = self.open_photo();
        if let Some(kind) = self.actions.undo(open.as_deref()) {
            self.undo_one(kind);
        }
    }
```

```rust
    /// Redo the most recently undone action.
    fn redo_action(&mut self) {
        let open = self.open_photo();
        if let Some(kind) = self.actions.redo(open.as_deref()) {
            self.redo_one(kind);
        }
    }
```

**(b) `apply_to_selection`** (around `main.rs:499`), the `match parts.len()` block:

```rust
        // A lone action does not need a wrapper — keep the log readable.
        match parts.len() {
            0 => {}
            1 => self.actions.push(parts.remove(0), open.as_deref()),
            _ => self.actions.push(ActionKind::Batch(parts), open.as_deref()),
        }
```

(`open` is the local added in task 2, step 5.)

**(c) `run_sync`** (around `main.rs:530`), which pushes an `ActionKind::Sync`. Capture the open path first:

```rust
        let open = self.open_photo();
        self.actions.push(
            ActionKind::Sync(Box::new(SyncAction {
```

and close that call with `, open.as_deref());` in place of the existing `);`.

**(d) `develop_toolbar`** (around `main.rs:1117`):

```rust
        let open = self.open_photo();
        let undo_label = match self.actions.peek_undo(open.as_deref()) {
            Some(ActionKind::PresetDelete { name }) => format!("Undo delete “{name}”"),
            _ => "Undo".to_string(),
        };
        let undo_click = ui
            .add_enabled(
                self.actions.can_undo(open.as_deref()),
                egui::Button::new(undo_label),
            )
            .clicked();
        let redo_click = ui
            .add_enabled(
                self.actions.can_redo(open.as_deref()),
                egui::Button::new("Redo"),
            )
            .clicked();
```

**(e) `preset_panel`** (around `main.rs:1223`):

```rust
        let open = self.open_photo();
        let undo_name = match self.actions.peek_undo(open.as_deref()) {
            Some(ActionKind::PresetDelete { name }) => Some(name.clone()),
            _ => None,
        };
```

**(f) the preset-delete push** (around `main.rs:1297`). Capture the open path immediately before it and pass it:

```rust
                let open = self.open_photo();
                self.actions.push(
                    ActionKind::PresetDelete {
```

closing that call with `, open.as_deref());`.

**(g) the `photo_edited` push** in `develop_body` (around `main.rs:2252`), which task 2 left as:

```rust
        if photo_edited {
            if let Some(path) = self.session.as_ref().map(|s| s.path.clone()) {
                // ⚠ `Some(path.as_path())`, not `Some(&path)`: the parameter is
                // `Option<&Path>` and deref coercion does not reach inside `Some`.
                self.actions.push(
                    ActionKind::PhotoEdit { path: path.clone() },
                    Some(path.as_path()),
                );
            }
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app --lib action_log
```

Expected: all pass, including the nine new tests.

- [ ] **Step 6: Prove the new tests can fail**

Temporarily break reachability by making everything reachable — in `fn reachable`, change the `PhotoEdit` arm to `ActionKind::PhotoEdit { .. } => true,` — then run:

```bash
cargo test -p app --lib action_log
```

Expected: `an_edit_to_another_photo_is_not_undoable_here`, `a_new_action_clears_only_the_redo_it_can_reach`, `a_batch_waits_until_every_member_is_reachable`, `a_batch_survives_a_visit_to_another_photo_intact`, `undo_and_redo_are_inverses_with_two_photos_interleaved` and `with_no_photo_open_only_global_actions_are_reachable` all **fail**. Restore the arm to `open == Some(path.as_path())` and re-run to confirm green again.

- [ ] **Step 7: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, all tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/action_log.rs crates/app/src/main.rs
git commit -m "feat(app): scope undo and redo to the photo on screen"
```

---

## Task 4: Turn it on — stash on switch, restore on return

**Files:**
- Modify: `crates/app/src/action_log.rs` — `forget_photo_edits` becomes `forget_edits_of`
- Modify: `crates/app/src/main.rs` — the `histories` field, `finalize_session`, `reload_if_affected`

- [ ] **Step 1: Write the failing tests for the targeted sweep**

In `action_log.rs`'s `mod tests`, **delete** these three now-obsolete tests: `forgetting_photo_edits_keeps_deletions_in_order`, `forgetting_photo_edits_cleans_the_redo_stack_too`, and `a_batch_of_only_photo_edits_is_dropped_entirely`. Replace `forgetting_photo_edits_reaches_inside_a_batch` and add the rest:

```rust
    #[test]
    fn forgetting_one_photos_edits_leaves_other_photos_alone() {
        // ⚠ The whole reason the sweep is targeted now: photo B's stack is still in the
        // stash, so its entries must survive photo A's being discarded.
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.push(edit("/b.RAF"), b());
        log.forget_edits_of(Path::new("/a.RAF"));

        assert_eq!(log.undo(a()), None, "A's entry is gone");
        assert_eq!(log.undo(b()), Some(edit("/b.RAF")), "B's is not");
    }

    #[test]
    fn forgetting_a_photos_edits_reaches_inside_a_batch() {
        // The member belongs to a history that was just discarded, so undoing it would pop
        // steps that history no longer holds. Dropping the whole batch instead would strand
        // the sidecar half of a paste with no way to undo it, so the member goes and the
        // rest stays.
        let mut log = ActionLog::default();
        log.push(
            ActionKind::Batch(vec![edit("/a.RAF"), sync_entry()]),
            a(),
        );
        log.forget_edits_of(Path::new("/a.RAF"));
        assert_eq!(log.undo(a()), Some(ActionKind::Batch(vec![sync_entry()])));
    }

    #[test]
    fn a_batch_of_only_that_photos_edits_is_dropped_entirely() {
        let mut log = ActionLog::default();
        log.push(
            ActionKind::Batch(vec![edit("/a.RAF"), edit("/a.RAF")]),
            a(),
        );
        log.forget_edits_of(Path::new("/a.RAF"));
        assert!(!log.can_undo(a()), "an empty batch is not an undo step");
    }

    #[test]
    fn forgetting_a_photos_edits_clears_its_undone_entries_too() {
        // Otherwise a redo after the discard would replay into a history that is gone.
        let mut log = ActionLog::default();
        log.push(edit("/a.RAF"), a());
        log.undo(a());
        log.forget_edits_of(Path::new("/a.RAF"));
        assert!(!log.can_redo(a()));
    }

    #[test]
    fn forgetting_a_photos_edits_keeps_global_actions_in_order() {
        let mut log = ActionLog::default();
        log.push(del("A"), a());
        log.push(edit("/a.RAF"), a());
        log.push(del("B"), a());
        log.forget_edits_of(Path::new("/a.RAF"));

        assert_eq!(log.undo(a()), Some(del("B")));
        assert_eq!(log.undo(a()), Some(del("A")));
        assert!(!log.can_undo(a()));
    }
```

Also update `a_sync_survives_a_photo_switch`, which calls `forget_photo_edits()`:

```rust
    #[test]
    fn a_sync_survives_a_photo_switch() {
        // A sync touches other photos' sidecars, never the live session, so it is global in
        // exactly the way a preset deletion is.
        let mut log = ActionLog::default();
        log.push(sync_entry(), a());
        log.push(edit("/a.RAF"), a());
        log.forget_edits_of(Path::new("/a.RAF"));
        assert_eq!(log.undo(a()), Some(sync_entry()));
        assert!(!log.can_undo(a()));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --lib action_log
```

Expected: **compile error**, `no method named forget_edits_of found for struct ActionLog`.

- [ ] **Step 3: Make the sweep targeted**

In `action_log.rs`, replace `forget_photo_edits` with:

```rust
    /// Drop the edits of one photo, keeping every other photo's and all global actions.
    ///
    /// ⚠ Called only when that photo's `History<Edits>` has actually been discarded — when a
    /// sync rewrote the sidecar under a live session, or when `HistoryStash` refused to hand
    /// a stale stack back. An entry left pointing at a history that no longer holds its steps
    /// would make an undo do nothing at all, silently.
    ///
    /// A batch keeps its other members: stranding the sidecar half of a paste with no way to
    /// undo it would be worse than losing the session half, which is unreachable anyway. A
    /// batch left with nothing is not an undo step and goes.
    pub fn forget_edits_of(&mut self, path: &Path) {
        let mine = |k: &ActionKind| matches!(k, ActionKind::PhotoEdit { path: p } if p == path);
        for e in self.entries.iter_mut() {
            if let ActionKind::Batch(parts) = &mut e.kind {
                parts.retain(|p| !mine(p));
            }
        }
        self.entries.retain(|e| match &e.kind {
            ActionKind::Batch(parts) => !parts.is_empty(),
            k => !mine(k),
        });
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app --lib action_log
```

Expected: all pass.

- [ ] **Step 5: Add the stash to `RustroomApp`**

In `crates/app/src/main.rs`, add the import next to the others:

```rust
use rustroom_app::edit_history::HistoryStash;
```

Add the field to `struct RustroomApp`, directly after `cache: BufferCache,`:

```rust
    /// Undo stacks for photos that are not open, so switching away and back does not throw
    /// one away. Unlike `cache`, which is capped at 3 because it holds decoded buffers, this
    /// is unbounded — it holds `Edits`, not pixels.
    histories: HistoryStash,
```

and initialise it in `RustroomApp::new`, directly after `cache: BufferCache::new(BUFFER_CACHE_CAP),`:

```rust
            histories: HistoryStash::default(),
```

- [ ] **Step 6: Stash and restore in `finalize_session`**

Replace the opening of `fn finalize_session` (around `main.rs:877`) — the two lines from the signature through `let edits = ...` — with:

```rust
    fn finalize_session(&mut self, ctx: &egui::Context, path: PathBuf, decoded: Arc<Decoded>) {
        // ⚠ The only place `self.session` is ever assigned, and so the only choke point the
        // stash needs: the outgoing history is put away here whether the switch came from a
        // cache hit or from a finished background decode.
        if let Some(old) = self.session.take() {
            self.histories.stash(old.path, old.history);
        }

        let edits = sidecar::load(&path).unwrap_or_default();
        let history = match self.histories.take_matching(&path, &edits) {
            Some(h) => h,
            None => {
                // Either this photo has no stack, or the one it had describes a state no
                // longer on disk — a sync or a paste rewrote its sidecar while it was closed.
                // Any log entry pointing at that stack would now pop steps it does not hold.
                self.actions.forget_edits_of(&path);
                History::new(edits.clone())
            }
        };
```

Then in the `Session { .. }` literal further down, replace:

```rust
            history: History::new(edits),
```

with:

```rust
            history,
```

and delete the `forget_photo_edits` call and its comment near the end of the function:

```rust
        // ⚠ The previous session's `History<Edits>` just went away with it, so any
        // `PhotoEdit` still in the log would make an undo pop *this* photo's history and
        // silently revert an edit the user never made here. Preset deletions are global
        // and stay.
        self.actions.forget_photo_edits();
```

Finally, `edits: edits.clone(),` in the `Session { .. }` literal is now the *last* use of `edits`, so the clone is redundant — change that line to the shorthand:

```rust
            edits,
```

The remaining `edits.clone()` calls, in `engine.request(..)` and in the `None` branch above, both precede it and must stay.

- [ ] **Step 7: Point `reload_if_affected` at the targeted sweep**

In `fn reload_if_affected` (around `main.rs:658`), replace the last two lines of the function:

```rust
        // Those history entries just became unreachable.
        self.actions.forget_photo_edits();
```

with:

```rust
        // That photo's history was just replaced, so its entries point at steps it no longer
        // holds. Every other photo's stack is untouched and stays.
        self.actions.forget_edits_of(&open);
```

- [ ] **Step 8: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, all tests pass. If clippy reports `forget_photo_edits` as dead code, it means step 3 left the old method behind — delete it.

- [ ] **Step 9: Commit**

```bash
git add crates/app/src/action_log.rs crates/app/src/main.rs
git commit -m "feat(app): a photo's undo stack survives a switch away and back"
```

---

## Task 5: Verify in the running app

**Files:** none — this is a manual check the tests cannot make.

- [ ] **Step 1: Launch the GUI**

```bash
cargo run --release -p app
```

- [ ] **Step 2: Walk the headline scenario**

Open a folder with at least three photos. Then:

1. On photo A, move Exposure twice. The Undo button is enabled.
2. Arrow to photo B. Undo is **disabled** (nothing of B's to undo yet).
3. Move Contrast on B once, then Ctrl+Z. B returns to neutral.
4. Arrow back to A. Undo is **enabled again**; Ctrl+Z twice walks A back through both exposure steps, and Ctrl+Shift+Z twice walks forward again.

Before this change, step 4's Undo would have been dead.

- [ ] **Step 3: Check the redo scoping**

On A: make an edit, Ctrl+Z it (redo now pending). Arrow to B, edit B. Arrow back to A — Ctrl+Shift+Z must still restore A's undone step.

- [ ] **Step 4: Check that a sync invalidates the right stack**

Open A, edit it, switch to B. In the Library grid select B and C, return to Develop on B, and sync B's settings to C. Now open C: its edits are the synced ones and its Undo is inert for anything from before the sync — the sync itself is still undoable as a global action from anywhere.

- [ ] **Step 5: Check a preset deletion still interleaves**

Delete a preset, move a slider on the open photo, press Ctrl+Z twice: the slider reverts, then the preset comes back. The Undo button's label reads "Undo delete '<name>'" on the second press.

- [ ] **Step 6: Update `CLAUDE.md`**

Add a section for this feature in the "Current status" block, following the house style of the sections already there: what landed, the load-bearing decisions (undo scoped to the open photo; one `undone` flag rather than two filtered stacks; the single `take_matching` check subsuming every invalidation site), the batch wart it removes, and the accepted caveats from the spec. Move the **▶ NEXT ACTION** on to the next item in the "What is left" list — `--format` on the CLI's `render` — and add this spec and plan to the docs list at the bottom.

- [ ] **Step 7: Commit the documentation**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-13-per-photo-edit-history.md
git commit -m "docs: record the per-photo edit history work"
```
