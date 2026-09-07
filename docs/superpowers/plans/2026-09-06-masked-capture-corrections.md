# Masked noise reduction and dehaze — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A mask carries noise reduction and dehaze, running in the **prefix** where the global pair already run.

**Architecture:** Two fields on `LocalAdjustments`; a `capture_stages` builder beside the existing `adjustment_stages`; a second `MaskStage` constructor (`new_capture`, `region: None`) appended to the prefix after the global dehaze; a `PrefixStepKey::MaskedCapture` keyed on a **projection** so masked NR does not re-run on every masked-exposure drag; and the tail passing both fields neutral, forced at compile time.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-06-masked-capture-corrections-design.md`

**Build note:** every `cargo` command needs libraw and lcms2 on the pkg-config path. Export once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **`cargo` builds in this repo take minutes.** Run them in the background and read the exit code from a file; never through a pipe.
⚠ **`cargo test` takes ONE positional filter, not two** — a second is `error: unexpected argument`.
**Commit messages carry no `Co-Authored-By` trailer.** **Never `git push`.** Branch `masked-capture-corrections`, already created.

---

## File structure

| file | change | responsibility |
|---|---|---|
| `crates/edit-model/src/lib.rs` | modify | the two new fields; `all_nonneutral` fixture |
| `crates/pipeline/src/lib.rs` | modify | `capture_stages`, the prefix insertion, `PrefixStepKey::MaskedCapture`, the tail's neutral pass, `all_edits`, order + projection tests |
| `crates/pipeline/src/stages/mask.rs` | modify | `MaskStage::new_capture`; the two stale comments |
| `crates/pipeline/tests/golden.rs` | modify | two new golden cases |
| `crates/pipeline/tests/golden/mask_nr.png` | create | reference |
| `crates/pipeline/tests/golden/mask_dehaze.png` | create | reference |
| `crates/app/src/render_engine.rs` | modify | masked NR + dehaze added to the master region invariant |
| `crates/app/src/develop/panel/mask_adjust.rs` | modify | the five sliders, and the rewritten ordering comment |
| `crates/app/examples/bench_masked.rs` | modify | masked-NR and masked-dehaze rows |

⚠ **No change to `tail_kernel_radius`, `EditGroups`, `merge_from` or `Session::render_margin`.** Nothing new lands in the tail. If a task finds itself editing one of those, **stop and re-read the spec** — it means the placement went wrong.

---

## Task 1: The two fields, and the neutral pass the compiler forces

**Files:**
- Modify: `crates/edit-model/src/lib.rs` (the `LocalAdjustments` fields)
- Modify: `crates/pipeline/src/lib.rs:531`-ish (the exhaustive literal in `build_pipeline_split`)

⚠ One task, one commit: adding the fields breaks `pipeline`'s compilation by design (`E0063`), so an `edit-model`-only commit would not build.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/edit-model/src/lib.rs`:

```rust
/// ⚠ Both halves, and both on their own. A test setting both could not tell a wired field from
/// an unwired one sitting beside a wired one.
#[test]
fn a_masked_noise_reduction_or_dehaze_makes_the_set_active() {
    assert!(!LocalAdjustments::default().is_active());

    let denoised = LocalAdjustments {
        noise_reduction: NoiseReduction {
            luminance: 40.0,
            ..NoiseReduction::default()
        },
        ..Default::default()
    };
    assert!(denoised.is_active(), "masked noise reduction must be active");

    let hazed = LocalAdjustments {
        dehaze: Dehaze { amount: 60.0 },
        ..Default::default()
    };
    assert!(hazed.is_active(), "masked dehaze must be active");
}

/// ⚠ `is_active` is amount-only on both, reused rather than reinvented: Detail and Smoothness
/// shape an effect that is off until Luminance or Color is turned up, so moving one alone must
/// not insert a whole-buffer stage into the prefix.
#[test]
fn masked_noise_reduction_modifiers_alone_are_inert() {
    let a = LocalAdjustments {
        noise_reduction: NoiseReduction {
            detail: 90.0,
            smoothness: 10.0,
            ..NoiseReduction::default()
        },
        ..Default::default()
    };
    assert!(
        !a.noise_reduction.is_active(),
        "detail and smoothness alone must insert no stage"
    );
}

/// ⚠ A sidecar written before today has neither field, and both must read as neutral — with
/// `detail` and `smoothness` at 50 from `NoiseReduction`'s hand-written `Default`, not 0.
#[test]
fn a_mask_written_before_the_capture_fields_reads_them_neutral() {
    let json = r#"{"mask":{"Linear":{"from":[0.0,0.0],"to":[1.0,0.0]}},
                   "adjustments":{"exposure":0.5}}"#;
    let m: MaskedAdjustment = serde_json::from_str(json).unwrap();
    assert_eq!(m.adjustments.exposure, 0.5, "fixture check: the json was read");
    assert_eq!(m.adjustments.noise_reduction, NoiseReduction::default());
    assert_eq!(m.adjustments.noise_reduction.detail, 50.0);
    assert_eq!(m.adjustments.dehaze, Dehaze::default());
}
```

⚠ `NoiseReduction` and `Dehaze` must derive `PartialEq` for these `assert_eq!`s — check, and add it if absent.

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p edit-model a_masked_noise_reduction_or_dehaze 2>&1 | tail -20
```

Expected: `error[E0560]: struct `LocalAdjustments` has no field named `noise_reduction``.

- [ ] **Step 3: Add the fields**

In `crates/edit-model/src/lib.rs`, in `LocalAdjustments`. Put them at the **top of the struct**, before `white_balance`, because that is where they run:

```rust
    /// Noise reduction confined to the mask — denoise a shadowed corner without smearing the
    /// rest of the frame.
    ///
    /// ⚠⚠ **This runs in the PREFIX, not the tail** — the only field here that does, together
    /// with `dehaze`. It is placed exactly where the global stage runs, so a masked Luminance 50
    /// means what a global Luminance 50 means. See `capture_stages`.
    pub noise_reduction: NoiseReduction,
    /// Dehaze confined to the mask — clear a distant ridge and leave the foreground alone.
    ///
    /// ⚠⚠ **Prefix, like `noise_reduction`, and for a sharper reason: the airlight.** It is
    /// estimated from the brightest 0.1% of the buffer the stage is handed. In the tail that
    /// buffer is the visible *region*, so a tail-side masked dehaze would re-estimate its
    /// airlight as you panned and the photograph would change under you. The prefix always runs
    /// whole-buffer, which is what makes a frame-global airlight possible at all.
    pub dehaze: Dehaze,
