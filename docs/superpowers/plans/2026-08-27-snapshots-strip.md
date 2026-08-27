# Snapshots, plan 2 of 2: the strip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A resizable, always-visible snapshot strip pinned to the bottom of the Develop panel: take one with a button, click a row to restore it, right-click to rename, delete or promote it to a virtual copy.

**Architecture:** A new bin module `crates/app/src/develop/panel/snapshots.rs` draws an `egui::Panel::bottom` **inside** the right panel, allocated *before* its `ScrollArea` so it is pinned and the sliders scroll behind it. `Session` caches the open copy's snapshot list — the same thing `Session::name` already does for the copy name — so the panel body never reads the sidecar. Every mutation goes through a method on `RustroomApp` that writes the sidecar, updates the cache, and pushes the undo action plan 1 added.

**Tech Stack:** Rust, egui 0.35, `chrono` for the timestamp text.

**Read first:** `docs/superpowers/specs/2026-08-27-snapshots-design.md`, and `docs/superpowers/plans/2026-08-27-snapshots-storage.md`, which must be complete.

**Before you start:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **Commit convention: no `Co-Authored-By` trailer.** ⚠ **Never push.** You are on branch `snapshots`.

⚠⚠ **THE BIN-CRATE DEAD-CODE TRAP APPLIES TO EVERY TASK HERE.** `develop/panel/*`, `theme` and `main.rs` are **binary** modules: a `pub fn` with no caller is dead code and `clippy -D warnings` rejects it. CLAUDE.md records nine instances of this. **Tasks 2 and 3 must land as one commit**, and so must Task 5's command with its menu entry.

⚠⚠ **THE CANVAS AND PANEL ARE THE LEAST-TESTED CODE IN THIS PROJECT.** CLAUDE.md records eleven separate occasions where a GUI defect was found by using the app and not by a test. Task 6's headless tests are not optional garnish; they are the only thing standing between this feature and that pattern. **Launch the GUI when the strip first draws** and again at the end.

---

### Task 1: `Session` caches the open copy's snapshots

**Files:**
- Modify: `crates/app/src/main.rs` (the `Session` struct; `finalize_session` around line 1524)
- Modify: `crates/app/src/test_app.rs` (the `Session` literal in `open_test_photo`, around line 145)

