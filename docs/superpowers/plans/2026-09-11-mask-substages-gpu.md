# M8.3j — a mask's sub-stages on the GPU — Implementation Plan

> **⚠ Conventions that are not negotiable (this repo):** commit messages carry **no
> `Co-Authored-By` trailer**; work happens on branch `m8.3j-mask-substages`; **never push** — the
> user pushes to `github.com/wladiKrin/rustroom` himself.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `gpu-pipeline::accelerate` reach the stages inside `MaskStage`, so a mask carrying
HSL (or grading, or sharpening) runs its sub-stage chain on the GPU instead of the CPU.

**Architecture:** `pipeline` gains one defaulted trait method, `PipelineStage::accelerate_inner`,
that offers a stage's inner sub-stage list to a caller-supplied transformer, plus a defaulted
`sub_stage_names` so the result is observable without pixels. `MaskStage` overrides both and hands
out the `LocalAdjustments` its sub-stages were built from. `gpu-pipeline` supplies a closure that
projects `LocalAdjustments → Edits` by exhaustive destructuring, then runs its **existing** run
detection and cost gate over the mask's stage list. Recursion depth is one: the inner call passes
no accelerator.

**Tech Stack:** Rust, `wgpu` (via the existing `gpu-pipeline` crate), `rayon`, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-11-mask-substages-gpu-design.md`

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/pipeline/src/lib.rs` | the `PipelineStage` seam | **Modify** — add `SubStageAccelerator`, `accelerate_inner`, `sub_stage_names` |
| `crates/pipeline/src/stages/mask.rs` | `MaskStage` | **Modify** — retain `adjustments` + `scale`, override both trait methods |
| `crates/gpu-pipeline/src/accelerate.rs` | run detection, cost gate, substitution | **Modify** — `sub_edits` projection, `walk`, the recursion |
| `crates/app/examples/bench_gpu_tail.rs` | the real-engine A/B | **Modify** — add three masked rows |
| `CLAUDE.md` | durable record | **Modify** — final task |

⚠ `crates/pipeline/src/lib.rs` is ~3,058 lines and `crates/gpu-pipeline/src/accelerate.rs` ~900.
Both are over the ~600-line marker and both grow here; neither is split, and that is reported
rather than silently done.

---

## Task 1: The seam in `pipeline`

**Files:**
- Modify: `crates/pipeline/src/lib.rs` (the `PipelineStage` trait, ~line 162)
- Modify: `crates/pipeline/src/stages/mask.rs` (struct, both constructors, the impl)
- Test: `crates/pipeline/src/stages/mask.rs` (its existing `mod tests`)

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block at the bottom of `crates/pipeline/src/stages/mask.rs`, after the
existing `fn masked(..)` helper:

```rust
/// ⚠⚠ **The recursion-depth assumption, as a property rather than a comment.** `accelerate`
/// offers a mask's sub-stages to an accelerator and passes a **no-op** one to the inner call,
/// which is sound only because a mask's sub-stages can never themselves contain a mask.
///
/// That is structural rather than a convention: `adjustment_stages` is exhaustive over
/// `LocalAdjustments`, which has **no `masks` field**, so there is nothing for it to build one
/// from. Asserted here so a future `LocalAdjustments::masks` is caught by a test rather than by
/// an infinite recursion at render time.
#[test]
fn a_masks_sub_stages_never_contain_a_mask() {
    let mut m = masked([0.0, 0.0], [1.0, 1.0]);
    m.adjustments.exposure = 0.5;
    m.adjustments.hsl.bands[0].saturation = 40.0;
    m.adjustments.sharpening = edit_model::Sharpening {
        amount: 60.0,
        ..Default::default()
    };
    let stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
    let names = stage.sub_stage_names();
    assert!(
        !names.is_empty(),
        "fixture check: the mask must carry sub-stages, got {names:?}"
    );
    assert!(
        !names.contains(&"mask"),
        "a mask's sub-stages must never contain a mask, got {names:?}"
    );
}

/// ⚠ The hook must actually replace the list, or every equivalence test in `gpu-pipeline` passes
/// against a no-op. This drives it with a transformer that **empties** the list, which no real
/// accelerator would do — the point is that what it returns is what the stage keeps.
#[test]
fn accelerate_inner_replaces_the_sub_stage_list() {
    let mut m = masked([0.0, 0.0], [1.0, 1.0]);
    m.adjustments.exposure = 0.5;
    let mut stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
    assert_eq!(
        stage.sub_stage_names(),
        vec!["exposure"],
        "fixture check: one sub-stage before the hook"
    );
    // ⚠ The return type is annotated: a bare `Vec::new()` leaves the element type to be inferred
    // from the later unsize coercion to `&dyn Fn`, which does not flow backwards.
    let drop_everything = |_: Vec<Box<dyn PipelineStage>>,
                           _: &edit_model::LocalAdjustments,
                           _: RenderScale|
     -> Vec<Box<dyn PipelineStage>> { Vec::new() };
    stage.accelerate_inner(&drop_everything);
    assert!(
        stage.sub_stage_names().is_empty(),
        "the hook must keep what the accelerator returned, got {:?}",
        stage.sub_stage_names()
    );
}

/// ⚠ The adjustments handed to the accelerator must be the mask's own, never the global `Edits`
/// — that is the whole "plausible photograph" failure this milestone exists to avoid. Captured
/// through the hook so the test sees exactly what `gpu-pipeline` will.
#[test]
fn accelerate_inner_hands_out_the_masks_own_adjustments() {
    let mut m = masked([0.0, 0.0], [1.0, 1.0]);
    m.adjustments.exposure = -1.25;
    let mut stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
    let seen = std::cell::Cell::new(f32::NAN);
    let capture = |s: Vec<Box<dyn PipelineStage>>,
                   a: &edit_model::LocalAdjustments,
                   _: RenderScale| {
        seen.set(a.exposure);
        s
    };
    stage.accelerate_inner(&capture);
    assert_eq!(seen.get(), -1.25);
}
```

