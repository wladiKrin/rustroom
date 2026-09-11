# M8.3g — the 8-bit readback

**Date:** 2026-09-11
**Status:** agreed, ready to plan

## 1. What this is

When the accelerated tail ends in a GPU chain, that chain's result is **display-referred `[0,1]`**
— `output_transform` is the last stage of every tail, and it clamps. So the value `pack` is about
to quantize already exists on the GPU in a form that fits in a byte.

Today the chain downloads **82 MB of f32** at rung 3200 and the CPU then quantizes it to 27 MB of
RGBA8. This milestone makes the chain's **last dispatch** do the quantization and download the
**27 MB** instead.

⚠⚠ **The numbering shifts again.** M8.3f was the resource cache, so this takes **M8.3g** and the
mask loop becomes **M8.3h**. `CLAUDE.md`'s NEXT ACTION block calling the mask loop M8.3g is stale
the moment this lands.

## 2. The measurement, and what it corrects

`CLAUDE.md`'s NEXT ACTION records this as *"4x less readback"*. Measured, it is **2.8x**, because a
download has a fixed cost no size reduction can touch. Decomposed with `probe_breakdown`
(3200x2136, best of 5, each step on its own submit so it can be timed separately):

| step | ms |
|---|---|
| staging memcpy (CPU) | 2.20 |
| upload copy (device) | 7.58 |
| **four dispatches** | **11.61** |
| download f32: copy+map 6.33 + `to_vec` 6.87 | **13.20** |

And the download's scaling, which is the number the whole feature rests on:

| readback | copy+map | `to_vec` | total |
|---|---|---|---|
| 82 MB (f32, today) | 6.33 | 6.87 | **13.20** |
| **27 MB (RGBA8)** | 2.59 | 2.19 | **4.78** |
| 6.8 MB | 1.32 | 0.57 | 1.89 |
| ~256 B | 1.28 | 0.00 | **1.28 — the fixed cost** |

So **8.4 ms** off the download, plus `pack`'s `to_rgba8` (**~2.2 ms** at a native rung, measured
when that pack was parallelised) which **disappears** rather than moving. About **10.6 ms** off a
42.3 ms TYPICAL frame at rung 3200 — projected **1.88x -> ~2.5x**.

⚠ That is a projection assembled from component measurements, exactly the kind of thing this
file's own record says to distrust: M8.3e's "~10 ms of per-encode overhead" was a consistent
explanation that turned out to name the wrong object. **The plan's last task measures the whole
frame on the real engine**, and that number is what gets recorded.

⚠ Two things this does **not** touch, stated so nobody looks for them later. The **dispatches**
(11.61 ms) are real per-pixel work on 6.8 Mpx and are not addressed here. The **upload half**
(9.78 ms) cannot shrink at all: the input is scene-referred f32 with headroom above white, and
WebGPU forbids `MAP_WRITE | STORAGE`, so there is no zero-copy path to find — which retires the
question M8.3a left open.

## 3. The gate

Three things downstream still need the float, and each is a **view mode or a setting**:

1. **A non-sRGB display space.** `pack` converts from the *float* deliberately — its own comment:
   *"An 8-bit -> 8-bit conversion compresses the range and costs roughly 0.8 bits, which shows as
   banding on a smooth sky."* ⚠ Opt-in and Linux/Windows-only: on macOS `tag_as_srgb` succeeds, so
   `effective_display_space` returns `Srgb` whatever the dropdown says.
2. **Visualize Spots.** `edge_image` is computed from the float.
3. **Soft proofing.** The second ProPhoto tail is decoded back to the working space to test the
   gamut, which an 8-bit round trip cannot support.

**Bytes only when all three are absent** — which is the default state, and always true on this
machine. Otherwise the f32 readback, byte-for-byte as it is today: the GPU tail still runs, only
the download stays wide.

⚠ The **grade range view** is a fourth float consumer and needs no work at all: it already bypasses
`accelerate` entirely, building its own halves through `build_pipeline_split_tapped`. Checked in
the code rather than assumed.

⚠⚠ A four-part condition spanning two crates is exactly the shape this project's record says goes
wrong silently. Two mitigations, both structural rather than remembered: the "is the chain last and
does it end in `output_transform`" half is answered **by construction** inside `accelerate` (S4),
and the "does anything need the float" half is derived **once** in `render_cached` and passed down,
never re-derived.

**Rejected:** *don't accelerate at all when the float is needed* — one decision in one place and a
structural invariant, but it costs a Linux user with a P3 monitor the **whole** GPU win (1.88x back
to 1.00x) rather than just the readback, and it would silently switch acceleration off under
Visualize Spots. **Rejected:** *move the display transform onto the GPU* — it removes condition 1
and neither of the others, so the gate survives anyway, and it relocates a step `pack` deliberately
owns: the histogram must be image-referred and is taken *before* the display transform, which is an
ordering that has already shipped wrong once.

