# Auto-advance after rating (M6.5c) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rating a photo moves on to the next one, so a culling pass is one keystroke per frame
instead of two.

**Architecture:** One rule — `MarkAction::advances()` — consulted by the two views that can rate
(the Library grid and Compare), gated by `Settings.auto_advance`. In the grid the advance is
`grid_nav::advance_past`, a new pure function that steps one past the **last** selected visible
photo. In Compare it is literally the arrow-key step on the bordered pane, reached through the
same code path the arrows use.

**Tech Stack:** Rust, egui 0.35 headless harness (`test_ui` / `test_app`), serde.

---

## ⚠⚠ A correction to the spec, made before any code was written

The spec's §2 reads *"In the grid and the filmstrip, advance PAST the whole selection."*

**The filmstrip cannot rate anything, so there is nothing there to advance.** `crate::interaction::culling_key`
has exactly three callers — `library.rs`, `multi/compare.rs`, `multi/survey.rs` — and
`filmstrip.rs` contains no `key_pressed` at all. That is not an oversight: `CLAUDE.md` records the
rule that resolves the digit collision, *"digits cull in Library, select tools in Develop"*, and the
filmstrip lives in Develop.

So the scope is **the Library grid and Compare**. Survey stays untouched for the reason the spec
gives (it is subtractive). This correction must land in the spec and in `CLAUDE.md` at Task 8.

## ⚠⚠ The one ordering hazard, and how the design makes it unrepresentable

`RustroomApp::apply_mark` reads the targets **out of the live selection**. Auto-advance *moves* the
selection. Do those in the wrong order and rating a run of five frames rates the wrong photo — a
data bug with nothing on screen to announce it.

The fix is not a comment. `library.rs`'s deferred `mark` local becomes
`Option<(Vec<PhotoId>, MarkAction)>`: the ids are captured **in the same expression that records the
mark**, before the advance below can touch the selection. A mark without its captured targets is
then not representable. `apply_mark` is deleted and its reasoning moves to `Library::selected_ids`,
so there is still exactly one rule about which photos a culling key acts on.

**Why the advance is safe under `refilter`.** `apply_mark_to` ends in `reload_marks`, which
refilters and calls `retain_visible`. That prunes photos the *new* marks have hidden — which are the
photos just rated, and they are no longer selected. The photo advanced *to* was not rated, so no
filter change can touch it and it survives the prune. This is why the advance may happen before the
mark is applied, which is in turn what lets `centre_on` be set on the same frame.

---

## File structure

| file | what changes |
|---|---|
| `crates/app/src/marks.rs` | `MarkAction::advances()` — the rule, beside the type |
| `crates/app/src/grid_nav.rs` | `advance_past(visible, selected)` — pure, testable without a window |
| `crates/app/src/settings.rs` | `Settings.auto_advance: bool` (default `true`) + two tests |
| `crates/app/src/library_state.rs` | `Library::selected_ids()` |
| `crates/app/src/catalog_ops.rs` | delete `RustroomApp::apply_mark` |
| `crates/app/src/library.rs` | capture targets, advance, four headless tests. ⚠ **970 lines already** |
| `crates/app/src/multi/compare.rs` | one step block serving arrows *and* auto-advance, two tests |
| `crates/app/src/modals/settings.rs` | the checkbox and its change-guarded write-back |
| `docs/superpowers/specs/2026-09-07-auto-advance-design.md`, `CLAUDE.md` | the filmstrip correction |

⚠ `crates/app/src/library.rs` is **970 lines**, well past the ~600 marker. This plan adds ~35
production lines and ~110 test lines to it. Reported rather than silently added to; splitting it is
its own change and is not in scope here.

---

### Task 1: `MarkAction::advances()`

**Files:**
- Modify: `crates/app/src/marks.rs`

- [ ] **Step 1: Write the failing test**

Add inside `mod tests` in `crates/app/src/marks.rs`:

```rust
    /// ⚠⚠ **Only a decision advances; a correction does not.** `0` clears a rating and `U` clears
    /// a flag — both *undo* a judgement rather than making one, and moving on after a correction
    /// walks away from the very photo you were fixing. Survey already records the same reasoning
    /// about the same key: "un-flagging undoes a decision, so a photo made undecided again
    /// belongs back on the wall."
    ///
    /// ⚠ A deliberate divergence from Lightroom, which advances on every rating including 0.
    ///
    /// ⚠ Asserted over the **whole** vocabulary rather than on one example: a rule that
    /// enumerates a list stops guarding the moment the list grows, and this file's own
    /// `action_for` is that list.
    #[test]
    fn only_a_decision_advances() {
        for r in 1..=5 {
            assert!(
                MarkAction::Rating(r).advances(),
                "{r} stars is a decision and must advance"
            );
        }
        for l in 1..=4 {
            assert!(MarkAction::Label(l).advances(), "a colour label is a decision");
        }
        assert!(MarkAction::Flag(Flag::Pick).advances());
        assert!(MarkAction::Flag(Flag::Reject).advances());

        assert!(
            !MarkAction::Rating(0).advances(),
            "0 clears a rating — a correction, not a decision"
        );
        assert!(
            !MarkAction::Flag(Flag::None).advances(),
            "U clears a flag — a correction, not a decision"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p app --lib only_a_decision_advances
```

