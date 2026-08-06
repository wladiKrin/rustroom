# Sharpening — design

Date: 2026-08-06
Status: approved

## Goal

Unsharp-mask sharpening with Lightroom's four controls — Amount, Radius, Detail, Masking —
completing the "detail" pair with noise reduction. NR without sharpening is half the story:
denoising softens, and nothing puts the edge definition back.

## History: designed, parked, unblocked

This was designed first and deliberately parked. At a fit-view preview scale of 0.256 a
full-resolution σ of 1.0 becomes σ=0.256 — a Gaussian whose kernel is `[0.0005, 0.999,
0.0005]`, i.e. the identity — so sharpening did **nothing** visible while editing. That is not
a bug: the 1600 px preview no longer contains the detail sharpening acts on.

Rather than ship a slider you had to export to evaluate, 1:1 region rendering was built first
(`specs/2026-08-06-one-to-one-region-rendering-design.md`). Two consequences for this design:

- **No `SIGMA_MIN` clamp.** At buffer-native the scale is 0.5, so σ=1.0 becomes a real 0.5
  blur. Sharpening still does almost nothing at fit view — correctly — and is judged zoomed in.
- **Sharpening lands in the region-cropped tail**, which introduces the margin invariant in §5.

The decisions below were settled during the original brainstorm and are recorded rather than
re-derived.

## 1. New primitive: `spatial::gaussian_blur`

```rust
/// Separable Gaussian blur of a single-channel plane, with a **float** sigma.
pub fn gaussian_blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32>;
```

Kernel radius `ceil(3σ)`, weights normalized to sum 1, **edge coordinates clamped**
(replicate) so a constant plane stays exactly constant. Both passes parallel over rows —
simpler than `box_blur`, whose vertical pass had to avoid a transpose, because a Gaussian
carries no running-sum state.

It sits **alongside** `box_blur`, not replacing it. They serve opposite regimes:

| | radius | why |
|---|---|---|
| `box_blur` (NR) | 6–32 px | O(1) in radius is what makes it affordable |
| `gaussian_blur` (sharpening) | 0.5–3 px | a direct kernel is cheaper here (7 taps) **and** integer box radii cannot represent sub-pixel blur at all |

`sigma <= 0` returns a copy.

## 2. The stage

Operating on Rec.709 luma and applied back as a ratio: no colour fringing on chromatic edges,
hue and saturation ratios preserved exactly, and no Oklab round trip (the cost that dominates
HSL and NR).

```
luma = 0.2126R + 0.7152G + 0.0722B          // linear light
d    = luma - gaussian_blur(luma, σ)        // the high-frequency detail

// Detail — halo suppression at high local contrast.
s    = detail/100
supp = s + (1 - s) / (1 + (|d| / HALO_KNEE)²)

// Masking — gate on edge strength. At masking = 0 the whole branch is skipped and
// mask = 1, which also avoids smoothstep(0, 0, ..) being degenerate.
edge = gaussian_blur(|d|, MASK_SIGMA · scale)
mask = smoothstep(0, MASK_MAX · masking/100, edge)

luma' = luma + (amount/100 · MAX_AMOUNT) · d · supp · mask

// Apply back, preserving hue.
gain = (1 + (luma' - luma) / max(luma, LUMA_EPS)).clamp(0, GAIN_MAX)
out  = rgb · gain
```

**Detail** at 100 gives `supp = 1` — a pure unsharp mask. At 0, `supp → 1` for small `|d|`
(fine texture keeps full sharpening) and `→ 0` for large `|d|` (halos around strong edges are
suppressed). This is our own model, not Adobe's: defensible and monotone, not a clone. It is
genuinely distinct from Masking, which gates on *where* rather than *how strong*.

**The near-black guard is load-bearing.** Without it a dark pixel beside a bright edge has
`luma ≈ 0` and the ratio explodes. `LUMA_EPS = 1e-3` with `GAIN_MAX = 4.0` bounds a sharpening
halo in near-black regions to +2 stops.

### Constants

| name | value | meaning |
|---|---|---|
| `MAX_AMOUNT` | 1.5 | Amount 100 |
| `HALO_KNEE` | 0.05 | luma excursion above which Detail starts suppressing |
| `MASK_SIGMA` | 2.0 full-res px | blur applied to `\|d\|` so the mask is smooth, not speckled. Scaled by `RenderScale` like the main σ, or the mask would change character with the render mode |
| `MASK_MAX` | 0.08 | edge strength at which Masking 100 still sharpens fully |
| `LUMA_EPS` | 1e-3 | near-black guard |
| `GAIN_MAX` | 4.0 | caps a halo at +2 stops |

## 3. Parameters

| slider | range | default | maps to |
|---|---|---|---|
| **Amount** | 0–100 | 0 (off) | strength |
| **Radius** | 0.5–3.0 | 1.0 | σ **in full-resolution pixels**, × `RenderScale` |
| **Detail** | 0–100 | 50 | halo suppression via `HALO_KNEE` |
| **Masking** | 0–100 | 0 (off) | edge gate via `MASK_MAX` |

