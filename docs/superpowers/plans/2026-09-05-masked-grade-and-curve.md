# Masked colour grading and tone curve — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A mask carries a tone curve and a three-way colour grade, in addition to everything it already carries.

**Architecture:** Both stages are already tail-native, so this re-runs M7.2's playbook: two fields on `LocalAdjustments`, two insertions in the shared `adjustment_stages` builder at the positions that mirror the global chain, a neutral pass from the global caller (forced at compile time), the two `..Default::default()` fixtures updated by hand, and two collapsed panel blocks reusing the existing widgets. No change to `MaskStage`, `tail_kernel_radius`, `PrefixKey`, `EditGroups` or `merge_from`.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-05-masked-grade-and-curve-design.md`

**Build note:** every `cargo` command needs libraw and lcms2 on the pkg-config path. Export once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** (explicit user preference). **Never `git push`** — the user pushes himself. Branch is `masked-grade-and-curve`, already created.

---

## File structure

| file | change | responsibility |
|---|---|---|
| `crates/edit-model/src/lib.rs` | modify | the two new fields, `Copy` dropped, stale doc rewritten, `all_nonneutral` fixture |
| `crates/pipeline/src/lib.rs` | modify | two insertions in `adjustment_stages`, the neutral pass in `build_pipeline_split`, `all_edits` fixture, order tests |
| `crates/pipeline/tests/golden.rs` | modify | two new golden cases |
| `crates/pipeline/tests/golden/mask_curve.png` | create | reference |
| `crates/pipeline/tests/golden/mask_grade.png` | create | reference |
| `crates/app/src/main.rs` | modify | `mask_grade_range` field |
| `crates/app/src/test_app.rs` | modify | the same field in the exhaustive fixture |
| `crates/app/src/develop/panel/mod.rs` | modify | pass `mask_grade_range` into `masks::show` |
| `crates/app/src/develop/panel/masks.rs` | modify | thread the new parameter through to `mask_adjust::show` |
| `crates/app/src/develop/panel/mask_adjust.rs` | modify | **the two collapsed blocks** — all new panel code lands here |
| `crates/app/examples/bench_masked.rs` | modify | masked-curve and masked-grade rows |

⚠ **`masks.rs` is 614 lines** and is already over the ~600 marker this project asks to be reported. This plan adds **one parameter** to it and nothing else; every new control goes in `mask_adjust.rs` (60 lines).

---

## Task 1: The two fields, `Copy` dropped, and the neutral pass the compiler forces

**Files:**
- Modify: `crates/edit-model/src/lib.rs:1242-1277` (the `LocalAdjustments` doc comment, derive and fields)
- Modify: `crates/pipeline/src/lib.rs:531` (the exhaustive literal in `build_pipeline_split`)

⚠ **This is one task and one commit deliberately.** Adding the fields breaks `pipeline`'s compilation by design (`E0063`), so a commit containing only the `edit-model` half would not build. Same shape as M7.2's dead-code trap: an item lands with what the compiler forces alongside it.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/edit-model/src/lib.rs` (alongside the existing tests, above `all_nonneutral`):

```rust
/// ⚠ Both halves. A mask carrying only a curve, and a mask carrying only a grade, must each
/// read as active on their own — a test that set both could not tell a wired field from an
/// unwired one sitting next to a wired one.
#[test]
fn a_masked_curve_or_a_masked_grade_makes_the_set_active() {
    assert!(
        !LocalAdjustments::default().is_active(),
        "the default set must stay inactive, or every mask would insert stages"
    );

    let curved = LocalAdjustments {
        tone_curve: ToneCurve {
            points: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
        },
        ..Default::default()
    };
    assert!(curved.is_active(), "a masked tone curve must be active");

    let graded = LocalAdjustments {
        color_grading: ColorGrading {
            shadows: GradeWheel {
                hue: 220.0,
                saturation: 40.0,
                luminance: -10.0,
            },
            ..ColorGrading::default()
        },
        ..Default::default()
    };
    assert!(graded.is_active(), "a masked colour grade must be active");
}

/// ⚠ A sidecar written before today has neither field. It must read as neutral, and the grade
/// must come back with `blending: 50` from `ColorGrading`'s hand-written `Default` — a derived
/// one would give it 0, i.e. hard-edged ranges, silently.
#[test]
fn a_mask_written_before_these_fields_reads_them_neutral() {
    let json = r#"{"mask":{"Linear":{"from":[0.0,0.0],"to":[1.0,0.0]}},
                   "adjustments":{"exposure":0.5}}"#;
    let m: MaskedAdjustment = serde_json::from_str(json).unwrap();
    assert_eq!(m.adjustments.exposure, 0.5, "fixture check: the json was read");
    assert!(m.adjustments.tone_curve.points.is_empty());
    assert_eq!(m.adjustments.color_grading, ColorGrading::default());
    assert_eq!(
        m.adjustments.color_grading.blending, 50.0,
        "a missing grade must come back at the hand-written neutral, not f32::default()"
    );
}
```

- [ ] **Step 2: Run it and watch it fail to compile**

```bash
cargo test -p edit-model a_masked_curve_or_a_masked_grade_makes_the_set_active 2>&1 | tail -20
```

Expected: `error[E0560]: struct `LocalAdjustments` has no field named `tone_curve``.

- [ ] **Step 3: Add the fields, drop `Copy`, and rewrite the stale doc**

In `crates/edit-model/src/lib.rs`, replace the doc comment and derive above `pub struct LocalAdjustments` (currently lines 1242-1250) with:

```rust
/// What a mask can carry: **everything the tail applies per pixel or per mask**, which since
/// M7.2 is most of the tail.
///
/// ⚠ Not an arbitrary subset, and no longer the per-pixel set the earlier version of this
/// comment described. It is exactly the set that lives in the **tail**, where "run the stage on
/// a clone, then blend the result by coverage" is correct. A spatial stage in that list —
/// local contrast, sharpening — reads neighbours from *outside* the mask, which is what avoids
/// a hard edge at the mask boundary and is the behaviour wanted.
///
/// ⚠⚠ **What is NOT here is the prefix: dehaze and noise reduction.** Both run before geometry
/// while `MaskStage` runs in the tail, so a masked one would act *after* the tone curve — a
/// different operation from the global stage wearing the same name, which is the failure the
/// Whites slider already cost this project once. Dehaze additionally estimates a frame-global
/// airlight, which has no meaning confined to a region. See `docs/BACKLOG.md`, "The rest of the
/// stages inside a mask", Part B.
///
/// ⚠ **Not `Copy`** — [`ToneCurve`] carries a `Vec`. [`MaskedAdjustment`] was already not `Copy`
/// for [`Mask::Brush`]'s stroke list, so dropping it here cost **zero** call sites, measured
/// with `cargo check --workspace --all-targets` rather than assumed. Still `Clone`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LocalAdjustments {
```

Then add the two fields. Put them **in pipeline order**, so the struct reads in the order the stages run — `tone_curve` immediately after the four tone-region fields, and `color_grading` immediately after `hsl`:

After the `pub blacks: f32,` line, insert:

```rust
    /// A point tone curve confined to the mask — dodge and burn with a curve.
    ///
    /// ⚠ Runs **after** the tone regions and **before** local contrast, mirroring the global
    /// chain: a local contrast measured before the curve would then be re-mapped by it.
    ///
    /// ⚠ This field is why [`LocalAdjustments`] is not `Copy`.
    pub tone_curve: ToneCurve,