⚠ `masked(..)` already exists in that test module and returns a `MaskedAdjustment` with a linear
gradient; `Geometry` and `RenderScale` are already imported there via `use crate::{..}` /
`use edit_model::{..}`. If `Geometry` is not in scope, add it to the existing `use edit_model::{..}`
line rather than writing a second `use`.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline a_masks_sub_stages_never_contain_a_mask accelerate_inner 2>&1 | tail -20
```

Expected: **compile error**, `no method named 'sub_stage_names'` / `no method named
'accelerate_inner'`.

⚠ A compile failure reads exactly like a pass if you only grep for `ok`. **Confirm a
`test result:` line is absent** — that is what distinguishes "did not compile" from "passed".

- [ ] **Step 3: Add the seam to the trait**

In `crates/pipeline/src/lib.rs`, replace the `PipelineStage` trait (currently three lines) with:

```rust
/// One stage of the develop pipeline. Pure: no shared state, independently testable.
/// (A future GPU stage implements the same trait — the seam for the Phase 5 port.)
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, image: ImageBuf) -> ImageBuf;

    /// Offer this stage's inner sub-stages for acceleration. Default: this stage has none.
    ///
    /// ⚠⚠ **`&mut self`, not `self: Box<Self>`.** `gpu_pipeline::accelerate` already owns each
    /// box from [`Pipeline::into_stages`], so the stage can `mem::take` its list, hand it over
    /// and put the result back — no cloning, no reconstruction. A `Box<Self>` receiver is
    /// object-safe but **cannot carry a default body**: returning `self` as
    /// `Box<dyn PipelineStage>` needs an unsize coercion, hence `Self: Sized`, which is exactly
    /// the bound that makes a method undispatchable on `dyn`.
    ///
    /// ⚠ The accelerator is handed the [`edit_model::LocalAdjustments`] the sub-stages were
    /// built from, because a GPU stage is built from *parameters* and the CPU stage cannot be
    /// asked for its own. Reading the **global** `Edits` instead is the failure this signature
    /// exists to prevent, and it would render a plausible photograph rather than an error.
    fn accelerate_inner(&mut self, _f: &SubStageAccelerator<'_>) {}

    /// The names of this stage's inner sub-stages, in run order. Default: none.
    ///
    /// ⚠⚠ **The only way to see what [`PipelineStage::accelerate_inner`] did without comparing
    /// pixels** — and pixels cannot see it, because an accelerated mask and a CPU one render the
    /// same picture by design. Every equivalence test would therefore pass against a hook that
    /// never fired, which is precisely how creative LUTs (W5) shipped fully tested and 100%
    /// non-functional.
    ///
    /// ⚠ M8.3g rejected a trait method for the byte-path question on the grounds that it leaks
    /// an **output-encoding** concern into a seam deliberately free of them. This one reports
    /// **composition**, which the trait already exposes a mutator for one method above; a trait
    /// you can change and cannot observe is worse than one with two views of one fact.
    fn sub_stage_names(&self) -> Vec<&'static str> {
        Vec::new()
    }
}

/// Transform a stage's inner sub-stage list, given the adjustments it was built from.
///
/// ⚠ The alias carries a lifetime because the closure `gpu-pipeline` supplies **borrows**: it
/// holds the outer `ProfileRendering<'_>` and a `&Arc<GpuContext>`. A `'static` alias would
/// force those to be owned, and `ProfileRendering` is a borrow by construction.
pub type SubStageAccelerator<'a> = dyn Fn(
        Vec<Box<dyn PipelineStage>>,
        &edit_model::LocalAdjustments,
        RenderScale,
    ) -> Vec<Box<dyn PipelineStage>>
    + 'a;
```

- [ ] **Step 4: Give `MaskStage` the two fields it needs**

In `crates/pipeline/src/stages/mask.rs`, add to the `MaskStage` struct, after `deposit`:

```rust
    /// The adjustments [`Self::stages`] was built from, kept so
    /// [`PipelineStage::accelerate_inner`] can hand them to an accelerator.
    ///
    /// ⚠⚠ **This field was deliberately dropped once and is deliberately back.** The old comment
    /// said everything needed was extracted at construction — true until a GPU builder needed the
    /// *parameters* a CPU stage does not expose. A `Box<dyn PipelineStage>` reports only `name`
    /// and `apply`, so the only alternative was downcasting every stage.
    adjustments: edit_model::LocalAdjustments,
    /// The rung these sub-stages were built for.
    ///
    /// ⚠ `GpuSharpenStage`'s sigma is in **full-resolution** pixels and only the scale says what
    /// that is here, so an accelerator without it cannot build the sharpen row at all.
    scale: RenderScale,
```

Then delete the now-stale sentence from the struct's leading comment — the block beginning
*"⚠ The `MaskedAdjustment` itself is deliberately not retained"* — and replace it with:

```rust
    // ⚠ The `MaskedAdjustment` is still not retained whole: the coverage source and the sub-stage
    // list are extracted at construction, and only the `LocalAdjustments` the sub-stages were
    // built from is kept beside them (see `adjustments` below, and why).
```

- [ ] **Step 5: Fill the fields in both constructors**

In `MaskStage::new_in_region`, the struct literal becomes:

```rust
        Self {
            composite: Self::composite(masked, geometry, prebuilt),
            stages: adjustment_stages(&masked.adjustments, baseline, scale),
            region,
            deposit,
            adjustments: masked.adjustments.clone(),
            scale,
        }
