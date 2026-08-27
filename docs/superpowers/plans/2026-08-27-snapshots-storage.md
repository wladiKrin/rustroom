# Snapshots, plan 1 of 2: storage and undo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give a photo's sidecar a list of named saved states per copy, with the undo actions and the persisted panel height that plan 2's UI will drive — all headless and fully tested, with no GUI code in existence.

**Architecture:** A `Snapshot` type lives in `crates/app/src/sidecar.rs` beside `CopyEntry`, which grows a `#[serde(default)] snapshots: Vec<Snapshot>`. Two new `ActionKind` variants in `action_log.rs` are scoped to their `PhotoId` exactly as `CopyRename` is. One new `Settings` field carries the strip's height. **Nothing calls any of it yet**, and that is deliberate — every one of these modules is in the **lib** (`rustroom_app`), where a `pub` item is public API and does not trip `dead_code`. That is the exact contrast CLAUDE.md draws between `tool` (lib, commits alone) and `HslChannel` (bin, must land with its caller).

**Tech Stack:** Rust, serde, `chrono` (already a dependency of `app`). No new dependencies.

**Read first:** `docs/superpowers/specs/2026-08-27-snapshots-design.md`.

**Before you start:** the build needs libraw and lcms2 on the pkg-config path.

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **Commit convention: no `Co-Authored-By` trailer.** ⚠ **Never push.** You are on branch `snapshots`.

---

### Task 1: The `Snapshot` type and its label

**Files:**
- Modify: `crates/app/src/sidecar.rs` (add the type just above `CopyEntry`, around line 54)

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block at the bottom of `crates/app/src/sidecar.rs`:

```rust
    #[test]
    fn an_unnamed_snapshot_is_labelled_by_its_number() {
        let s = Snapshot {
            n: 3,
            name: None,
            taken: 0,
            edits: Edits::default(),
        };
        assert_eq!(s.label(), "Snapshot 3");
    }

    #[test]
    fn a_named_snapshot_is_labelled_by_its_name() {
        let s = Snapshot {
            n: 3,
            name: Some("Before the crop".to_string()),
            taken: 0,
            edits: Edits::default(),
        };
        assert_eq!(s.label(), "Before the crop");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p app --lib sidecar::tests::an_unnamed_snapshot 2>&1 | tail -20`
Expected: FAIL to compile — `cannot find struct Snapshot`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/app/src/sidecar.rs` immediately above `/// One version of a photo.` (the `CopyEntry` doc comment):

```rust
/// One saved state within a copy — a named point in that copy's editing.
///
/// ⚠⚠ **Deliberately outside `History<Edits>`, and joined to it at exactly one point.** Taking
/// one changes no `Edits`, so there is nothing for an undo stack of `Edits` to hold; *restoring*
/// one does change them, so `History::push` covers it and Ctrl+Z undoes a restore for free.
/// Undoing past a snapshot cannot touch it, because it was never in the stack. That independence
/// is what makes a snapshot unable to vanish silently — which is why this replaced the dropped
/// "persistent undo history". See the design doc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Identity: `max + 1` within its copy, **never reused**.
    ///
    /// ⚠ The copy-number rule, for the copy-number reason: an *unnamed* snapshot's label is
    /// derived from this, so reusing a number would silently rename an existing row.
    pub n: u32,
    /// What it is called, when it has been given a name. `None` follows the number.
    #[serde(default)]
    pub name: Option<String>,
    /// When it was taken — **local wall-clock seconds** since the epoch.
    ///
    /// ⚠ The same convention `Capture::taken` stores its date under, and for the same two
    /// reasons: rendering it needs no timezone, and wall-clock is what a person means by "when
    /// I took it". Read it back with `crate::modals::info::taken_text`'s convention.
    pub taken: i64,
    pub edits: Edits,
}

impl Snapshot {
    /// What this snapshot is called on screen.
    ///
    /// ⚠ **The one rule, with every consumer going through it** — the strip's row, the rename
    /// field's initial text, the name seeded into a virtual copy. That is `entries::shown_name`'s
    /// rule and the reason it exists: this project has already shipped a bug where the Library
    /// printed `file_name` while a copy's real name lived somewhere else, because two
    /// implementations of "what is this called" existed.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("Snapshot {}", self.n))
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p app --lib sidecar::tests:: 2>&1 | tail -5`
Expected: PASS, all sidecar tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/sidecar.rs
git commit -m "feat(sidecar): a Snapshot type with one labelling rule"
```

---

### Task 2: `CopyEntry` carries snapshots, and old sidecars still load

**Files:**
- Modify: `crates/app/src/sidecar.rs` (the `CopyEntry` struct, around line 56)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/app/src/sidecar.rs`:

