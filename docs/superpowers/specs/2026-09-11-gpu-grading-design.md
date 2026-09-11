# Colour grading on the GPU — design

**Date:** 2026-09-11 · **Milestone:** **M8.3d** · **Branch:** `m8.3d-gpu-grading`

⚠ **This takes the M8.3d slot and renumbers what was there**, because the measurement in §1 moved it
ahead: **the mask loop becomes M8.3e** and **the prefix M8.3f**. `CLAUDE.md` and `docs/BACKLOG.md`
both still name the old numbering in places and are corrected when this lands — flagged here so the
next reader does not treat a stale "M8.3d — the mask loop" as current.

## 1. Why this, and why now rather than the mask loop

M8.3c measured a caveat nobody predicted. The global tail runs

```
… → saturation → hsl → color_grading → sharpen → …
```

so `hsl` and `sharpening` are adjacent **only while the grade is neutral**. Turn a wheel and
`color_grading` — which is not accelerated — lands between them, `accelerate` splits the run, and
the same edits pay **two** round trips instead of one:

| tail (1600×1068, 19.6 MB, best of 4 warm) | CPU | GPU | |
|---|---|---|---|
| hsl only | 27.71 ms | 17.99 ms | 1.54× |
| hsl + sharpening, **one chain** | 37.67 ms | 24.78 ms | **1.52×** |
| hsl + **grade** + sharpening, **two chains** | 47.95 ms | 45.93 ms | **1.04×** |

⚠⚠ **And the merge was measured directly, so the size of the loss is known rather than inferred:**
the same two stages as one chain against two ran **21.84 ms vs 32.01 ms**. A second round trip
costs about 10 ms, which is essentially the whole win.

So the grade is not one more stage to port — it is the stage that **disables** the feature for
anyone who grades. That is what moved it ahead of M8.3d, which remains the case actually reported
as slow and is a larger, different mechanism (§8).

## 2. What makes this cheap: the stage is already a LUT

`ColorGradingStage::apply` is, per pixel, in full:

1. `prophoto_to_oklab(rgb)` → `[l, a, b]`
2. `grade_at(l)` → `[da, db, dl]`, a **256-entry** table, linearly interpolated
3. `oklab_to_prophoto([l + dl, a + da, b + db])`
4. each channel clamped at **0 only** — never at 1, since the output transform is the only place
   clipping belongs

No `atan2`, no `cos`/`sin`, no band weighting, **no branch**. It is strictly simpler than
`HslStage`, which needed the rotation identity and two Taylor polynomials. The whole three-wheel
grade collapsing to a 1-D table is the stage's own founding decision, recorded at its `lut` field;
this slice inherits that rather than re-deriving it.

⚠ **The table is built on the CPU and uploaded, never rebuilt in WGSL** — `GpuHslStage`'s rule, and
for its reason: two constructions of one table is how they come to disagree about Blending and
Balance, which nothing in the rendered image would announce. That needs one accessor,
`ColorGradingStage::lut() -> &[[f32; 3]]`, mirroring `HueTable::entries`.

## 3. The shared WGSL prelude, and the guard on it

Both shaders need the same ~15 lines: `cbrt_signed`, the fused ProPhoto→LMS matrix, and the
Oklab→ProPhoto matrix. Today `hsl.rs` carries them inline; a second copy in a second shader is the
shape this project already records at its worst — **six copies of a Rec.709 luma triple, every one
of which became wrong the day the working space changed.**

So: `const OKLAB_WGSL: &str`, concatenated into both shaders at build.

⚠⚠ **The guard is a behavioural cross-check, not a parse of the shader text — and that is a
deliberate correction to the first proposal.** `color::oklab`'s matrices are **literals inside
`prophoto_to_oklab` and `oklab_to_prophoto`**, not public constants, so there is nothing to
substitute from and a text-parsing test would only compare digits against digits while breaking on
any reformatting. Instead a test runs colours through **the prelude on the GPU** and through
**`color::prophoto_to_oklab` on the CPU** and asserts they agree — over a set that must include a
**negative** sample and one **above 1.0**, since those are the two regions where a wrong
`cbrt_signed` or a stray clamp hides and an ordinary in-gamut colour cannot see either. That tests the property
that matters rather than a proxy for it, and it is the shape `the_profile_agrees_with_our_own_maths`
already uses to check our ICC output against lcms2.