```

After the `pub hsl: Hsl,` line, insert:

```rust
    /// Three-way colour grading confined to the mask — split-tone a sky, warm a face.
    ///
    /// ⚠ Runs **after** HSL, mirroring the global chain: HSL corrects colours that are *in* the
    /// picture while a grade is laid on top, so reversed an HSL band tweak would act on the tint
    /// just added.
    ///
    /// ⚠⚠ **Its exclusion from masks was re-decided, not inherited (2026-09-05).** The stated
    /// reason was *"`LocalAdjustments` is per-pixel-only and a grade is global look"*; M7.2 made
    /// the first half false, and the second half is a judgement about taste rather than about
    /// structure.
    ///
    /// ⚠ `blending` and `balance` are carried too. They are properties of the *range
    /// decomposition* shared by all four wheels, so omitting them would make a masked grade a
    /// different control from the global one wearing the same name.
    pub color_grading: ColorGrading,
```

- [ ] **Step 4: Check the workspace and watch the forcing function fire**

```bash
cargo check --workspace --all-targets 2>&1 | tail -20
```

Expected — **this failure is the point of the exercise, not an accident**:

```
error[E0063]: missing fields `color_grading` and `tone_curve` in initializer of `LocalAdjustments`
   --> crates/pipeline/src/lib.rs:531:10
```

- [ ] **Step 5: Pass both neutral from the global caller**

In `crates/pipeline/src/lib.rs`, inside the `adjustment_stages(...)` call in `build_pipeline_split_in_region_with_brush` (the literal beginning at line 531), add `tone_curve` after `blacks` and `color_grading` after `hsl`, and extend the existing ⚠⚠ comment. The literal becomes:

```rust
        &edit_model::LocalAdjustments {
            white_balance: edits.white_balance,
            exposure: edits.exposure,
            contrast: edits.contrast,
            highlights: edits.highlights,
            shadows: edits.shadows,
            whites: edits.whites,
            blacks: edits.blacks,
            saturation: 0.0,
            vibrance: 0.0,
            // ⚠⚠ Neutral for the identical reason `saturation` above is zero: **this tail places
            // each of these itself**, further down, at a position each has earned. Passing the
            // real values would apply every one of them **twice** and move every golden.
            //
            // ⚠ Inside a mask there is no such second placement, so a mask passes its real
            // values and the ordering within the shared builder is the whole story.
            tone_curve: edit_model::ToneCurve::default(),
            local_contrast: edit_model::LocalContrast::default(),
            hsl: edit_model::Hsl::default(),
            color_grading: edit_model::ColorGrading::default(),
            sharpening: edit_model::Sharpening::default(),
        },
```

⚠ `ToneCurve::default()` is an empty `points` vec, which inserts no stage — the same "be exactly neutral by inserting nothing rather than trusting the arithmetic" rule `WhiteBalance` follows.

- [ ] **Step 6: Run the new tests and the whole suite**

```bash
cargo test -p edit-model 2>&1 | tail -5
cargo test --all 2>&1 | grep "test result" | tail -20
```

Expected: both new tests PASS, every other test still passes. **Read the passed-counts**, do not just look for the word `ok`.

- [ ] **Step 7: Confirm the goldens did not move**

```bash
cargo test -p pipeline --test golden 2>&1 | grep "test result"
```

Expected: `test result: ok. 55 passed; 0 failed`. ⚠ **55, and read the number** — nothing here changes a pixel, so a moved golden is a bug and a *skipped* golden proves nothing.

- [ ] **Step 8: Commit**

```bash
git add crates/edit-model/src/lib.rs crates/pipeline/src/lib.rs
git commit -q -F - <<'EOF'
feat(edit-model): a mask carries a tone curve and a colour grade

The two fields only. Nothing reads them yet -- the stages come next -- so
this changes no pixel and all 55 goldens stay byte-identical.

LocalAdjustments loses Copy, because ToneCurve holds a Vec. Measured
rather than assumed: cargo check --workspace --all-targets is clean with
zero call-site changes, since MaskedAdjustment has not been Copy since
the brush landed.

build_pipeline_split's exhaustive literal did what it exists to do and
refused to compile until both fields were passed neutral -- the global
tail places both stages itself, so real values there would apply each
twice and move every golden.

