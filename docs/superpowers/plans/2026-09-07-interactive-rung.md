# Interactive Rung Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** While a gesture is live, render one or two ladder rungs lower and restore full quality on release — turning a 137.9 ms drag frame into ~35 ms — with the amount exposed as a setting whose `Full` value disables the feature outright.

**Architecture:** Three pieces, two of them pure. `DragQuality` is a serde enum on `Settings` mapping to a rung count. `view_region::rung_below` walks the ladder `tier_max_dim` builds and steps back along it — never halving, because the top rung is uneven (6246, not 6400). The canvas reads egui's pointer state, hoists the drop count before it borrows the session, and passes it to `rung_below` at the one line that currently reads `let max_dim = rung;`. No settle timer: `canvas/mod.rs` already re-requests whenever `max_dim != s.render_rung`.

**Tech Stack:** Rust, egui 0.35, serde. Spec: `docs/superpowers/specs/2026-09-07-interactive-rung-design.md`.

**⚠ Every build and test command in this plan needs this first:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**⚠ Project conventions that are not negotiable:** commit messages carry **no `Co-Authored-By` trailer**; **never push** — the user pushes to `github.com/wladiKrin/rustroom` himself. Work continues on the existing branch `masked-exposure-stutter`.

**⚠ The GUI binary is `Rustroom`**, so a bin-filtered test run is `cargo test -p app --bin Rustroom`. `settings.rs` and `view_region.rs` are **lib** modules, so they are `cargo test -p app --lib`. Getting this wrong prints `0 passed; N filtered out` and **exits 0**, which reads as a pass — this project records that trap three times. **Read the passed-count on every filtered run.**

---

### Task 1: `DragQuality` on `Settings`