⚠ The panel body runs sixty times a second. Reading the sidecar there is the `apply_view_demosaic` mistake in a new costume — CLAUDE.md cites that rule a dozen times. `Session::name` already caches the copy name from the sidecar for exactly this reason; this follows it.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` at the bottom of `crates/app/src/main.rs`:

```rust
    /// ⚠ Read once when the photo opens, never in a panel body — the `apply_view_demosaic` rule.
    /// `Session::name` caches the copy name for the same reason and from the same envelope.
    #[test]
    fn opening_a_photo_caches_its_snapshots() {
        let dir = test_app::TestDir::new("sessionsnaps");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();

        let mut sc = rustroom_app::sidecar::Sidecar::default();
        sc.set_edits(0, Edits::default());
        sc.add_snapshot(
            0,
            rustroom_app::sidecar::Snapshot {
                n: 1,
                name: Some("Warm".to_string()),
                taken: 0,
                edits: Edits::default(),
            },
        );
        rustroom_app::sidecar::save_all(&photo, &sc).unwrap();

        assert_eq!(
            rustroom_app::sidecar::load_all(&photo).snapshots_of(0).len(),
            1,
            "fixture check: the sidecar really holds one snapshot"
        );

        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        // Point the open session at the real file, then reload as `finalize_session` would.
        let s = app.session.as_mut().unwrap();
        s.path = photo.clone();
        s.snapshots = rustroom_app::sidecar::load_all(&photo).snapshots_of(0).to_vec();

        assert_eq!(app.session.as_ref().unwrap().snapshots.len(), 1);
        assert_eq!(
            app.session.as_ref().unwrap().snapshots[0].label(),
            "Warm",
            "the cache must carry the label, not just the count"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p app --bin Rustroom opening_a_photo_caches 2>&1 | tail -20`
Expected: FAIL to compile — `no field snapshots on type Session`.

- [ ] **Step 3: Write the implementation**

Add a field to `Session` in `crates/app/src/main.rs`, immediately after `name`:

```rust
    /// This copy's snapshots, cached from the sidecar when the photo opened.
    ///
    /// ⚠⚠ **Cached rather than read where it is drawn.** The strip is in the panel body, which
    /// runs every frame; a `load_all` there would read the file sixty times a second — the
    /// `apply_view_demosaic` rule. `name` above is cached from the same envelope for the same
    /// reason.
    ///
    /// ⚠ Kept in step by the `RustroomApp::*_snapshot` methods, which are the only writers. A
    /// second place that mutates the sidecar's snapshots without updating this is two facts that
    /// can disagree.
    snapshots: Vec<rustroom_app::sidecar::Snapshot>,
```

In `finalize_session`, replace the `name:` field's initialiser so the envelope is read **once** rather than twice, and both fields come from it. Above the `Session { .. }` literal add:

```rust
        // ⚠ One read for both, rather than a `load_all` per field: they come from the same
        // envelope and a second read is a second chance for them to disagree.
        let envelope = sidecar::load_all(&id.path);
```

Then in the literal:

```rust
            // ⚠ Read from the sidecar rather than carried from the Library entry, so a session
            // opened without a Library (single-file mode) still knows its own name.
            name: envelope.name_of(id.copy).map(str::to_string),
            snapshots: envelope.snapshots_of(id.copy).to_vec(),
```

⚠ `envelope` must be created **before** the existing `let stored = sidecar::load_copy(&path, id.copy);` line is used, and you may leave that line as it is — it is a different question (this copy's edits) and changing it is out of scope.

In `crates/app/src/test_app.rs`, add to the `Session` literal in `open_test_photo`, after `name: None,`:

```rust
        snapshots: Vec::new(),
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p app --bin Rustroom opening_a_photo_caches 2>&1 | tail -5`
Expected: PASS.

Run: `cargo test --all 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/main.rs crates/app/src/test_app.rs
git commit -m "feat(session): cache the open copy's snapshots"
```

---

### Task 2 + Task 3: The five operations and the strip that calls them

⚠⚠ **ONE COMMIT.** Every method below is on `RustroomApp` in a **binary** crate; without the panel calling them, `clippy -D warnings` fails on dead code. Do the work in the order given, but commit once at the end.

**Files:**
- Modify: `crates/app/src/main.rs` (methods on `impl RustroomApp`, near `create_virtual_copy` around line 2427; the `undo_one`/`redo_one` arms added by plan 1 around lines 1218 and 1288)
- Create: `crates/app/src/develop/panel/snapshots.rs`
- Modify: `crates/app/src/develop/panel/mod.rs` (declare the module; call it)
- Modify: `crates/app/src/theme.rs` (move `committed` here)

- [ ] **Step 1: Write the failing tests for the operations**

Append to `mod tests` at the bottom of `crates/app/src/main.rs`:

```rust
    /// Open a real file with a real sidecar, so the snapshot methods have something to write to.
    fn app_on_a_real_photo(
        dir: &test_app::TestDir,
        ctx: &egui::Context,
    ) -> (RustroomApp, std::path::PathBuf) {
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, ctx, 8, 4);
        app.session.as_mut().unwrap().path = photo.clone();
        (app, photo)
    }

    #[test]
    fn taking_a_snapshot_stores_the_current_edits_and_shows_them_in_the_strip() {
        let dir = test_app::TestDir::new("takesnap");
        let ctx = test_ui::ctx();
        let (mut app, photo) = app_on_a_real_photo(&dir, &ctx);
        app.session.as_mut().unwrap().edits.exposure = 1.25;

        app.take_snapshot();

        let s = app.session.as_ref().unwrap();
        assert_eq!(s.snapshots.len(), 1, "the cache must see it immediately");
        assert_eq!(s.snapshots[0].edits.exposure, 1.25);
        assert_eq!(s.snapshots[0].label(), "Snapshot 1");
        assert_eq!(
            rustroom_app::sidecar::load_all(&photo).snapshots_of(0).len(),
            1,
            "and it must be on disk, not only in memory"
        );
    }

    /// ⚠ Asserted on `can_undo`, **not** on `edits`. The edits change for reasons other than the
    /// push, so asserting on them cannot see a missing `history.push` — the exact correction the
    /// headless canvas tests already had to make.
    #[test]
    fn restoring_a_snapshot_is_one_undo_step() {
        let dir = test_app::TestDir::new("restoresnap");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        app.session.as_mut().unwrap().edits.exposure = 1.25;
        app.take_snapshot();
        app.session.as_mut().unwrap().edits.exposure = -2.0;
        app.session.as_mut().unwrap().history.push(
            app.session.as_ref().unwrap().edits.clone(),
        );

        app.restore_snapshot(1);

        let s = app.session.as_ref().unwrap();
        assert_eq!(s.edits.exposure, 1.25, "the snapshot's edits are now live");
        assert!(s.history.can_undo(), "and Ctrl+Z can take it back");
    }

    /// ⚠ `selected_mask` is an index into `edits.masks`; a restored state has a different list.
    /// Every read site guards with `.get(i)`, so a stale index is harmless *today* — but a mask
    /// added later would land at that index and appear selected without anyone selecting it.
    #[test]
    fn restoring_a_snapshot_clears_the_mask_selection() {
        let dir = test_app::TestDir::new("restoremask");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();
        app.session.as_mut().unwrap().selected_mask = Some(0);

        app.restore_snapshot(1);

        assert_eq!(app.session.as_ref().unwrap().selected_mask, None);
    }

    #[test]
    fn deleting_a_snapshot_is_undoable() {
        let dir = test_app::TestDir::new("delsnap");
        let ctx = test_ui::ctx();
        let (mut app, photo) = app_on_a_real_photo(&dir, &ctx);
        app.session.as_mut().unwrap().edits.exposure = 0.5;
        app.take_snapshot();

        app.delete_snapshot(1);
        assert!(app.session.as_ref().unwrap().snapshots.is_empty());
        assert!(rustroom_app::sidecar::load_all(&photo)
            .snapshots_of(0)
            .is_empty());

        app.undo_action(&ctx);
        assert_eq!(
            app.session.as_ref().unwrap().snapshots.len(),
            1,
            "undo must put it back in the cache"
        );
        assert_eq!(
            rustroom_app::sidecar::load_all(&photo).snapshots_of(0)[0]
                .edits
                .exposure,
            0.5,
            "with its edits, from the log's own copy"
        );
    }

    #[test]
    fn renaming_a_snapshot_is_undoable_in_both_directions() {
        let dir = test_app::TestDir::new("renamesnap");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();

        app.rename_snapshot(1, Some("Warm".to_string()));
        assert_eq!(app.session.as_ref().unwrap().snapshots[0].label(), "Warm");

        app.undo_action(&ctx);
        assert_eq!(
            app.session.as_ref().unwrap().snapshots[0].label(),
            "Snapshot 1",
            "undo restores the derived label"
        );

        app.redo_action(&ctx);
        assert_eq!(
            app.session.as_ref().unwrap().snapshots[0].label(),
            "Warm",
            "redo puts the name back"
        );
    }

    /// ⚠ Spec point 2 of "what snapshots deliberately do not do". `copy_from_snapshot` builds the
    /// new copy through `set_edits`/`set_name`, so it carries no snapshots — but that is a
    /// decision, not an accident, and an inherited list would duplicate every saved state on
    /// every copy made.
    #[test]
    fn a_copy_made_from_a_snapshot_does_not_inherit_the_snapshots() {
        let dir = test_app::TestDir::new("copyfromsnap");
        let ctx = test_ui::ctx();
        let (mut app, photo) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();
        app.take_snapshot();

        app.copy_from_snapshot(&ctx, 1);

        let sc = rustroom_app::sidecar::load_all(&photo);
        assert_eq!(sc.snapshots_of(0).len(), 2, "the original keeps both");
        assert!(
            sc.snapshots_of(1).is_empty(),
            "the new copy starts with none of its own"
        );
    }

    /// ⚠ `max + 1` end to end, through the app rather than only through `Sidecar`.
    #[test]
    fn a_snapshot_number_is_not_reused_after_a_delete() {
        let dir = test_app::TestDir::new("renumbersnap");
        let ctx = test_ui::ctx();
        let (mut app, _) = app_on_a_real_photo(&dir, &ctx);
        app.take_snapshot();
        app.take_snapshot();
        app.delete_snapshot(2);
        app.take_snapshot();

        let labels: Vec<String> = app
            .session
            .as_ref()
            .unwrap()
            .snapshots
            .iter()
            .map(|s| s.label())
            .collect();
        assert_eq!(labels, vec!["Snapshot 1", "Snapshot 3"]);
    }
```

⚠ Check whether `mod tests` in `main.rs` already imports `egui`, `Edits` and `test_app`; add whatever is missing to its `use` block rather than fully qualifying everything.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app --bin Rustroom taking_a_snapshot 2>&1 | tail -20`
Expected: FAIL to compile — `no method named take_snapshot`.

- [ ] **Step 3: Write the five operations**

Add to `impl RustroomApp` in `crates/app/src/main.rs`, immediately after `create_virtual_copy`:

```rust
    /// Save the open copy's current edits as a new snapshot.
    ///
    /// ⚠⚠ **Pushes no undo action, deliberately.** It changes no `Edits`, so `History<Edits>` has
    /// nothing to hold; and it follows `create_virtual_copy`, which likewise pushes nothing while
    /// `delete_virtual_copy` does. Creation is additive and reversed by deleting; deletion
    /// destroys the only record of a state.
    fn take_snapshot(&mut self) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let (path, copy, edits) = (s.path.clone(), s.copy, s.edits.clone());
        let mut sidecar = sidecar::load_all(&path);
        let n = sidecar.next_snapshot(copy);
        let snapshot = rustroom_app::sidecar::Snapshot {
            n,
            name: None,
            // ⚠ Local wall-clock as naive seconds — `Capture::taken`'s convention, so
            // `modals::info::taken_text` reads it back correctly with no timezone anywhere.
            taken: chrono::Local::now().naive_local().and_utc().timestamp(),
            edits,
        };
        sidecar.add_snapshot(copy, snapshot.clone());
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not save the snapshot: {e}");
            return;
        }
        if let Some(s) = self.session.as_mut() {
            s.snapshots.push(snapshot);
        }
        self.status = format!("Saved {}", format_args!("Snapshot {n}"));
    }

    /// Make snapshot `n` the open copy's live edits. Returns whether a history step was recorded.
    ///
    /// ⚠⚠ **The one place snapshots and `History<Edits>` meet, and it pushes no `ActionKind`
    /// itself.** `develop::body` already records exactly one `ActionKind::PhotoEdit` from the
    /// `photo_edited` flag both panel halves return — so pushing here too would make one restore
    /// take two presses of Ctrl+Z. This reports like a slider and lets the existing path log it.
    fn restore_snapshot(&mut self, n: u32) -> bool {
        let tool = self.tool;
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        let Some(snap) = s.snapshots.iter().find(|x| x.n == n) else {
            return false;
        };
        let label = snap.label();
        s.edits = snap.edits.clone();
        // ⚠ An index into the *old* mask list. Every read site guards with `.get(i)`, so a stale
        // one is harmless today — but a mask added later would land at that index and appear
        // selected without anyone having selected it. A photo switch clears it for this reason.
        s.selected_mask = None;
        s.active_mask_handle = None;
        let pushed = s.history.push(s.edits.clone());
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        self.status = format!("Restored {label}");
        pushed
    }

    /// Delete snapshot `n`, keeping it in the undo log so Ctrl+Z can put it back.
    fn delete_snapshot(&mut self, n: u32) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let (path, copy) = (s.path.clone(), s.copy);
        let mut sidecar = sidecar::load_all(&path);
        let Some(entry) = sidecar.remove_snapshot(copy, n) else {
            return;
        };
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not delete the snapshot: {e}");
            return;
        }
        let label = entry.label();
        if let Some(s) = self.session.as_mut() {
            s.snapshots.retain(|x| x.n != n);
        }
        let id = PhotoId::new(path, copy);
        self.actions.push(
            ActionKind::SnapshotDelete {
                id: id.clone(),
                entry: Box::new(entry),
            },
            Some(&id),
        );
        self.status = format!("Deleted {label} — Ctrl+Z to restore");
    }

    /// Rename snapshot `n`. Undo and redo both come through here with the names swapped, so the
    /// two directions cannot drift apart — `set_copy_name`'s rule.
    fn rename_snapshot(&mut self, n: u32, name: Option<String>) {
        let from = self
            .session
            .as_ref()
            .and_then(|s| s.snapshots.iter().find(|x| x.n == n))
            .and_then(|s| s.name.clone());
        if !self.set_snapshot_name(n, name.clone()) {
            return;
        }
        let id = self.open_photo();
        if let Some(id) = id {
            self.actions.push(
                ActionKind::SnapshotRename {
                    id: id.clone(),
                    n,
                    from,
                    to: name,
                },
                Some(&id),
            );
        }
    }

    /// Write a snapshot's name to the sidecar and the cache. The half both directions share.
    fn set_snapshot_name(&mut self, n: u32, name: Option<String>) -> bool {
        let Some(s) = self.session.as_ref() else {
            return false;
        };
        let (path, copy) = (s.path.clone(), s.copy);
        let mut sidecar = sidecar::load_all(&path);
        if !sidecar.set_snapshot_name(copy, n, name.clone()) {
            return false;
        }
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not rename the snapshot: {e}");
            return false;
        }
        if let Some(s) = self.session.as_mut() {
            if let Some(x) = s.snapshots.iter_mut().find(|x| x.n == n) {
                x.name = name;
            }
        }
        true
    }

    /// Restore a deleted snapshot from the undo log's own copy of it.
    fn put_back_snapshot(&mut self, id: &PhotoId, entry: rustroom_app::sidecar::Snapshot) {
        let mut sidecar = sidecar::load_all(&id.path);
        let label = entry.label();
        sidecar.add_snapshot(id.copy, entry.clone());
        // ⚠ Sorted by number, not left where `add_snapshot` appended it: a restored snapshot must
        // return to the position it had, or the strip reorders under an undo.
        if let Some(c) = sidecar.copies.iter_mut().find(|c| c.copy == id.copy) {
            c.snapshots.sort_by_key(|s| s.n);
        }
        match sidecar::persist_all(&id.path, &sidecar) {
            Ok(()) => self.status = format!("Restored {label}"),
            Err(e) => self.status = format!("Could not restore the snapshot: {e}"),
        }
        if let Some(s) = self.session.as_mut() {
            if s.path == id.path && s.copy == id.copy {
                s.snapshots.push(entry);
                s.snapshots.sort_by_key(|x| x.n);
            }
        }
    }

    /// Promote a snapshot to its own Library row, without disturbing the open photo.
    ///
    /// ⚠ Seeds the new copy's name from the snapshot's **only when it has one** — an unnamed
    /// snapshot gives an unnamed copy, which then follows the file's name as any unnamed copy
    /// does. Freezing a derived label into a stored one would strand the copy on it.
    fn copy_from_snapshot(&mut self, ctx: &egui::Context, n: u32) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let Some(snap) = s.snapshots.iter().find(|x| x.n == n) else {
            return;
        };
        let (path, edits, name) = (s.path.clone(), snap.edits.clone(), snap.name.clone());
        self.persist_current_session();

        let mut sidecar = sidecar::load_all(&path);
        let copy = sidecar.next_copy();
        sidecar.set_edits(copy, edits);
        sidecar.set_name(copy, name);
        if let Err(e) = sidecar::persist_all(&path, &sidecar) {
            self.status = format!("Could not create a copy: {e}");
            return;
        }
        if let Some(cat) = &self.catalog {
            if let Err(e) = cat.add_copy(&path, copy) {
                eprintln!("catalog: {e}");
            }
        }
        let id = PhotoId::new(path, copy);
        self.rebuild_entries(Some(&id));
        self.status = format!("Created copy {copy}");
        self.open_path(ctx, id);
    }
```

⚠ **Check `ActionLog::push`'s real signature against `crates/app/src/action_log.rs` before writing these calls.** ⚠ `restore_snapshot` deliberately pushes no `ActionKind` — `develop::body` (line 51) already records one `PhotoEdit` from the `photo_edited` flag, and a second push would make one restore take two presses of Ctrl+Z.

- [ ] **Step 4: Replace plan 1's placeholder undo arms**

In `crates/app/src/main.rs`, replace the temporary arm in `undo_one`:

```rust
            ActionKind::SnapshotDelete { id, entry } => {
                self.put_back_snapshot(&id, *entry);
            }
            ActionKind::SnapshotRename { id, n, from, to: _ } => {
                // One function, arguments swapped — `set_copy_name`'s rule, so the two
                // directions cannot drift apart.
                let _ = self.open_photo().filter(|o| *o == id);
                self.set_snapshot_name(n, from);
            }
```

and in `redo_one`:

```rust
            ActionKind::SnapshotDelete { id, entry } => {
                let mut sidecar = sidecar::load_all(&id.path);
                sidecar.remove_snapshot(id.copy, entry.n);
                match sidecar::persist_all(&id.path, &sidecar) {
                    Ok(()) => self.status = format!("Deleted {}", entry.label()),
                    Err(e) => self.status = format!("Could not delete the snapshot: {e}"),
                }
                if let Some(s) = self.session.as_mut() {
                    if s.path == id.path && s.copy == id.copy {
                        s.snapshots.retain(|x| x.n != entry.n);
                    }
                }
            }
            ActionKind::SnapshotRename { id: _, n, from: _, to } => {
                self.set_snapshot_name(n, to);
            }
```

⚠ Both actions are photo-scoped, so `reachable` already guarantees the open photo **is** the one they name — that is why `set_snapshot_name` can act on the open session without re-checking the id. Delete the stray `let _ = self.open_photo()...` line above; it was a reminder, not code. Write a one-line comment in its place saying why no check is needed.

- [ ] **Step 5: Move `committed` into `theme`**

⚠ CLAUDE.md records that `committed(ui, resp)` is *"written ONCE and serves all three name fields"*. A fourth caller means it stops being private to `collections_panel`.

Cut it from `crates/app/src/collections_panel.rs` and add to `crates/app/src/theme.rs`:

```rust
/// Whether a text field was **committed** — Enter pressed while it had focus.
///
/// ⚠ `lost_focus() && key_pressed(Enter)`, never a bare `lost_focus()`: clicking away is
/// abandoning a name, not confirming it. Written once and shared by every in-place name field —
/// collection, keyword, smart collection, snapshot — because two guards for one rule is how they
/// drift.
pub fn committed(ui: &egui::Ui, resp: &egui::Response) -> bool {
    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}
```

In `collections_panel.rs`, replace its three call sites with `crate::theme::committed(ui, &resp)`.

- [ ] **Step 6: Write the strip**

Create `crates/app/src/develop/panel/snapshots.rs`:

```rust
//! The Develop snapshot strip: a photo's named saved states, pinned below the adjustments.
//!
//! ⚠⚠ **A deliberate departure from the exclusive-panel rule.** Every other control in this panel
//! belongs to exactly one rail tool; this one is drawn under all of them. A snapshot list is
//! *navigation*, not a tool — the whole point is that the other states of the photo you are
//! working on are always in reach. Behind a fifth rail tool it would be hidden precisely when it
//! is wanted.
//!
//! ⚠ `ui.set_min_height(ui.available_height())` is load-bearing: a resizable egui panel stores its
//! *content's* rendered rect as its size each frame, so content smaller than the panel makes it
//! shrink back every frame. The filmstrip already carries this note.
//!
//! ⚠⚠ **The scroll area here is a SIBLING of the panel's own, not nested inside it.** The strip is
//! allocated before the right panel's `ScrollArea`, so the two occupy disjoint rects and the wheel
//! is never ambiguous. This looks exactly like the nesting the Library panel's three inner scroll
//! areas were removed for on 2026-08-27 — and it is not that. Do not "fix" it.

use crate::{theme, RustroomApp};
use eframe::egui;

/// What a click in the strip asked for. Applied after the panel closes, because egui holds
/// `&mut app` through the closure — the shape every panel in this app uses.
enum Action {
    None,
    Take,
    Restore(u32),
    Delete(u32),
    Rename(u32, Option<String>),
    MakeCopy(u32),
}

/// The strip's id, and so its resize handle's — see [`show`].
const PANEL_ID: &str = "snapshot_strip";

/// Draw the strip. Returns whether the photo's edits changed, which the caller records as one
/// history entry — the same contract `panel::show` already has.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    if app.session.is_none() {
        return false;
    }
    let mut action = Action::None;
    // ⚠ Capped at half the *live* panel height rather than at the stored value, so a stored
    // height taller than a shrunken window is clamped rather than obeyed. egui clamps the
    // persisted size into this range itself — see `Panel::outer_size`.
    let max = (ui.available_height() * 0.5).max(rustroom_app::settings::STRIP_HEIGHT_RANGE.0 as f32);
    let stored = app.settings.clamped_strip_height();

    let panel = egui::Panel::bottom(PANEL_ID)
        .resizable(true)
        .default_size(stored)
        .min_size(rustroom_app::settings::STRIP_HEIGHT_RANGE.0 as f32)
        .max_size(max)
        .show(ui, |ui| {
            // Fill the panel's height so egui stores the panel size, not the smaller content
            // size — otherwise a resizable panel shrinks back each frame.
            ui.set_min_height(ui.available_height());

            let count = app.session.as_ref().map_or(0, |s| s.snapshots.len());
            ui.horizontal(|ui| {
                ui.label(if count == 0 {
                    "Snapshots".to_string()
                } else {
                    format!("Snapshots ({count})")
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("+")
                        .on_hover_text("Save the current settings as a snapshot")
                        .clicked()
                    {
                        action = Action::Take;
                    }
                });
            });

            if count == 0 {
                theme::hint(ui, "Press + to save the current settings.");
                return;
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let snaps: Vec<(u32, String, i64)> = app
                        .session
                        .as_ref()
                        .map(|s| {
                            s.snapshots
                                .iter()
                                .map(|x| (x.n, x.label(), x.taken))
                                .collect()
                        })
                        .unwrap_or_default();
                    for (n, label, taken) in snaps {
                        // ⚠ egui's auto-ids shift with what was drawn before them, and this list
                        // changes as snapshots are taken and deleted — the hazard that broke the
                        // rename field under virtualisation.
                        ui.push_id(n, |ui| {
                            row(app, ui, n, &label, taken, &mut action);
                        });
                    }
                });
        });

    // ⚠ Persisted on the **release** frame only, never per frame: a resize runs at 60 Hz. egui
    // deliberately recomputes the panel size on `drag_stopped` so the released size is the one
    // stored, and `InnerResponse::response.rect` is the panel's *outer* rect — the same quantity
    // `default_size` takes, so feeding it back cannot drift.
    let resize_id = egui::Id::new(PANEL_ID).with("__resize");
    if ui
        .read_response(resize_id)
        .is_some_and(|r| r.drag_stopped())
    {
        let h = panel.response.rect.height().round() as u32;
        if h != app.settings.snapshot_strip_height {
            app.settings.snapshot_strip_height = h;
            app.save_settings();
        }
    }

    apply(app, ctx, action)
}

/// One row: its label, its timestamp, a click to restore and a menu to manage it.
fn row(
    app: &mut RustroomApp,
    ui: &mut egui::Ui,
    n: u32,
    label: &str,
    taken: i64,
    action: &mut Action,
) {
    // The rename field replaces the label while this row owns the draft.
    if app.snapshot_rename.as_ref().is_some_and(|(m, _)| *m == n) {
        let text = app.snapshot_rename.as_mut().map(|(_, t)| t).unwrap();
        let resp = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
        if resp.gained_focus() || !resp.has_focus() {
            resp.request_focus();
        }
        if theme::committed(ui, &resp) {
            let typed = text.trim().to_string();
            // ⚠ Typing the default label back in stores `None`, so the snapshot keeps following
            // its number — the rule an unnamed virtual copy already follows. Freezing the derived
            // string would strand it.
            let name = if typed.is_empty() || typed == format!("Snapshot {n}") {
                None
            } else {
                Some(typed)
            };
            *action = Action::Rename(n, name);
        } else if resp.lost_focus() {
            // ⚠ Clicking away abandons. Only Enter commits.
            app.snapshot_rename = None;
        }
        return;
    }

    let resp = ui
        .horizontal(|ui| {
            let r = ui.selectable_label(false, label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::caption(ui, &crate::modals::info::taken_text(Some(taken)));
            });
            r
        })
        .inner;

    if resp.clicked() {
        *action = Action::Restore(n);
    }
    resp.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            app.snapshot_rename = Some((n, label.to_string()));
            ui.close();
        }
        if ui.button("Make virtual copy").clicked() {
            *action = Action::MakeCopy(n);
            ui.close();
        }
        if ui.button("Delete").clicked() {
            *action = Action::Delete(n);
            ui.close();
        }
    });
}

/// Apply what the frame asked for. Returns whether the photo's edits changed.
fn apply(app: &mut RustroomApp, ctx: &egui::Context, action: Action) -> bool {
    match action {
        Action::None => false,
        Action::Take => {
            app.take_snapshot();
            false
        }
        Action::Restore(n) => app.restore_snapshot(n),
        Action::Delete(n) => {
            app.delete_snapshot(n);
            false
        }
        Action::Rename(n, name) => {
            app.rename_snapshot(n, name);
            app.snapshot_rename = None;
            false
        }
        Action::MakeCopy(n) => {
            app.copy_from_snapshot(ctx, n);
            false
        }
    }
}
```

⚠ **Verify each egui call against the vendored source before trusting it** — `ui.close()`, `resp.context_menu`, `ui.read_response`, `Panel::min_size`/`max_size`, and `InnerResponse::response.rect`. The vendored crate is at
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/egui-0.35.0`. `ui.close()` in particular may be spelled `ui.close_menu()` in this version; check `src/ui.rs`.

⚠ `crate::modals::info::taken_text` must be `pub`; make it so if it is not, and note in its doc comment that the strip is its second caller.

- [ ] **Step 7: Add the two `RustroomApp` fields the strip needs**

In `crates/app/src/main.rs`, add to `RustroomApp`:

```rust
    /// The snapshot row being renamed, and what has been typed into it.
    ///
    /// ⚠ A scratch draft, exactly like `rename_draft`: writing through on every keystroke would
    /// push one undo step per character and commit a half-typed name on focus loss.
    snapshot_rename: Option<(u32, String)>,
```

⚠ Clear it wherever `rename_draft` is cleared on a view switch and on a photo switch — grep for `rename_draft = None` and add a sibling line at each. A draft left behind belongs to a photo that is no longer open.

Add `snapshot_rename: None,` to the exhaustive literal in `crates/app/src/test_app.rs::for_test` — that literal is exhaustive on purpose, so the compiler will tell you.

⚠ Check whether `RustroomApp::save_settings` exists; if the codebase writes settings inline instead, follow whatever `apply_view_demosaic` does and call that rather than inventing a method.

- [ ] **Step 8: Wire it into the panel**

In `crates/app/src/develop/panel/mod.rs`, add `pub mod snapshots;` beside the other module declarations.

Inside `show`'s `egui::Panel::right("panel").show(ui, |ui| { ... })` closure, **before** the `egui::ScrollArea::vertical()` line, add:

```rust
            // ⚠⚠ **Before the scroll area, and that ordering is the whole feature.** egui
            // allocates in call order, so the strip takes its height from the panel's bottom edge
            // and the scroll area gets what is left — which is what pins the strip while forty
            // sliders scroll behind it. Moved after, it would scroll away with them.
            if app.pending.is_none() {
                photo_edited |= snapshots::show(app, ui, &ui.ctx().clone());
            }
```

⚠ `photo_edited` is declared at the top of `show`; this adds a third writer to it. Confirm the declaration is `let mut photo_edited = false;` and that the function returns it.

- [ ] **Step 9: Build, test, lint**

Run: `cargo build --workspace --all-targets 2>&1 | tail -30`
Expected: success. Fix compile errors against the vendored egui source, not by guessing.

Run: `cargo test --all 2>&1 | tail -20`
Expected: PASS, including the six new tests from Step 1.

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20; echo "clippy exit: $?"`
Expected: `clippy exit: 0`. ⚠ **Read the exit code.** CLAUDE.md records two commits that went in with clippy failing because a green `cargo test` was mistaken for evidence about clippy.

Run: `cargo fmt --check && echo "fmt clean"`

- [ ] **Step 10: Look at it**

```bash
cargo run --release -p app 2>&1 | tail -5 &
```

⚠ Always `--release`, in the background. Open a photo, then check by hand — this is the part no test covers:

1. The strip is at the bottom of the right panel and **stays there while you scroll the sliders**.
2. It is present under **all four** rail tools.
3. `+` adds a row reading `Snapshot 1` with today's date beside it.
4. Clicking a row visibly changes the image; Ctrl+Z takes it back.
5. Right-click → Rename puts a focused field in the row; Enter commits, clicking away does not.
6. Right-click → Delete removes it; Ctrl+Z restores it **in its original position**.
7. Right-click → Make virtual copy opens a new copy without changing the photo you were on.
8. Dragging the strip's top edge resizes it; more rows than fit scroll **inside** it while the sliders still scroll independently.
9. Quit and relaunch: the height you chose is still there.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "feat(develop): a pinned, resizable snapshot strip"
```

---

### Task 4: Take a snapshot from the menu

**Files:**
- Modify: `crates/app/src/menu.rs` (the Photo menu, around line 238)
- Modify: `crates/app/src/commands.rs` (the dispatcher, around line 39)

⚠ One commit: `Command::TakeSnapshot` with no menu entry is an unreachable variant, and the dispatcher's exhaustive match is what forces both halves to land together.

- [ ] **Step 1: Add the command**

In `crates/app/src/menu.rs`, add a variant to `Command` beside `CreateVirtualCopy`:

```rust
    TakeSnapshot,
```

and an entry in the Photo section, above `Create virtual copy`:

```rust
                action("Take snapshot", Command::TakeSnapshot, None),
```

⚠ **No keyboard shortcut.** Check `Shortcut::safe_as_native_accelerator`'s rule before adding one: a bare key is stolen from every text field and a text-editing chord breaks editing inside one. Lightroom binds no default shortcut to this either. Leaving it unbound is a decision, not an omission.

- [ ] **Step 2: Build to find the exhaustive match**

Run: `cargo build --workspace --all-targets 2>&1 | tail -20`
Expected: FAIL — `commands.rs` does not cover `Command::TakeSnapshot`. That is the match doing its job.

- [ ] **Step 3: Dispatch it**

In `crates/app/src/commands.rs`, beside `Command::CreateVirtualCopy`:

```rust
        Command::TakeSnapshot => app.take_snapshot(),
```

⚠ `take_snapshot` is private to `main.rs`'s `impl RustroomApp`; `commands.rs` is a sibling bin module, so privacy is fine — but confirm, and if the neighbouring calls use `pub(crate)`, match them.

- [ ] **Step 4: Verify**

Run: `cargo test --all 2>&1 | tail -10`
Expected: PASS, including `menu`'s completeness tests. ⚠ If a test asserts every `Command` has a menu entry or a shortcut listing, it must now cover this one — **complete the table rather than weakening the test**, the rule CLAUDE.md records for `every_shortcut_maps_to_an_egui_key`.

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -10; echo "clippy exit: $?"`

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/menu.rs crates/app/src/commands.rs
git commit -m "feat(menu): Take snapshot in the Photo menu"
```

---

### Task 5: The headless tests the eleven-times-recorded gap demands

**Files:**
- Modify: `crates/app/src/develop/panel/snapshots.rs` (a `mod tests` at the bottom)

⚠⚠ **This is the task that stops this feature joining the eleven GUI defects found by use and not by test.** The pure functions in plan 1 carry thirty tests and will produce no defect; every bug in this feature will be in the wiring below.

- [ ] **Step 1: Write the tests**

Append to `crates/app/src/develop/panel/snapshots.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};
    use rustroom_app::tool::Tool;

    /// ⚠⚠ **The requirement the user actually asked for, and the only test that can see it.**
    /// "Always active" is otherwise a visual claim: a strip drawn under `Tool::Edit` alone looks
    /// perfectly correct in every screenshot anyone happens to take of the Edit panel.
    #[test]
    fn the_strip_is_present_under_every_tool() {
        for tool in Tool::ALL {
            let ctx = test_ui::ctx();
            let mut app = test_app::for_test();
            test_app::open_test_photo(&mut app, &ctx, 8, 4);
            app.tool = tool;

            let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                crate::develop::panel::show(&mut app, ui);
            });
            assert!(
                labels.iter().any(|(l, _)| l == "+"),
                "the Take-snapshot button must be reachable under {tool:?}"
            );
        }
    }

    /// ⚠ The spec asks that the height survive a photo switch. It does **by construction** — the
    /// value lives in `Settings`, not on `Session` — and this states that as a property so a
    /// future move onto `Session` breaks a test rather than a habit. A *restart* is covered by
    /// `settings::tests`' round trip; the *minimum* by `a_nonsense_stored_strip_height_is_clamped`.
    #[test]
    fn the_strip_height_is_app_wide_not_per_photo() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.settings.snapshot_strip_height = 200;
        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        assert_eq!(app.settings.snapshot_strip_height, 200);
        test_app::open_test_photo(&mut app, &ctx, 16, 8);
        assert_eq!(
            app.settings.snapshot_strip_height, 200,
            "opening another photo must not reset the strip"
        );
    }

    /// The strip draws nothing when no photo is open — the panel says "No image open." instead.
    #[test]
    fn the_strip_draws_nothing_without_a_session() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            crate::develop::panel::show(&mut app, ui);
        });
        assert!(!labels.iter().any(|(l, _)| l == "+"));
    }

    /// A snapshot's row is labelled by `Snapshot::label`, so a named one shows its name.
    #[test]
    fn a_named_snapshot_shows_its_name_in_the_strip() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        app.session.as_mut().unwrap().snapshots.push(
            rustroom_app::sidecar::Snapshot {
                n: 1,
                name: Some("Before the crop".to_string()),
                taken: 0,
                edits: Default::default(),
            },
        );

        let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            crate::develop::panel::show(&mut app, ui);
        });
        assert!(
            labels.iter().any(|(l, _)| l == "Before the crop"),
            "got {:?}",
            labels.iter().map(|(l, _)| l).collect::<Vec<_>>()
        );
    }
}
```

⚠ `develop::panel::show` takes `(&mut RustroomApp, &mut egui::Ui)` and returns `bool`; confirm against the real signature. ⚠ `test_ui::labelled_centres` finds only **interactive** widgets — a plain `ui.label` produces no AccessKit node. If the row's label is not found, that is because `selectable_label` is what makes it findable; do not weaken the test, check the widget.

- [ ] **Step 2: Run them**

Run: `cargo test -p app --bin Rustroom snapshots::tests 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Sabotage each one**