Also rewrites the doc comment above LocalAdjustments, which M7.2 left
stale: it claimed spatial stages were out of scope while local_contrast,
sharpening and hsl were declared directly beneath it. It now names the
prefix stages as the genuine remainder, with the reason.
EOF
```

---

## Task 2: The two exhaustive fixtures, updated by hand

**Files:**
- Modify: `crates/pipeline/src/lib.rs:1054` (`all_edits`, the mask at ~1134)
- Modify: `crates/edit-model/src/lib.rs:2317` (`all_nonneutral`, the mask at ~2402)

⚠⚠ **Neither fixture fails to compile.** Both build their `LocalAdjustments` with `..Default::default()`, so two fields whose entire purpose is to be non-neutral would land untested by the very construct that exists to force a decision about them — **silently**. Both already carry M7.2's warning comment saying exactly this. Honour it.

- [ ] **Step 1: Write the failing guard test**

Add to the `tests` module in `crates/edit-model/src/lib.rs`:

```rust
/// ⚠⚠ The fixtures do not fail to compile when `LocalAdjustments` grows, so this asserts what
/// the type system cannot: that `all_nonneutral`'s mask really carries every field non-neutral.
/// Without it a new field lands untested by the construct that exists to force a decision.
#[test]
fn the_exhaustive_fixture_carries_a_non_neutral_curve_and_grade() {
    let a = &all_nonneutral().masks[0].adjustments;
    assert!(
        !a.tone_curve.points.is_empty(),
        "all_nonneutral's mask must carry a real curve"
    );
    assert!(
        a.color_grading.is_active(),
        "all_nonneutral's mask must carry a real grade"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p edit-model the_exhaustive_fixture_carries 2>&1 | tail -20
```

Expected: FAIL with `all_nonneutral's mask must carry a real curve`.

- [ ] **Step 3: Update `all_nonneutral`'s mask**

In `crates/edit-model/src/lib.rs`, inside `all_nonneutral`'s `masks: vec![MaskedAdjustment { … adjustments: LocalAdjustments { … } }]`, add the two fields alongside the existing hand-set ones (keep the existing ⚠⚠ comment as it is — it now covers five fields rather than three):

```rust
                    tone_curve: ToneCurve {
                        points: vec![[0.0, 0.0], [0.45, 0.6], [1.0, 1.0]],
                    },
                    color_grading: ColorGrading {
                        shadows: GradeWheel {
                            hue: 210.0,
                            saturation: 30.0,
                            luminance: -8.0,
                        },
                        highlights: GradeWheel {
                            hue: 45.0,
                            saturation: 25.0,
                            luminance: 6.0,
                        },
                        blending: 60.0,
                        balance: 15.0,
                        ..ColorGrading::default()
                    },
```

- [ ] **Step 4: Update `all_edits`'s mask the same way**

In `crates/pipeline/src/lib.rs`, inside `all_edits`'s mask (the `adjustments: edit_model::LocalAdjustments { … }` at ~1134), add:

```rust
                    tone_curve: edit_model::ToneCurve {
                        points: vec![[0.0, 0.0], [0.45, 0.6], [1.0, 1.0]],
                    },
                    color_grading: edit_model::ColorGrading {
                        shadows: edit_model::GradeWheel {
                            hue: 210.0,
                            saturation: 30.0,
                            luminance: -8.0,
                        },
                        blending: 60.0,
                        ..edit_model::ColorGrading::default()
                    },
```

- [ ] **Step 5: Run the guard and the key tests**

```bash
cargo test -p edit-model the_exhaustive_fixture_carries 2>&1 | grep "test result"
cargo test -p pipeline prefix_key 2>&1 | grep "test result"
```

Expected: both PASS. ⚠ **Read the passed-count on the filtered run** — `prefix_key_ignores_every_downstream_field` is the test `all_edits` exists for, and a filter that matched nothing would print `ok` too.

- [ ] **Step 6: Commit**

```bash
git add crates/edit-model/src/lib.rs crates/pipeline/src/lib.rs
git commit -q -F - <<'EOF'
test: the exhaustive fixtures carry a masked curve and grade

Neither all_edits nor all_nonneutral fails to compile when
LocalAdjustments grows -- both fill the rest with ..Default::default() --
so both were updated by hand, which is what M7.2's warning comment on
each of them asks for.

Adds the assertion the type system cannot make: that all_nonneutral's
mask really carries a non-empty curve and an active grade. Without it the
two new fields would be untested by the very construct that exists to
force a decision about them, with nothing to say so.
EOF
```

---

## Task 3: The tone curve runs inside a mask

**Files:**
- Modify: `crates/pipeline/src/lib.rs:369-419` (`adjustment_stages`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/pipeline/src/lib.rs`, beside the other ordering tests:

```rust
/// A mask's tone curve runs after its tone regions and before its local contrast.
///
/// ⚠⚠ **Order, not merely presence.** A masked curve that ran in the wrong place still produces
/// a plausible photograph, so a test asserting only that the stage exists would pass against the
/// wrong chain. The rule it pins is the global one, mirrored: a local contrast measured before
/// the curve would then be re-mapped by it.
///
/// ⚠ Asserted on the **mask's own sub-pipeline**, which `stage_names()` cannot see through
/// `MaskStage`, so it calls the shared builder directly.
#[test]
fn a_masked_tone_curve_runs_after_the_tone_regions_and_before_local_contrast() {
    let a = edit_model::LocalAdjustments {
        shadows: 20.0,
        tone_curve: edit_model::ToneCurve {
            points: vec![[0.0, 0.0], [0.5, 0.65], [1.0, 1.0]],
        },
        local_contrast: edit_model::LocalContrast {
            texture: 0.0,
            clarity: 30.0,
        },
        ..Default::default()
    };
    let names: Vec<&'static str> =
        adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
            .iter()
            .map(|s| s.name())
            .collect();
    let at = |n: &str| names.iter().position(|x| *x == n);
    assert_eq!(
        names.iter().filter(|n| **n == "tone_curve").count(),
        1,
        "exactly one curve stage: {names:?}"
    );
    assert!(
        at("tone_regions") < at("tone_curve"),
        "the curve must follow the tone regions: {names:?}"
    );
    assert!(
        at("tone_curve") < at("local_contrast"),
        "the curve must precede local contrast: {names:?}"
    );
}

/// An empty curve inserts no stage at all.
///
/// ⚠ Exactly the global rule: empty is what keeps `ToneCurveStage` out of the pipeline, so a
/// mask with a default curve pays nothing rather than running a 1024-entry identity LUT.
#[test]
fn a_masked_curve_with_no_points_inserts_no_stage() {
    let a = edit_model::LocalAdjustments {
        exposure: 0.5,
        ..Default::default()
    };
    let names: Vec<&'static str> =
        adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
            .iter()
            .map(|s| s.name())
            .collect();
    assert!(!names.contains(&"tone_curve"), "{names:?}");
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p pipeline a_masked_tone_curve_runs_after 2>&1 | tail -20
```

Expected: FAIL — `exactly one curve stage: ["tone_regions", "local_contrast"]` (count 0, not 1).

- [ ] **Step 3: Insert the stage**

In `crates/pipeline/src/lib.rs`, in `adjustment_stages`, immediately after the `ToneRegionsStage` block and **before** the `local_contrast` block, add:

```rust
    // After the tone regions and before local contrast, mirroring the global chain: a local
    // contrast measured before the curve would then be re-mapped by it.
    //
    // ⚠ Empty points insert nothing — the global rule, and what keeps a mask with a default
    // curve from paying for a 1024-entry identity LUT on every render.
    if !a.tone_curve.points.is_empty() {
        out.push(Box::new(ToneCurveStage::new(&a.tone_curve.points)));
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pipeline a_masked_tone_curve a_masked_curve_with_no_points 2>&1 | grep "test result"
```

⚠ That runs two filters in one invocation, which cargo takes as a **substring**, not a regex — check the passed-count is `2 passed`, not merely `ok`.

- [ ] **Step 5: Confirm nothing else moved**

```bash
cargo test --all 2>&1 | grep "test result" | tail -20
cargo test -p pipeline --test golden 2>&1 | grep "test result"
```

Expected: all green; goldens `55 passed`.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -q -F - <<'EOF'
feat(pipeline): a mask carries a tone curve

Inserted in the shared adjustment_stages builder after the tone regions
and before local contrast, mirroring the global chain -- so what you read
top to bottom in the panel is what runs.

The test asserts ORDER, not presence. A masked curve running in the wrong
place still produces a plausible photograph, so a test that only checked
the stage existed would pass against the wrong chain. It calls the shared
builder directly because stage_names() cannot see through MaskStage.

Empty points insert no stage, exactly as globally, so a mask with a
default curve does not pay for a 1024-entry identity LUT per render.

All 55 goldens byte-identical.
EOF
```

---

## Task 4: The colour grade runs inside a mask

**Files:**
- Modify: `crates/pipeline/src/lib.rs:369-419` (`adjustment_stages`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/pipeline/src/lib.rs`:

```rust
/// A mask's colour grade runs after its HSL and before its sharpening.
///
/// ⚠⚠ **After HSL is the load-bearing half.** HSL corrects colours that are *in* the picture
/// while a grade is laid on top; reversed, an HSL band tweak would act on the tint just added.
/// Before sharpening for the reason sharpening is last anywhere: it must not amplify what a
/// later stage would then stretch.
#[test]
fn a_masked_colour_grade_runs_after_hsl_and_before_sharpening() {
    let mut hsl = edit_model::Hsl::default();
    hsl.bands[0].saturation = 20.0;
    let a = edit_model::LocalAdjustments {
        hsl,
        color_grading: edit_model::ColorGrading {
            shadows: edit_model::GradeWheel {
                hue: 220.0,
                saturation: 45.0,
                luminance: -12.0,
            },
            ..edit_model::ColorGrading::default()
        },
        sharpening: edit_model::Sharpening {
            amount: 50.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let names: Vec<&'static str> =
        adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
            .iter()
            .map(|s| s.name())
            .collect();
    let at = |n: &str| names.iter().position(|x| *x == n);
    assert_eq!(
        names.iter().filter(|n| **n == "color_grading").count(),
        1,
        "exactly one grade stage: {names:?}"
    );
    assert!(
        at("hsl") < at("color_grading"),
        "the grade must follow HSL: {names:?}"
    );
    assert!(
        at("color_grading") < at("sharpening"),
        "the grade must precede sharpening: {names:?}"
    );
}

/// Blending and balance alone insert no stage.
///
/// ⚠ The rule `NoiseReduction::is_active` and `Vignette::is_active` already follow, reused here
/// rather than reinvented: they shape ranges that tint nothing until a wheel is turned up, so
/// dragging one alone must not insert a stage that would cost an Oklab round trip for no change.
#[test]
fn masked_grading_modifiers_alone_insert_no_stage() {
    let a = edit_model::LocalAdjustments {
        exposure: 0.5,
        color_grading: edit_model::ColorGrading {
            blending: 90.0,
            balance: -40.0,
            ..edit_model::ColorGrading::default()
        },
        ..Default::default()
    };
    let names: Vec<&'static str> =
        adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
            .iter()
            .map(|s| s.name())
            .collect();
    assert!(!names.contains(&"color_grading"), "{names:?}");
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p pipeline a_masked_colour_grade_runs_after 2>&1 | tail -20
```

Expected: FAIL — `exactly one grade stage:` with count 0.

- [ ] **Step 3: Insert the stage**

In `crates/pipeline/src/lib.rs`, in `adjustment_stages`, immediately after the `HslStage` block and **before** the `SharpenStage` block, add:

```rust
    // After HSL, mirroring the global chain: HSL corrects colours that are *in* the picture
    // while a grade is laid on top, so reversed an HSL band tweak would act on the tint just
    // added.
    //
    // ⚠ `is_active` is wheels-only, so blending and balance alone insert nothing — they shape
    // ranges that tint nothing until a wheel is turned up.
    //
    // ⚠ A third consumer of `ColorGradingStage::new`, which builds a 256-entry table per
    // construction. Cheap, and measured rather than assumed — see `bench_masked`.
    if a.color_grading.is_active() {
        out.push(Box::new(ColorGradingStage::new(&a.color_grading)));
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pipeline a_masked_colour_grade masked_grading_modifiers 2>&1 | grep "test result"
```

Expected: `2 passed`.

- [ ] **Step 5: Update `adjustment_stages`' own doc comment**

Its ⚠⚠ paragraph currently names three neutral-passed fields. Replace that paragraph with:

```rust
/// ⚠⚠ **FIVE of these are passed NEUTRAL by the global caller** — `tone_curve`,
/// `local_contrast`, `hsl`, `color_grading` and `sharpening` — because the global chain places
/// each itself, at a position each has earned. Passing the real values would apply them twice
/// and move every golden. This is the same rule `saturation` has followed here since masking
/// landed; see the caller for all of them.
```

- [ ] **Step 6: Run the whole suite and the goldens**

```bash
cargo test --all 2>&1 | grep "test result" | tail -20
cargo test -p pipeline --test golden 2>&1 | grep "test result"
```

Expected: all green; goldens `55 passed`.

- [ ] **Step 7: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -q -F - <<'EOF'
feat(pipeline): a mask carries a three-way colour grade

Inserted after HSL and before sharpening, mirroring the global chain.
After HSL is the load-bearing half: HSL corrects colours that are in the
picture while a grade is laid on top, so reversed an HSL band tweak would
act on the tint just added.

is_active is wheels-only, so blending and balance alone insert nothing --
the rule NoiseReduction and Vignette already follow, reused rather than
reinvented.

adjustment_stages' doc comment now names five neutral-passed fields
rather than three.

All 55 goldens byte-identical.
EOF
```

---

## Task 5: Two new goldens

**Files:**
- Modify: `crates/pipeline/tests/golden.rs` (append beside the other `golden_mask_*` cases)
- Create: `crates/pipeline/tests/golden/mask_curve.png`
- Create: `crates/pipeline/tests/golden/mask_grade.png`

- [ ] **Step 1: Write the two golden tests**

Append to `crates/pipeline/tests/golden.rs`:

```rust
#[test]
fn golden_mask_curve() {
    // A lifted-midtone curve confined to the left half of the chart, so the reference carries
    // both the curve's shape and the coverage ramp across it.
    //
    // ⚠ The curve lifts rather than crushes, and stops at 0.62 rather than higher, so nothing
    // pins at 255: a clipped sample cannot tell this curve from a stronger one, which is the
    // weakness the radial exposure golden was re-authored to remove.
    check_golden(
        "mask_curve",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: true,
                }),
                adjustments: edit_model::LocalAdjustments {
                    tone_curve: edit_model::ToneCurve {
                        points: vec![[0.0, 0.0], [0.4, 0.62], [1.0, 1.0]],
                    },
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_grade() {
    // A split tone confined to a diagonal gradient: cool shadows, warm highlights, with
    // blending and balance both off their neutral so the reference covers the range
    // decomposition rather than only the wheels.
    check_golden(
        "mask_grade",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.1, 0.0],
                    to: [0.9, 1.0],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    color_grading: edit_model::ColorGrading {
                        shadows: edit_model::GradeWheel {
                            hue: 240.0,
                            saturation: 45.0,
                            luminance: -10.0,
                        },
                        highlights: edit_model::GradeWheel {
                            hue: 50.0,
                            saturation: 40.0,
                            luminance: 8.0,
                        },
                        blending: 65.0,
                        balance: 20.0,
                        ..edit_model::ColorGrading::default()
                    },
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p pipeline --test golden golden_mask_curve golden_mask_grade 2>&1 | tail -20
```

Expected: FAIL with `missing golden mask_curve; run with UPDATE_GOLDEN=1`.

- [ ] **Step 3: Generate the references**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_mask_curve golden_mask_grade 2>&1 | grep "test result"
```

- [ ] **Step 4: Measure the new references rather than eyeballing them**

⚠⚠ **A golden nobody has measured is just a hash.** This project has re-authored two references for clipping and one chart for bleeding, each found by measuring. Check both new files have no pinned samples and real discriminating power:

```bash
python3 - <<'PY'
from struct import unpack
import zlib, sys
def read(p):
    d = open(p,'rb').read(); i=8; w=h=None; idat=b''
    while i < len(d):
        ln = unpack('>I', d[i:i+4])[0]; typ = d[i+4:i+8]
        if typ == b'IHDR': w,h = unpack('>II', d[i+8:i+16])
        if typ == b'IDAT': idat += d[i+8:i+8+ln]
        i += 12+ln
    raw = zlib.decompress(idat); out=[]; stride=w*3; prev=bytearray(stride)
    pos=0
    for _ in range(h):
        f=raw[pos]; line=bytearray(raw[pos+1:pos+1+stride]); pos+=1+stride
        for x in range(stride):
            a=line[x-3] if x>=3 else 0; b=prev[x]; c=prev[x-3] if x>=3 else 0
            if f==1: line[x]=(line[x]+a)&255
            elif f==2: line[x]=(line[x]+b)&255
            elif f==3: line[x]=(line[x]+(a+b)//2)&255
            elif f==4:
                p=a+b-c; pa,pb,pc=abs(p-a),abs(p-b),abs(p-c)
                pr=a if (pa<=pb and pa<=pc) else (b if pb<=pc else c)
                line[x]=(line[x]+pr)&255
        out += list(line); prev=line
    return w,h,out
for name in ("mask_curve","mask_grade"):
    w,h,px = read(f"crates/pipeline/tests/golden/{name}.png")
    pinned = sum(1 for v in px if v==255)
    print(f"{name}: {w}x{h}  pinned-at-255 {pinned}/{len(px)}  distinct {len(set(px))}")
PY
```

Expected: **`pinned-at-255` well under a few percent** and `distinct` in the low hundreds. ⚠ If either reference pins heavily, weaken the edit (lower the curve's midpoint, reduce a wheel's saturation) and regenerate — **never** widen `TOL`.

- [ ] **Step 5: Confirm the two references differ from each other and from `neutral`**

```bash
cmp -s crates/pipeline/tests/golden/mask_curve.png crates/pipeline/tests/golden/mask_grade.png && echo "IDENTICAL - BUG" || echo "differ, good"
cmp -s crates/pipeline/tests/golden/mask_curve.png crates/pipeline/tests/golden/neutral.png && echo "IDENTICAL TO NEUTRAL - BUG" || echo "differ, good"
cmp -s crates/pipeline/tests/golden/mask_grade.png crates/pipeline/tests/golden/neutral.png && echo "IDENTICAL TO NEUTRAL - BUG" || echo "differ, good"
```

Expected: three `differ, good`. ⚠ A stage that never ran would produce a reference identical to `neutral` and the golden test would pass forever against it.

- [ ] **Step 6: Run the full golden suite**

```bash
cargo test -p pipeline --test golden 2>&1 | grep "test result"
ls crates/pipeline/tests/golden | wc -l
```

Expected: `57 passed`, and 57 files. ⚠ **The 55 that existed must be byte-identical** — that is the acceptance criterion, not a nicety:

```bash
git status --short crates/pipeline/tests/golden/
```

Expected: exactly two `??` lines (the new files) and **no `M` lines**.

- [ ] **Step 7: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/mask_curve.png crates/pipeline/tests/golden/mask_grade.png
git commit -q -F - <<'EOF'
test(pipeline): goldens for a masked curve and a masked grade

55 -> 57 goldens, and the 55 are byte-identical -- checked with git
status on the golden directory, not assumed.

Both references were measured rather than eyeballed: neither pins
samples at 255, so neither can mistake this edit for a stronger one.
That is the weakness the radial exposure golden had to be re-authored to
remove. Both were also compared against neutral.png, since a stage that
never ran would produce a reference identical to it and the test would
pass forever against that.
EOF
```

---

## Task 6: The panel

**Files:**
- Modify: `crates/app/src/main.rs:342` (add `mask_grade_range` beside `mask_hsl_channel`) and `:577` (its initialiser)
- Modify: `crates/app/src/test_app.rs:136` (the exhaustive fixture)
- Modify: `crates/app/src/develop/panel/mod.rs:230-242` (the `masks::show` call)
- Modify: `crates/app/src/develop/panel/masks.rs:32-38` (signature) and `:205` (the `mask_adjust::show` call)
- Modify: `crates/app/src/develop/panel/mask_adjust.rs` (the two collapsed blocks — **all new UI code**)

⚠ **This is one commit.** `mask_grade_range` is a field on a bin-crate type with no reader until the panel uses it, so a commit adding only the field fails `clippy -D warnings` as dead code. At least the fifth recorded instance of that trap in this project.

- [ ] **Step 1: Write the failing wiring tests**

⚠⚠ **This is the test that matters, and W5's record is why.** Creative LUTs had four passing pipeline tests and shipped 100% non-functional, because nothing proved the table ever reached the pipeline. Ask what carries the data to the tested code, and test *that*. Here the carrier is panel → `LocalAdjustments`.

Add to the `tests` module in `crates/app/src/develop/panel/mask_adjust.rs` (create the module if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::panel::{adjustments, color_wheel};
    use eframe::egui;

    /// Drive the real `mask_adjust::show` through one pointer gesture over the tone-curve
    /// editor, and report the adjustments it left behind.
    ///
    /// ⚠⚠ **Five passes, and the split is what makes this honest.** Lay out; move to `from`;
    /// press *while still there*; move to `to`; release. egui resolves interaction against the
    /// widgets registered on the **previous** pass, so a harness that moves and presses on the
    /// same pass lands its press away from the target and the test passes for the wrong reason
    /// — exactly how `a_left_drag_does_not_pan` went inert once already.
    ///
    /// ⚠ No screen coordinate is written down. `curve_rect` comes back from the widget, which
    /// is why `tone_curve::show` returns its rect at all — a hardcoded point is brittle in the
    /// worst way, because a layout change moves the target while the test keeps passing.
    fn drag_on_the_curve(from: [f32; 2], to: [f32; 2]) -> (LocalAdjustments, Edited) {
        let ctx = egui::Context::default();
        let mut a = LocalAdjustments::default();
        let mut ed = Edited::default();
        let mut channel = adjustments::HslChannel::default();
        let mut range = color_wheel::GradeRange::default();
        let mut rect = egui::Rect::NOTHING;
        let at = |r: egui::Rect, n: [f32; 2]| {
            egui::pos2(r.left() + n[0] * r.width(), r.bottom() - n[1] * r.height())
        };

        for pass in 0..5 {
            let (p, q) = (at(rect, from), at(rect, to));
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 2400.0),
                )),
                ..Default::default()
            };
            match pass {
                1 => input.events.push(egui::Event::PointerMoved(p)),
                2 => input.events.push(egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }),
                3 => input.events.push(egui::Event::PointerMoved(q)),
                4 => input.events.push(egui::Event::PointerButton {
                    pos: q,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }),
                _ => {}
            }
            let _ = ctx.run_ui(input, |ui| {
                rect = show_for_test(ui, &mut a, &mut channel, &mut range, &mut ed);
            });
        }
        (a, ed)
    }

    /// ⚠⚠ **The carrier test.** The pipeline half of this milestone is proven by
    /// `a_masked_tone_curve_runs_after_the_tone_regions_and_before_local_contrast`; nothing
    /// there proves the panel's value ever reaches `LocalAdjustments`. W5 shipped a milestone
    /// whose maths was fully proven and which was 100% non-functional for exactly that reason.
    #[test]
    fn dragging_the_masked_curve_reaches_the_masks_own_tone_curve() {
        let (a, ed) = drag_on_the_curve([0.5, 0.5], [0.5, 0.8]);
        assert!(
            !a.tone_curve.points.is_empty(),
            "the drag never reached the mask's tone_curve"
        );
        assert!(ed.changed, "the preview must be re-requested");
        assert!(ed.settled, "and a settled drag is one undo step");
    }

    /// ⚠ The mask's grade range must be its own. Sharing the global panel's would silently
    /// change which wheel the global panel shows when you switch back to it — the reason
    /// `mask_hsl_channel` exists.
    #[test]
    fn the_masked_grade_range_is_not_the_global_one() {
        let ctx = egui::Context::default();
        let mut a = LocalAdjustments::default();
        let mut ed = Edited::default();
        let mut channel = adjustments::HslChannel::default();
        let mut mask_range = color_wheel::GradeRange::Highlights;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 2400.0),
                )),
                ..Default::default()
            },
            |ui| {
                show_for_test(ui, &mut a, &mut channel, &mut mask_range, &mut ed);
            },
        );
        assert_eq!(
            mask_range,
            color_wheel::GradeRange::Highlights,
            "drawing the panel must not reset the mask's own selected wheel"
        );
    }
}
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p app --lib dragging_the_masked_curve 2>&1 | tail -20
```

Expected: a compile error — `show_for_test` and the `mask_grade_range` parameter do not exist yet.

⚠ **`--lib`, not `--bin Rustroom`.** `mask_adjust` is a bin module, so this filter is wrong on purpose in the plan's *expectation* only if you get it wrong — see Step 8, which pins the right invocation. If `--lib` reports `0 passed; N filtered out`, that is the false-pass trap, not a pass.

- [ ] **Step 3: Add `mask_grade_range` to `RustroomApp`**

In `crates/app/src/main.rs`, immediately after the `mask_hsl_channel` field (line ~342):

```rust
    /// Which colour-grading range a **mask's** wheel is showing. View state, like
    /// `mask_hsl_channel`.
    ///
    /// ⚠ Its own field rather than sharing `grade_range`: switching to a mask would otherwise
    /// silently change which wheel the global panel shows when you switch back.
    mask_grade_range: develop::panel::color_wheel::GradeRange,
```

And in the initialiser at line ~577, after `mask_hsl_channel: Default::default(),`:

```rust
            mask_grade_range: Default::default(),
```

In `crates/app/src/test_app.rs`, after `mask_hsl_channel: Default::default(),` (line ~136):

```rust
        mask_grade_range: Default::default(),
```

⚠ That fixture's literal is exhaustive on purpose — a new field on `RustroomApp` is a compile error there, forcing a decision about what a test should see. Do not reach for `..Default::default()`.

- [ ] **Step 4: Thread it through `masks::show`**

In `crates/app/src/develop/panel/masks.rs`, change the signature (line ~32):

```rust
pub fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    mask_hsl: &mut super::adjustments::HslChannel,
    mask_grade: &mut super::color_wheel::GradeRange,
    brush: BrushTool<'_>,
    ed: &mut Edited,
) {
```

and the call at line ~205:

```rust
            super::mask_adjust::show(ui, &mut m.adjustments, baseline, mask_hsl, mask_grade, ed);
```

In `crates/app/src/develop/panel/mod.rs`, in the `Tool::Masks => masks::show(` arm, add after `&mut app.mask_hsl_channel,`:

```rust
                            &mut app.mask_grade_range,
```

In `masks.rs`'s own test helper `click_in_panel`, add beside the existing `channel` local:

```rust
        // ⚠ A local, like `channel`: a test must not be able to assert about the *global*
        // panel's selected wheel.
        let mut grade = crate::develop::panel::color_wheel::GradeRange::default();
```

and pass `&mut grade` in the `show(...)` call.

- [ ] **Step 5: Add the two collapsed blocks**

In `crates/app/src/develop/panel/mask_adjust.rs`, change the signature and add the blocks. The imports at the top become:

```rust
use crate::develop::panel::{adjustments, color_wheel, hsl_bands, tone_curve, Edited};
use edit_model::LocalAdjustments;
use eframe::egui;
```

The signature becomes:

```rust
pub fn show(
    ui: &mut egui::Ui,
    a: &mut LocalAdjustments,
    baseline: color::wb::Illuminant,
    channel: &mut adjustments::HslChannel,
    grade: &mut color_wheel::GradeRange,
    ed: &mut Edited,
) {
```

Insert the **tone curve block** immediately after the `Blacks` slider and before the `Vibrance` slider, so the panel reads in pipeline order:

```rust
    // ⚠ Collapsed, closed by default. A mask's section is already the tallest thing in the
    // panel; a curve editor is a ~220 pt square rather than a row of sliders.
    //
    // ⚠ The open/closed state lives in egui's memory and its id path is the same for every
    // mask, so it is **shared across masks**. Deliberate, not an accident: you want the block
    // to stay open as you click through masks comparing them.
    egui::CollapsingHeader::new("Tone Curve")
        .id_salt("mask_tone_curve")
        .default_open(false)
        .show(ui, |ui| {
            // The same widget the global panel draws, unchanged — `hsl_bands`' rule, and
            // `adjustments.rs`' own "One layout learned once".
            tone_curve::show(ui, &mut a.tone_curve, ed);
        });
```

Insert the **colour grading block** immediately after the `hsl_bands::show(...)` call and its `ui.separator()`, before the Sharpening block:

```rust
    ui.separator();
    // ⚠ Collapsed for the curve's reason: a wheel is a ~188 pt square plus five sliders.
    egui::CollapsingHeader::new("Color Grading")
        .id_salt("mask_color_grading")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for r in color_wheel::GradeRange::ALL {
                    ui.selectable_value(grade, r, r.label());
                }
            });
            let diameter = ui.available_width().min(188.0);
            {
                // Scoped so the borrow of `color_grading` ends before Blending and Balance,
                // which need the struct itself.
                color_wheel::show(ui, &mut a.color_grading, *grade, diameter, ed);
                let w = grade.wheel_mut(&mut a.color_grading);
                ed.slider(ui, &mut w.hue, 0.0..=360.0, "Hue");
                ed.slider(ui, &mut w.saturation, 0.0..=100.0, "Saturation");
                ed.slider(ui, &mut w.luminance, -100.0..=100.0, "Luminance");
            }
            ui.separator();
            let g = &mut a.color_grading;
            // ⚠⚠ **Blending and Balance are carried inside a mask, and that is parity rather
            // than padding.** They are properties of the *range decomposition* shared by all
            // four wheels, so omitting them would make a masked grade a different control from
            // the global one wearing the same name — the Whites-slider failure this project has
            // already paid for once.
            ed.slider(ui, &mut g.blending, 0.0..=100.0, "Blending");
            ed.slider(ui, &mut g.balance, -100.0..=100.0, "Balance");
        });