```

In `MaskStage::new_capture`:

```rust
        Self {
            composite: Self::composite(masked, geometry, prebuilt),
            stages: crate::capture_stages(&masked.adjustments, scale),
            region: None,
            // ⚠ The capture half runs in the **prefix**, which always renders whole-buffer and is
            // cached by `PrefixCache` at a coarser grain. There is nothing for a coverage cache to
            // add here, and a deposit would report a whole-frame map under a region's key.
            deposit: None,
            adjustments: masked.adjustments.clone(),
            scale,
        }
```

⚠ **Both constructors, and the capture one is not an oversight.** `accelerate` is only ever called
on the tail, so the prefix's `MaskStage` never has its hook invoked — and if it ever were, its
sub-stages are `capture_stages` (noise reduction and dehaze), neither of which is in `REPLACEABLE`,
so the list would come back unchanged. **Safe by construction in both directions**, which is better
than safe by one caller remembering.

- [ ] **Step 6: Implement both trait methods on `MaskStage`**

In `crates/pipeline/src/stages/mask.rs`, inside `impl PipelineStage for MaskStage`, after
`fn name`:

```rust
    fn accelerate_inner(&mut self, f: &crate::SubStageAccelerator<'_>) {
        let taken = std::mem::take(&mut self.stages);
        self.stages = f(taken, &self.adjustments, self.scale);
    }

    fn sub_stage_names(&self) -> Vec<&'static str> {
        self.stages.iter().map(|s| s.name()).collect()
    }
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cargo test -p pipeline a_masks_sub_stages_never_contain_a_mask accelerate_inner 2>&1 | tail -20
```

Expected: `test result: ok. 3 passed`.

- [ ] **Step 8: Run the whole pipeline suite plus the goldens**

```bash
cargo test -p pipeline > /tmp/m83j-t1.log 2>&1; echo "EXIT=$?"; grep "test result:" /tmp/m83j-t1.log
grep -c "^test golden_" /tmp/m83j-t1.log
```

Expected: `EXIT=0`, every `test result: ok`, and **63** golden tests having run with none failing.

⚠ Read the exit code **from its own command**, never through a pipe — `cmd | grep` reports grep's
status. This repo has shipped a failing gate that way at least twice.

- [ ] **Step 9: Commit**

```bash
git add crates/pipeline/src/lib.rs crates/pipeline/src/stages/mask.rs
git commit -m "feat: a stage can offer its inner sub-stages for acceleration"
```

---

## Task 2: The projection, made total by the compiler

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs`
- Test: `crates/gpu-pipeline/src/accelerate.rs` (its `mod tests`)

- [ ] **Step 1: Write the failing test**

Add inside `mod tests` in `crates/gpu-pipeline/src/accelerate.rs`:

```rust
/// ⚠⚠ **The projection must carry every field a GPU builder reads, and forgetting one renders a
/// plausible photograph rather than an error.** So this sets all eight to values nothing else in
/// the suite uses and reads each back — a test naming "the projection" while checking only
/// `exposure` would be blind to the other seven.
#[test]
fn the_projection_carries_every_field_a_gpu_builder_reads() {
    let a = edit_model::LocalAdjustments {
        exposure: -1.25,
        highlights: -33.0,
        shadows: 44.0,
        whites: 22.0,
        blacks: -11.0,
        hsl: with_hsl().hsl,
        color_grading: graded(Edits::default()).color_grading,
        sharpening: Sharpening {
            amount: 63.0,
            radius: 1.7,
            detail: 44.0,
            masking: 21.0,
        },
        ..Default::default()
    };
    let e = sub_edits(&a);
    assert_eq!(e.exposure, -1.25);
    assert_eq!(e.highlights, -33.0);
    assert_eq!(e.shadows, 44.0);
    assert_eq!(e.whites, 22.0);
    assert_eq!(e.blacks, -11.0);
    assert_eq!(e.hsl, a.hsl);
    assert_eq!(e.color_grading, a.color_grading);
    assert_eq!(e.sharpening, a.sharpening);
}

/// ⚠ The projection must carry **nothing else**. A field that leaked in would be a global value
/// applied inside a mask — same class of bug, opposite direction.
#[test]
fn the_projection_carries_nothing_a_gpu_builder_does_not_read() {
    let a = edit_model::LocalAdjustments {
        contrast: 40.0,
        saturation: 35.0,
        vibrance: 30.0,
        ..Default::default()
    };
    let e = sub_edits(&a);
    let d = Edits::default();
    assert_eq!(e.contrast, d.contrast);
    assert_eq!(e.saturation, d.saturation);
    assert_eq!(e.vibrance, d.vibrance);
    assert_eq!(e.masks.len(), 0, "a projection must never carry masks");
}
```