Run each, confirm a `test result:` line appears (a sabotage that fails to compile reads exactly like one that passes), then restore.

| sabotage | expected to fail |
|---|---|
| in `panel::show`, guard the strip with `if tool == Tool::Edit` | `the_strip_is_present_under_every_tool` (3 of 4 tools) |
| in `row`, draw `format!("Snapshot {n}")` instead of `label` | `a_named_snapshot_shows_its_name_in_the_strip` |
| in `show`, remove the `app.session.is_none()` early return | `the_strip_draws_nothing_without_a_session` |

⚠ **Record any sabotage that is inert rather than papering over it.** Moving the strip *after* the scroll area is predicted to be inert — no test can see pinning — and saying so is better than inventing a test that appears to cover it. CLAUDE.md records several such honest notes.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/develop/panel/snapshots.rs
git commit -m "test(develop): the snapshot strip is present under every tool"
```

---

### Task 6: Close the feature

- [ ] **Step 1: The goldens must not have moved**

Run: `cargo test -p pipeline --test golden 2>&1 | tail -5`
Expected: **47 passing, none regenerated.** This feature touches no pipeline code; a moved golden means something leaked.

- [ ] **Step 2: Full green**

```bash
cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
echo "exit: $?"
```
Expected: `exit: 0`.

- [ ] **Step 3: Count the tests**

Run: `cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{s+=$4} END {print s}'`
The baseline before plan 1 was **1165**.

- [ ] **Step 4: Launch it and use it**

```bash
cargo run --release -p app 2>&1 | tail -5 &
```

Walk the nine checks from Task 2's Step 10 again, on a real RAW this time, and then two more that only a real photo can show:

10. Take a snapshot, then press **"Reset adjustments"**. The snapshot must still be there — this is the `is_neutral` fix, and its failure mode is silent deletion of the sidecar.
11. Take a snapshot, switch to another photo, switch back. The strip must still show it.

- [ ] **Step 5: Update `CLAUDE.md`**

Add a section for snapshots to the "Current status" block, in the house style: what shipped, the ⚠ findings, the test count, and the accepted caveats from the spec. ⚠ **Move the `▶ NEXT ACTION` on to item 4 of the agreed order — the remaining backlog** — and mark **M3.4 as closed**, which closes Phase 3's last outstanding milestone half.

⚠ Remove the Snapshots entry from `docs/BACKLOG.md`, and update the "order of work" list in `CLAUDE.md` that names it as next.

- [ ] **Step 6: Commit and hand over**

```bash
git add -A
git commit -m "docs: snapshots shipped; M3.4 closed"
```

⚠ **Do not push and do not merge.** Report the branch state and let the user merge, as every feature in this project has been handled.