```

Then extend `LocalAdjustments`' own doc comment, which §8 of the spec notes still describes only the tail:

```rust
/// ⚠ **Two of these run in the PREFIX and the rest in the tail** — `noise_reduction` and
/// `dehaze` are corrections of the *capture*, and they are placed where the global stages of
/// the same name run rather than where `MaskStage` happens to sit. `capture_stages` builds that
/// pair; `adjustment_stages` builds everything else.
```

- [ ] **Step 4: Check the workspace and watch the forcing function fire**

```bash
cargo check -p pipeline 2>&1 | grep -E "^error|-->" | head
```

Expected: `error[E0063]: missing fields `dehaze` and `noise_reduction`` at `build_pipeline_split`'s literal.

- [ ] **Step 5: Pass both neutral from the tail**

In that literal, add the two fields at the top and extend the ⚠⚠ comment:

```rust
        &edit_model::LocalAdjustments {
            // ⚠⚠ **Neutral because these two do not run here at all** — unlike the five below,
            // which this tail places itself further down. `noise_reduction` and `dehaze` are
            // built by `capture_stages` and run in the **prefix**, where the global pair run.
            // Passing them here would apply a capture correction after the tone curve, on a
            // buffer that may be only the visible region — which for dehaze means an airlight
            // that changes as you pan.
            noise_reduction: edit_model::NoiseReduction::default(),
            dehaze: edit_model::Dehaze::default(),
            white_balance: edits.white_balance,
            // … the rest unchanged …
```

- [ ] **Step 6: Run the new tests and the whole suite**

```bash
cargo test -p edit-model a_masked_noise_reduction_or_dehaze 2>&1 | grep "test result"
cargo test -p edit-model masked_noise_reduction_modifiers_alone 2>&1 | grep "test result"
cargo test -p edit-model a_mask_written_before_the_capture_fields 2>&1 | grep "test result"
cargo test --all > /tmp/t.log 2>&1; echo "TEST=$?"; grep -E "FAILED" /tmp/t.log | head
```

Expected: each new test `1 passed`; `TEST=0`. **Read the passed-counts.**

- [ ] **Step 7: Confirm the goldens did not move**

```bash
cargo test -p pipeline --test golden 2>&1 | grep "test result"
git status --short crates/pipeline/tests/golden/
```

Expected: `63 passed` (57 references + 6 helper tests), and **no output from `git status`**.

- [ ] **Step 8: Commit**

```bash
git add crates/edit-model/src/lib.rs crates/pipeline/src/lib.rs
git commit -q -F - <<'EOF'
feat(edit-model): a mask carries noise reduction and dehaze

The two fields only. Nothing reads them yet, so this changes no pixel and
all 57 goldens stay byte-identical, checked with git status on the golden
directory rather than assumed.

Placed at the top of the struct because that is where they run: both are
corrections of the capture and belong in the prefix, beside the global
stages of the same name -- not in the tail where MaskStage sits.

build_pipeline_split's exhaustive literal refused to compile until both
were passed neutral. The comment there says WHY it differs from the five
neutral fields beside it: those are placed by the tail further down,
these do not run in the tail at all.
EOF
```

---

## Task 2: `capture_stages` and the prefix insertion

**Files:**
- Modify: `crates/pipeline/src/stages/mask.rs` (`new_capture`, and the two stale comments)
- Modify: `crates/pipeline/src/lib.rs` (`capture_stages`, the projection, the prefix step)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/pipeline/src/lib.rs`:

```rust
/// A mask's capture corrections run in the **prefix**, after the global pair, in mask order.
///
/// ⚠⚠ This is the whole placement decision as a test. In the tail a masked dehaze would
/// re-estimate its airlight from the visible region and change as you panned.
#[test]
fn masked_capture_corrections_run_in_the_prefix_after_the_global_pair() {
    let masked = |amount: f32| edit_model::MaskedAdjustment {
        mask: edit_model::Mask::default(),
        adjustments: edit_model::LocalAdjustments {
            dehaze: edit_model::Dehaze { amount },
            ..Default::default()
        },
    };
    let edits = Edits {
        noise_reduction: edit_model::NoiseReduction {
            luminance: 30.0,
            ..Default::default()
        },
        dehaze: edit_model::Dehaze { amount: 20.0 },
        masks: vec![masked(50.0), masked(70.0)],
        ..Edits::default()
    };
    let (prefix, tail) = build_pipeline_split(&edits, RenderScale::FULL);
    assert_eq!(
        prefix.stage_names(),
        vec!["noise_reduction", "dehaze", "mask", "mask"],
        "the capture masks follow the global pair, one per active mask"
    );
    // ⚠ The tail still gets its own mask stages — a mask carries both halves.
    assert!(
        tail.stage_names().iter().all(|n| *n != "noise_reduction" && *n != "dehaze"),
        "no capture stage may leak into the tail: {:?}",
        tail.stage_names()
    );
}

/// A mask with no capture correction costs nothing in the prefix.
#[test]
fn a_mask_without_capture_corrections_adds_no_prefix_stage() {
    let edits = Edits {
        masks: vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::default(),
            adjustments: edit_model::LocalAdjustments {
                exposure: 0.8,
                ..Default::default()
            },
        }],
        ..Edits::default()
    };
    let (prefix, _) = build_pipeline_split(&edits, RenderScale::FULL);
    assert!(prefix.stage_names().is_empty(), "{:?}", prefix.stage_names());
}

/// Inside one mask, noise reduction precedes dehaze — the global prefix's own order, and for
/// its reason: dehaze is a contrast stretch that amplifies noise.
#[test]
fn a_masks_capture_stages_denoise_before_dehazing() {
    let a = edit_model::LocalAdjustments {
        noise_reduction: edit_model::NoiseReduction {
            luminance: 40.0,
            ..Default::default()
        },
        dehaze: edit_model::Dehaze { amount: 50.0 },
        ..Default::default()
    };
    let names: Vec<&'static str> = capture_stages(&a, RenderScale::FULL)
        .iter()
        .map(|s| s.name())
        .collect();
    assert_eq!(names, vec!["noise_reduction", "dehaze"]);
}

/// ⚠ `capture_stages` and `adjustment_stages` partition a mask's adjustments — no stage may be
/// built by both, or it would run twice.
#[test]
fn the_two_mask_builders_do_not_overlap() {
    let a = all_edits().masks[0].adjustments.clone();
    let cap: Vec<&'static str> = capture_stages(&a, RenderScale::FULL)
        .iter()
        .map(|s| s.name())
        .collect();
    let tail: Vec<&'static str> =
        adjustment_stages(&a, color::wb::Illuminant::default(), RenderScale::FULL)
            .iter()
            .map(|s| s.name())
            .collect();
    for n in &cap {
        assert!(!tail.contains(n), "{n} is built by both builders");
    }
    assert!(!cap.is_empty(), "fixture check: the fixture carries capture corrections");
}
```

- [ ] **Step 2: Run one and watch it fail**

```bash
cargo test -p pipeline masked_capture_corrections_run_in_the_prefix 2>&1 | tail -20
```

Expected: a compile error — `capture_stages` does not exist.

- [ ] **Step 3: Add `capture_stages` beside `adjustment_stages`**

In `crates/pipeline/src/lib.rs`, immediately after `adjustment_stages`:

```rust
/// The **prefix** stages for one set of adjustments: the corrections of the capture.
///
/// ⚠⚠ **Its own builder rather than a branch inside [`adjustment_stages`], because these run in
/// a different half of the pipeline.** Noise reduction and dehaze are placed where the *global*
/// stages of the same name run — after geometry, before every tonal adjustment — so a masked
/// Luminance 50 means what a global Luminance 50 means.
///
/// ⚠⚠ **Dehaze is why the placement is forced rather than preferred.** Its airlight is estimated
/// from the brightest 0.1% of the buffer it is handed. The tail may run on the visible *region*,
/// so a tail-side masked dehaze would re-estimate the airlight as the user panned. The prefix
/// always runs whole-buffer.
///
/// ⚠ Noise reduction before dehaze, mirroring the global prefix for its stated reason: dehaze is
/// a contrast stretch that amplifies noise.
///
/// ⚠ `capture_stages` and [`adjustment_stages`] **partition** a mask's adjustments — a stage in
/// both would run twice. `the_two_mask_builders_do_not_overlap` is the guard.
pub(crate) fn capture_stages(
    a: &edit_model::LocalAdjustments,
    scale: RenderScale,
) -> Vec<Box<dyn PipelineStage>> {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    if a.noise_reduction.is_active() {
        out.push(Box::new(NoiseReductionStage::new(&a.noise_reduction, scale)));
    }
    if a.dehaze.is_active() {
        out.push(Box::new(DehazeStage::new(&a.dehaze, scale)));
    }
    out
}

/// A mask reduced to just its capture corrections — what the prefix stage is built from **and**
/// what its cache key holds, so the two cannot drift.
///
/// ⚠⚠ **The projection is what stops the prefix cache thrashing.** Keyed on the whole
/// `MaskedAdjustment`, every masked-exposure drag would invalidate the checkpoint and re-run a
/// whole-buffer noise reduction — 117 ms at native, on a slider that has nothing to do with it.
pub(crate) fn capture_projection(m: &edit_model::MaskedAdjustment) -> edit_model::MaskedAdjustment {
    edit_model::MaskedAdjustment {
        mask: m.mask.clone(),
        adjustments: edit_model::LocalAdjustments {
            noise_reduction: m.adjustments.noise_reduction,
            dehaze: m.adjustments.dehaze,
            ..Default::default()
        },
    }
}
```

- [ ] **Step 4: Add `MaskStage::new_capture`, and fix the two stale comments**

In `crates/pipeline/src/stages/mask.rs`, replace the stale line in `new_in_region`:

```rust
        // No spatial sub-stage yet: every adjustment a mask can carry is per-pixel. The stroke
        // radius scales through `FrameMap`, not through this.
        let _ = scale;
```

with nothing at all — `scale` is used on the next line by `adjustment_stages`, and the claim has
been false since M7.2 put local contrast and sharpening inside a mask.

Then add the second constructor beside `new_in_region`:

```rust
    /// For the **prefix** half of a mask: its corrections of the capture.
    ///
    /// ⚠⚠ **`region` is `None` and there is no parameter for it**, which is the point rather
    /// than an omission. The prefix always runs whole-buffer, and that is what makes dehaze's
    /// frame-global airlight correct *and* stable as the user pans. A `Region` here would
    /// reintroduce exactly the bug the placement exists to avoid.
    ///
    /// ⚠ Identical to [`Self::new_in_region`] but for the sub-stage builder — same coverage
    /// code, same `CoverageSource` match, so a new `Mask` variant is a compile error in both.
    pub fn new_capture(
        masked: &MaskedAdjustment,
        scale: RenderScale,
        geometry: &Geometry,
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    ) -> Self {
        Self {
            source: Self::coverage_source(masked, geometry, prebuilt),
            stages: crate::capture_stages(&masked.adjustments, scale),
            region: None,
        }
    }
```

and extract the `match &masked.mask { … }` block from `new_in_region` into a shared
`fn coverage_source(masked, geometry, prebuilt) -> CoverageSource`, keeping its ⚠⚠ exhaustiveness
comment verbatim, so both constructors go through one match.

- [ ] **Step 5: Insert the prefix step**

In `build_prefix_steps`, after the global dehaze block:

```rust
    // ⚠ After the global pair, mirroring the tail's own rule that a mask acts on the global look
    // rather than being re-mapped by it.
    //
    // ⚠ One step per mask that actually carries a capture correction; a mask with only tail
    // adjustments costs nothing here.
    for (i, m) in edits.masks.iter().enumerate() {
        let p = capture_projection(m);
        if !p.adjustments.noise_reduction.is_active() && !p.adjustments.dehaze.is_active() {
            continue;
        }
        steps.push(PrefixStep {
            // ⚠⚠ The key holds **the projection the stage was built from**, so key and stage
            // cannot drift — the property this function exists to guarantee.
            key: PrefixStepKey::MaskedCapture(p.clone()),
            stage: Box::new(MaskStage::new_capture(
                &p,
                scale,
                &edits.geometry,
                brush.and_then(|b| b.get(i)),
            )),
        });
    }
```

Add the variant to `PrefixStepKey`:

```rust
    /// One mask's capture corrections. ⚠ Holds the **projection**, not the whole
    /// `MaskedAdjustment` — see `capture_projection` for why.
    MaskedCapture(edit_model::MaskedAdjustment),
```

⚠ `build_prefix_steps` now needs `brush: Option<&BrushCoverage>`. Add it as a parameter, and at
`PrefixKey::new`'s call site pass `None` with a comment mirroring the existing `lens: None` one:

```rust
        // ⚠ `None` here for `brush` for the same reason as `lens` above: a coverage map is
        // needed to RUN the stage, never to KEY it — the key is derived from the edits, and
        // `PrefixStepKey::MaskedCapture` carries them. Construction stays cheap because
        // `CoverageSource::Painted` only clones the stroke list; rasterisation is in `apply`.
        let steps = build_prefix_steps(edits, scale, None, resampler, None);
```

Update `build_pipeline_split_in_region_with_brush` to pass its `brush` through.

- [ ] **Step 6: Run the four tests**

```bash
for t in masked_capture_corrections_run_in_the_prefix \
         a_mask_without_capture_corrections_adds_no_prefix_stage \
         a_masks_capture_stages_denoise_before_dehazing \
         the_two_mask_builders_do_not_overlap; do
  echo -n "$t: "; cargo test -p pipeline "$t" 2>&1 | grep "test result" | head -1
done
```

Expected: `1 passed` on each.

- [ ] **Step 7: Update the prefix tripwire test**

`the_prefix_holds_perspective_spots_geometry_noise_reduction_and_dehaze` asserts the exact prefix
stage list, and `all_edits()` carries a mask. Once that mask gains capture corrections (Task 3) it
will fail. **Update it now** to expect the trailing `"mask"`, and rename it to
`the_prefix_holds_the_capture_stages_and_the_capture_masks`. Keep its tripwire comment.

- [ ] **Step 8: Full suite and goldens**

```bash
cargo test --all > /tmp/t.log 2>&1; echo "TEST=$?"; grep -E "FAILED" /tmp/t.log | head
cargo test -p pipeline --test golden 2>&1 | grep "test result"
git status --short crates/pipeline/tests/golden/
```

Expected: `TEST=0`, `63 passed`, no golden modified.

- [ ] **Step 9: Commit**

```bash
git add crates/pipeline/src/lib.rs crates/pipeline/src/stages/mask.rs
git commit -q -F - <<'EOF'
feat(pipeline): a mask's noise reduction and dehaze run in the prefix

capture_stages beside adjustment_stages: the two builders partition a
mask's adjustments, and a test asserts no stage is built by both.

MaskStage::new_capture takes no region parameter at all, which is the
point rather than an omission -- the prefix always runs whole-buffer, and
that is what makes dehaze's frame-global airlight correct AND stable as
the user pans. A tail-side masked dehaze would re-estimate its airlight
from whatever was on screen.

The coverage_source match is now shared by both constructors, so a new
Mask variant is still a compile error in exactly one place.

Deletes a stale line while it is here: new_in_region claimed "no spatial
sub-stage yet: every adjustment a mask can carry is per-pixel" with a
dead `let _ = scale;` beneath it. Both have been false since M7.2, and
scale is used on the following line.

All 57 goldens byte-identical.
EOF
```

---

## Task 3: The cache key, and the projection that stops it thrashing

**Files:**
- Modify: `crates/pipeline/src/lib.rs` (`all_edits` fixture, projection tests)
- Modify: `crates/edit-model/src/lib.rs` (`all_nonneutral` fixture)

- [ ] **Step 1: Write the failing tests**

```rust
/// ⚠⚠ **Both directions, and the second is the one that matters.** The prefix key must change
/// when a mask's noise reduction changes — and must NOT change when its exposure does, or every
/// masked-exposure drag would re-run a whole-buffer denoise the slider has nothing to do with.
#[test]
fn the_prefix_key_follows_a_masks_capture_corrections_and_ignores_the_rest() {
    let base = Edits {
        masks: vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::default(),
            adjustments: edit_model::LocalAdjustments {
                noise_reduction: edit_model::NoiseReduction {
                    luminance: 30.0,
                    ..Default::default()
                },
                exposure: 0.5,
                ..Default::default()
            },
        }],
        ..Edits::default()
    };
    let key = |e: &Edits| PrefixKey::new(e, RenderScale::FULL, Resampler::default());

    let mut louder = base.clone();
    louder.masks[0].adjustments.noise_reduction.luminance = 80.0;
    assert_ne!(
        key(&base),
        key(&louder),
        "a masked noise-reduction change must invalidate the prefix"
    );

    let mut brighter = base.clone();
    brighter.masks[0].adjustments.exposure = 2.0;
    assert_eq!(
        key(&base),
        key(&brighter),
        "a masked EXPOSURE change must not re-run a whole-buffer denoise"
    );

    let mut hazier = base.clone();
    hazier.masks[0].adjustments.dehaze.amount = 40.0;
    assert_ne!(key(&base), key(&hazier), "masked dehaze must invalidate too");

    let mut moved = base.clone();
    moved.masks[0].mask = edit_model::Mask::Radial(edit_model::RadialGradient::default());
    assert_ne!(
        key(&base),
        key(&moved),
        "the mask's SHAPE decides which pixels are denoised, so it belongs in the key"
    );
}

/// ⚠ The fixtures do not fail to compile when `LocalAdjustments` grows, so this asserts what the
/// type system cannot. Part A's equivalent caught exactly this.
#[test]
fn the_exhaustive_fixture_carries_masked_capture_corrections() {
    let a = &all_edits().masks[0].adjustments;
    assert!(a.noise_reduction.is_active(), "the fixture's mask must denoise");
    assert!(a.dehaze.is_active(), "and must dehaze");
}
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p pipeline the_exhaustive_fixture_carries_masked_capture 2>&1 | grep -E "test result|must denoise"
```

Expected: FAIL — `the fixture's mask must denoise`.

- [ ] **Step 3: Update both fixtures by hand**

⚠⚠ Neither fails to compile; both fill the rest with `..Default::default()`. This is the third
milestone in a row to need this, and both fixtures carry a warning comment saying so.

In `pipeline::all_edits`'s mask:

```rust
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 35.0,
                        color: 25.0,
                        ..Default::default()
                    },
                    dehaze: edit_model::Dehaze { amount: 40.0 },
```

The same two in `edit_model::all_nonneutral`'s mask (without the `edit_model::` prefixes).

- [ ] **Step 4: Run the key tests**

```bash
cargo test -p pipeline the_prefix_key_follows_a_masks_capture 2>&1 | grep "test result"
cargo test -p pipeline the_exhaustive_fixture_carries_masked_capture 2>&1 | grep "test result"
cargo test -p pipeline prefix_key 2>&1 | grep "test result"
cargo test -p edit-model the_exhaustive_fixture 2>&1 | grep "test result"
```

Expected: all pass. ⚠ `prefix_key_ignores_every_downstream_field` is the one `all_edits` exists
for — **read its passed-count**.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/lib.rs crates/edit-model/src/lib.rs
git commit -q -F - <<'EOF'
test(pipeline): the prefix key follows a mask's capture corrections only

The key holds the PROJECTION the stage was built from, not the whole
MaskedAdjustment, and the test pins both directions. The second is the
one that matters: a masked-exposure drag must NOT invalidate the
checkpoint, or every exposure frame would re-run a whole-buffer noise
reduction -- 117 ms at native -- for a slider with nothing to do with it.

The mask's shape is in the key, because it decides which pixels are
denoised.

Both exhaustive fixtures updated by hand. Neither fails to compile when
LocalAdjustments grows; both carry a warning comment saying so, and this
is the third milestone in a row to need it.
EOF
```

---

## Task 4: The master region invariant

**Files:**
- Modify: `crates/app/src/render_engine.rs` (`a_region_render_matches_the_whole_render_cropped`)

⚠⚠ **This is the strongest guard in the milestone and the test that proves the placement.** If the airlight — or anything else here — ever became region-dependent, this fails.

- [ ] **Step 1: Add the capture corrections to its mask**

In that test's `masks[0].adjustments`, beside the existing `local_contrast`:

```rust
                    // ⚠⚠ **The capture corrections belong in this edit set, and their absence
                    // would be silent.** They run in the *prefix*, whole-buffer, and that is the
                    // entire reason for their placement: dehaze estimates its airlight from the
                    // brightest 0.1% of the buffer it is handed, so if it ever ran on the
                    // visible region instead, the same pixel would render differently depending
                    // on where the viewport was — and this comparison is what catches it.
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 40.0,
                        color: 30.0,
                        ..Default::default()
                    },
                    dehaze: edit_model::Dehaze { amount: 45.0 },