Expected: FAIL — `no method named advances found`.

- [ ] **Step 3: Implement it**

In `crates/app/src/marks.rs`, add immediately after the `MarkAction` enum declaration:

```rust
impl MarkAction {
    /// Whether applying this mark should move on to the next photo.
    ///
    /// ⚠⚠ **Beside the type, never in a view.** The grid and Compare both consult it, and a rule
    /// written in two places is one that will disagree in one of them — the rule
    /// `EDIT_GROUP_NAMES`, `Tool::ALL`, `FlagSet::ALL` and `DragQuality` already follow.
    ///
    /// ⚠ `Rating(0)` and `Flag::None` are **corrections**: they clear a judgement rather than
    /// making one, and advancing off a photo you were in the middle of fixing is the one thing
    /// that would make this feature annoying rather than fast. Survey records the same reasoning
    /// about `U` for the same reason. A deliberate divergence from Lightroom.
    pub fn advances(self) -> bool {
        !matches!(self, MarkAction::Rating(0) | MarkAction::Flag(Flag::None))
    }
}
```

- [ ] **Step 4: Run it and watch it pass**

```bash
cargo test -p app --lib only_a_decision_advances
```

Expected: PASS, `1 passed`. ⚠ Read the passed-count, not the `ok` — a filter that matches nothing
also prints `ok`.

- [ ] **Step 5: Sabotage it**

Change the body to `true`, re-run, confirm the test **fails** naming the correction, then restore.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/marks.rs
git commit -m "$(cat <<'EOF'
Add MarkAction::advances, the one rule about moving on

Beside the type rather than in a view: the grid and Compare both consult
it. Rating(0) and Flag::None are corrections and do not advance.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `grid_nav::advance_past`

**Files:**
- Modify: `crates/app/src/grid_nav.rs`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/app/src/grid_nav.rs` (create the module if the file has none — check
with `grep -n "mod tests" crates/app/src/grid_nav.rs` first and append to the existing one):

```rust
    /// ⚠⚠ **Past the whole selection, not one step from the active photo.** A culling key acts on
    /// the *selection* — "select a run of frames and press X" — so stepping one from the active
    /// photo would leave you standing inside the run you just rated.
    #[test]
    fn advance_past_steps_past_the_whole_selection() {
        let visible: Vec<usize> = (0..10).collect();
        assert_eq!(
            advance_past(&visible, &[3, 4, 5]),
            Some(6),
            "landed inside the run that was just rated"
        );
        // ⚠ The order the caller happens to hand them in must not matter.
        assert_eq!(advance_past(&visible, &[5, 3, 4]), Some(6));
    }

    /// ⚠ Clamped, never wrapping — `step_by`'s rule, and for its reason: wrapping to the top of a
    /// folder mid-cull is disorienting, and the end of the list is information.
    #[test]
    fn advance_past_clamps_at_the_end() {
        let visible: Vec<usize> = (0..4).collect();
        assert_eq!(advance_past(&visible, &[3]), Some(3), "it wrapped or vanished");
    }

    /// ⚠ A hidden photo cannot be the anchor: the selection is pruned to the visible list by
    /// `retain_visible`, so one that is not there is stale and stepping from it would jump
    /// somewhere nobody selected.
    #[test]
    fn advance_past_ignores_a_selection_with_nothing_visible_in_it() {
        let visible = vec![0usize, 2, 4];
        assert_eq!(advance_past(&visible, &[1, 3]), None);
        assert_eq!(advance_past(&visible, &[]), None);
    }
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p app --lib advance_past
```

Expected: FAIL — `cannot find function advance_past`.

- [ ] **Step 3: Implement it**

Add to `crates/app/src/grid_nav.rs`, directly below `step_by`:

```rust
/// The photo to land on after rating `selected`.
///
/// ⚠⚠ **One past the LAST selected photo, not one past the active one.** A culling key acts on the
/// whole selection, so a single step from the active photo lands *inside* the run just rated.
///
/// ⚠ `visible` is sorted ascending, so the largest selected visible index **is** the last one in
/// visible order — no separate ordering pass, and none should be added: a second notion of "last"
/// is how the two would drift.
///
/// ⚠ `None` when nothing selected is visible. The selection is kept inside `visible` by
/// `retain_visible`, so that means there is nothing to advance from.
pub fn advance_past(visible: &[usize], selected: &[usize]) -> Option<usize> {
    let last = selected
        .iter()
        .copied()
        .filter(|i| visible.binary_search(i).is_ok())
        .max()?;
    step_by(visible, last, 1)
}
```

- [ ] **Step 4: Run them and watch them pass**

```bash
cargo test -p app --lib advance_past
```

Expected: PASS, `3 passed`.

- [ ] **Step 5: Sabotage it**

Swap `.max()` for `.min()`, re-run, confirm `advance_past_steps_past_the_whole_selection` fails with
`Some(4)` against `Some(6)`, then restore.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/grid_nav.rs
git commit -m "$(cat <<'EOF'
Add grid_nav::advance_past, one step past a whole selection

A culling key acts on the selection, so a single step from the active
photo lands inside the run just rated. Clamps like step_by.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `Settings.auto_advance`

**Files:**
- Modify: `crates/app/src/settings.rs`

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/app/src/settings.rs`, beside its siblings:

```rust
    #[test]
    fn a_settings_file_from_before_auto_advance_still_loads() {
        // ⚠⚠ The forgiving-load property, and the trap `snapshot_strip_height` and `write_xmp`
        // both document: `load()` is infallible, so a field that fails to parse does not surface
        // an error — it silently reverts *every* setting. A **field-level** `#[serde(default)]`
        // would be worse still: it would use `bool`'s own default of `false` and ship the feature
        // disabled for everyone who already has a settings file, which is most people.
        //
        // ⚠ Asserts the **value**, not the mechanism, so it catches either way round.
        let f = TempFile::new("auto_advance_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert!(
            loaded.auto_advance,
            "an existing settings file must get the feature, not silently opt out of it"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p app --lib a_settings_file_from_before_auto_advance_still_loads
```

Expected: FAIL — `no field auto_advance on type Settings`.

- [ ] **Step 3: Add the field**

In `crates/app/src/settings.rs`, add to the end of `pub struct Settings` (after `drag_quality`):

```rust
    /// Move on to the next photo after a rating, a flag or a colour label.
    ///
    /// ⚠ Defaults to **true**: it is the whole point of M6.5c, and a culling feature nobody finds
    /// is a feature that did not ship. The correction case that usually argues for defaulting off
    /// is answered by [`crate::marks::MarkAction::advances`] instead — `0` and `U` do not advance,
    /// so fixing a mis-rating does not run away from you.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field falls back to this struct's `Default` — which is `true`. A field-level one
    /// would use `bool`'s own default of `false` instead, silently disabling the feature for every
    /// existing user. The same trap `write_xmp` and `snapshot_strip_height` both document.
    pub auto_advance: bool,
```

And to `impl Default for Settings`, after `drag_quality: DragQuality::default(),`:

```rust
            auto_advance: true,
```

- [ ] **Step 4: Add it to the exhaustive round-trip literal**

In `settings_round_trip_carries_the_export_options`, after `drag_quality: DragQuality::Full,`:

```rust
            // Non-default for the same reason as its neighbours: the preference defaults to *on*,
            // so persisting `false` is what proves it round-trips rather than being re-defaulted.
            auto_advance: false,
```

⚠ That literal is deliberately exhaustive — it is what turned this field into a compile error here,
exactly as it did for `drag_quality`.

- [ ] **Step 5: Run both and watch them pass**

```bash
cargo test -p app --lib settings:: 2>&1 | tail -5
```

Expected: PASS, and the passed-count unchanged apart from the one new test.

- [ ] **Step 6: Sabotage it**

Put `#[serde(default)]` on the `auto_advance` field, re-run
`a_settings_file_from_before_auto_advance_still_loads`, confirm it **fails**
(`left: false, right: true`), then restore.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/settings.rs
git commit -m "$(cat <<'EOF'
Add Settings.auto_advance, default on

No field-level serde default: that would use bool's own false and ship
the feature disabled for every existing settings file.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `Library::selected_ids`, replacing `RustroomApp::apply_mark`

**Files:**
- Modify: `crates/app/src/library_state.rs`
- Modify: `crates/app/src/catalog_ops.rs:409-426`
- Modify: `crates/app/src/library.rs` (the one call site)

⚠ This task lands as **one** commit. `apply_mark` has exactly one caller; removing it and changing
that caller in separate commits leaves a `dead_code` failure under `-D warnings` in between — the
binary-crate trap this project has recorded at least ten times.

- [ ] **Step 1: Add `selected_ids`**

In `crates/app/src/library_state.rs`, inside `impl Library`, directly after `refilter`:

```rust
    /// Every selected row's [`PhotoId`].
    ///
    /// ⚠⚠ **The selection, not the active photo** — the rule a paste, a preset and a sync already
    /// follow, and culling is where it matters most: select a run of frames and press `X`.
    ///
    /// ⚠ It exists as a method rather than being read inline **so the targets can be captured
    /// before auto-advance moves the selection.** `library.rs` records that ordering; this is the
    /// one place that decides which photos a culling key acts on.
    pub(crate) fn selected_ids(&self) -> Vec<PhotoId> {
        self.selection
            .iter()
            .filter_map(|i| self.entries.get(i))
            .map(|e| e.id.clone())
            .collect()
    }
```

- [ ] **Step 2: Delete `apply_mark`**

In `crates/app/src/catalog_ops.rs`, delete the whole of this (the doc comment and the function —
its reasoning has moved to `selected_ids` above):

```rust
    /// Apply a culling action to every selected photo.
    ///
    /// ⚠ The **selection**, not the active photo — the rule a paste, a preset and a sync already
    /// follow, and culling is where it matters most: select a run of frames and press `X`.
    pub(crate) fn apply_mark(&mut self, action: rustroom_app::marks::MarkAction) {
        let chosen: Vec<PhotoId> = self
            .library
            .as_ref()
            .map(|l| {
                l.selection
                    .iter()
                    .filter_map(|i| l.entries.get(i))
                    .map(|e| e.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.apply_mark_to(&chosen, action);
    }
```

- [ ] **Step 3: Change the `mark` local to carry its targets**

In `crates/app/src/library.rs`, replace:

```rust
    let mut mark: Option<rustroom_app::marks::MarkAction> = None;
```

with:

```rust
    // ⚠⚠ **The targets travel WITH the action, and that is the whole safety of auto-advance.**
    // The advance below moves the selection, and `apply_mark` used to read the targets out of the
    // live selection *after* the frame — so rating a run of five frames would have rated the wrong
    // photo, silently. Carrying the ids here makes "a mark without its targets" unrepresentable
    // rather than something a comment asks the next reader to remember.
    let mut mark: Option<(Vec<PhotoId>, rustroom_app::marks::MarkAction)> = None;
```

And replace the call site:

```rust
    if let Some(a) = mark {
        app.apply_mark(a);
    }
```

with:

```rust
    if let Some((targets, a)) = mark {
        app.apply_mark_to(&targets, a);
    }
```

And in the input closure, replace:

```rust
            mark = crate::interaction::culling_key(i);
```

with:

```rust
            if let Some(a) = crate::interaction::culling_key(i) {
                mark = Some((lib.selected_ids(), a));
            }
```

- [ ] **Step 4: Build and run the whole app suite**

```bash
cargo test -p app 2>&1 | tail -5
```

Expected: PASS, the count unchanged from before this task — this moves no behaviour, and a changed
count would mean it did.

- [ ] **Step 5: Clippy**

```bash
cargo clippy -p app --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "EXIT=$?"
```

Expected: `EXIT=0`. ⚠ Read the exit code from its own line, never through a pipe.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/library_state.rs crates/app/src/catalog_ops.rs crates/app/src/library.rs
git commit -m "$(cat <<'EOF'
Capture a culling key's targets with the action

apply_mark read the targets out of the live selection after the frame,
which auto-advance is about to move. Carrying the ids alongside the
action makes the wrong order unrepresentable; apply_mark is replaced by
Library::selected_ids, so one place still decides who a key acts on.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: the grid advances

**Files:**
- Modify: `crates/app/src/library.rs`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/app/src/library.rs`. The first helper mirrors `compare.rs`'s:

```rust
    fn grid_frame(app: &mut RustroomApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(crate::test_ui::screen()),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            show(app, ui, &ctx);
        });
    }

    fn grid_key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn grid_app(n: usize) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.view = crate::AppView::Library;
        app.library = Some(crate::test_app::library_of(n));
        // ⚠ Without this `set_marks` is a no-op on an uncatalogued photo and every rating
        // assertion below fails against correct code — see `catalog_library`.
        crate::test_app::catalog_library(&mut app);
        app
    }

    /// ⚠⚠ **The carrier test, and both halves matter.** That the selection moved is the feature;
    /// that the photos rated are the ones that *were* selected is the bug the capture ordering
    /// exists to prevent — an implementation that advanced first would rate row 6 and leave the
    /// run untouched, and an assertion on the selection alone would pass against it.
    #[test]
    fn rating_advances_past_the_whole_selection() {
        let ctx = crate::test_ui::ctx();
        let mut app = grid_app(12);
        app.library
            .as_mut()
            .unwrap()
            .selection
            .select_indices(&[3, 4, 5]);

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::Num3)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(
            lib.selection.active(),
            Some(6),
            "it did not step past the run that was just rated"
        );
        for row in [3usize, 4, 5] {
            assert_eq!(
                lib.marks.get(&lib.entries[row].id).map(|m| m.rating),
                Some(3),
                "row {row} was selected and should have been rated"
            );
        }
        assert_eq!(
            lib.marks.get(&lib.entries[6].id).map(|m| m.rating),
            None,
            "the photo advanced TO was rated — the targets were captured after the advance"
        );
    }

    /// ⚠ A correction must not walk away from the photo you are fixing.
    #[test]
    fn a_correction_does_not_advance() {
        let ctx = crate::test_ui::ctx();
        let mut app = grid_app(12);
        app.library.as_mut().unwrap().selection.click(4);

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::Num0)]);
        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(4),
            "clearing a rating moved on"
        );

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::U)]);
        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(4),
            "un-flagging moved on"
        );
    }

    /// ⚠ The setting genuinely gates it — and the rating must still land, or "off" would mean
    /// "the key does nothing" rather than "the key does not move on".
    #[test]
    fn auto_advance_off_leaves_the_selection_alone() {
        let ctx = crate::test_ui::ctx();
        let mut app = grid_app(12);
        app.settings.auto_advance = false;
        app.library.as_mut().unwrap().selection.click(4);

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::Num3)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.selection.active(), Some(4), "it advanced anyway");
        assert_eq!(
            lib.marks.get(&lib.entries[4].id).map(|m| m.rating),
            Some(3),
            "turning advancing off also stopped the rating"
        );
    }

    /// ⚠ Clamped, never wrapping: the end of the list is information, and being thrown back to
    /// the top of a folder mid-cull is disorienting.
    #[test]
    fn rating_the_last_photo_stays_there() {
        let ctx = crate::test_ui::ctx();
        let mut app = grid_app(5);
        app.library.as_mut().unwrap().selection.click(4);

        grid_frame(&mut app, &ctx, vec![grid_key(egui::Key::Num3)]);

        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(4),
            "it wrapped to the top of the folder"
        );
    }
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p app --lib library::tests 2>&1 | tail -20
```

Expected: `rating_advances_past_the_whole_selection` and `rating_the_last_photo_stays_there` FAIL
(the selection did not move). The other two pass already — that is fine and expected; they are the
guards on the gate, not on the feature.

- [ ] **Step 3: Hoist the setting**

In `crates/app/src/library.rs`, beside the other values copied out of `app` before the closure
borrows `app.library` (next to `let scroll_was = app.grid_scroll;`), add:

```rust
    // ⚠ Hoisted before the closure borrows `app.library`, like `scroll_was` and the composition
    // guide: reading `app.settings` inside would be a second borrow of `app`.
    let auto_advance = app.settings.auto_advance;
