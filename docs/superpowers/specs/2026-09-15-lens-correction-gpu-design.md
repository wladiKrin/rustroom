# M8.9 — lens correction on the GPU, aimed at export

**Date:** 2026-09-15
**Status:** approved, two plans

## 1. What this is, and what re-priced it

The agreed systematic order names item 3 as *"port lens correction to the GPU"*, on two facts that are
both still true: it is the largest single stage anywhere in the prefix, and it is **on by default for
every RAW**, so every photographer pays it on every photograph.

⚠⚠ **What changed is where the value is.** M8.8 gave `LensCorrectionStage` an
`input_rect_for`/`apply_region` pair, so a cold entry into 1:1 now scopes the whole prefix chain —
**1689–1869 → 218–225 ms** — and lens correction is **118 ms of that 221** rather than the ~696 ms it
was whole-buffer. It is still 54% of the scoped chain, and an order of magnitude less absolute time.

**So the port's value moved to EXPORT.** Export renders whole-buffer, sequentially, once per photo:

| | |
|---|---|
| `lens_correction` alone, 26.0 Mpx, whole buffer | **650.2–695.8 ms** |
| …of which the displacement **field** | 35.6–36.1 ms (6%) |
| …of which the **resampling** | **597.5–646.8 ms (94%)** |

Re-measured at full resolution for this milestone: **stage 717.4 ms, inner loop 671.6, field 38.3,
resampling 633.3.** The port attacks the 94%.

## 2. ⚠⚠ Export does not touch the GPU seam at all

Found by grepping for `accelerate_prefix_stage`'s callers *before* designing anything:

> `batch_export::export_one` builds a plain CPU pipeline through `build_pipeline_spec` and renders
> it. `accelerate_prefix_stage` has **exactly one caller**, `render_engine.rs:1225`, the preview
> worker — and only on the unscoped path.

**So "port lens correction, aimed at export" is two pieces of work, not one.** Hence two plans.

### 2.1 ⚠⚠ AND IT IS WORSE THAN THAT: THE GUI'S EXPORT APPLIES NO LENS CORRECTION AT ALL

Found while reading `export_one` to plan the wiring, and **verified end to end rather than by
reading**. `export_one` builds through `pipeline::build_pipeline_spec`, which hardcodes
`lens: None` — and `decode_for_export` discards the metadata, keeping only the white-balance
baseline, so there is no `LensShot` in that path to pass even if it did.

Three renders of one frame agree that this is real:

| path | lens correction |
|---|---|
| the preview (`render_cached` → `build_prefix_steps(.., lens, ..)`) | **applied** |
| `rustroom render` (its own `build_pipeline_split_in_region_with_brush(.., lens, ..)`) | **applied** |
| **the GUI's Export dialog** (`export_one` → `build_pipeline_spec`) | **not applied** |

Measured on `DSCF6296.RAF`, XF16-80 at 19.2 mm f/8 — a *mild* case, since barrel distortion and
vignetting are both worst at the wide end and wide open. Corrected against uncorrected, through the
shipped CLI at full resolution:

> **worst 187 of 255, mean 6.95, and 33.5% of samples differing by more than 4 levels.**

⚠ `lens_diff`'s radial bands read ~1.00 at this aperture, so the **vignetting** half is small here;
what the numbers above are mostly showing is the **geometry**, which that probe structurally cannot
see. The recorded worst displacement on this lens is **50.59 px**.

**So the milestone's framing is corrected: plan 2 is not "light up the GPU for export", it is
"lens correction must reach export at all, and then cross the seam".** You cannot accelerate a stage
that does not run.

⚠⚠ **A consequence stated rather than discovered: this makes every RAW export ~700 ms/photo slower
and changes the pixels of every exported RAW.** Both are correct — the export is supposed to match
the preview — and the GPU port is what pays the time back. It is the reason the two halves belong in
one milestone.

## 3. Plan 1 — `GpuLensCorrectionStage`

The third arm on `accelerate_prefix_stage`, beside `dehaze` and `noise_reduction`.

### 3.1 The field travels as a coarse displacement grid

⚠⚠ **This is the milestone's central mechanism, and it was settled by measurement rather than by
argument.** The alternative was reimplementing lensfun's distortion model in WGSL — a second
definition of the correction that can disagree with `Corrector::fill_row`, made harder by
`Modifier`'s `norm_scale`/`center_x`/`center_y` being **private**.

Instead the CPU builds the field it already knows how to build, on a coarse lattice, and the shader
interpolates bilinearly between grid points. `probe_lens`'s sweep, on a 6246×4170 frame:

| step | points | MiB | build | worst source error | worst gain error |
|---|---|---|---|---|---|
| per-pixel | 26.0 M | **695** | 38.3 ms | 0 | 0 |
| 8 | 408,146 | 10.9 | 2.6 ms | 0.0007 px | 0.000000 |
| **16** | **102,704** | **2.74** | **1.4 ms** | **0.0027 px** | **0.000000** |
| 32 | 25,905 | 0.69 | 0.8 ms | 0.0111 px | 0.000000 |
| 64 | 6,536 | 0.17 | 0.5 ms | 0.0348 px | 0.000000 |

**2.74 MiB against 695 MiB — 0.9% of the 298 MiB image.**

⚠⚠ **CORRECTED IN PLACE AFTER THE SWEEP: the step ships at 8, not 16.** See §3.6 — the measurement
that decides it is *output levels per pixel of lattice error*, and it came out at **~1000, where this
spec estimated ~250**. Step 8 is 10.9 MiB and 2.6 ms, still noise against a 298 MiB image and a
650 ms stage, and four times more accurate.

⚠ **And a second, unlooked-for win: building the grid at step 16 costs 1.4 ms against the full
field's 38.3**, because it calls `fill_row` only every 16th row.

### 3.2 ⚠⚠ It stores DISPLACEMENT, and the reason is PRECISION rather than interpolability

Bilinear interpolation is exact for the linear part of the field either way — a grid of absolute
source positions interpolates just as well as a grid of displacements. What differs is **f32
resolution**: a coordinate near 6246 resolves to ~0.0005 px, while a displacement near 50 resolves
to ~4e-6. The residual after subtracting the identity is small, so it keeps far more of its mantissa.

The shader reconstructs `source = pixel + interp(displacement)`.

### 3.3 The vignetting gain rides the same grid

A seventh component beside `[dxR, dyR, dxG, dyG, dxB, dyB]`. **Exact to six decimals at every step
tested** — the gain field is far smoother than the displacement field, so nothing is given up.

### 3.4 ⚠ The resampler weights are the CPU's own LUT, uploaded

`Weights` gains a `lut()` accessor. The rule `ToneCurveStage::lut` and `ColorGradingStage::lut`
already follow: **substituted, never transcribed**, so the two cannot disagree about a kernel nobody
re-derives.

⚠⚠ **`Resampler::Bilinear`'s LUT is deliberately EMPTY** — its own comment says it is computed
exactly, because the table quantises the phase and `1.0 - (1.0 - f)` is not `f` in f32. So the shader
needs a bilinear branch, or bilinear alone diverges while every other kernel agrees. That hazard gets
its own test.

### 3.5 ⚠⚠ THREE scratch planes — corrected in place, this said "none"

It is a **gather**, and that is precisely why it *cannot* be in place: an output pixel reads the
source anywhere within the resampler's support, and those pixels may already have been overwritten.
Every stage before this one touched only its own pixel, which is what made the chain's one-image
contract hold.

`scratch_planes()` returns **3**, one plane per channel. Scratch is **single-channel** by
construction (`width * height * 4` bytes), which is exactly the deinterleaved layout a per-channel
sampler wants; the chain allocates the widest request once and shares it; and `encode` is handed a
`&mut ComputePass` and **cannot issue a buffer copy**, so a stage-owned source buffer would need a
copy kernel anyway. A `seed` dispatch then a `gather` dispatch, in one pass, on the in-pass memory
barrier M8.3b established by measurement.

The grid and the LUT remain ordinary uploaded buffers.

### 3.6 ⚠⚠ The grid step is SWEPT against OUTPUT 8-BIT LEVELS — and the sweep moved it

**Recommended 16; measured, and it ships at 8.** The sweep, Catmull-Rom on a 200×160 hard grid chart
with a synthetic `b = -0.05`, each step's lattice error taken from `lens_grid`'s own sweep:

| step | lattice error | worst output levels |
|---|---|---|
| 1 | exact at every pixel | **0.00009** |
| 2 | 0.0017 px | 2.57 |
| 4 | 0.0069 px | 7.68 |
| **8** | **0.028 px** | **28.4** |
| 16 | 0.111 px | 94.7 |
| 32 | 0.459 px | 221.0 |

⚠⚠ **That is about 1000 output levels per pixel of lattice error, where this spec estimated ~250.**
A hard 0.1/0.9 edge resampled through a kernel that overshoots produces gradients far steeper than
the 156 levels/px an edge naively suggests. Carried to the real XF16-80 — 0.0007 px at step 8 against
0.0027 at step 16 — that is roughly **0.7 levels against 2.7**, on a chart far harsher than any
photograph.

⚠ Step 8 costs **10.9 MiB and 2.6 ms** against step 16's 2.74 MiB and 1.4 ms. Both are noise against
a 298 MiB image and a 650 ms stage, so there is nothing to trade.

⚠⚠ **And the step-1 row is the milestone's sharpest measurement: with the lattice exact, the shader
agrees with the CPU to 0.00009 of a level, on all four kernels.** So everything above that row is
interpolation and none of it is the shader — which is what separates the two questions and is why
the equivalence tests run at step 1 while the lattice gets a sweep of its own.