⚠ `with_hsl()`, `graded(..)` and `Sharpening` are already in that test module.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p gpu-pipeline --release the_projection_carries 2>&1 | tail -20
```

Expected: **compile error**, `cannot find function 'sub_edits'`. Confirm no `test result:` line.

⚠ `--release` throughout for `gpu-pipeline`: acquiring an adapter costs **44–75 s in debug against
21 ms in release**.

- [ ] **Step 3: Write the projection**

In `crates/gpu-pipeline/src/accelerate.rs`, after the `TRANSFER_COST` constant:

```rust
/// Project a mask's [`LocalAdjustments`] onto the [`Edits`] the `REPLACEABLE` builders read.
///
/// ⚠⚠ **Written as an exhaustive destructuring on purpose: adding a field to `LocalAdjustments`
/// is a COMPILE ERROR here.** The alternative — `a.exposure`, `a.hsl`, … field by field — leaves a
/// new adjustment silently rendering as its default inside every mask, which is a plausible
/// photograph rather than an error. That is `MaskStage::source_for`'s rule (*"exhaustive on
/// purpose — a catch-all here is a trap"*) applied to the projection.
///
/// ⚠ The eight carried fields are exactly the ones a `REPLACEABLE` builder reads. The eight
/// ignored ones have **no GPU stage**: white balance, contrast, the tone curve, local contrast and
/// saturation are M8.3e plan 2's; noise reduction and dehaze run in the prefix and are never in a
/// mask's *tail* sub-stage list at all.
///
/// ⚠ `display_rendering` and `output_transform` are the two builders not built from `Edits`, and
/// neither can occur in a mask's sub-stage list — `adjustment_stages` does not place them.
fn sub_edits(a: &edit_model::LocalAdjustments) -> Edits {
    let edit_model::LocalAdjustments {
        // carried — a GPU builder reads each of these
        exposure,
        highlights,
        shadows,
        whites,
        blacks,
        hsl,
        color_grading,
        sharpening,
        // deliberately not carried — no GPU stage exists for them
        noise_reduction: _,
        dehaze: _,
        white_balance: _,
        contrast: _,
        tone_curve: _,
        saturation: _,
        vibrance: _,
        local_contrast: _,
    } = a;
    Edits {
        exposure: *exposure,
        highlights: *highlights,
        shadows: *shadows,
        whites: *whites,
        blacks: *blacks,
        hsl: *hsl,
        color_grading: *color_grading,
        sharpening: *sharpening,
        ..Edits::default()
    }
}
```

⚠ `Hsl`, `ColorGrading` and `Sharpening` are all `Copy`, so these are dereferences and **not**
`.clone()` — clippy's `clone_on_copy` rejects the latter.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p gpu-pipeline --release the_projection_carries 2>&1 | tail -20
```

Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Verify the exhaustiveness guard actually guards**

Temporarily add a field to `LocalAdjustments` and confirm the projection refuses to compile:

```bash
cp crates/edit-model/src/lib.rs /tmp/m83j-em.bak
python3 - <<'PY'
import re, sys
p = "crates/edit-model/src/lib.rs"
s = open(p).read()
old = "    pub color_grading: ColorGrading,\n}"
assert s.count(old) == 1, f"anchor matched {s.count(old)} times"
s = s.replace(old, "    pub color_grading: ColorGrading,\n    pub sabotage_field: f32,\n}")
open(p, "w").write(s)
print("SABOTAGE APPLIED")
PY
cargo check -p gpu-pipeline 2>&1 | grep -c "pattern does not mention" ; echo "EXIT=$?"
cp /tmp/m83j-em.bak crates/edit-model/src/lib.rs && touch crates/edit-model/src/lib.rs
cargo check -p gpu-pipeline 2>&1 | tail -3
```

Expected: the grep count is **≥ 1** (`pattern does not mention field \`sabotage_field\``), and the
restore leaves `cargo check` clean.

⚠ `touch` after restoring from a backup — `cp` preserves the backup's older mtime and cargo will
reuse the artifact built from the **sabotaged** source. This repo has been misled by that once.

⚠ Assert the edit landed (`SABOTAGE APPLIED` plus the `assert`), because a scripted edit that
matches nothing exits 0 and the check then "passes" meaninglessly.

- [ ] **Step 6: Commit**

```bash
git add crates/gpu-pipeline/src/accelerate.rs
git commit -m "feat: project a mask's adjustments onto the edits the GPU builders read"
```

---

## Task 3: The recursion, depth one

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs`
- Test: `crates/gpu-pipeline/src/accelerate.rs` (its `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests`:

```rust
/// A mask carrying HSL, for the tests below. One band is enough to make `adjustment_stages`
/// place `HslStage`, and `hsl` costs a measured 60.0 against a 27.0 transfer.
fn mask_carrying(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
    edit_model::MaskedAdjustment {
        mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
        refine: Vec::new(),
        adjustments: a,
    }
}

/// The sub-stage names of the first `mask` stage in a pipeline.
fn mask_sub_stages(p: &Pipeline) -> Vec<&'static str> {
    p.sub_stage_names_of("mask")
        .expect("fixture check: the pipeline must contain a mask stage")
}

/// ⚠⚠ **The carrier: a mask's sub-stages really do become a chain.** Every equivalence test in
/// this file compares an accelerated render against a CPU one, and a hook that never fired makes
/// the two trivially identical — so something has to assert the chain was **built**. That is the
/// W5 failure mode (creative LUTs shipped fully tested and 100% dead) as a test.
#[test]
fn a_masked_hsl_becomes_a_chain_inside_the_mask() {
    let Some(ctx) = ctx() else { return };
    let mut e = Edits::default();
    e.masks = vec![mask_carrying(edit_model::LocalAdjustments {
        hsl: with_hsl().hsl,
        ..Default::default()
    })];
    let before = mask_sub_stages(&tail_of(&e));
    assert_eq!(
        before,
        vec!["hsl"],
        "fixture check: the mask must carry exactly hsl before acceleration"
    );
    let after = mask_sub_stages(&accel(
        tail_of(&e),
        &e,
        SCALE,
        ProfileRendering::default(),
        RenderSpec::default(),
        &ctx,
    ));
    assert_eq!(
        after,
        vec!["gpu_chain"],
        "the mask's hsl must have become a chain"
    );
}

/// ⚠⚠ **The gate reaches the right verdict INSIDE a mask, and this is the case the catalogue
/// actually has.** Its one masked photograph carries `exposure: -3.6` and nothing else — 1.3 ms
/// of work against a 27.0 ms transfer — so it must be **refused**. A milestone that accelerated it
/// would make that photograph slower.
#[test]
fn a_mask_carrying_only_exposure_is_not_worth_a_chain() {
    let Some(ctx) = ctx() else { return };
    let mut e = Edits::default();
    e.masks = vec![mask_carrying(edit_model::LocalAdjustments {
        exposure: -3.6,
        ..Default::default()
    })];
    let after = mask_sub_stages(&accel(
        tail_of(&e),
        &e,
        SCALE,
        ProfileRendering::default(),
        RenderSpec::default(),
        &ctx,
    ));
    assert_eq!(
        after,
        vec!["exposure"],
        "exposure alone must stay on the CPU inside a mask"
    );
}

