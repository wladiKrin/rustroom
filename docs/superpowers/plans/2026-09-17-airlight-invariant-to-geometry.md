# The airlight becomes invariant to geometry: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The dehaze airlight stops moving when the frame is reframed, on both the global and the masked path.

**Architecture:** Two changes. `prefix_airlight` skips the three resampling keys while replaying; `capture_stages` gains an airlight and is handed the same value already in scope in `build_prefix_steps`.

**Spec:** `docs/superpowers/specs/2026-09-17-airlight-invariant-to-geometry.md`

⚠ Prefix cargo with `PKG_CONFIG_PATH`. ⚠⚠ Never run two gates concurrently.

---

### Task 1: The replay skips the resampling family

**Files:** Modify `crates/pipeline/src/lib.rs` (`prefix_airlight`, ~line 761)

- [ ] **Step 1: Add the skip**

Replace:

```rust
    let mut cur = small;
    for s in &steps[..upto] {
        cur = s.stage.apply(cur);
    }
```

with:

```rust
    let mut cur = small;
    for s in &steps[..upto] {
        // ⚠⚠ **The resampling family is SKIPPED, and that is the whole milestone.** The airlight is
        // an order statistic over the frame, so replaying geometry made it a function of the
        // ANGLE — measured at 6.06-6.81 levels across an ordinary Straighten drag on DSCF6291.
        // `docs/BACKLOG.md` said M8.7's replay "does not move during a drag at all"; that
        // conflates DETERMINISTIC with INVARIANT, since a drag changes an edit every frame.
        //
        // ⚠ Spot removal and noise reduction stay: neither reframes, and both are part of what
        // dehaze actually sees.
        //
        // ⚠⚠ The consequence, stated rather than discovered: **a crop no longer re-estimates.** A
        // crop into a hazy sky and one into a clear foreground share a value. That IS invariance —
        // and a crop drag is continuous, so re-estimating there would reintroduce the defect in the
        // gesture most likely to show it.
        if matches!(
            s.key,
            PrefixStepKey::LensCorrection(..)
                | PrefixStepKey::Perspective(..)
                | PrefixStepKey::Geometry(..)
        ) {
            continue;
        }
        cur = s.stage.apply(cur);
    }
```

- [ ] **Step 2: Build**

`cargo build --release -q -p pipeline` — expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "fix(pipeline): the airlight replay skips the resampling family"
```

---

### Task 2: The masked path is supplied the same value

**Files:** `crates/pipeline/src/lib.rs` (`capture_stages`, `build_prefix_steps`), `crates/pipeline/src/stages/mask.rs` (`new_capture`)

- [ ] **Step 1: `capture_stages` takes an airlight**

```rust
fn capture_stages(
    a: &edit_model::LocalAdjustments,
    scale: RenderScale,
    airlight: Option<[f32; 3]>,
) -> Vec<Box<dyn PipelineStage>> {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    if a.noise_reduction.is_active() {
        out.push(Box::new(NoiseReductionStage::new(
            &a.noise_reduction,
            scale,
        )));
    }
    if a.dehaze.is_active() {
        // ⚠⚠ **The same frame-global value the global dehaze gets.** Without one this called
        // `DehazeStage::new`, which estimates from the stage's own input on every frame — measured
        // at 9.19-11.13 levels across a Straighten drag, WORSE than the global path it sits beside.
        out.push(match airlight {
            Some(air) => Box::new(DehazeStage::with_airlight(&a.dehaze, scale, air)),
            None => Box::new(DehazeStage::new(&a.dehaze, scale)),
        });
    }
    out
}
```

- [ ] **Step 2: `new_capture` threads it**

In `mask.rs`, add `airlight: Option<[f32; 3]>` as the last parameter of `new_capture` and pass it:

```rust
            stages: crate::capture_stages(&masked.adjustments, scale, airlight),
```

- [ ] **Step 3: `build_prefix_steps` passes what it already holds**

```rust
            stage: Box::new(MaskStage::new_capture(&p, scale, &edits.geometry, &[], airlight)),