```

- [ ] **Step 4: Advance where the mark is recorded**

Replace the block written in Task 4:

```rust
            if let Some(a) = crate::interaction::culling_key(i) {
                mark = Some((lib.selected_ids(), a));
            }
```

with:

```rust
            if let Some(a) = crate::interaction::culling_key(i) {
                // ⚠⚠ **Targets first, then move.** `selected_ids` is read before the advance
                // below touches the selection; the other order rates the photo you moved *to*.
                mark = Some((lib.selected_ids(), a));
                // ⚠⚠ Auto-advance is the arrow key's own step, reached through the same two
                // lines — `step_by` then `centre_on` — so the grid cannot come to disagree with
                // itself about where "the next photo" is.
                //
                // ⚠ It happens **before** the mark is applied, which is what lets `centre_on` be
                // set on this frame rather than needing a field to carry it to the next one.
                // Safe because `apply_mark_to` refilters, and `retain_visible` only prunes what
                // the *new* marks hid — which is the run just rated, no longer selected. The
                // photo advanced to was not rated, so no filter change can reach it.
                if auto_advance && a.advances() {
                    let selected: Vec<usize> = lib.selection.iter().collect();
                    if let Some(n) = rustroom_app::grid_nav::advance_past(&lib.visible, &selected) {
                        lib.selection.click(n);
                        centre_on = lib.visible.binary_search(&n).ok();
                    }
                }
            }