/// ⚠⚠ **Accelerating the tail must not reach into the PREFIX**, or `PrefixKey` would describe two
/// different computations under one key. The mask below carries both halves — noise reduction runs
/// in the prefix, HSL in the tail — so one `Edits` exercises both, and the assertions are the two
/// directions of one property.
#[test]
fn accelerating_the_tail_leaves_the_prefixs_mask_alone() {
    let Some(ctx) = ctx() else { return };
    let mut e = Edits::default();
    e.masks = vec![mask_carrying(edit_model::LocalAdjustments {
        hsl: with_hsl().hsl,
        noise_reduction: edit_model::NoiseReduction {
            luminance: 40.0,
            color: 40.0,
            ..Default::default()
        },
        ..Default::default()
    })];
    let (prefix, tail) = pipeline::build_pipeline_split(&e, SCALE);
    assert!(
        prefix.sub_stage_names_of("mask").is_some(),
        "fixture check: the prefix must carry a capture mask"
    );
    let accelerated = accel(
        tail,
        &e,
        SCALE,
        ProfileRendering::default(),
        RenderSpec::default(),
        &ctx,
    );
    assert_eq!(
        mask_sub_stages(&accelerated),
        vec!["gpu_chain"],
        "the tail's mask must be accelerated"
    );
    assert!(
        !prefix
            .sub_stage_names_of("mask")
            .unwrap()
            .contains(&"gpu_chain"),
        "the prefix's mask must be untouched"
    );
}
```

- [ ] **Step 2: Add the `Pipeline` accessor the tests need**

In `crates/pipeline/src/lib.rs`, in `impl Pipeline`, after `stage_names`:

```rust
    /// The sub-stage names of the first stage with this name, if it has any.
    ///
    /// ⚠ `stage_names` reports the top level only, and a `Box<dyn PipelineStage>` cannot be
    /// downcast — so without this a test can see *that* a pipeline contains a mask and nothing
    /// about what the mask contains. `None` means no stage of that name; an empty `Vec` means one
    /// with no sub-stages.
    pub fn sub_stage_names_of(&self, name: &str) -> Option<Vec<&'static str>> {
        self.stages
            .iter()
            .find(|s| s.name() == name)
            .map(|s| s.sub_stage_names())
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p gpu-pipeline --release a_masked_hsl_becomes_a_chain a_mask_carrying_only_exposure accelerating_the_tail_leaves 2>&1 | tail -30
```

Expected: `3 failed`, each reporting `left: ["hsl"], right: ["gpu_chain"]` or similar — the hook is
never invoked yet, so nothing inside a mask changes.

⚠ If instead there is **no `test result:` line**, it did not compile; fix that before reading
anything into the outcome.

- [ ] **Step 4: Factor the walk and add the recursion**

In `crates/gpu-pipeline/src/accelerate.rs`, replace the body of `pub fn accelerate` with a thin
wrapper and move the loop into a private `walk`. The public signature is **unchanged**.

```rust
pub fn accelerate(
    tail: Pipeline,
    edits: &Edits,
    scale: RenderScale,
    profile: ProfileRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
) -> (Pipeline, Option<GpuChain>) {
    // ⚠⚠ **Depth one, and it is structural rather than a bolted-on limit.** The accelerator handed
    // to the outer walk recurses into a mask; the walk it starts is handed **no** accelerator. That
    // is sound because `adjustment_stages` is exhaustive over `LocalAdjustments`, which has no
    // `masks` field — a mask's sub-stages can never contain a mask. Pinned by
    // `a_masks_sub_stages_never_contain_a_mask` in `pipeline`.
    let inner = |stages: Vec<Box<dyn PipelineStage>>,
                 a: &edit_model::LocalAdjustments,
                 sub_scale: RenderScale| {
        let e = sub_edits(a);
        // ⚠ `rejoin`, because a mask wants one plain list. The inner walk can never return a
        // trailing chain anyway: `output_transform` is not something `adjustment_stages` places,
        // so the byte path is unreachable here **by construction** and a mask always pays the
        // float round trip that `TRANSFER_COST` describes.
        let (p, c) = walk(Pipeline::new(stages), &e, sub_scale, profile, spec, ctx, None);
        rejoin(p, c).into_stages()
    };
    // ⚠ The cast is explicit: an unsize coercion from `&{closure}` to `&dyn Fn` inside a `Some(..)`
    // is not guaranteed to be inferred through the wrapper.
    walk(
        tail,
        edits,
        scale,
        profile,
        spec,
        ctx,
        Some(&inner as &pipeline::SubStageAccelerator<'_>),
    )
}

/// One pass of run detection over a stage list.
///
/// ⚠ `inner` is `None` for the recursive call and `Some` for the outer one — see `accelerate`.
#[allow(clippy::too_many_arguments)]
fn walk(
    tail: Pipeline,
    edits: &Edits,
    scale: RenderScale,
    profile: ProfileRendering<'_>,
    spec: RenderSpec,
    ctx: &Arc<GpuContext>,
    inner: Option<&pipeline::SubStageAccelerator<'_>>,
) -> (Pipeline, Option<GpuChain>) {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    // ⚠⚠ **Both halves.** A run that turns out not to be worth the transfer has to be put back,
    // and `GpuStage::cpu()` hands out a *borrow* that cannot be pushed into `out` — so the
    // original travels beside the GPU stage and `flush` chooses between them.
    let mut run: Vec<Candidate> = Vec::new();
    for mut s in tail.into_stages() {
        // ⚠ Offered to **every** stage the walk sees, before the lookup. A replaceable stage has
        // no inner list and inherits the empty default, so one rule covers both arms rather than
        // one arm carrying an exception nobody would think to check.
        if let Some(f) = inner {
            s.accelerate_inner(f);
        }
        match REPLACEABLE.iter().find(|(n, _)| *n == s.name()) {
            Some((_, build)) => run.push((build(edits, scale, profile, spec, ctx), s)),
            None => {
                // ⚠ A chain followed by a CPU stage is **not** the tail's last stage, so it goes
                // straight back into the pipeline and can never be a candidate for the byte path.
                if let Some(c) = flush(&mut run, &mut out, ctx) {
                    out.push(Box::new(c));
                }
                out.push(s);
            }
        }
    }
    // ⚠⚠ Whatever this produces **is** last, by construction — the loop is over, so there is
    // nothing left to push after it.
    match flush(&mut run, &mut out, ctx) {
        // ⚠ The name is **checked**, not assumed.
        Some(c) if c.last_name() == Some("output_transform") => (Pipeline::new(out), Some(c)),
        Some(c) => {
            out.push(Box::new(c));
            (Pipeline::new(out), None)
        }
        None => (Pipeline::new(out), None),
    }
}
```

⚠ The `#[allow(clippy::too_many_arguments)]` goes **immediately above `fn walk`** and nowhere else.
This repo has already shipped an insertion that separated that attribute from the function it was
written for and silently reattached it to a neighbouring item — caught only by clippy.

⚠ Keep `accelerate`'s existing doc comment on the public function; only the body moves. Add to it:

```rust
/// ⚠ Since M8.3j it also recurses **one level** into a stage that carries sub-stages — in
/// practice `MaskStage`, which the tail sees as one opaque stage named `"mask"`. That is what
/// makes masked HSL reachable at all.
```

and **delete** the now-false line from that doc comment:

```rust
/// ⚠ Only the **global** tail. A mask's own stages are inside `MaskStage`, so masked HSL is still
/// not accelerated — the case reported in use as slow, and M8.3d's.
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p gpu-pipeline --release a_masked_hsl_becomes_a_chain a_mask_carrying_only_exposure accelerating_the_tail_leaves 2>&1 | tail -20
```

Expected: `test result: ok. 3 passed`.

⚠ If the run reports `0 passed; N filtered out`, cargo took the names as **substrings** and matched
nothing — read the passed-count, never the word `ok`.

- [ ] **Step 6: Verify the recursion is what makes them pass**

```bash
cp crates/gpu-pipeline/src/accelerate.rs /tmp/m83j-acc.bak
python3 - <<'PY'
p = "crates/gpu-pipeline/src/accelerate.rs"
s = open(p).read()
old = "        if let Some(f) = inner {\n            s.accelerate_inner(f);\n        }\n"
assert s.count(old) == 1, f"anchor matched {s.count(old)} times"
open(p, "w").write(s.replace(old, ""))
print("SABOTAGE APPLIED")
PY
cargo test -p gpu-pipeline --release a_masked_hsl_becomes_a_chain accelerating_the_tail_leaves 2>&1 | grep "test result:"
cp /tmp/m83j-acc.bak crates/gpu-pipeline/src/accelerate.rs && touch crates/gpu-pipeline/src/accelerate.rs
```

Expected: **2 failed** under the sabotage, and a `test result:` line is present (so it compiled).
`a_mask_carrying_only_exposure_is_not_worth_a_chain` is expected to **pass** under it — recorded as
inert for this sabotage rather than counted, because with the hook removed the mask keeps
`["exposure"]` for the trivial reason that nothing ran.

- [ ] **Step 7: Commit**

```bash
git add crates/gpu-pipeline/src/accelerate.rs crates/pipeline/src/lib.rs
git commit -m "feat: accelerate recurses one level into a mask's sub-stages"
```

---

## Task 4: The equivalence test — the guard that can see a wrong projection

**Files:**
- Test: `crates/gpu-pipeline/src/accelerate.rs` (its `mod tests`)

- [ ] **Step 1: Write the test**

Add inside `mod tests`:

```rust
/// ⚠⚠ **The guard on the projection, and the only thing that can see a mis-mapped field.** Every
/// one of the eight projected fields is non-default here, so a projection that forgot `whites` (or
/// carried the *global* value) renders a different photograph. A fixture setting only `hsl` would
/// be blind to seven of the eight.
///
/// ⚠ The fixture check matters as much as the assertion: an accelerated mask and a CPU one agree
/// trivially if the mask covers nothing, so the masked render must first be shown to differ from
/// the unmasked one.
#[test]
fn a_mask_carrying_every_ported_stage_renders_the_same_picture() {
    let Some(ctx) = ctx() else { return };
    let adjustments = edit_model::LocalAdjustments {
        exposure: 0.6,
        highlights: -30.0,
        shadows: 35.0,
        whites: 18.0,
        blacks: -14.0,
        hsl: with_hsl().hsl,
        color_grading: graded(Edits::default()).color_grading,
        sharpening: Sharpening {
            amount: 55.0,
            radius: 1.2,
            detail: 50.0,
            masking: 0.0,
        },
        ..Default::default()
    };
    let mut e = Edits::default();
    e.masks = vec![mask_carrying(adjustments)];

    let unmasked = tail_of(&Edits::default()).run(scene_ramp());
    let cpu = tail_of(&e).run(scene_ramp());
    assert!(
        !crate::compare(&unmasked, &cpu).is_exact(),
        "fixture check: the mask must actually change the picture"
    );

    let gpu = accel(
        tail_of(&e),
        &e,
        SCALE,
        ProfileRendering::default(),
        RenderSpec::default(),
        &ctx,
    )
    .run(scene_ramp());
    let d = crate::compare(&cpu, &gpu);
    assert!(
        d.worst_levels < 0.02,
        "worst {} levels over {} of {} samples",
        d.worst_levels,
        d.differing,
        d.total
    );
}
```

- [ ] **Step 2: Run it and record what it measures**

```bash
cargo test -p gpu-pipeline --release a_mask_carrying_every_ported_stage 2>&1 | tail -20
```

Expected: `test result: ok. 1 passed`.

⚠ **If it fails on the bound rather than on the fixture check, do NOT widen the number and move
on.** Read the reported `worst` value and decide deliberately, following `a_three_stage_run_renders_the_same_picture`'s
precedent: that test carries **0.02** because three transcendental stages each under 0.01 measurably
compose to 0.01153, and its doc comment says that a further widening would be *"a finding to
investigate rather than a number to adjust"*. This chain is up to five stages, and the mask blend
multiplies every difference by a coverage of at most 1, so it can only **shrink** them. If the
measured value exceeds 0.02, write the measured figure and that reasoning into the doc comment,
set the bound just above it, and record it as a finding for `CLAUDE.md`.

- [ ] **Step 3: Verify a wrong projection fails it**

```bash
cp crates/gpu-pipeline/src/accelerate.rs /tmp/m83j-acc2.bak
python3 - <<'PY'
p = "crates/gpu-pipeline/src/accelerate.rs"
s = open(p).read()
old = "        whites: *whites,\n"
assert s.count(old) == 1, f"anchor matched {s.count(old)} times"
open(p, "w").write(s.replace(old, ""))
print("SABOTAGE APPLIED")
PY
cargo test -p gpu-pipeline --release a_mask_carrying_every_ported_stage 2>&1 | grep "test result:"
cp /tmp/m83j-acc2.bak crates/gpu-pipeline/src/accelerate.rs && touch crates/gpu-pipeline/src/accelerate.rs
cargo test -p gpu-pipeline --release a_mask_carrying_every_ported_stage 2>&1 | grep "test result:"
```

Expected: **1 failed** under the sabotage (`whites` dropped means `GpuToneRegionsStage` renders
with a different white point), then **1 passed** after the restore.

⚠ If the sabotage **passes**, that is a finding about the fixture, not a formality — the ramp may
not reach the tones `whites` moves. Widen `scene_ramp`'s range in the test, or pick a different
projected field to drop, and record which one discriminates.

- [ ] **Step 4: Commit**

```bash
git add crates/gpu-pipeline/src/accelerate.rs
git commit -m "test: a mask carrying every ported stage renders the same picture on the GPU"
```

---

## Task 5: The gate

**Files:** none — verification only.

- [ ] **Step 1: Format**

```bash
cargo fmt --all
git diff --stat
```

⚠ If `cargo fmt` reflowed anything, re-read any anchor a later step depends on. This repo has had
two scripted edits match nothing because `fmt` had reflowed their target between reading and
patching.

- [ ] **Step 2: Run the three gates, each exit code read from its own file**

```bash
cargo fmt --all --check > /tmp/m83j-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83j-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/m83j-test.log 2>&1; echo "TEST=$?"
```

Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`.

⚠ **Read the clippy exit code before believing anything.** This file records at least seventeen
occasions on which clippy caught what a fully green test suite did not, and several commits that
went in with clippy failing because the gate was sequenced with `;` or piped into `grep`.

- [ ] **Step 3: Confirm the goldens ran and none moved**

```bash
grep -c "^test golden_" /tmp/m83j-test.log
grep -c "GPU adapter" /tmp/m83j-test.log
grep "test result:" /tmp/m83j-test.log | grep -v "0 failed" || echo "no failures"
```

Expected: **63** goldens, **0** GPU skips, `no failures`.

⚠ The goldens are a guard on the **CPU** path only — `grep -rln gpu tests` returns nothing — so 63
unmoved goldens prove the defaulted no-op and the two re-added `MaskStage` fields changed nothing.
**They are blind to every line this milestone adds.** Say that when reporting, rather than letting
63 green files read as coverage of the new path.

- [ ] **Step 4: Confirm no `⚠` finding was lost**

```bash
git stash -q
grep -rh "⚠" crates/ | sort > /tmp/m83j-warn-before.txt
git stash pop -q
grep -rh "⚠" crates/ | sort > /tmp/m83j-warn-after.txt
comm -23 /tmp/m83j-warn-before.txt /tmp/m83j-warn-after.txt
```

Expected: only the two lines this plan deliberately deletes — the *"deliberately not retained"*
sentence in `mask.rs` and the *"Only the global tail"* line in `accelerate`'s doc comment. Anything
else was dropped by accident; restore it.

⚠ Run this **before** committing. W0's own rule, and M8.3i nearly lost six findings to a rewrite
without it.

- [ ] **Step 5: Commit any formatting**

```bash
git add -A crates/
git status --short
git diff --cached --stat
git commit -m "style: cargo fmt" || echo "nothing to commit"
```

⚠ Read `git diff --cached --stat` **before** committing, not after. A scripted edit in M8.3g spliced
by a computed index, deleted ten tests, and `git add -A` committed it.

---

## Task 6: Measure it

**Files:**
- Modify: `crates/app/examples/bench_gpu_tail.rs`

- [ ] **Step 1: Add the masked rows**

In `crates/app/examples/bench_gpu_tail.rs`, above the `cases` array, add:

```rust
    /// A mask carrying only what it is named for, under a full-frame linear gradient.
    fn masked(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
        edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                from: [0.5, 0.0],
                to: [0.5, 1.0],
                invert: false,
            }),
            refine: Vec::new(),
            adjustments: a,
        }
    }

    // ⚠⚠ **The case M8.3j exists for.** Masked HSL is the one reported in use as slow, and
    // M8.3i's decomposition put its sub-stages at 68.8 ms of an 82 ms mask.
    let masked_hsl = Edits {
        masks: vec![masked(edit_model::LocalAdjustments {
            hsl: hsl(),
            ..Default::default()
        })],
        ..Edits::default()
    };
    let masked_hsl_x3 = Edits {
        masks: (0..3)
            .map(|_| {
                masked(edit_model::LocalAdjustments {
                    hsl: hsl(),
                    ..Default::default()
                })
            })
            .collect(),
        ..Edits::default()
    };
    // ⚠⚠ **The control, and the honest headline: this is what the catalogue's ONE masked
    // photograph carries.** 1.3 ms of sub-stage work against a 27.0 ms transfer, so the cost gate
    // must refuse it and this row must not move. A row that got faster here would mean the gate
    // was reaching the wrong verdict inside a mask.
    let masked_exposure = Edits {
        masks: vec![masked(edit_model::LocalAdjustments {
            exposure: -3.6,
            ..Default::default()
        })],
        ..Edits::default()
    };