```

- [ ] **Step 4: Fix the two test call sites** of `capture_stages` (lib.rs ~3876, ~3888) by passing `None`, and any `new_capture` call sites the compiler names.

- [ ] **Step 5: Build and test**

`cargo test --release -q -p pipeline 2>&1 | grep -E 'test result|error\['`

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "fix(pipeline): a mask's dehaze is given the frame-global airlight"
```

---

### Task 3: The invariance test

**Files:** `crates/pipeline/src/lib.rs` tests module

- [ ] **Step 1: Write it**

```rust
    /// ⚠⚠ The property this milestone exists for: **reframing must not move the airlight.**
    #[test]
    fn the_airlight_does_not_move_when_the_frame_is_reframed() {
        let img = structured(220, 180);
        let base = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            ..Default::default()
        };
        let want = super::prefix_airlight(&base, &img, Resampler::default(), None);

        let mut angled = base.clone();
        angled.geometry.angle = 6.0;
        let mut cropped = base.clone();
        cropped.geometry.crop = edit_model::CropRect {
            left: 0.2,
            top: 0.2,
            right: 0.8,
            bottom: 0.8,
        };
        let mut both = angled.clone();
        both.geometry.crop = cropped.geometry.crop;

        for (name, e) in [("angle", &angled), ("crop", &cropped), ("both", &both)] {
            let got = super::prefix_airlight(e, &img, Resampler::default(), None);
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() < 1e-6,
                    "{name} moved the airlight on ch{c}: {} vs {}",
                    got[c],
                    want[c]
                );
            }
        }
    }
```

- [ ] **Step 2: Run** — expected PASS.
- [ ] **Step 3: Commit.**

---

### Task 4: Sabotage

- [ ] **Step 1:** Remove `PrefixStepKey::Geometry(..)` from the skip list in `prefix_airlight`.
- [ ] **Step 2:** Run the invariance test → **must FAIL** on `angle` and `both`.
- [ ] **Step 3:** `git checkout crates/pipeline/src/lib.rs`, re-run → PASS.

⚠⚠ If it PASSES with `Geometry` back in the replay, the fixture does not move the airlight and the test proves nothing — pick a `structured` size or angle that does.

---

### Task 5: The golden prediction, then the gate

- [ ] **Step 1: ⚠ Predict first, then look.** All three dehaze goldens use `Edits::default()`
geometry, so **no golden should move**. Run the goldens alone:

`cargo test --release -q -p pipeline --test golden 2>&1 | grep 'test result'`

⚠⚠ If any golden moves, **stop and understand why** before accepting it. Do not regenerate a
reference to make a surprise go away — `crates/pipeline/tests` must stay unmodified.

- [ ] **Step 2: `cargo fmt`, commit any change, then gate**, reading three exit codes separately,
confirming 74 goldens ran and `git diff HEAD --stat` is empty.

---

### Task 6: Re-measure, launch, record

- [ ] **Step 1:** `probe_masked_airlight` on `DSCF6291`, `DSCF6296`, `DSCF6509` — **both rows at or
near 0.00 levels**. ⚠ Report all three: `DSCF6509` was near zero before the change too, so it
cannot evidence anything on its own.
- [ ] **Step 2:** Launch the GUI. What to look at: dehaze a hazy photograph and drag **Straighten** —
the dehazed tone must not breathe as the angle moves.
- [ ] **Step 3:** Record in `CLAUDE.md` (overwrite in place), `docs/JOURNAL.md`, and **correct the
`docs/BACKLOG.md` entry that says the replay does not move at all.**
- [ ] **Step 4:** Do not merge, do not push.

## Self-Review

**Spec coverage.** Done-when 1 → Task 1. 2 → Task 2. 3 → Task 3. 4 → Task 4. 5 → Task 5 step 1.
6 → Task 6 step 1. 7 → Task 5 step 2. 8 → Task 6 step 2, performed by the user.

**Placeholders.** None.

**Type consistency.** `capture_stages(&LocalAdjustments, RenderScale, Option<[f32;3]>)` is defined in
Task 2 step 1 and called in steps 2 and 4; `new_capture(.., Option<[f32;3]>)` in step 2 and called in
step 3. `prefix_airlight(&Edits, &ImageBuf, Resampler, Option<LensShot>) -> [f32;3]` is unchanged.
