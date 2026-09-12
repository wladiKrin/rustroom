# M8.3l — the mask's shared upload

**Date:** 2026-09-12
**Branch:** `m8.3l-shared-upload`
**Status:** design

## The problem, measured

A masked frame with an auto-masked brush uploads **the same 78 MB plane to the GPU twice**.

`MaskStage::apply` does, in order:

1. `build_map` — hands `GpuBrushRaster` a `BaseRef { plane: image.data(), w, h, x0, y0 }`. The
   rasteriser uploads that plane so the auto-mask gate can read it.
2. `let mut adjusted = image.clone();`
3. `for stage in &self.stages { adjusted = stage.apply(adjusted) }` — and since M8.3j the first
   sub-stage is a `GpuChain`, which uploads `image.data()` again, byte for byte.

The two planes are the same bytes in the same layout. `ImageBuf::data()` is `w*h*3` f32; so is
`GpuImage`, whose usages (`STORAGE | COPY_DST | COPY_SRC`) are a superset of what the gate needs.
No conversion stands between them.

### What the second upload costs

`RUSTROOM_DEBUG_MASK` now splits `GpuBrushRaster::run()` three ways, and a one-off probe put the
device-side copy on its own submit. Real brush, `DSCF6274.RAF`, rung 3200, box 1510×2019, 3645
dabs, 78.1 MB base, warm rounds:

| phase | with the base | without |
|---|---|---|
| host memcpy into staging | 2.06–2.53 ms | 0 |
| device `copy_buffer_to_buffer` (own submit) | 6.19–13.93 ms | 0 |
| compute + readback copies + `poll(Wait)` | 21.5–27.1 ms | 13.8–16.5 ms |
| readback + narrowing | 1.36–1.61 ms | 1.18–1.22 ms |

⚠ Round 0 is discarded throughout: the first staging take costs 18.3–18.7 ms rather than ~2.1,
because it allocates the pooled buffer.

So one upload is **~8–12 ms**: ~2.1 ms of host memcpy plus ~6–10 ms of device copy. Deleting the
chain's upload should take the mask's `sub=` phase from ~24–27 ms to ~12–15 ms.

⚠ **The ~16 ms gap between the base and no-base columns is NOT all upload, and the first reading of
it said so wrongly.** Splitting the submit showed the gate's own per-pixel Oklab work is ~5–7 ms of
it. Only the upload half is recoverable here; the gate is real work and stays.

## Why the opportunity is real on this catalogue

The catalogue's one masked photograph carries a brush mask with 22 of 36 strokes auto-masked, and
its `LocalAdjustments` fuse into **one** `GpuChain` (`RUSTROOM_DEBUG_MASK` prints `stages=1` on the
accelerated render against `stages=2` on the CPU one). So the chain *is* `stages[0]`, and the base
plane *is* its input. Both preconditions hold on the only photograph that can exercise this.

Honest frame budget on a global drag, after the bench fix below: **126.4–131.5 ms**, of which
coverage is 31.8–43.5 ms and the sub-stage chain 23.6–29.8 ms. Removing ~10 ms is ~8% of the frame.

## ⚠⚠ A prerequisite that came first: the bench was lying about this exact row

`bench_gpu_tail`'s `REAL SIDECAR, global drag` row rendered each exposure **off then on inside one
round**. `MaskCoverageKey::base` is the truncated `Edits` plus the rung, and `gpu_tail` lives in
`FrameSpec`, **not** in `Edits` — so the CPU frame populated the auto-mask coverage cache and the
GPU frame, one line later, read it for free. `cov=0.00` on every accelerated render.

That row is the only thing pricing the work this milestone touches, so it was fixed first: the two
configurations now walk **disjoint exposure ladders** (`+0.003` EV on the `on` frame — same work,
distinct key). Reverting that line is the sabotage; it restores `cov=0.00`.

⚠ **It retracts M8.3k's headline from 3.06–3.20× to 1.78–1.98×.** Recorded in `CLAUDE.md` under
M8.3k. The transferable rule: *when an optimisation measures as exactly free, suspect the harness
before believing the code — free is the signature of work that never ran.*

## Design

### Where it lives

Entirely inside `gpu-pipeline`, plus **one defaulted method** on the existing `CoverageRasterizer`
trait in `pipeline`. Nothing depends on `gpu-pipeline` and that stays true.

