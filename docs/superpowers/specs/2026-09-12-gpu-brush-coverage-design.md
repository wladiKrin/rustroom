# M8.3k — the brush coverage rasteriser on the GPU

**Status:** design 2026-09-12. Branch `m8.3k-gpu-brush-coverage`.

## 1. Why this one, and why the other three candidates are not

Measured **2026-09-12, before choosing**, over all 40 sidecars and one grep of the vendored
`wgpu-hal`:

| candidate | measured value on this catalogue |
|---|---|
| M8.3e plan 2 (white balance, contrast, tone curve, saturation) | **ZERO** — 0 of 40 sidecars use any of them globally, and all four are **neutral in the one mask** |
| range-mask coverage cache | **ZERO** — the catalogue contains **no colour-range mask at all** |
| `wgpu::PipelineCache` | **ZERO on this machine, permanently** — `Features::PIPELINE_CACHE` is advertised by the **Vulkan backend alone**; on Metal it cannot be enabled |
| **brush coverage** | **62.3–70.1 ms**, the largest measured column on the one photograph anyone here masks |

⚠⚠ **Three of the four items `CLAUDE.md` listed as "open in Phase 8" are worth nothing here**, and
that was established by measuring rather than by argument. The `PipelineCache` finding is the
sharpest: M8.3f called it *"backend-limited"* and the limit turns out to be *Vulkan-only*, so the
~150 ms first render is not reachable on the developer's own hardware at all.

## 2. What is actually being paid, decomposed

`DSCF6274.RAF`, rung 3200, 3123×2085, CPU, best of 5:

| phase | ms | share of the mask |
|---|---|---|
| **coverage** | **62.3–70.1** | **44%** |
| clone | 2.0–2.3 | 1% |
| sub-stages (M8.3j's column, now ~25 on the GPU) | 71.7–77.7 | 49% |
| blend | 9.1–10.5 | 6% |

Within coverage, from `rasterize_in`'s own readout: **dab splat 52.3–58.0 ms**, auto-mask gate
Oklab conversion **9.7–11.9 ms**, over a box of 1510×2019 (3.05 Mpx), **36 strokes, 3,645 dabs**,
`gate_rows=1718` of 2019.

**⚠⚠ AND IT IS UNCACHEABLE ACROSS A GLOBAL SLIDER BY CONSTRUCTION.** `BrushCache` declines any
brush whose `needs_base()` is true, because the auto-mask gate reads **pixel values** rather than
positions, and the auto-mask coverage cache's key is invalidated by any global edit. So this is
paid on **every frame of exactly the drag people notice** — the one M8.3i was built for and which
the user confirmed as *"visibly quicker"*.

**The case this targets, stated as a number:** a global slider drag on that photograph is
**~132 ms** today (67.87 ms cache-warm frame + ~64 ms of re-rasterised coverage) and should reach
**~77 ms**.

⚠ M8.3i already took this column **2.9×** (228 → ~64) by making the dab loop a parallel row
gather. **A further attack therefore needs a different mechanism, not more of the same** — which is
why this is the GPU rather than another CPU pass.

## 3. The crux: a per-pixel gather needs a spatial index

On the CPU each **row** walks the dabs that touch it. A GPU kernel is per **pixel**, and a pixel
cannot afford to test all 3,645 dabs — 3.05 Mpx × 3,645 is **11.1 billion** tests. So some spatial
index is needed.

**⚠⚠ ONE DISPATCH, WITH THE INDEX BUILT IN WORKGROUP-SHARED MEMORY — no second pass, no CPU
binning, and no global atomics.** One workgroup owns one **16×16 tile** (256 threads):

1. The workgroup's threads **cooperatively scan the dab list once**, appending the dabs whose
   bounding box intersects this tile to a `var<workgroup>` array, via a workgroup-local
   `atomicAdd` counter.
2. `workgroupBarrier()`.
3. Each thread rasterises **its own pixel** from that shared list.

**The arithmetic is what makes this the right shape.** A tile scanning every dab is
11,900 tiles × 3,645 dabs = **43.4 million** bounding-box tests — three orders of magnitude below
the per-pixel figure, and a quarter of the work a separate binning pass would do twice over.

⚠ **The shared list holds `u32` dab indices and `Limits::default()` guarantees
`max_compute_workgroup_storage_size = 16384` bytes** — verified in `wgpu-types`, and
`GpuContext` requests `DeviceDescriptor::default()`, so it gets exactly those defaults. That is
**4,095 indices** after the counter. Average occupancy on the real brush is
3,645 × 72 / 11,900 ≈ **22 dabs per tile**; a heavily over-painted tile might reach a few hundred.

⚠ **Overflow needs no CPU fallback.** A tile whose list fills simply **re-scans the whole dab list
per pixel** — correct, slower, and confined to that tile. That is strictly better than the CPU
fallback an earlier draft of this spec proposed, and it removes the "silent performance cliff"
caveat entirely.

⚠⚠ **Dabs are stored stroke-major, so an ascending index IS stroke order** — which is what lets
the shared list stay a plain sorted index list rather than carrying stroke boundaries. The kernel
walks it, tracks the current stroke from each dab's `stroke` field, and composites when that
changes. That reproduces §4's two-level rule exactly.

## 4. The semantics that must survive, and they are not obvious

`CLAUDE.md`'s M8.3i section records these as load-bearing; **a naive port destroys two of them.**

- **⚠⚠ TWO LEVELS: `max` WITHIN a stroke, then COMPOSITE across strokes in order.** Across strokes
  it is `cov += (1 − cov)·a·flow`, and erase is `min(cov, 1 − a)`. `min`/`max` between strokes
  agrees at 0 and 1 and differs at **every partial coverage**, so flattening the two levels is a
  different picture wearing the same name. Per pixel the kernel therefore loops **strokes in
  order**, taking a running `max` over that stroke's dabs, then composites once.
- **⚠⚠ THE SHORT-CIRCUIT BEFORE THE GATE IS LOAD-BEARING AND MUST BE PORTED, NOT DROPPED.**
  `if a <= running_max { continue; }` sits *above* the auto-mask gate, and is sound because the
  gate can only ever **reduce** an alpha. Dabs are laid a quarter-radius apart and overlap heavily,
  so most lose. ⚠ M8.3i states the consequence directly: *"a gather that evaluated the gate for
  every dab touching a pixel would be correct and dramatically slower, and every test would still
  pass."* It survives the port unchanged — a per-pixel running max is exactly the same algorithm.
- **⚠ An inverted brush with no strokes covers EVERYTHING**, so `outside` is 255 not 0, and
  `is_inert` must keep meaning what it means.

## 4.1 The four functions the shader must reproduce, read out of the CPU source

Transcribed rather than paraphrased, because every one of them is a place a port drifts:

```
dab_alpha(d, r, hard):          q = d/r;  q >= 1 -> 0
                                hard >= 1 - 1e-4 -> 1          // fast path only, not a NaN guard
                                t = clamp((1-q)/(1-hard), 0, 1);  t*t*(3-2t)

auto_mask_ceiling(tol):         t = clamp(tol/100, 0, 1);  0.01 + t*(0.35 - 0.01)

auto_mask_gate(px, ref, ceil):  d = |px - ref| in Oklab (all THREE components, lightness included)
                                lo = ceil * 0.5
                                d <= lo -> 1;  d >= ceil -> 0
                                q = (d-lo)/(ceil-lo);  1 - q*q*(3-2q)

composite (MaskOp):             Add      -> c + (1-c)*a        // "over"
                                Subtract -> c * (1-a)          // erase
                                a = stroke_max * flow, skipped when a <= 0
```

Final quantisation is `(clamp(c, 0, 1) * 255 + 0.5) as u8`, with `invert` applied as `1 - c`
**before** the clamp.

⚠⚠ **`auto_mask_gate` uses the FULL Oklab distance, lightness included, and that is load-bearing
rather than incidental.** Its doc records why: an earlier design stored the reference in the stroke
and had to drop the lightness term, because a stored reference is sampled from the *displayed*
frame while the pixels come from `MaskStage`'s input — measured at a mean lightness disagreement of
**+0.07 to +0.11**, 7–10× the chroma disagreement. Both ends are read from one buffer here, so the
shader must keep all three components.

⚠ **NaN reaches 0 through the CAST, not the clamp** — `f32::clamp` propagates NaN while Rust's
float-to-int casts saturate. WGSL has neither, so the shader tests `c != c` explicitly, exactly as
M8.3g's quantiser does.

⚠ **WGSL's `round` breaks ties to even; Rust's `f32::round` breaks them away from zero.** M8.3g
records this as an off-by-one-level bug on every exact tie. The `+ 0.5` then truncate above is
already the safe spelling and must be kept — do not "tidy" it into `round`.

## 5. The buffer is already resident, and that is the design's one real economy

M8.3j uploads the mask's image for the sub-stage chain. **The auto-mask gate reads the same
pixels** — `BaseView` wraps the incoming `image`, and `adjusted` starts as a byte-for-byte clone of
it — so coverage and sub-stages can share **one** upload.

Order inside `MaskStage::apply` is coverage → clone → sub-stages, so the upload happens for
coverage and the chain then finds it resident. ⚠ That is the whole reason to do this *after* M8.3j
rather than before.

**The transfer budget**, from M8.3g's measured components at rung 3200: upload 7.58 ms (shared, so
**zero marginal**), coverage-map download **~1.9 ms** (6.5 MB at u8, against the 6.8 MB row's
1.89 ms). So the round trip costs ~1.9 ms marginal against ~64 ms of work.

