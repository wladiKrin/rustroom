# A hue LUT for `HslStage` — design

**Status:** specced 2026-09-05. Branch `hue-lut`.

`HslStage` is the most expensive per-pixel stage in the pipeline, and the most expensive thing a
mask can carry. This makes it about **1.75× faster** without changing what it does to a photograph.

⚠ **It helps the global slider and every mask identically**, because both build the same stage. That
is what made it the next milestone after M7.2 rather than the bounding box, which helps two of the
five mask kinds.

---

## 1. The finding that reshaped this milestone

**M7.2 recorded that a hue LUT was worth ~2×, and that the cost was `atan2`, `cos` and `sin`. The
second half was wrong, and measuring it changed what gets built.**

The estimate came from comparing HSL (+34.9 ms) against colour grading (+15.0 ms) — two stages doing
the same Oklab round trip — and attributing the ~20 ms gap to the transcendentals, because those are
the visible difference in the source. **Nobody looked inside `band_weights`.**

⚠⚠ **This is the third time HSL's cost has been mis-reasoned in this project.** The M7.2 spec first
called masked HSL "per-pixel and nearly free"; corrected, it claimed ~4× from a LUT; corrected
again, ~2× from `cos`/`sin`. Every one was reasoning where a measurement was available. **The
decomposition took twenty minutes.**

## 2. The measurement

`cargo run --release -p app --example bench_hsl <raw>`, kept. X-T4 `.RAF`, 1600×1068 fit preview,
release, best of 7. Each variant is cumulative, so consecutive deltas are that step's cost. All
figures are above a **2.14 ms** `map_pixels`-plus-rayon harness floor.

| step | cost | share |
|---|---|---|
| Oklab round trip — **irreducible, untouched here** | 7.29 ms | 27% |
| chroma via `hypot()` | 2.62 ms | 10% |
| hue via `atan2()` | 3.38 ms | 13% |
| **band weighting** | **11.17 ms** | **42%** |
| chroma fade + `cos`/`sin` | 2.56 ms | 10% |
| **total above floor** | **26.54 ms** | |

Measured alternatives: `(a*a + b*b).sqrt()` is **2.15 ms** (−0.47), and a band weighting with no
`fmodf` that sums only the two live bands is **5.55 ms** (−5.62).

⚠⚠ **The band weighting is the largest single cost — larger than the Oklab round trip — and
`cos`/`sin`, the thing the LUT was named for, is under 10% even bundled with the chroma fade.**

⚠ **The kernel measured is proven identical to the shipped one, by pixels rather than by timing.**
`pipeline::stages` is private, so the example cannot call the real `HslStage` — which forced the
better check: apply the copied kernel, run the neutral tail over it, and assert bit-equality against
the HSL tail. A matching *duration* would only have shown it costs about the same.

### Why `band_weights` is so expensive

```rust
let span  = (BAND_HUES[next] - BAND_HUES[i] + 360.0) % 360.0;   // a CONSTANT, recomputed
let delta = (h - BAND_HUES[i] + 360.0) % 360.0;                 // fmodf, per band
```

Float `%` is `fmodf` — a libm call, not a hardware instruction on ARM — and the loop runs until it
finds the bracketing pair, so it averages **~7 `fmodf` per pixel**. `span` depends only on the
constant table. The caller then sums **24 multiply-adds over an array with exactly two non-zero
entries**.

## 3. Part 1 — the two changes with no design content

1. `chroma = a.hypot(b)` → `(a*a + b*b).sqrt()`. `hypot` is written to survive intermediate
   overflow; Oklab chroma components are order 0.1 and cannot approach it.
2. `band_weights` computes its spans from a `const` table, replaces each `fmodf` with a conditional
   add, and returns **the live pair `(i, j, s)`** rather than eight weights — so the caller does six
   multiply-adds instead of twenty-four.

**Together: −6.09 ms, i.e. −23%, before any LUT exists.**

### ⚠⚠ These are NOT bit-identical, and that was measured rather than assumed

`(x + 360.0) % 360.0` does not round-trip in f32: adding 360 to a value near 330 pushes it into a
binade with 2× the exponent spacing, and subtracting 360 does not recover the original. Swept over
3.6 M hues against the current implementation:

- **4 of the 8 spans differ**, by ~1e-5 degrees.
- **59% of weight vectors differ bit-for-bit** — and the **worst difference is 1.07e-6**, which is
  0.0003 of an 8-bit level.