```

- [ ] **Step 5: Run them and watch them pass**

```bash
cargo test -p app --lib library::tests 2>&1 | tail -5
```

Expected: PASS, `19 passed` (15 before, 4 new).

- [ ] **Step 6: Sabotage it, twice**

1. Move the `mark = Some(..)` line to *after* the advance block. Re-run: confirm
   `rating_advances_past_the_whole_selection` fails on the last assertion — *"the photo advanced TO
   was rated"*. Restore.
2. Drop `&& a.advances()`. Re-run: confirm `a_correction_does_not_advance` fails. Restore.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/library.rs
git commit -m "$(cat <<'EOF'
The grid advances past the whole selection after a rating

The arrow key's own step, reached through the same two lines. Runs
before the mark is applied so the view follows on the same frame; safe
because retain_visible only prunes what the new marks hid.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Compare advances the bordered pane

**Files:**
- Modify: `crates/app/src/multi/compare.rs`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/app/src/multi/compare.rs`:

```rust
    /// ⚠⚠ **Auto-advance is exactly the arrow-key step on the bordered pane**, and both halves
    /// matter: rating the candidate must move the candidate, and rating the *reference* must move
    /// the reference. "The border is where the buttons work" has to mean every key or it means
    /// none of them.
    ///
    /// ⚠ This is **not** the removal that was reversed on 2026-09-02. Removal takes a frame out
    /// of the comparison; advancing steps to the next frame to compare against.
    #[test]
    fn rating_advances_the_bordered_pane() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(8);
        app.library.as_mut().unwrap().selection.click(2);
        crate::test_app::catalog_library(&mut app);

        // Border on the candidate: the right pane moves, the reference holds.
        app.compare_focus = 1;
        let before = app.compare_candidate.clone();
        frame(&mut app, &ctx, vec![key(egui::Key::Num3)]);
        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(2),
            "the reference moved while the border was on the candidate"
        );
        assert_ne!(
            app.compare_candidate, before,
            "the candidate did not advance"
        );

        // Border on the reference: the left pane moves.
        app.compare_focus = 0;
        frame(&mut app, &ctx, vec![key(egui::Key::Num4)]);
        assert_ne!(
            app.library.as_ref().unwrap().selection.active(),
            Some(2),
            "the reference did not advance while the border was on it"
        );
    }

    /// ⚠ The same correction rule the grid follows, re-asserted here because this view has its
    /// own `ui.input` block and could have forgotten it.
    #[test]
    fn a_correction_does_not_advance_in_compare() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(8);
        app.library.as_mut().unwrap().selection.click(2);
        crate::test_app::catalog_library(&mut app);
        app.compare_focus = 1;
        let before = app.compare_candidate.clone();

        frame(&mut app, &ctx, vec![key(egui::Key::U)]);

        assert_eq!(
            app.compare_candidate, before,
            "clearing a flag moved on to the next candidate"
        );
    }
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p app --lib compare::tests 2>&1 | tail -20
```

