# M8.11 — a mask's coverage and blend on the GPU

Item 4 of the agreed systematic order: *mask coverage + the blend*. Brush coverage (M8.3k), the
shared base upload (M8.3l) and all ten sub-stages (M8.3j, M8.3m, M8.3n) are already ported. What is
left is the two phases that have never had a GPU path — **computing a mask's coverage** and
**blending the adjusted image back by it** — and they are one mechanism rather than two.

## 1. The measurement that scoped it

⚠ Taken **before** any design, on `DSCF6274.RAF` at rung 3200 (3123×2085, 6.5 Mpx), best of 5,
CPU path, ⚠ load 5.17 — **not a quiet machine**, which biases every column pessimistic.

| mask | cov | clone | sub | **blend** | total |
|---|---|---|---|---|---|
| colour range | 0.00 | 2.1–3.2 | 3.9–5.9 | **68.5–90.7** | 74.5–97.8 |
| brush, plain | 13.0–18.8 | 2.1–3.8 | 4.0–5.0 | **10.3–11.9** | 29.5–37.7 |
| auto brush | 22.1–24.4 | 2.2–3.0 | 4.0–5.1 | **10.8–12.1** | 41.0–43.8 |
| **real sidecar** | 74.6–77.8 | 2.2–3.9 | 79.7–94.9 | **10.7–12.7** | 171.7–184.3 |

**⚠⚠ `cov=0.00` FOR A COLOUR RANGE IS NOT FREE COVERAGE — IT IS COVERAGE BEING COMPUTED INSIDE THE
BLEND LOOP.** `Composite::at` runs `prophoto_to_oklab` + `atan2` + `hypot` per pixel there, so the
`blend` column means two different things by mask kind: **68–91 ms of Oklab** for a range, **10–13 ms
of actual lerp** for a brush. Any reading of this table that treats `blend` as one quantity is wrong,
and `cpu_cost` below is shaped by that.

**⚠ The shares invert once the already-ported parts are subtracted.** In the accelerated path the
real sidecar's coverage is ~36 ms (M8.3k measured 31.8–43.5) and its sub-stages ~25 ms (M8.3j), so
the blend is the **largest remaining CPU cost in a mask** — and for a range mask it is essentially
the whole mask.

**⚠⚠ The catalogue is deliberately NOT the argument.** Re-scanned today: 40 sidecars, **1
mask-bearing copy, a Brush with 0 refinements** — no range mask, no gradient, no radial anywhere.
That is unchanged since M8.3k, and `CLAUDE.md` records that this scan **must not be cited as a
prioritisation argument again**, because the user has said plainly he has not been using Rustroom
yet. The case here is structural and is made from the mechanism, not from the scan.

## 2. The structural fact

Today `GpuChain::apply` inside a mask calls `run_with(.., Download::Float)`. So an accelerated mask:

1. uploads the clone,
2. runs the sub-stages,
3. **downloads the whole adjusted frame as floats**,
4. blends on the CPU, reading `base` + `adjusted` + coverage and writing a third image.

There is a full round trip in the middle of every accelerated mask, and a CPU pass over ~3 images
after it. **A mask also ends a GPU run** — `every_tail_stage_is_replaceable_or_a_mask` records that
it is the only thing in the tail that still can.

## 3. Scope (decided)

**Both halves, one resident chain.** Coverage for every kind computed on the GPU into a plane,
composited by `MaskOp`, then the blend as the final dispatch. The clone and the float download both
disappear, and the mask stops splitting the tail.

Rejected: *blend only* (leaves range coverage on the CPU at 68–91 ms) and *coverage only* (pays a new
download to avoid a CPU pass it then still performs).

## 4. Architecture

### 4.1 One `GpuStage`, not a chain of them — and that is forced

`GpuChain` lends **the same** scratch planes to every stage; its own doc says *"nothing written here
survives the call"*. A `base` copy parked in shared planes would be clobbered by the first sub-stage
wanting scratch (sharpening 4, local contrast 5, NR 4).

So `GpuMaskStage` is **one** stage. It asks for `4 + max(sub.scratch_planes())` planes, keeps the
first four (3 for `base`, 1 for coverage) and **lends the tail of the slice onward** to its
sub-stages' `encode`. That stays inside the documented contract — the mask asks for all of them —
while giving the sub-stages exactly the shape they already expect.

### 4.2 What `encode` records, in one compute pass

1. **`seed`** — a copy dispatch, `image → p0..p2`. This *is* the clone, on the GPU.
   ⚠ A dispatch rather than `copy_buffer_to_buffer`, which is an encoder operation and cannot run
   inside a pass — `GpuLensCorrectionStage` seeds with a shader for the same reason.
2. **coverage** — **one** dispatch (see §5.4), writing `p3`.
3. **sub-stages** — each existing `GpuStage`'s own `encode` against the chain's image, which is
   thereby turned from `base` into `adjusted` in place.
4. **`blend`** — `image = p0..p2 + (image − p0..p2) · p3`.