## 4. The seam

`accelerate` returns `(Pipeline, Option<GpuChain>)`: the pipeline **without** its final chain, plus
that chain — `Some` only when the chain genuinely is the tail's last stage **and** its own last
stage is `output_transform`.

⚠⚠ **The condition is answered where it is provable.** `accelerate` is the only code that knows how
the tail was regrouped; a caller asking "is the last stage a chain?" would have to downcast a
`Box<dyn PipelineStage>`, and a caller *assuming* it would be wrong exactly when the cost gate
refused the final run. The name is checked rather than inferred from "`output_transform` is always
last", which is true today and is an assumption rather than a guarantee.

`render_cached` then runs the returned chain one of two ways — `run_rgba8` under the gate, `apply`
otherwise — and `rejoin(pipeline, chain)` puts it back for the paths that want one plain
`Pipeline` (the proof tail, and the gated-off case).

**Rejected:** *a method on `PipelineStage`*. It reads better at the call site and it leaks an
output-encoding concern into the seam trait `CLAUDE.md` keeps deliberately free of them, giving
every stage in the workspace a method that is `None` for all but one of them.

**Rejected:** *expand the bytes back to f32 inside the chain*. The only option needing no API change
at all, and measured it gives back most of what it saves: `pack`'s 2.2 ms quantize stays **and** a
27 MB -> 82 MB expand pass is added.

## 5. The shader

A final dispatch, appended by the chain only in bytes mode, reading the resident f32 image and
writing `u32`-packed RGBA8 into its own `w*h` buffer. `GpuOutputTransformStage` is **unchanged**, so
its equivalence tests keep testing what they were written for.

⚠⚠ **`floor(x * 255.0 + 0.5)`, never `round`.** WGSL's `round` breaks ties to the **even** whole
number; Rust's `f32::round`, which `ImageBuf::quantize` uses, breaks them **away from zero**. And
`x * 255` lands on exact halves routinely — `0.5/255`, `1.5/255`, and so on — so a transliterated
`round()` is off by one level on every tie while being correct everywhere else. ⚠ A fixture that
never lands on a half **cannot see this**, which is why S6 names the fixture rather than the
assertion.

⚠ Alpha is 255. The quantizer must reproduce `quantize`'s whole behaviour, not only its rounding:
`clamp(0,1)` first, and **NaN must reach 0** — on the CPU that happens through the saturating
`as u8` cast rather than through the clamp, which returns NaN for NaN.

## 6. `pack`

`pack` takes `Packed::{Float(ImageBuf), Rgba8 { bytes, w, h }}`.

⚠ The `Rgba8` arm needs **no `ImageBuf` at all**, which is what makes it small: the histogram
already comes from the quantized bytes (`hist` is taken from `out.to_rgba8()` today, deliberately,
so that it describes the image rather than the monitor), and `srgb` and `rgba` become **one shared
allocation** — precisely the existing `display_space == Srgb` branch.

⚠ The three float-only features are **unrepresentable** on that arm rather than guarded against:
`Packed::Rgba8` is constructible only where the gate held.

## 7. Testing

**The one that matters** — `the_gpu_bytes_are_what_the_cpu_would_have_quantized`: the chain's
RGBA8 output **byte-identical** to `to_rgba8` of the same chain's f32 output. ⚠ Its fixture is the
test, not the assertion: it must carry **exact quantization halves** (`k/510` for odd `k`), a
negative, a value above 1, and a NaN. Byte-identical rather than within a level, because both sides
are a defined rounding of the same number and a tolerance would hide exactly the tie bug S5 names.

- **The gate, one test per condition**, each asserting the *float* path is taken — and a fourth
  asserting bytes are taken when none is active, since three "does not" tests pass against an
  implementation that never takes the byte path at all.
- **`accelerate`'s split**: `Some` when the tail ends in a chain ending in `output_transform`;
  `None` when a CPU stage follows the chain; `None` when the final run was refused by the cost gate.
- **The carrier** — a frame rendered through the byte path is identical to the same frame rendered
  through the float path. ⚠ Without it every test above can pass while nothing reaches the texture,
  which is how creative LUTs shipped fully tested and 100% dead.
- ⚠ **63 goldens must stay byte-identical.** They never touch this crate, so movement means
  something leaked out of the gate.

## 8. Accepted caveats

1. **No win with a non-sRGB display space, under Visualize Spots, or while soft proofing** — the
   gate, stated as a cost rather than discovered as one.
2. **Nothing for export.** `batch_export` never calls `accelerate`, and ~93% of an export is
   libraw's demosaic upstream of everything wgpu touches.
3. **`gpu_tail` is still off by default.**
4. **The dispatches (11.61 ms) and the upload (9.78 ms) are untouched**, and the upload cannot be
   improved at all — S2.
5. **The projection is assembled from components** and is not yet a frame measurement.
6. Measured on one machine, one adapter (Apple M2, Metal), at one rung.
