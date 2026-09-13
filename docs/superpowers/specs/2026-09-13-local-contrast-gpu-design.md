# M8.3n — local contrast on the GPU

**Date:** 2026-09-13
**Status:** design

Texture and Clarity as compute shaders — the last unported mask sub-stage, and the last stage that
can split a mask's chain or take the leading position and deny M8.3l's shared upload.

## ⚠⚠ What this is worth, measured first, because the honest answer is not "a faster catalogue"

Re-scanned today (M8.3m's lesson: **never quote a catalogue figure from memory**). Of 40 sidecars
and 40 copies: **0 use Texture, 1 uses Clarity (`DSCF6546.RAF.json`, `clarity: -5.0`), 0 use either
in a mask.** So this will not move this catalogue, exactly as M8.3m did not, and this spec does not
claim it will.

What it buys is the three things `CLAUDE.md`'s milestone block named, and they are structural
rather than numeric:

1. **It is the last stage that can split a mask's sub-stage chain.** After this a mask carries
   **10 of 10** sub-stages on the GPU, so "port the rest of a mask" stops being a list.
2. **It is the last stage that can deny a mask M8.3l's shared upload** by taking the leading
   position. M8.3l shares the base plane only with a mask whose sub-stages *begin* with a
   replaceable stage; `local_contrast` is now the only one that can begin a chain and refuse.
3. **It is the largest unported per-pixel-adjacent tail stage** — `texture_minus` is the single
   most expensive stage in the whole tail, above `hsl`.

## The measurement that decides the design

`local_contrast` was the one stage `probe_stages` did not cover, which is why this project's own
numbers could never say what it costs. Four cases added, split **by band and by sign** — positive
Texture measures against a Gaussian and negative against a guided filter, two different amounts of
work behind one slider. At 3200×2136, deltas over baseline, band across three runs:

| case | CPU |
|---|---|
| `texture_plus` | 12.8–25.2 ms |
| **`texture_minus`** | **50.4–66.8 ms** — the most expensive stage in the tail |
| `clarity` | 17.0–32.1 ms |
| `texture+clarity` | 29.3–42.9 ms |

⚠ `texture+clarity` is **less** than the sum of its parts, because the luma extraction and the
final gain pass are shared. That is the same shape as `probe_stages`' own tail rows and is why the
GPU stage must compute both bands in one pass rather than as two stages.

### ⚠⚠ The "three blur kinds" framing overstated the work by a factor of three

`CLAUDE.md` has recorded since M8.3c that this stage "needs three blur kinds (`box_blur`,
`wide_blur`, the guided filter) where sharpening needed one", and that is what scoped it out. It is
not true as a statement about **primitives**:

- `wide_blur` **is** three iterated `box_blur` passes.
- `guided_filter` **is** four-to-six `box_blur` passes plus a per-pixel slope/offset step.
- `gaussian_blur` is **already on the GPU**, in `sharpen.rs`'s `blur_h`/`blur_v`, kernel uploaded
  from `spatial::gaussian_kernel` — proven and shipped since M8.3c.

**So `box_blur` is the only genuinely new GPU primitive.** It does need its own shader rather than
reusing `blur_h`: its border rule is that **the window shrinks and the sum is divided by the count
actually inside it**, where `gaussian_blur` *replicates* the edge sample. Those are different
answers at a border, and a constant plane stays exactly constant only under the first.

Decomposed at the same size: `gaussian_blur(σ=2)` 13.3–15.4 ms (13 taps), `wide_blur(σ=40)`
13.5–13.8, `guided_filter(r=2, self)` 40.4–42.1, and ⚠⚠ **`box_blur`'s CPU cost is flat in the
radius** — 4.68 ms at r=40 against 4.63 at r=2, because it is a running sum.

## ⚠⚠ The load-bearing risk, probed before the design: a naive GPU box blur LOSES at Clarity's radius