```

⚠ **Do not touch the `inset`.** It is derived from `tail_kernel_radius`, and these two contribute
nothing to it because they are not in the tail. If the test now fails at the edges, the placement
is wrong — **stop and re-read the spec** rather than widening the inset.

- [ ] **Step 2: Run it**

```bash
cargo test -p app --lib a_region_render_matches_the_whole_render_cropped 2>&1 | grep -E "test result|assertion"
```

Expected: `1 passed`. ⚠ `render_engine` is a **lib** module — `--lib`, not `--bin Rustroom`, and
**read the passed-count**: `0 passed; N filtered out` is the false-pass trap, not a pass.

- [ ] **Step 3: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -q -F - <<'EOF'
test(app): the region invariant covers masked capture corrections

The master invariant is the test that proves this milestone's placement.
Dehaze estimates its airlight from the brightest 0.1% of the buffer it is
handed; in the tail that buffer is the visible region, so a tail-side
masked dehaze would render the same pixel differently depending on where
the viewport was. Running whole-buffer in the prefix is what prevents
that, and this comparison is what would catch it coming back.

The inset is untouched and must stay so: it derives from
tail_kernel_radius, and neither stage is in the tail.
EOF
```

---

## Task 5: Two goldens

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: `crates/pipeline/tests/golden/mask_nr.png`, `crates/pipeline/tests/golden/mask_dehaze.png`