```rust
    /// ⚠ The guard on the field's `#[serde(default)]`. Without it every sidecar written before
    /// this feature fails to parse, and `load_all` returns `Sidecar::default()` — which is
    /// **silent loss of every edit anyone has ever made**, with no error anywhere.
    #[test]
    fn a_sidecar_from_before_snapshots_still_loads_its_edits() {
        let dir = TempDir::new("presnap");
        let photo = dir.0.join("a.RAF");
        std::fs::write(
            sidecar::path(&photo),
            r#"{"copies":[{"copy":0,"name":null,"edits":{"exposure":1.5}}]}"#,
        )
        .unwrap();

        let s = load_all(&photo);
        assert_eq!(s.copies.len(), 1, "the copy must survive");
        assert_eq!(s.copies[0].edits.exposure, 1.5, "its edits must survive");
        assert!(
            s.copies[0].snapshots.is_empty(),
            "and it must read as having no snapshots"
        );
    }

    #[test]
    fn snapshots_round_trip_through_the_file() {
        let dir = TempDir::new("snaproundtrip");
        let photo = dir.0.join("a.RAF");
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.copies[0].snapshots.push(Snapshot {
            n: 1,
            name: Some("Warm".to_string()),
            taken: 1_756_000_000,
            edits: Edits {
                exposure: 0.75,
                ..Default::default()
            },
        });
        save_all(&photo, &s).unwrap();

        let back = load_all(&photo);
        assert_eq!(back, s, "the whole envelope must survive the round trip");
    }
```

⚠ `sidecar::path` in the first test is reachable as plain `path` inside this module; write `path(&photo)` rather than `sidecar::path(&photo)`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app --lib sidecar::tests::snapshots_round_trip 2>&1 | tail -20`
Expected: FAIL to compile — `no field snapshots on type CopyEntry`.

- [ ] **Step 3: Write the implementation**

In `crates/app/src/sidecar.rs`, add a field to `CopyEntry`, after `edits`:

```rust
    /// This copy's named saved states, oldest first.
    ///
    /// ⚠ **Per copy, not per file**, because each copy has its own `Edits` and so its own
    /// history to bookmark. The same split marks take, and the opposite of keywords, for the
    /// same underlying reason: a snapshot describes *the image you are making*.
    ///
    /// ⚠⚠ **`#[serde(default)]` belongs here and is required** — without it every sidecar
    /// written before this feature fails to parse and `load_all` silently returns an empty
    /// envelope, discarding every edit. ⚠ It must still **never** be added to `Sidecar` itself;
    /// that combination is the documented data-loss trap — see `load_all`'s doc comment.
    #[serde(default)]
    pub snapshots: Vec<Snapshot>,
```

Then fix the two struct literals in this file that construct `CopyEntry` — in `Sidecar::set_edits` and `Sidecar::set_name` — by adding `snapshots: Vec::new(),` to each.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p app --lib sidecar:: 2>&1 | tail -5`
Expected: PASS.

Then check nothing else in the workspace constructs a `CopyEntry`:

Run: `cargo build --workspace --all-targets 2>&1 | tail -20`
Expected: success. If a literal elsewhere fails, add `snapshots: Vec::new(),` to it.

- [ ] **Step 5: Sabotage — confirm the legacy test is not vacuous**

Remove the `#[serde(default)]` line you just added, then:

Run: `cargo test -p app --lib sidecar::tests::a_sidecar_from_before_snapshots 2>&1 | tail -20`
Expected: **FAIL**. ⚠ If it passes, the test is not seeing the field — do not proceed; fix the test first.