Radius is the one slider not on a 0–100 scale, deliberately: it is a physical pixel quantity,
and naming it in pixels is what makes "full-res radius, scaled for the render" comprehensible.

`Sharpening::is_active()` is **amount-only** — the same rule as `NoiseReduction` and
`Vignette`. Radius, Detail and Masking are modifiers.

## 4. Pipeline position

```
… → hsl → vignette → SHARPENING → output transform
```

Twelve stages. Sharpening runs after the vignette, so darkened corners sharpen proportionally
less in absolute terms — correct for a ratio-based model.

It lands in the preview cache's **tail**, so it re-runs per slider move. That is right (its
parameters change often) and cheap: σ is small, so the kernel is a handful of taps.

`PrefixKey` is untouched. `all_edits()` in the pipeline tests will fail to compile until the
new field is placed — the tripwire working as designed.

## 5. ⚠ The margin invariant

Noise reduction's guarantee that no region boundary can ever produce a seam came from it living
in the **prefix**, which always runs whole-buffer. Sharpening is in the **tail**, which runs on
the *cropped* region — so it sharpens using neighbours that do not exist at the region's edge,
producing a band of wrong output there.

That band is at most `ceil(3σ)` wide: **9 px** at the largest full-resolution radius, ~5 at
buffer-native. `REGION_MARGIN` is **128 px**, and the visible rect is always at least that far
inside the rendered rect, so the band falls entirely within the margin and is never displayed.

This is now a real invariant rather than a coincidence:

> **`REGION_MARGIN` must exceed the kernel radius of every spatial stage in the tail.**

It is asserted by a test comparing `REGION_MARGIN` against the maximum sharpening radius, so
that raising `Radius`'s range — or adding another spatial tail stage — fails loudly instead of
producing seams that only appear while panning at 1:1.

### This weakens the master invariant, and the test must say so

`a_region_render_matches_the_whole_render_cropped` currently asserts

```
render_region(buffer, edits, region) == crop(render_whole(buffer, edits), region)
```

**bit-identically**. With a spatial stage in the tail that is no longer true at the region's
edge, and it never can be — the region genuinely does not have those neighbours.

The test passes today only because its edit set has no sharpening, which would leave the
weakening silent and undiscovered. So it is extended rather than left alone: sharpening is
**added** to the edit set, and the comparison is **inset by `ceil(3σ)`**. The interior stays
bit-identical, the excluded band is precisely what `REGION_MARGIN` keeps off screen, and the
inset is derived from the radius rather than hard-coded — so it tracks any future change.

## 6. Testing

**`gaussian_blur`** — σ ≤ 0 is identity; a constant plane stays exactly constant (weights
normalized *and* edges replicated); the impulse response is symmetric and peaks at centre;
larger σ strictly reduces the variance of a noisy plane; a known small case (σ=1, radius 3) has
weights matching `exp(-x²/2σ²)` normalized.

**Stage** — Amount 0 is an exact identity; a flat region is untouched (`d = 0`); **contrast
across a hard edge increases**; hue ratios preserved; Detail 100 equals a pure USM while
Detail 0 suppresses a strong edge more than fine texture; **Masking 80 leaves a noisy flat
patch alone while an edge still sharpens** (the pairing that justifies the control); no negative
or NaN samples; the near-black guard bounds gain rather than exploding.

**Margin invariant** — `REGION_MARGIN > max_kernel_radius(RADIUS_RANGE.1)`.

**Master invariant, extended** — `a_region_render_matches_the_whole_render_cropped` gains
sharpening in its edit set and insets its comparison by `ceil(3σ)`, per §5. Without that change
the test would keep passing while quietly no longer covering the case it exists for.

**Goldens** — three on the existing `noise_chart()`, whose hard edge *and* noise are exactly
what sharpening interacts with: `sharpen_basic`, `sharpen_masked` (masking 80 — should visibly
spare the noise), `sharpen_detail_low`. **The 20 existing references stay byte-identical.**

**Perf** — a `bench_preview` case; expect a few ms at preview scale (two Gaussians at small σ).

## 7. Accepted caveats

1. **Sharpening is judged at 1:1, not at fit.** At fit scale the effect is nearly invisible —
   which is correct, since neither the preview nor a fit-view of the export contains that
   detail. This is why 1:1 rendering came first.
2. Even at 1:1 the editing buffer is a **half-size** decode, so what you see corresponds to
   ~2× the radius the export will apply, and the half-size path skips demosaic entirely while
   export runs AHD. Bounded judgement, not exact.
3. No output/print sharpening for resized exports — a separate feature.
4. Detail is our own model, not Adobe's.
5. Sharpening after the vignette means darkened corners sharpen less in absolute terms.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- Three new golden references committed; the 20 existing ones byte-identical.
- The margin invariant is asserted by a test.
- In the GUI: the four sliders work, participate in undo/redo, round-trip through the sidecar,
  and the effect is visible when zoomed to 1:1.
- `CLAUDE.md` updated: 12 stages, `gaussian_blur` alongside `box_blur` and why, the margin
  invariant, and measured perf.