- **Exactly one hue in 3.6 M selects a different band pair**, at a span boundary — where that band's
  weight is ~0 and the neighbour's is ~1, so the *output* is continuous across the disagreement.

**So the new code is the more accurate one**, and the goldens are expected to stay byte-identical
after quantisation. ⚠ **Expected, not assumed** — it is a thing the plan checks, not a claim.

## 4. Part 2 — the LUT, and the rotation identity that comes with it

### What the table holds

`[dh, sat, lum]` per entry — the **band-weighted slider values**, before the chroma fade. `N` entries
uniformly over `[0, 360)`, sampled with linear interpolation and **wraparound** between entry `N−1`
and entry `0`.

⚠ **It stores the parameters, not the finished complex factor**, and that is the load-bearing
choice. The chroma fade multiplies `dh`/`sat`/`lum` **before** they are used, so a table holding the
finished factor would be valid only where `chroma ≥ CHROMA_FADE` and would need a second code path
below it. **On a dark or desaturated frame that "slow" path becomes the common one**, making the
stage's cost depend on the picture. Two paths for one operation is the shape this project has been
bitten by repeatedly. One table, one path, no branch.

⚠ Storing plain values also makes interpolation **across the 360°→0° seam** simply correct. A table
of complex factors would interpolate along a chord there rather than an arc.

### The rotation, without `cos` and `sin`

`cos(hue)` and `sin(hue)` are already in hand — they are `a/chroma` and `b/chroma`. Expanding
`cos(hue + δ)` and multiplying by the new chroma, **the chroma cancels exactly**:

```
k  = 1 + sat/100                       ⚠ sat, lum and dh are the POST-FADE values
a' = k · (a·cos δ − b·sin δ)
b' = k · (a·sin δ + b·cos δ)
l' = l · (1 + lum/200)
```

⚠ The two existing `.max(0.0)` clamps stay. Both are unreachable given the ±100 slider range —
`k ∈ [0, 2]` by construction — so they are defence in depth, and this project's own rule applies:
**an unreachable guard is an untested one**, so say so rather than writing a test that pretends to
reach it.

`δ` is bounded by `MAX_HUE_SHIFT_DEG` at **±30° = ±0.5236 rad**, so `cos δ` and `sin δ` are short
Taylor polynomials rather than libm calls:

- `sin δ = δ(1 − δ²/6 + δ⁴/120)` — truncation error ≤ **2.1e-6**
- `cos δ = 1 − δ²/2 + δ⁴/24 − δ⁶/720` — truncation error ≤ **1.4e-7**

This removes the general `cos`/`sin` and the `to_radians`. ⚠ It does **not** remove a division —
the current code performs none. What matters is that the naive form of this identity *would* need
`a/chroma` and `b/chroma`, and the cancellation means that division is never performed. It also
removes the `atan2 → cos` round trip the current code does perform, so the rotation is **more**
accurate as well as cheaper; `hue` survives only as the table index.

### ⚠⚠ The bound is an assumption held by a constant somewhere else, so it is a COMPILE-TIME assertion

The polynomial's error bounds hold **only** because `MAX_HUE_SHIFT_DEG` is 30. If that constant is
ever raised, the approximation degrades **smoothly and silently** — there is no value at which a
test suddenly fails. So it takes a `const _: () = assert!(MAX_HUE_SHIFT_DEG <= 30.0)`, following
`MID_BUDGET_BYTES` and `MIN_RADIUS`: raising it becomes a build error rather than a slow drift.

⚠ This is the family of bug this project already records — *"a constant chosen to compensate for a
different bug moves once that bug is fixed"*. Here the dependency runs between two constants that do
not mention each other, which is worse.

## 5. Sizing the table — a measurement with a precedent

**`N` is chosen from what it drifts in 8-bit levels, not from taste.** The precedent is exact:
`ProfileToneCurve`'s LUT was raised 1024 → 4096 because at 1024 it drifted **0.61 of an 8-bit level**
from the spline it approximates.

⚠ **Two things are measured at each `N`, not one.** Accuracy *and* cost: at 4096 entries the table
is 48 KB, which is a large fraction of L1, and a table that no longer fits can be slower than
computing the thing. Hue is spatially coherent across neighbouring pixels so access should stay
cache-friendly — **should**, which is why it is measured.