⚠ Confirm the sabotage **compiled**: the output must contain a `test result:` line. A sabotage that fails to compile looks exactly like one that passes. Restore the attribute afterwards and re-run to confirm green.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/sidecar.rs
git commit -m "feat(sidecar): CopyEntry carries a snapshot list"
```

---

### Task 3: A neutral copy holding snapshots is still worth a file

**Files:**
- Modify: `crates/app/src/sidecar.rs` (`Sidecar::is_neutral`, around line 175)

⚠ **This is the task that prevents real data loss.** `persist_all` deletes the file when `is_neutral()`, so without it: take a snapshot, press "Reset adjustments", and every snapshot is gone.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
    /// ⚠ Take a snapshot, then reset the adjustments: `persist_all` must **not** delete the
    /// sidecar. `is_neutral` already documents this hazard for names — "a name or a second copy
    /// is worth a file even with neutral edits" — and snapshots are the third case of that rule.
    #[test]
    fn resetting_a_photo_does_not_delete_its_snapshots() {
        let dir = TempDir::new("snapneutral");
        let photo = dir.0.join("a.RAF");
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.copies[0].snapshots.push(Snapshot {
            n: 1,
            name: None,
            taken: 0,
            edits: Edits {
                exposure: 2.0,
                ..Default::default()
            },
        });

        assert!(
            !s.is_neutral(),
            "a copy holding snapshots is worth a file however neutral its own edits are"
        );
        persist_all(&photo, &s).unwrap();
        assert!(path(&photo).exists(), "the sidecar must still be on disk");
        assert_eq!(load_all(&photo).copies[0].snapshots.len(), 1);
    }

    /// The other half: nothing stored at all is still worth deleting.
    #[test]
    fn a_neutral_photo_with_no_snapshots_is_still_neutral() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        assert!(s.is_neutral());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p app --lib sidecar::tests::resetting_a_photo 2>&1 | tail -20`
Expected: FAIL — `a copy holding snapshots is worth a file however neutral its own edits are`.

- [ ] **Step 3: Write the implementation**

Replace the body of `Sidecar::is_neutral` in `crates/app/src/sidecar.rs`:

```rust
    pub fn is_neutral(&self) -> bool {
        match self.copies.as_slice() {
            [] => true,
            // ⚠ `snapshots.is_empty()` is the third clause of a rule this comment already
            // states: a copy holding saved states is worth a file however neutral its *current*
            // edits are. Without it, "take a snapshot, then Reset adjustments" deletes the
            // sidecar and every snapshot in it.
            [only] => {
                only.copy == 0
                    && only.name.is_none()
                    && only.edits == Edits::default()
                    && only.snapshots.is_empty()
            }
            _ => false,
        }
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p app --lib sidecar:: 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Sabotage**

Delete the `&& only.snapshots.is_empty()` clause.
Run: `cargo test -p app --lib sidecar::tests::resetting_a_photo 2>&1 | tail -20`
Expected: **FAIL**. Confirm a `test result:` line appears, then restore.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/sidecar.rs
git commit -m "fix(sidecar): a copy holding snapshots is not neutral"
```

---

### Task 4: `Sidecar` helpers — numbering, add, remove, rename