```

Finally add the test-only entry point the harness drives, just above the `#[cfg(test)] mod tests`:

```rust
/// The panel as it ships, returning the curve editor's own rect.
///
/// ⚠ Test-only, and it exists for one reason: a headless test must aim a drag at a point on the
/// curve, and the alternative is a hardcoded screen coordinate — brittle in the worst way,
/// because a layout change then moves the target while the test keeps passing and grabbing
/// nothing. `test_ui`'s module doc records that fault happening twice.
#[cfg(test)]
fn show_for_test(
    ui: &mut egui::Ui,
    a: &mut LocalAdjustments,
    channel: &mut adjustments::HslChannel,
    grade: &mut color_wheel::GradeRange,
    ed: &mut Edited,
) -> egui::Rect {
    let mut rect = egui::Rect::NOTHING;
    egui::CollapsingHeader::new("Tone Curve")
        .id_salt("mask_tone_curve")
        .default_open(true)
        .show(ui, |ui| {
            rect = tone_curve::show(ui, &mut a.tone_curve, ed);
        });
    show(ui, a, color::wb::Illuminant::default(), channel, grade, ed);
    rect
}
```

⚠ The harness opens its own copy of the block because the shipped one is **closed by default**, and a `CollapsingHeader` that is closed does not run its body — so the widget would never be laid out and the drag would land on nothing. The shipped `show` is still called, which is what keeps the test driving production code rather than a re-declared panel.

