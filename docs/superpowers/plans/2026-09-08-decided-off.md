# "Decided off" for the rendering and the lens corrections — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turning the camera rendering or the lens corrections off on an otherwise untouched photo
survives a photo switch.

**Architecture:** `Edits.rendering: Option<Rendering>` and `Edits.lens_correction:
Option<LensCorrection>`, `None` = never decided. Two accessors, `applied_rendering()` and
`applied_lens_correction()`, are the one read rule (undecided applies as off). Preferences are
still resolved at first open in `Settings::initial_edits`, which seeds `Some` only for a
preference that is on. `is_neutral` is untouched: a decided-off `Some` is not the default, so the
sidecar is kept. Undecided fields are absent from the sidecar (`skip_serializing_if`).

**Tech Stack:** Rust, serde, egui 0.35 headless harness.

Spec: `docs/superpowers/specs/2026-09-08-decided-off-design.md`.

⚠⚠ **Standing conventions:** commit messages carry **no `Co-Authored-By` trailer**; **never
`git push`**. Branch `rendering-option` exists and holds the spec.

⚠ **Every `cargo` command assumes:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ Two oversized files are edited and must be named in the report: `crates/edit-model/src/lib.rs`
(~3,700 lines) and `crates/app/src/settings.rs` (~1,900).

⚠⚠ **Task 1 is one commit by necessity.** Changing a field's type breaks every crate that reads
it, and every commit must build the whole workspace (`--all-targets`, examples included). The
compiler enumerates the sites; this plan lists every one it will name, with the exact edit.

---

### Task 1: The two `Option` fields, and every read and write site

**Files:**
- Modify: `crates/edit-model/src/lib.rs` — fields (~line 1526 and 1539), new `impl Edits` accessors, `all_nonneutral` (~2556, 2647), tests (~2433–2465, ~3629–3690)
- Modify: `crates/pipeline/src/lib.rs` (~216–226, ~784), `crates/pipeline/tests/golden.rs` (~1425)
- Modify: `crates/rustroom/src/main.rs` (188, 203)
- Modify: `crates/app/src/settings.rs` (`initial_edits`, `initial_lens_correction`, tests ~1520–1616)
- Modify: `crates/app/src/render_engine.rs` (~248), `thumb_loader.rs` (~103, ~320–326), `auto_tone_tool.rs` (83), `perspective_detect.rs` (53–60), `edit_clipboard.rs` (169), `looks.rs` (442, 471), `develop/canvas/spot_overlay.rs` (422–425), `develop/panel/adjustments.rs` (265–278, 440–449, tests 565–605), `develop/panel/geometry.rs` (101, 109, tests 982, 1017), `develop/panel/mod.rs` (446)
- Modify: `crates/app/examples/*.rs` — `look_probe` (4), `sat_probe`, `rendering_sheet`, `rendering_vs_contrast`, `strip_vs_canvas`, `bench_crop_rung`, `proof_hist_probe`

- [ ] **Step 1: Write the failing edit-model tests**

In `crates/edit-model/src/lib.rs`'s `mod tests`, **replace** `the_default_rendering_is_colorimetric_only`
and `a_sidecar_written_before_the_rendering_still_loads` with:

```rust
    /// ⚠ `None` is *never decided*, and it applies as `Rendering::None` — which is what keeps
    /// `Edits::default()` strictly neutral, all 59 goldens and the CLI untouched.
    #[test]
    fn the_default_rendering_is_undecided_and_applies_as_colorimetric() {
        assert_eq!(Edits::default().rendering, None);
        assert_eq!(Edits::default().applied_rendering(), Rendering::None);
        assert_eq!(Edits::default().lens_correction, None);
        assert!(!Edits::default().applied_lens_correction().is_active());
    }

    /// ⚠⚠ THE DISTINCTION THIS CHANGE EXISTS FOR: decided off is not the default. Before it, a
    /// photo whose rendering was switched off read as neutral, its sidecar was deleted, and the
    /// preference switched it back on at the next open.
    #[test]
    fn a_decided_off_is_not_the_default() {
        let off = Edits {
            rendering: Some(Rendering::None),
            ..Default::default()
        };
        assert_ne!(off, Edits::default(), "decided-off rendering read as neutral");
        assert_eq!(off.applied_rendering(), Rendering::None, "but still applies as off");
        let lens_off = Edits {
            lens_correction: Some(LensCorrection::default()),
            ..Default::default()
        };
        assert_ne!(lens_off, Edits::default(), "decided-off lens read as neutral");
        assert!(!lens_off.applied_lens_correction().is_active());
    }

    /// ⚠ Serde ignores what it does not know, so a sidecar from before either field reads as
    /// *undecided* — which applies as off, exactly what those photos were rendered with.
    #[test]
    fn a_sidecar_written_before_the_rendering_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure":0.5}"#).expect("parses");
        assert_eq!(e.rendering, None);
        assert_eq!(e.lens_correction, None);
        assert_eq!(e.exposure, 0.5);
    }

    /// ⚠ A sidecar that already says `"None"` was written by whoever switched the rendering off,
    /// so *decided off* is the reading they meant — and nothing existing changes meaning.
    #[test]
    fn an_existing_off_in_a_sidecar_reads_as_decided_off() {
        let e: Edits = serde_json::from_str(r#"{"rendering":"None"}"#).unwrap();
        assert_eq!(e.rendering, Some(Rendering::None));
        let e: Edits = serde_json::from_str(r#"{"rendering":"Camera"}"#).unwrap();
        assert_eq!(e.rendering, Some(Rendering::Camera));
        let e: Edits =
            serde_json::from_str(r#"{"lens_correction":{"distortion":false}}"#).unwrap();
        assert_eq!(e.lens_correction, Some(LensCorrection::default()));
    }

    /// ⚠ Undecided is **absent** from the file, never `null`: a photo that was never asked
    /// writes the same bytes it always did.
    #[test]
    fn an_undecided_field_is_absent_from_the_sidecar() {
        let text = serde_json::to_string(&Edits::default()).unwrap();
        assert!(!text.contains("\"rendering\""), "{text}");
        assert!(!text.contains("\"lens_correction\""), "{text}");
        let decided = Edits {
            rendering: Some(Rendering::None),
            lens_correction: Some(LensCorrection::default()),
            ..Default::default()
        };
        let text = serde_json::to_string(&decided).unwrap();
        assert!(text.contains("\"rendering\":\"None\""), "{text}");
        assert!(text.contains("\"lens_correction\""), "{text}");
        let back: Edits = serde_json::from_str(&text).unwrap();
        assert_eq!(back, decided);
    }
```

Then update the literals the compiler will flag in the remaining tests:

- `the_rendering_travels_on_its_own_group_and_by_default`: `rendering: Some(Rendering::Camera),`
  and `assert_eq!(dst.rendering, Some(Rendering::Camera), …)`.
- The lens serde test at ~3629: `assert_eq!(e.lens_correction.unwrap().scale, 100.0); assert!(e.lens_correction.unwrap().distortion);`
- `lens_corrections_travel_on_their_own_group_not_with_detail`: `assert_eq!(only_detail.lens_correction, None, …)`; the `only_lens` line is unchanged.
- `all_nonneutral()`: wrap the `lens_correction: LensCorrection { … }` literal in `Some(..)` and
  `rendering: Some(Rendering::Camera),`.

- [ ] **Step 2: The fields and the accessors**

Replace the `lens_correction` field line (~1526) with:

```rust
    /// The lens corrections. `None` = **never decided**, which applies as none;
    /// `Some(LensCorrection::default())` = **decided off**, a different fact that survives a
    /// photo switch. Read through [`Edits::applied_lens_correction`].
    ///
    /// ⚠ `skip_serializing_if`: undecided is *absent* from the sidecar, never `null`, so a photo
    /// that was never asked writes the same bytes it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_correction: Option<LensCorrection>,
```

Replace the `rendering` field and its doc (~1532–1539) with:

```rust
    /// How the scene is mapped onto a display. See [`Rendering`].
    ///
    /// ⚠ `None` = **never decided**, which applies as `Rendering::None` — what keeps
    /// `Edits::default()` strictly neutral, and therefore all 59 goldens and the CLI untouched.
    /// `Some(Rendering::None)` = **decided off**: not the default, so a sidecar holding it is
    /// kept, which is the whole fix. The GUI seeds `Some(Settings::default_rendering)` on a
    /// photo with no sidecar, the rule `initial_lens_correction` follows.
    /// Read through [`Edits::applied_rendering`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendering: Option<Rendering>,
```