- [ ] **Step 1: Write the two tests**

⚠ The dehaze golden must use `hazy_chart()`, the fixture the existing dehaze goldens use — the
plain `chart()` has no haze to remove and the reference would be near-neutral. The NR golden needs
`noise_chart()`, for the same reason: no noise, nothing to measure.

```rust
#[test]
fn golden_mask_nr() {
    // Luminance and colour noise reduction confined to the left half, so the reference carries
    // the denoised and the untouched halves of the same noisy chart side by side.
    check_golden_with(
        "mask_nr",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: true,
                }),
                adjustments: edit_model::LocalAdjustments {
                    noise_reduction: edit_model::NoiseReduction {
                        luminance: 60.0,
                        color: 60.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
        noise_chart(),
    );
}

#[test]
fn golden_mask_dehaze() {
    // Haze removed from the left half only. ⚠ The airlight is estimated from the WHOLE chart,
    // which is the placement's whole point — the untouched half still contributes to it.
    check_golden_with(
        "mask_dehaze",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: true,
                }),
                adjustments: edit_model::LocalAdjustments {
                    dehaze: edit_model::Dehaze { amount: 70.0 },
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
        hazy_chart(),
    );
}
```

- [ ] **Step 2: Run and watch them fail**

```bash
cargo test -p pipeline --test golden golden_mask_nr 2>&1 | grep -E "test result|missing golden"
```