- [ ] **Step 6: Run the wiring tests**

```bash
cargo test -p app --bin Rustroom dragging_the_masked_curve the_masked_grade_range 2>&1 | grep "test result"
```

Expected: `2 passed`. ⚠⚠ **Read the passed-count.** `mask_adjust` is a **bin** module, so `--bin Rustroom` is right here and `--lib` would report `0 passed; N filtered out` with exit 0 — the trap this project hit twice in one session at M7.2.

- [ ] **Step 7: Run the whole app suite and clippy**

```bash
cargo test -p app 2>&1 | grep "test result"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "CLIPPY=$?"
```

Expected: green, `CLIPPY=0`. ⚠ **Read the exit code from a file, never through a pipe** — a `cargo clippy | grep` pipeline reports grep's status, which has let a failing clippy into a commit here before. At least the ninth recorded instance of clippy catching what a green suite did not.

- [ ] **Step 8: Report the file sizes**

```bash
wc -l crates/app/src/develop/panel/masks.rs crates/app/src/develop/panel/mask_adjust.rs
```

⚠ **`masks.rs` was 614 lines before this task** and is already over the ~600 marker this project asks to be reported. Say what it is now in the commit message. It should have grown by ~4 lines (one parameter, one argument, one test local); everything else went into `mask_adjust.rs`.