### The handoff

A single slot on `GpuContext` (reached through the `Arc` both participants already hold):

```rust
pub(crate) struct SharedPlane {
    slot: Mutex<Option<Deposit>>,
}
struct Deposit {
    img: GpuImage,
    by: std::thread::ThreadId,
}
```

- `GpuBrushRaster::run`, when it uploads a base, allocates a **`GpuImage`** instead of a bare
  buffer, uploads into it through the cached staging pool exactly as now, binds `img.buffer` as
  binding 2, and **deposits** the `GpuImage` after `poll(Wait)` has confirmed the submission
  finished — the same rule that governs when the staging buffer goes back to the pool.
- `GpuChain::run_with`, when it is allowed to (below), **takes** the deposit instead of allocating
  a `GpuImage` and uploading. Take-and-clear: one use.

### The three guards, and why each is needed

1. **The chain must be `stages[0]`.** `MaskStage::apply` clones `image` and pipes it through the
   sub-stages in order, so only the *first* one receives the base bytes. A chain at index 1 gets
   whatever index 0 produced.

   Guarded **statically**: `walk` gains a `share_input: bool` parameter — `false` from
   `accelerate`, `true` from the recursive call inside the `inner` closure. When `flush` pushes a
   chain and `out.is_empty()`, and `share_input`, the chain is marked `may_take_shared`. Nothing
   else can ever be marked.

2. **The deposit must belong to this mask's apply.** A frame whose coverage came from
   `BrushCache`'s prebuilt map deposits nothing; without a clear, the chain would take the
   *previous* frame's plane and render wrong pixels.

   Guarded by clearing: `CoverageRasterizer` gains `fn begin_mask(&self) {}` (defaulted, so the
   trait's other implementors — there are none outside `gpu-pipeline` — are unaffected).
   `MaskStage::apply` calls it once at the very top, before any coverage is built. `GpuBrushRaster`
   implements it by emptying the slot.

3. **Dimensions must match**, and the take must be on the **same thread** that deposited.
   `GpuContext` is a process-wide `shared()`, so a concurrent export render could otherwise collide
   with a preview. A `ThreadId` stamp makes that unrepresentable: `MaskStage::apply` and its
   sub-stage loop run on one call stack, so a legitimate handoff is always same-thread.

### What does not change

- The gate shader, `brush_wgsl`, and every coverage result — the base buffer's contents and binding
  are identical, only its allocation moves.
- `GpuChain`'s in-place mutation of its image. The chain runs strictly after the coverage pass, so
  mutating the shared plane is safe; nothing reads it afterwards.
- The fallback in every direction: no adapter, no gate, a box below `MIN_GPU_BOX_PX`, a chain that
  is not `stages[0]`, a dimension mismatch, a different thread — each falls through to today's
  behaviour at the cost of the check.

## Testing

⚠ **A shared upload changes no pixel any test can read** — the same bytes reach the same buffer by
a different route. This is the M8.3j/M8.3k situation again, so the evidence must be an **event
counter**, never a pixel comparison.

`GpuChain` already has `self.uploads: AtomicUsize`. The tests assert on it:

1. `a_masked_chain_at_index_zero_skips_its_upload` — one mask, auto-masked brush, one replaceable
   sub-stage. Assert the chain's upload count is **0** while the render is byte-identical to the
   unshared one.
2. `a_chain_behind_a_cpu_stage_still_uploads` — same mask with a non-replaceable stage first.
   Assert the count is **1**.
3. `a_cached_coverage_frame_still_uploads` — coverage supplied prebuilt, so nothing deposits.
   Assert **1**, and assert the pixels match a fresh render (this is the guard-2 regression test:
   without `begin_mask` it would render last frame's plane).
4. `a_mismatched_size_is_not_taken` — deposit at one size, chain at another. Assert **1**.
5. Parity: the existing GPU/CPU golden set must not move.

Each guard is sabotage-verified: delete it, watch the named test fail, restore. A sabotage that
does **not** fail gets recorded as a finding about the code rather than counted as a pass.

## What this is not

Not a GPU blend. The mask's `blend=` phase is 9.2–10.4 ms and its `clone=` 2.1, and both could go
if the composite ran on the GPU — but `composite.at` covers analytic shapes, colour ranges and
refinements, so that is its own milestone, not this one.