**Files:**
- Modify: `crates/app/src/settings.rs` (add the enum after `PanDrag`'s impl, ~line 118; add the field to `Settings`; add it to `impl Default for Settings`; add tests)

⚠ `settings.rs` is in the **lib**, so `pub` items are public API and `dead_code` does not fire. This task therefore lands on its own even though `ALL` and `label()` have no caller until Task 4 — the opposite of the bin-crate trap this project records five times.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block at the bottom of `crates/app/src/settings.rs`:

```rust
    #[test]
    fn drag_quality_maps_to_rungs() {
        // ⚠⚠ `Full` must be exactly zero. It is the off switch: when the GPU port lands, the
        // feature is disabled from Settings rather than removed from the code, and
        // `rung_below` returns its input unchanged at n == 0.
        assert_eq!(DragQuality::Full.rungs_dropped(), 0);
        assert_eq!(DragQuality::Faster.rungs_dropped(), 1);
        assert_eq!(DragQuality::Fastest.rungs_dropped(), 2);
        // Every variant is in ALL, so the Settings combo cannot silently omit one.
        assert_eq!(DragQuality::ALL.len(), 3);
        for q in DragQuality::ALL {
            assert!(!q.label().is_empty(), "{q:?} has no label");
        }
    }

    #[test]
    fn a_settings_file_from_before_drag_quality_still_loads() {
        // ⚠ `SettingsStore::load` is infallible, so a field that fails to parse silently reverts
        // *every* setting — which is why each new field needs this.
        let f = TempFile::new("drag_quality_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete":false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert_eq!(
            loaded.drag_quality,
            DragQuality::Faster,
            "an absent drag_quality must default to Faster"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --lib drag_quality 2>&1 | tail -20
```

Expected: FAIL to compile — `cannot find type DragQuality in this scope`.

- [ ] **Step 3: Add the enum**

In `crates/app/src/settings.rs`, immediately after the closing `}` of `impl PanDrag` (just before the `/// User preferences that survive a restart.` doc comment on `Settings`):

```rust
/// How much resolution a live gesture may trade for responsiveness.
///
/// ⚠⚠ **`Full` is zero rungs dropped, and that is the whole reason this is a setting rather than
/// a constant.** When the GPU port lands and the tail runs 5–15× faster, the feature is switched
/// off from Settings rather than removed from the code — **the setting is the deletion
/// mechanism.** It also lets a slower or faster machine pick its own point on the trade, which a
/// hardcoded rung cannot.
///
/// ⚠ `ALL`, `label` and `rungs_dropped` live **beside the type**, the rule `EDIT_GROUP_NAMES`,
/// `Tool::ALL` and `FlagSet::ALL` already follow, so the Settings UI cannot invent a second
/// ordering or a second mapping.
///
/// ⚠ `Fastest` differs from `Faster` **only when zoomed**: fitted, a two-rung drop from 3200
/// clamps at `fit_cap` and lands where one rung already did. Recorded so it is not later filed
/// as a bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DragQuality {
    /// Render a drag at the same rung as an idle view — the behaviour before this existed.
    Full,
    #[default]
    Faster,
    Fastest,
}

impl DragQuality {
    pub const ALL: [DragQuality; 3] = [
        DragQuality::Full,
        DragQuality::Faster,
        DragQuality::Fastest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DragQuality::Full => "Full quality",
            DragQuality::Faster => "Faster",
            DragQuality::Fastest => "Fastest",
        }
    }

    /// How many rungs of the render ladder a live gesture drops.
    pub fn rungs_dropped(self) -> u32 {
        match self {
            DragQuality::Full => 0,
            DragQuality::Faster => 1,
            DragQuality::Fastest => 2,
        }
    }
}
```

- [ ] **Step 4: Add the field to `Settings`**

In `crates/app/src/settings.rs`, inside `pub struct Settings`, after the `pub composition_guide: ...` field (the last field before `write_xmp`; add it at the end of the struct body if the order differs):

```rust
    /// How much resolution a live gesture may trade for responsiveness.
    ///
    /// ⚠⚠ **Measured, not assumed:** on a large window a fitted photo renders at rung 3200
    /// (6.8 Mpx) at **137.9 ms mean over 131 instrumented drag frames — 7.2 fps** — where the fit
    /// rung renders the same photo in 35.2 ms. Dropping one rung is 4× fewer pixels.
    ///
    /// ⚠ Not an `Edits` field: it is a rendering-quality preference rather than part of the
    /// photograph's description, so it must not travel with a paste, a preset, a sync or a
    /// sidecar — `resampler`'s rule, for `resampler`'s reason.
    pub drag_quality: DragQuality,
```

And in `impl Default for Settings`, alongside its neighbours:

```rust
            drag_quality: DragQuality::default(),
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app --lib drag_quality 2>&1 | tail -20
```

Expected: `test result: ok. 2 passed`. **Read the passed-count — 2, not 0.**

- [ ] **Step 6: Sabotage-verify the serde guard**

Temporarily change `#[serde(default)]` above `pub struct Settings` to `#[serde(deny_unknown_fields)]`, run the test, confirm it fails, then restore it.

```bash
cargo test -p app --lib a_settings_file_from_before_drag_quality 2>&1 | tail -12
```

Expected while sabotaged: FAIL. **Confirm a `test result:` line appears** — a sabotage that fails to compile looks exactly like one that passes.

- [ ] **Step 7: Commit**

```bash
cargo fmt && cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E "^(error|warning)" | head
git add crates/app/src/settings.rs
git commit -m "Settings: DragQuality, with Full as the off switch

How many render-ladder rungs a live gesture may drop. Full is zero, so the
feature is disabled from Settings rather than removed from the code once the
GPU port makes it unnecessary."
```

---

### Task 2: `view_region::rung_below`

**Files:**
- Modify: `crates/app/src/view_region.rs` (add the function after `is_native`, ~line 83; add tests to its `mod tests`)

⚠ Also a **lib** module, so this too lands on its own.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/app/src/view_region.rs`:

```rust
    #[test]
    fn rung_below_is_the_identity_at_zero() {
        // ⚠ The disabled path is the SAME CODE, not a branch around it — there is no second
        // route through the renderer to keep in step.
        for rung in [1600, 3200, 6246] {
            assert_eq!(rung_below(rung, 1600, 6246, 0), rung);
        }
    }

    #[test]
    fn rung_below_steps_down_the_uneven_top_rung() {
        // ⚠⚠ **The halving trap.** The ladder doubles from fit_cap and *clamps* at buffer_long,
        // so its top rung is 6246, not 6400. Halving gives 3123 — a value `tier_max_dim` can
        // never produce, which would mint a new max_dim, a new CacheKey, and grow the prefix
        // cache's key set without bound while looking entirely correct on screen.
        assert_eq!(rung_below(6246, 1600, 6246, 1), 3200);
        assert_ne!(rung_below(6246, 1600, 6246, 1), 3123);
        assert_eq!(rung_below(3200, 1600, 6246, 1), 1600);
    }

    #[test]
    fn rung_below_floors_at_the_fit_cap() {
        assert_eq!(rung_below(6246, 1600, 6246, 2), 1600);
        // Deeper than the ladder is deep still lands on the fit rung rather than below it.
        assert_eq!(rung_below(6246, 1600, 6246, 9), 1600);
        assert_eq!(rung_below(1600, 1600, 6246, 1), 1600);
        // A buffer no larger than the fit cap has a one-rung ladder and nowhere to go.
        assert_eq!(rung_below(1200, 1200, 1200, 2), 1200);
    }

    #[test]
    fn rung_below_returns_a_real_ladder_rung() {
        // The property the halving trap violates, asserted over the whole ladder: every result
        // must be a value `tier_max_dim` itself can produce.
        for &buffer in &[3123usize, 6246, 7728] {
            let reachable: Vec<usize> = (0..=48)
                .map(|i| tier_max_dim(1600, buffer, 1.0, 1.0 + i as f32 * 0.25))
                .collect();
            for &rung in &reachable {
                for n in 0..=4 {
                    let got = rung_below(rung, 1600, buffer, n);
                    assert!(
                        reachable.contains(&got),
                        "rung_below({rung}, 1600, {buffer}, {n}) = {got}, \
                         which tier_max_dim cannot produce"
                    );
                }
            }
        }
    }

    #[test]
    fn rung_below_leaves_a_rung_it_does_not_recognise_alone() {
        // A rung off the ladder means a bug elsewhere. Returning it unchanged is the answer that
        // invents nothing; stepping back from a guessed position would fabricate a max_dim.
        assert_eq!(rung_below(2500, 1600, 6246, 1), 2500);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --lib rung_below 2>&1 | tail -20
```

Expected: FAIL to compile — `cannot find function rung_below in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/app/src/view_region.rs`, immediately after the closing `}` of `is_native`:

```rust
/// The rung `n` steps below `rung` on the ladder [`tier_max_dim`] walks, floored at `fit_long`.
///
/// ⚠⚠ **It rebuilds the ladder and steps back along it; it must never halve.** The ladder doubles
/// from `fit_long` and **clamps** at `buffer_long`, so its top rung is uneven — 6246, not 6400 —
/// and halving that gives 3123, a value [`tier_max_dim`] can never produce. Feeding one to the
/// renderer mints a new `max_dim`, hence a new `CacheKey`, and grows the prefix cache's key set
/// without bound while looking entirely correct on screen.
///
/// ⚠ `n == 0` returns `rung` unchanged, so the disabled path is the **same code** rather than a
/// branch around it — there is no second route through the renderer to keep in step.
///
/// ⚠ A `rung` that is not on the ladder is returned unchanged rather than snapped to one. That
/// means a bug upstream, and inventing a nearby rung would hide it behind a fabricated `max_dim`.
pub fn rung_below(rung: usize, fit_long: usize, buffer_long: usize, n: u32) -> usize {
    if n == 0 {
        return rung;
    }
    let fit_long = fit_long.max(1);
    let mut ladder = vec![fit_long];
    let mut r = fit_long;
    while r < buffer_long {
        r = (r * 2).min(buffer_long);
        ladder.push(r);
    }
    let Some(i) = ladder.iter().position(|&v| v == rung) else {
        return rung;
    };
    ladder[i.saturating_sub(n as usize)]
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app --lib rung_below 2>&1 | tail -20
```

Expected: `test result: ok. 5 passed`. **Read the passed-count — 5, not 0.**

- [ ] **Step 5: Sabotage-verify the halving trap**

Temporarily replace the body's ladder walk with a naive halving:

```rust
    let mut out = rung;
    for _ in 0..n {
        out = (out / 2).max(fit_long);
    }
    out
```

Run the tests, confirm **both** `rung_below_steps_down_the_uneven_top_rung` and
`rung_below_returns_a_real_ladder_rung` fail (the first with `3123`), then restore.

```bash
cargo test -p app --lib rung_below 2>&1 | tail -25
```

Expected while sabotaged: 2 failures, and a `test result:` line present.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E "^(error|warning)" | head
git add crates/app/src/view_region.rs
git commit -m "view_region: rung_below, ladder-aware and never a halving

The ladder clamps at buffer_long, so its top rung is 6246 rather than 6400 and
halving it gives 3123 — a max_dim tier_max_dim cannot produce, which would grow
the prefix cache's key set without bound while looking correct on screen."
```

---

### Task 3: Wire the canvas

**Files:**
- Modify: `crates/app/src/develop/canvas/mod.rs` (add the hoist near line 58; replace `let max_dim = rung;` at line 120)

- [ ] **Step 1: Add the gate hoist**

In `crates/app/src/develop/canvas/mod.rs`, immediately after the line `let app_guide = app.settings.composition_guide;` (~line 58):

```rust
    // ⚠⚠ **The interactive rung's gate.** Hoisted before `app.session` is borrowed: `app.settings`
    // is a disjoint field, but reading it *through* `app` after the borrow is E0502 — the rule
    // `tool` and `app_guide` already follow. `DragQuality` is `Copy`, so this is free.
    //
    // ⚠ `is_decidedly_dragging` rather than `any_down`: its condition includes
    // `!could_any_button_be_click() && !any_click()`, which is what stops a plain click costing
    // two renders — one down to the low rung and one back up. Verified in egui 0.35's source,
    // not assumed.
    //
    // ⚠⚠ **A gate on the POINTER, never on the render — and that is what makes it structurally
    // unable to oscillate.** The 2026-08-13 progressive-fit feature asked "is the showing
    // resolution not yet sharp?", a question the render itself answers; so the render answered
    // it, the gate flipped, and it counted that as interaction and reset its own timer. Nothing
    // a render does can change this one.
    //
    // ⚠ Its two documented edges are both benign: it is false on the *first* frame of a drag and
    // can be true on the *release* frame. The render worker drains its queue to the newest
    // request, so the first frame's full-rung request is discarded before it renders; the release
    // is restored one frame later.
    let app_drag_drop = if ui.ctx().input(|i| i.pointer.is_decidedly_dragging()) {
        app.settings.drag_quality.rungs_dropped()
    } else {
        0
    };
```

- [ ] **Step 2: Replace the vestigial binding**

At `crates/app/src/develop/canvas/mod.rs:120`, replace this line:

```rust
                    let max_dim = rung;
```

with:

```rust
                    // ⚠⚠ **The interactive rung.** A live gesture renders `app_drag_drop` rungs
                    // lower and the release restores full quality — measured at 137.9 ms against
                    // 35.2 ms on the reporter's window. There is **no settle timer**: the
                    // `max_dim != s.render_rung` disjunction below already re-requests on any
                    // rung change, so the release edge triggers the full-quality render by
                    // itself, one frame later rather than 200 ms.
                    //
                    // ⚠⚠ `native` is computed from the DROPPED `max_dim`, which is the *opposite*
                    // of the 2026-08-13 spec's warning — and correct, because this override goes
                    // DOWN where that one went up. Theirs could report native at fit zoom and make
                    // the whole image vanish; this one cannot. At rung 1600 whole-image is right,
                    // and a fitted view at a high rung is caught by `covers_whole_image` below.
                    let max_dim =
                        view_region::rung_below(rung, fit_cap, s.buffer_long, app_drag_drop);
```

- [ ] **Step 3: Build and check the borrow compiles**

```bash
cargo build --release -p app 2>&1 | grep -E "^(error|warning)" -A 6 | head -30; echo BUILD_DONE
```

Expected: no errors. If `E0502` appears, the hoist is in the wrong place — it must be **above** `if let Some(s) = app.session.as_mut()`.

- [ ] **Step 4: Run the whole suite**

```bash
cargo test --all 2>&1 | grep -E "FAILED|^error" | head
cargo test --all 2>&1 | grep -E "^test result: ok" | awk -F'[ ;]' '{s+=$4} END {print "TOTAL PASSED:", s}'
```

Expected: no failures, and **TOTAL PASSED: 2024** (2017 before this work + 2 from Task 1 + 5 from Task 2).

- [ ] **Step 5: Verify the goldens are byte-identical**

```bash
cargo test -p pipeline --test golden 2>&1 | tail -5
```

Expected: `test result: ok. 59 passed`. Nothing in this change touches a pixel path, so a moved golden means something is wrong.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E "^(error|warning)" | head
git add crates/app/src/develop/canvas/mod.rs
git commit -m "Drop a render rung while a gesture is live

Fills the seam the 2026-08-13 progressive-fit feature left behind — canvas
line 120 has read 'let max_dim = rung;' since it was removed.

The gate is egui's is_decidedly_dragging, a question about the pointer rather
than about the render, so unlike the gate that killed that feature it cannot
oscillate. No settle timer: the max_dim != render_rung disjunction already
re-requests on the release edge."
```

---

### Task 4: The Settings control

**Files:**
- Modify: `crates/app/src/modals/settings.rs` (import; local at ~line 17; the control after the `Pan with` block ~line 180; the write-back at ~line 228)

- [ ] **Step 1: Import and read the local**

In `crates/app/src/modals/settings.rs`, change the import on line 7:

```rust
use rustroom_app::settings::{Accent, DragQuality, PanDrag, ThemeChoice};
```

and add after `let mut pan_drag = app.settings.pan_drag;` (~line 17):

```rust
    let mut drag_quality = app.settings.drag_quality;
```

- [ ] **Step 2: Add the control**

In the same file, immediately after the `theme::hint(..)` call that follows the `Pan with` combo (the one beginning `"Scrolling and two-finger swipe always pan"`), and **before** the `ui.separator();` that precedes the Close button:

```rust
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Preview while dragging");
                egui::ComboBox::from_id_salt("drag_quality_pick")
                    .selected_text(drag_quality.label())
                    .show_ui(ui, |ui| {
                        for q in DragQuality::ALL {
                            ui.selectable_value(&mut drag_quality, q, q.label());
                        }
                    });
            });
            theme::hint(
                ui,
                "Renders at a lower resolution while a slider or the canvas is being dragged, \
                 then at full quality the moment you let go. Each step is about a quarter as \
                 many pixels. Sharpening is the one adjustment that reads weakly while dragging \
                 — judge it at 1:1, or choose Full quality.",
            );