**Files:**
- Modify: `crates/app/src/sidecar.rs` (methods on `impl Sidecar`, after `next_copy`)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests`:

```rust
    fn snap(n: u32, exposure: f32) -> Snapshot {
        Snapshot {
            n,
            name: None,
            taken: 0,
            edits: Edits {
                exposure,
                ..Default::default()
            },
        }
    }

    /// ⚠ `max + 1`, never `count + 1`. Take three, delete the second, take a fourth: it must be
    /// `Snapshot 4`. Reusing `3` would point the new row at an existing label.
    #[test]
    fn snapshot_numbers_are_never_reused() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        for i in 1..=3 {
            let n = s.next_snapshot(0);
            assert_eq!(n, i);
            s.add_snapshot(0, snap(n, i as f32));
        }
        s.remove_snapshot(0, 2);
        assert_eq!(
            s.next_snapshot(0),
            4,
            "the highest ever used is 3, so the next is 4"
        );
    }

    /// Numbering is per copy: copy 1's first snapshot is 1 even when copy 0 has three.
    #[test]
    fn snapshot_numbering_is_per_copy() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.set_edits(1, Edits::default());
        s.add_snapshot(0, snap(1, 1.0));
        s.add_snapshot(0, snap(2, 2.0));
        assert_eq!(s.next_snapshot(1), 1);
    }

    #[test]
    fn removing_a_snapshot_returns_it_so_undo_can_put_it_back() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.add_snapshot(0, snap(1, 1.0));
        s.add_snapshot(0, snap(2, 2.0));

        let taken = s.remove_snapshot(0, 1).expect("snapshot 1 exists");
        assert_eq!(taken.n, 1);
        assert_eq!(taken.edits.exposure, 1.0);
        assert_eq!(s.snapshots_of(0).len(), 1);
        assert!(s.remove_snapshot(0, 1).is_none(), "removing twice is None");
    }

    #[test]
    fn a_snapshot_can_be_renamed_and_unnamed() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.add_snapshot(0, snap(1, 1.0));

        assert!(s.set_snapshot_name(0, 1, Some("Warm".to_string())));
        assert_eq!(s.snapshots_of(0)[0].label(), "Warm");
        assert!(s.set_snapshot_name(0, 1, None));
        assert_eq!(s.snapshots_of(0)[0].label(), "Snapshot 1");
        assert!(
            !s.set_snapshot_name(0, 9, Some("nope".to_string())),
            "renaming a snapshot that is not there must report that it did nothing"
        );
    }

    /// Adding to a copy with no entry yet creates it — the shape `set_edits` already has.
    #[test]
    fn adding_a_snapshot_to_an_absent_copy_creates_it() {
        let mut s = Sidecar::default();
        s.add_snapshot(2, snap(1, 1.0));
        assert_eq!(s.snapshots_of(2).len(), 1);
        assert_eq!(s.copies[0].copy, 2);
    }

    #[test]
    fn snapshots_of_an_absent_copy_is_empty() {
        let s = Sidecar::default();
        assert!(s.snapshots_of(7).is_empty());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app --lib sidecar::tests::snapshot_numbers 2>&1 | tail -20`
Expected: FAIL to compile — `no method named next_snapshot`.

- [ ] **Step 3: Write the implementation**

Add to `impl Sidecar` in `crates/app/src/sidecar.rs`, after `next_copy`:

```rust
    /// One copy's snapshots, oldest first. Empty when that copy has none, or is not there.
    pub fn snapshots_of(&self, copy: u32) -> &[Snapshot] {
        self.get(copy).map_or(&[], |c| c.snapshots.as_slice())
    }

    /// The number a new snapshot of `copy` should take: one past the highest ever used **there**.
    ///
    /// ⚠ `max + 1`, never "first free" and never `count + 1`. An unnamed snapshot's label is
    /// derived from its number, so reusing one silently renames an existing row — the same
    /// reasoning `next_copy` carries.
    ///
    /// ⚠ Per copy: two copies of one file number their snapshots independently, because a
    /// snapshot belongs to a copy's own editing.
    pub fn next_snapshot(&self, copy: u32) -> u32 {
        self.snapshots_of(copy)
            .iter()
            .map(|s| s.n)
            .max()
            .map_or(1, |m| m + 1)
    }

    /// Append a snapshot to `copy`, creating the copy's entry if it has none.
    pub fn add_snapshot(&mut self, copy: u32, snapshot: Snapshot) {
        match self.copies.iter_mut().find(|c| c.copy == copy) {
            Some(c) => c.snapshots.push(snapshot),
            None => self.copies.push(CopyEntry {
                copy,
                name: None,
                edits: Edits::default(),
                snapshots: vec![snapshot],
            }),
        }
        self.copies.sort_by_key(|c| c.copy);
    }

    /// Take snapshot `n` out of `copy`, **returning it** so an undo can put it back.
    ///
    /// ⚠ Returned rather than discarded because nothing else holds it — `ActionKind::CopyDelete`
    /// carries a whole `CopyEntry` for exactly this reason.
    pub fn remove_snapshot(&mut self, copy: u32, n: u32) -> Option<Snapshot> {
        let c = self.copies.iter_mut().find(|c| c.copy == copy)?;
        let i = c.snapshots.iter().position(|s| s.n == n)?;
        Some(c.snapshots.remove(i))
    }

    /// Rename snapshot `n` of `copy`. Reports whether it was there to rename.
    pub fn set_snapshot_name(&mut self, copy: u32, n: u32, name: Option<String>) -> bool {
        let Some(c) = self.copies.iter_mut().find(|c| c.copy == copy) else {
            return false;
        };
        match c.snapshots.iter_mut().find(|s| s.n == n) {
            Some(s) => {
                s.name = name;
                true
            }
            None => false,
        }
    }
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p app --lib sidecar:: 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Sabotage the numbering**

Change `next_snapshot`'s body to `self.snapshots_of(copy).len() as u32 + 1`.
Run: `cargo test -p app --lib sidecar::tests::snapshot_numbers_are_never_reused 2>&1 | tail -20`
Expected: **FAIL** with `left: 3, right: 4`. Confirm a `test result:` line appears, then restore.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/sidecar.rs
git commit -m "feat(sidecar): snapshot numbering, add, remove and rename"
```

---

### Task 5: The two undo actions

**Files:**
- Modify: `crates/app/src/action_log.rs` (the `ActionKind` enum around line 19, and `reachable` around line 115)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` at the bottom of `crates/app/src/action_log.rs`. First read the existing helpers there — there is already a `fn id(p: &str) -> PhotoId`-style helper and a `PhotoId` import; reuse them rather than writing new ones. If the helper is named differently, adapt these calls to it.

```rust
    fn a_snapshot() -> Box<crate::sidecar::Snapshot> {
        Box::new(crate::sidecar::Snapshot {
            n: 1,
            name: None,
            taken: 0,
            edits: Edits::default(),
        })
    }

    /// ⚠ Scoped to its own photo, exactly as `CopyRename` is: Ctrl+Z while looking at another
    /// photo must not delete or resurrect a snapshot you cannot see.
    #[test]
    fn a_snapshot_deletion_is_reachable_only_from_its_own_photo() {
        let mine = PhotoId::original(PathBuf::from("/a.RAF"));
        let other = PhotoId::original(PathBuf::from("/b.RAF"));
        let mut log = ActionLog::default();
        log.push(
            ActionKind::SnapshotDelete {
                id: mine.clone(),
                entry: a_snapshot(),
            },
            Some(&mine),
        );

        assert!(
            log.undo(Some(&other)).is_none(),
            "unreachable from another photo"
        );
        assert!(log.undo(Some(&mine)).is_some(), "reachable from its own");
    }

    #[test]
    fn a_snapshot_rename_is_reachable_only_from_its_own_photo() {
        let mine = PhotoId::new(PathBuf::from("/a.RAF"), 1);
        let other = PhotoId::original(PathBuf::from("/a.RAF"));
        let mut log = ActionLog::default();
        log.push(
            ActionKind::SnapshotRename {
                id: mine.clone(),
                n: 1,
                from: None,
                to: Some("Warm".to_string()),
            },
            Some(&mine),
        );

        assert!(
            log.undo(Some(&other)).is_none(),
            "copy 0 must not reach copy 1's snapshot rename"
        );
        assert!(log.undo(Some(&mine)).is_some());
    }
```

⚠ Check the exact signatures of `ActionLog::push` and `ActionLog::undo` in this file before writing — read the surrounding tests and match them.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app --lib action_log::tests::a_snapshot_deletion 2>&1 | tail -20`
Expected: FAIL to compile — `no variant named SnapshotDelete`.

- [ ] **Step 3: Write the implementation**

Add two variants to `ActionKind` in `crates/app/src/action_log.rs`, immediately after `CopyDelete`:

```rust
    /// A deleted snapshot, held so undo can put it back.
    ///
    /// ⚠ Carries the whole entry because nothing else holds it — `CopyDelete`'s shape and its
    /// reason. Boxed for the reason `Sync` is: an inline `Edits` would make **every** entry in
    /// the log that large, including the zero-sized `PhotoEdit`, which is by far the commonest.
    ///
    /// ⚠ **Taking** a snapshot has no variant, deliberately. It follows `create_virtual_copy`,
    /// which pushes nothing while `delete_virtual_copy` does: creation is additive and reversed
    /// by deleting, deletion destroys the only record of a state. It is also the same rule the
    /// design states — taking a snapshot is not an undo step, because it changes no `Edits`.
    SnapshotDelete {
        id: PhotoId,
        entry: Box<crate::sidecar::Snapshot>,
    },
    /// A snapshot renamed — `CopyRename`'s shape exactly, and scoped the same way.
    SnapshotRename {
        id: PhotoId,
        n: u32,
        from: Option<String>,
        to: Option<String>,
    },
```

Then extend `reachable`'s first arm so both are photo-scoped:

```rust
        ActionKind::PhotoEdit { id }
        | ActionKind::CopyRename { id, .. }
        | ActionKind::SnapshotDelete { id, .. }
        | ActionKind::SnapshotRename { id, .. } => open == Some(id),
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p app --lib action_log:: 2>&1 | tail -5`
Expected: PASS.

Run: `cargo build --workspace --all-targets 2>&1 | tail -30`
Expected: **failures** in `crates/app/src/main.rs` — `undo_one` and `redo_one` match on `ActionKind` exhaustively and now do not cover the two new variants. That is the exhaustive match doing its job.

- [ ] **Step 5: Add temporary arms so the workspace builds**

⚠ Plan 2 replaces these with real behaviour. They exist only so this plan's commits each build and pass, which is non-negotiable.

In `crates/app/src/main.rs`, add to **both** `undo_one` and `redo_one`:

```rust
            // ⚠ Wired up in plan 2, with `snapshots_of`/`add_snapshot`/`remove_snapshot`. The
            // variants exist first so the log's scoping can be tested headlessly.
            ActionKind::SnapshotDelete { .. } | ActionKind::SnapshotRename { .. } => {}
```

- [ ] **Step 6: Run the whole suite**

Run: `cargo test --all 2>&1 | tail -20`
Expected: PASS. ⚠ **Check the exit code, not a grep for failures** — a build failure emits no `test result:` lines at all and reads as clean.

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20; echo "clippy exit: $?"`
Expected: `clippy exit: 0`.

- [ ] **Step 7: Sabotage the scoping**

In `reachable`, move `SnapshotDelete` out of that arm into the global arm beside `CopyDelete`.
Run: `cargo test -p app --lib action_log::tests::a_snapshot_deletion 2>&1 | tail -20`
Expected: **FAIL** on `unreachable from another photo`. Confirm a `test result:` line, then restore.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/action_log.rs crates/app/src/main.rs
git commit -m "feat(action-log): photo-scoped snapshot delete and rename actions"
```

---

### Task 6: The strip's persisted height

**Files:**
- Modify: `crates/app/src/settings.rs` (the `Settings` struct ~line 206, the consts ~line 212, `impl Settings` ~line 219, `impl Default` ~line 269)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/app/src/settings.rs`:

```rust
    /// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts **every**
    /// setting. The sibling of `a_settings_file_from_before_the_panel_sections_still_loads`.
    #[test]
    fn a_settings_file_from_before_the_snapshot_strip_still_loads() {
        let f = TempFile::new("presnapstrip");
        std::fs::write(&f.0, r#"{"library_cell":200}"#).unwrap();
        let got = SettingsStore::new(f.0.clone()).load();
        assert_eq!(got.library_cell, 200, "the stored field must survive");
        assert_eq!(
            got.snapshot_strip_height, DEFAULT_STRIP_HEIGHT,
            "the new one must fall back to its default"
        );
    }

    #[test]
    fn a_nonsense_stored_strip_height_is_clamped_into_range() {
        let mut s = Settings::default();
        s.snapshot_strip_height = 0;
        assert_eq!(s.clamped_strip_height(), STRIP_HEIGHT_RANGE.0 as f32);
        s.snapshot_strip_height = 99_999;
        assert_eq!(s.clamped_strip_height(), STRIP_HEIGHT_RANGE.1 as f32);
    }

    /// ⚠ The default must sit inside the range, or the very first frame is clamped to something
    /// nobody chose.
    #[test]
    fn the_default_strip_height_is_within_its_own_range() {
        assert!(DEFAULT_STRIP_HEIGHT >= STRIP_HEIGHT_RANGE.0);
        assert!(DEFAULT_STRIP_HEIGHT <= STRIP_HEIGHT_RANGE.1);
    }
```

⚠ Check the existing helper's name in that test module — the settings tests use a temp-file guard; read `a_settings_file_from_before_the_panel_sections_still_loads` and copy its exact construction rather than assuming `TempFile::new`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app --lib settings::tests::a_settings_file_from_before_the_snapshot 2>&1 | tail -20`
Expected: FAIL to compile — `no field snapshot_strip_height`.

- [ ] **Step 3: Write the implementation**

Add the field to `Settings` in `crates/app/src/settings.rs`, after `panel_sections`. ⚠ **No serde
attribute on the field**, exactly as `display_space` has none: `Settings` carries a
**container-level** `#[serde(default)]`, so a missing field is filled from
`Settings::default()` — which gives `DEFAULT_STRIP_HEIGHT`. A field-level `#[serde(default)]`
would use the *field type's* default instead, i.e. `0u32`, silently handing every existing user a
collapsed strip.

```rust
    /// The Develop snapshot strip's outer height in whole points.
    ///
    /// ⚠ `u32`, not `f32`, for the two reasons `library_cell` already carries: `Settings` derives
    /// `Eq`, which no float satisfies; and it makes a NaN height **unrepresentable** rather than
    /// something to guard against — NaN defeats `clamp` entirely, since it fails every comparison
    /// and comes back unchanged.
    ///
    /// ⚠ Still clamped by [`Settings::clamped_strip_height`] rather than trusted: this is read
    /// from a file a user can edit.
    ///
    /// ⚠ Written when the resize drag **settles**, never per frame — a resize runs at 60 Hz. The
    /// `apply_view_demosaic` rule.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field already falls back to this struct's `Default`. A field-level one would give
    /// `0` instead, which clamps to a collapsed strip.
    pub snapshot_strip_height: u32,
```

Add the constants beside `CELL_STEP`:

```rust
/// The snapshot strip's default outer height — about four rows at the compact metrics.
pub const DEFAULT_STRIP_HEIGHT: u32 = 110;
/// The smallest and largest the strip can be dragged to.
///
/// ⚠ The minimum is the header row alone, so someone who wants the space back can drag it away
/// once and have it stay gone. The maximum here is a floor under the *stored* value; the panel
/// additionally caps itself at half the live panel height, so a shrunken window clamps further.
pub const STRIP_HEIGHT_RANGE: (u32, u32) = (28, 400);
```

Add the accessor to `impl Settings`, beside `clamped_cell`:

```rust
    /// The snapshot strip's height in points, guaranteed usable.
    pub fn clamped_strip_height(&self) -> f32 {
        self.snapshot_strip_height
            .clamp(STRIP_HEIGHT_RANGE.0, STRIP_HEIGHT_RANGE.1) as f32
    }
```

Add to `impl Default for Settings`, after `panel_sections`:

```rust
            snapshot_strip_height: DEFAULT_STRIP_HEIGHT,
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p app --lib settings:: 2>&1 | tail -5`
Expected: PASS.

Run: `cargo build --workspace --all-targets 2>&1 | tail -20`
Expected: **failure** in `settings.rs`'s own `settings_round_trip_carries_the_export_options`, which uses an exhaustive `Settings { .. }` literal. That literal is exhaustive **on purpose** — it exists so the next field is a compile error. Add `snapshot_strip_height: DEFAULT_STRIP_HEIGHT + 20,` to it and assert it round trips, matching how `library_cell` is handled there.

- [ ] **Step 5: Run the whole suite and clippy**

Run: `cargo test --all 2>&1 | tail -20`
Expected: PASS.

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -20; echo "clippy exit: $?"`
Expected: `clippy exit: 0`.

Run: `cargo fmt --check && echo "fmt clean"`
Expected: `fmt clean`.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/settings.rs
git commit -m "feat(settings): the snapshot strip's persisted height"
```

---

### Task 7: Close the plan

- [ ] **Step 1: Confirm the goldens did not move**

Run: `cargo test -p pipeline --test golden 2>&1 | tail -5`
Expected: 47 passing. ⚠ **Not one golden may move.** This plan touches no pipeline code; a moved golden means something is wrong, and regenerating would bake the error into every reference.

- [ ] **Step 2: Count the tests**

Run: `cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{s+=$4} END {print s}'`
Record the number in the handoff note for plan 2. The baseline before this plan was **1165**.

- [ ] **Step 3: Verify nothing in the GUI is reachable yet**

Run: `grep -rn "Snapshot" crates/app/src/develop crates/app/src/shell.rs 2>/dev/null | wc -l`
Expected: `0`. Plan 1 is invisible by design — if you can tell it shipped, something has leaked.
