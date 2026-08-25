# Lens corrections (M3.2), plan 2 of 2 — the panel

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lens corrections are visible and controllable in the Develop panel, and on by preference for photos that have no sidecar yet.

**Architecture:** The resolved profile is **captured by `RenderEngine` at construction**, not passed per request — the same shape C4 used for the camera profile, and the reason is the same: the engine is rebuilt per photo, so per-photo constants belong in it. The `Corrector` is rebuilt inside each render because it depends on the *rendered* dimensions, which differ between the fit and native rungs.

**Tech Stack:** `eframe`/`egui` 0.35, the `lens-profile` crate from plan 1.

**Spec:** `docs/superpowers/specs/2026-08-25-lens-corrections-design.md`
**Predecessor:** `docs/superpowers/plans/2026-08-25-lens-corrections-headless.md` (complete, green).

**Build environment:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing rules:** no `Co-Authored-By`; never push; sabotage every new test, confirming it **compiled**; clippy clean by **exit code**, never through `grep`. ⚠ `cd` persists between shell calls — return to the repo root. ⚠ zsh does not word-split `$VAR`; use `${=VAR}`.

⚠ **Panel work is the area this project's record says is untested.** The pure decisions here — "should this photo get corrections?", "what does the panel say when nothing matched?" — are extracted into testable functions deliberately, so that the untested part is only egui glue.

---

### Task 1: `Settings.auto_lens_correction` and the apply-on-open rule

**Files:**
- Modify: `crates/app/src/settings.rs`

- [ ] **Step 1: Write the failing tests**

Add to `settings.rs`'s test module:

```rust
    #[test]
    fn a_settings_file_from_before_auto_lens_correction_still_loads() {
        // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
        // reverts *every* setting. `#[serde(default)]` is what stands in the way.
        let s: Settings = serde_json::from_str(r#"{"confirm_preset_delete":false}"#).unwrap();
        assert!(!s.confirm_preset_delete);
        assert!(
            s.auto_lens_correction,
            "the new field must default to on, not to false"
        );
    }

    #[test]
    fn corrections_are_applied_only_to_a_photo_with_no_sidecar() {
        // ⚠ The whole point of the preference living here rather than in `Edits`: a photo you
        // have already edited keeps exactly what it had, so nothing in the catalog re-renders.
        let on = Settings {
            auto_lens_correction: true,
            ..Settings::default()
        };
        assert!(on.initial_lens_correction(false).is_active(), "fresh photo");
        assert!(
            !on.initial_lens_correction(true).is_active(),
            "a photo with a sidecar must be left alone"
        );
    }

    #[test]
    fn the_preference_off_leaves_even_a_fresh_photo_neutral() {
        let off = Settings {
            auto_lens_correction: false,
            ..Settings::default()
        };
        assert!(!off.initial_lens_correction(false).is_active());
    }

    #[test]
    fn the_applied_default_enables_all_three_corrections() {
        let s = Settings {
            auto_lens_correction: true,
            ..Settings::default()
        };
        let l = s.initial_lens_correction(false);
        assert!(l.distortion && l.tca && l.vignetting, "{l:?}");
        assert_eq!(l.scale, 100.0, "and autoscale by default");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib settings 2>&1 | grep -E "^error" | head -3
```

Expected: `no field auto_lens_correction`.

- [ ] **Step 3: Add the field and the rule**

In `crates/app/src/settings.rs`, add to `Settings`:

```rust
    /// Turn lens corrections on for photos that have no sidecar yet.
    ///
    /// ⚠ A **preference**, not an op-list default. `Edits::default()` stays strictly neutral so
    /// the CLI, the goldens and every existing sidecar are untouched; this only decides what a
    /// *fresh* photo starts with. Defaulting the op-list instead would dent that invariant in a
    /// way the golden harness structurally cannot see, because the goldens are synthetic buffers
    /// with no lens metadata and the stage stays inert there.
    ///
    /// ⚠ Consequence, accepted and documented rather than discovered: the GUI and the CLI then
    /// disagree on a fresh photo — the CLI renders neutral, the GUI renders corrected.
    pub auto_lens_correction: bool,
```

Add `auto_lens_correction: true` to the hand-written `Default` impl for `Settings`, and:

```rust
impl Settings {
    /// What a newly-opened photo's lens corrections should start as.
    ///
    /// Extracted as a pure function so the rule is testable without a window — the decision is
    /// the part worth testing, and the egui glue is not.
    pub fn initial_lens_correction(&self, has_sidecar: bool) -> edit_model::LensCorrection {
        if self.auto_lens_correction && !has_sidecar {
            edit_model::LensCorrection {
                distortion: true,
                tca: true,
                vignetting: true,
                ..Default::default()
            }
        } else {
            edit_model::LensCorrection::default()
        }
    }
}
```

⚠ If `Settings` already has an `impl Settings` block, add the method there rather than opening a second one.

- [ ] **Step 4: Run**

```bash
cargo test -p app --lib settings 2>&1 | grep -E "test result:|FAILED"
```

Expected: all PASS.

- [ ] **Step 5: Sabotage — ignore `has_sidecar`**

Change the condition to `if self.auto_lens_correction {`. Confirm it compiles, then run.

Expected: `corrections_are_applied_only_to_a_photo_with_no_sidecar` FAILS. Restore.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(app): a preference for lens corrections on new photos"
```

---

### Task 2: Carry the profile on the session and into the render engine

**Files:**
- Modify: `crates/app/src/render_engine.rs`
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Give the engine a profile**

In `crates/app/src/render_engine.rs`, add above `RenderEngine`:

```rust
/// A resolved lens profile plus the shot it was resolved for.
///
/// ⚠ Captured by the engine at construction rather than passed per request — the engine is
/// rebuilt per photo, so this cannot vary within one engine's life. The same shape `DecodeWorker`
/// uses for the camera profile, and for the same reason: a per-request parameter would impose a
/// choice on callers that have no business making it.
#[derive(Clone)]
pub struct LensShot {
    pub profile: lens_profile::LensProfile,
    pub shot: lens_profile::Shot,
}
```

Change `RenderEngine::new` to take `lens: Option<LensShot>` after `source_scale`, store it in the worker closure, and pass it down to `render_cached`.

In `render_cached`, build the corrector from the **preview's** dimensions:

```rust
    // ⚠ Rebuilt per render, not cached: it depends on the *rendered* size, which differs between
    // the fit rung and the native one. Building it is cheap — the database is already parsed, and
    // this is a `Modifier::new` plus three `enable_*` calls.
    let corrector = lens.as_ref().filter(|_| edits.lens_correction.is_active()).and_then(|l| {
        lens_profile::Corrector::new(
            &l.profile,
            &l.shot,
            preview.width(),
            preview.height(),
            lens_profile::Corrections {
                distortion: edits.lens_correction.distortion,
                tca: edits.lens_correction.tca,
                vignetting: edits.lens_correction.vignetting,
                scale: edits.lens_correction.scale,
            },
        )
    });
```

and pass it as the third argument to `build_prefix_steps`, replacing the `None` plan 1 left with its "until the GUI resolves a lens profile — plan 2 of M3.2" comment. **Delete that comment**; it is now stale.

Add `lens-profile = { path = "../lens-profile" }` to `crates/app/Cargo.toml`.

- [ ] **Step 2: Resolve the profile when a photo opens**

In `crates/app/src/main.rs`, where the `Session` is built (`finalize_session`), resolve the profile once from the decoded metadata and hand it to `RenderEngine::new`. Store on `Session`:

```rust
    /// The lens the shot was taken with, if the file recorded one.
    pub lens: Option<raw_decode::LensMetadata>,
    /// Whether a profile was found for it — drives what the panel says.
    pub lens_profile_found: bool,
```

⚠ `Catalogue::bundled()` decompresses 56 XML files. Build it **once per photo open**, never in the panel body — that body runs sixty times a second, and a database load there is the `apply_view_demosaic` mistake in a different costume.

- [ ] **Step 3: Apply the preference on open**

Where a fresh photo's `Edits` are established (no sidecar found), set:

```rust
        edits.lens_correction = self.settings.initial_lens_correction(had_sidecar);
```

⚠ Only when there is no sidecar — `initial_lens_correction` already encodes that, so pass the flag rather than branching at the call site.

- [ ] **Step 4: Build and check**

```bash
cargo build --all 2>&1 | grep -E "^error" -A 5 | head -20
cargo test --all 2>&1 | grep -cE "test result: FAILED"
```

Expected: builds, `0` failures, 47 goldens unchanged.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(app): resolve a lens profile per photo and render through it"
```

---

### Task 3: The panel block

**Files:**
- Modify: `crates/app/src/develop/panel/adjustments.rs`
- Modify: `crates/app/src/develop/panel/mod.rs`

- [ ] **Step 1: Write the failing test**

The decision worth testing is what the panel *says*, not how it draws. Add to `adjustments.rs`'s test module:

```rust
    #[test]
    fn the_lens_line_names_what_it_could_not_find() {
        // ⚠ A miss must be diagnosable rather than mysteriously inert: the commonest reason the
        // controls do nothing is an unlisted lens, and the only way to act on that is to know
        // which name failed to match.
        assert_eq!(lens_status(None, false), "No lens recorded in this file");
        assert_eq!(
            lens_status(Some("XF8mmF3.5 R WR"), false),
            "No profile for XF8mmF3.5 R WR"
        );
        assert_eq!(
            lens_status(Some("XF23mmF2 R WR"), true),
            "XF23mmF2 R WR"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --bin Rustroom lens_line 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find function lens_status`.

- [ ] **Step 3: Implement**

In `crates/app/src/develop/panel/adjustments.rs`:

```rust
/// What the Detail section's lens line reads.
///
/// A free function rather than inline formatting so the wording is pinned by a test — the
/// no-match case is the one a user will actually hit and the one that must not be silent.
pub fn lens_status(lens: Option<&str>, profile_found: bool) -> String {
    match (lens, profile_found) {
        (None, _) => "No lens recorded in this file".to_string(),
        (Some(name), false) => format!("No profile for {name}"),
        (Some(name), true) => name.to_string(),
    }
}
```

- [ ] **Step 4: Draw the block**

`adjustments::show` needs the lens facts. Add two parameters — `lens: Option<&str>` and `lens_profile_found: bool` — and pass them from `panel/mod.rs` out of the session (disjoint field borrows, the pattern `hsl_channel` and `grade_range` already use).

At the **top** of the Detail section body, before Noise Reduction:

```rust
        // ⚠ First in Detail because it is first in the pipeline: a capture correction to get out
        // of the way before judging noise or sharpening. Detail is this codebase's
        // capture-specific group — "noise, lens, output size".
        ui.label("Lens Corrections");
        ui.label(egui::RichText::new(lens_status(lens, lens_profile_found)).small().weak());
        let l = &mut edits.lens_correction;
        ui.add_enabled_ui(lens_profile_found, |ui| {
            if ui.checkbox(&mut l.distortion, "Distortion").changed() {
                ed.discrete();
            }
            if ui.checkbox(&mut l.tca, "Chromatic aberration").changed() {
                ed.discrete();
            }
            if ui.checkbox(&mut l.vignetting, "Vignetting").changed() {
                ed.discrete();
            }
            // ⚠ Named "Scale", not "Crop": only *pincushion* correction empties the frame, and
            // barrel correction's autoscale is exactly 1, so on many photos this slider does
            // nothing at all — which is correct, and would read as a broken crop control.
            ed.slider(ui, &mut l.scale, 0.0..=100.0, "Scale");
        });
        ui.separator();
```

⚠ `ui.add_enabled_ui` rather than hiding the controls: a disabled row that names the unmatched lens explains itself; an absent row does not.

⚠ **Lens vignetting correction is not the creative Vignette**, which stays in Effects. The label here says "Vignetting" inside a "Lens Corrections" block, which is what keeps them apart.

- [ ] **Step 5: Build, lint, run**

```bash
cargo build -p app 2>&1 | grep -E "^error" -A 5 | head -20
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy exit: $?"
cargo test -p app --bin Rustroom lens_line 2>&1 | grep -E "test result:"
```

Expected: builds, `clippy exit: 0`, test PASS.

- [ ] **Step 6: Sabotage — collapse the no-match case**

Change the `(Some(name), false)` arm to return `name.to_string()`. Confirm it compiles, then run.

Expected: `the_lens_line_names_what_it_could_not_find` FAILS. Restore.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(app): lens corrections in the Detail panel"
```

---

### Task 4: Run it, and record

- [ ] **Step 1: Full green check**

```bash
cargo fmt --all --check; echo "fmt exit: $?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep -cE "test result: FAILED"
ls crates/pipeline/tests/golden/ | wc -l
```

Expected: `0`, `0`, `0`, `47`. ⚠ This plan touches no pipeline code, so **any golden movement is a bug**.

- [ ] **Step 2: Launch the GUI in the background**

```bash
cargo run --release -p app
```

Tell the user what to look at: open one of the Fuji RAFs, go to **Detail**, and confirm (a) the lens name shows, (b) the three boxes are ticked on a photo with no sidecar and the image visibly changes when they are unticked, (c) the Scale slider does something on a long-end zoom frame and nothing on a wide-end one — **which is correct**, and (d) a JPEG shows "No lens recorded in this file" with the controls greyed.

- [ ] **Step 3: Record in CLAUDE.md**

Add a section in the established style, and update the RESUME block:

- **▶ NEXT ACTION** becomes **M3.4 virtual copies**, with perspective correction and the creative-LUT half both parked in `docs/BACKLOG.md`.
- Test and golden counts from the real run.
- The findings worth carrying: the pure-Rust port removing the C-dependency risk entirely; ⚠ **the barrel/pincushion direction, which an earlier draft had backwards** — pincushion is what empties the frame, barrel's autoscale is exactly 1; the port having **no autoscale API**, so ours follows `GetAutoScale` but is not validated against it; **no combined subpixel+geometry call**, so displacements are summed with a ~0.08 px bound; the synthetic-XML seam that makes lens goldens possible in CI at all; the measured 4.79× corner lift being **real** (61% falloff flattened to within 3%); and the measured per-pixel API cost (+1.56 s on 26 Mpx, 7.7× `GeometryStage`).
- Add both new doc paths to the list at the bottom.
- ⚠ Note that **`brew install lensfun` is not a dependency** — the crate is pure Rust — so nobody adds it to the setup line.

- [ ] **Step 4: Mark M3.2 in the roadmap**

In `~/Documents/obsidian/private/opensource_lightroom/03-plan-of-action.md`, mark M3.2 🟡: distortion, TCA and vignetting done; **perspective transform** outstanding and backlogged.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: record M3.2 lens corrections"
```

---

## Definition of done

- Lens corrections are controllable from the Detail section, with the matched lens named and a miss diagnosable.
- On by preference for photos with no sidecar; already-edited photos untouched.
- 47 goldens unchanged; `fmt`, `clippy -D warnings`, `test --all` green.
- The GUI launched and looked at, and whatever that surfaced recorded.
- CLAUDE.md and the roadmap updated; the branch left unpushed.