```

Then change the `cases` array from `[(&str, &Edits); 5]` to:

```rust
    let cases: [(&str, &Edits); 8] = [
        ("TYPICAL of this catalogue", &typical),
        ("unedited, camera rendering", &neutral_rendered),
        ("unedited, colorimetric", &Edits::default()),
        ("hsl only", &hsl_only),
        ("hsl + grade + sharpening", &three),
        ("1 masked HSL (gradient)", &masked_hsl),
        ("3 masked HSL (gradient)", &masked_hsl_x3),
        ("1 masked exposure — the control", &masked_exposure),
    ];
```

- [ ] **Step 2: Check it compiles and is clean**

```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83j-clippy2.log 2>&1; echo "CLIPPY=$?"
```

Expected: `CLIPPY=0`.

- [ ] **Step 3: Run the bench on a real RAW**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app --example bench_gpu_tail ~/Pictures/fuji/DSCF6291.RAF
```

⚠ `RAW_FIXTURE` moved to `~/Pictures/fuji` on 2026-09-10; if that file is absent, use any `.RAF`
there. ⚠ Quote the results as a **band across runs**, not a single figure — M8.3h had to retract a
headline for quoting the top of a band as though it were the value. ⚠ Note how idle the machine
was; M8.3f and M8.3g both carry that caveat because the CPU column is biased by contention.