- [ ] **Step 9: Commit**

```bash
git add crates/app/src/main.rs crates/app/src/test_app.rs crates/app/src/develop/panel/mod.rs crates/app/src/develop/panel/masks.rs crates/app/src/develop/panel/mask_adjust.rs
git commit -q -F - <<'EOF'
feat(app): a mask's panel carries a tone curve and a colour grade

Two collapsed blocks in mask_adjust.rs, closed by default, reusing
tone_curve::show and color_wheel::show unchanged -- the straight reuse
hsl_bands got at M7.2, honouring adjustments.rs' own "One layout learned
once". A mask's section is already the tallest thing in the panel and
these two are squares, not rows of sliders.

RustroomApp gains mask_grade_range, mirroring mask_hsl_channel: a mask
must not share the global panel's selected wheel, or switching back
silently shows a different range.

The collapsed state lives in egui memory with the same id path for every
mask, so it is shared across masks. Deliberate -- you want the block to
stay open while clicking through masks comparing them.

THE TEST THAT MATTERS IS THE CARRIER TEST. The pipeline half is already
proven by the ordering tests, and nothing there proves the panel's value
reaches LocalAdjustments. W5 shipped a milestone whose maths was fully
proven and which was 100% non-functional for exactly that reason. The
drag is aimed with the rect tone_curve::show returns, never a hardcoded
coordinate.

One commit, not two: mask_grade_range has no reader until the panel uses
it, and a bin-crate field with no reader fails clippy -D warnings as dead
code.
EOF
```