Expected: `rating_advances_the_bordered_pane` FAILS — *"the candidate did not advance"*.
`a_correction_does_not_advance_in_compare` passes already; it is the guard on the gate.

- [ ] **Step 3: Hoist the setting**

In `crates/app/src/multi/compare.rs`, beside the existing `let typing = ...` line — but **above** it,
outside anything holding `app.library` — add:

```rust
    // ⚠ Copied out before the closure borrows `app.library`, the shape this file already uses.
    let auto_advance = app.settings.auto_advance;
```

⚠ Place it wherever `app` is still free — the same place `plan(&app, ..)` is called. If the closure
has already borrowed `app.library` at the `typing` line, hoist it further up; the compiler will say
so.

- [ ] **Step 4: Fold the arrows and auto-advance into one step**

Replace the arrow loop:

```rust
            for (key, forward) in [(egui::Key::ArrowRight, true), (egui::Key::ArrowLeft, false)] {
                if !i.key_pressed(key) {
                    continue;
                }
                // ⚠⚠ **The arrows move the BORDERED pane**, which is the same rule the culling
                // keys follow — "the border is where the buttons work" has to mean every key or
                // it means none of them. It shipped moving the candidate whichever pane was
                // bordered, which made the border a half-truth.
                //
                // ⚠ Stepping the reference is `step` with the roles swapped: the function's job
                // is "walk the pool, skipping this one photo", and which of the two is being
                // held still is the caller's business. No second traversal.
                if focus == 0 {
                    if let Some(next) = compare_pair::step(
                        &pool,
                        cand_or(candidate, reference),
                        Some(reference),
                        forward,
                    ) {
                        out.reference = Some(next);
                    }
                } else if let Some(next) = compare_pair::step(&pool, reference, candidate, forward)
                {
                    out.candidate = Some(lib.entries.get(next).map(|e| e.id.clone()));
                }
            }
```

with just the key reading:

```rust
            // ⚠⚠ **What moves the bordered pane is decided ONCE, below.** The arrows and
            // auto-advance both set this flag rather than each carrying their own copy of the
            // step: two implementations of "step the bordered pane" is exactly how the two would
            // come to disagree about which pane the border means.
            let mut step_forward: Option<bool> = None;
            for (key, forward) in [(egui::Key::ArrowRight, true), (egui::Key::ArrowLeft, false)] {
                if i.key_pressed(key) {
                    step_forward = Some(forward);
                }
            }
```

Then extend the culling block. Replace:

```rust
            if let (Some(a), Some(t)) = (crate::interaction::culling_key(i), target) {
                if let Some(e) = lib.entries.get(t) {
                    out.mark = Some((e.id.clone(), a));
                }
```

with:

```rust
            if let (Some(a), Some(t)) = (crate::interaction::culling_key(i), target) {
                if let Some(e) = lib.entries.get(t) {
                    out.mark = Some((e.id.clone(), a));
                }
                // ⚠⚠ **Auto-advance is the arrow's forward step and nothing else** — it goes
                // through the one block below, so the border means the same thing to both keys.
                //
                // ⚠ This is *not* the removal reversed on 2026-09-02: removal takes a frame out
                // of the comparison, advancing steps to the next frame to compare against.
                if auto_advance && a.advances() {
                    step_forward = Some(true);
                }
```

Finally, add the single step block at the **end** of the `ui.input` closure, after the culling
block closes:

```rust
            // ⚠⚠ **The arrows and auto-advance both land here, and this moves the BORDERED
            // pane** — "the border is where the buttons work" has to mean every key or it means
            // none of them. It shipped moving the candidate whichever pane was bordered, which
            // made the border a half-truth.
            //
            // ⚠ Stepping the reference is `step` with the roles swapped: the function's job is
            // "walk the pool, skipping this one photo", and which of the two is being held still
            // is the caller's business. No second traversal.
            //
            // ⚠ Reads the frame's *incoming* `focus`, so an Enter in the same frame moves the
            // border for the next one rather than redirecting this step — which is what the
            // arrows already did.
            if let Some(forward) = step_forward {
                if focus == 0 {
                    if let Some(next) = compare_pair::step(
                        &pool,
                        cand_or(candidate, reference),
                        Some(reference),
                        forward,
                    ) {
                        out.reference = Some(next);
                    }
                } else if let Some(next) = compare_pair::step(&pool, reference, candidate, forward)
                {
                    out.candidate = Some(lib.entries.get(next).map(|e| e.id.clone()));
                }
            }
```

- [ ] **Step 5: Run the whole Compare suite**

```bash
cargo test -p app --lib compare::tests 2>&1 | tail -5
```

Expected: PASS. Every pre-existing arrow test must still pass **unchanged** — that is the evidence
folding the two paths into one moved no behaviour.

- [ ] **Step 6: Sabotage it**

Change the step block's condition to `if focus == 1` (always step the candidate). Re-run: confirm
`rating_advances_the_bordered_pane` fails on *"the reference did not advance"*, **and** that
`the_arrows_move_the_bordered_pane` fails too — one block, two guards. Restore.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/multi/compare.rs
git commit -m "$(cat <<'EOF'
Compare advances the bordered pane after a rating

One block decides what stepping the bordered pane means; the arrows and
auto-advance both set the same flag. Not the removal reversed on
2026-09-02 — that took a frame out of the comparison, this steps to the
next frame to compare against.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: the Settings checkbox

**Files:**
- Modify: `crates/app/src/modals/settings.rs`

⚠ No test: this is egui glue, and the rule it carries (the change guard) is asserted by its
neighbours' own reasoning rather than by a headless test that cannot reach a dialog body.

- [ ] **Step 1: Copy the value out**

In `crates/app/src/modals/settings.rs`, beside the other locals at the top of `show`:

```rust
    let mut auto_advance = app.settings.auto_advance;
```

- [ ] **Step 2: Draw the row**

Immediately after the `write_xmp` hint and its `ui.separator();`, insert:

```rust
            ui.checkbox(&mut auto_advance, "Move to the next photo after rating");
            theme::hint(
                ui,
                "In the Library grid a rating, a flag or a colour label steps past everything \
                 that was selected; in Compare it steps the bordered pane, exactly as the arrow \
                 keys do. Clearing a rating with 0 or a flag with U never moves on — a \
                 correction should leave you on the photo you are fixing.",
            );
            ui.separator();
```

- [ ] **Step 3: Write it back, change-guarded**

Beside the `drag_quality` write-back at the bottom of `show`:

```rust
    // ⚠ A change guard like its neighbours, and for `pan_drag`'s weaker reason: this has no side
    // effect beyond the write, because both views read `settings.auto_advance` afresh every
    // frame. The guard is here so the settings file is not rewritten on every frame the dialog
    // is open.
    if auto_advance != app.settings.auto_advance {
        app.settings.auto_advance = auto_advance;
        app.save_settings();
    }
```

- [ ] **Step 4: Build and check clippy**

```bash
cargo clippy -p app --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "EXIT=$?"
```

Expected: `EXIT=0`.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/modals/settings.rs
git commit -m "$(cat <<'EOF'
Expose auto-advance in Settings

Change-guarded like its neighbours: this body runs every frame the
dialog is open.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: the full gate, then the documents

**Files:**
- Modify: `docs/superpowers/specs/2026-09-07-auto-advance-design.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Run all three gates, each from its own exit code**

```bash
cargo fmt --all > /tmp/f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/t.log 2>&1; echo "TEST=$?"
tail -3 /tmp/t.log
```

Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`. ⚠ Read each exit code from its own line — a pipe reports
the *last* command's status, which is the trap this project has hit three times.

- [ ] **Step 2: Confirm the goldens ran and none moved**

```bash
grep -c "^test golden_" /tmp/t.log
git status --short crates/pipeline/tests
```

Expected: `59`, and no modified reference PNGs. ⚠ A count is the assertion — a filter that silently
ran nothing also prints `ok`.

- [ ] **Step 3: Correct the spec's §2**

In `docs/superpowers/specs/2026-09-07-auto-advance-design.md`, replace the §2 heading

```
### 2. In the grid and the filmstrip, advance PAST the whole selection
```

with

```
### 2. In the Library grid, advance PAST the whole selection

⚠⚠ **CORRECTED WHILE PLANNING: the filmstrip is NOT in scope, because it cannot rate anything.**
`interaction::culling_key` has exactly three callers — `library.rs`, `compare.rs`, `survey.rs` —
and `filmstrip.rs` contains no key handling at all. That follows from the rule `CLAUDE.md` already
records for the digit collision: *digits cull in Library, select tools in Develop*, and the
filmstrip lives in Develop. Scope is the grid and Compare.
```

- [ ] **Step 4: Update `CLAUDE.md`**

Replace the `▶ NEXT ACTION` paragraph in the RESUME HERE block with a section recording M6.5c, the
filmstrip correction, the capture-ordering hazard, and that **M6.2 metadata writing is what remains
of Phase 6**. Add `2026-09-07-auto-advance-design.md` to the specs list and
`2026-09-07-auto-advance.md` to the plans list at the foot of the file.

- [ ] **Step 5: Commit the documents**

```bash
git add docs CLAUDE.md
git commit -m "$(cat <<'EOF'
Record M6.5c, and correct the spec about the filmstrip

The filmstrip cannot rate anything, so there was nothing there to
advance — culling_key has three callers and filmstrip.rs is not one.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 6: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Run in the background. What to look at, since none of it is testable:

1. **The rhythm of a real culling pass.** Open a folder, hold on the grid, press `1`–`5` down a
   run. Does the pace feel right, or does it move before you have looked?
2. **Selecting a run** of five, then rating: it should land after all five, not inside them.
3. **`0` and `U`** must leave you where you are.
4. **Compare**: rate the candidate, it steps; press Enter to move the border, rate again, the
   *reference* steps.
5. **The bottom of the viewport**: advancing off the last visible row should scroll to follow,
   and rating the very last photo in the folder should stay put rather than wrapping.
6. **Settings → "Move to the next photo after rating"**, off and on again.

---

## Self-review

**Spec coverage.** §1 Compare → Task 6. §2 grid → Tasks 2, 4, 5 (**§2's filmstrip half is refuted;
Task 8 records the correction**). §3 `advances()` → Task 1. §4 the setting → Tasks 3, 7. §5 Survey
untouched → no task, verified by `survey.rs` being absent from every file list. Test table: all
eight rows are covered, with `the_grid_advances_past_the_whole_selection` split into a pure
`grid_nav` test (Task 2) and a headless carrier (Task 5) — the pure one proves the arithmetic and
nothing else, and this project's record says the carrier is the one that matters.

**Placeholders.** None: every step carries the literal text to insert or the exact command to run.

**Type consistency.** `advances(self) -> bool` on `MarkAction`; `advance_past(&[usize], &[usize]) ->
Option<usize>` in `grid_nav`; `selected_ids(&self) -> Vec<PhotoId>` on `Library`;
`mark: Option<(Vec<PhotoId>, MarkAction)>` in `library.rs`; `auto_advance: bool` on `Settings`.
Each is defined once, in the task named above, and used with that exact signature everywhere after.