Expected, at rung 3200: the two masked-HSL rows show a speed-up, and **the masked-exposure control
does not move beyond the ±4 ms noise floor**.

- [ ] **Step 4: Price the mask's own phases**

```bash
RUSTROOM_DEBUG_MASK=1 cargo run --release -p app --example mask_breakdown ~/Pictures/fuji/DSCF6291.RAF 2>&1 | tail -30
```

⚠ This runs through `render_once`, which does **not** call `accelerate`, so it reports the **CPU**
decomposition. That is what it is for here: it re-confirms the 68.8 ms sub-stage figure this
milestone is measured against, on this machine and this day. The GPU number comes from Step 3.

- [ ] **Step 5: Commit**

```bash
git add crates/app/examples/bench_gpu_tail.rs
git commit -m "bench: the masked rows, and the control the cost gate must refuse"
```

---

## Task 7: Record it

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Launch the GUI for the confirmation-by-use check**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Say what to look at:
- **Settings → `gpu_tail` ON** (it is off by default, so a fresh install sees nothing).
- Open a real `.RAF`, add a **mask carrying HSL**, and drag one of its HSL bands.
- **The picture must not change** when the toggle is flipped — that is the one case a wrong
  projection would show, and it would show as a *plausible* photograph, so compare deliberately
  rather than looking for something obviously wrong.