Directly after the `Edits` struct (before `impl Edits` with `merge_from`, or as a new impl block):

```rust
impl Edits {
    /// What the pipeline applies: the decided rendering, else none.
    ///
    /// ⚠ **The one read rule** — every consumer goes through here, so "undecided renders as
    /// off" is written once.
    pub fn applied_rendering(&self) -> Rendering {
        self.rendering.unwrap_or_default()
    }

    /// What the pipeline applies: the decided lens corrections, else none. Same rule.
    pub fn applied_lens_correction(&self) -> LensCorrection {
        self.lens_correction.unwrap_or_default()
    }
}
```

`merge_from` needs no change: its destructure binds `lens_correction` and `rendering` by name and
copies them verbatim, which now copies the `Option` — spec §2.

- [ ] **Step 3: Run edit-model**

Run: `cargo test -p edit-model 2>&1 | grep -E "^test result|panicked|^error"`
Expected: `test result: ok.`

- [ ] **Step 4: pipeline, golden, CLI**

`crates/pipeline/src/lib.rs` ~216–226:

```rust
    let lens_correction = edits.applied_lens_correction();
    if lens_correction.is_active() {
        if let Some(c) = lens {
            steps.push(PrefixStep {
                key: PrefixStepKey::LensCorrection(lens_correction, resampler),
                stage: Box::new(LensCorrectionStage::new(
                    c,
                    lens_profile::Corrections {
                        distortion: lens_correction.distortion,
                        tca: lens_correction.tca,
                        vignetting: lens_correction.vignetting,
                        scale: lens_correction.scale,
                    },
                    resampler,
                )),
            });
```

~784: `match edits.applied_rendering() {`. The six test literals (957, 1019, 1055, 1079, 1134,
1311): `rendering: Some(edit_model::Rendering::Camera),`. `tests/golden.rs` ~1425: wrap the
`LensCorrection { … }` literal in `Some(..)`.

`crates/rustroom/src/main.rs` 188 and 203: `if edits.applied_lens_correction().is_active() {`.

Run: `cargo test -p pipeline -p rustroom 2>&1 | grep -E "^test result|panicked|^error" | head`
Expected: every `test result: ok.` and 59 goldens unchanged.

- [ ] **Step 5: The settings seam**

`Settings::initial_edits` becomes:

```rust
    pub fn initial_edits(&self, stored: Option<edit_model::Edits>) -> edit_model::Edits {
        match stored {
            Some(e) => e,
            None => edit_model::Edits {
                lens_correction: self.initial_lens_correction(),
                rendering: self.initial_rendering(),
                ..edit_model::Edits::default()
            },
        }
    }

    /// What a **fresh** photo's rendering starts as.
    ///
    /// ⚠ `None` — *undecided* — when the preference is off, **not** `Some(Rendering::None)`:
    /// decided-off is a fact worth a sidecar, and seeding it would write a file for every photo
    /// ever opened under an off preference, where today none is written.
    pub fn initial_rendering(&self) -> Option<edit_model::Rendering> {
        (self.default_rendering != edit_model::Rendering::None).then_some(self.default_rendering)
    }
```

and `initial_lens_correction`:

```rust
    /// What a **fresh** photo's lens corrections start as — `None`, undecided, when the
    /// preference is off, for `initial_rendering`'s reason.
    ///
    /// ⚠ No `has_sidecar` parameter any more: the "only when there is no sidecar" half of the
    /// rule lives in [`Self::initial_edits`], where it applies to every preference at once
    /// rather than being repeated per field and got wrong for the second one.
    pub fn initial_lens_correction(&self) -> Option<edit_model::LensCorrection> {
        self.auto_lens_correction.then(|| edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            ..Default::default()
        })
    }
```

Its tests: `a_fresh_photo_opens_rendered_and_corrected` → `assert_eq!(fresh.rendering, Some(Camera))`
and `assert!(fresh.applied_lens_correction().is_active())`. `a_photo_with_a_sidecar_keeps_exactly_what_it_stored`
→ `rendering: Some(Rendering::None)`, `lens_correction: Some(LensCorrection::default())`,
`assert_eq!(got.rendering, Some(Rendering::None), …)`, `assert!(!got.applied_lens_correction().is_active(), …)`.
`the_preferences_off_open_a_fresh_photo_neutral` → `assert_eq!(fresh.rendering, None)`,
`assert_eq!(fresh.lens_correction, None)` (the `fresh == Edits::default()` assertion is the one
that matters and stays). `corrections_are_applied_only_to_a_photo_with_no_sidecar` → both reads
through `.applied_lens_correction().is_active()`.

