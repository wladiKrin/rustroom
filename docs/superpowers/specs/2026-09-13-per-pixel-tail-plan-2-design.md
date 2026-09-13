# M8.3m — the rest of the per-pixel tail (M8.3e plan 2)

**Date:** 2026-09-13
**Status:** design

White balance, contrast, the tone curve and saturation as compute shaders — the four stages
M8.3e's plan 2 named and did not build.

## ⚠⚠ What this is worth, stated first, because the honest answer is not "a faster catalogue"

**0 of 40 sidecars use any of these four globally.** That was measured before M8.3k and it is why
plan 2 has been deferred three times. **The global bench rows will not move on real data, and this
spec does not claim they will.**

What it buys instead is three things, in ascending order of value:

### 1. Two runs that are refused today become one run that pays

The tail's order is
`wb → exposure → contrast → tone_regions → tone_curve → local_contrast → saturation → hsl →
grading → mask → vignette → sharpening → display_rendering → creative_look → output_transform`.

`contrast` and `tone_curve` sit **between** ported stages, so a photo using them splits the
per-pixel tail into pieces too small to pay:

| | today | after |
|---|---|---|
| | `[exposure]` **1.3 — refused** | `[wb, exposure, contrast, tone_regions, tone_curve]` **32.5 ✓** |
| | `[tone_regions]` **18.0 — refused** | |
| | `[hsl, grading]` 108.0 ✓ | `[saturation, hsl, grading]` 115.2 ✓ |

So **19.3 ms of already-ported work is stranded on the CPU today** purely because a CPU stage sits
between it and its neighbours. Porting the four recovers that as well as their own 20.4 ms.

### 2. Nine of ten mask sub-stages, leaving exactly one

A mask can carry ten sub-stages; five are ported. After this, **only `local_contrast` is not** —
which turns "port the rest of a mask" from a list into a single named item.

### 3. ⚠⚠ It is what makes M8.3l's shared upload reachable for more than one mask shape

This is the reason to do it now rather than fourth. M8.3l shares the base plane **only with a
mask whose sub-stages begin with a GPU-able stage** — `MaskStage::apply` pipes the clone through
its sub-stages in order, so only `stages[0]` receives the plane the gate was handed. A mask
starting with white balance, contrast or the tone curve pays **both** uploads today. Three of the
four are exactly those leading positions.

## Design

Four `Dispatch` clients, each per-pixel, in-place, `scratch_planes = 0` — the `GpuExposureStage`
shape, which since M8.3f is ~20 lines of stage plus a shader string.

| stage | shader | cost |
|---|---|---|
| `white_balance` | three multiplies, gains as uniforms | **4.0** |
| `contrast` | `max(PIVOT + (s - PIVOT) * slope, 0)` per sample | **1.2** |
| `tone_curve` | 1024-entry LUT, linear interpolation, per sample | **8.0** |
| `saturation` | ProPhoto luma, chroma, one scale per channel | **7.2** |

Costs measured by `pipeline`'s `probe_stages` at 3200×2136, three runs:
wb **3.77–4.33**, contrast **0.97–1.97**, tone_curve **8.07–9.27**, saturation **7.19–9.39**.
⚠ Each shipped value sits at the **bottom** of its band, which is the conservative direction for a
`cpu_cost`: a lower number makes a run *less* likely to chain, and the gate must fail toward the
CPU. (`TRANSFER_COST` is rounded up from the top of its band for the mirror reason.)

⚠ The same run re-measured the three existing constants as a sanity check: exposure 1.07–1.50
against a recorded 1.3 ✓, tone_regions 15.9–17.0 against 18.0, display_rendering 23.3–24.9 against
25.7. The two older ones read slightly high, i.e. slightly *less* conservative than they claim.
Recorded, not changed — re-tuning a shipped constant is its own milestone, and M8.3h is the
precedent for how much care that takes.

### ⚠⚠ Every constant is SUBSTITUTED from the CPU's own definition, never transcribed

`luma_wgsl` already establishes this and its doc says why it is *strictly stronger* than
`oklab_wgsl`: a substituted constant **cannot** disagree, where a transcribed one is merely checked
for agreeing. `color`'s own doc records six copies of a Rec.709 triple that all went wrong on one
day.

- `contrast::PIVOT` and `saturation::EPS` become `pub` and are interpolated into their shaders.
- `saturation` reuses **`luma_wgsl()`**, so it cannot invent a second luminance.
- `tone_curve` uploads **the CPU stage's own LUT** via a new `ToneCurveStage::lut()` — the rule
  `ColorGradingStage::lut()` and `HueTable::entries` already follow. Two constructions of one curve
  is how a preview stops being evidence about the render.

### ⚠ The tone curve's shader must reproduce `sample()` exactly, clamp included

`sample` does `v.clamp(0.0, 1.0)` **before** indexing, so a scene-referred highlight above 1.0 is
pulled to the curve's top entry. That is the CPU's behaviour, so parity requires it — and it is the
opposite of the rule `ProfileHueSatMap`'s `v`, `ColorGradingStage`'s index and the display
rendering's LUT each follow, where the *index* clamps and the *value* keeps its headroom. ⚠ A
shader that "fixed" it would be a different stage wearing the same name, and only a fixture
carrying samples above 1.0 can see the difference.

### White balance needs the baseline illuminant, and `RenderSpec` already carries it

M8.3e added `RenderSpec.baseline` anticipating exactly this, so no signature changes. The builder
recomputes the gains the CPU stage would get:
`adaptation_gains(Illuminant::new(wb.kelvin.unwrap_or(baseline.kelvin), wb.tint), baseline, …)`.
⚠ That expression exists in `adjustment_stages` and must not be written a second time — extract it
to a named function in `pipeline` and call it from both, or the GPU and CPU stages can disagree
about what "as shot" means.

## Equivalence

**Expected bit-identical for all four**, and that is a claim to verify rather than assume. Every
operation is exactly representable and performed in the same order; the gains and the slope are
computed CPU-side and arrive as uniforms, exactly as `GpuExposureStage`'s gain does — which is what
makes a divergence mean *plumbing*, not float drift.

⚠ The one known exception stays: the GPU flushes subnormals to zero and the CPU does not
(`a_subnormal_result_is_flushed_to_zero_on_the_gpu`). Fixtures use `1.0e-20`, not
`f32::MIN_POSITIVE`.

⚠ Every fixture must carry **negatives, values above 1.0, and both sides of each parameter's
neutral** — exposure's test was blind to the only real divergence this crate has found because it
tested one direction only.

## Testing

1. Per stage: bit-identity against the CPU stage on an awkward fixture, both sides of neutral.
2. Per stage: the name matches the CPU stage's, since six order-asserting tests match on it.
3. ⚠ **`tone_curve`: a fixture with samples above 1.0**, or the clamp is untested.
4. ⚠ **`saturation`: a coloured fixture**, since a neutral pixel has zero chroma and is untouched at
   any setting — a grey fixture cannot see the stage at all.
5. ⚠ **The decision table**, M8.3h's lesson: a test asserting *which* runs chain before and after,
   so "two refused runs become one" is pinned rather than believed.
6. The 63 goldens must not move — these are GPU-only additions and the CPU path is untouched.

## Not in scope

`local_contrast` (three blur kinds), the `vignette` (position-dependent) and the `creative_look`
(a 3-D texture) each need a different mechanism and stay on the CPU. After this they are the only
three per-pixel-or-not tail stages left, and `local_contrast` is the one that blocks a mask.