Expected: `missing golden mask_nr; run with UPDATE_GOLDEN=1`.

- [ ] **Step 3: Generate**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_mask_nr 2>&1 | grep "test result"
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_mask_dehaze 2>&1 | grep "test result"
```

- [ ] **Step 4: Measure them, and compare against the unmasked reference**

⚠⚠ **A golden nobody has measured is just a hash**, and this project has re-authored three
references found this way. Check each has no meaningful clipping and real discriminating power,
**and that it differs from the corresponding un-masked chart render** — a stage that never ran
would produce a reference identical to it and the test would pass forever against that.

```bash
python3 - <<'PY'
import zlib
from struct import unpack
def read(p):
    d=open(p,'rb').read(); i=8; w=h=None; idat=b''
    while i < len(d):
        ln=unpack('>I',d[i:i+4])[0]; typ=d[i+4:i+8]
        if typ==b'IHDR': w,h=unpack('>II',d[i+8:i+16])
        if typ==b'IDAT': idat+=d[i+8:i+8+ln]
        i+=12+ln
    raw=zlib.decompress(idat); stride=w*3; out=[]; prev=bytearray(stride); pos=0
    for _ in range(h):
        f=raw[pos]; line=bytearray(raw[pos+1:pos+1+stride]); pos+=1+stride
        for x in range(stride):
            a=line[x-3] if x>=3 else 0; b=prev[x]; c=prev[x-3] if x>=3 else 0
            if f==1: line[x]=(line[x]+a)&255
            elif f==2: line[x]=(line[x]+b)&255
            elif f==3: line[x]=(line[x]+(a+b)//2)&255
            elif f==4:
                p2=a+b-c; pa,pb,pc=abs(p2-a),abs(p2-b),abs(p2-c)
                pr=a if (pa<=pb and pa<=pc) else (b if pb<=pc else c)
                line[x]=(line[x]+pr)&255
        out+=list(line); prev=line
    return w,h,out
import sys
for name in ("mask_nr","mask_dehaze"):
    w,h,px=read(f"crates/pipeline/tests/golden/{name}.png")
    pinned=sum(1 for v in px if v==255)
    print(f"{name:12} {w}x{h}  pinned@255 {pinned}/{len(px)} ({100*pinned/len(px):.2f}%)  distinct {len(set(px))}")
PY
```

Expected: pinning low, `distinct` in the low hundreds. ⚠ If either pins heavily, weaken the edit
and regenerate — **never** widen `TOL`.

Then confirm each really changed something:

```bash
cd crates/pipeline/tests/golden
cmp -s mask_nr.png mask_dehaze.png && echo "IDENTICAL - BUG" || echo "differ, good"
cd - >/dev/null
```

- [ ] **Step 5: Full golden suite, and the 57 must be untouched**

```bash
cargo test -p pipeline --test golden 2>&1 | grep "test result"
ls crates/pipeline/tests/golden | wc -l
git status --short crates/pipeline/tests/golden/
```

Expected: `65 passed`, **59** files, and exactly two `??` lines with **no `M` lines**.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/mask_nr.png crates/pipeline/tests/golden/mask_dehaze.png
git commit -q -F - <<'EOF'
test(pipeline): goldens for masked noise reduction and dehaze

57 -> 59 goldens, and the 57 are byte-identical, checked with git status
on the golden directory rather than assumed.

Each uses the chart its global twin uses -- noise_chart and hazy_chart --
because the plain chart has neither noise to remove nor haze to clear,
and the reference would have been near-neutral and unable to discriminate
anything.

Both measured rather than eyeballed, and compared against each other, for
the reason three earlier references had to be re-authored.
EOF
```

---

## Task 6: The panel

**Files:**
- Modify: `crates/app/src/develop/panel/mask_adjust.rs`

⚠ **Mirroring the global panel, not the pipeline** — Dehaze beside Texture and Clarity, noise
reduction beside Sharpening. That contradicts this file's own ordering comment, which must be
rewritten rather than left to look like an oversight.

- [ ] **Step 1: Write the failing carrier test**

Add to `mask_adjust.rs`'s `tests` module, reusing the `Harness` Part A built:

```rust
/// ⚠⚠ **The carrier test.** The pipeline half is proven by the prefix-order tests; nothing there
/// proves the panel's sliders reach `LocalAdjustments`. W5 shipped a milestone whose maths was
/// fully proven and which was 100% non-functional for exactly that reason.
#[test]
fn the_capture_sliders_reach_the_masks_own_fields() {
    for (label, read): (&str, fn(&LocalAdjustments) -> f32) in [
        ("Dehaze", |a| a.dehaze.amount),
        ("Luminance", |a| a.noise_reduction.luminance),
        ("Smoothness", |a| a.noise_reduction.smoothness),
    ] {
        let mut h = Harness::new();
        let before = read(&h.a);
        h.drag_slider(label);
        assert_ne!(
            read(&h.a),
            before,
            "dragging {label} never reached the mask's own field"
        );
    }
}
```

⚠ `Harness` currently only drags the curve. Add a `drag_slider(&mut self, label: &str)` that
locates the slider by label with `test_ui::find` and drags it right — the same five-pass shape
`drag_curve` uses, and **for the same reason**: egui resolves interaction against the widgets
registered on the *previous* pass.

⚠⚠ **"Luminance" and "Smoothness" are chosen deliberately.** "Detail" and "Amount" already appear
in the Sharpening block, so a test naming those could pass against the wrong slider. Check for
collisions before adding any label to this list — `test_ui::find` takes the first match.

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p app --bin Rustroom the_capture_sliders_reach 2>&1 | tail -20
```

Expected: a compile error (`drag_slider` missing), then once that exists, a failure naming the
slider that is not drawn.

- [ ] **Step 3: Add the sliders**

**Dehaze** — immediately after the Clarity slider, matching the global `tone` block:

```rust
    // ⚠⚠ **Panel order deliberately does NOT follow pipeline order here, and this is the one
    // place in this file where that is true.** Dehaze and noise reduction run in the *prefix*,
    // before everything else in this section — but `CLAUDE.md` records the rule for exactly
    // these two stages: *"The pipeline runs NR and dehaze before white balance because that is
    // where the maths wants them; NR is judged last, at 1:1, once tone is settled. Panel follows
    // workflow, pipeline follows maths."* They are placed where the **global panel** puts them,
    // so "one layout learned once" keeps holding.
    ed.slider(ui, &mut a.dehaze.amount, -100.0..=100.0, "Dehaze");
```

**Noise reduction** — its own labelled block immediately *above* the Sharpening block, matching
the global Detail section, which puts NR first for its own recorded reason:

```rust
    ui.separator();
    // ⚠ Above Sharpening, as in the global panel: you must denoise before you can judge
    // sharpening. A deliberate divergence from Lightroom, which puts Sharpening first.
    ui.label("Noise Reduction");
    ed.slider(ui, &mut a.noise_reduction.luminance, 0.0..=100.0, "Luminance");
    ed.slider(ui, &mut a.noise_reduction.detail, 0.0..=100.0, "Detail");
    ed.slider(ui, &mut a.noise_reduction.color, 0.0..=100.0, "Color");
    ed.slider(ui, &mut a.noise_reduction.smoothness, 0.0..=100.0, "Smoothness");
```

Then rewrite the function's top ⚠⚠ doc comment, which currently claims *"what you read top to
bottom is what runs"*, to state the exception and cite the recorded rule.

- [ ] **Step 4: Run the carrier test and the whole app suite**

```bash
cargo test -p app --bin Rustroom the_capture_sliders_reach 2>&1 | grep "test result"
cargo test -p app 2>&1 | grep "test result"
```

⚠ **Read the passed-count**: `mask_adjust` is a **bin** module, so `--bin Rustroom` is right and
`--lib` would report `0 passed; N filtered out` with exit 0.

- [ ] **Step 5: Gates and file size**

```bash
cargo fmt --check > /tmp/f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
wc -l crates/app/src/develop/panel/mask_adjust.rs
```

⚠ Read clippy's exit code **from a file, never through a pipe**. Report the file size — it was 299
lines after Part A.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/develop/panel/mask_adjust.rs
git commit -q -F - <<'EOF'
feat(app): a mask's panel carries noise reduction and dehaze

Placed where the GLOBAL panel puts them -- Dehaze beside Texture and
Clarity, noise reduction above Sharpening -- not where the pipeline runs
them, which is before everything else in the section.

That contradicts this file's own "what you read top to bottom is what
runs", so the comment is rewritten to state the exception and cite the
rule CLAUDE.md already records for exactly these two stages: the pipeline
runs them first because that is where the maths wants them, and NR is
judged last, at 1:1, once tone is settled. Panel follows workflow,
pipeline follows maths -- and "one layout learned once" is what stops the
two panels naming the same controls differently.

The carrier test drags three real sliders and asserts each reaches the
mask's own field. Luminance and Smoothness are named deliberately:
"Detail" and "Amount" also appear in the Sharpening block, so a test
naming those could pass against the wrong slider.
EOF
```

---

## Task 7: The range-mask caveat, pinned

**Files:**
- Modify: `crates/pipeline/src/stages/mask.rs`

- [ ] **Step 1: Write the test**

```rust
/// ⚠⚠ **A range mask selects on PRE-TONAL pixels in its capture half — accepted, and pinned here
/// so it is a documented property rather than something found later and filed as a bug.**
///
/// `MaskStage` computes coverage from the buffer it is applied to. The capture half runs in the
/// prefix, before every tonal adjustment, so a luminance range selects the tones of the
/// *scene* rather than of the rendered photograph. For a correction of the capture that is
/// arguably the more correct answer, and it is what the global stages already do.
#[test]
fn a_luminance_range_selects_pre_tonal_tones_in_the_capture_half() {
    // A ramp whose lower half sits inside the band and whose upper half does not.
    let src = ramp(64, 16);
    let masked = MaskedAdjustment {
        mask: Mask::Luminance(edit_model::LuminanceRange {
            low: 0.0,
            high: 40.0,
            ..Default::default()
        }),
        adjustments: LocalAdjustments {
            dehaze: edit_model::Dehaze { amount: 80.0 },
            ..Default::default()
        },
    };
    let out = MaskStage::new_capture(
        &masked,
        RenderScale::FULL,
        &edit_model::Geometry::default(),
        None,
    )
    .apply(src.clone());

    let moved = |x: usize| (out.pixel(x, 8)[0] - src.pixel(x, 8)[0]).abs() > 1e-4;
    assert!(moved(4), "the dark end is inside the band and must change");
    assert!(!moved(60), "the bright end is outside it and must not");
}
```

⚠ Use whatever `ramp`/`row_lightness` helpers already exist in that module's tests rather than
writing new ones.

- [ ] **Step 2: Run it**

```bash
cargo test -p pipeline a_luminance_range_selects_pre_tonal 2>&1 | grep -E "test result|assertion"
```

Expected: `1 passed`. ⚠ If the assertion is backwards, **check the band's direction against
`LuminanceRange`'s own tests before changing the code** — a test asserting something false about
correct code is a failure mode this project records repeatedly.

- [ ] **Step 3: Commit**

```bash
git add crates/pipeline/src/stages/mask.rs
git commit -q -F - <<'EOF'
test(pipeline): a range mask selects pre-tonal tones in its capture half

Accepted behaviour, pinned so it is a documented property rather than
something found later and filed as a bug.

MaskStage computes coverage from the buffer it is applied to. The capture
half runs in the prefix, before every tonal adjustment, so a luminance
range selects the tones of the SCENE rather than of the rendered
photograph -- while the same mask's tail half selects post-tonal. One
mask, two selections.

For a correction of the capture that is arguably the more correct answer
rather than a compromise, and it is what the global stages already do:
global noise reduction is applied to pre-tonal data too.
EOF
```

---

## Task 8: Measure

**Files:**
- Modify: `crates/app/examples/bench_masked.rs`

- [ ] **Step 1: Add the two adjustment builders and their rows**

```rust
/// A masked denoise. ⚠ Whole-buffer by construction — it runs in the prefix, so a small radial
/// pays for the entire frame. That is the strongest case yet made for the deferred bounding box.
fn denoise() -> LocalAdjustments {
    LocalAdjustments {
        noise_reduction: NoiseReduction {
            luminance: 50.0,
            color: 50.0,
            ..NoiseReduction::default()
        },
        ..Default::default()
    }
}

/// A masked dehaze. ⚠ Its airlight comes from the whole frame, which is why it is in the prefix.
fn haze() -> LocalAdjustments {
    LocalAdjustments {
        dehaze: Dehaze { amount: 60.0 },
        ..Default::default()
    }
}
```

Rows, beside the existing gradient ones:

```rust
        ("1 masked NR       (gradient)", masked(1, gradient, denoise)),
        ("1 masked dehaze   (gradient)", masked(1, gradient, haze)),
        ("1 masked NR       (small radial)", masked(1, radial, denoise)),
```

- [ ] **Step 2: ⚠⚠ Say which cost the bench is reporting**

The `fit` loop calls `render_once`, which builds the prefix **fresh every iteration** — so these
rows report the **cold** cost. The cached cost is what a user feels when dragging an unrelated
slider, and it is ~0. Add that to the file's module doc so the table cannot be misread:

```rust
//! ⚠⚠ **The masked NR and dehaze rows are the COLD cost.** Both run in the prefix, so
//! `render_once` rebuilds them every iteration — but in the app they are *cached*, and a slider
//! that is not theirs re-renders from the checkpoint after them. Read these as "what it costs
//! when you move that slider", not as a per-frame cost.
```

⚠ The **native region** loop times the tail alone on a warm prefix, so these rows will show ~0 for
the capture masks — **which is the correct answer and must be said**, not silently omitted.

- [ ] **Step 3: Build, then run on a real RAW**

```bash
cargo build --release -p app --example bench_masked 2>&1 | grep -E "^error" | head
ls ~/Desktop/fuji/*.RAF | head -1
cargo run --release -p app --example bench_masked ~/Desktop/fuji/DSCF6223.RAF
```

⚠ If no `.RAF` is on this machine, **stop and report it** rather than inventing figures.

Reference points from the same machine: neutral 12.2 ms at fit; masked curve +5.4; masked grade
+15.8; masked clarity +10.1; masked HSL +23.7. Whole-buffer NR 50/50 is 31.4 ms at fit and dehaze
60 is 32.3, so expect the masked ones near those.

- [ ] **Step 4: Commit, with the table pasted in**

No optimisation is in scope. If a number is bad, report it and scope it separately.

---

## Task 9: Sabotage the guards, then run every gate

⚠⚠ **A sabotage that does not fail is a finding about the test.** Confirm each one **compiled** —
a sabotage that fails to build looks exactly like one that passes. Every run must produce a
`test result:` line.

- [ ] **Step 1: Double application**

Pass the real values from the tail's literal: `noise_reduction: edits.noise_reduction, dehaze: edits.dehaze`.

```bash
cargo test -p pipeline the_global_tail_does_not_apply_the_mask_only_stages_twice 2>&1 | grep -E "test result|ran twice"
```

⚠⚠ **Expect this to PASS, and that is the finding.** Part A found this guard names a *list* and
was blind to two new fields; it now names seven and will be blind to these two as well.
**Extend its array to include `noise_reduction` and `dehaze`, and its fixture to set them**, then
re-run and watch it fail. Also check the goldens move:

```bash
cargo test -p pipeline --test golden 2>&1 | grep "test result"
```

Then revert the sabotage, keeping the guard extension.

- [ ] **Step 2: Key on the whole `MaskedAdjustment`**

Replace `PrefixStepKey::MaskedCapture(p.clone())` with `MaskedCapture(m.clone())`.

```bash
cargo test -p pipeline the_prefix_key_follows_a_masks_capture 2>&1 | grep -E "test result|must not re-run"
```

Expected: FAIL on the "masked EXPOSURE change must not re-run" half. Revert.

- [ ] **Step 3: Give the capture stage a region**

Change `MaskStage::new_capture`'s `region: None` to thread a `Some(...)` through from the caller.

```bash
cargo test -p app --lib a_region_render_matches_the_whole_render_cropped 2>&1 | grep -E "test result|assertion"
```

Expected: FAIL. ⚠ If it passes, the region invariant is not seeing the capture stages — **that is
a finding about the test**, and its edit set needs strengthening. Revert.

- [ ] **Step 4: The panel writes nowhere**

Point the Dehaze slider at a scratch local.

```bash
cargo test -p app --bin Rustroom the_capture_sliders_reach 2>&1 | grep -E "test result|never reached"
```

Expected: FAIL. Revert.

- [ ] **Step 5: Every gate, on the reverted tree**

```bash
git status --short
cargo fmt --check > /tmp/fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/test.log 2>&1; echo "TEST=$?"
grep -E "FAILED" /tmp/test.log | head
echo -n "TOTAL TESTS: "; grep -oE "ok\. [0-9]+ passed" /tmp/test.log | grep -oE "[0-9]+" | awk '{s+=$1} END {print s}'
echo -n "GOLDEN TESTS THAT RAN: "; grep -c "^test golden_" /tmp/test.log
```

Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`, and **59** golden tests confirmed to have *run*.

- [ ] **Step 6: Commit the guard extension**

---

## Task 10: Run the GUI and hand over the checklist

⚠ Nothing here can be self-checked: `AXIsProcessTrusted()` is false, so synthetic input is dropped
and `screencapture` fails. Launch it, say what to look at, **wait for the report**.

- [ ] **Step 1: Launch**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Background, always `--release`.

- [ ] **Step 2: Hand over the checklist**

1. **A radial mask on a noisy shadow, Luminance 60.** The masked area smooths; the rest keeps its grain.
2. **The same mask, Color 80.** Chroma speckle goes and the luminance detail stays — the split that makes the two sliders worth having separately.
3. **A graduated filter over a distant ridge, Dehaze +70.** The ridge clears; the foreground does not.
4. ⚠⚠ **With that masked dehaze active, zoom to 1:1 and pan around.** **The masked area must not change as you pan.** This is the whole reason the stage is in the prefix, and it is the one item no test can fully stand in for.
5. ⚠ **Drag a masked *exposure* slider with a masked NR also active.** It must stay responsive — if it stutters, the prefix key is not projecting and every frame is re-running a whole-buffer denoise.
6. **A luminance range mask carrying NR.** ⚠ Expect it to select the *scene's* shadows rather than the rendered ones — the accepted caveat, and worth seeing once so it is a known behaviour.
7. **Global NR and masked NR both on.** The masked area gets both.

- [ ] **Step 3: Wait for the report, then fix what comes back**

---

## Task 11: Update `CLAUDE.md` and `docs/BACKLOG.md`

- [ ] **Step 1: The milestone section**

Above Part A's, following its shape. Cover at minimum:

- **⚠⚠ The recorded blocker was factually wrong** — NR and dehaze run *after* geometry, so the
  coordinate-space objection was never true; it had propagated through four documents.
- **The real blocker was dehaze's airlight under a region render**, and placement answers it.
- **The projection**, and why keying on the whole `MaskedAdjustment` would have been a silent
  performance bug.
- **The double-application guard needed extending again** — Part A's lesson, immediately repeated.
- The measured table, the range-mask caveat, and what the GUI checklist found.

- [ ] **Step 2: Correct the "before geometry" sentence** in `CLAUDE.md`, `docs/BACKLOG.md` and
  Part A's spec. ⚠ Leave M7.2's spec as a historical record but note the correction where it is
  quoted.

- [ ] **Step 3: Close the backlog entry.** Part A and Part B both done → the whole *"rest of the
  stages inside a mask"* entry is closed. ⚠ Record what is **still** outside a mask (lens
  correction, perspective, spots, geometry, vignette, creative look, the output transform) and why,
  so the next reader does not think masks now carry everything.

- [ ] **Step 4: Update the RESUME block** — counts, and a new next action.

- [ ] **Step 5: Commit**

---

## Self-review

**Spec coverage:** §2 model → Task 1. §3 split → Task 2. §4 placement → Task 2, proved by Task 4.
§5 key → Task 3. §6 range masks → Task 7. §7 panel → Task 6. §8 stale things → Task 2 (code) and
Task 11 (docs). §9 testing → Tasks 3–7 and 9. §10 cost → Task 8. §11 non-goals → Task 11.

**Placeholders:** the only intentional gaps are Task 8's measured table and Task 11's counts, which
cannot be known before the runs that produce them; both say to paste real output and to stop rather
than invent.

**Type consistency:** `capture_stages(&LocalAdjustments, RenderScale)` and
`capture_projection(&MaskedAdjustment) -> MaskedAdjustment` are used identically in Tasks 2, 3 and
7. `MaskStage::new_capture(masked, scale, geometry, prebuilt)` matches its call site in
`build_prefix_steps` and in Task 7's test. `PrefixStepKey::MaskedCapture(MaskedAdjustment)` holds
one mask per variant, matching the per-mask loop. Stage name strings are `"noise_reduction"` and
`"dehaze"`, matching the shipped `PipelineStage::name` returns.

**Known risk, flagged rather than hidden:** Task 2 Step 5 adds a parameter to `build_prefix_steps`,
which has several callers. Expect the compiler to name them; **do not** give it a default by
wrapping in a second function — one signature, all callers updated.