- Then drag a **global** slider on the same photo and watch it stay responsive.
- ⚠ Also worth exercising: a mask carrying **only exposure**, which must be neither faster nor
  slower — the control.

- [ ] **Step 2: Write the section into `CLAUDE.md`**

Insert a new section immediately **above** `**M8.3i — the parallel brush rasterizer (2026-09-11).**`
titled `**M8.3j — a mask's sub-stages on the GPU (2026-09-11).**`, carrying:

- the test count before → after, the golden count and that **none moved**;
- the spec and plan filenames;
- ⚠⚠ **that the 63 goldens are blind to this milestone's new path**, and what covers it instead;
- the measured bench table from Task 6 Step 3, quoted as a band;
- ⚠⚠ the honest headline: **no gain on the catalogue's one masked photograph**, by design;
- the seam decision (`&mut self` over `self: Box<Self>`, and why a default body forces it);
- the `sub_stage_names` decision and its contrast with M8.3g's rejected trait method;
- the exhaustive-destructuring projection and its compile-error guard;
- the depth-one argument and why it is structural;
- every sabotage that fired, **and the one recorded as inert** (Task 3 Step 6);
- the accepted caveats from the spec's §9.

- [ ] **Step 3: Update the RESUME block's next action**

In the `▶ NEXT ACTION` block, mark M8.3j done and name the next candidates from the list already
there: **M8.3e plan 2** (white balance, contrast, tone curve, saturation — now worth more, since
each is also a mask sub-stage and would raise this milestone above 5 of 10), the **range-mask
coverage cache**, **local contrast**, the **vignette**, the **creative LUT**, `wgpu::PipelineCache`,
and **M8.1**'s leftovers. ⚠ Note that **M7.3b is DEFERRED, not done.**

⚠ Also correct, in place, the M8.3i section's caveat (1), which currently reads *"nothing for
masked HSL's 68.8 ms of sub-stages — that is M8.3j"*.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record M8.3j, a mask's sub-stages on the GPU"
```

- [ ] **Step 5: Report the state, and stop**

Report: the branch name, the test count, the golden count, the three gate exit codes, the bench
band, and whether it has been confirmed by use.

⚠ **Do not merge and do not push.** The user merges (fast-forward, his call) and pushes himself.