---

## Task 7: Measure both stages

**Files:**
- Modify: `crates/app/examples/bench_masked.rs`

⚠ **Measuring is required here, not optional.** This file's own record: HSL's cost was mis-reasoned **three times**, every one from comparing against colour grading instead of decomposing, and the hue LUT then found the band weighting at 42% where `cos`/`sin` was under 10%. A third consumer of `ColorGradingStage::new` is exactly the kind of thing that gets estimated and is wrong.

- [ ] **Step 1: Add the two adjustment builders**

In `crates/app/examples/bench_masked.rs`, after `hsl_adjust()`:

```rust
/// A masked tone curve. ⚠ `ToneCurveStage::new` builds a **1024**-entry LUT per construction,
/// against colour grading's 256 — so of the two stages added in this milestone, the curve is the
/// larger table build, which is the opposite of what the backlog entry implied.
fn curve() -> LocalAdjustments {
    LocalAdjustments {
        tone_curve: ToneCurve {
            points: vec![[0.0, 0.0], [0.4, 0.6], [1.0, 1.0]],
        },
        ..Default::default()
    }
}

/// A masked split tone. ⚠ The **third** consumer of `ColorGradingStage::new`, whose 256-entry
/// table is built per stage, i.e. per mask per render rather than per pixel.
fn grade() -> LocalAdjustments {
    LocalAdjustments {
        color_grading: ColorGrading {
            shadows: GradeWheel {
                hue: 240.0,
                saturation: 45.0,
                luminance: -10.0,
            },
            highlights: GradeWheel {
                hue: 50.0,
                saturation: 40.0,
                luminance: 8.0,
            },
            ..ColorGrading::default()
        },
        ..Default::default()
    }
}
```

Extend the `use edit_model::{...}` list at the top with `ColorGrading, GradeWheel, ToneCurve`.

- [ ] **Step 2: Add the rows**

In `main`'s `cases` vec, after the `3 masked HSL (gradient)` entry:

```rust
        ("1 masked curve   (gradient)", masked(1, gradient, curve)),
        ("3 masked curve   (gradient)", masked(3, gradient, curve)),
        ("1 masked grade   (gradient)", masked(1, gradient, grade)),
        ("3 masked grade   (gradient)", masked(3, gradient, grade)),
```

and after the `1 masked HSL (small radial)` entry:

```rust
        ("1 masked curve   (small radial)", masked(1, radial, curve)),
        ("1 masked grade   (small radial)", masked(1, radial, grade)),
```

- [ ] **Step 3: Build it**

```bash
cargo build --release -p app --example bench_masked 2>&1 | tail -5
```

Expected: no errors.

- [ ] **Step 4: Run it on a real RAW and record the numbers**

```bash
ls ~/Desktop/fuji/*.RAF 2>/dev/null | head -3 || echo "no fixture at ~/Desktop/fuji"
cargo run --release -p app --example bench_masked <path-to.RAF>
```

⚠ If no `.RAF` is on this machine, **stop and report that** rather than inventing figures. The bench is the deliverable of this task; a table nobody ran is worse than no table.

Record the fit and native-region columns. Reference points from M7.2 on the same machine: neutral **12.7 ms** at fit; one masked clarity **+10.1**; one masked HSL **+33.1**, since improved to **+23.0** by the hue LUT.

- [ ] **Step 5: Commit**

⚠ Put the measured table in the commit message. **No optimisation is in scope** — if a number is bad, report it and scope it separately, the way M7.2 handled the bounding box and the hue LUT.

```bash
git add crates/app/examples/bench_masked.rs
git commit -q -F - <<'EOF'
perf(app): measure a masked curve and a masked grade

<paste the measured table here, fit and native region, with the deltas
against neutral>

Measured rather than estimated, because this file's record says to: HSL's
cost was mis-reasoned three times, every one from comparing against
colour grading instead of decomposing, and measuring the hue LUT then
found the band weighting at 42% where cos/sin was under 10%.

One correction the measurement was set up for: the tone curve builds a
1024-entry LUT per construction against colour grading's 256, so the
curve is the larger table build of the two -- the opposite of what the
backlog entry implied.

No optimisation here. If a number is bad it gets its own milestone, the
way the bounding box and the hue LUT each did.
EOF
```

---

## Task 8: Sabotage the guards, then run every gate

⚠⚠ **A sabotage that does not fail is a finding about the test, not a formality to wave through.** And **confirm the sabotage compiled** before believing anything — a sabotage that fails to compile looks exactly like one that passes.

- [ ] **Step 1: Sabotage the double application**

In `crates/pipeline/src/lib.rs`, in `build_pipeline_split`'s literal, change `tone_curve: edit_model::ToneCurve::default(),` to `tone_curve: edits.tone_curve.clone(),` and `color_grading: edit_model::ColorGrading::default(),` to `color_grading: edits.color_grading,`.

```bash
cargo test -p pipeline the_global_tail_does_not_apply_the_mask_only_stages_twice 2>&1 | grep -E "test result|panicked" | head -5
```

Expected: **FAIL**. ⚠ If it passes, that test enumerates only `local_contrast`, `sharpening` and `hsl` — extend its array to `["local_contrast", "sharpening", "hsl", "tone_curve", "color_grading"]` and update its doc comment, then re-run. **That extension is required either way** — a guard that names three of five fields is a guard that will miss the next one.

- [ ] **Step 2: Confirm the same sabotage moves goldens**

```bash
cargo test -p pipeline --test golden 2>&1 | grep -c "FAILED\|panicked"
```

Expected: several golden failures. That double signal — a named test *and* moved references — was the strongest guard in M7.2 and must hold here.

- [ ] **Step 3: Revert the sabotage and confirm green**

```bash
git checkout -- crates/pipeline/src/lib.rs
cargo test -p pipeline 2>&1 | grep "test result"
```

- [ ] **Step 4: Sabotage the order — the grade before HSL**

In `adjustment_stages`, move the `color_grading` block above the `hsl` block.

```bash
cargo test -p pipeline a_masked_colour_grade_runs_after 2>&1 | grep -E "test result|the grade must follow"
```

Expected: **FAIL** with `the grade must follow HSL`. Then `git checkout -- crates/pipeline/src/lib.rs`.

- [ ] **Step 5: Sabotage the order — the curve after local contrast**

Move the `tone_curve` block below the `local_contrast` block.

```bash
cargo test -p pipeline a_masked_tone_curve_runs_after 2>&1 | grep -E "test result|must precede"
```