```

- [ ] **Step 3: Add the write-back**

At the bottom of `show`, after the `pan_drag` write-back block and before `if close {`:

```rust
    // ⚠ A change guard like its neighbours, and for `pan_drag`'s weaker reason: this has no side
    // effect beyond the write, because the canvas reads `settings.drag_quality` afresh every
    // frame — no worker rebuild, no re-decode. The guard is here so the settings file is not
    // rewritten on every frame the dialog is open.
    if drag_quality != app.settings.drag_quality {
        app.settings.drag_quality = drag_quality;
        app.save_settings();
    }
```

- [ ] **Step 4: Build and run the suite**

```bash
cargo build --release -p app 2>&1 | grep -E "^error" -A 6 | head -20; echo BUILD_DONE
cargo test --all 2>&1 | grep -E "FAILED|^error" | head
cargo test --all 2>&1 | grep -E "^test result: ok" | awk -F'[ ;]' '{s+=$4} END {print "TOTAL PASSED:", s}'
```

Expected: no errors, **TOTAL PASSED: 2024**.

⚠ If the dialog-class completeness test fails, it is asserting over a fixed-size array of dialogs and this task adds no dialog — investigate rather than editing the array.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E "^(error|warning)" | head
git add crates/app/src/modals/settings.rs
git commit -m "Settings: expose the drag quality trade

Labels rather than a rung count — 'rung' is internal vocabulary. The hint names
sharpening explicitly, since it is the one adjustment the measurement showed
vanishing at the fit rung."
```

---

### Task 5: Verify in the running app

**Files:** none modified.

- [ ] **Step 1: Launch with the instrument on**

```bash
RUSTROOM_DEBUG_RENDER=1 cargo run --release -p app > /tmp/rustroom-rung.log 2>&1 &
```

- [ ] **Step 2: Ask the user to exercise it**

Ask them to: open a photo; drag the global exposure slider at the fitted view; release; then zoom to 1:1 and drag it again. Then report back.

- [ ] **Step 3: Read the log**

```bash
grep "^render:" /tmp/rustroom-rung.log | awk '{ms=$2; rung=$5; mpx=$8; gsub(/[()]/,"",mpx); key=rung"|"mpx; n[key]++; s[key]+=ms} END {for(k in n){split(k,a,"|"); printf "rung %-6s %6s Mpx  n=%-4d  mean %6.1f ms -> %4.1f fps\n", a[1],a[2],n[k],s[k]/n[k],1000/(s[k]/n[k])}}' | sort -n
```

Expected: **two rungs per view now**, where the previous log had one. Fitted should show a cluster at rung 1600 (~35 ms, ~28 fps) during the drag and a few frames at 3200 (~138 ms) on release. `prefix_ran` should stay 0 — if it starts reading non-zero, `FINALS_CAP` needs raising from 2 to 3, which the spec anticipates.

- [ ] **Step 4: Update `CLAUDE.md` and `docs/BACKLOG.md`**

Add a section to `CLAUDE.md` recording: the measured before/after, that the mask was never involved, that the high rung is honest resolution rather than waste, the `rung_visibility` finding about sharpening, and that `Full` is the GPU off-switch. Mark the backlog's masked-exposure entry **resolved**, and move the remaining open item — the 94.9–242.3 ms frame-time *variance* at one rung, which is undiagnosed and distinct from the mean — into its own entry.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md docs/BACKLOG.md
git commit -m "CLAUDE.md: the interactive rung, and what the measurement overturned"
```