### 4.3 The substitution

`MaskStage` regains its `MaskedAdjustment` beside the `LocalAdjustments` it got back at M8.3j,
behind one accessor, and `accelerate` recognises the stage by name and builds the twin. ⚠ Third time
the *"a `Box<dyn PipelineStage>` reports only `name` and `apply`"* problem has been answered this
way. `MaskStage` is otherwise **untouched** and serves as `GpuStage::cpu()`'s fallback and as the
equivalence reference.

### 4.4 Memory

A plane is `w·h·4`. Rung 3200 (6.5 Mpx): four planes ~104 MB, nine ~234 MB. The fit rung is a
quarter of that. At a native rung the tail is **region-scoped**, so the region rather than the frame
sets the size.

⚠⚠ **The case to watch is export**, which since M8.9 threads `gpu_tail` through and runs the tail
**whole-buffer at full resolution** — 26 Mpx makes nine planes ~937 MB. `GpuImage::new`'s device
ceiling gate degrades that to the CPU twin rather than killing the worker; that gate exists because
M8.3o shipped exactly that crash, and this is the first stage ever to ask for nine planes.

### 4.5 `cpu_cost` is computed, not a constant

⚠⚠ **Mask-kind dependent in a way no other stage's is** — 68–91 ms for a colour range against
10–13 ms for a brush, both at 6.5 Mpx. A single constant would be wrong by ~7× one way or the other.
It is summed from the composite's own components at construction, each rounded **down** — the
conservative direction, since a lower number makes a run less likely to chain and the gate must fail
toward the CPU.

## 5. The kernels

Every constant and every matrix is **substituted from its CPU definition, never transcribed** — the
rule `luma_wgsl` already follows and `oklab_wgsl`'s doc contrasts itself against.

### 5.1 Analytic — `Linear`, `Radial`

Pure functions of `u`, `v`, `aspect`. ⚠ `RadialGradient::coverage` **takes `aspect` and ignores it**,
guarded on the CPU by `radial_coverage_is_independent_of_aspect`; the shader must preserve that, not
"fix" it. ⚠ `FEATHER_EPS` is load-bearing in the radial ramp (`0.0/0.0` at exactly `d == 1.0`) and
carries across.

### 5.2 Luminance range

`lightness(rgb) = linear_to_srgb(luminance(rgb)) · 100`. `luminance` comes from `luma_wgsl`, whose
coefficients are already substituted from `color::PROPHOTO_LUMA`. ⚠ `linear_to_srgb` **clamps its
input to `[0,1]`** — that clamp is the *index* clamp a range mask wants and must not be dropped: a
specular highlight selects at the top of the scale while keeping its headroom.

⚠ The sRGB encode currently lives inline inside `output_transform`'s shader. It is extracted into a
shared `srgb_wgsl` module so the two cannot drift — the `luma_wgsl` / `oklab_wgsl` precedent.

### 5.3 Colour range

`hue_chroma(rgb)` = `prophoto_to_oklab` (from `OKLAB_WGSL`), `atan2(b, a)` in degrees, `hypot(a, b)`.
⚠ `atan2` returns `-180..=180` and **nothing normalises it** — `ColourRange::coverage_at` takes its
span and offset with `rem_euclid(360)`, which is the one wrap rule. WGSL has no `rem_euclid`, so it
is written as `x - floor(x / 360) * 360` and given its own test at negative angles, because the
naive `x % 360` differs in sign there and the failure is a **seam on magentas** rather than an error.
⚠ Chroma is **not** clamped and must not be — it is the gate's own input.

### 5.4 One dispatch for the whole composite, and this is load-bearing

⚠⚠ **`BaseFacts` memoises `lightness` and `hue_chroma` per pixel so that three colour components
cost ONE Oklab round trip, not three** — `docs/BACKLOG.md` puts that round trip at 42% of a range
mask's coverage cost, and the CPU has a dedicated conversion **counter test** because the pixels come
out bit-identical either way and a regression would merely cost more.

A dispatch per component would recompute the facts per component and silently lose that property. So
the whole composite is **one** dispatch whose component chain is **unrolled at shader-build time**,
computing the facts once before the chain exactly as the CPU does. ⚠ The shader text is the program
cache's key, so a varying component count is already handled (M8.3d's rule).

⚠ Recorded honestly: on the GPU this property is **structural rather than guarded** — there is no
observable equivalent to the CPU's counter, and asserting on shader text is the *"parse digits out of
a shader"* anti-pattern `oklab_wgsl` explicitly rejects.

### 5.5 Painted components, and the coverage cache