⚠ The prelude is **not** made public API of `gpu-pipeline`. It is a crate-internal `const`, the
narrow version of the `pub use stages::hue_table` decision.

## 4. The trap, which is already on record

`grade_at` clamps the **index** into the table, never the pixel's own `l`:

```rust
let t = l.clamp(0.0, 1.0) * (LUT_SIZE - 1) as f32;
```

A scene-referred value above white has `L > 1`, and clamping the *value* would drag every highlight
back toward white — destroying exactly the headroom the wide working space exists to preserve.
⚠⚠ **A careless transliteration writes `clamp(l, 0.0, 1.0)` in WGSL and does precisely that**, on a
photograph, invisibly, because the result is still a plausible picture. It is the identical trap
`ProfileHueSatMap`'s `v` is documented for, and it gets its own named test with a fixture above 1.0.

## 5. Substitution is one line

M8.3c replaced the if-chain with a table, so the whole wiring is one entry:

```rust
("color_grading", |e, _, ctx| Box::new(GpuGradingStage::new(&e.color_grading, ctx))),
```

⚠ `scratch_planes()` returns **0** — the stage is per-pixel and in-place-safe, so the chain
allocates nothing extra and `GpuSharpenStage`'s four planes remain the max. ⚠ The `RenderScale`
argument is ignored, as `GpuHslStage` ignores it: the grade has no spatial extent.

⚠ `ColorGrading::is_active()` is **wheels-only** (Blending and Balance are modifiers), unchanged —
so a neutral grade still inserts no stage, nothing splits, and the two-chain case simply does not
arise. That is why this caveat only ever bit people who grade.

## 6. Testing

| test | what it is for |
|---|---|
| `the_gpu_grade_matches_the_cpu_within_a_hundredth_of_a_level` | the equivalence harness, tolerance 0.01 levels — HSL measured 0.00104 and sharpening 0.00905, so this is the established band |
| `a_lightness_above_one_keeps_its_headroom` | §4. Fixture deliberately above 1.0; a value-clamping shader fails it and passes everything else |
| `a_negative_sample_does_not_become_nan` | `cbrt_signed`. ⚠ The fixture must be **computed, not guessed** — M8.3b's first attempt at this test used an input whose LMS components were all positive, so `pow` never saw a negative base and the sabotage passed |
| `the_wgsl_prelude_agrees_with_color_oklab` | §3. ⚠ The colour set must span a negative sample and one above 1.0 — an in-gamut-only fixture cannot see either failure |
| `the_stage_really_grades_rather_than_copying` | ⚠⚠ every equivalence test above passes against a GPU **and** CPU stage that both do nothing. This asserts the fixture is one the grade moves |
| `it_asks_for_no_scratch_planes` | §5 — a chain of hsl + grade must still allocate zero |

⚠ Every test skips without an adapter, as the crate's others do, so a green Linux CI run is **not**
evidence about this code.

## 7. Acceptance: the bench's third row

`gpu_run_bench`'s third configuration reads **1.04× across two chains** today. With grading
accelerated it must become **one chain of three** — the first three-stage run, and the best case
run-merging has. The row's `chains` count is printed, so the structural half is visible directly.

⚠⚠ **If the row does not improve, the slice did not work**, and that is a finding to report rather
than a number to reframe. ⚠ Re-measure on a **quiet machine**: M8.3c's first bench run was taken
while `cargo test --workspace` saturated the cores and read 49.55 ms of CPU for *less* work than a
later 37.97 ms.

## 8. Non-goals

- **Masks (now M8.3e).** A mask's stages live inside `MaskStage`, which the tail sees as one
  opaque stage, so `accelerate` cannot reach them by name at all. A different mechanism, not another
  table entry — and still the case actually reported in use as slow.
- **Local contrast.** Needs three blur kinds (`box_blur`, `wide_blur`, the guided filter) where
  sharpening needed one, and it is **adjacent to nothing accelerated** — it runs before saturation
  and hsl, so porting it alone buys a third round trip rather than joining a run.
- **The prefix (now M8.3f)**, where the 156.6 ms native one-off is.
- **Shader and pipeline caching.** Both shaders are still rebuilt on every encode; the bench's
  round-0 column shows the cost. Contained, and nothing here depends on it.
- **The `StageAccelerator` hook.** Still the right answer once a dozen stages are accelerated, and
  still blocked because `RenderSpec` is `Copy + PartialEq` and an `Arc<dyn Trait>` is neither. Three
  entries is not a dozen.