⚠ This is deliberate, and the reason is recorded twice already in this file: `MIN_GPU_BOX_PX` was low
by ~8× and `MAX_TILE_DABS` sat past an 8.7× occupancy cliff — **both were reasoned about instead of
swept.** This is the third, and the sweep disagreed with the reasoning again.

### 3.7 Device limits

⚠⚠ **Read the device-limit section before writing a line.** Export runs whole-buffer at 26 Mpx, so
this is the **third stage ever** to allocate a full-resolution GPU image — **298 MiB in and 298 MiB
out**. Both are already handled rather than newly risky: `GpuImage::new` refuses past
`ctx.max_image_bytes()` (the `min` of `max_buffer_size` and `max_storage_buffer_binding_size`, 4 GiB
on an M2 since the adapter's own limits are requested), and `guarded` turns any residual wgpu panic
into a CPU fallback. What this milestone owes is to **exercise it at 1:1 and in a real export**, not
to build new machinery.

### 3.8 The forcing function

`every_other_prefix_stage_is_left_alone` currently names the three resamplers as the stages with no
GPU twin. **Adding an arm breaks it**, which is exactly what makes this a deliberate act rather than
a silent one — the same shape that fired when noise reduction landed.

## 4. Plan 2 — lens correction reaches export, and crosses the seam

First `export_one` runs the prefix as **steps** with the lens shot threaded through, the way
`render_cached` and the CLI both already do — which is the §2.1 fix. Then it offers each step to
`accelerate_prefix_stage`, exactly as `render_engine` does.

### 4.1 ⚠⚠ `gpu_tail` is threaded as a PARAMETER, on a written-down precedent

The user's choice — reuse `gpu_tail` rather than add an `ExportOptions` field — lands on the shape
that already exists. `ExportWorker::spawn`'s doc comment says why, about `resampler`:

> ⚠ `resampler` is a parameter rather than a field of `ExportOptions`, and that is the whole reason
> there is one setting instead of two: in `ExportOptions` it would be saved into every export preset,
> so a preset could disagree with the preview about which kernel the photograph is rendered with.
> **Two facts that can disagree.**

So `gpu_tail` travels `start_export → spawn → run → export_one`.

⚠⚠ **The naming hazard, flagged and accepted.** That setting is named for the *tail*; it already
governs the preview's prefix seam, and it will now govern export too. The Settings hint has been
corrected for exactly this three times (`gpu_hsl → gpu_tail`, the Effects edit group, the enumerating
hint). **It gets reworded to describe the rule and not enumerate** — a hint that lists a growing set
goes silently wrong.

### 4.2 Dehaze and noise reduction light up for free

Both are already ported, and neither has ever run on the GPU during an export. That is what makes
plan 2 worth landing on its own merits even if plan 1 slipped.

## 5. Accepted caveats, stated up front

1. **The scoped preview path stays on the CPU by design.** `render_engine.rs:1188` says so: the
   seam's twins run whole-buffer, so a scoped chain is the CPU's. The preview gains only on
   **unscoped** renders — the fit rung, the warm-behind, and cold entries the area gate refuses.
2. **Not bit-identical, and this is the least bit-faithful GPU stage in the crate.** Per-pixel
   stages agree to about a hundredth of a level; with the lattice exact this one does too
   (0.00009), but at the shipped step 8 a real lens costs roughly **0.7 of a level** on a hard
   edge — FMA contraction and subnormal flush-to-zero on top. A resampler interpolating a field is
   inherently a different bargain from a per-pixel multiply, and it is stated rather than glossed.
3. **298 MiB in, 298 MiB out** at 26 Mpx, degraded to the CPU rather than to a dead worker.
4. **`gpu_tail` now governs three things under a name that says "tail".**
5. **`gpu_tail` is still off by default**, so nobody gets a rendering change they did not ask for.
6. **Every exported RAW changes**, because §2.1 makes export apply a correction it was silently
   skipping. That is the fix, not a side effect — but it is a visible change to anyone who has
   exported from the GUI before today.
7. One machine, one adapter (Apple M2, Metal), one lens profile.

## 6. ⚠⚠ No speed-up is projected, deliberately

Composing component measurements has missed in this crate **three times** — M8.3g over-predicted a
whole frame by ~2×, M8.3f was right in magnitude and wrong by 8× in attribution, M8.3o was optimistic
by ~18%.

For scale only, and **as a projection rather than a result**: composing M8.3g's decomposed transfer
costs to 26 Mpx suggests roughly **85–120 ms of round trip** against 671.6 ms of CPU work. That
number is not a claim. **The last task of each plan measures the real thing**, and the spec carries no
predicted ratio.