That flatness is exactly what a GPU thread does not have. `crates/gpu-pipeline/src/box_probe.rs`
(the M8.3f precedent: probe the design's central assumption before speccing it) runs `passes`
iterations of a separable box blur at radius `r`, timed against a `passes = 0` run so the
subtraction isolates the blur from the round trip. Apple M2, 3200×2136:

| | naive O(r) | **running sum O(1)** | CPU | best |
|---|---|---|---|---|
| Clarity `wide_blur` r=40 ×3 | 43.3–46.9 ms | **8.85–12.32** | 13.5 | **running, 1.1–1.5×** |
| Texture guided r=2 ×4 | **3.98–6.51** | 12.78–14.77 | 28.0 | **naive, 4.3–7.0×** |

**The naive shader is 3.4× SLOWER than the CPU at Clarity's radius and 7× faster at Texture's.**
Clarity's 81-tap window × 2 passes × 3 iterations is 486 taps per pixel; the CPU keeps a running
sum and pays five.

### The design that falls out: two shaders, chosen by radius

Crossover **swept, not guessed** — `MIN_GPU_BOX_PX` was swept and turned out to be low by ~8×, and
`MAX_TILE_DABS` was sized by a *limit* rather than the data and made the whole rasteriser 5× slower
than the CPU. Three passes, the shape both consumers use:

| r | 1 | 2 | 3 | 4 | 6 | **8** | 10 | 16 | 24 | 40 |
|---|---|---|---|---|---|---|---|---|---|---|
| naive | 4.06 | 4.96 | 6.41 | 7.57 | 10.29 | 11.83 | 12.71 | 21.89 | 29.19 | 46.89 |
| running | 11.36 | 11.17 | 11.30 | 11.32 | 17.40 | **10.20** | 11.52 | 11.70 | 12.35 | 11.64 |

The running sum is **flat in the radius**, as O(1) predicts; the naive one is linear. They cross
between **r = 6 and r = 8**, so **`RUNNING_SUM_MIN_RADIUS = 7`**.

⚠ The running row's 17.40 at r = 6 is **noise, not a bump** — it sits above both its neighbours in
a series that is otherwise flat to within 2 ms across a forty-fold change in radius, and the
mechanism (a running sum does the same work at every radius) admits no r-dependence. Named here so
nobody reads a feature into it; the plan re-runs the sweep and quotes a band.

⚠ The radius depends on the render scale, so the choice is made per encode rather than per stage.
`texture_guided_radius` gives **r = 2** at native and **r = 1** at fit (its `.max(1)`);
`wide_blur_radius(CLARITY_SIGMA · scale)` gives **r = 40** at native and **r = 10** at fit. Both
sit well clear of the threshold at native, and Clarity's fit-resolution r = 10 is the one case near
it — where the two shaders are within 1.2 ms of each other, so the threshold's exact value is not
load-bearing there.

### ⚠⚠ The running-sum shader is a transliteration of `blur_rows`, so it can be BIT-IDENTICAL

`blur_rows` primes `sum` by summing `[0, min(r+1, w))` in ascending order, then slides one step per
pixel with one add and one subtract, dividing by the live window count. The shader does exactly
that, one thread per row. **Same operations, same order, same divisor** — so unlike every ported
stage with a multiply-add, this one has no contraction to fuse and should come back bit-identical.

⚠ **The naive shader cannot be**, and that is not a defect: re-summing a 5-sample window is a
*different* accumulation from a running sum carried across 3200 pixels, and it is the **more**
accurate of the two — the CPU's running sum is the one that drifts. The divergence is bounded by
the CPU's own accumulated error. The guard is therefore a bound in 8-bit levels, as
`saturation`'s is, not an equality.

⚠ The cost of the running sum is coalescing: on the horizontal pass adjacent threads walk adjacent
**rows**, so their reads sit `w` floats apart. That is the ~11 ms floor visible at every radius in
the sweep, and it is why the naive shader wins below r = 7 despite doing more arithmetic. The
vertical pass has the opposite property — adjacent threads are adjacent columns, so a step reads
one contiguous row — which is exactly why `blur_cols` advances every column together.

## ⚠⚠ Porting is all-or-nothing, so "ship Texture and skip Clarity" was never available

`REPLACEABLE`'s `Builder` returns `Box<dyn GpuStage>` with **no `Option`**, so a GPU stage cannot
decline. Both bands and both signs of Texture must ship together or the row cannot exist. That is
what made the probe above load-bearing rather than interesting: had the wide-radius blur stayed at
0.29×, the milestone would have had to either solve it differently or not happen.

## Design

One `GpuStage`, encoding its dispatches into **one compute pass** over shared scratch planes — the
`sharpen.rs` shape, deliberately **not** a `Dispatch` client, because `Dispatch` encodes exactly one
per-pixel kernel and this needs a dependent sequence.

The dispatch sequence mirrors `apply_with` exactly:

1. `luma_pass` — RGB → `l`, through **`luma_wgsl()`**, so it cannot invent a second working-space
   luminance. (`luma` delegates to `color::luminance`; its own comment records that this was once
   one of six copies of a Rec.709 triple, all of which went wrong on one day.)
2. **Texture band**, when `k_t != 0`:
   - `k_t > 0`: a separable Gaussian in the same kernel-upload shape as `sharpen.rs`'s `blur_h` /
     `blur_v`, with the weights from `spatial::gaussian_kernel(TEXTURE_SIGMA · s)` **uploaded**
     rather than recomputed — the rule `HueTable::entries` and `ColorGradingStage::lut` follow, and
     the reason `gaussian_kernel` was made `pub` at M8.3c.
   - `k_t < 0`: the self-guided filter at `texture_guided_radius(scale)` — square, two box blurs,
     one per-pixel slope/offset kernel, two more box blurs, one combine.
   - `accumulate_texture`: `delta += k_t · (l − base)`.
3. **Clarity band**, when `k_c != 0`: three box-blur iterations at `wide_blur_radius(CLARITY_SIGMA · s)`,
   then `accumulate_clarity`: `delta += k_c · (l − base) · midtone_weight(l) · halo_suppression(l − base, floor)`.
4. `apply_gain` — `gain = clamp(1 + delta / max(l, LUMA_EPS), 0, GAIN_MAX)`, a scalar RGB multiply.

⚠ **Both bands read `l` and never each other's output.** That is what makes the two sliders exactly
additive, and a shader that chained them would be a different stage wearing the same name — the
sort of change that produces a plausible photograph rather than an error.

### Every constant substituted, never transcribed

`MID_GRAY`, `CLARITY_HALO_KNEE`, `CLARITY_HALO_FLOOR`, `LUMA_EPS`, `GAIN_MAX`, `TEXTURE_SIGMA`,
`CLARITY_SIGMA` and `TEXTURE_GUIDED_EPS` are interpolated into the WGSL from their `pipeline`
definitions; the Gaussian kernel is uploaded from `spatial::gaussian_kernel`; the radii come from
`wide_blur_radius` and `texture_guided_radius`. A substituted constant **cannot** disagree, where a
transcribed one is merely checked for agreeing — `luma_wgsl`'s own doc records why that is strictly
stronger than `oklab_wgsl`. Several become `pub(crate)` or `pub` for it.

### ⚠⚠ The covariance is NOT clamped and the variance IS — and getting that wrong costs 4.7e-6

`guided_filter`'s own comment says why: *"unlike a variance it can legitimately be negative when
`p` and `guide` are anti-correlated."* In the **self-guided** case `cov` and `var` are the same
expression, so the obvious reduction `s = var / (var + eps)` looks identical and is not — on flat
areas where float cancellation pushes the value slightly negative it silently drops the small
negative slope. Measured while writing the reference implementation for this spec: **that mistake
costs 4.7e-6 of divergence**, which is how it was caught. The shader keeps both.

### ⚠ A CPU win rides along, and it is a prerequisite rather than a bonus

In the self-guided case (`p == guide`) `mean_p` **is** `mean_guide` and `cross` **is** `guide_sq`,
so `guided_filter` computes **four** statistic box blurs where two suffice. Removing the duplicates
is **bit-identical** and **30% faster**: 40.6 → 28.3 ms at this size.

⚠ It rides along because setting a `cpu_cost` against code with a known 30% of fat in it is
**exactly the M8.3h staleness failure** — a constant that describes a measured cost, gone stale
because something else made that cost cheaper, with nothing in the build to say so. It also speeds
**noise reduction**, which is self-guided for the same reason; dehaze uses the general form and is
unaffected.

⚠ The `cpu_cost` constants are therefore measured **after** the slimming, and each ships at the
**bottom** of its band — the conservative direction, since a lower number makes a run less likely
to chain and the gate must fail toward the CPU.

## Scratch planes

The chain allocates the **max** across its stages, and `sharpen` currently sets 4. **This stage
needs 5**, set by its worst path — negative Texture — and the other paths fit inside it:

| plane | negative Texture | positive Texture | Clarity |
|---|---|---|---|
| `P0` | `l` | `l` | `l` |
| `P1` | `delta` | `delta` | `delta` |
| `P2` | `l²` → `mean_g_sq` → `slope` → `mean_slope` | blur intermediate | base (3 box iterations, in place) |
| `P3` | `mean_g` → `offset` → `mean_offset` | base | box intermediate |
| `P4` | box intermediate | — | — |

Two things make 5 enough rather than 7. A box blur is **in place at the plane level** — `box_h`
writes `P2 → P4` and `box_v` writes `P4 → P2`, so a statistic never needs a destination of its own.
And the final combine is **folded into the accumulate**: one kernel reads `P2`, `P3` and `l` and
writes `delta += k_t · (l − (P2·l + P3))`, so the guided base is never materialised. Texture
finishes into `delta` before Clarity starts, so the two bands share `P2`–`P4`.

At 6.8 Mpx that is **136 MB** of scratch, against `sharpen`'s 109. ⚠ It is allocated per **chain**,
not per stage, so a chain containing this stage pays it whether or not its neighbours want it — the
property M8.3c's `scratch_planes` doc already records, now with a larger number behind it.

## Equivalence

- Both **white_balance**-class claims are off the table: this stage is full of multiply-adds, so
  the GPU will fuse and round once where the CPU rounds twice. The bound is **8-bit levels**, as
  `saturation`'s and `contrast`'s are.
- ⚠⚠ **`apply_gain`'s `clamp(.., 0.0, ..)` needs a raw-float assertion.** `compare` quantizes to
  8-bit levels, where **a negative sample and 0 are the same value** — the blind spot M8.3e
  recorded for the output transform's clamp and which M8.3m then hit three more times in one day.
  The stage's own doc claims it "can never emit a negative sample"; that claim needs a test that
  can see it.
- ⚠ Fixtures must carry **negatives, values above 1.0, and both signs of both sliders** — four
  cases, because positive and negative Texture take genuinely different code paths.
- ⚠ A fixture must have **real spatial structure at both scales**, or the stage is inert on it: a
  flat plane has `l − base == 0` everywhere and every shader passes. A separate fixture check
  asserts the CPU stage really moves the fixture before anything is compared against it.

## Testing

1. Per path: equivalence within a bounded number of levels — `texture_plus`, `texture_minus`,
   `clarity`, and both together (which is the only case that exercises the shared `l` and the
   summed `delta`).
2. **The box blur against `spatial::box_blur` directly**, both shaders, at radii either side of the
   threshold — the running one asserted **bit-identical**, the naive one bounded.
3. **The threshold**, asserted as a decision: which shader each of the four real radii selects.
4. The border rule: a constant plane must stay **exactly** constant, which is the property that
   separates the shrinking window from edge replication and which no photographic fixture can see.
5. The negative-clamp raw-float assertion above.
6. The CPU slimming: `guided_filter` self-guided must be **bit-identical** before and after, and
   the three noise-reduction goldens must not move.
7. The decision table: which runs chain before and after, since `local_contrast` sits between
   `tone_curve` and `saturation` and porting it makes the whole per-pixel head **contiguous
   through to `hsl`**.
8. The 63 goldens must not move — the CPU path changes only by the bit-identical slimming.

## Not in scope

The **vignette** (position-dependent) and the **creative look** (a 3-D texture) still split the
tail and each need a different mechanism. After this they are the only two.