- [ ] **Step 6: App read sites**

- `render_engine.rs` ~248: `rendering: None,` (with the comment gaining: *undecided, not decided
  off — a stripped copy is a render input, not a choice anyone made*).
- `thumb_loader.rs` ~103: `rendering: None,`; test ~320: `rendering: Some(Camera)`, ~326:
  `assert_eq!(t.rendering, None, …)`.
- `auto_tone_tool.rs` 83: `median_target_for(edits.applied_rendering(), baseline_ev)`.
- `perspective_detect.rs` 53–60: `let lc = edits.applied_lens_correction();` above the match, then
  `match (lc.is_active(), lens)` and `lc.distortion` etc.
- `spot_overlay.rs` 422–425: `let lc = s.edits.applied_lens_correction();` above, fields from `lc`.
- `edit_clipboard.rs` 169: `rendering: Some(edit_model::Rendering::Camera),`.
- `looks.rs` 442 and 471 (tests): `Some(..)`.
- `panel/mod.rs` 446 (test): `s.edits.rendering = Some(edit_model::Rendering::Camera);`.

- [ ] **Step 7: The two panel writers**

`adjustments.rs` ~265–278:

```rust
    let mut rendered = edits.applied_rendering() == edit_model::Rendering::Camera;
    if ui
        .checkbox(&mut rendered, "Camera rendering")
        .on_hover_text(
            "Maps the scene's light onto the display the way the camera would. \
             Off is colorimetric only, which looks flat and is what the CLI renders.",
        )
        .changed()
    {
        // ⚠ Always `Some`: a click is a decision, and *decided off* is what survives a photo
        // switch — `None` would read as never-asked and the preference would switch it back on.
        edits.rendering = Some(if rendered {
            edit_model::Rendering::Camera
        } else {
            edit_model::Rendering::None
        });
```

`adjustments.rs` ~440–449, the lens boxes:

```rust
            // ⚠ Read the applied value into a local and write the whole struct back as `Some`
            // on any change: a click decides, and decided-off must not collapse back to
            // undecided. Binding the checkboxes to an `Option`'s inside would need it to be
            // `Some` first, which would turn merely *looking* at the panel into a decision.
            let mut l = edits.applied_lens_correction();
            let mut touched = false;
            touched |= ui.checkbox(&mut l.distortion, "Distortion").changed();
            touched |= ui.checkbox(&mut l.tca, "Chromatic aberration").changed();
            // ⚠ Not the creative Vignette, which stays in Effects. This one is corrective and
            // pre-crop; that one is creative and post-crop. The section heading keeps them apart.
            touched |= ui.checkbox(&mut l.vignetting, "Vignetting").changed();
            if touched {
                edits.lens_correction = Some(l);
                ed.discrete();
            }
```

⚠ Read the ~15 lines that follow (the Scale slider in this section) and route them the same way:
if it binds `&mut l.scale` through `ed.slider`, bind `&mut l.scale` on the local and set
`edits.lens_correction = Some(l)` when its response `changed()`, folding the response into `ed`
with `ed.absorb(&r)`. Whatever the slider's exact shape, the rule is: **every write to the lens
correction goes through `Some(l)`.**

Its tests (~565–605): `assert_eq!(edits.rendering, Some(edit_model::Rendering::Camera), …)` and
`Some(edit_model::Rendering::None)`; the Auto test's literal `rendering: Some(Camera)`.

`geometry.rs` ~101–112:

```rust
    if lens_profile_found && s.edits.applied_lens_correction().distortion {
        ui.label("Lens");
        // (comment unchanged)
        // ⚠ `if let Some`: drawn only while distortion is on, which means the field is
        // already decided — binding through `get_or_insert` would turn drawing into deciding.
        if let Some(l) = &mut s.edits.lens_correction {
            ed.slider(ui, &mut l.scale, 0.0..=100.0, "Lens scale");
        }
        ui.separator();
    }
```