**Target: no golden moves.** If that cannot be met at a sane `N`, the fallback is to re-author the
three HSL goldens and say by how much they moved — never to widen `TOL`.

## 5b. Where the code goes, decided before it is written

**Everything lives in `crates/pipeline/src/stages/hsl.rs`.** No new crate, no new module, and
nothing in `app` at all: this changes how a stage computes, not what it computes.

- **No `Edits` field**, so none of the exhaustive sites move, no edit group changes, and no panel
  work — which is what makes the goldens a complete acceptance test.
- **No new golden.** The three existing HSL goldens are the check, and they are meant to stay
  byte-identical (§5).
- The small-angle `sin`/`cos` stay **private to this file** rather than moving to `color`. They are
  correct only for `|δ| ≤ 0.5236`, which is a fact about *this* stage's slider range; exporting them
  would offer a general-looking pair of functions with a silent domain restriction.
- ⚠ `hsl.rs` is **370 lines** today. The table, the sampler, the polynomials and their tests will
  push it up; if it passes ~600 the sampler and the polynomials are the natural extraction, and this
  is flagged now rather than discovered later.

## 6. Testing — the four that would otherwise name what they cannot detect

1. **The table against direct evaluation.** ⚠ A test that calls `band_weights` on *both* sides
   proves nothing about sampling and interpolation — it is the colour-grading trap, where
   `the_table_agrees_with_direct_evaluation` stayed green under a hue-convention swap. The
   discriminating form evaluates the **whole kernel exactly** (direct weights, libm `cos`/`sin`) and
   asserts a bound on the difference, swept densely enough to land inside cells rather than on
   sample points.
2. **The rotation identity.** ⚠ A fixture at `δ = 0` cannot see a swapped `cos`/`sin` or a flipped
   sign, because `cos 0 = 1` and `sin 0 = 0` make it the identity; a **neutral** pixel cannot see it
   either, since `a = b = 0`. The discriminating fixture is a **saturated pixel with a non-zero hue
   shift**, asserting the resulting **hue** measured independently — not merely that the pixel moved.
3. **The wraparound.** ⚠ `i1 = i0 + 1` without `% N` is wrong only in the final cell,
   `hue ∈ [360 − 360/N, 360)`. A sweep at coarse spacing will step straight over it. It needs a probe
   deliberately inside the last cell, and one that interpolates across the seam.
4. **The `δ` bound.** Not a test — a compile-time assertion, because the failure is gradual (§4).

⚠ Two existing tests are the structural safety net and must pass **unchanged**:
`band_centers_match_canonical_colors` (the table has not drifted from the canonical sRGB hues) and
the equal-bands-equal-global property (the weights still sum to 1 at every hue).

## 7. What this deliberately does not deliver

1. **The Oklab round trip — 7.29 ms, 27%, the largest remaining item.** Untouched. Cutting it means
   approximating `cbrt`, which is a different question with a different accuracy argument, and it
   would affect every Oklab consumer (`hsl`, `noise_reduction`, `sharpening`, `local_contrast`,
   `color_grading`), not just this stage.
2. **`atan2` — 3.38 ms.** A LUT indexed by a *pseudo-angle* would delete it, since a table can be
   built over any monotonic parameterisation of hue. Deliberately not taken: it needs a
   sampling-density argument that the uniform-in-hue table does not, and it is a contained follow-up
   if the remaining cost ever matters.
3. **Nothing for colour grading**, which already has its LUT and is not the expensive stage.
4. **No change to the model.** Same eight bands, same centres, same smoothstep blend, same chroma
   fade, same ±30° hue range. ⚠ **A photograph must look the same**, and the goldens are how that is
   checked.

## 8. Expected result

| | above floor | vs today |
|---|---|---|
| today | 26.54 ms | — |
| after Part 1 | ~20.5 ms | 1.3× |
| after Part 2 | **~15 ms** | **~1.75×** |
| floor (Oklab + `atan2`) | ~10.7 ms | 2.5× |

⚠ **~1.75×, not the ~2× recorded in `CLAUDE.md`** — that figure predated separating the chroma fade
from `cos`/`sin`. Masked HSL should go from +33.1 ms to roughly +20 ms at fit; whether that changes
"a bit slow but usable" into "fine" is a judgement about a photograph and is checked by using it.