Expected: **FAIL** with `the curve must precede local contrast`. Then `git checkout -- crates/pipeline/src/lib.rs`.

- [ ] **Step 6: Sabotage the carrier — the panel writes nowhere**

In `mask_adjust.rs`, change the tone curve block's body to draw against a throwaway:

```rust
            let mut scratch = edit_model::ToneCurve::default();
            tone_curve::show(ui, &mut scratch, ed);
```

```bash
cargo test -p app --bin Rustroom dragging_the_masked_curve 2>&1 | grep -E "test result|never reached"
```

Expected: **FAIL** with `the drag never reached the mask's tone_curve`. ⚠ **This is the W5 sabotage** — it reproduces exactly the shape of bug that shipped a fully-tested milestone dead. Then `git checkout -- crates/app/src/develop/panel/mask_adjust.rs`.

- [ ] **Step 7: Sabotage the fixtures**

In `edit-model`'s `all_nonneutral`, delete the `tone_curve` line from the mask.

```bash
cargo test -p edit-model the_exhaustive_fixture_carries 2>&1 | grep -E "test result|must carry"
```

Expected: **FAIL**. Then `git checkout -- crates/edit-model/src/lib.rs`.

- [ ] **Step 8: Run every gate together, on the reverted tree**

```bash
git status --short
cargo fmt --check > /tmp/fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/test.log 2>&1; echo "TEST=$?"
grep "test result" /tmp/test.log
grep -c "^test golden_" /tmp/test.log
```

Expected: `git status` clean apart from nothing; `FMT=0`, `CLIPPY=0`, `TEST=0`. ⚠ **Read the passed-counts and confirm 57 golden tests actually RAN** — `test result: ok` says nothing about what ran, and a silently skipped golden suite is how a moved reference ships.

- [ ] **Step 9: Commit any test extension from Step 1**

If `the_global_tail_does_not_apply_the_mask_only_stages_twice` had to be extended:

```bash
git add crates/pipeline/src/lib.rs
git commit -q -F - <<'EOF'
test(pipeline): the double-application guard names all five fields

It enumerated local_contrast, sharpening and hsl, so passing the real
tone_curve or color_grading from the global tail left it green -- a guard
that names three of five is a guard that misses the next one.

Watched failing under the sabotage: hand edits.tone_curve and
edits.color_grading to the shared builder. It fails its own assertion AND
moves goldens, which is the double signal that made this the strongest
guard in M7.2.
EOF
```

---

## Task 9: Run the GUI and hand over the checklist

⚠ **Nothing here can be self-checked.** `AXIsProcessTrusted()` is false on this machine, so synthetic input is silently dropped and `screencapture` fails. Launch the app, say what to look at, and **wait for the report** — this project's record is that every comparable milestone's defects were found by using it, and M7.2 was the first with none.

- [ ] **Step 1: Launch it**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Run it in the background, always `--release`.

- [ ] **Step 2: Hand over this checklist**

1. **A graduated filter over a sky, Tone Curve block opened, midtones lifted.** The sky's midtones lift; the foreground does not. This is the carrier working end to end.
2. **The same mask, curve pulled down.** Dodge and burn with a curve, which is the more obviously wanted of the two.
3. **A radial mask on a face, Color Grading opened, highlights warmed.** The face warms; the room behind it does not.
4. **Blending and Balance on that masked grade.** They must visibly reshape which tones take the tint — if they do nothing, the range decomposition is not reaching the stage.
5. ⚠ **Global grade and masked grade both active, different hues.** The masked one must lay on *top* of the global look rather than replacing it, and the global panel's selected wheel must not change when you click into the mask and back.
6. ⚠ **Switch between two masks with both blocks open.** The blocks stay open (deliberate, shared state) and each mask shows *its own* curve and wheel — not the previous mask's.
7. ⚠ **Zoom to 1:1 with a masked grade active** and check responsiveness against the numbers from Task 7.

- [ ] **Step 3: Wait for the report, then fix what comes back**

Every defect reported here is a real one. Do not close the milestone on green tests alone — M7.2 is the *only* milestone in this project's record whose GUI wiring produced no defects, and the record says to read that as the panel being a straight extraction of a layout that already worked, not as the class of bug being over.

---

## Task 10: Update `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md` (the RESUME block, and a new section beside "M7.2 — masked spatial adjustments")

- [ ] **Step 1: Write the milestone section**

Add a section immediately above M7.2's, following its shape: the headline (test and golden counts before → after, confirmed-by-use status), then the ⚠ findings. Cover at minimum:

- **`LocalAdjustments` lost `Copy` and it cost nothing** — measured with `cargo check --workspace --all-targets`, zero call-site changes, because `MaskedAdjustment` had not been `Copy` since the brush landed.
- **The stale doc comment M7.2 left behind**, now corrected — a comment that says spatial stages are out of scope with three of them declared beneath it.
- **The tone curve builds the larger table** (1024 vs 256), correcting the backlog's framing.
- **The measured table** from Task 7.
- **The double-application guard had to be extended** to five fields, if it did.
- **What the GUI checklist found.**
- **Accepted caveats:** Part B still outstanding and why; no bounding box (deferred a fifth time); the collapsed state is shared across masks; a mask's section is taller again.

- [ ] **Step 2: Update the RESUME block**

Replace the `▶ NEXT ACTION` and the `▶ ASKED FOR AT M7.2's MERGE` paragraphs so they say Part A is closed and **Part B is what remains**, with its blocking design question intact. Update the test and golden counts in the state-at-handoff line.

- [ ] **Step 3: Update `docs/BACKLOG.md`**

Mark Part A done with its date and the spec/plan filenames; **leave Part B's entry exactly as it is**, including the three ways out and the airlight question. ⚠ Do not let Part A's completion imply anything about Part B — the whole point of the split is that they are not comparable in size.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/BACKLOG.md
git commit -q -F - <<'EOF'
docs: close masked colour grading and the tone curve

<counts before -> after, and what using it found>

Part B -- dehaze and noise reduction -- is untouched and its backlog
entry is left exactly as it was, blocking design question and all. The
split exists because the two halves are not comparable in size, and Part
A landing says nothing about Part B.
EOF
```

---

## Self-review

**Spec coverage:** §2's two fields and the `Copy` loss → Task 1. §2's stale doc comment → Task 1 Step 3. §2's sidecar compatibility → Task 1's serde test. §3's two insertions and their order → Tasks 3 and 4. §3's neutral pass → Task 1 Step 5, sabotaged in Task 8. §3's exhaustive-sites table → Task 2. §4's collapsed blocks, `mask_grade_range` and placement → Task 6. §5's bench → Task 7. §6's goldens → Task 5; its wiring test → Task 6; its three named sabotages → Task 8 Steps 1, 4-5 and 6. §7's caveats → Task 10.

**Placeholders:** the only intentional gaps are Task 7's measured table and Task 10's counts, which cannot be known before the runs that produce them; both steps say explicitly to paste real output and to stop rather than invent it.

**Type consistency:** `adjustment_stages(&LocalAdjustments, Illuminant, RenderScale)` is used identically in Tasks 3, 4 and their tests. `tone_curve::show(ui, &mut ToneCurve, &mut Edited) -> egui::Rect` and `color_wheel::show(ui, &mut ColorGrading, GradeRange, f32, &mut Edited) -> egui::Rect` match the shipped signatures. `mask_adjust::show`'s new `grade: &mut GradeRange` parameter is added in Task 6 Step 5 and passed from `masks.rs` in Step 4 in the same position. Stage name strings are `"tone_curve"` and `"color_grading"`, matching the shipped `PipelineStage::name` returns.