Its tests ~982 and ~1017: `s.edits.lens_correction = Some(edit_model::LensCorrection { distortion, ..Default::default() });`
and `… { distortion: true, ..Default::default() }`.

- [ ] **Step 8: Examples**

`cargo build --all-targets -p app 2>&1 | grep -E "^error" -A 4` names each. The transformation is
one shape: wrap the literal in `Some(..)`. Known sites: `look_probe.rs` 168, 183, 231, 270;
`sat_probe.rs` 49; `rendering_sheet.rs` 34 (`rendering: Some(r)`); `rendering_vs_contrast.rs`
57; `strip_vs_canvas.rs` 87; `bench_crop_rung.rs` 85 (`lens_correction: Some(lens_on)`);
`proof_hist_probe.rs` — wherever its `rendering` parameter lands in an `Edits` literal.

- [ ] **Step 9: The whole workspace**

```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/do_clippy.txt 2>&1; echo "clippy exit $?"
```
```bash
cargo test --all > /tmp/do_tests.txt 2>&1; echo "test exit $?"; grep -E "^test result" /tmp/do_tests.txt | awk '{p+=$4; f+=$6} END {print "passed", p, "failed", f}'; echo "goldens: $(( $(grep -cE '^test .*golden.* ok$' /tmp/do_tests.txt) - 7 ))"
```
```bash
cargo fmt --all -- --check; echo "fmt exit $?"
```

Expected: all exits 0, failed 0, goldens 59. ⚠ Read each exit code; commit only on all three.

- [ ] **Step 10: Commit**

```bash
git add -A crates
git commit -m "Edits: rendering and lens_correction become Option — decided off is not the default"
```

---

### Task 2: The bug as a test, at the sidecar

**Files:**
- Modify: `crates/app/src/sidecar.rs` — tests after `a_sidecar_holding_only_a_neutral_original_is_neutral` (~line 798)

- [ ] **Step 1: Write the tests**

```rust
    /// ⚠⚠ THE M5.4b LEFTOVER, AS A TEST. Before the `Option`, an otherwise-untouched photo whose
    /// rendering was switched off read as `Edits::default()`, so `persist` deleted its sidecar
    /// and the preference switched the rendering back on at the next open. Decided off is not
    /// the default now, so the file is written and kept.
    #[test]
    fn a_decided_off_rendering_or_lens_is_worth_a_sidecar() {
        for edits in [
            Edits {
                rendering: Some(edit_model::Rendering::None),
                ..Edits::default()
            },
            Edits {
                lens_correction: Some(edit_model::LensCorrection::default()),
                ..Edits::default()
            },
        ] {
            let mut s = Sidecar::default();
            s.set_edits(0, edits.clone());
            assert!(!s.is_neutral(), "decided-off read as neutral: {edits:?}");
        }
    }

    /// And the other half: a fresh photo under off-preferences is `None`/`None`, which IS
    /// neutral, so no sidecar is written for it — no litter.
    #[test]
    fn an_undecided_photo_is_still_neutral() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        assert!(s.is_neutral());
    }

    /// End to end on disk: persist a decided-off rendering, load it back, and it is still
    /// decided off — the reported symptom was that it came back *on*.
    #[test]
    fn a_decided_off_rendering_survives_persist_and_load() {
        let dir = TempDir::new("decided_off");
        let photo = dir.0.join("a.RAF");
        let off = Edits {
            rendering: Some(edit_model::Rendering::None),
            ..Edits::default()
        };
        persist(&photo, &off).unwrap();
        assert!(path(&photo).exists(), "the sidecar was deleted as neutral");
        assert_eq!(load(&photo), Some(off));
    }
```

⚠ `Sidecar::set_edits` and `Sidecar::default()` — if the envelope's constructor is named
differently, use whatever `a_sidecar_holding_only_a_neutral_original_is_neutral` uses two lines
above; the assertion is the point. `TempDir`, `persist`, `load` and `path` are already in this
file.

- [ ] **Step 2: Run**

Run: `cargo test -p app --lib sidecar 2>&1 | grep -E "^test result|panicked"`
Expected: `test result: ok.` — these pass on first run because Task 1 already made them true, so
prove they are live with a sabotage that reintroduces the old meaning: in `is_neutral`, replace
`only.edits == Edits::default()` with a comparison of the **applied** values,