## 5.1 ⚠⚠ It is NOT a `GpuStage`, and that was checked rather than assumed

`GpuStage::encode` takes `img: &GpuImage` and produces an image in place; its `scratch` is
`GpuImage::scratch(ctx, w, h, n)` — **image-sized single-channel f32 planes**. A rasteriser needs
a **variable-length dab buffer**, a **tile index**, and it produces a `u8` **coverage map** rather
than an image.

So `GpuBrushRaster` is its own type with its own buffers and its own readback — closer to
`GpuChain::run_rgba8` than to a stage. ⚠ That is the same reasoning `GpuStage` itself rests on:
*"deliberately not `PipelineStage` — that trait's signature forces a round trip"*. Forcing this
into `GpuStage` would mean sizing scratch by the image for data that is not image-shaped.

## 6. Scope — two plans, and plan 1 ships nothing user-visible

**Plan 1 — headless.** `GpuBrushRaster` in `gpu-pipeline`: the binning pass, the rasterise pass,
the gate, and a `CoverageMap` that agrees with `rasterize_in`. Proved against the CPU rasteriser on
fixtures and on the real 36-stroke brush, with **no `MaskStage` change at all**.

**Plan 2 — wiring.** `MaskStage` uses it when a context is available, sharing M8.3j's upload.

⚠ The split is M3.1a's and M8.3i's: prove the maths where a canvas bug cannot be confused with a
model bug. Plan 1 is where every semantic in §4 is pinned.

## 7. Equivalence — and why it cannot be bit-identical

M8.3i's guard is a **frozen sequential copy** of `rasterize_in` compared **bit for bit**. That
standard cannot hold here: the gate converts to Oklab, which means `cbrt` and a matrix on the GPU,
and M8.3a established that GPU and CPU float differ (subnormals flush, transcendentals differ in
the last bits).

So the guard is the **coverage map in 8-bit levels**, the unit `compare` already uses — coverage
*is* a `u8`, so "within 1 level" is a statement about the stored value rather than about a
rendering of it.

| guard | what it catches |
|---|---|
| GPU map vs `rasterize_in`, **all 3.05 Mpx**, on the **real 36-stroke brush** | any semantic drift at scale |
| a two-stroke fixture where stroke order changes the answer | the two-level combine flattened to `min`/`max` |
| an **erase** stroke over a paint stroke | the erase rule |
| a brush with `invert` and no strokes | `outside` = 255 |
| a dab count per tile exceeding the shared-list capacity | the overflow path — it must **re-scan per pixel**, never clip silently. ⚠ Reachable only by forcing the capacity down in a test, since no real brush comes near 4,095 |
| a **timing** assertion | **none — deliberately.** A benchmark is not a test; this project records that twice |

⚠⚠ **The last row of §4 is the one a test cannot see**: dropping the short-circuit is *correct* and
merely slow, so **every equivalence test passes against it**. The guard is a **dab-test counter**
behind `#[cfg(test)]` — the `PARALLEL_STROKES` precedent — asserting the gather does far fewer gate
evaluations than dabs-touching-pixels. **An observable describing the current state cannot see its
own absence; it has to count events.**

## 8. Accepted caveats, stated in advance

1. **Not bit-identical to the CPU rasteriser** — a real change of standard from M8.3i, argued in §7.
2. **The shared list has a fixed capacity (4,095 dabs per 16×16 tile)**; a tile that overflows
   re-scans the whole dab list per pixel. Correct, slower, and confined to that tile — so it is a
   local slowdown rather than the silent cliff a CPU fallback would have been.
3. **Nothing for a plain (non-auto) brush that `BrushCache` already serves** — that path is a cache
   hit and pays nothing today.
4. **Nothing for the analytic masks**, whose coverage is already 0.00 ms.
5. **`gpu_tail` is still off by default**, so this reaches nobody who has not found the checkbox.
6. One machine, one adapter (Apple M2, Metal), one rung, one photographer's single masked brush.