**Decided: keep the existing CPU-side cache and move the plane across the bus.** A cache hit
**uploads** the stored `CoverageMap`; a miss rasterises on the GPU (M8.3k's path) and **downloads
once** to deposit. At rung 3200 a plane is **6.5 MB as `u8`**, which M8.3g's decomposition puts at
~2 ms — against **31.8–43.5 ms** to rasterise a gating brush. `BrushCoverage`, `MaskCoverageKey` and
the deposit seam in `render_engine` are untouched, so the auto-mask coverage cache's
confirmed-by-use behaviour is preserved exactly.

⚠ Analytic and range coverage have no cache today and simply become dispatches every frame; their
plane never leaves the GPU. A mask with no gating brush deposits nothing today and deposits nothing
after, so nothing changes for those kinds.

⚠ **Backlogged rather than decided here: moving the coverage cache onto the GPU.** It would remove
the ~2 ms transfer entirely by keeping the plane resident between frames, but it needs `GpuCache`
(whose stated rule is *outlive the render*) to hold a keyed image and the deposit/lookup seam to
reach into the GPU context. Folding a caching question into a porting milestone is how this
project's two worst cache bugs happened — the prefix eviction, and the screen-rect tint cache — so it
gets its own measurement later.

### 5.6 The composite algebra

`MaskOp::combine`'s product rules — `Add` `a + (1−a)·b`, `Subtract` `a·(1−b)`, `Intersect` `a·b` —
are substituted, not re-derived. ⚠ They agree with `min`/`max` at 0 and 1 and differ at **every
partial coverage**, and the CPU rule is the brush rasteriser's own algebra; a second spelling here
would disagree with erase-within-a-brush.

⚠ `base_inert_is_enough` has no GPU equivalent and needs none: it is a CPU-side early return taken
before any dispatch is recorded, and `MaskStage` keeps it.

## 6. Equivalence and testing

**⚠ Bit-identity is off the table and the reason is already on record.** The coverage kernels carry
`cbrt`, `atan2` and `pow`; the blend is a multiply-add the GPU fuses (M8.3m's divergence class). The
bound is stated in 8-bit levels per mask kind, measured rather than asserted.

- **`a_gpu_mask_renders_the_same_picture`**, parameterised over **analytic, luminance, colour, brush
  and a composite** — five fixtures, because the kernels are genuinely different code paths and a
  single gradient fixture would leave four of them unexercised.
- **⚠⚠ The carrier, which is what stops this shipping green and dead.** An accelerated mask and a
  CPU mask produce the same picture **by design**, so every correctness test passes with the whole
  substitution deleted — the W5 failure mode. `GpuChain` already counts downloads per chain, so the
  assertion is that a tail containing a mask downloads **once**: that is simultaneously the proof the
  mask ran on the GPU *and* the proof it no longer splits the run.
- **⚠ The `rem_euclid` wrap** gets its own test at negative hue angles (§5.3).
- **⚠ The index clamp** in §5.2 gets a test with a sample above 1.0, asserting the coverage saturates
  while the *pixel* keeps its headroom — the `ProfileHueSatMap` trap, which this repo has now hit
  three times.
- **Sabotages to run and watch fail**, at minimum: the substitution removed (carrier only); the
  radial's ignored `aspect` "corrected"; the `linear_to_srgb` clamp dropped; `rem_euclid` spelled as
  `%`; a `MaskOp` swapped for `min`/`max`; the seed dispatch omitted so `base` reads as `adjusted`;
  the scratch slice handed to sub-stages **not** offset by 4, so a sub-stage clobbers `base`.

**⚠ Three tests lose their wedge and must be re-aimed rather than deleted** —
`every_tail_stage_is_replaceable_or_a_mask`, `a_cpu_stage_between_two_gpu_stages_produces_two_chains`
and `merging_preserves_order` all rely on a mask being the one unreplaceable thing left in the tail.
This is the **fifth** time a property here becomes unconstructible by being fixed (after M8.3e's *"a
tail with nothing replaceable"*, M8.3h's *"a trailing run refused"*, M8.3n's *"a chain behind a CPU
stage"* and M8.3p's vignette). They get a hand-built fixture carrying a deliberately unreplaceable
stage, and each says in its doc what it used to use.

**⚠ The 63 goldens are the guard on the CPU path and are structurally blind to this milestone** —
`grep -rln gpu tests` returns nothing, and no golden has a GPU context. What they prove is that
`MaskStage` was not disturbed.

## 7. Accepted caveats

1. **Not bit-identical**, per §6; the bound is measured per mask kind.
2. **Nine scratch planes at the worst case**, so a full-resolution **export** degrades to the CPU
   twin through `GpuImage::new`'s ceiling rather than accelerating.
3. **`gpu_tail` is still off by default**, so a fresh install sees no change.
4. **The coverage cache costs a ~2 ms transfer each way** where a GPU-side cache would cost none —
   backlogged with its reasoning (§5.5).
5. **The GPU cannot preserve the one-Oklab-round-trip property as a guarded invariant**, only
   structurally (§5.4).
6. **`cpu_cost` is summed from components and rounded down**, so a composite of many cheap components
   is under-reported and stays on the CPU — the safe direction, and real work left unaccelerated.
7. **One machine, one adapter (Apple M2, Metal)**, and the bench machine was **not quiet** (load 5.17).