```rust
only.edits.applied_rendering() == edit_model::Rendering::None
    && !only.edits.applied_lens_correction().is_active()
    && Edits { rendering: None, lens_correction: None, ..only.edits.clone() } == Edits::default()
```

which is exactly "decided off reads as neutral". `a_decided_off_rendering_or_lens_is_worth_a_sidecar`
and `a_decided_off_rendering_survives_persist_and_load` must both fail. Revert.

- [ ] **Step 3: Commit**

```bash
git add crates/app/src/sidecar.rs
git commit -m "Sidecar: decided-off is worth a file — the M5.4b leftover as a test"
```

---

### Task 3: The panel writers' tests

**Files:**
- Modify: `crates/app/src/develop/panel/adjustments.rs` — `mod tests`

- [ ] **Step 1: Write the tests**

After `the_rendering_checkbox_switches_both_ways_and_marks_the_edit`:

```rust
    /// ⚠ The checkbox writes `Some` in both directions. `None` on untick would read as
    /// never-asked, and the preference would switch the rendering back on at the next open —
    /// the exact bug the `Option` exists to end.
    #[test]
    fn unticking_the_rendering_decides_off_rather_than_forgetting() {
        let ctx = crate::test_ui::ctx();
        let buf = tone_fixture();
        let mut edits = edit_model::Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, "Camera rendering", |ui| {
            ed = Edited::default();
            tone(ui, &mut edits, &buf, None, &Default::default(), &mut ed);
        });
        assert_eq!(edits.rendering, Some(edit_model::Rendering::None));
        assert_ne!(edits, edit_model::Edits::default(), "decided off must not be neutral");
    }

    /// Same for the lens boxes: unticking the last one leaves `Some(off)`, not `None`.
    #[test]
    fn unticking_the_last_lens_correction_decides_off() {
        let ctx = crate::test_ui::ctx();
        let mut edits = edit_model::Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, "Distortion", |ui| {
            ed = Edited::default();
            lens(ui, &mut edits, true, &mut ed);
        });
        assert_eq!(
            edits.lens_correction,
            Some(edit_model::LensCorrection::default()),
            "the last untick collapsed to undecided"
        );
        assert!(ed.settled);
    }
```

⚠ `lens(ui, edits, lens_profile_found, ed)` names the function that draws the Lens section; read
its real signature at ~line 430 and call it as it is. If the section is drawn only through the
whole `show`, drive `show` with a `test_app` session that has `lens.shot` set, the way
`the_lens_scale_appears_only_when_a_profile_matched_and_distortion_is_on` does in `geometry.rs`.

- [ ] **Step 2: Run**

Run: `cargo test -p app --bin Rustroom adjustments 2>&1 | grep -E "^test result|panicked"`
Expected: `test result: ok.` **Sabotage:** change the checkbox to write `edits.rendering = rendered.then_some(Camera)` and watch the first test fail; revert.

- [ ] **Step 3: Commit**

```bash
git add crates/app/src/develop/panel/adjustments.rs
git commit -m "Panels: a click decides, in both directions"
```

---

### Task 4: Docs and the GUI

- [ ] **Step 1: Gates** — as Task 1 Step 9, each exit code read.
- [ ] **Step 2: Docs.** Roadmap M5.4b leftover (1) marked done in `03-plan-of-action.md`'s M5.4
  entry if it lists it (grep `Option`); `CLAUDE.md`: the M5.4b section's caveat (4b) and the two
  "M5.4b leftovers" mentions in the resume block become one leftover (`MEDIAN_TARGET`), the
  handoff count, and a short section **"Decided off (2026-09-08)"** above M5.5c's carrying: the
  bug is conditional on the other preference (the note had it unconditional); undecided seeds
  `None` not `Some(off)` and why; `skip_serializing_if` and the sidecar table; `merge_from`
  verbatim and what Reset does; the rejected "compare against the seed" alternative; the two
  oversized files. Spec status → built.
- [ ] **Step 3: Launch** `cargo run --release -p app` in the background. What to look at: with
  auto lens correction **off** in Settings, open a fresh RAW, untick Camera rendering, arrow to
  the next photo and back — it stays off. Then tick it again and repeat — stays on.
- [ ] **Step 4: Commit and report** — test delta, 59 goldens, branch not merged, not pushed.
